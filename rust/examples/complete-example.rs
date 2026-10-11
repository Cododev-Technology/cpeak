// Here you can see all the features that Cpeak offers (excluding the authentication features), in one small piece of code.
// Run it with: cargo run --example complete-example
#![allow(unused_variables, clippy::needless_return)]

use std::collections::HashMap;

use cpeak::{
    cookie_parser, cors, json, parse_json, render, serve_static, CookieOptions,
    CookieParserOptions, CorsOptions, Cpeak, CpeakError, CpeakOptions, CpeakRequest, CpeakResponse,
    Next,
};

fn main() {
    let mut server = Cpeak::new(CpeakOptions {
        mime_types: HashMap::from([("mp3".into(), "audio/mpeg".into())]),
        ..Default::default()
    });

    server.before_each(serve_static("./public", None));

    server.before_each(render());

    // For parsing JSON bodies
    server.before_each(parse_json(None));

    // For reading and setting cookies
    server.before_each(cookie_parser(CookieParserOptions {
        secret: Some("your-secret-key-min-32-chars-long".into()),
    }));

    // For enabling CORS
    server.before_each(cors(CorsOptions {
        origin: "http://localhost:3000".into(),
        credentials: true,
        methods: "GET,POST,PUT,DELETE".into(),
        ..Default::default()
    }));

    // Adding custom middleware functions
    server.before_each(async |req, res, next| {
        req.set("custom", "This is some string");
        next()
    });

    // A middleware function that can be specified to run before some particular routes
    let test_route_middleware =
        async |req: &mut CpeakRequest, res: &mut CpeakResponse, next: Next| {
            req.set("whatever", "some calculated value maybe");

            if req.params.get("test").map(String::as_str) != Some("something special") {
                return Err(CpeakError::new(400, "an error message"));
            }

            next()
        };

    // Adding route handlers
    server.route("get", "/", async |req, res| {
        return res
            .render(
                "./public/index.html",
                json!({
                    "test": "some testing value",
                    "number": "2343242"
                }),
                "text/html",
            )
            .await;
    });

    server.route_with(
        "get",
        "/old-url",
        test_route_middleware,
        async |req, res| {
            return res.redirect("/new-url");
        },
    );

    server.route_with(
        "get",
        "/api/document/:title",
        test_route_middleware,
        async |req, res| {
            // Reading URL variables (route parameters)
            let title = &req.params["title"];

            // Reading URL parameters (query strings) (like /users?filter=active)
            let filter = req.query.get("filter");

            // Reading JSON request body
            let anything = &req.body["anything"];

            // Handling errors
            if anything == "not-expected-thing" {
                return Err(CpeakError::new(400, "Invalid property."));
            }

            // Sending a JSON response
            return res
                .status(200)
                .json(json!({ "message": "This is a test response" }));
        },
    );

    // Reading and setting cookies
    server.route("post", "/login", async |req, res| {
        // Reads are available via req.cookies and req.signed_cookies
        let session_id = req.signed_cookies.get("sessionId");

        // Set a signed session cookie
        res.cookie(
            "sessionId",
            "abc123",
            CookieOptions {
                signed: true,
                http_only: true,
                secure: true,
                ..Default::default()
            },
        );
        return res.status(200).json(json!({ "message": "Logged in" }));
    });

    // Sending a file response
    server.route("get", "/file", async |req, res| {
        // Make sure to specify a correct path and MIME type...
        return res
            .status(200)
            .send_file("./public/file.txt", "text/plain")
            .await;
    });

    // Handle all the errors that could happen in the routes
    server.handle_err(async |error, req, res| {
        if let Some(status) = error.status {
            return res.status(status).json(json!({ "error": error.message }));
        } else {
            eprintln!("{error:?}");
            return res.status(500).json(json!({
                "error": "Sorry, something unexpected happened from our side."
            }));
        }
    });

    server
        .listen(3000, || {
            println!("Server has started on port 3000");
        })
        .wait();
}
