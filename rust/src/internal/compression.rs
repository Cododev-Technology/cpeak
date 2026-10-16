use std::collections::HashMap;
use std::io::{self, Write};
use std::pin::Pin;

use bytes::{Bytes, BytesMut};
use flate2::write::{GzEncoder, ZlibEncoder};
use flate2::Compression;
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::internal::errors::CpeakError;
use crate::internal::files::{self, File};
use crate::internal::types::{CompressionOptions, ResolvedCompressionConfig};
use crate::types::LocalBoxFuture;
use crate::CpeakResponse;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Br,
    Gzip,
    Deflate,
}

impl Encoding {
    fn as_str(self) -> &'static str {
        match self {
            Encoding::Br => "br",
            Encoding::Gzip => "gzip",
            Encoding::Deflate => "deflate",
        }
    }
}

// Same as the /text|json|javascript|css|xml|svg/i regex of the Node.js version
const COMPRESSIBLE_TYPE: [&str; 6] = ["text", "json", "javascript", "css", "xml", "svg"];

pub(crate) fn is_compressible_type(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    COMPRESSIBLE_TYPE.iter().any(|kind| mime.contains(kind))
}

// Same as the /(?:^|,)\s*no-transform\s*(?:,|$)/i regex of the Node.js version
fn is_no_transform(cache_control: &str) -> bool {
    cache_control
        .split(',')
        .any(|directive| directive.trim().eq_ignore_ascii_case("no-transform"))
}

// A body that's already in memory and smaller than this goes out as gzip when the client takes
// both gzip and brotli. Brotli sets up a table of half a megabyte for every response before it
// compresses a single byte, so on a body of a few KB it takes two to three times as long as
// gzip to save 2 to 7%. 64KB is one chunk of a streamed body (CHUNK_SIZE below).
const SMALL_BODY: usize = 64 * 1024;

// Parse Accept-Encoding and pick a compression algorithm the server supports.
// Handles q=0 to disable an algorithm. Cpeak's preference is br > gzip > deflate, and for a
// small body (see SMALL_BODY) gzip > br > deflate.
fn pick_encoding(header: &str, small_body: bool) -> Option<Encoding> {
    let preference = if small_body {
        [Encoding::Gzip, Encoding::Br, Encoding::Deflate]
    } else {
        [Encoding::Br, Encoding::Gzip, Encoding::Deflate]
    };
    pick_from(header, &preference)
}

// The first of these algorithms that the client accepts
fn pick_from(header: &str, preference: &[Encoding]) -> Option<Encoding> {
    if header.is_empty() {
        return None;
    }

    let mut accepted: HashMap<String, f64> = HashMap::new();
    let mut wildcard: Option<f64> = None;

    for part in header.split(',') {
        let mut pieces = part.trim().split(';');
        let raw_name = pieces.next().unwrap_or("");
        let name = raw_name.trim().to_lowercase();
        if name.is_empty() {
            continue;
        }

        let mut q = 1.0;
        for p in pieces {
            let p = p.trim();
            let is_q = p
                .get(..2)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("q="));
            if is_q && p.len() > 2 {
                let value = &p[2..];
                if value.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
                    q = value.parse().unwrap_or(f64::NAN);
                }
            }
        }
        if q.is_nan() {
            q = 0.0;
        }

        if name == "*" {
            wildcard = Some(q);
        } else {
            accepted.insert(name, q);
        }
    }

    let try_pick = |encoding: Encoding| -> bool {
        let q = accepted.get(encoding.as_str()).copied().or(wildcard);
        q.is_some_and(|q| q > 0.0)
    };

    preference
        .iter()
        .copied()
        .find(|encoding| try_pick(*encoding))
}

// Handling the Vary HTTP header
fn append_vary(res: &mut CpeakResponse, value: &str) {
    let Some(existing) = res
        .get_header("Vary")
        .filter(|existing| !existing.is_empty())
    else {
        res.set_header("Vary", value);
        return;
    };
    let current: Vec<String> = existing
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if current
        .iter()
        .any(|v| v == "*" || v.eq_ignore_ascii_case(value))
    {
        return;
    }
    let mut updated = current;
    updated.push(value.to_string());
    res.set_header("Vary", updated.join(", "));
}

// Brotli options. The max quality is 11, which is really slow for live
// responses. We go with 4 unless the developer specifies otherwise.
//
// The window is 512KB (lgwin 19), like nginx's. Brotli sets aside twice its window in memory
// for every response over 64KB, so with the 4MB window that brotli itself defaults to, a
// 75KB response takes 8MB to compress. Only a body bigger than the window gets smaller with
// a bigger one: a 2MB JavaScript bundle by about 3%.
fn brotli_opts_for(config: &ResolvedCompressionConfig) -> (u32, u32) {
    let quality = config.brotli.quality.unwrap_or(4).min(11);
    let lgwin = config.brotli.lgwin.unwrap_or(19).clamp(10, 24);
    (quality, lgwin)
}

// Our compressors write into an in-memory buffer. After feeding each chunk in,
// we take out whatever compressed output is ready and send it to the client.
enum Compressor {
    Br(Box<brotli::CompressorWriter<Vec<u8>>>),
    Gzip(GzEncoder<Vec<u8>>),
    Deflate(ZlibEncoder<Vec<u8>>),
}

fn create_compressor(encoding: Encoding, config: &ResolvedCompressionConfig) -> Compressor {
    match encoding {
        Encoding::Br => {
            let (quality, lgwin) = brotli_opts_for(config);
            Compressor::Br(Box::new(brotli::CompressorWriter::new(
                Vec::new(),
                16 * 1024,
                quality,
                lgwin,
            )))
        }
        Encoding::Gzip => Compressor::Gzip(GzEncoder::new(
            Vec::new(),
            Compression::new(config.gzip.level.unwrap_or(6).min(9)),
        )),
        // HTTP's "deflate" is the zlib format (RFC 1950), which is what Node.js's createDeflate produces too
        Encoding::Deflate => Compressor::Deflate(ZlibEncoder::new(
            Vec::new(),
            Compression::new(config.deflate.level.unwrap_or(6).min(9)),
        )),
    }
}

impl Compressor {
    // Feed in a chunk and take out the compressed output that's ready so far
    fn write(&mut self, chunk: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Compressor::Br(writer) => {
                writer.write_all(chunk)?;
                Ok(std::mem::take(writer.get_mut()))
            }
            Compressor::Gzip(encoder) => {
                encoder.write_all(chunk)?;
                Ok(std::mem::take(encoder.get_mut()))
            }
            Compressor::Deflate(encoder) => {
                encoder.write_all(chunk)?;
                Ok(std::mem::take(encoder.get_mut()))
            }
        }
    }

    // Finish the compressed stream and take out the rest of the output
    fn finish(self) -> io::Result<Vec<u8>> {
        match self {
            Compressor::Br(writer) => Ok(writer.into_inner()),
            Compressor::Gzip(encoder) => encoder.finish(),
            Compressor::Deflate(encoder) => encoder.finish(),
        }
    }
}

// Decides what to do with this response
// A `size` of None means we don't know it ahead of time (the Node.js version uses Infinity for that).
// `in_memory` is true when the whole body is in a buffer, and false when it's a stream.
fn negotiate(
    res: &CpeakResponse,
    mime: &str,
    size: Option<u64>,
    in_memory: bool,
    config: &ResolvedCompressionConfig,
) -> (Option<Encoding>, bool) {
    if !is_eligible(res, mime) {
        return (None, false);
    }

    if size.is_some_and(|size| size < config.threshold as u64) {
        return (None, true);
    }

    let small_body = in_memory && size.is_some_and(|size| size < SMALL_BODY as u64);
    let encoding = pick_encoding(&res.req_accept_encoding, small_body);
    (encoding, true)
}

// Whether this response can go out compressed at all
fn is_eligible(res: &CpeakResponse, mime: &str) -> bool {
    // Whether this content type is worth trying to compress at all.
    // Some types are already compressed and don't compress well.
    if !is_compressible_type(mime) {
        return false;
    }

    if res.req_is_head {
        return false;
    }

    // RFC specification: don't transform responses that ask not to be transformed.
    if res.get_header("Cache-Control").is_some_and(is_no_transform) {
        return false;
    }

    !res.get_header("Content-Encoding")
        .is_some_and(|existing| !existing.is_empty() && existing != "identity")
}

// Sends the copy of a file that was compressed ahead of time (see cpeak::precompress), when
// there is one that the client takes. It goes out exactly as it is on the disk, so nothing
// gets compressed. False when there is none for this client: the file then goes out the way
// it always did.
pub(crate) async fn send_precompressed(
    res: &mut CpeakResponse,
    path: &str,
    mime: &str,
) -> Result<bool, CpeakError> {
    if !is_eligible(res, mime) {
        return Ok(false);
    }

    let copies = files::precompressed(path);
    let made: &[Encoding] = match (copies.br, copies.gzip) {
        (true, true) => &[Encoding::Br, Encoding::Gzip],
        (true, false) => &[Encoding::Br],
        (false, true) => &[Encoding::Gzip],
        (false, false) => return Ok(false),
    };

    // From here on, what the client gets depends on what it accepts
    append_vary(res, "Accept-Encoding");
    let Some(encoding) = pick_from(&res.req_accept_encoding, made) else {
        return Ok(false);
    };
    let copy_path = match encoding {
        Encoding::Br => format!("{path}.br"),
        _ => format!("{path}.gz"),
    };

    // (a copy that just went away is no reason to fail: the file itself is still there)
    match files::read(&copy_path) {
        Ok(File::Content(content, _)) => {
            res.set_header("Content-Type", mime);
            res.set_header("Content-Encoding", encoding.as_str());
            res.set_header("Content-Length", content.len());
            res.end(content)?;
        }
        Ok(File::TooBig(size)) => {
            let Ok(copy) = tokio::fs::File::open(&copy_path).await else {
                return Ok(false);
            };
            res.set_header("Content-Type", mime);
            res.set_header("Content-Encoding", encoding.as_str());
            res.set_header("Content-Length", size);
            pipe(Readable::new(copy), res).await?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

// The strongest compression there is, for the copies that cpeak::precompress() makes. It's
// far too slow for a response that someone is waiting for, and just right for a build step.
pub(crate) fn compress_as_small_as_possible(buffer: &[u8], extension: &str) -> io::Result<Vec<u8>> {
    if extension == "br" {
        let mut writer = brotli::CompressorWriter::new(Vec::new(), 64 * 1024, 11, 24);
        writer.write_all(buffer)?;
        return Ok(writer.into_inner());
    }

    let mut compressor = libdeflater::Compressor::new(libdeflater::CompressionLvl::best());
    let mut compressed = vec![0; compressor.gzip_compress_bound(buffer.len())];
    let written = compressor
        .gzip_compress(buffer, &mut compressed)
        .map_err(io::Error::other)?;
    compressed.truncate(written);
    Ok(compressed)
}

// Resolves compression options into a complete config. Called once at Cpeak construction.
pub(crate) fn resolve_compression_options(input: CompressionOptions) -> ResolvedCompressionConfig {
    input
}

// The final point used by res.compress, res.json, res.send_file and res.render
// when compression is enabled by the developer.
pub(crate) async fn compress_and_send(
    res: &mut CpeakResponse,
    mime: &str,
    body: Body,
    config: &ResolvedCompressionConfig,
    size: Option<u64>,
) -> Result<(), CpeakError> {
    let mut readable = match body {
        Body::Buffer(buffer) => return compress_and_send_buffer(res, mime, buffer, config),
        Body::Readable(readable) => readable,
    };

    res.set_header("Content-Type", mime);

    let (encoding, eligible) = negotiate(res, mime, size, false, config);

    let Some(encoding) = encoding else {
        if eligible {
            append_vary(res, "Accept-Encoding");
        }
        if let Some(size) = size {
            res.set_header("Content-Length", size);
        }
        return pipe(readable, res).await;
    };

    res.set_header("Content-Encoding", encoding.as_str());
    append_vary(res, "Accept-Encoding");

    let mut compressor = create_compressor(encoding, config);
    while let Some(chunk) = readable.next_chunk().await? {
        let compressed = compressor.write(&chunk)?;
        if !compressed.is_empty() {
            res.write(compressed).await?;
        }
    }
    res.end(compressor.finish()?)
}

// The same as compress_and_send, for a body that's already in memory. We compress it in one go,
// so no waiting is needed, and we can also tell the client the final Content-Length.
pub(crate) fn compress_and_send_buffer(
    res: &mut CpeakResponse,
    mime: &str,
    buffer: Bytes,
    config: &ResolvedCompressionConfig,
) -> Result<(), CpeakError> {
    res.set_header("Content-Type", mime);

    let (encoding, eligible) = negotiate(res, mime, Some(buffer.len() as u64), true, config);

    let Some(encoding) = encoding else {
        if eligible {
            append_vary(res, "Accept-Encoding");
        }
        res.set_header("Content-Length", buffer.len());
        return res.end(buffer);
    };

    res.set_header("Content-Encoding", encoding.as_str());
    append_vary(res, "Accept-Encoding");

    let compressed = compress_buffer(encoding, &buffer, config)?;
    res.set_header("Content-Length", compressed.len());
    res.end(compressed)
}

// Compress a whole body in one go
fn compress_buffer(
    encoding: Encoding,
    buffer: &[u8],
    config: &ResolvedCompressionConfig,
) -> io::Result<Vec<u8>> {
    match encoding {
        Encoding::Gzip => deflate_in_one_go(buffer, config.gzip.level, true),
        Encoding::Deflate => deflate_in_one_go(buffer, config.deflate.level, false),
        Encoding::Br => {
            let mut compressor = create_compressor(encoding, config);
            let mut compressed = compressor.write(buffer)?;
            compressed.extend(compressor.finish()?);
            Ok(compressed)
        }
    }
}

// gzip and deflate for a body that's already in memory, with libdeflate: it only compresses
// whole buffers, and does that faster than a zlib that has to be ready for a stream. Each
// worker thread keeps one compressor (for the level it used last), so a response doesn't
// have to set one up.
fn deflate_in_one_go(buffer: &[u8], level: Option<u32>, gzip: bool) -> io::Result<Vec<u8>> {
    use std::cell::RefCell;

    use libdeflater::{CompressionLvl, Compressor};

    thread_local! {
        static COMPRESSOR: RefCell<Option<(u32, Compressor)>> = const { RefCell::new(None) };
    }

    let level = level.unwrap_or(6).min(9);
    COMPRESSOR.with(|kept| {
        let mut kept = kept.borrow_mut();
        if !matches!(&*kept, Some((kept_level, _)) if *kept_level == level) {
            let compression_level = CompressionLvl::new(level as i32)
                .map_err(|_| io::Error::other("invalid compression level"))?;
            *kept = Some((level, Compressor::new(compression_level)));
        }
        let (_, compressor) = kept.as_mut().expect("the compressor was just made");

        // HTTP's "deflate" is the zlib format (RFC 1950)
        let mut compressed = vec![
            0;
            if gzip {
                compressor.gzip_compress_bound(buffer.len())
            } else {
                compressor.zlib_compress_bound(buffer.len())
            }
        ];
        let written = if gzip {
            compressor.gzip_compress(buffer, &mut compressed)
        } else {
            compressor.zlib_compress(buffer, &mut compressed)
        }
        .map_err(io::Error::other)?;
        compressed.truncate(written);
        Ok(compressed)
    })
}

// Pipes a readable stream into the response, respecting backpressure
pub(crate) async fn pipe(
    mut readable: Readable,
    res: &mut CpeakResponse,
) -> Result<(), CpeakError> {
    while let Some(chunk) = readable.next_chunk().await? {
        res.write(chunk).await?;
    }
    res.end(Bytes::new())
}

// The body you pass to res.compress(): a buffer (like a Node.js Buffer or string), or a Readable stream.
pub enum Body {
    Buffer(Bytes),
    Readable(Readable),
}

impl From<Bytes> for Body {
    fn from(buffer: Bytes) -> Self {
        Body::Buffer(buffer)
    }
}

impl From<Vec<u8>> for Body {
    fn from(buffer: Vec<u8>) -> Self {
        Body::Buffer(Bytes::from(buffer))
    }
}

impl From<&'static [u8]> for Body {
    fn from(buffer: &'static [u8]) -> Self {
        Body::Buffer(Bytes::from_static(buffer))
    }
}

impl From<String> for Body {
    fn from(string: String) -> Self {
        Body::Buffer(Bytes::from(string))
    }
}

impl From<&'static str> for Body {
    fn from(string: &'static str) -> Self {
        Body::Buffer(Bytes::from_static(string.as_bytes()))
    }
}

impl From<Readable> for Body {
    fn from(readable: Readable) -> Self {
        Body::Readable(readable)
    }
}

// A readable stream of bytes, like a Node.js Readable.
// Wrap anything that implements tokio's AsyncRead (a file, a socket, ...) with `Readable::new()`.
pub struct Readable {
    source: Box<dyn ChunkSource>,
}

impl Readable {
    pub fn new(reader: impl AsyncRead + 'static) -> Self {
        Self::from_source(ReaderSource {
            reader: Box::pin(reader),
        })
    }

    pub(crate) fn from_source(source: impl ChunkSource + 'static) -> Self {
        Self {
            source: Box::new(source),
        }
    }

    // The next chunk of the stream, or None once the stream has ended
    pub(crate) async fn next_chunk(&mut self) -> io::Result<Option<Bytes>> {
        self.source.next_chunk().await
    }
}

pub(crate) trait ChunkSource {
    fn next_chunk(&mut self) -> LocalBoxFuture<'_, io::Result<Option<Bytes>>>;
}

// The same chunk size that Node.js's fs.createReadStream uses by default (its highWaterMark)
const CHUNK_SIZE: usize = 64 * 1024;

struct ReaderSource {
    reader: Pin<Box<dyn AsyncRead>>,
}

impl ChunkSource for ReaderSource {
    fn next_chunk(&mut self) -> LocalBoxFuture<'_, io::Result<Option<Bytes>>> {
        Box::pin(async move {
            let mut buffer = BytesMut::with_capacity(CHUNK_SIZE);
            let bytes_read = self.reader.read_buf(&mut buffer).await?;
            if bytes_read == 0 {
                return Ok(None);
            }
            Ok(Some(buffer.freeze()))
        })
    }
}
