// Middleware functions
mod common;

use common::{request, serve};
use cpeak::cpeak;
use serde_json::json;

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.before_each(async |req, res, next| {
            let value = req.query.get("value");

            if value.map(String::as_str) == Some("random") {
                return res.status(400).json(json!({ "error": "an error msg" }));
            }

            next()
        });

        server.before_each(async |req, _res, next| {
            req.set("foo", "text");
            next()
        });

        // Rust can't add new methods to res at runtime like JavaScript can, so this
        // middleware changes the res object itself instead.
        server.before_each(async |_req, res, next| {
            res.status(401).set_header("X-Modified-By", "middleware");
            next()
        });

        server.route("get", "/bar", async |req, res| {
            res.status(200)
                .json(json!({ "message": req.get::<&str>("foo") }))
        });

        server.route("get", "/bar-more", async |_req, res| res.json(json!({})));

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
    assert_eq!(res.header("x-modified-by"), Some("middleware"));
}

#[test]
fn should_exit_the_middleware_and_route_chain_if_a_middleware_wants_to() {
    let res = request(server(), "GET", "/bar?value=random").send();
    assert_eq!(res.status, 400);
    assert_eq!(res.json().get("message"), None);
    assert_eq!(res.json(), json!({ "error": "an error msg" }));
}
