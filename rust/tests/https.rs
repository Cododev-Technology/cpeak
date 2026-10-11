// HTTPS: serving an app over TLS, with the `https` option
mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use common::{parse_response, TestResponse};
use cpeak::{json, Cpeak, CpeakOptions, HttpsOptions};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

const KEY: &str = include_str!("files/key.pem");
const CERT: &str = include_str!("files/cert.pem");

fn build() -> Cpeak {
    let mut app = Cpeak::new(CpeakOptions {
        https: Some(HttpsOptions {
            key: KEY.to_string(),
            cert: CERT.to_string(),
        }),
        ..Default::default()
    });

    app.route("get", "/", async |_req, res| {
        res.json(json!({ "secure": true }))
    });

    app.route("get", "/hello/:name", async |req, res| {
        res.json(json!({ "hello": req.params["name"] }))
    });

    // Sends the request body back as it came
    app.route("post", "/echo", async |req, res| {
        let mut body = Vec::new();
        while let Some(chunk) = req.chunk().await? {
            body.extend_from_slice(&chunk);
        }
        res.set_header("Content-Type", "application/octet-stream")
            .end(body)
    });

    // A response of the size the path says, in one piece
    app.route("get", "/big/:size", async |req, res| {
        let size: usize = req.params["size"].parse().unwrap();
        res.set_header("Content-Type", "application/octet-stream")
            .end(pattern(size))
    });

    // The same, written a piece at a time
    app.route("get", "/stream/:size", async |req, res| {
        let size: usize = req.params["size"].parse().unwrap();
        res.set_header("Content-Type", "application/octet-stream");
        for piece in pattern(size).chunks(10_000) {
            res.write(piece.to_vec()).await?;
        }
        res.end("")
    });

    app
}

fn port() -> u16 {
    common::serve(build)
}

// Bytes that are easy to tell apart when some go missing or change places
fn pattern(size: usize) -> Vec<u8> {
    (0..size).map(|index| (index * 31 % 251) as u8).collect()
}

// The test certificate is one we made ourselves, so no client would trust it. This one does.
#[derive(Debug)]
struct TrustsAnything(Arc<CryptoProvider>);

impl ServerCertVerifier for TrustsAnything {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

type Connection = StreamOwned<ClientConnection, TcpStream>;

// A TLS connection to the server, offering the given protocols (ALPN)
fn connect_offering(port: u16, protocols: &[&str]) -> Connection {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(TrustsAnything(provider)))
        .with_no_client_auth();
    config.alpn_protocols = protocols
        .iter()
        .map(|protocol| protocol.as_bytes().to_vec())
        .collect();

    let client =
        ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").unwrap())
            .unwrap();
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    StreamOwned::new(client, stream)
}

fn connect(port: u16) -> Connection {
    connect_offering(port, &["http/1.1"])
}

fn get(path: &str, last: bool) -> Vec<u8> {
    let close = if last { "Connection: close\r\n" } else { "" };
    format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n{close}\r\n").into_bytes()
}

fn post(path: &str, body: &[u8], last: bool) -> Vec<u8> {
    let close = if last { "Connection: close\r\n" } else { "" };
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n{close}\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(body);
    request
}

// Everything the server sends until it closes the connection. This fails if the server just
// drops the connection, without ending the TLS session first.
fn read_to_end(connection: &mut Connection) -> Vec<u8> {
    let mut raw = Vec::new();
    connection.read_to_end(&mut raw).unwrap();
    raw
}

// One response, from a connection that stays open
fn read_response(connection: &mut Connection) -> TestResponse {
    let mut raw = Vec::new();
    let mut byte = [0];
    while !raw.ends_with(b"\r\n\r\n") {
        connection.read_exact(&mut byte).unwrap();
        raw.push(byte[0]);
    }
    let length: usize = parse_response(&raw, true)
        .header("content-length")
        .expect("a response without a Content-Length")
        .parse()
        .unwrap();
    let mut body = vec![0; length];
    connection.read_exact(&mut body).unwrap();
    raw.extend_from_slice(&body);
    parse_response(&raw, false)
}

#[test]
fn should_serve_a_request_over_https() {
    let mut connection = connect(port());
    connection.write_all(&get("/", true)).unwrap();
    let res = parse_response(&read_to_end(&mut connection), false);

    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "secure": true }));
}

#[test]
fn should_use_tls_1_3_with_the_certificate_of_the_options() {
    let mut connection = connect(port());
    connection.write_all(&get("/", true)).unwrap();
    read_to_end(&mut connection);

    assert_eq!(
        connection.conn.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    assert_eq!(connection.conn.alpn_protocol(), Some(&b"http/1.1"[..]));

    let ours = CertificateDer::from_pem_slice(CERT.as_bytes()).unwrap();
    let served = connection.conn.peer_certificates().unwrap();
    assert_eq!(served.len(), 1);
    assert_eq!(served[0], ours);
}

// A client that says nothing about the protocol it speaks gets HTTP/1.1 all the same
#[test]
fn should_serve_a_client_that_offers_no_protocol() {
    let mut connection = connect_offering(port(), &[]);
    connection.write_all(&get("/hello/there", true)).unwrap();
    let res = parse_response(&read_to_end(&mut connection), false);

    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "hello": "there" }));
    assert_eq!(connection.conn.alpn_protocol(), None);
}

// We only speak HTTP/1.1, so a client that only speaks HTTP/2 can't be served
#[test]
fn should_refuse_a_client_that_only_speaks_http_2() {
    let mut connection = connect_offering(port(), &["h2"]);
    let sent = connection.write_all(&get("/", true));
    let mut raw = Vec::new();
    let received = connection.read_to_end(&mut raw);

    assert!(sent.is_err() || received.is_err());
    assert!(raw.is_empty());
}

#[test]
fn should_serve_many_requests_on_one_connection() {
    let mut connection = connect(port());

    for name in ["one", "two", "three"] {
        connection
            .write_all(&get(&format!("/hello/{name}"), false))
            .unwrap();
        let res = read_response(&mut connection);
        assert_eq!(res.status, 200);
        assert_eq!(res.json(), json!({ "hello": name }));
    }
}

// The requests are sent without waiting for the responses, which come back in the same order
#[test]
fn should_serve_pipelined_requests_over_https() {
    let mut connection = connect(port());

    let mut requests = Vec::new();
    for index in 0..20 {
        requests.extend_from_slice(&get(&format!("/hello/{index}"), false));
    }
    requests.extend_from_slice(&post("/echo", b"the last one", true));
    connection.write_all(&requests).unwrap();

    for index in 0..20 {
        let res = read_response(&mut connection);
        assert_eq!(res.json(), json!({ "hello": index.to_string() }));
    }
    let res = read_response(&mut connection);
    assert_eq!(res.body, b"the last one");
    assert!(read_to_end(&mut connection).is_empty());
}

#[test]
fn should_send_back_request_bodies_of_any_size_byte_for_byte() {
    let mut connection = connect(port());

    // From nothing at all to bodies that take many TLS records
    for size in [0, 1, 1024, 10_240, 16_384, 16_385, 100_000, 1_000_000] {
        let body = pattern(size);
        connection.write_all(&post("/echo", &body, false)).unwrap();
        let res = read_response(&mut connection);

        assert_eq!(res.status, 200, "a body of {size} bytes");
        assert!(res.body == body, "a body of {size} bytes came back changed");
    }
}

#[test]
fn should_send_back_a_chunked_request_body() {
    let mut connection = connect(port());
    let body = pattern(50_000);

    let mut request =
        b"POST /echo HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    for chunk in body.chunks(7_000) {
        request.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        request.extend_from_slice(chunk);
        request.extend_from_slice(b"\r\n");
    }
    request.extend_from_slice(b"0\r\n\r\n");
    connection.write_all(&request).unwrap();

    let res = parse_response(&read_to_end(&mut connection), false);
    assert_eq!(res.status, 200);
    assert!(res.body == body);
}

#[test]
fn should_send_big_responses_in_one_piece_or_many() {
    for path in ["/big/3000000", "/stream/3000000"] {
        let mut connection = connect(port());
        connection.write_all(&get(path, true)).unwrap();
        let res = parse_response(&read_to_end(&mut connection), false);

        assert_eq!(res.status, 200, "{path}");
        assert!(res.body == pattern(3_000_000), "{path} came out changed");
    }
}

// A client that reads slowly makes the server wait for it, without losing anything
#[test]
fn should_wait_for_a_client_that_reads_slowly() {
    let mut connection = connect(port());
    connection.write_all(&get("/stream/2000000", true)).unwrap();

    let mut raw = Vec::new();
    let mut piece = [0; 20_000];
    loop {
        let read = connection.read(&mut piece).unwrap();
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&piece[..read]);
        if raw.len() < 400_000 {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    let res = parse_response(&raw, false);
    assert!(res.body == pattern(2_000_000));
}

#[test]
fn should_serve_many_https_connections_at_once() {
    let port = port();
    let clients: Vec<_> = (0..32)
        .map(|index| {
            std::thread::spawn(move || {
                let mut connection = connect(port);
                for _ in 0..5 {
                    let body = pattern(5_000 + index);
                    connection.write_all(&post("/echo", &body, false)).unwrap();
                    assert!(read_response(&mut connection).body == body);
                }
            })
        })
        .collect();

    for client in clients {
        client.join().unwrap();
    }
}

// Plain HTTP on the port of an HTTPS server isn't answered: the connection is closed
#[test]
fn should_not_answer_plain_http() {
    let mut stream = TcpStream::connect(("127.0.0.1", port())).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    stream.write_all(&get("/", true)).unwrap();

    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    assert!(!raw.starts_with(b"HTTP/"));
}

// And the server is fine afterwards
#[test]
fn should_keep_serving_after_a_failed_handshake() {
    let port = port();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(b"this is not TLS at all\r\n\r\n").unwrap();
    drop(stream);

    // A client that leaves in the middle of the handshake
    drop(TcpStream::connect(("127.0.0.1", port)).unwrap());

    let mut connection = connect(port);
    connection.write_all(&get("/", true)).unwrap();
    let res = parse_response(&read_to_end(&mut connection), false);
    assert_eq!(res.json(), json!({ "secure": true }));
}

#[test]
fn should_refuse_https_options_that_are_not_a_key_and_a_certificate() {
    let options = |key: &str, cert: &str| CpeakOptions {
        https: Some(HttpsOptions {
            key: key.to_string(),
            cert: cert.to_string(),
        }),
        ..Default::default()
    };

    for (key, cert) in [("not a key", CERT), (KEY, "not a certificate"), (CERT, KEY)] {
        let created = std::panic::catch_unwind(|| Cpeak::new(options(key, cert)));
        let message = created
            .err()
            .and_then(|panic| panic.downcast::<String>().ok())
            .expect("the options should have been refused");
        assert!(
            message.starts_with("[cpeak] Invalid https options"),
            "{message}"
        );
    }
}

// Printing the options (in your logs, for example) doesn't show the private key
#[test]
fn should_not_show_the_key_when_the_options_are_printed() {
    let options = HttpsOptions {
        key: KEY.to_string(),
        cert: CERT.to_string(),
    };
    let printed = format!("{options:?}");

    assert!(!printed.contains("PRIVATE KEY"));
    assert!(printed.contains("CERTIFICATE"));
}
