// req.method(), req.url(), req.header(), req.headers(), req.as_http_mut(), req.set()/get(),
// and res.send_response()
mod common;

use bytes::Bytes;
use common::{request, serve};
use cpeak::http::{self, StatusCode};
use cpeak::{cookie_parser, cpeak, json, Cpeak};
use std::collections::VecDeque;
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use http_body_util::Full;

#[derive(Clone)]
struct UserId(u32);

#[derive(Clone)]
struct SessionId(String);

fn build() -> Cpeak {
    let mut server = cpeak();
    server.before_each(cookie_parser(None));
    server.before_each(async |req, _res, next| {
        req.set(UserId(7));
        req.set(SessionId("abc".to_string()));
        next()
    });

    server.route("get", "/info", async |req, res| {
        let http = req.as_http_mut();
        let uri = http.uri().to_string();
        let version = format!("{:?}", http.version());
        res.json(json!({
            "method": req.method().as_str(),
            "is_get": req.method() == "GET",
            "url": req.url(),
            "uri": uri,
            "version": version,
            "agent": req.header("User-Agent"),
            "missing": req.header("x-missing"),
            "tags": req
                .headers()
                .get_all("x-tag")
                .iter()
                .map(|value| value.to_str().unwrap())
                .collect::<Vec<_>>(),
            "first_tag": req.header("x-tag"),
        }))
    });

    server.route("get", "/values", async |req, res| {
        if let Some(user) = req.get_mut::<UserId>() {
            user.0 += 1;
        }
        res.json(json!({
            "user": req.get::<UserId>().map(|user| user.0),
            "session": req.get::<SessionId>().map(|session| session.0.clone()),
            "nothing": req.get::<String>().is_none(),
        }))
    });

    server.route("get", "/cookies", async |req, res| res.json(&req.cookies));

    // A response another crate made: its status, headers and body
    server.route("get", "/made", async |_req, res| {
        res.set_header("X-Before", "kept");
        res.set_header("Content-Type", "text/plain");
        let response = http::Response::builder()
            .status(StatusCode::ACCEPTED)
            .header("Content-Type", "application/json")
            .header("Set-Cookie", "a=1")
            .header("Set-Cookie", "b=2")
            .body(Full::new(Bytes::from_static(br#"{"made":true}"#)))
            .unwrap();
        res.send_response(response)
    });

    server.route("get", "/made-stream", async |_req, res| {
        let body = Chunks(VecDeque::from(["one ", "two ", "three"]));
        res.send_response(http::Response::new(body))
    });

    server.route("get", "/made-empty", async |_req, res| {
        let response = http::Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(http_body_util::Empty::<Bytes>::new())
            .unwrap();
        res.send_response(response)
    });

    // The URL as a middleware function changed it
    server.route("get", "/rewritten", async |req, res| {
        *req.as_http_mut().uri_mut() = "/elsewhere?x=1".parse().unwrap();
        res.json(json!({ "url": req.url() }))
    });

    server
}

// A body of another crate that comes in pieces, one at a time
struct Chunks(VecDeque<&'static str>);

impl http_body::Body for Chunks {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, Infallible>>> {
        // One piece per turn, so it goes out as a stream
        cx.waker().wake_by_ref();
        Poll::Ready(
            self.0
                .pop_front()
                .map(|chunk| Ok(http_body::Frame::data(Bytes::from_static(chunk.as_bytes())))),
        )
    }
}

fn port() -> u16 {
    serve(build)
}

#[test]
fn should_give_the_method_url_and_headers() {
    let body = request(port(), "GET", "/info?page=2")
        .header("User-Agent", "tester")
        .header("X-Tag", "one")
        .header("X-Tag", "two")
        .send()
        .json();
    assert_eq!(body["method"], "GET");
    assert_eq!(body["is_get"], true);
    assert_eq!(body["url"], "/info?page=2");
    assert_eq!(body["uri"], "/info?page=2");
    assert_eq!(body["version"], "HTTP/1.1");
    assert_eq!(body["agent"], "tester");
    assert_eq!(body["missing"], serde_json::Value::Null);
    assert_eq!(body["tags"], json!(["one", "two"]));
    assert_eq!(body["first_tag"], "one");
}

#[test]
fn should_keep_values_by_their_type() {
    let body = request(port(), "GET", "/values").send().json();
    assert_eq!(body["user"], 8);
    assert_eq!(body["session"], "abc");
    assert_eq!(body["nothing"], true);
}

#[test]
fn should_read_cookies_sent_in_more_than_one_header() {
    let body = request(port(), "GET", "/cookies")
        .header("Cookie", "a=1; b=2")
        .header("Cookie", "c=3")
        .send()
        .json();
    assert_eq!(body, json!({ "a": "1", "b": "2", "c": "3" }));
}

#[test]
fn should_send_a_response_made_by_another_crate() {
    let response = request(port(), "GET", "/made").send();
    assert_eq!(response.status, 202);
    assert_eq!(response.text(), r#"{"made":true}"#);
    // Its own Content-Type wins over the one set before, the others stay
    assert_eq!(
        response.headers_all("content-type"),
        vec!["application/json"]
    );
    assert_eq!(response.header("x-before"), Some("kept"));
    assert_eq!(response.headers_all("set-cookie"), vec!["a=1", "b=2"]);
}

#[test]
fn should_send_a_streamed_response_made_by_another_crate() {
    let response = request(port(), "GET", "/made-stream").send();
    assert_eq!(response.status, 200);
    assert_eq!(response.text(), "one two three");
}

#[test]
fn should_send_an_empty_response_made_by_another_crate() {
    let response = request(port(), "GET", "/made-empty").send();
    assert_eq!(response.status, 204);
    assert!(response.body.is_empty());
}

#[test]
fn should_give_the_url_a_middleware_function_put_in() {
    let body = request(port(), "GET", "/rewritten").send().json();
    assert_eq!(body["url"], "/elsewhere?x=1");
}
