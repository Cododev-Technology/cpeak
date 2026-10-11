export interface CompressionOptions {
  // Responses with a Content-Length below this many bytes are sent uncompressed.
  // Below ~1KB, TCP/TLS framing overhead can outweigh the savings.
  threshold?: number;
  brotli?: import("node:zlib").BrotliOptions;
  gzip?: import("node:zlib").ZlibOptions;
  deflate?: import("node:zlib").ZlibOptions;
}

export type ResolvedCompressionConfig = Required<CompressionOptions>;
