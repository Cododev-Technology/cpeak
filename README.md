# Cpeak

[![npm version](https://badge.fury.io/js/cpeak.svg)](https://www.npmjs.com/package/cpeak)
[![crates.io](https://img.shields.io/crates/v/cpeak.svg)](https://crates.io/crates/cpeak)

Cpeak is a minimal and fast HTTP framework for Node.js and for Rust.

The two versions have the same API, the same names and the same concepts, so if you know one, you already know how to use the other:

- **[Cpeak for Node.js](node)**: use it if you want maximum development speed with the speed of Fastify, and features of Express.js. Cpeak for Node.js has <strong>zero dependencies</strong>.
- **[Cpeak for Rust](rust)**: use it if you want the best performance, control and memory usage over your application. Cpeak for rust is one of the <strong>fastest HTTP frameworks on the internet</strong>.

Both give you routing, middleware functions, JSON parsing, cookie parsing, CORS, compression, authentication and more, with a codebase that you can easily navigate, audit and change if you want. The Rust codebase mirrors the Node.js one file by file.

While Cpeak is being built for production, it is also an educational project that was started as part of the [Understanding Node.js: Core Concepts](https://www.udemy.com/course/understanding-nodejs-core-concepts/?referralCode=0BC21AC4DD6958AE6A95) course. If you want to learn how to build a framework like this, and get to a point where you can build things like this yourself and beyond, check out this course!

## Cpeak for Node.js

```bash
npm install cpeak
```

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

The full documentation is in [node/README.md](node/README.md).

## Cpeak for Rust

```bash
cargo add cpeak
```

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

The full documentation is in [rust/README.md](rust/README.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md), and [SECURITY.md](SECURITY.md) for reporting a vulnerability.

## License

[MIT](LICENSE)
