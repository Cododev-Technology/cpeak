// Serving static files with serve_static
mod common;

use std::collections::HashMap;

use common::{request, serve};
use cpeak::{serve_static, Cpeak, CpeakOptions, ServeStaticOptions};

fn server() -> u16 {
    serve(|| {
        let mut server = Cpeak::new(CpeakOptions {
            mime_types: HashMap::from([("m4a".into(), "audio/mp4".into())]),
            ..Default::default()
        });

        server.before_each(serve_static("./tests/files", None));

        server
    })
}

#[test]
fn should_return_the_correct_file_with_the_correct_mime_type() {
    let text_res = request(server(), "GET", "/test.txt").send();
    let css_res = request(server(), "GET", "/styles.css").send();

    let file_text_content = std::fs::read_to_string("./tests/files/test.txt").unwrap();
    let file_css_content = std::fs::read_to_string("./tests/files/styles.css").unwrap();

    assert_eq!(text_res.status, 200);
    assert_eq!(text_res.header("content-type"), Some("text/plain"));
    assert_eq!(text_res.text(), file_text_content);

    assert_eq!(css_res.status, 200);
    assert_eq!(css_res.header("content-type"), Some("text/css"));
    assert_eq!(css_res.text(), file_css_content);
}

#[test]
fn should_return_the_correct_file_with_the_specified_mime_type_by_the_developer() {
    let res = request(server(), "GET", "/audio.m4a").send();

    // read the file as binary
    let file_buffer = std::fs::read("./tests/files/audio.m4a").unwrap();

    assert_eq!(res.status, 200);
    assert_eq!(res.header("content-type"), Some("audio/mp4"));
    assert_eq!(res.body, file_buffer);
}

#[test]
fn should_serve_files_in_subfolders_and_skip_unknown_paths() {
    let nested = request(server(), "GET", "/partials/nav.html").send();
    assert_eq!(nested.status, 200);
    assert_eq!(nested.header("content-type"), Some("text/html"));

    let missing = request(server(), "GET", "/nope.txt").send();
    assert_eq!(missing.status, 404);
}

// The prefix, live and exclude options
fn server2() -> u16 {
    serve(|| {
        let mut server = Cpeak::new(CpeakOptions::default());

        server.before_each(serve_static(
            "./tests/files",
            ServeStaticOptions {
                prefix: "/static".into(),
                exclude: vec!["partials".into()],
                ..Default::default()
            },
        ));

        server.before_each(serve_static(
            "./target",
            ServeStaticOptions {
                prefix: "/live".into(),
                live: true,
                ..Default::default()
            },
        ));

        server
    })
}

#[test]
fn should_serve_files_under_a_prefix() {
    let prefixed = request(server2(), "GET", "/static/test.txt").send();
    assert_eq!(prefixed.status, 200);
    assert_eq!(prefixed.header("content-type"), Some("text/plain"));

    let unprefixed = request(server2(), "GET", "/test.txt").send();
    assert_eq!(unprefixed.status, 404);
}

#[test]
fn should_not_serve_excluded_folders() {
    let res = request(server2(), "GET", "/static/partials/nav.html").send();
    assert_eq!(res.status, 404);
}

#[test]
fn should_pick_up_new_files_in_live_mode() {
    let port = server2();
    let file_name = format!("cpeak-live-{}.txt", std::process::id());
    let path = format!("./target/{file_name}");

    let before = request(port, "GET", &format!("/live/{file_name}")).send();
    assert_eq!(before.status, 404);

    std::fs::write(&path, "created after startup").unwrap();
    let after = request(port, "GET", &format!("/live/{file_name}")).send();
    std::fs::remove_file(&path).unwrap();

    assert_eq!(after.status, 200);
    assert_eq!(after.text(), "created after startup");
}

#[test]
fn should_not_serve_files_outside_the_folder_in_live_mode() {
    let res = request(server2(), "GET", "/live/../Cargo.toml").send();
    assert_eq!(res.status, 404);

    let encoded = request(server2(), "GET", "/live/%2e%2e/Cargo.toml").send();
    assert_eq!(encoded.status, 404);
}
