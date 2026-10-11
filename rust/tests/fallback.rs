// Fallback handler
mod common;

use common::{request, serve};
use cpeak::{cpeak, CpeakError};
use serde_json::json;

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.route("get", "/health-check", async |_req, res| {
            res.status(200).json(json!({ "matched": "static" }))
        });

        // A GET-scoped wildcard so we can verify fallback only fires for methods the wildcard does not cover
        server.route("get", "*", async |_req, res| {
            res.status(200).json(json!({ "matched": "wildcard" }))
        });

        server.handle_err(async |error, _req, res| {
            res.status(500).json(json!({ "error": error.message }))
        });

        server.fallback(async |req, res| {
            if req.url == "/boom" {
                return Err(CpeakError::other("fallback boom"));
            }
            res.status(418)
                .json(json!({ "matched": "fallback", "method": req.method }))
        });

        server
    })
}

#[test]
fn should_fire_for_unmatched_requests_across_methods() {
    let post = request(server(), "POST", "/anything").send();
    assert_eq!(post.status, 418);
    assert_eq!(
        post.json(),
        json!({ "matched": "fallback", "method": "POST" })
    );

    let del = request(server(), "DELETE", "/x/y/z").send();
    assert_eq!(del.status, 418);
    assert_eq!(
        del.json(),
        json!({ "matched": "fallback", "method": "DELETE" })
    );
}

#[test]
fn should_not_preempt_static_or_wildcard_route_matches() {
    let static_hit = request(server(), "GET", "/health-check").send();
    assert_eq!(static_hit.status, 200);
    assert_eq!(static_hit.json(), json!({ "matched": "static" }));

    let wild_hit = request(server(), "GET", "/anything").send();
    assert_eq!(wild_hit.status, 200);
    assert_eq!(wild_hit.json(), json!({ "matched": "wildcard" }));
}

#[test]
fn should_route_a_thrown_error_to_handle_err() {
    let res = request(server(), "POST", "/boom").send();
    assert_eq!(res.status, 500);
    assert_eq!(res.json(), json!({ "error": "fallback boom" }));
}

#[test]
#[should_panic(expected = "CPEAK_ERR_DUPLICATE_FALLBACK")]
fn should_throw_duplicate_fallback_when_registered_twice() {
    let mut app = cpeak();

    app.fallback(async |_req, res| res.status(404).end(""));
    app.fallback(async |_req, res| res.status(404).end(""));
}
