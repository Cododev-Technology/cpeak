// A tiny HTTP client for the tests (what supertest does for the Node.js version),
// plus a helper to start a Cpeak server in the background.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use cpeak::Cpeak;

// Starts the server returned by `build` once per test binary, and gives back its port
pub fn serve(build: fn() -> Cpeak) -> u16 {
    static STARTED: OnceLock<Mutex<HashMap<usize, u16>>> = OnceLock::new();
    let mut started = STARTED.get_or_init(Default::default).lock().unwrap();
    *started
        .entry(build as usize)
        .or_insert_with(|| start(build))
}

// Starts a server on a free port and gives back the port
pub fn start(build: impl FnOnce() -> Cpeak) -> u16 {
    let server = build().listen(("127.0.0.1", 0), || {});
    server.address().port()
}

pub struct TestRequest {
    port: u16,
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

pub fn request(port: u16, method: &str, path: &str) -> TestRequest {
    TestRequest {
        port,
        method: method.to_string(),
        path: path.to_string(),
        headers: Vec::new(),
        body: None,
    }
}

impl TestRequest {
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn json(mut self, value: &serde_json::Value) -> Self {
        self.headers
            .push(("Content-Type".to_string(), "application/json".to_string()));
        self.body = Some(serde_json::to_vec(value).unwrap());
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }

    // Like supertest's .query(): adds the query string to the path
    pub fn query(mut self, params: &[(&str, &str)]) -> Self {
        let query: Vec<String> = params
            .iter()
            .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
            .collect();
        self.path = format!("{}?{}", self.path, query.join("&"));
        self
    }

    pub fn send(self) -> TestResponse {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .unwrap();

        let mut head = format!(
            "{} {} HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n",
            self.method, self.path, self.port
        );
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if let Some(body) = &self.body {
            head.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        head.push_str("\r\n");

        // Send the head and the body in one go, like supertest does
        let mut raw_request = head.into_bytes();
        if let Some(body) = &self.body {
            raw_request.extend_from_slice(body);
        }
        stream.write_all(&raw_request).unwrap();

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        parse_response(&raw, self.method == "HEAD")
    }
}

pub struct TestResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl TestResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn headers_all(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
            .collect()
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|error| panic!("not JSON ({error}): {}", self.text()))
    }
}

pub fn parse_response(raw: &[u8], is_head: bool) -> TestResponse {
    let head_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("no end of headers in the response");
    let head = String::from_utf8_lossy(&raw[..head_end]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap();
    let status = status_line.split(' ').nth(1).unwrap().parse().unwrap();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();

    let rest = &raw[head_end + 4..];
    let is_chunked = headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("transfer-encoding") && value.eq_ignore_ascii_case("chunked")
    });
    let body = if is_head {
        Vec::new()
    } else if is_chunked {
        decode_chunked(rest)
    } else {
        rest.to_vec()
    };

    TestResponse {
        status,
        headers,
        body,
    }
}

fn decode_chunked(mut raw: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let line_end = raw.windows(2).position(|window| window == b"\r\n").unwrap();
        let size_line = String::from_utf8_lossy(&raw[..line_end]);
        let size = usize::from_str_radix(size_line.split(';').next().unwrap().trim(), 16).unwrap();
        raw = &raw[line_end + 2..];
        if size == 0 {
            return body;
        }
        body.extend_from_slice(&raw[..size]);
        raw = &raw[size + 2..];
    }
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}
