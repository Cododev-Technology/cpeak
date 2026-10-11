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
    // Base-2 logarithm of the sliding window size, from 10 to 24. Default: 22
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
