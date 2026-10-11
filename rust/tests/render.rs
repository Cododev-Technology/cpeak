// Rendering a template with the render middleware
mod common;

use common::{request, serve, start};
use cpeak::render::MAX_PATTERN;
use cpeak::{cpeak, render, CompressionOptions, Cpeak, CpeakOptions};
use serde_json::json;

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();
        server.before_each(render());

        server.route("get", "/", async |_req, res| {
            res.render(
                "./tests/files/index.html",
                json!({
                    "title": "Home",
                    "body": "Welcome to the Home Page"
                }),
                "text/html",
            )
            .await
        });

        server.route("get", "/inferred", async |_req, res| {
            res.render(
                "./tests/files/index.html",
                json!({
                    "title": "Home",
                    "body": "Welcome to the Home Page"
                }),
                None,
            )
            .await
        });

        server.route("get", "/escape", async |_req, res| {
            res.render(
                "./tests/files/escape.html",
                json!({
                    "content": "<script>alert('xss')</script>",
                    "raw": "<strong>bold</strong>"
                }),
                "text/html",
            )
            .await
        });

        server.route("get", "/missing", async |_req, res| {
            res.render("./tests/files/nope.html", json!({}), None).await
        });

        server.handle_err(async |error, _req, res| {
            res.status(500).json(json!({ "code": error.code }))
        });

        server.route("get", "/include", async |_req, res| {
            res.render(
                "./tests/files/with-include.html",
                json!({
                    "title": "With Include",
                    "navTitle": "My Nav",
                    "body": "Page body"
                }),
                "text/html",
            )
            .await
        });

        server
    })
}

#[test]
fn should_render_the_html_file_with_variables_injected_and_infer_mime_type() {
    let explicit = request(server(), "GET", "/").send();
    let inferred = request(server(), "GET", "/inferred").send();

    assert_eq!(explicit.status, 200);
    assert!(explicit
        .header("content-type")
        .unwrap()
        .starts_with("text/html"));
    assert!(explicit.text().contains("<title>Home</title>"));
    assert!(explicit.text().contains("<p>Welcome to the Home Page</p>"));

    assert_eq!(inferred.status, 200);
    assert!(inferred
        .header("content-type")
        .unwrap()
        .starts_with("text/html"));
    assert!(inferred.text().contains("<title>Home</title>"));
}

#[test]
fn should_escape_html_in_curly_braces_and_output_raw_html_in_cpeak_html_tags() {
    let res = request(server(), "GET", "/escape").send();

    assert_eq!(res.status, 200);
    assert!(res
        .text()
        .contains("&lt;script&gt;alert(&#39;xss&#39;)&lt;/script&gt;"));
    assert!(!res.text().contains("<script>"));
    assert!(res.text().contains("<strong>bold</strong>"));
}

#[test]
fn should_resolve_and_inline_cpeak_include_partials() {
    let res = request(server(), "GET", "/include").send();

    assert_eq!(res.status, 200);
    assert!(res.text().contains("<title>With Include</title>"));
    assert!(res.text().contains("<nav>My Nav</nav>"));
    assert!(res.text().contains("<main>Page body</main>"));
    assert!(!res.text().contains("<cpeak"));
}

#[test]
fn should_interpolate_template_tags_that_straddle_the_safe_tail_chunk_boundary() {
    // Place {{ name }} so it starts 5 bytes before the safe/tail boundary
    // (str.len() - MAX_PATTERN) and ends 5 bytes inside the tail.
    const PAD: usize = 5;
    const TOTAL: usize = MAX_PATTERN + 72;
    let content =
        "A".repeat(TOTAL - MAX_PATTERN - PAD) + "{{ name }}" + &"B".repeat(MAX_PATTERN - PAD);

    let tmp_file = std::env::temp_dir()
        .join(format!("cpeak-render-boundary-{}.html", std::process::id()))
        .to_string_lossy()
        .into_owned();
    std::fs::write(&tmp_file, content).unwrap();

    let file = tmp_file.clone();
    let port = start(move || {
        let mut tmp_server = cpeak();
        tmp_server.before_each(render());
        tmp_server.route("get", "/boundary", async move |_req, res| {
            res.render(&file, json!({ "name": "World" }), "text/html")
                .await
        });
        tmp_server
    });

    let res = request(port, "GET", "/boundary").send();
    let _ = std::fs::remove_file(&tmp_file);

    assert_eq!(res.status, 200);
    assert!(
        !res.text().contains("{{"),
        "raw template syntax should not appear in output"
    );
    assert!(
        res.text().contains("World"),
        "variable should be substituted"
    );
}

#[test]
fn should_re_read_the_file_on_every_request_reflecting_any_changes() {
    let tmp_file = std::env::temp_dir()
        .join(format!("cpeak-render-test-{}.html", std::process::id()))
        .to_string_lossy()
        .into_owned();
    std::fs::write(&tmp_file, "<p>{{ value }}</p>").unwrap();

    let file = tmp_file.clone();
    let port = start(move || {
        let mut tmp_server = cpeak();
        tmp_server.before_each(render());
        tmp_server.route("get", "/live", async move |_req, res| {
            res.render(&file, json!({ "value": "original" }), "text/html")
                .await
        });
        tmp_server
    });

    let first = request(port, "GET", "/live").send();
    assert!(first.text().contains("<p>original</p>"));

    std::fs::write(&tmp_file, "<p>{{ value }}</p><span>updated</span>").unwrap();

    let second = request(port, "GET", "/live").send();
    let _ = std::fs::remove_file(&tmp_file);
    assert!(second.text().contains("<span>updated</span>"));
}

#[test]
fn should_render_big_templates_chunk_by_chunk_and_keep_multi_byte_characters_intact() {
    // Bigger than one 64KB chunk, with "✓" (3 bytes) and tags all over the place
    let content = "<p>{{ word }} ✓ <cpeak html={raw} /></p>\n".repeat(5000);
    let tmp_file = std::env::temp_dir()
        .join(format!("cpeak-render-big-{}.html", std::process::id()))
        .to_string_lossy()
        .into_owned();
    std::fs::write(&tmp_file, &content).unwrap();

    let file = tmp_file.clone();
    let port = start(move || {
        let mut tmp_server = Cpeak::new(CpeakOptions {
            compression: Some(CompressionOptions::default()),
            ..Default::default()
        });
        tmp_server.before_each(render());
        tmp_server.route("get", "/big", async move |_req, res| {
            res.render(&file, json!({ "word": "héllo", "raw": "<b>é</b>" }), None)
                .await
        });
        tmp_server
    });

    // With compression enabled, but the client doesn't ask for it
    let res = request(port, "GET", "/big").send();
    let _ = std::fs::remove_file(&tmp_file);

    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "<p>héllo ✓ <b>é</b></p>\n".repeat(5000));
}

#[test]
fn should_throw_render_fail_when_the_template_is_missing() {
    let res = request(server(), "GET", "/missing").send();
    assert_eq!(res.status, 500);
    assert_eq!(res.json()["code"], "CPEAK_ERR_RENDER_FAIL");
}

#[tokio::test]
async fn should_render_to_a_string_with_render_string() {
    let html = render::string(
        "./tests/files/with-include.html",
        json!({ "title": "String", "navTitle": "Nav", "body": "Body" }),
    )
    .await
    .unwrap();

    assert!(html.contains("<title>String</title>"));
    assert!(html.contains("<nav>Nav</nav>"));
    assert!(html.contains("<main>Body</main>"));
}

#[test]
fn should_throw_render_not_enabled_without_the_render_middleware() {
    let port = start(|| {
        let mut tmp_server = cpeak();
        tmp_server.route("get", "/", async |_req, res| {
            res.render("./tests/files/index.html", json!({}), None)
                .await
        });
        tmp_server.handle_err(async |error, _req, res| {
            res.status(500).json(json!({ "code": error.code }))
        });
        tmp_server
    });

    let res = request(port, "GET", "/").send();
    assert_eq!(res.status, 500);
    assert_eq!(res.json()["code"], "CPEAK_ERR_RENDER_NOT_ENABLED");
}
