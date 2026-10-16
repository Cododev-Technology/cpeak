// Running a function once the response is sent, with res.on_finish()
mod common;

use std::collections::HashMap;
use std::sync::Mutex;

use common::{request, serve};
use cpeak::{cpeak, json, CpeakError};

// What the on_finish functions of each route have seen so far (by route path)
static FINISHED: Mutex<Option<HashMap<String, Vec<String>>>> = Mutex::new(None);

fn record(path: &str, event: String) {
    FINISHED
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .entry(path.to_string())
        .or_default()
        .push(event);
}

fn recorded(path: &str) -> Vec<String> {
    FINISHED
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|finished| finished.get(path).cloned())
        .unwrap_or_default()
}

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        // Added before the routes run, so it sees the status that they end up sending
        server.before_each(async |req, res, next| {
            let path = req.url.clone();
            res.on_finish(move |res| {
                record(&path, format!("{} {}", res.status_code, res.status_message));
            });
            next()
        });

        server.route("get", "/ok", async |_req, res| {
            res.json(json!({ "message": "done" }))
        });

        server.route("get", "/missing", async |_req, res| {
            res.status(404).json(json!({ "error": "not here" }))
        });

        server.route("get", "/moved", async |_req, res| res.redirect("/ok"));

        server.route("get", "/in-order", async |req, res| {
            let path = req.url.clone();
            res.on_finish(move |_res| record(&path, "second".to_string()));
            let path = req.url.clone();
            res.on_finish(move |_res| record(&path, "third".to_string()));
            res.end("")
        });

        // The handler doesn't end the response, so Cpeak does
        server.route("get", "/left-open", async |_req, res| {
            res.status(204);
            Ok(())
        });

        server.route("get", "/stream", async |req, res| {
            res.set_header("Content-Type", "text/plain");
            res.write("one ").await?;
            assert!(recorded(&req.url).is_empty());
            res.write("two ").await?;
            res.end("three")
        });

        // An error after the headers were sent: the response is destroyed, not finished
        server.route("get", "/destroyed", async |_req, res| {
            res.write("partial").await?;
            Err(CpeakError::other("something went wrong halfway"))
        });

        server.route("get", "/throws", async |_req, _res| {
            Err(CpeakError::new(403, "not allowed"))
        });

        server.route("get", "/listener-panics", async |_req, res| {
            res.on_finish(|_res| panic!("a bug in an on_finish function"));
            res.json(json!({ "message": "still sent" }))
        });

        server.handle_err(async |error, _req, res| {
            res.status(error.status.unwrap_or(500))
                .json(json!({ "error": error.message }))
        });

        server
    })
}

#[test]
fn should_run_the_function_once_the_response_is_sent() {
    let res = request(server(), "GET", "/ok").send();
    assert_eq!(res.status, 200);
    assert_eq!(recorded("/ok"), ["200 OK"]);
}

#[test]
fn should_get_the_status_that_the_response_went_out_with() {
    let res = request(server(), "GET", "/missing").send();
    assert_eq!(res.status, 404);
    assert_eq!(recorded("/missing"), ["404 Not Found"]);

    let res = request(server(), "GET", "/moved").send();
    assert_eq!(res.status, 302);
    assert_eq!(recorded("/moved"), ["302 Found"]);
}

#[test]
fn should_run_the_functions_in_the_order_they_were_added() {
    let res = request(server(), "GET", "/in-order").send();
    assert_eq!(res.status, 200);
    assert_eq!(recorded("/in-order"), ["200 OK", "second", "third"]);
}

#[test]
fn should_run_when_cpeak_ends_the_response_for_the_handler() {
    let res = request(server(), "GET", "/left-open").send();
    assert_eq!(res.status, 204);
    assert_eq!(recorded("/left-open"), ["204 No Content"]);
}

#[test]
fn should_run_once_at_the_end_of_a_streamed_response() {
    let res = request(server(), "GET", "/stream").send();
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "one two three");
    assert_eq!(recorded("/stream"), ["200 OK"]);
}

#[test]
fn should_run_for_a_response_sent_by_handle_err() {
    let res = request(server(), "GET", "/throws").send();
    assert_eq!(res.status, 403);
    assert_eq!(recorded("/throws"), ["403 Forbidden"]);
}

#[test]
fn should_not_run_for_a_response_that_was_destroyed() {
    use std::io::{Read, Write};

    // The connection is cut off halfway, so we read whatever comes back by hand
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", server())).unwrap();
    stream
        .write_all(b"GET /destroyed HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);

    assert!(recorded("/destroyed").is_empty());
}

#[test]
fn should_still_send_the_response_when_a_function_panics() {
    let res = request(server(), "GET", "/listener-panics").send();
    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "message": "still sent" }));
    // The function that was added before the one that panicked has run
    assert_eq!(recorded("/listener-panics"), ["200 OK"]);
}
