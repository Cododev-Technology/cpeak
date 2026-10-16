use std::time::Duration;

use crate::{CpeakRequest, CpeakResponse};

#[derive(Debug, Clone)]
pub struct CompressionOptions {
    // Responses with a Content-Length below this many bytes are sent uncompressed.
    // Below ~1KB, TCP/TLS framing overhead can outweigh the savings.
    pub threshold: usize,
    pub brotli: BrotliOptions,
    pub gzip: ZlibOptions,
    pub deflate: ZlibOptions,
}

impl Default for CompressionOptions {
    fn default() -> Self {
        Self {
            threshold: 1024,
            brotli: BrotliOptions::default(),
            gzip: ZlibOptions::default(),
            deflate: ZlibOptions::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BrotliOptions {
    // From 0 to 11. Default: 4 (11 is the max, which is really slow for live responses)
    pub quality: Option<u32>,
    // Base-2 logarithm of the sliding window size, from 10 to 24. Default: 19 (512KB)
    pub lgwin: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct ZlibOptions {
    // From 0 (no compression) to 9 (best compression). Default: 6
    pub level: Option<u32>,
}

pub(crate) type ResolvedCompressionConfig = CompressionOptions;

// For HTTPS. They're what you'd pass to `https.createServer()` in Node.js: the private key
// and the certificate (followed by the certificates that signed it, if there are any), both
// as the text of their PEM files.
#[derive(Clone, Default)]
pub struct HttpsOptions {
    pub key: String,
    pub cert: String,
}

// Printing the options shouldn't leak the private key (into your logs, for example)
impl std::fmt::Debug for HttpsOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpsOptions")
            .field("key", &"[hidden]")
            .field("cert", &self.cert)
            .finish()
    }
}

// Where the lines of the log go
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogOutput {
    Stdout,
    Stderr,
    // A file, appended to. A {day} in the path (like "logs/{day}/info.log") starts a new
    // file every day, and the folders are made as they're needed.
    File(String),
}

// The clock the dates of the log are written in
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogTimeZone {
    #[default]
    Utc,
    // The time zone of the machine
    Local,
}

#[derive(Debug, Clone)]
pub struct LogOptions {
    // Where cpeak::log("...") and the request lines go. Default: LogOutput::Stdout
    pub output: LogOutput,
    // Where cpeak::log(&error) goes. Default: LogOutput::Stderr
    pub errors: LogOutput,
    // Default: LogTimeZone::Utc
    pub time_zone: LogTimeZone,
    // Each worker thread collects its lines and hands them over to be written once it has this
    // many bytes, or once flush_every has passed. Defaults: 64KB and 100ms
    pub buffer_size: usize,
    pub flush_every: Duration,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            output: LogOutput::Stdout,
            errors: LogOutput::Stderr,
            time_zone: LogTimeZone::Utc,
            buffer_size: 64 * 1024,
            flush_every: Duration::from_millis(100),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RequestLogOptions {
    // What a line looks like. These get replaced with the values of the request:
    // {date} {iso} {ip} {method} {url} {path} {status} {status_message} {ms} {us} {bytes}
    // and {header:NAME} for any request header (like {header:user-agent})
    pub format: String,
    // Read the IP of the client from this header when it has a value (behind a proxy or a
    // load balancer, like "x-forwarded-for"). Default: None, the address of the connection
    pub ip_header: Option<String>,
    // Which requests get a line. Default: all of them. With more than one of these set, a
    // request has to pass every one.
    // Only the ones with a status of 400 and above
    pub only_errors: bool,
    // Only the ones that took longer than this
    pub slower_than: Option<Duration>,
    // Only 1 in this many
    pub sample: Option<u32>,
    // Your own rule: return true to leave a request out
    pub skip: Option<fn(&CpeakRequest, &CpeakResponse) -> bool>,
}

impl Default for RequestLogOptions {
    fn default() -> Self {
        Self {
            format: "{date} -- {ip} -- {method} {url} {status} {status_message} -- {ms} ms"
                .to_string(),
            ip_header: None,
            only_errors: false,
            slower_than: None,
            sample: None,
            skip: None,
        }
    }
}
