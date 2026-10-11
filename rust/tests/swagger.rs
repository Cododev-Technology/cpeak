// Serving the Swagger UI with swagger
mod common;

use common::{request, serve};
use cpeak::{cpeak, serve_static, swagger, ServeStaticOptions};
use serde_json::json;

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        let swagger_document = json!({
            "openapi": "3.0.0",
            "info": { "title": "Test API", "version": "1.0.0" }
        });

        server.before_each(swagger(&swagger_document, None));
        server.before_each(serve_static(
            "./tests/files",
            ServeStaticOptions {
                prefix: "/api-docs".into(),
                ..Default::default()
            },
        ));

        server.before_each(swagger(&swagger_document, Some("/docs")));

        server
    })
}

#[test]
fn should_redirect_the_prefix_to_the_index_page() {
    let res = request(server(), "GET", "/api-docs").send();
    assert_eq!(res.status, 302);
    assert_eq!(res.header("location"), Some("/api-docs/index.html"));

    let with_slash = request(server(), "GET", "/api-docs/").send();
    assert_eq!(with_slash.status, 302);
}

#[test]
fn should_serve_the_spec_as_json() {
    let res = request(server(), "GET", "/api-docs/spec.json").send();
    assert_eq!(res.status, 200);
    assert_eq!(res.json()["info"]["title"], "Test API");
}

#[test]
fn should_serve_the_swagger_initializer_pointing_to_the_spec() {
    let res = request(server(), "GET", "/api-docs/swagger-initializer.js").send();
    assert_eq!(res.status, 200);
    assert_eq!(res.header("content-type"), Some("application/javascript"));
    assert!(res.text().contains("url: \"/api-docs/spec.json\""));
}

#[test]
fn should_let_serve_static_serve_the_swagger_ui_files_under_the_same_prefix() {
    let res = request(server(), "GET", "/api-docs/index.html").send();
    assert_eq!(res.status, 200);
    assert_eq!(res.header("content-type"), Some("text/html"));
}

#[test]
fn should_serve_the_docs_under_a_different_path() {
    let res = request(server(), "GET", "/docs/spec.json").send();
    assert_eq!(res.status, 200);
    assert_eq!(res.json()["openapi"], "3.0.0");
}
