// Parsing request bodies with parse_json
mod common;

use common::{request, serve};
use cpeak::{cpeak, parse_json, ParseJsonOptions};
use serde_json::json;

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.before_each(parse_json(None));

        server.route("post", "/do-something", async |req, res| {
            res.status(205).json(json!({ "receivedData": req.body }))
        });

        server
    })
}

#[test]
fn should_return_the_same_data_that_was_sent_in_request_body_as_json() {
    let obj = json!({
        "key1": "value1",
        "key2": 42,
        "key3": {
            "nestedKey1": "nestedValue1",
            "nestedKey2": ["arrayValue1", "arrayValue2", 1000]
        },
        "key4": true
    });

    let res = request(server(), "POST", "/do-something").json(&obj).send();

    assert_eq!(res.status, 205);
    assert_eq!(res.json()["receivedData"], obj);
}

#[test]
fn should_treat_an_empty_json_body_as_an_empty_object() {
    let res = request(server(), "POST", "/do-something")
        .header("Content-Type", "application/json")
        .send();

    assert_eq!(res.json()["receivedData"], json!({}));
}

#[test]
fn should_accept_json_media_types_with_a_suffix() {
    let res = request(server(), "POST", "/do-something")
        .header("Content-Type", "application/vnd.api+json; charset=utf-8")
        .body(r#"{"type":"articles"}"#)
        .send();

    assert_eq!(res.json()["receivedData"], json!({ "type": "articles" }));
}

#[test]
fn should_skip_bodies_that_are_not_json() {
    let res = request(server(), "POST", "/do-something")
        .header("Content-Type", "text/plain")
        .body("hello")
        .send();

    assert_eq!(res.json()["receivedData"], json!(null));
}

fn server2() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.before_each(parse_json(ParseJsonOptions { limit: 16 }));

        server.route("post", "/limited", async |req, res| {
            res.json(json!({ "receivedData": req.body }))
        });

        server.handle_err(async |error, _req, res| {
            res.status(error.status.unwrap_or(500))
                .json(json!({ "error": error.message, "code": error.code }))
        });

        server
    })
}

#[test]
fn should_reject_bodies_over_the_limit_with_413() {
    let res = request(server2(), "POST", "/limited")
        .json(&json!({ "this body": "is longer than sixteen bytes" }))
        .send();

    assert_eq!(res.status, 413);
    assert_eq!(
        res.json(),
        json!({ "error": "JSON body too large", "code": "CPEAK_ERR_PAYLOAD_TOO_LARGE" })
    );
}

#[test]
fn should_reject_invalid_json_with_400() {
    let res = request(server2(), "POST", "/limited")
        .header("Content-Type", "application/json")
        .body("{nope")
        .send();

    assert_eq!(res.status, 400);
    assert_eq!(
        res.json(),
        json!({ "error": "Invalid JSON format", "code": "CPEAK_ERR_INVALID_JSON" })
    );
}
