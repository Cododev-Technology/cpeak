// Listening, and closing the server
mod common;

use std::net::TcpStream;
use std::time::Duration;

use common::request;
use cpeak::cpeak;
use serde_json::json;

#[tokio::test]
async fn should_give_the_address_and_close_gracefully() {
    let mut app = cpeak();
    app.route("get", "/slow", async |_req, res| {
        tokio::time::sleep(Duration::from_millis(300)).await;
        res.json(json!({ "done": true }))
    });

    let mut called = false;
    let server = app.listen(("127.0.0.1", 0), || called = true);
    let port = server.address().port();
    assert!(
        called,
        "the callback should run once the server is listening"
    );
    assert_ne!(port, 0);

    // A request that is in progress when we close the server gets to finish
    let slow = std::thread::spawn(move || request(port, "GET", "/slow").send());
    tokio::time::sleep(Duration::from_millis(100)).await;

    server.close();
    server.await;

    let res = slow.join().unwrap();
    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "done": true }));

    // Once closed, the server doesn't accept new connections
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
}

#[test]
fn should_listen_on_an_address_string() {
    let mut app = cpeak();
    app.route("get", "/", async |_req, res| {
        res.json(json!({ "ok": true }))
    });

    // Port 0 lets the operating system pick a free port
    let server = app.listen("127.0.0.1:0", || {});
    let res = request(server.address().port(), "GET", "/").send();

    assert_eq!(res.json(), json!({ "ok": true }));
    server.close();
}

// A plain `fn main` with no async runtime of its own can keep the server running
#[test]
fn should_block_on_wait_until_the_server_closes() {
    let mut app = cpeak();
    app.route("get", "/", async |_req, res| {
        res.json(cpeak::json!({ "ok": true }))
    });

    let server = app.listen(("127.0.0.1", 0), || {});
    let port = server.address().port();

    let closer = server.clone();
    let client = std::thread::spawn(move || {
        let res = request(port, "GET", "/").send();
        closer.close();
        res
    });

    // Returns only once the client thread has closed the server
    server.wait();

    let res = client.join().unwrap();
    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "ok": true }));
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
}

// Like Node.js (EADDRINUSE): a port that a server is listening on can't be listened on again
#[test]
fn should_fail_to_listen_on_a_port_that_is_taken() {
    let server = cpeak().listen(("127.0.0.1", 0), || {});
    let port = server.address().port();

    let second = std::panic::catch_unwind(|| cpeak().listen(("127.0.0.1", port), || {}));
    assert!(
        second.is_err(),
        "listening on a taken port should have failed"
    );

    server.close();
    server.wait();

    // Once the first server is closed, the port is free again
    let third = cpeak().listen(("127.0.0.1", port), || {});
    assert_eq!(third.address().port(), port);
    third.close();
    third.wait();
}

// Every connection is served, whichever worker it ends up on
#[test]
fn should_serve_many_connections_at_once() {
    let mut app = cpeak();
    app.route("get", "/", async |_req, res| {
        res.json(json!({ "ok": true }))
    });
    let server = app.listen(("127.0.0.1", 0), || {});
    let port = server.address().port();

    let clients: Vec<_> = (0..64)
        .map(|_| std::thread::spawn(move || request(port, "GET", "/").send().status))
        .collect();
    for client in clients {
        assert_eq!(client.join().unwrap(), 200);
    }

    server.close();
    server.wait();
}
