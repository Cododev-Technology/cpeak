// CORS middleware
mod common;

use common::{request, serve};
use cpeak::{cors, cpeak, CorsOptions, OriginInput};
use regex::Regex;
use serde_json::json;

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.before_each(cors(CorsOptions {
            origin: "http://localhost:3000".into(),
            credentials: true,
            ..Default::default()
        }));

        server.route("get", "/test-cors", async |_req, res| {
            res.status(200)
                .json(json!({ "message": "CORS test successful" }))
        });

        server
    })
}

#[test]
fn should_add_cors_headers_to_the_response() {
    let res = request(server(), "GET", "/test-cors")
        .header("Origin", "http://localhost:3000")
        .send();

    assert_eq!(res.status, 200);
    assert_eq!(
        res.header("access-control-allow-origin"),
        Some("http://localhost:3000")
    );
    assert_eq!(res.header("access-control-allow-credentials"), Some("true"));
}

#[test]
fn should_handle_options_preflight_requests() {
    let res = request(server(), "OPTIONS", "/test-cors")
        .header("Origin", "http://localhost:3000")
        .header("Access-Control-Request-Method", "GET")
        .header(
            "Access-Control-Request-Headers",
            "Content-Type, Authorization",
        )
        .send();

    assert_eq!(res.status, 204);
    assert_eq!(
        res.header("access-control-allow-origin"),
        Some("http://localhost:3000")
    );
    assert_eq!(
        res.header("access-control-allow-methods"),
        Some("GET,HEAD,PUT,PATCH,POST,DELETE")
    );
    assert_eq!(
        res.header("access-control-allow-headers"),
        Some("Content-Type, Authorization")
    );
}

#[test]
fn should_skip_origins_that_are_not_allowed() {
    let res = request(server(), "GET", "/test-cors")
        .header("Origin", "http://evil.example")
        .send();

    assert_eq!(res.status, 200);
    assert_eq!(res.header("access-control-allow-origin"), None);
}

// These tests verify that each CorsOptions field is actually reflected in the
// preflight (OPTIONS) and regular responses
fn server2() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.before_each(cors(CorsOptions {
            origin: "http://localhost:3000".into(),
            methods: ["GET"].join(","),
            allowed_headers: ["X-Custom-Token"].join(","),
            exposed_headers: ["X-Request-Id"].join(","),
            max_age: 30,
            ..Default::default()
        }));

        server.route("get", "/test", async |_req, res| {
            res.set_header("X-Request-Id", "abc123");
            res.status(200).json(json!({}))
        });

        server
    })
}

#[test]
fn preflight_access_control_allow_methods_reflects_the_methods_option() {
    let res = request(server2(), "OPTIONS", "/test")
        .header("Origin", "http://localhost:3000")
        .header("Access-Control-Request-Method", "POST")
        .send();

    assert_eq!(res.status, 204);
    assert_eq!(res.header("access-control-allow-methods"), Some("GET"));
}

#[test]
fn preflight_access_control_allow_headers_reflects_the_allowed_headers_option() {
    let res = request(server2(), "OPTIONS", "/test")
        .header("Origin", "http://localhost:3000")
        .header("Access-Control-Request-Method", "GET")
        .header("Access-Control-Request-Headers", "X-Custom-Token")
        .send();

    assert_eq!(res.status, 204);
    assert_eq!(
        res.header("access-control-allow-headers"),
        Some("X-Custom-Token")
    );
}

#[test]
fn preflight_access_control_max_age_reflects_the_max_age_option() {
    let res = request(server2(), "OPTIONS", "/test")
        .header("Origin", "http://localhost:3000")
        .header("Access-Control-Request-Method", "GET")
        .send();

    assert_eq!(res.status, 204);
    assert_eq!(res.header("access-control-max-age"), Some("30"));
}

#[test]
fn response_access_control_expose_headers_reflects_the_exposed_headers_option() {
    let res = request(server2(), "GET", "/test")
        .header("Origin", "http://localhost:3000")
        .send();

    assert_eq!(res.status, 200);
    assert_eq!(
        res.header("access-control-expose-headers"),
        Some("X-Request-Id")
    );
}

// Every kind of origin rule: "*", a list, a regex, a function, and the defaults
fn server3() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.route("get", "/default", async |_req, res| res.json(json!({})));

        server.route_with(
            "get",
            "/list",
            cors(CorsOptions {
                origin: vec!["https://a.com", "https://b.com"].into(),
                ..Default::default()
            }),
            async |_req, res| res.json(json!({})),
        );

        server.route_with(
            "get",
            "/regex",
            cors(CorsOptions {
                origin: Regex::new(r"\.example\.com$").unwrap().into(),
                ..Default::default()
            }),
            async |_req, res| res.json(json!({})),
        );

        server.route_with(
            "get",
            "/function",
            cors(CorsOptions {
                origin: OriginInput::function(async |origin| origin == "https://fn.com"),
                ..Default::default()
            }),
            async |_req, res| res.json(json!({})),
        );

        server.route_with("options", "/default", cors(None), async |_req, res| {
            res.json(json!({}))
        });

        server
    })
}

fn allow_origin(path: &str, origin: &str) -> Option<String> {
    request(server3(), "GET", path)
        .header("Origin", origin)
        .send()
        .header("access-control-allow-origin")
        .map(str::to_string)
}

#[test]
fn should_support_list_regex_and_function_origins() {
    assert_eq!(
        allow_origin("/list", "https://b.com").as_deref(),
        Some("https://b.com")
    );
    assert_eq!(allow_origin("/list", "https://c.com"), None);

    assert_eq!(
        allow_origin("/regex", "https://api.example.com").as_deref(),
        Some("https://api.example.com")
    );
    assert_eq!(allow_origin("/regex", "https://example.org"), None);

    assert_eq!(
        allow_origin("/function", "https://fn.com").as_deref(),
        Some("https://fn.com")
    );
    assert_eq!(allow_origin("/function", "https://other.com"), None);
}

#[test]
fn should_allow_every_origin_and_echo_the_requested_headers_with_the_defaults() {
    let res = request(server3(), "OPTIONS", "/default")
        .header("Origin", "https://anywhere.com")
        .header("Access-Control-Request-Method", "PUT")
        .header("Access-Control-Request-Headers", "X-Anything")
        .send();

    assert_eq!(res.status, 204);
    assert_eq!(res.header("access-control-allow-origin"), Some("*"));
    assert_eq!(
        res.header("access-control-allow-headers"),
        Some("X-Anything")
    );
    assert_eq!(res.header("access-control-max-age"), Some("86400"));
    assert!(res.body.is_empty());
}
