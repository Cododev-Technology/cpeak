// WebSockets with fastwebsockets: req.as_http_mut() for the upgrade and res.send_response()
// for the 101, over HTTP and HTTPS (wss://)
mod common;

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{request, start};
use cpeak::{cpeak, Cpeak, CpeakOptions, CpeakRequest, CpeakResponse, HttpsOptions};
use fastwebsockets::{upgrade, FragmentCollector, Frame, OpCode, Payload, Role, WebSocket};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

// Sends back every text and binary message it gets
async fn echo(req: &mut CpeakRequest, res: &mut CpeakResponse) -> Result<(), cpeak::CpeakError> {
    let (response, websocket) = upgrade::upgrade(req.as_http_mut())?;
    cpeak::spawn_local(async move {
        let Ok(websocket) = websocket.await else {
            return;
        };
        let mut websocket = FragmentCollector::new(websocket);
        while let Ok(frame) = websocket.read_frame().await {
            match frame.opcode {
                OpCode::Close => break,
                OpCode::Text | OpCode::Binary => match websocket.write_frame(frame).await {
                    Ok(()) => {}
                    Err(_) => break,
                },
                _ => {}
            }
        }
    });
    res.send_response(response)
}

// The same, with the WebSocket on one of cpeak::spawn()'s threads: the upgraded connection
// leaves the worker's thread
async fn echo_elsewhere(
    req: &mut CpeakRequest,
    res: &mut CpeakResponse,
) -> Result<(), cpeak::CpeakError> {
    let (response, websocket) = upgrade::upgrade(req.as_http_mut())?;
    cpeak::spawn(async move {
        let Ok(websocket) = websocket.await else {
            return;
        };
        let mut websocket = FragmentCollector::new(websocket);
        while let Ok(frame) = websocket.read_frame().await {
            match frame.opcode {
                OpCode::Close => break,
                OpCode::Text | OpCode::Binary => match websocket.write_frame(frame).await {
                    Ok(()) => {}
                    Err(_) => break,
                },
                _ => {}
            }
        }
    });
    res.send_response(response)
}

fn app(options: CpeakOptions) -> Cpeak {
    let mut server = Cpeak::new(options);
    server.route("get", "/ws", echo);
    server.route("get", "/ws-elsewhere", echo_elsewhere);
    server.route("get", "/plain", async |_req, res| res.end("plain"));
    server
}

fn port() -> u16 {
    common::serve(|| app(CpeakOptions::default()))
}

const UPGRADE: &str = "GET /ws HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";

// Reads the head of a response, a byte at a time so nothing after it is taken
async fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let byte = stream.read_u8().await.expect("the connection closed");
        head.push(byte);
    }
    String::from_utf8(head).unwrap()
}

async fn connect(port: u16) -> WebSocket<TcpStream> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(UPGRADE.as_bytes()).await.unwrap();
    let head = read_head(&mut stream).await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    WebSocket::after_handshake(stream, Role::Client)
}

async fn send_and_receive(websocket: &mut WebSocket<TcpStream>, message: &[u8]) -> Vec<u8> {
    websocket
        .write_frame(Frame::binary(Payload::Borrowed(message)))
        .await
        .unwrap();
    let frame = websocket.read_frame().await.unwrap();
    assert_eq!(frame.opcode, OpCode::Binary);
    frame.payload.to_vec()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn should_switch_protocols_with_the_accept_key() {
    let port = port();
    runtime().block_on(async {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(UPGRADE.as_bytes()).await.unwrap();
        let head = read_head(&mut stream).await.to_lowercase();

        assert!(
            head.starts_with("http/1.1 101 switching protocols"),
            "{head}"
        );
        assert!(head.contains("upgrade: websocket"), "{head}");
        assert!(head.contains("connection: upgrade"), "{head}");
        // The key of RFC 6455's own example
        assert!(
            head.contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="),
            "{head}"
        );
        assert!(!head.contains("content-length"), "{head}");
    });
}

#[test]
fn should_echo_text_and_binary_messages() {
    let port = port();
    runtime().block_on(async {
        let mut websocket = connect(port).await;

        websocket
            .write_frame(Frame::text(Payload::Borrowed(b"hello")))
            .await
            .unwrap();
        let frame = websocket.read_frame().await.unwrap();
        assert_eq!(frame.opcode, OpCode::Text);
        assert_eq!(&*frame.payload, b"hello");

        for i in 0..200u32 {
            let message = i.to_be_bytes();
            assert_eq!(send_and_receive(&mut websocket, &message).await, message);
        }
    });
}

#[test]
fn should_echo_a_message_bigger_than_what_cpeak_holds_back() {
    let port = port();
    runtime().block_on(async {
        let mut websocket = connect(port).await;
        let message: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(send_and_receive(&mut websocket, &message).await, message);
        // And small ones after it
        assert_eq!(send_and_receive(&mut websocket, b"after").await, b"after");
    });
}

#[test]
fn should_upgrade_a_connection_that_already_had_a_request() {
    let port = port();
    runtime().block_on(async {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream
            .write_all(b"GET /plain HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let head = read_head(&mut stream).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let mut body = [0; 5];
        stream.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"plain");

        stream.write_all(UPGRADE.as_bytes()).await.unwrap();
        let head = read_head(&mut stream).await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let mut websocket = WebSocket::after_handshake(stream, Role::Client);
        assert_eq!(send_and_receive(&mut websocket, b"same").await, b"same");
    });
}

#[test]
fn should_serve_many_websockets_at_once() {
    let port = port();
    runtime().block_on(async {
        let mut clients = Vec::new();
        for client in 0..50u32 {
            clients.push(tokio::spawn(async move {
                let mut websocket = connect(port).await;
                for i in 0..20u32 {
                    let message = format!("{client}-{i}");
                    assert_eq!(
                        send_and_receive(&mut websocket, message.as_bytes()).await,
                        message.as_bytes()
                    );
                }
            }));
        }
        for client in clients {
            client.await.unwrap();
        }
    });
}

#[test]
fn should_answer_a_close_with_a_close() {
    let port = port();
    runtime().block_on(async {
        let mut websocket = connect(port).await;
        websocket
            .write_frame(Frame::close(1000, b"bye"))
            .await
            .unwrap();
        let frame = websocket.read_frame().await.unwrap();
        assert_eq!(frame.opcode, OpCode::Close);
    });
}

#[test]
fn should_keep_serving_plain_requests() {
    let port = port();
    assert_eq!(request(port, "GET", "/plain").send().text(), "plain");
}

// ---------------------------------------------------------------------------------------
// Over HTTPS, with a blocking TLS client and hand-made frames

const KEY_PEM: &str = include_str!("files/key.pem");
const CERT_PEM: &str = include_str!("files/cert.pem");

fn https_port() -> u16 {
    start(|| {
        app(CpeakOptions {
            https: Some(HttpsOptions {
                key: KEY_PEM.to_string(),
                cert: CERT_PEM.to_string(),
            }),
            ..Default::default()
        })
    })
}

#[derive(Debug)]
struct AcceptAnyCert;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

// A masked client frame (RFC 6455, 5.3)
fn client_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x80 | opcode];
    let len = payload.len();
    if len < 126 {
        frame.push(0x80 | len as u8);
    } else if len <= u16::MAX as usize {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    let mask = [0x12, 0x34, 0x56, 0x78];
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(i, byte)| byte ^ mask[i % 4]),
    );
    frame
}

// An unmasked server frame: its opcode and payload
fn read_server_frame(stream: &mut impl Read) -> (u8, Vec<u8>) {
    let mut start = [0; 2];
    stream.read_exact(&mut start).unwrap();
    assert_eq!(
        start[1] & 0x80,
        0,
        "a frame from the server must not be masked"
    );
    let opcode = start[0] & 0x0f;
    let len = match start[1] & 0x7f {
        126 => {
            let mut len = [0; 2];
            stream.read_exact(&mut len).unwrap();
            u16::from_be_bytes(len) as usize
        }
        127 => {
            let mut len = [0; 8];
            stream.read_exact(&mut len).unwrap();
            u64::from_be_bytes(len) as usize
        }
        len => len as usize,
    };
    let mut payload = vec![0; len];
    stream.read_exact(&mut payload).unwrap();
    (opcode, payload)
}

#[test]
fn should_echo_over_https() {
    let port = https_port();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
    .with_no_client_auth();
    let connection =
        rustls::ClientConnection::new(Arc::new(config), "localhost".try_into().unwrap()).unwrap();
    let socket = StdTcpStream::connect(("127.0.0.1", port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut stream = rustls::StreamOwned::new(connection, socket);

    stream.write_all(UPGRADE.as_bytes()).unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");

    for message in [&b"over tls"[..], &vec![7; 100_000]] {
        stream.write_all(&client_frame(0x2, message)).unwrap();
        stream.flush().unwrap();
        let (opcode, payload) = read_server_frame(&mut stream);
        assert_eq!(opcode, 0x2);
        assert_eq!(payload, message);
    }
}

#[test]
fn should_work_with_the_cpeak_shortcut_too() {
    // cpeak() is the same as Cpeak::new(CpeakOptions::default())
    let port = start(|| {
        let mut server = cpeak();
        server.route("get", "/ws", echo);
        server
    });
    runtime().block_on(async {
        let mut websocket = connect(port).await;
        assert_eq!(send_and_receive(&mut websocket, b"x").await, b"x");
    });
}

#[test]
fn should_hand_the_connection_to_another_thread() {
    let port = port();
    runtime().block_on(async {
        let mut clients = Vec::new();
        for client in 0..20u32 {
            clients.push(tokio::spawn(async move {
                let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                let upgrade = UPGRADE.replace("GET /ws ", "GET /ws-elsewhere ");
                stream.write_all(upgrade.as_bytes()).await.unwrap();
                let head = read_head(&mut stream).await;
                assert!(head.starts_with("HTTP/1.1 101"), "{head}");
                let mut websocket = WebSocket::after_handshake(stream, Role::Client);
                for i in 0..50u32 {
                    let message = format!("{client}:{i}");
                    assert_eq!(
                        send_and_receive(&mut websocket, message.as_bytes()).await,
                        message.as_bytes()
                    );
                }
            }));
        }
        for client in clients {
            client.await.unwrap();
        }
    });
}

// ---------------------------------------------------------------------------------------
// Closing the server

// Echoes until the server closes, then says goodbye with 1001 (going away)
async fn echo_until_closing(
    req: &mut CpeakRequest,
    res: &mut CpeakResponse,
) -> Result<(), cpeak::CpeakError> {
    let (response, websocket) = upgrade::upgrade(req.as_http_mut())?;
    cpeak::spawn_local(async move {
        let Ok(websocket) = websocket.await else {
            return;
        };
        let mut websocket = FragmentCollector::new(websocket);
        loop {
            tokio::select! {
                frame = websocket.read_frame() => match frame {
                    Ok(frame) if matches!(frame.opcode, OpCode::Text | OpCode::Binary) => {
                        if websocket.write_frame(frame).await.is_err() {
                            break;
                        }
                    }
                    Ok(frame) if frame.opcode == OpCode::Close => break,
                    Ok(_) => {}
                    Err(_) => break,
                },
                _ = cpeak::closing() => {
                    let _ = websocket.write_frame(Frame::close(1001, b"closing")).await;
                    break;
                }
            }
        }
    });
    res.send_response(response)
}

// Closes the server and gives back how long it took to stop
fn close_and_time(server: cpeak::CpeakServer) -> Duration {
    let started = Instant::now();
    server.close();
    server.wait();
    started.elapsed()
}

#[test]
fn should_tell_a_websocket_that_the_server_is_closing() {
    let mut app = cpeak();
    app.route("get", "/ws", echo_until_closing);
    let server = app.listen(("127.0.0.1", 0), || {});
    let port = server.address().port();

    runtime().block_on(async {
        let mut websocket = connect(port).await;
        assert_eq!(send_and_receive(&mut websocket, b"before").await, b"before");

        let stopping = std::thread::spawn(move || close_and_time(server));
        let frame = websocket.read_frame().await.unwrap();
        assert_eq!(frame.opcode, OpCode::Close);
        assert_eq!(
            u16::from_be_bytes([frame.payload[0], frame.payload[1]]),
            1001
        );

        let took = stopping.join().unwrap();
        assert!(took < Duration::from_secs(5), "took {took:?}");
    });
}

#[test]
fn should_stop_a_websocket_that_keeps_going_after_the_timeout() {
    let mut app = Cpeak::new(CpeakOptions {
        shutdown_timeout: Some(Duration::from_millis(300)),
        ..Default::default()
    });
    app.route("get", "/ws", echo);
    let server = app.listen(("127.0.0.1", 0), || {});
    let port = server.address().port();

    runtime().block_on(async {
        let mut websocket = connect(port).await;
        assert_eq!(send_and_receive(&mut websocket, b"before").await, b"before");

        let stopping = std::thread::spawn(move || close_and_time(server));
        // The server drops the connection once the timeout is over
        assert!(websocket.read_frame().await.is_err());

        let took = stopping.join().unwrap();
        assert!(took >= Duration::from_millis(300), "took {took:?}");
        assert!(took < Duration::from_secs(5), "took {took:?}");
    });
}
