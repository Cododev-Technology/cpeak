// Route middleware functions
mod common;

use common::{request, serve};
use cpeak::{cpeak, CpeakError, CpeakRequest, CpeakResponse, Next};
use serde_json::json;

async fn mid1(
    req: &mut CpeakRequest,
    res: &mut CpeakResponse,
    next: Next,
) -> Result<(), CpeakError> {
    let value = req.query.get("value");

    if value.map(String::as_str) == Some("random") {
        return res.status(400).json(json!({ "error": "an error msg" }));
    }

    next()
}

async fn mid2(
    req: &mut CpeakRequest,
    _res: &mut CpeakResponse,
    next: Next,
) -> Result<(), CpeakError> {
    req.set("foo", "text");
    next()
}

// Rust can't add new methods to res at runtime like JavaScript can, so this
// middleware changes the res object itself instead.
async fn mid3(
    _req: &mut CpeakRequest,
    res: &mut CpeakResponse,
    next: Next,
) -> Result<(), CpeakError> {
    res.status(401);
    next()
}

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.route_with("get", "/bar", (mid1, mid2), async |req, res| {
            res.status(200)
                .json(json!({ "message": req.get::<&str>("foo") }))
        });

        server.route_with("get", "/bar-more", mid3, async |_req, res| {
            res.json(json!({}))
        });

        // Route middleware only run for their own route
        server.route("get", "/plain", async |req, res| {
            res.json(json!({ "foo": req.get::<&str>("foo") }))
        });

        server
    })
}

#[test]
fn should_modify_the_req_object_with_a_new_property() {
    let res = request(server(), "GET", "/bar").send();
    assert_eq!(res.status, 200);
    assert_eq!(res.json()["message"], "text");
}

#[test]
fn should_modify_the_res_object() {
    let res = request(server(), "GET", "/bar-more").send();
    assert_eq!(res.status, 401);
}

#[test]
fn should_exit_the_middleware_and_route_chain_if_a_middleware_wants_to() {
    let res = request(server(), "GET", "/bar?value=random").send();
    assert_eq!(res.status, 400);
    assert_eq!(res.json().get("message"), None);
    assert_eq!(res.json(), json!({ "error": "an error msg" }));
}

#[test]
fn should_not_run_route_middleware_for_other_routes() {
    let res = request(server(), "GET", "/plain").send();
    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "foo": null }));
}
