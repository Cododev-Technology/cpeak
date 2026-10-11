// Returning files with send_file
mod common;

use common::{request, serve};
use cpeak::cpeak;
use serde_json::json;

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.route("get", "/file", async |_req, res| {
            res.status(200)
                .send_file("./tests/files/test.txt", "text/plain")
                .await
        });

        server.route("get", "/file-inferred", async |_req, res| {
            res.status(200)
                .send_file("./tests/files/test.txt", None)
                .await
        });

        // Just like the Node.js version, HEAD requests need their own route
        server.route("head", "/file", async |_req, res| {
            res.status(200)
                .send_file("./tests/files/test.txt", "text/plain")
                .await
        });

        server.route("get", "/file-missing", async |_req, res| {
            res.send_file("./tests/files/nope.txt", None).await
        });

        server.route("get", "/not-a-file", async |_req, res| {
            res.send_file("./tests/files/partials", "text/html").await
        });

        server.route("get", "/download", async |_req, res| {
            res.attachment("test.txt")
                .send_file("./tests/files/test.txt", None)
                .await
        });

        server.handle_err(async |error, _req, res| {
            res.status(500)
                .json(json!({ "code": error.code, "message": error.message }))
        });

        server
    })
}

#[test]
fn should_get_a_file_as_the_response_with_the_correct_mime_type() {
    let res = request(server(), "GET", "/file").send();

    let file_content = std::fs::read_to_string("./tests/files/test.txt").unwrap();

    assert_eq!(res.status, 200);
    assert_eq!(res.header("content-type"), Some("text/plain"));
    assert_eq!(res.text(), file_content);
}

#[test]
fn should_infer_the_mime_type_from_the_file_extension_when_omitted() {
    let res = request(server(), "GET", "/file-inferred").send();

    let file_content = std::fs::read_to_string("./tests/files/test.txt").unwrap();

    assert_eq!(res.status, 200);
    assert_eq!(res.header("content-type"), Some("text/plain"));
    assert_eq!(res.text(), file_content);
}

#[test]
fn should_answer_head_requests_with_the_headers_only() {
    let res = request(server(), "HEAD", "/file").send();

    let file_size = std::fs::metadata("./tests/files/test.txt").unwrap().len();

    assert_eq!(res.status, 200);
    assert_eq!(res.header("content-type"), Some("text/plain"));
    assert_eq!(
        res.header("content-length"),
        Some(file_size.to_string().as_str())
    );
    assert!(res.body.is_empty());
}

#[test]
fn should_fail_with_file_not_found_for_a_missing_file() {
    let res = request(server(), "GET", "/file-missing").send();
    assert_eq!(res.status, 500);
    assert_eq!(res.json()["code"], "CPEAK_ERR_FILE_NOT_FOUND");
}

#[test]
fn should_fail_with_not_a_file_for_a_folder() {
    let res = request(server(), "GET", "/not-a-file").send();
    assert_eq!(res.status, 500);
    assert_eq!(res.json()["code"], "CPEAK_ERR_NOT_A_FILE");
}

#[test]
fn should_prompt_a_download_with_attachment() {
    let res = request(server(), "GET", "/download").send();
    assert_eq!(res.status, 200);
    assert_eq!(
        res.header("content-disposition"),
        Some("attachment; filename=\"test.txt\"")
    );
}
