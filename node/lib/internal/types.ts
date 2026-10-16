export interface CompressionOptions {
  // Responses with a Content-Length below this many bytes are sent uncompressed.
  // Below ~1KB, TCP/TLS framing overhead can outweigh the savings.
  threshold?: number;
  brotli?: import("node:zlib").BrotliOptions;
  gzip?: import("node:zlib").ZlibOptions;
  deflate?: import("node:zlib").ZlibOptions;
}

export type ResolvedCompressionConfig = Required<CompressionOptions>;

// Where the lines of the log go. A {day} in the path of a file (like "logs/{day}/info.log")
// starts a new file every day, and the folders are made as they're needed.
export type LogOutput = "stdout" | "stderr" | { file: string };

// The clock the dates of the log are written in
export type LogTimeZone = "utc" | "local";

export interface LogOptions {
  // Where log("...") and the request lines go. Default: "stdout"
  output?: LogOutput;
  // Where log(error) goes. Default: "stderr"
  errors?: LogOutput;
  // Default: "utc"
  timeZone?: LogTimeZone;
  // The lines are collected and written out together once there are this many bytes, or once
  // flushEvery (milliseconds) has passed. Defaults: 64KB and 100ms
  bufferSize?: number;
  flushEvery?: number;
}

export interface RequestLogOptions {
  // What a line looks like. These get replaced with the values of the request:
  // {date} {iso} {ip} {method} {url} {path} {status} {status_message} {ms} {us} {bytes}
  // and {header:NAME} for any request header (like {header:user-agent})
  format?: string;
  // Read the IP of the client from this header when it has a value (behind a proxy or a
  // load balancer, like "x-forwarded-for"). Default: the address of the connection
  ipHeader?: string;
  // Which requests get a line. Default: all of them. With more than one of these set, a
  // request has to pass every one.
  // Only the ones with a status of 400 and above
  onlyErrors?: boolean;
  // Only the ones that took longer than this many milliseconds
  slowerThan?: number;
  // Only 1 in this many
  sample?: number;
  // Your own rule: return true to leave a request out
  skip?: (
    req: import("../types").CpeakRequest,
    res: import("../types").CpeakResponse
  ) => boolean;
}
