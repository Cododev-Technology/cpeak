// tower Layers as middleware functions (cpeak::layer) and tower Services as handlers
// (cpeak::service): tower-http, tower_governor, tower-sessions, axum's middleware and Router
mod common;

use std::io::Read as _;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use common::{request, serve};
use cpeak::http::{self, header, HeaderValue, StatusCode};
use cpeak::{cpeak, json, Cpeak, CpeakError, CpeakRequest, CpeakResponse, Next};
use http_body_util::Full;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::GovernorLayer;
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{
    MakeRequestId, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_layer::Layer;
use tower_service::Service;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};

// Hands what it wraps to another thread, which Cpeak doesn't allow, and answers 500 itself
#[derive(Clone)]
struct ToAnotherThread;

impl<S> Layer<S> for ToAnotherThread {
    type Service = ToAnotherThreadService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        ToAnotherThreadService(inner)
    }
}

#[derive(Clone)]
struct ToAnotherThreadService<S>(S);

impl<S, B> Service<http::Request<B>> for ToAnotherThreadService<S>
where
    S: Clone + Send + 'static,
{
    type Response = http::Response<Full<Bytes>>;
    type Error = std::convert::Infallible;
    type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _request: http::Request<B>) -> Self::Future {
        let inner = self.0.clone();
        let moved = std::thread::spawn(move || drop(inner.clone())).join();
        assert!(
            moved.is_err(),
            "using the rest of the request on another thread panics"
        );
        let response = http::Response::builder()
            .status(500)
            .body(Full::new(Bytes::from_static(b"moved")))
            .unwrap();
        std::future::ready(Ok(response))
    }
}

// axum's own way to write a middleware function
async fn axum_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("x-axum-middleware", HeaderValue::from_static("ran"));
    response
}

// Request ids 1, 2, 3...
#[derive(Clone, Default)]
struct Counter(std::sync::Arc<std::sync::atomic::AtomicU64>);

impl MakeRequestId for Counter {
    fn make_request_id<B>(&mut self, _request: &http::Request<B>) -> Option<RequestId> {
        let id = self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        Some(RequestId::new(HeaderValue::from(id)))
    }
}

// Answers 429 by itself, without calling what it wraps
#[derive(Clone)]
struct TooMany;

impl<S> Layer<S> for TooMany {
    type Service = TooManyService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        TooManyService(inner)
    }
}

#[derive(Clone)]
struct TooManyService<S>(S);

impl<S, B> Service<http::Request<B>> for TooManyService<S>
where
    S: Service<http::Request<B>>,
{
    type Response = http::Response<Full<Bytes>>;
    type Error = S::Error;
    type Future = std::future::Ready<Result<Self::Response, S::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        let blocked = request.headers().contains_key("x-block");
        let response = http::Response::builder()
            .status(if blocked { 429 } else { 200 })
            .body(Full::new(Bytes::from_static(b"from the layer")))
            .unwrap();
        std::future::ready(Ok(response))
    }
}

fn wait(ms: u64) -> impl std::future::Future<Output = ()> {
    cpeak::sleep(Duration::from_millis(ms))
}

fn build() -> Cpeak {
    let mut server = cpeak();

    // Before the layers: not covered by the timeout below
    server.before_each(async |req, _res, next| {
        if req.header("x-slow-before").is_some() {
            wait(300).await;
        }
        next()
    });
    server.before_each(cpeak::layer(SetRequestIdLayer::x_request_id(
        Counter::default(),
    )));
    server.before_each(cpeak::layer(PropagateRequestIdLayer::x_request_id()));
    server.before_each(cpeak::layer(SetResponseHeaderLayer::overriding(
        header::SERVER,
        HeaderValue::from_static("cpeak-test"),
    )));
    server.before_each(cpeak::layer(TimeoutLayer::with_status_code(
        StatusCode::REQUEST_TIMEOUT,
        Duration::from_millis(150),
    )));
    // After the timeout layer: covered by it
    server.before_each(async |req, res, next| {
        if req.header("x-slow-after").is_some() {
            wait(300).await;
        }
        res.set_header("X-From-Middleware", "yes");
        next()
    });

    server.route("get", "/hello", async |_req, res| res.end("hello"));

    // A value a tower layer put on the request, read with req.get()
    server.route("get", "/request-id", async |req, res| {
        let id = req
            .get::<RequestId>()
            .and_then(|id| id.header_value().to_str().ok())
            .unwrap_or("none")
            .to_string();
        res.json(json!({ "id": id, "header": req.header("x-request-id") }))
    });

    server.route("get", "/slow", async |_req, res| {
        wait(400).await;
        res.end("too late")
    });

    // A body that's written in pieces, through the layers
    server.route("get", "/stream", async |_req, res| {
        res.set_header("Content-Type", "text/plain");
        for i in 0..5 {
            res.write(format!("part {i}\n")).await?;
            wait(5).await;
        }
        res.end("end\n")
    });

    server.route("get", "/error", async |_req, _res| {
        Err(CpeakError::new(418, "I'm a teapot"))
    });

    // Layers on one route only, in a route_with() tuple
    server.route_with(
        "get",
        "/compressed",
        (
            async |_req: &mut CpeakRequest, res: &mut CpeakResponse, next: Next| {
                res.set_header("X-Route-Middleware", "ran");
                next()
            },
            cpeak::layer(CompressionLayer::new()),
        ),
        async |_req, res| {
            res.set_header("Content-Type", "text/plain");
            res.end("compress me ".repeat(200))
        },
    );
    server.route_with(
        "post",
        "/limited",
        cpeak::layer(RequestBodyLimitLayer::new(16)),
        async |req, res| {
            let mut body = Vec::new();
            while let Some(chunk) = req.chunk().await? {
                body.extend_from_slice(&chunk);
            }
            res.end(body)
        },
    );
    server.route_with(
        "get",
        "/blockable",
        cpeak::layer(TooMany),
        async |_req, res| res.end("from the handler"),
    );

    let rate_limit = GovernorConfigBuilder::default()
        .per_second(60)
        .burst_size(2)
        .finish()
        .unwrap();
    server.route_with(
        "get",
        "/rate-limited",
        cpeak::layer(GovernorLayer::new(rate_limit)),
        async |_req, res| res.end("allowed"),
    );
    server.route_with(
        "get",
        "/visits",
        cpeak::layer(SessionManagerLayer::new(MemoryStore::default()).with_secure(false)),
        async |req, res| {
            let session = req.get::<Session>().cloned().unwrap();
            let visits = session.get::<u32>("visits").await?.unwrap_or(0) + 1;
            session.insert("visits", visits).await?;
            res.json(json!({ "visits": visits }))
        },
    );
    server.route_with(
        "get",
        "/axum-middleware",
        cpeak::layer(axum::middleware::from_fn(axum_middleware)),
        async |_req, res| res.end("handler"),
    );
    // Buffer runs what it wraps in a task of its own (tokio::spawn)
    server.route_with(
        "get",
        "/buffered",
        cpeak::layer(tower::buffer::BufferLayer::new(8)),
        async |_req, res| res.end("buffered"),
    );
    server.route_with(
        "get",
        "/another-thread",
        cpeak::layer(ToAnotherThread),
        async |_req, res| res.end("handler"),
    );

    server.handle_err(async |error, _req, res| {
        res.status(error.status.unwrap_or(500))
            .json(json!({ "error": error.message }))
    });
    server
}

fn port() -> u16 {
    serve(build)
}

#[test]
fn should_run_the_layers_around_a_route() {
    let response = request(port(), "GET", "/hello").send();
    assert_eq!(response.status, 200);
    assert_eq!(response.text(), "hello");
    assert_eq!(response.header("server"), Some("cpeak-test"));
    assert_eq!(response.header("x-from-middleware"), Some("yes"));
    assert!(response.header("x-request-id").is_some());
}

#[test]
fn should_share_the_values_that_a_layer_puts_on_the_request() {
    let response = request(port(), "GET", "/request-id").send();
    let body = response.json();
    assert_ne!(body["id"], "none");
    assert_eq!(body["id"], body["header"]);
    assert_eq!(
        response.header("x-request-id"),
        body["id"].as_str(),
        "PropagateRequestIdLayer copies it to the response"
    );
}

#[test]
fn should_time_out_the_route_and_what_comes_after_the_layer() {
    let response = request(port(), "GET", "/slow").send();
    assert_eq!(response.status, 408);

    let response = request(port(), "GET", "/hello")
        .header("X-Slow-After", "1")
        .send();
    assert_eq!(response.status, 408);
}

#[test]
fn should_not_time_out_what_comes_before_the_layer() {
    let response = request(port(), "GET", "/hello")
        .header("X-Slow-Before", "1")
        .send();
    assert_eq!(response.status, 200);
    assert_eq!(response.text(), "hello");
}

#[test]
fn should_stream_a_body_through_the_layers() {
    let response = request(port(), "GET", "/stream").send();
    assert_eq!(response.status, 200);
    assert_eq!(
        response.text(),
        "part 0\npart 1\npart 2\npart 3\npart 4\nend\n"
    );
    assert_eq!(response.header("server"), Some("cpeak-test"));
}

#[test]
fn should_send_the_errors_of_the_route_through_the_layers() {
    let response = request(port(), "GET", "/error").send();
    assert_eq!(response.status, 418);
    assert_eq!(response.json()["error"], "I'm a teapot");
    assert_eq!(response.header("server"), Some("cpeak-test"));
}

#[test]
fn should_compress_with_a_route_layer() {
    let response = request(port(), "GET", "/compressed")
        .header("Accept-Encoding", "gzip")
        .send();
    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-encoding"), Some("gzip"));
    assert_eq!(response.header("x-route-middleware"), Some("ran"));
    let mut text = String::new();
    flate2::read::GzDecoder::new(&response.body[..])
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "compress me ".repeat(200));
}

#[test]
fn should_limit_the_body_with_a_layer() {
    let response = request(port(), "POST", "/limited").body("small").send();
    assert_eq!(response.status, 200);
    assert_eq!(response.text(), "small");

    let response = request(port(), "POST", "/limited")
        .body("this body is more than 16 bytes")
        .send();
    assert_eq!(response.status, 413);
}

#[test]
fn should_let_a_layer_answer_by_itself() {
    let response = request(port(), "GET", "/blockable")
        .header("X-Block", "1")
        .send();
    assert_eq!(response.status, 429);
    assert_eq!(response.text(), "from the layer");
}

// ---------------------------------------------------------------------------------------
// cpeak::service(): a whole axum Router as a route

fn with_axum() -> Cpeak {
    use axum::extract::Path;
    use axum::routing::{get, post};

    let users = axum::Router::new()
        .route(
            "/api/users/{id}",
            get(async |Path(id): Path<String>| format!("user {id}")),
        )
        .route(
            "/api/users",
            post(async |body: String| (StatusCode::CREATED, format!("created {body}"))),
        );

    let mut server = cpeak();
    server.before_each(async |_req, res, next| {
        res.set_header("X-Cpeak", "before");
        next()
    });
    server.route("get", "/api/*", cpeak::service(users.clone()));
    server.route("post", "/api/*", cpeak::service(users));
    server.route("get", "/own", async |_req, res| res.end("own route"));
    server
}

#[test]
fn should_answer_with_an_axum_router() {
    let port = serve(with_axum);

    let response = request(port, "GET", "/api/users/42").send();
    assert_eq!(response.status, 200);
    assert_eq!(response.text(), "user 42");
    // What the Cpeak middleware set goes out along with axum's response
    assert_eq!(response.header("x-cpeak"), Some("before"));

    let response = request(port, "POST", "/api/users").body("joseph").send();
    assert_eq!(response.status, 201);
    assert_eq!(response.text(), "created joseph");

    // axum's own 404, for a path under /api/ that it doesn't have
    let response = request(port, "GET", "/api/nothing").send();
    assert_eq!(response.status, 404);

    let response = request(port, "GET", "/own").send();
    assert_eq!(response.text(), "own route");
}

#[test]
fn should_rate_limit_with_tower_governor() {
    let port = port();
    assert_eq!(request(port, "GET", "/rate-limited").send().status, 200);
    assert_eq!(request(port, "GET", "/rate-limited").send().status, 200);
    let limited = request(port, "GET", "/rate-limited").send();
    assert_eq!(limited.status, 429);
    assert!(limited.header("retry-after").is_some());
}

#[test]
fn should_keep_sessions_with_tower_sessions() {
    let port = port();
    let first = request(port, "GET", "/visits").send();
    assert_eq!(first.json()["visits"], 1);
    let cookie = first
        .header("set-cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let second = request(port, "GET", "/visits")
        .header("Cookie", &cookie)
        .send();
    assert_eq!(second.json()["visits"], 2);
}

#[test]
fn should_run_axum_middleware() {
    let response = request(port(), "GET", "/axum-middleware").send();
    assert_eq!(response.text(), "handler");
    assert_eq!(response.header("x-axum-middleware"), Some("ran"));
}

#[test]
fn should_refuse_to_run_the_request_on_another_thread() {
    let port = port();
    let response = request(port, "GET", "/another-thread").send();
    assert_eq!(response.status, 500);
    assert_eq!(response.text(), "moved");
    // The server is fine
    assert_eq!(request(port, "GET", "/hello").send().text(), "hello");
}

#[test]
fn should_work_with_a_layer_that_spawns_a_task() {
    let response = request(port(), "GET", "/buffered").send();
    assert_eq!(response.status, 200);
    assert_eq!(response.text(), "buffered");
}
