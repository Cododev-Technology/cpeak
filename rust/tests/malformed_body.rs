// A request whose body can't be read: a broken one is the client's mistake (400), and a
// body cut short is a client that left
mod common;

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use common::start;
use cpeak::cpeak;

// Sends a raw request and gives back the status line of the response
fn status_line(port: u16, request: &[u8], hang_up: bool) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(request).unwrap();
    if hang_up {
        stream.shutdown(Shutdown::Write).unwrap();
    }
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    let response = String::from_utf8_lossy(&response);
    response.lines().next().unwrap_or("").to_string()
}

// A chunk size of -1 isn't a chunk size
const BROKEN_CHUNKS: &[u8] =
    b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n-1\r\nhello\r\n0\r\n\r\n";

async fn read_body(
    req: &mut cpeak::CpeakRequest,
    res: &mut cpeak::CpeakResponse,
) -> Result<(), cpeak::CpeakError> {
    while req.chunk().await?.is_some() {}
    res.end("read it all")
}

#[test]
fn should_answer_400_to_a_malformed_body_when_the_app_has_no_error_handler() {
    let port = start(|| {
        let mut server = cpeak();
        server.route("post", "/", read_body);
        server
    });

    assert_eq!(
        status_line(port, BROKEN_CHUNKS, false),
        "HTTP/1.1 400 Bad Request"
    );
}

#[test]
fn should_give_the_error_handler_a_400_for_a_malformed_body_and_a_disconnect_for_a_cut_one() {
    let port = start(|| {
        let mut server = cpeak();
        server.route("post", "/", read_body);
        server.handle_err(async |error, _req, res| {
            // Tells the test what the handler was given
            let status = if error.client_disconnect {
                299
            } else {
                error.status.unwrap_or(500)
            };
            res.status(status).end("")
        });
        server
    });

    assert_eq!(
        status_line(port, BROKEN_CHUNKS, false),
        "HTTP/1.1 400 Bad Request"
    );

    // 10 bytes promised, 3 sent, then the client stops sending
    let cut = b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 10\r\n\r\nabc";
    assert!(status_line(port, cut, true).starts_with("HTTP/1.1 299"));
}
