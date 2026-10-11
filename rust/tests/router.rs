// Routing logic & URL & query parameters
mod common;

use common::{request, serve};
use cpeak::cpeak;
use serde_json::json;

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.route(
            "get",
            "/document/:title/more/:another/final",
            async |req, res| {
                let title = req.params.get("title");
                let another = req.params.get("another");
                let query = &req.query;
                res.status(200)
                    .json(json!({ "title": title, "another": another, "query": query }))
            },
        );

        // Static beats param at the same depth, regardless of registration order.
        server.route("get", "/users/:id", async |req, res| {
            res.status(200)
                .json(json!({ "matched": "param", "id": req.params["id"] }))
        });
        server.route("get", "/users/me", async |_req, res| {
            res.status(200).json(json!({ "matched": "static-me" }))
        });

        // Static beats wildcard at the same depth.
        server.route("get", "/api/*", async |_req, res| {
            res.status(200).json(json!({ "matched": "api-wildcard" }))
        });
        server.route("get", "/api/health", async |_req, res| {
            res.status(200).json(json!({ "matched": "api-health" }))
        });

        // Static is preferred, but the param branch must still match on a dead end.
        server.route("get", "/a/b/c", async |_req, res| {
            res.status(200).json(json!({ "matched": "abc-static" }))
        });
        server.route("get", "/a/:x/d", async |req, res| {
            res.status(200)
                .json(json!({ "matched": "axd-param", "x": req.params["x"] }))
        });

        // Same-position param names across different methods. We have a separate tree per method, so no conflict.
        server.route("post", "/comments/:pageId", async |req, res| {
            res.status(200)
                .json(json!({ "method": "post", "params": req.params }))
        });
        server.route("put", "/comments/:id", async |req, res| {
            res.status(200)
                .json(json!({ "method": "put", "params": req.params }))
        });

        // Same param slot at /lookup/:_, but the leaves diverge so each route gets
        // its own param name on the captured value.
        server.route("get", "/lookup/:userId", async |req, res| {
            res.status(200)
                .json(json!({ "matched": "lookup-user", "params": req.params }))
        });
        server.route("get", "/lookup/:slug/posts", async |req, res| {
            res.status(200)
                .json(json!({ "matched": "lookup-slug-posts", "params": req.params }))
        });

        // A wildcard keeps the params captured before it, and also matches an empty remainder.
        server.route("get", "/files/:owner/*", async |req, res| {
            res.status(200)
                .json(json!({ "matched": "files-wildcard", "params": req.params }))
        });

        // Final fallback for any unmatched GET.
        server.route("get", "*", async |_req, res| {
            res.status(200).json(json!({ "matched": "root-wildcard" }))
        });

        server
    })
}

#[test]
fn should_extract_multiple_url_params_alongside_the_query_string() {
    let tags = json!(["nodejs", "express", "url-params"]).to_string();
    let author = json!({ "name": "John Doe", "id": 123 }).to_string();
    let metadata = json!({ "version": "1.0.0", "language": "en" }).to_string();

    let expected = json!({
        "title": "some-title",
        "another": "thisISsome__more-text",
        "query": {
            "filter": "comments-date",
            "page": "2",
            "sortBy": "date-desc",
            "tags": tags,
            "author": author,
            "isPublished": "true",
            "metadata": metadata
        }
    });

    let res = request(
        server(),
        "GET",
        "/document/some-title/more/thisISsome__more-text/final",
    )
    .query(&[
        ("filter", "comments-date"),
        ("page", "2"),
        ("sortBy", "date-desc"),
        ("tags", &tags),
        ("author", &author),
        ("isPublished", "true"),
        ("metadata", &metadata),
    ])
    .send();

    assert_eq!(res.status, 200);
    assert_eq!(res.json(), expected);
}

#[test]
fn should_pick_static_over_param_at_the_same_depth_fall_to_param_otherwise_and_decode_encoded_values(
) {
    let static_hit = request(server(), "GET", "/users/me").send();
    assert_eq!(static_hit.status, 200);
    assert_eq!(static_hit.json(), json!({ "matched": "static-me" }));

    let param_hit = request(server(), "GET", "/users/42").send();
    assert_eq!(param_hit.status, 200);
    assert_eq!(param_hit.json(), json!({ "matched": "param", "id": "42" }));

    // 'a b/c' encoded becomes 'a%20b%2Fc' which is a single segment since '/' is encoded.
    let encoded = request(server(), "GET", "/users/a%20b%2Fc").send();
    assert_eq!(encoded.status, 200);
    assert_eq!(encoded.json(), json!({ "matched": "param", "id": "a b/c" }));
}

#[test]
fn should_pick_static_over_wildcard_and_fall_to_wildcard_for_unclaimed_paths() {
    let static_hit = request(server(), "GET", "/api/health").send();
    assert_eq!(static_hit.status, 200);
    assert_eq!(static_hit.json(), json!({ "matched": "api-health" }));

    let wild_hit = request(server(), "GET", "/api/v1/users/123").send();
    assert_eq!(wild_hit.status, 200);
    assert_eq!(wild_hit.json(), json!({ "matched": "api-wildcard" }));
}

#[test]
fn should_keep_the_static_only_match_but_backtrack_into_the_param_branch_on_a_dead_end() {
    let exact = request(server(), "GET", "/a/b/c").send();
    assert_eq!(exact.status, 200);
    assert_eq!(exact.json(), json!({ "matched": "abc-static" }));

    let backtracked = request(server(), "GET", "/a/b/d").send();
    assert_eq!(backtracked.status, 200);
    assert_eq!(
        backtracked.json(),
        json!({ "matched": "axd-param", "x": "b" })
    );
}

#[test]
fn should_fall_through_to_the_bare_star_root_wildcard_for_unmatched_gets() {
    let res = request(server(), "GET", "/nothing/registered/here").send();
    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "matched": "root-wildcard" }));
}

#[test]
fn should_return_a_404_with_body_for_a_method_that_has_no_registered_routes() {
    let res = request(server(), "PATCH", "/random").send();
    assert_eq!(res.status, 404);
    assert_eq!(res.json(), json!({ "error": "Cannot PATCH /random" }));
}

#[test]
fn should_allow_different_param_names_at_the_same_position_when_the_paths_diverge() {
    let single = request(server(), "GET", "/lookup/abc").send();
    assert_eq!(single.status, 200);
    assert_eq!(
        single.json(),
        json!({ "matched": "lookup-user", "params": { "userId": "abc" } })
    );

    let deeper = request(server(), "GET", "/lookup/abc/posts").send();
    assert_eq!(deeper.status, 200);
    assert_eq!(
        deeper.json(),
        json!({ "matched": "lookup-slug-posts", "params": { "slug": "abc" } })
    );
}

#[test]
fn should_keep_separate_trees_per_method() {
    let post = request(server(), "POST", "/comments/home").send();
    assert_eq!(
        post.json(),
        json!({ "method": "post", "params": { "pageId": "home" } })
    );

    let put = request(server(), "PUT", "/comments/7").send();
    assert_eq!(
        put.json(),
        json!({ "method": "put", "params": { "id": "7" } })
    );
}

#[test]
fn should_keep_the_params_captured_before_a_wildcard_and_match_an_empty_remainder() {
    let deep = request(server(), "GET", "/files/alice/docs/report.pdf").send();
    assert_eq!(
        deep.json(),
        json!({ "matched": "files-wildcard", "params": { "owner": "alice" } })
    );

    let empty_remainder = request(server(), "GET", "/files/alice").send();
    assert_eq!(
        empty_remainder.json(),
        json!({ "matched": "files-wildcard", "params": { "owner": "alice" } })
    );
}

#[test]
#[should_panic(expected = "CPEAK_ERR_DUPLICATE_ROUTE")]
fn should_throw_duplicate_route_when_a_route_is_registered_twice() {
    let mut server = cpeak();
    server.route("get", "/twice", async |_req, res| res.end(""));
    server.route("GET", "/twice", async |_req, res| res.end(""));
}

#[test]
#[should_panic(expected = "CPEAK_ERR_INVALID_ROUTE")]
fn should_throw_invalid_route_for_named_wildcards() {
    let mut server = cpeak();
    server.route("get", "/files/*name", async |_req, res| res.end(""));
}

#[test]
#[should_panic(expected = "CPEAK_ERR_INVALID_ROUTE")]
fn should_throw_invalid_route_when_the_wildcard_is_not_the_last_segment() {
    let mut server = cpeak();
    server.route("get", "/files/*/more", async |_req, res| res.end(""));
}
