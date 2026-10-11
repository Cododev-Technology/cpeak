// Error handling with handle_err
mod common;

use common::{request, serve};
use cpeak::{cpeak, CpeakError, CpeakRequest, CpeakResponse, Next};
use serde_json::json;

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        let mid1 = async |req: &mut CpeakRequest, _res: &mut CpeakResponse, next: Next| {
            let value = req.query.get("value");

            if value.map(String::as_str) == Some("random") {
                return Err(CpeakError::new(401, "another error msg"));
            }

            next()
        };

        server.route_with("patch", "/foo/:bar", mid1, async |req, res| {
            let bar = &req.params["bar"];

            if bar == "random" {
                return Err(CpeakError::new(403, "an error msg"));
            }

            res.status(200).json(json!({ "bar": bar }))
        });

        // Returning the result of an async response method lets the
        // framework route the error to handle_err.
        server.route("get", "/sendfile-returned", async |_req, res| {
            res.send_file("./tests/files/test.unknownext", None).await
        });

        server.handle_err(async |error, req, res| {
            // Mimics res.json failing inside handle_err (a failure while compressing, the socket closed mid-stream, etc.)
            if req.get::<bool>("broken_json").is_some() {
                return Err(CpeakError::other("simulated json rejection"));
            }
            if let Some(code) = error.code {
                return res.status(500).json(json!({ "code": code }));
            }
            res.status(error.status.unwrap_or(500))
                .json(json!({ "error": error.message }))
        });

        server.route("get", "/handle-err-broken-json", async |req, _res| {
            req.set("broken_json", true);
            Err(CpeakError::new(500, "trigger handleErr"))
        });

        // In Rust, a panic is the closest thing to an unexpected thrown error
        server.route("get", "/panic", async |req, _res| {
            let user: Option<&String> = req.headers.get("x-missing-user");
            let _name = user.expect("something unexpected");
            Ok(())
        });

        // Any error works with the `?` operator, and arrives in handle_err without a status
        server.route("get", "/question-mark", async |_req, res| {
            let number: u16 = "not a number".parse()?;
            res.json(json!({ "number": number }))
        });

        server
    })
}

#[test]
fn should_route_a_thrown_error_from_a_handler_to_handle_err() {
    let res = request(server(), "PATCH", "/foo/random").send();
    assert_eq!(res.status, 403);
    assert_eq!(res.json(), json!({ "error": "an error msg" }));
}

#[test]
fn should_route_a_thrown_error_from_a_middleware_to_handle_err() {
    let res = request(server(), "PATCH", "/foo/random?value=random").send();
    assert_eq!(res.status, 401);
    assert_eq!(res.json(), json!({ "error": "another error msg" }));
}

#[test]
fn should_funnel_async_errors_from_a_returned_res_send_file_to_handle_err() {
    let res = request(server(), "GET", "/sendfile-returned").send();
    assert_eq!(res.status, 500);
    assert_eq!(res.json()["code"], "CPEAK_ERR_MISSING_MIME");
}

#[test]
fn should_still_respond_with_500_when_res_json_inside_handle_err_fails() {
    let res = request(server(), "GET", "/handle-err-broken-json").send();
    assert_eq!(res.status, 500);
}

#[test]
fn should_route_a_panic_to_handle_err() {
    let res = request(server(), "GET", "/panic").send();
    assert_eq!(res.status, 500);
    assert_eq!(res.json(), json!({ "error": "something unexpected" }));
}

#[test]
fn should_route_errors_from_the_question_mark_operator_to_handle_err() {
    let res = request(server(), "GET", "/question-mark").send();
    assert_eq!(res.status, 500);
    assert_eq!(
        res.json(),
        json!({ "error": "invalid digit found in string" })
    );
}

#[test]
fn should_close_the_connection_after_an_error() {
    let res = request(server(), "PATCH", "/foo/random").send();
    assert_eq!(res.header("connection"), Some("close"));
}
