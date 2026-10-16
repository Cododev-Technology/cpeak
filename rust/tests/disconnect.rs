// A client that goes away while its request is being handled
mod common;

use std::io::Write;
use std::net::TcpStream;
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use std::time::Duration;

use common::start;
use cpeak::cpeak;

// Where the /stream route tells its test how it ended: true if res.write() failed
static ENDED: Mutex<Option<Sender<bool>>> = Mutex::new(None);

#[test]
fn should_fail_the_writes_of_a_route_whose_client_left_before_the_response_started() {
    let (ended, has_ended) = channel();
    *ENDED.lock().unwrap() = Some(ended);

    let port = start(|| {
        let mut server = cpeak();

        server.route("get", "/stream", async |_req, res| {
            // By the time anything is sent, the client is gone
            cpeak::sleep(Duration::from_millis(100)).await;

            // More than a streamed response keeps in line for its client (64KB)
            let mut write_failed = false;
            for _ in 0..8 {
                if res.write(vec![b'a'; 64 * 1024]).await.is_err() {
                    write_failed = true;
                    break;
                }
            }
            if let Some(ended) = &*ENDED.lock().unwrap() {
                let _ = ended.send(write_failed);
            }
            res.end("")
        });

        server
    });

    // Ask for it, and hang up while the route is at work, without waiting for the answer
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .write_all(b"GET /stream HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    std::thread::sleep(Duration::from_millis(30));
    drop(stream);

    // The route runs to its end, like a request handler in Node.js does when the client is
    // gone. Its writes fail, so it doesn't wait forever for a client to read them.
    let write_failed = has_ended
        .recv_timeout(Duration::from_secs(5))
        .expect("the route is still waiting for a client that left");
    assert!(write_failed);
}
