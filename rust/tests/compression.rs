// Compression
mod common;

use std::io::{Cursor, Read, Write};

use common::{request, serve, TestResponse};
use cpeak::{cpeak, CompressionOptions, Cpeak, CpeakOptions, Readable};
use serde_json::json;

fn large() -> String {
    "x".repeat(2048)
}

fn small() -> String {
    "x".repeat(50)
}

// A body of more than 64KB: from that size on brotli is preferred over gzip
fn big() -> String {
    "All work and no play makes Jack a dull boy. ".repeat(2000)
}

// Bytes that don't compress well, so we don't need a random number crate
fn random_bytes(size: usize, mut seed: u64) -> Vec<u8> {
    (0..size)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect()
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn gunzip(data: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(data)
        .read_to_end(&mut decoded)
        .unwrap();
    decoded
}

fn big_text_file() -> String {
    let path = format!("./target/cpeak-big-{}.txt", std::process::id());
    std::fs::write(
        &path,
        "All work and no play makes Jack a dull boy.\n".repeat(500),
    )
    .unwrap();
    path
}

fn server() -> u16 {
    serve(|| {
        let mut server = Cpeak::new(CpeakOptions {
            compression: Some(CompressionOptions::default()),
            ..Default::default()
        });

        server.route("get", "/large-json", async |_req, res| {
            res.status(200).json(json!({ "data": large() }))
        });

        server.route("head", "/large-json", async |_req, res| {
            res.status(200).json(json!({ "data": large() }))
        });

        server.route("get", "/big-json", async |_req, res| {
            res.status(200).json(json!({ "data": big() }))
        });

        server.route("get", "/small-json", async |_req, res| {
            res.status(200).json(json!({ "data": small() }))
        });

        server.route("get", "/png", async |_req, res| {
            res.status_code = 200;
            res.compress("image/png", vec![0u8; 2048], None).await
        });

        server.route("get", "/no-transform", async |_req, res| {
            res.set_header("Cache-Control", "no-transform");
            res.status(200).json(json!({ "data": large() }))
        });

        server.route("get", "/already-encoded", async |_req, res| {
            let pre_gzipped = gzip(&random_bytes(2048, 42));
            res.set_header("Content-Type", "application/json");
            res.set_header("Content-Encoding", "gzip");
            res.status_code = 200;
            res.end(pre_gzipped)
        });

        server.route("get", "/with-vary", async |_req, res| {
            res.set_header("Vary", "Cookie");
            res.status(200).json(json!({ "data": large() }))
        });

        server.route("get", "/streamed-large", async |_req, res| {
            res.status_code = 200;
            let payload = "a".repeat(100_000).into_bytes();
            let size = payload.len() as u64;
            res.compress("text/css", Readable::new(Cursor::new(payload)), size)
                .await
        });

        server.route("get", "/streamed-huge", async |_req, res| {
            // 4 MB of random bytes to exercise backpressure.
            res.status_code = 200;
            let chunks = random_bytes(16 * 1024 * 256, 7);
            res.compress(
                "application/javascript",
                Readable::new(Cursor::new(chunks)),
                16 * 1024 * 256,
            )
            .await
        });

        server.route("get", "/big-file", async |_req, res| {
            res.send_file(&big_text_file(), None).await
        });

        server
    })
}

fn get(path: &str, accept_encoding: &str) -> TestResponse {
    request(server(), "GET", path)
        .header("Accept-Encoding", accept_encoding)
        .send()
}

#[test]
fn should_compress_with_gzip_and_tell_the_client_the_final_content_length() {
    let res = get("/large-json", "gzip");
    assert_eq!(res.status, 200);
    assert_eq!(res.header("content-encoding"), Some("gzip"));
    assert_eq!(
        res.header("content-length"),
        Some(res.body.len().to_string().as_str())
    );
    let decoded: serde_json::Value = serde_json::from_slice(&gunzip(&res.body)).unwrap();
    assert_eq!(decoded, json!({ "data": large() }));
}

#[test]
fn should_prefer_brotli_over_gzip_for_a_big_body_when_both_are_accepted() {
    let res = get("/big-json", "br, gzip");
    assert_eq!(res.header("content-encoding"), Some("br"));
    let mut decoded = Vec::new();
    brotli::BrotliDecompress(&mut &res.body[..], &mut decoded).unwrap();
    let decoded: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(decoded, json!({ "data": big() }));
}

// Under 64KB, brotli takes much longer than gzip to save very little
#[test]
fn should_prefer_gzip_over_brotli_for_a_small_body_when_both_are_accepted() {
    for accepted in ["br, gzip", "gzip, br", "gzip, deflate, br, zstd"] {
        let res = get("/large-json", accepted);
        assert_eq!(res.header("content-encoding"), Some("gzip"));
        assert_eq!(
            res.header("content-length"),
            Some(res.body.len().to_string().as_str())
        );
        let decoded: serde_json::Value = serde_json::from_slice(&gunzip(&res.body)).unwrap();
        assert_eq!(decoded, json!({ "data": large() }));
    }
}

#[test]
fn should_use_brotli_for_a_small_body_when_the_client_only_accepts_brotli() {
    let res = get("/large-json", "br");
    assert_eq!(res.header("content-encoding"), Some("br"));
    let mut decoded = Vec::new();
    brotli::BrotliDecompress(&mut &res.body[..], &mut decoded).unwrap();
    let decoded: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(decoded, json!({ "data": large() }));
}

#[test]
fn should_compress_a_big_body_with_gzip_when_the_client_only_accepts_gzip() {
    let res = get("/big-json", "gzip");
    assert_eq!(res.header("content-encoding"), Some("gzip"));
    let decoded: serde_json::Value = serde_json::from_slice(&gunzip(&res.body)).unwrap();
    assert_eq!(decoded, json!({ "data": big() }));
}

#[test]
fn should_fall_back_to_deflate_when_client_only_accepts_deflate() {
    let res = get("/large-json", "deflate");
    assert_eq!(res.header("content-encoding"), Some("deflate"));
    let mut decoded = Vec::new();
    flate2::read::ZlibDecoder::new(&res.body[..])
        .read_to_end(&mut decoded)
        .unwrap();
    let decoded: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(decoded, json!({ "data": large() }));
}

#[test]
fn should_append_accept_encoding_to_an_existing_vary_header() {
    let res = get("/with-vary", "gzip");
    assert_eq!(res.header("content-encoding"), Some("gzip"));
    let vary = res.header("vary").unwrap_or_default().to_lowercase();
    assert!(
        vary.contains("cookie"),
        "expected Cookie in Vary, got: {vary}"
    );
    assert!(
        vary.contains("accept-encoding"),
        "expected Accept-Encoding in Vary, got: {vary}"
    );
}

#[test]
fn should_send_identity_response_when_client_only_accepts_identity() {
    let res = get("/large-json", "identity");
    assert_eq!(res.header("content-encoding"), None);
    assert_eq!(res.json(), json!({ "data": large() }));
}

#[test]
fn should_respect_q_0_to_explicitly_disable_all_encodings() {
    let res = get("/large-json", "gzip;q=0, deflate;q=0, br;q=0");
    assert_eq!(res.header("content-encoding"), None);
}

#[test]
fn should_skip_compression_but_still_set_vary_when_body_is_below_threshold() {
    let res = get("/small-json", "gzip");
    assert_eq!(res.header("content-encoding"), None);
    let vary = res.header("vary").unwrap_or_default().to_lowercase();
    assert!(
        vary.contains("accept-encoding"),
        "expected Accept-Encoding in Vary, got: {vary}"
    );
}

#[test]
fn should_skip_compression_for_head_requests() {
    let res = request(server(), "HEAD", "/large-json")
        .header("Accept-Encoding", "gzip")
        .send();
    assert_eq!(res.status, 200);
    assert_eq!(res.header("content-encoding"), None);
}

#[test]
fn should_respect_cache_control_no_transform() {
    let res = get("/no-transform", "gzip");
    assert_eq!(res.header("content-encoding"), None);
}

#[test]
fn should_not_double_encode_a_response_that_is_already_compressed() {
    let res = get("/already-encoded", "gzip");
    assert_eq!(res.header("content-encoding"), Some("gzip"));
    assert_eq!(gunzip(&res.body).len(), 2048);
}

#[test]
fn should_not_compress_or_set_vary_for_non_compressible_content_types() {
    let res = get("/png", "gzip");
    assert_eq!(res.header("content-encoding"), None);
    let vary = res.header("vary").unwrap_or_default().to_lowercase();
    assert!(
        !vary.contains("accept-encoding"),
        "did not expect Accept-Encoding in Vary, got: {vary}"
    );
}

#[test]
fn should_compress_a_large_streamed_body_via_res_compress() {
    let res = get("/streamed-large", "gzip");
    assert_eq!(res.status, 200);
    assert_eq!(res.header("content-encoding"), Some("gzip"));
    assert_eq!(res.header("transfer-encoding"), Some("chunked"));
    let decoded = String::from_utf8(gunzip(&res.body)).unwrap();
    assert_eq!(decoded.len(), 100_000);
    assert!(decoded.starts_with('a'));
}

#[test]
fn should_handle_backpressure_on_a_huge_streamed_body_without_hanging() {
    let res = get("/streamed-huge", "gzip");
    assert_eq!(res.status, 200);
    assert_eq!(res.header("content-encoding"), Some("gzip"));
    assert_eq!(gunzip(&res.body).len(), 16 * 1024 * 256);
}

#[test]
fn should_compress_files_sent_with_send_file() {
    let res = get("/big-file", "gzip");
    assert_eq!(res.status, 200);
    assert_eq!(res.header("content-type"), Some("text/plain"));
    assert_eq!(res.header("content-encoding"), Some("gzip"));
    assert_eq!(gunzip(&res.body).len(), 44 * 500);
}

fn server2() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.route("get", "/compress", async |_req, res| {
            res.compress("text/plain", "hello", None).await
        });

        server.handle_err(async |error, _req, res| {
            res.status(500).json(json!({ "code": error.code }))
        });

        server
    })
}

#[test]
fn should_throw_compression_not_enabled_for_res_compress_without_the_option() {
    let res = request(server2(), "GET", "/compress").send();
    assert_eq!(res.status, 500);
    assert_eq!(res.json()["code"], "CPEAK_ERR_COMPRESSION_NOT_ENABLED");
}
