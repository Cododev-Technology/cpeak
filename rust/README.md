# Cpeak

[![crates.io](https://img.shields.io/crates/v/cpeak.svg)](https://crates.io/crates/cpeak)
[![HTTP Arena H/1.1](https://img.shields.io/endpoint?url=https://www.http-arena.com/badge/cpeak/h1.json)](https://www.http-arena.com/#type=emerging,flagship&tuned=1)

Cpeak is a minimal and fast Rust framework. As of October 2026, it's ranked #1 of 95 frameworks on the [HttpArena](https://www.http-arena.com/) HTTP/1.1 leaderboard, surpassing Drogon (C++), Vert.x (Java), Bun (TS), Actix Web (Rust) and others. It's the Rust port of the [Cpeak Node.js framework](https://github.com/Cododev-Technology/cpeak/tree/main/node), with the same API, the same names and the same concepts. You would use Cpeak Node.js if you want maximum development speed, and Cpeak for Rust if you want the best performance, control and memory usage over your application.

Cpeak gives you routing, middleware functions, JSON parsing, cookie parsing, CORS, compression, authentication and more. Built on top of [tokio](https://tokio.rs) and [hyper](https://hyper.rs). If you know Express.js or the Node.js version of Cpeak, you already know how to use it.

Cpeak for Rust has all the features of the Node.js version of Cpeak, and it uses every CPU core of your machine out of the box. Additionally, it has a codebase that you can easily navigate, audit and change if you want. It mirrors the Node.js codebase file by file, with the same names and the same comments. On HttpArena's 64-core machine it handles 2.59 million plain requests per second, and 1.28 million JSON responses per second over TLS. You can see all the results [here](https://www.http-arena.com/frameworks/cpeak/). _Videos on navigating and understanding the codebase will be added soon._

Cpeak started as an educational project, and we now run our own products on it. At Cododev we build real products, get real users, and then use them to offer a unique kind of education for the age of AI. To see how we teach, and some of the products built with Cpeak, visit [cododev.ca](https://www.cododev.ca).

## Why Cpeak?

- **Minimalism**: No unnecessary bloat, with just a handful of well-known dependencies. Just the core essentials you need to build fast and reliable applications.
- **Performance**: Engineered to be fast, **Cpeak** won’t sacrifice speed for excessive customizability.
- **Express.js Compatible**: You can easily refactor from Express.js or the Node.js version of Cpeak to Cpeak for Rust and vice versa. The functions, the options and the way they work are the same, just written the Rust way.

## Table of Contents

- [Getting Started](#getting-started)
  - [Hello World App](#hello-world-app)
- [Documentation](#documentation)
  - [Including](#including)
  - [Initializing](#initializing)
  - [Middleware](#middleware)
  - [Route Handling](#route-handling)
  - [Route Middleware](#route-middleware)
  - [Fallback Handler](#fallback-handler)
  - [URL Variables & Parameters](#url-variables--parameters)
  - [Reading the Request Body](#reading-the-request-body)
  - [Sending a Response](#sending-a-response)
  - [After the Response Is Sent](#after-the-response-is-sent)
  - [Sending Files](#sending-files)
  - [Redirecting](#redirecting)
  - [MIME Types](#mime-types)
  - [Compression](#compression)
  - [Error Handling](#error-handling)
  - [Waiting](#waiting)
  - [Listening](#listening)
  - [HTTPS](#https)
  - [Util Functions](#util-functions)
    - [serve_static](#serve_static)
    - [parse_json](#parse_json)
    - [render](#render)
    - [cookie_parser](#cookie_parser)
    - [swagger](#swagger)
    - [auth](#auth)
    - [cors](#cors)
- [Complete Example](#complete-example)
- [Versioning Notice](#versioning-notice)

## Getting Started

Ready to dive in? Install **Cpeak** via cargo:

```bash
cargo add cpeak
```

That's it, no async runtime or JSON crate to set up. Cpeak brings its own event loops, and gives you the `json!` macro. Your routes and middleware functions are async closures, so Cpeak needs Rust 1.85 or newer.

For the best performance, Cpeak makes [mimalloc](https://github.com/microsoft/mimalloc) the memory allocator of your program. A program can only have one, so if your app sets its own `#[global_allocator]`, install Cpeak without it:

```bash
cargo add cpeak --no-default-features
```

### Hello World App:

```rust
use cpeak::{cpeak, json};

fn main() {
    let mut server = cpeak();

    server.route("get", "/", async |_req, res| {
        return res.json(json!({ "message": "Hi there!" }));
    });

    server
        .listen(3000, || {
            println!("Server has started on port 3000");
        })
        .wait();
}
```

## Documentation

### Including

Include the framework like this:

```rust
use cpeak::cpeak;
```

Because of the minimalistic philosophy, you won’t add unnecessary code to your app as soon as you include the framework. If at any point you want to use a particular utility function (like `parse_json` and `serve_static`), include it like the line below. The ones you don't use are left out of your compiled program:

```rust
use cpeak::{cpeak, parse_json, serve_static};
```

### Initializing

Initialize the Cpeak server like this:

```rust
let mut server = cpeak();
```

Now you can use this server object to start listening, add route logic, add middleware functions, and handle errors.

If you want to pass options to the server (like the ones in [MIME Types](#mime-types) and [Compression](#compression)), initialize it with `Cpeak::new()` instead:

```rust
let mut server = Cpeak::new(CpeakOptions {
    // your options
    ..Default::default()
});
```

### Middleware

If you add a middleware function, that function will run before your route logic kicks in. Here you can customize the request object, return an error, or do anything else you want to do prior to your route logic, like authentication.

After calling `next`, the next middleware function is going to run if there’s any; otherwise, the route logic is going to run. `next()` returns `Ok(())`, so you can end your middleware with it. If a middleware function returns without calling `next`, nothing after it runs, so make sure to send a response in that case.

```rust
server.before_each(async |req, res, next| {
    if req.headers.contains_key("authentication") {
        // Your authentication logic...
        req.set("userId", "<something>");
        req.set("custom", "This is some string");
        next()
    } else {
        // Return an error and close the request...
        return res.status(401).json(json!({ "error": "Unauthorized" }));
    }
});

server.before_each(async |req, res, next| {
    println!(
        "The custom value was added from the previous middleware: {}",
        req.get::<&str>("custom").unwrap()
    );
    next()
});
```

In Rust, you can't add new properties to an object at runtime, so to attach your own values to the request, use `req.set("name", value)`, and read them later with `req.get::<Type>("name")`. The value can be of any type.

### Route Middleware

You can also add middleware functions for a particular route handler. Rust doesn't have variadic functions, so routes with middleware use `server.route_with()`, which takes the middleware as the third argument:

```rust
let require_auth = async |req: &mut CpeakRequest, res: &mut CpeakResponse, next: Next| {
    // Check if user is logged in, if so then:
    if req.headers.contains_key("authorization") {
        req.set("test", "this is a test value");
        return next();
    }

    // If user is not logged in:
    return Err(CpeakError::new(401, "Unauthorized"));
};

server.route_with("get", "/profile", require_auth, async |req, res| {
    println!("{}", req.get::<&str>("test").unwrap()); // this is a test value
    Ok(())
});
```

You can add as many middleware functions as you want for a route, by putting them in a tuple:

```rust
server.route_with(
    "get",
    "/profile",
    (require_auth, another_function, one_more),
    async |req, res| {
        // your logic
        Ok(())
    },
);
```

Your middleware functions can also be regular async functions, like `async fn require_auth(req: &mut CpeakRequest, res: &mut CpeakResponse, next: Next) -> Result<(), CpeakError>`.

### Route Handling

You can add new routes like this:

```rust
server.route("patch", "/the-path-you-want", async |req, res| {
    // your route logic
    Ok(())
});
```

First add the HTTP method name you want to handle, then the path, and finally, the callback. The callback is an async closure that gets `req` (a `&mut CpeakRequest`) and `res` (a `&mut CpeakResponse`), and returns a `Result<(), CpeakError>`. The response methods like `res.json()` return that same type, so you can just return them, exactly like you'd do in the Node.js version. Your callbacks are shared by the worker threads, so anything they capture needs to be thread-safe, like an `Arc<Mutex<...>>` or a database pool.

Under the hood, Cpeak stores your routes in a radix tree, so finding the right route for an incoming request is roughly O(log n) in your total route count. In plain terms, that means matching stays fast even when your app has hundreds or thousands of routes. You won't pay a linear scan per request as your route table grows.

### Fallback Handler

If no route or middleware matches an incoming request, Cpeak returns a default `404` message. You can replace it with your own handler using `server.fallback`:

```rust
server.fallback(async |req, res| {
    return res.status(404).json(json!({ "error": "not found" }));
});
```

Use this for custom 404 pages, JSON error envelopes, or serving an SPA's `index.html` for unknown paths. Static, param, and `*` wildcard routes still win when they match, the fallback only fires once the router has no match.

### URL Variables & Parameters

To be consistent with the Node.js version and the broader Node.js community and frameworks, we call the HTTP URL parameters (query strings) '**query**', and the path variables (route parameters) '**params**'.

Here’s how we can read both:

```rust
// Imagine request URL is example.com/test/my-title/more-text?filter=newest
server.route("patch", "/test/:title/more-text", async |req, res| {
    let title = &req.params["title"];
    let filter = req.query.get("filter");

    println!("{title}"); // my-title
    println!("{filter:?}"); // Some("newest")
    Ok(())
});
```

Both `req.params` and `req.query` are a `HashMap<String, String>`, so use `.get()` when a value might be missing.

### Reading the Request Body

A JSON body is read and parsed for you by [parse_json](#parse_json). For any other body, like plain text or a file upload, use `req.chunk()`. It's what `req.on("data", ...)` is in Node.js: every call gives you the next chunk of the body, and `None` once the whole body has been read.

```rust
server.route("post", "/upload", async |req, res| {
    let mut size = 0;

    while let Some(chunk) = req.chunk().await? {
        // Do something with this chunk: write it to a file, hash it, pass it on...
        size += chunk.len();
    }

    return res.json(json!({ "received": size }));
});
```

The body is read from the client as you ask for it, so an upload of any size takes up very little memory.

If you want the whole body in one piece, collect the chunks:

```rust
server.route("post", "/notes", async |req, res| {
    let mut body = Vec::new();
    while let Some(chunk) = req.chunk().await? {
        body.extend_from_slice(&chunk);
    }
    let note = String::from_utf8_lossy(&body);

    // rest of your logic...
    return res.status(201).json(json!({ "length": note.len() }));
});
```

### Sending a Response

For a response that isn't JSON or a file, set your headers and finish with `res.end()`. They are what `res.setHeader()` and `res.end()` are in Node.js:

```rust
server.route("get", "/hello", async |req, res| {
    return res
        .set_header("Content-Type", "text/plain")
        .end("Hello there!");
});
```

`res.end()` takes the body (a `String`, a `Vec<u8>`, a string literal, ...), or `""` for none. `res.set_header()` replaces a header that's already set. To add another one with the same name (like a second `Set-Cookie`), use `res.append_header()`.

To send the body in pieces, use `res.write()`. The status and the headers go out with the first piece:

```rust
use std::time::Duration;

server.route("get", "/countdown", async |req, res| {
    res.set_header("Content-Type", "text/plain");

    for number in (1..=3).rev() {
        res.write(format!("{number}\n")).await?;
        cpeak::sleep(Duration::from_secs(1)).await;
    }

    return res.end("Liftoff!\n");
});
```

### After the Response Is Sent

To run some code once a response has been sent (to log the request, for example), use `res.on_finish()`. It's what `res.on("finish", ...)` is in Node.js:

```rust
use std::time::Instant;

server.before_each(async |req, res, next| {
    let started = Instant::now();
    let method = req.method.clone();
    let url = req.url.clone();

    res.on_finish(move |res| {
        println!(
            "{method} {url} {} {} -- {} ms",
            res.status_code,
            res.status_message,
            started.elapsed().as_millis()
        );
    });

    next()
});
```

Your function gets the status code and the status message that the response went out with (`res.statusCode` and `res.statusMessage` in Node.js), so add it before the response ends, like in a middleware function. You can add as many as you want, and they run in the order you added them. A response that is cut off instead of finished (an error after its headers were sent) doesn't run them.

### Sending Files

You can send a file as a stream anywhere in your route or middleware logic like this:

```rust
server.route("get", "/testing", async |req, res| {
    return res.status(200).send_file("<file-path>", "<mime-type>").await;

    // Example:
    // return res.status(200).send_file("./images/sun.jpeg", "image/jpeg").await;
});
```

The file’s binary content will be in the HTTP response body content, sent to the client chunk by chunk, so even big files don't take up much memory. Make sure you specify a correct path relative to your CWD (you can build it with `std::path` for better compatibility).

The MIME type argument is optional. When omitted (by passing `None`), Cpeak infers it from the file extension using its built-in MIME registry (see [MIME Types](#mime-types)):

```rust
return res.status(200).send_file("./images/sun.jpeg", None).await;
```

Passing the MIME type explicitly is the fastest path, Cpeak skips the extension lookup entirely.

Files up to 1MB are kept in memory, and they're checked on the disk again when they haven't been in the last tenth of a second. So when you change a file, the new version is sent within a tenth of a second. This goes for `serve_static` and `res.render` too.

Bigger files, and anything you send with `res.write`, are streamed with backpressure: the server only reads ahead about half a megabyte per client. Sending a 1GB file takes a few MB of memory, however slow the client is.

### Redirecting

If you want to redirect to a new URL, you can simply do:

```rust
return res.redirect("https://whatever.com");
```

### MIME Types

`serve_static`, `res.send_file`, and `res.render` all share a single MIME type registry. Out of the box it covers the common web types:

```
  html: "text/html",
  css: "text/css",
  js: "application/javascript",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  png: "image/png",
  svg: "image/svg+xml",
  gif: "image/gif",
  ico: "image/x-icon",
  webp: "image/webp",
  avif: "image/avif",
  txt: "text/plain",
  json: "application/json",
  map: "application/json",
  xml: "application/xml",
  pdf: "application/pdf",
  webmanifest: "application/manifest+json",
  mp4: "video/mp4",
  webm: "video/webm",
  mp3: "audio/mpeg",
  wav: "audio/wav",
  eot: "application/vnd.ms-fontobject",
  otf: "font/otf",
  ttf: "font/ttf",
  woff: "font/woff",
  woff2: "font/woff2"
```

If you need to serve something else, register it once on the `Cpeak::new()` constructor:

```rust
let mut server = Cpeak::new(CpeakOptions {
    mime_types: HashMap::from([
        ("mp3".into(), "audio/mpeg".into()),
        ("m4a".into(), "audio/mp4".into()),
    ]),
    ..Default::default()
});
```

We keep this list small on purpose. Cpeak isn't going to load the [thousands of MIME types from the IANA registry](https://www.iana.org/assignments/media-types/media-types.xhtml) into your process memory on the off chance you might serve an `.apk` or a `.fla` one day. You tell us what you serve, we keep memory tight.

### Compression

You can enable HTTP response compression at construction time. Once enabled, `serve_static`, `res.json()`, `res.send_file()` and `res.render()` will compress eligible responses automatically, and you also get a `res.compress()` method on the response for custom payloads. Cpeak supports brotli, gzip and deflate, and picks the best one that the client accepts. That's brotli, except for a body under 64KB that is already in memory (like what `res.json()` sends): there brotli takes two to three times as long as gzip to save a few percent, so that one goes out as gzip when the client accepts it.

Fire it up with the defaults like this:

```rust
let mut server = Cpeak::new(CpeakOptions {
    compression: Some(CompressionOptions::default()),
    ..Default::default()
});
```

Or pass options to tune the behavior:

```rust
let mut server = Cpeak::new(CpeakOptions {
    compression: Some(CompressionOptions {
        threshold: 1024, // bytes — responses smaller than this are sent uncompressed. Default: 1024
        brotli: BrotliOptions { quality: Some(4), lgwin: Some(19) }, // quality from 0 to 11. Default: 4
        gzip: ZlibOptions { level: Some(6) },    // level from 0 to 9. Default: 6
        deflate: ZlibOptions { level: Some(6) }, // level from 0 to 9. Default: 6
    }),
    ..Default::default()
});
```

For arbitrary payloads, like a `Vec<u8>`, `String`, or `Readable` stream, use `res.compress`:

```rust
server.route("get", "/report", async |req, res| {
    let csv = build_csv_report().await;
    return res.compress("text/csv", csv, None).await;
});
```

When you're streaming, you can pass a known size as the third argument. Cpeak will use it to decide eligibility against `threshold`, and to set `Content-Length` if the body ends up being sent uncompressed. A `Readable` wraps anything that implements tokio's `AsyncRead`. This example uses the reqwest, tokio-util and futures-util crates:

```rust
use cpeak::Readable;
use futures_util::TryStreamExt;
use tokio_util::io::StreamReader;

server.route("get", "/proxy/feed", async |req, res| {
    let upstream = reqwest::get("https://example.com/feed.xml").await?;
    let size = upstream.content_length();
    let body = StreamReader::new(upstream.bytes_stream().map_err(std::io::Error::other));
    return res.compress("application/xml", Readable::new(body), size).await;
});
```

You must first enable compression at construction time to use `res.compress`.

### Error Handling

If anywhere in your route functions or route middleware functions you want to return an error, you can just return the error and let the automatic error handler catch it:

```rust
server.route("get", "/api/document/:title", async |req, res| {
    let title = &req.params["title"];

    if title.len() > 500 {
        return Err(CpeakError::new(400, "Title too long."));
    }

    // The rest of your logic...
    Ok(())
});
```

**Make sure** to call `server.handle_err` and pass a function like this to have the automatic error handler work properly:

```rust
server.handle_err(async |error, req, res| {
    if let Some(status) = error.status {
        return res.status(status).json(json!({ "error": error.message }));
    } else {
        // Log the unexpected errors somewhere so you can keep track of them...
        eprintln!("{error:?}");
        return res.status(500).json(json!({
            "error": "Sorry, something unexpected happened on our side."
        }));
    }
});
```

_The error object is the object that you returned earlier in your routes or middleware._

The `?` operator works too. Any error you pass up with `?` (a database error, an IO error, ...) arrives in `handle_err` without a `status`, so it's treated as an unexpected error. And if one of your functions panics, Cpeak catches the panic and hands it to `handle_err` the same way, so a bug in one route never takes your server down.

One thing to keep in mind: `res.send_file`, `res.render`, `res.compress`, and `res.json` all return a `Result`. **Return** that result (or use `?` on it) so the framework can route any error to your `handle_err`. The first three are async, so make sure to `.await` them:

```rust
server.route("get", "/file", async |req, res| {
    return res.send_file("./images/sun.jpeg", "image/jpeg").await;
});
```

Or, in a handler that does other work alongside the send:

```rust
server.route("get", "/avatars/:id", async |req, res| {
    let path = db.lookup_avatar_path(&req.params["id"]).await?;
    res.send_file(&path, None).await?;
    Ok(())
});
```

### Waiting

If a route or a middleware function has to wait for some time, use `cpeak::sleep`. It's what `await setTimeout(ms)` from `node:timers/promises` is in Node.js:

```rust
use std::time::Duration;

server.route("get", "/slow", async |req, res| {
    cpeak::sleep(Duration::from_millis(100)).await;
    return res.json(json!({ "message": "Sorry for the wait!" }));
});
```

Only that request waits. Its worker thread handles other requests in the meantime, so thousands of requests can be waiting at once. Don't use `std::thread::sleep` in your routes: it stops the whole worker thread, and every request on it.

### Listening

Start listening on a specific port like this:

```rust
server
    .listen(3000, || {
        println!("Server has started on port 3000");
    })
    .wait();
```

`server.listen` accepts a port (and listens over both IPv4 and IPv6), or anything that resolves to a socket address, like `"127.0.0.1:3000"` or `("localhost", 3000)`. Cpeak starts one worker thread per CPU core, each with its own event loop, a bit like running a cluster of Node.js processes.

`server.listen` gives you back the running server, and calling `wait` on it keeps your program running for as long as the server does. If you're already inside async code (like a `#[tokio::main]` function), `.await` the server instead. You can also keep it around to get its address, or to close it. `close` stops accepting new connections, and lets the requests in progress finish:

```rust
let running = server.listen(3000, || {});
println!("Listening on {}", running.address());

// Later on...
running.close();
running.wait(); // returns once every connection is closed
```

### HTTPS

To serve your app over HTTPS, give Cpeak your private key and your certificate at construction time. They are what you'd pass to `https.createServer()` in Node.js: the contents of the two PEM files.

```rust
use cpeak::{Cpeak, CpeakOptions, HttpsOptions};

let mut server = Cpeak::new(CpeakOptions {
    https: Some(HttpsOptions {
        key: std::fs::read_to_string("./key.pem").unwrap(),
        cert: std::fs::read_to_string("./cert.pem").unwrap(),
    }),
    ..Default::default()
});

server
    .listen(443, || {
        println!("Server has started on port 443");
    })
    .wait();
```

If your certificate was signed by an intermediate certificate, `cert` needs that one too, right after yours. With Let's Encrypt, that's the `fullchain.pem` file.

A server speaks either HTTP or HTTPS. To have both, say HTTP on port 80 and HTTPS on port 443, put your routes in a function and make two servers with it:

```rust
fn app(https: Option<HttpsOptions>) -> Cpeak {
    let mut server = Cpeak::new(CpeakOptions {
        https,
        ..Default::default()
    });

    server.route("get", "/", async |_req, res| {
        return res.json(json!({ "message": "Hi there!" }));
    });

    server
}

fn main() {
    let key = std::fs::read_to_string("./key.pem").unwrap();
    let cert = std::fs::read_to_string("./cert.pem").unwrap();

    let http = app(None).listen(80, || {});
    let https = app(Some(HttpsOptions { key, cert })).listen(443, || {});

    http.wait();
    https.wait();
}
```

Under the hood this is [rustls](https://github.com/rustls/rustls), with TLS 1.3 and 1.2.

### Util Functions

There are utility functions that you can include and use as middleware functions. These are meant to make it easier for you to build HTTP applications. In the future, many more will be added, and the ones you don't use are left out of your compiled program. No need to have many dependencies for simple applications!

The list of utility functions as of now:

- serve_static
- parse_json
- render
- cookie_parser
- swagger
- auth
- cors

Including any one of them is done like this:

```rust
use cpeak::{cpeak, util_name};
```

Rust doesn't have default arguments, so where a utility function takes options, pass `None` to go with the defaults, or pass the options struct to tune it.

#### serve_static

With this middleware function, you can automatically set a folder in your project to be served by Cpeak. Here’s how to set it up:

```rust
server.before_each(serve_static("./public", None));
```

`serve_static` infers each file's MIME type from its extension. If your folder contains types that aren't in the built-in registry, register them on the `Cpeak::new()` constructor (see [MIME Types](#mime-types)).

You can also serve your static files under a URL prefix by passing a `prefix` option. This is useful when you want all static assets to live under a specific path like `/static`:

```rust
server.before_each(serve_static(
    "./public",
    ServeStaticOptions {
        prefix: "/static".into(),
        ..Default::default()
    },
));
```

With this setup, a file at `./public/app.js` would be served at `/static/app.js` instead of `/app.js`.

If file names in the served folder change while the server is running (e.g. during development), pass `live: true` so the middleware stats the disk on each request instead of caching the file map at startup:

```rust
server.before_each(serve_static(
    "./public",
    ServeStaticOptions {
        live: true,
        ..Default::default()
    },
));
```

And if there are files or folders in there that shouldn't be served, list them (relative to the folder) in the `exclude` option, like `exclude: vec!["drafts".into()]`.

#### parse_json

With this middleware function, you can easily read and send JSON in HTTP message bodies in route and middleware functions. Fire it up like this:

```rust
// You can pass an optional limit option to indicate the maximum
// JSON body size that your server will accept.
server.before_each(parse_json(ParseJsonOptions { limit: 1024 * 1024 })); // default value is 1024 * 1024 (1MB)
```

Read and send JSON from HTTP messages like this:

```rust
server.route("put", "/api/user", async |req, res| {
    // Reading JSON from the HTTP request:
    let email = &req.body["email"];

    // rest of your logic...

    // Sending JSON in the HTTP response:
    return res.status(201).json(json!({ "message": "Something was created..." }));
});
```

`req.body` is a `serde_json::Value`, so you can also turn it into your own struct with serde, like `let user: User = serde_json::from_value(req.body.clone())?;`.

JSON is read and written by [sonic-rs](https://github.com/cloudwego/sonic-rs), which gives the exact same output as serde_json, just faster.

#### render

With this function you can do server side rendering before sending a file to a client. This can be useful for dynamic customization and search engine optimization.

First fire it up like this:

```rust
server.before_each(render());
```

And then for rendering:

```rust
server.route("get", "/", async |req, res| {
    return res
        .render(
            "./public/index.html",
            json!({
                "title": "Page title",
                "name": "Allan"
            }),
            "text/html",
        )
        .await;
});
```

The third argument (the MIME type) is optional. When omitted (by passing `None`), Cpeak infers it from the file extension using the registry on the `Cpeak::new()` constructor.

You can then inject the variables into your file in {{ variable_name }} like this:

```HTML
<html>
    <head>
        <title>{{ title }}</title>
    </head>
    <body>
        <h1>{{ name }}</h1>
    </body>
</html>
```

The values in {{ }} are HTML-escaped. To inject raw HTML instead, use `<cpeak html={variable_name} />`, and to include another file, use `<cpeak include="./partials/nav.html" />` (the path is relative to the file that includes it). If you want the rendered text instead of sending it, use `render::string("./public/index.html", data).await`.

#### cookie_parser

With this middleware function, you can easily read and set cookies in your route and middleware functions. Fire it up like this:

```rust
server.before_each(cookie_parser(None));
```

If you need to use signed cookies, pass a secret (it has to be at least 32 characters long):

```rust
server.before_each(cookie_parser(CookieParserOptions {
    secret: Some("your-secret-key-min-32-chars-long".into()),
}));
```

Signed cookies use HMAC to verify integrity. The original value stays readable by the client, but any tampering with it will be detected on the server side. This makes them a solid choice for session identifiers or user IDs where you want to prevent impersonation without hiding the value itself.

Read incoming cookies like this:

```rust
server.route("get", "/dashboard", async |req, res| {
    // Regular cookies
    let theme = req.cookies.get("theme");

    // Signed cookies — Some(None) if the signature is invalid or the value was tampered with
    let user_id = req.signed_cookies.get("userId");

    return res.status(200).json(json!({ "theme": theme, "userId": user_id }));
});
```

Set cookies on the response like this:

```rust
server.route("post", "/login", async |req, res| {
    // A plain cookie
    res.cookie(
        "theme",
        "dark",
        CookieOptions {
            http_only: true,
            max_age: Some(7 * 24 * 60 * 60 * 1000),
            ..Default::default()
        },
    );

    // A signed cookie
    res.cookie(
        "userId",
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
```

Clear a cookie like this:

```rust
res.clear_cookie("userId", None);
```

The full list of cookie options you can pass as the third argument to `res.cookie()`:

- `signed` — sign the cookie value with HMAC using the secret you provided to `cookie_parser`
- `http_only` — prevents client-side JavaScript from accessing the cookie
- `secure` — instructs the browser to send the cookie only over HTTPS
- `same_site` — controls cross-site cookie behavior; accepts `SameSite::Strict`, `SameSite::Lax`, or `SameSite::None`
- `max_age` — cookie lifetime in milliseconds
- `expires` — a specific expiration `SystemTime` for the cookie
- `path` — path the cookie is valid for (defaults to `"/"`)
- `domain` — domain the cookie is valid for

The signed cookies are compatible with the Node.js version of Cpeak, so as long as you use the same secret, both versions can read each other's cookies.

#### swagger

With this middleware function, you can serve an interactive Swagger UI for your API documentation. It works alongside the `serve_static` utility and the Swagger UI static assets.

Start by downloading the latest release of [swagger-ui](https://github.com/swagger-api/swagger-ui/releases), and copy its `dist` folder into your project (like `./swagger-ui`). Your spec can be anything that implements serde's `Serialize`, like a `serde_json::Value` loaded from a JSON file (or loaded from a YAML file with a serde YAML crate).

Then fire it up like this:

```rust
use cpeak::{cpeak, serve_static, swagger, ServeStaticOptions};

let mut server = cpeak();

let swagger_document: serde_json::Value =
    serde_json::from_str(&std::fs::read_to_string("./src/swagger.json").unwrap()).unwrap();

server.before_each(swagger(&swagger_document, None));
server.before_each(serve_static(
    "./swagger-ui",
    ServeStaticOptions {
        prefix: "/api-docs".into(),
        ..Default::default()
    },
));
```

Once set up, your Swagger UI will be available at `/api-docs`. The `swagger` middleware handles serving your spec at `/api-docs/spec.json` and wiring up the Swagger UI initializer, while `serve_static` serves all the Swagger UI static assets under the same prefix.

If you want to serve the docs under a different path, pass it as the second argument to `swagger` and match the prefix in `serve_static`:

```rust
server.before_each(swagger(&swagger_document, Some("/docs")));
server.before_each(serve_static(
    "./swagger-ui",
    ServeStaticOptions {
        prefix: "/docs".into(),
        ..Default::default()
    },
));
```

#### auth

With this middleware you can add a full-fledged authentication system to your application with emails, username and password authentication, with features such as Forgot Password, Update Password and so forth. We use timing-safe comparisons throughout. It attaches helper methods directly to `req` so your route handlers stay clean.

Fire it up like this:

```rust
use cpeak::{auth, cookie_parser, cpeak, parse_json, AuthOptions, FindToken, RevokeToken, SaveToken};

let mut app = cpeak();

app.before_each(parse_json(None));
app.before_each(cookie_parser(None));

app.before_each(auth(AuthOptions {
    // Required
    secret: "your-secret-min-32-chars-long!!!".into(), // used to sign token IDs with HMAC
    save_token: SaveToken::new(async |token_id, user_id, expires_at| { /* store in your DB */ Ok(()) }),
    find_token: FindToken::new(async |token_id| { /* return Some(TokenRecord { user_id, expires_at }) or None */ Ok(None) }),

    // Enables req.logout()
    revoke_token: Some(RevokeToken::new(async |token_id| { /* delete from your DB */ Ok(()) })),

    // Optional PBKDF2 tuning (defaults shown):
    iterations: 210_000, // higher = slower brute-force
    keylen: 64,          // derived key length in bytes
    digest: "sha512".into(),
    salt_size: 32,

    // Optional token signing tuning (defaults shown):
    hmac_algorithm: "sha256".into(),
    token_id_size: 20,
    token_expiry: 7 * 24 * 60 * 60 * 1000, // 7 days in ms
}));
```

Once set up, the following methods are available on `req` inside your routes and middleware (they're all async and return a `Result`, so you `.await?` them):

| Method                                             | Description                                                                                                                                |
| -------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `req.hash_password(&password)`                     | Hashes a password with PBKDF2. Store the result; never store plaintext.                                                                    |
| `req.login(&password, &hashed_password, &user_id)` | Verifies the password and if correct, creates a signed token. Returns the token string to send to the client, or `None` on wrong password. |
| `req.verify_token(&token)`                         | Validates a token's HMAC signature and expiry. Returns `Some(VerifiedToken { user_id })` or `None`.                                        |
| `req.logout(&token)`                               | Revokes the token via your `revoke_token` callback. Only available when `revoke_token` is provided.                                        |

Here are the two most common middleware patterns you'll want to set up:

```rust
// Returns a 401 error if the request has no valid token. Use on protected routes.
async fn require_auth(
    req: &mut CpeakRequest,
    res: &mut CpeakResponse,
    next: Next,
) -> Result<(), CpeakError> {
    let Some(token) = req.headers.get("authorization") else {
        return Err(CpeakError::new(401, "Unauthorized."));
    };

    let Some(result) = req.verify_token(token).await? else {
        return Err(CpeakError::new(401, "Unauthorized."));
    };

    req.set("user", json!({ "id": result.user_id }));
    next()
}

// Silently sets the user when a valid token is present, but lets the request through either way.
// Useful for routes accessible by both authenticated and unauthenticated users.
async fn optional_auth(
    req: &mut CpeakRequest,
    _res: &mut CpeakResponse,
    next: Next,
) -> Result<(), CpeakError> {
    if let Some(token) = req.headers.get("authorization") {
        if let Some(result) = req.verify_token(token).await? {
            req.set("user", json!({ "id": result.user_id }));
        }
    }
    next()
}
```

The same password hashing is also available outside of your routes, with `cpeak::hash_password` and `cpeak::verify_password`. The hashes and tokens are compatible with the Node.js version of Cpeak, so you can move your users from one version to the other as they are.

For complete working examples, see:

- [`examples/auth-localstorage.rs`](examples/auth-localstorage.rs) — token sent via the `Authorization` header (suited for SPAs and mobile clients)
- [`examples/auth-cookies.rs`](examples/auth-cookies.rs) — token stored in an `http_only` cookie (protects against XSS)

#### cors

The CORS middleware allows you to enable Cross-Origin Resource Sharing in your application.

```rust
server.before_each(cors(CorsOptions {
    origin: "http://localhost:3000".into(), // &str, list of &str, Regex, bool, or OriginInput::function(async |origin| ...). Default: "*" (all origins)
    methods: "GET,POST,PUT,DELETE".into(),  // allowed HTTP methods. Default: "GET,HEAD,PUT,PATCH,POST,DELETE"
    allowed_headers: "Content-Type".into(), // headers the browser may send. Default: echoes request headers for origin "*", else "Content-Type, Authorization"
    exposed_headers: "X-Request-Id".into(), // response headers the browser may read. Default: none
    credentials: true,                      // adds Access-Control-Allow-Credentials: true. Default: false
    max_age: 3600,                          // seconds to cache preflight result in the browser. Default: 86400
    preflight_continue: false,              // pass OPTIONS preflight to next middleware instead of auto-responding. Default: false
    options_success_status: 204,            // status code for successful preflight responses. Default: 204
}));
```

Or if you don't care and want to allow everything with the default settings, just do:

```rust
server.before_each(cors(None));
```

## Complete Example

Here you can see all the features that Cpeak offers (excluding the authentication features), in one small piece of code. You can also find it in [`examples/complete-example.rs`](examples/complete-example.rs), ready to run with `cargo run --example complete-example`:

```rust
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
                "<path-to-file-relative-to-cwd>",
                json!({
                    "test": "some testing value",
                    "number": "2343242"
                }),
                "<mime-type>",
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
            .send_file("<path-to-file-relative-to-cwd>", "<mime-type>")
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
```

## Versioning Notice

#### Version `0.x.x`

All version `0.x.x` releases are considered to be in active development. These versions include ongoing feature additions and API changes as we refine the framework, and keep it in line with the Node.js version of Cpeak. Frequent updates may require code changes, so version `0.x.x` is not recommended for production environments.
For new features, bug fixes, and other changes that don't break existing code, the patch version will be increased. For changes that break existing code, the minor version will be increased.

#### Version `1.x.x`

Version `1.x.x` and beyond will be our first production-ready releases. These versions are intended for stable, long-term use, with a focus on backward compatibility and minimal breaking changes.
