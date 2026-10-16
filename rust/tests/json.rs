// The JSON type (cpeak::Value), turning it into a struct, and the order of the keys
mod common;

use common::{request, serve};
use cpeak::{cpeak, from_value, json, parse_json, CpeakError, Value};
use serde::Deserialize;

#[derive(Deserialize)]
struct User {
    name: String,
    age: u8,
}

// A function of an app can take and give back JSON without the app adding serde_json itself
fn describe(body: &Value) -> Value {
    json!({ "kind": if body.is_object() { "object" } else { "something else" } })
}

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.before_each(parse_json(None));

        server.route("post", "/describe", async |req, res| {
            res.json(describe(&req.body))
        });

        server.route("post", "/user", async |req, res| {
            let user: User = from_value(req.body.clone())?;
            res.json(json!({ "greeting": format!("Hi {}, {} years old!", user.name, user.age) }))
        });

        server.route("get", "/order", async |_req, res| {
            res.json(json!({ "zebra": 1, "apple": 2, "mango": { "b": true, "a": false } }))
        });

        server.route("post", "/echo", async |req, res| res.json(&req.body));

        server.handle_err(async |error: CpeakError, _req, res| {
            res.status(400).json(json!({ "error": error.message }))
        });

        server
    })
}

#[test]
fn should_let_an_app_use_the_json_type_without_serde_json() {
    let res = request(server(), "POST", "/describe")
        .json(&json!({ "any": "thing" }))
        .send();
    assert_eq!(res.json(), json!({ "kind": "object" }));

    let res = request(server(), "POST", "/describe")
        .json(&json!([1, 2, 3]))
        .send();
    assert_eq!(res.json(), json!({ "kind": "something else" }));
}

#[test]
fn should_turn_a_body_into_a_struct() {
    let res = request(server(), "POST", "/user")
        .json(&json!({ "name": "Joe", "age": 30 }))
        .send();

    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "greeting": "Hi Joe, 30 years old!" }));
}

#[test]
fn should_fail_for_a_body_that_does_not_fit_the_struct() {
    let res = request(server(), "POST", "/user")
        .json(&json!({ "name": "Joe" }))
        .send();

    assert_eq!(res.status, 400);
    assert_eq!(res.json(), json!({ "error": "missing field `age`" }));
}

// Like JavaScript, with the preserve_order feature (on by default)
#[cfg(feature = "preserve_order")]
#[test]
fn should_send_the_keys_of_an_object_in_the_order_they_were_added() {
    let res = request(server(), "GET", "/order").send();

    assert_eq!(
        res.text(),
        r#"{"zebra":1,"apple":2,"mango":{"b":true,"a":false}}"#
    );
}

#[cfg(feature = "preserve_order")]
#[test]
fn should_keep_the_keys_of_a_body_in_the_order_the_client_sent_them() {
    let body = r#"{"zebra":1,"apple":2,"mango":{"b":true,"a":false}}"#;
    let res = request(server(), "POST", "/echo")
        .header("Content-Type", "application/json")
        .body(body)
        .send();

    assert_eq!(res.text(), body);
}

// Without the feature, serde_json sorts the keys by name
#[cfg(not(feature = "preserve_order"))]
#[test]
fn should_sort_the_keys_of_an_object_without_preserve_order() {
    let res = request(server(), "GET", "/order").send();

    assert_eq!(
        res.text(),
        r#"{"apple":2,"mango":{"a":false,"b":true},"zebra":1}"#
    );
}
