# Cpeak

[![npm version](https://badge.fury.io/js/cpeak.svg)](https://www.npmjs.com/package/cpeak)

Cpeak is a minimal and fast Node.js framework inspired by Express.js & Fastify.

This project is designed to be improved until it's ready for use in complex production applications, aiming to be more performant and have a cleaner structure than Express.js and other common frameworks.

Cpeak replaces Express.js, body-parser, cookie-parser, compression, CORS, Passport and more, in one zero-dependency package. You can use it as a drop-in replacement for Express.js, and many npm packages that work with Express.js will also work with Cpeak. 

Cpeak currently has almost all the features of Express.js, with roughly the same performance as Fastify. Additionally, it has a codebase that you can easily navigate, audit and change if you want. *Benchmarks and test results, and videos on navigating and understanding the codebase will be added soon.*

While Cpeak is being built for production, it is also an educational project that was started as part of the [Understanding Node.js: Core Concepts](https://www.udemy.com/course/understanding-nodejs-core-concepts/?referralCode=0BC21AC4DD6958AE6A95) course. If you want to learn how to build a framework like this, and get to a point where you can build things like this yourself and beyond, check out this course!

## Why Cpeak?

- **Minimalism**: No unnecessary bloat, with zero dependencies. Just the core essentials you need to build fast and reliable applications.
- **Performance**: Engineered to be fast, **Cpeak** won’t sacrifice speed for excessive customizability.
- **Educational**: Every new change made in the project will be explained in great detail in this [YouTube playlist](https://www.youtube.com/playlist?list=PLCiGw8i6NhvqsA-ZZcChJ0kaHZ3hcIVdY). Follow this project and let's see what it takes to build an industry-leading product!
- **Express.js Compatible**: You can easily refactor from Cpeak to Express.js and vice versa. Many npm packages that work with Express.js will also work with Cpeak.

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
  - [Sending Files](#sending-files)
  - [Redirecting](#redirecting)
  - [MIME Types](#mime-types)
  - [Compression](#compression)
  - [Error Handling](#error-handling)
  - [Listening](#listening)
  - [Util Functions](#util-functions)
    - [serveStatic](#servestatic)
    - [parseJSON](#parsejson)
    - [render](#render)
    - [cookieParser](#cookieparser)
    - [swagger](#swagger)
    - [auth](#auth)
    - [cors](#cors)
- [Complete Example](#complete-example)
- [Versioning Notice](#versioning-notice)

## Getting Started

Ready to dive in? Install **Cpeak** via npm:

```bash
npm install cpeak
```

Cpeak is a **pure ESM** package, and to use it, your project needs to be an ESM as well. You can learn more about that [here](https://gist.github.com/sindresorhus/a39789f98801d908bbc7ff3ecc99d99c).

### Hello World App:

```javascript
import cpeak from "cpeak";

const server = cpeak();

server.route("get", "/", (req, res) => {
  return res.json({ message: "Hi there!" });
});

server.listen(3000, () => {
  console.log("Server has started on port 3000");
});
```

## Documentation

### Including

Include the framework like this:

```javascript
import cpeak from "cpeak";
```

Because of the minimalistic philosophy, you won’t add unnecessary objects to your memory as soon as you include the framework. If at any point you want to use a particular utility function (like `parseJSON` and `serveStatic`), include it like the line below, and only at that point will it be moved into memory:

```javascript
import cpeak, { serveStatic, parseJSON } from "cpeak";
```

### Initializing

Initialize the Cpeak server like this:

```javascript
const server = cpeak();
```

Now you can use this server object to start listening, add route logic, add middleware functions, and handle errors.

### Middleware

If you add a middleware function, that function will run before your route logic kicks in. Here you can customize the request object, return an error, or do anything else you want to do prior to your route logic, like authentication.

After calling `next`, the next middleware function is going to run if there’s any; otherwise, the route logic is going to run.

```javascript
server.beforeEach((req, res, next) => {
  if (req.headers.authentication) {
    // Your authentication logic...
    req.userId = "<something>";
    req.custom = "This is some string";
    next();
  } else {
    // Return an error and close the request...
    return res.status(401).json({ error: "Unauthorized" });
  }
});

server.beforeEach((req, res, next) => {
  console.log(
    "The custom value was added from the previous middleware: ",
    req.custom
  );
  next();
});
```

### Route Middleware

You can also add middleware functions for a particular route handler like this:

```javascript
const requireAuth = (req, res, next) => {
  // Check if user is logged in, if so then:
  req.test = "this is a test value";
  next();

  // If user is not logged in:
  throw { status: 401, message: "Unauthorized" };
};

server.route("get", "/profile", requireAuth, (req, res) => {
  console.log(req.test); // this is a test value
});
```

You can add as many middleware functions as you want for a route:

```javascript
server.route(
  "get",
  "/profile",
  requireAuth,
  anotherFunction,
  oneMore,
  (req, res) => {
    // your logic
  }
);
```

### Route Handling

You can add new routes like this:

```javascript
server.route("patch", "/the-path-you-want", (req, res) => {
  // your route logic
});
```

First add the HTTP method name you want to handle, then the path, and finally, the callback. The `req` and `res` object types are the same as in the Node.js HTTP module (`http.IncomingMessage` and `http.ServerResponse`). You can read more about them in the [official Node.js documentation](https://nodejs.org/docs/latest/api/http.html).

Under the hood, Cpeak stores your routes in a radix tree, so finding the right route for an incoming request is roughly O(log n) in your total route count. In plain terms, that means matching stays fast even when your app has hundreds or thousands of routes. You won't pay a linear scan per request as your route table grows.

### Fallback Handler

If no route or middleware matches an incoming request, Cpeak returns a default `404` message. You can replace it with your own handler using `server.fallback`:

```javascript
server.fallback((req, res) => {
  return res.status(404).json({ error: "not found" });
});
```

Use this for custom 404 pages, JSON error envelopes, or serving an SPA's `index.html` for unknown paths. Static, param, and `*` wildcard routes still win when they match, the fallback only fires once the router has no match.

### URL Variables & Parameters

To be more consistent with the broader Node.js community and frameworks, we call the HTTP URL parameters (query strings) '**query**', and the path variables (route parameters) '**params**'.

Here’s how we can read both:

```javascript
// Imagine request URL is example.com/test/my-title/more-text?filter=newest
server.route("patch", "/test/:title/more-text", (req, res) => {
  const title = req.params.title;
  const filter = req.query.filter;

  console.log(title); // my-title
  console.log(filter); // newest
});
```

### Sending Files

You can send a file as a Node.js Stream anywhere in your route or middleware logic like this:

```javascript
server.route("get", "/testing", (req, res) => {
  return res.status(200).sendFile("<file-path>", "<mime-type>");

  // Example:
  // return res.status(200).sendFile("./images/sun.jpeg", "image/jpeg");
});
```

The file’s binary content will be in the HTTP response body content. Make sure you specify a correct path relative to your CWD (use the `path` module for better compatibility).

The MIME type argument is optional. When omitted, Cpeak infers it from the file extension using its built-in MIME registry (see [MIME Types](#mime-types)):

```javascript
return res.status(200).sendFile("./images/sun.jpeg");
```

Passing the MIME type explicitly is the fastest path, Cpeak skips the extension lookup entirely.

### Redirecting

If you want to redirect to a new URL, you can simply do:

```javascript
res.redirect("https://whatever.com");
```

### MIME Types

`serveStatic`, `res.sendFile`, and `res.render` all share a single MIME type registry. Out of the box it covers the common web types:

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
  txt: "text/plain",
  json: "application/json",
  webmanifest: "application/manifest+json",
  eot: "application/vnd.ms-fontobject",
  otf: "font/otf",
  ttf: "font/ttf",
  woff: "font/woff",
  woff2: "font/woff2"
```

If you need to serve something else, register it once on the `cpeak()` constructor:

```javascript
const server = cpeak({
  mimeTypes: { mp3: "audio/mpeg", m4a: "audio/mp4" }
});
```

We keep this list small on purpose. Cpeak isn't going to load the [thousands of MIME types from the IANA registry](https://www.iana.org/assignments/media-types/media-types.xhtml) into your process memory on the off chance you might serve an `.apk` or a `.fla` one day. You tell us what you serve, we keep memory tight.

### Compression

You can enable HTTP response compression at construction time. Once enabled, `serveStatic`, `res.json()` and `res.sendFile()` will compress eligible responses automatically, and you also get a `res.compress()` method on the response for custom payloads.

Fire it up with the defaults like this:

```javascript
const server = cpeak({ compression: true });
```

Or pass options to tune the behavior:

```javascript
const server = cpeak({
  compression: {
    threshold: 1024, // bytes — responses smaller than this are sent uncompressed. Default: 1024
    brotli: {},      // node:zlib BrotliOptions
    gzip: {},        // node:zlib ZlibOptions
    deflate: {}      // node:zlib ZlibOptions
  }
});
```

For arbitrary payloads, like a `Buffer`, `string`, or `Readable` stream, use `res.compress`:

```javascript
server.route("get", "/report", async (req, res) => {
  const csv = await buildCsvReport();
  await res.compress("text/csv", csv);
});
```

When you're streaming, you can pass a known size as the third argument. Cpeak will use it to decide eligibility against `threshold`, and to set `Content-Length` if the body ends up being sent uncompressed:

```javascript
import { Readable } from "node:stream";

server.route("get", "/proxy/feed", async (req, res) => {
  const upstream = await fetch("https://example.com/feed.xml");
  const size = Number(upstream.headers.get("content-length"));
  await res.compress("application/xml", Readable.fromWeb(upstream.body), size);
});
```

You must first enable compression at construction time to use `res.compress`.

### Error Handling

If anywhere in your route functions or route middleware functions you want to return an error, you can just throw the error and let the automatic error handler catch it:

```javascript
server.route("get", "/api/document/:title", (req, res) => {
  const title = req.params.title;

  if (title.length > 500) throw { status: 400, message: "Title too long." };

  // The rest of your logic...
});
```

**Make sure** to call `server.handleErr` and pass a function like this to have the automatic error handler work properly:

```javascript
server.handleErr((error, req, res) => {
  if (error && error.status) {
    return res.status(error.status).json({ error: error.message });
  } else {
    // Log the unexpected errors somewhere so you can keep track of them...
    console.error(error);
    return res.status(500).json({
      error: "Sorry, something unexpected happened on our side."
    });
  }
});
```

_The error object is the object that you threw earlier in your routes or middleware._

One thing to keep in mind: `res.sendFile`, `res.render`, `res.compress`, and `res.json` all return a `Promise`. **Return** that promise (or `await` it in an `async` handler) so the framework can route any rejection to your `handleErr`:

```javascript
server.route("get", "/file", (req, res) => {
  return res.sendFile("./images/sun.jpeg", "image/jpeg");
});
```

Or, in an `async` handler that does other work alongside the send:

```javascript
server.route("get", "/avatars/:id", async (req, res) => {
  const path = await db.lookupAvatarPath(req.params.id);
  await res.sendFile(path);
});
```

### Listening

Start listening on a specific port like this:

```javascript
server.listen(3000, () => {
  console.log("Server has started on port 3000");
});
```

`server.listen` accepts the same arguments as Node.js's [http.Server.listen](https://nodejs.org/docs/latest/api/net.html#serverlistenoptions-callback), so you can also pass a host, an options object, or a Unix socket path.

### Util Functions

There are utility functions that you can include and use as middleware functions. These are meant to make it easier for you to build HTTP applications. In the future, many more will be added, and you only move them into memory once you include them. No need to have many npm dependencies for simple applications!

The list of utility functions as of now:

- serveStatic
- parseJSON
- render
- cookieParser
- swagger
- auth
- cors

Including any one of them is done like this:

```javascript
import cpeak, { utilName } from "cpeak";
```

#### serveStatic

With this middleware function, you can automatically set a folder in your project to be served by Cpeak. Here’s how to set it up:

```javascript
server.beforeEach(serveStatic("./public"));
```

`serveStatic` infers each file's MIME type from its extension. If your folder contains types that aren't in the built-in registry, register them on the `cpeak()` constructor (see [MIME Types](#mime-types)).

You can also serve your static files under a URL prefix by passing a `prefix` option. This is useful when you want all static assets to live under a specific path like `/static`:

```javascript
server.beforeEach(
  serveStatic("./public", { prefix: "/static" })
);
```

With this setup, a file at `./public/app.js` would be served at `/static/app.js` instead of `/app.js`.

If file names in the served folder change while the server is running (e.g. during development), pass `live: true` so the middleware stats the disk on each request instead of caching the file map at startup:

```javascript
server.beforeEach(serveStatic("./public", { live: true }));
```

#### parseJSON

With this middleware function, you can easily read and send JSON in HTTP message bodies in route and middleware functions. Fire it up like this:

```javascript
// You can pass an optional limit option to indicate the maximum
// JSON body size that your server will accept.
server.beforeEach(parseJSON({ limit: 1024 * 1024 })); // default value is 1024 * 1024 (1MB)
```

Read and send JSON from HTTP messages like this:

```javascript
server.route("put", "/api/user", (req, res) => {
  // Reading JSON from the HTTP request:
  const email = req.body.email;

  // rest of your logic...

  // Sending JSON in the HTTP response:
  return res.status(201).json({ message: "Something was created..." });
});
```

#### render

With this function you can do server side rendering before sending a file to a client. This can be useful for dynamic customization and search engine optimization.

First fire it up like this:

```javascript
server.beforeEach(render());
```

And then for rendering:

```javascript
server.route("get", "/", (req, res, next) => {
  return res.render(
    "./public/index.html",
    {
      title: "Page title",
      name: "Allan"
    },
    "text/html"
  );
});
```

The third argument (the MIME type) is optional. When omitted, Cpeak infers it from the file extension using the registry on the `cpeak()` constructor.

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

#### cookieParser

With this middleware function, you can easily read and set cookies in your route and middleware functions. Fire it up like this:

```javascript
server.beforeEach(cookieParser());
```

If you need to use signed cookies, pass a secret:

```javascript
server.beforeEach(cookieParser({ secret: "your-secret-key" }));
```

Signed cookies use HMAC to verify integrity. The original value stays readable by the client, but any tampering with it will be detected on the server side. This makes them a solid choice for session identifiers or user IDs where you want to prevent impersonation without hiding the value itself.

Read incoming cookies like this:

```javascript
server.route("get", "/dashboard", (req, res) => {
  // Regular cookies
  const theme = req.cookies.theme;

  // Signed cookies — returns false if the signature is invalid or the value was tampered with
  const userId = req.signedCookies.userId;

  return res.status(200).json({ theme, userId });
});
```

Set cookies on the response like this:

```javascript
server.route("post", "/login", (req, res) => {
  // A plain cookie
  res.cookie("theme", "dark", { httpOnly: true, maxAge: 7 * 24 * 60 * 60 * 1000 });

  // A signed cookie
  res.cookie("userId", "abc123", { signed: true, httpOnly: true, secure: true });

  return res.status(200).json({ message: "Logged in" });
});
```

Clear a cookie like this:

```javascript
res.clearCookie("userId");
```

The full list of cookie options you can pass as the third argument to `res.cookie()`:

- `signed` — sign the cookie value with HMAC using the secret you provided to `cookieParser`
- `httpOnly` — prevents client-side JavaScript from accessing the cookie
- `secure` — instructs the browser to send the cookie only over HTTPS
- `sameSite` — controls cross-site cookie behavior; accepts `"strict"`, `"lax"`, or `"none"`
- `maxAge` — cookie lifetime in milliseconds
- `expires` — a specific expiration `Date` for the cookie
- `path` — path the cookie is valid for (defaults to `"/"`)
- `domain` — domain the cookie is valid for

#### swagger

With this middleware function, you can serve an interactive Swagger UI for your API documentation. It works alongside the `serveStatic` utility and two npm packages: `swagger-ui-dist` (the Swagger UI static assets) and `yamljs` (to load your YAML spec file).

Start by installing the dependencies:

```bash
npm install swagger-ui-dist yamljs
```

Then fire it up like this:

```javascript
import cpeak, { swagger, serveStatic } from "cpeak";
import YAML from "yamljs";
import swaggerUiDist from "swagger-ui-dist";
import path from "node:path";

const server = cpeak();

const swaggerDocument = YAML.load(
  path.join(path.resolve(), "./src/swagger.yml")
);

server.beforeEach(swagger(swaggerDocument));
server.beforeEach(
  serveStatic(swaggerUiDist.getAbsoluteFSPath(), undefined, {
    prefix: "/api-docs",
  })
);
```

Once set up, your Swagger UI will be available at `/api-docs`. The `swagger` middleware handles serving your spec at `/api-docs/spec.json` and wiring up the Swagger UI initializer, while `serveStatic` serves all the Swagger UI static assets under the same prefix.

If you want to serve the docs under a different path, pass it as the second argument to `swagger` and match the prefix in `serveStatic`:

```javascript
server.beforeEach(swagger(swaggerDocument, "/docs"));
server.beforeEach(
  serveStatic(swaggerUiDist.getAbsoluteFSPath(), undefined, {
    prefix: "/docs",
  })
);
```

#### auth

With this middleware you can add a full-fledged authentication system to your application with emails, username and password authentication, with features such as Forgot Password, Update Password and so forth. We have no external dependencies, with timing-safe comparisons throughout. It attaches helper methods directly to `req` so your route handlers stay clean.

Fire it up like this:

```javascript
import cpeak, { parseJSON, cookieParser, auth } from "cpeak";

const app = cpeak();

app.beforeEach(parseJSON());
app.beforeEach(cookieParser());

app.beforeEach(
  auth({
    // Required
    secret: "your-secret-min-32-chars-long!!!", // used to sign token IDs with HMAC
    saveToken: async (tokenId, userId, expiresAt) => { /* store in your DB */ },
    findToken: async (tokenId) => { /* return { userId, expiresAt } or null */ },

    // Enables req.logout()
    revokeToken: async (tokenId) => { /* delete from your DB */ },

    // Optional PBKDF2 tuning (defaults shown):
    iterations: 210_000,  // higher = slower brute-force
    keylen: 64,           // derived key length in bytes
    digest: "sha512",
    saltSize: 32,

    // Optional token signing tuning (defaults shown):
    hmacAlgorithm: "sha256",
    tokenIdSize: 20,
    tokenExpiry: 7 * 24 * 60 * 60 * 1000, // 7 days in ms
  })
);
```

Once set up, the following methods are available on `req` inside your routes and middleware:

| Method | Description |
|--------|-------------|
| `req.hashPassword({ password })` | Hashes a password with PBKDF2. Store the result; never store plaintext. |
| `req.login({ password, hashedPassword, userId })` | Verifies the password and if correct, creates a signed token. Returns the token string to send to the client, or `null` on wrong password. |
| `req.verifyToken(token)` | Validates a token's HMAC signature and expiry. Returns `{ userId }` or `null`. |
| `req.logout(token)` | Revokes the token via your `revokeToken` callback. Only available when `revokeToken` is provided. |

Here are the two most common middleware patterns you'll want to set up:

```javascript
// Throws 401 if the request has no valid token. Use on protected routes.
const requireAuth = async (req, res, next) => {
  const token = req.headers["authorization"];
  if (!token) throw { status: 401, message: "Unauthorized." };

  const result = await req.verifyToken(token);
  if (!result) throw { status: 401, message: "Unauthorized." };

  req.user = { id: result.userId };
  next();
};

// Silently sets req.user when a valid token is present, but lets the request through either way.
// Useful for routes accessible by both authenticated and unauthenticated users.
const optionalAuth = async (req, _res, next) => {
  const token = req.headers["authorization"];
  if (token) {
    const result = await req.verifyToken(token);
    if (result) req.user = { id: result.userId };
  }
  next();
};
```

For complete working examples, see:

- [`examples/auth-localstorage.js`](examples/auth-localstorage.js) — token sent via the `Authorization` header (suited for SPAs and mobile clients)
- [`examples/auth-cookies.js`](examples/auth-cookies.js) — token stored in an `httpOnly` cookie (protects against XSS)

#### cors
The CORS middleware allows you to enable Cross-Origin Resource Sharing in your application.

```javascript
server.beforeEach(cors({
  origin: "http://localhost:3000",  // string, string[], RegExp, boolean, or async (origin) => boolean. Default: "*" (all origins)
  methods: "GET,POST,PUT,DELETE",   // allowed HTTP methods. Default: "GET,HEAD,PUT,PATCH,POST,DELETE"
  allowedHeaders: "Content-Type",   // headers the browser may send. Default: echoes request headers for origin:"*", else "Content-Type, Authorization"
  exposedHeaders: "X-Request-Id",   // response headers the browser may read. Default: none
  credentials: true,                // adds Access-Control-Allow-Credentials: true. Default: false
  maxAge: 3600,                     // seconds to cache preflight result in the browser. Default: 86400
  preflightContinue: false,         // pass OPTIONS preflight to next middleware instead of auto-responding. Default: false
  optionsSuccessStatus: 204         // status code for successful preflight responses. Default: 204
}));
```

Or if you don't care and want to allow everything with the default settings, just do:

```javascript
server.beforeEach(cors());
```

## Complete Example

Here you can see all the features that Cpeak offers (excluding the authentication features), in one small piece of code:

```javascript
import cpeak, { serveStatic, parseJSON, render, cookieParser, cors } from "cpeak";

const server = cpeak({
  mimeTypes: { mp3: "audio/mpeg" }
});

server.beforeEach(serveStatic("./public"));

server.beforeEach(render());

// For parsing JSON bodies
server.beforeEach(parseJSON());

// For reading and setting cookies
server.beforeEach(cookieParser({ secret: "your-secret-key" }));

// For enabling CORS
server.beforeEach(cors({
  origin: "http://localhost:3000",
  credentials: true,
  methods: "GET,POST,PUT,DELETE"
}));

// Adding custom middleware functions
server.beforeEach((req, res, next) => {
  req.custom = "This is some string";
  next();
});

// A middleware function that can be specified to run before some particular routes
const testRouteMiddleware = (req, res, next) => {
  req.whatever = "some calculated value maybe";

  if (req.params.test !== "something special") {
    throw { status: 400, message: "an error message" };
  }

  next();
};

// Adding route handlers
server.route("get", "/", (req, res, next) => {
  return res.render(
    "<path-to-file-relative-to-cwd>",
    {
      test: "some testing value",
      number: "2343242"
    },
    "<mime-type>"
  );
});

server.route("get", "/old-url", testRouteMiddleware, (req, res, next) => {
  return res.redirect("/new-url");
});

server.route("get", "/api/document/:title", testRouteMiddleware, (req, res) => {
  // Reading URL variables (route parameters)
  const title = req.params.title;

  // Reading URL parameters (query strings) (like /users?filter=active)
  const filter = req.query.filter;

  // Reading JSON request body
  const anything = req.body.anything;

  // Handling errors
  if (anything === "not-expected-thing")
    throw { status: 400, message: "Invalid property." };

  // Sending a JSON response
  return res.status(200).json({ message: "This is a test response" });
});

// Reading and setting cookies
server.route("post", "/login", (req, res) => {
  // Reads are available via req.cookies and req.signedCookies
  const sessionId = req.signedCookies.sessionId;

  // Set a signed session cookie
  res.cookie("sessionId", "abc123", { signed: true, httpOnly: true, secure: true });
  return res.status(200).json({ message: "Logged in" });
});

// Sending a file response
server.route("get", "/file", (req, res) => {
  // Make sure to specify a correct path and MIME type...
  return res.status(200).sendFile("<path-to-file-relative-to-cwd>", "<mime-type>");
});

// Handle all the errors that could happen in the routes
server.handleErr((error, req, res) => {
  if (error && error.status) {
    return res.status(error.status).json({ error: error.message });
  } else {
    console.error(error);
    return res.status(500).json({
      error: "Sorry, something unexpected happened from our side."
    });
  }
});

server.listen(3000, () => {
  console.log("Server has started on port 3000");
});
```

## Versioning Notice

#### Version `1.x.x`

Version `1.x.x` represents the initial release of our framework, developed during the _Understanding Node.js Core Concepts_ course. These versions laid the foundation for our project.

#### Version `2.x.x`

All version `2.x.x` releases are considered to be in active development, following the completion of the course. These versions include ongoing feature additions and API changes as we refine the framework. Frequent updates may require code changes, so version `2.x.x` is not recommended for production environments.
For new features, bug fixes, and other changes that don't break existing code, the patch version will be increased. For changes that break existing code, the minor version will be increased.

#### Version `3.x.x`

Version `3.x.x` and beyond will be our first production-ready releases. These versions are intended for stable, long-term use, with a focus on backward compatibility and minimal breaking changes.
