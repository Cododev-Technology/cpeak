import assert from "node:assert";
import http from "node:http";
import zlib from "node:zlib";
import { Readable } from "node:stream";
import { Buffer } from "node:buffer";
import crypto from "node:crypto";
import cpeak from "../lib/index";
import type { Cpeak, CpeakRequest, CpeakResponse } from "../lib/types";

const PORT = 7543;

const LARGE = "x".repeat(2048);
const SMALL = "x".repeat(50);

// Bypass supertest so we can set Accept-Encoding precisely (q=0, identity, etc.)
// and read raw bytes without auto-decompression.
function rawRequest(opts: {
  path: string;
  method?: string;
  headers?: Record<string, string>;
}): Promise<{
  statusCode: number;
  headers: http.IncomingHttpHeaders;
  body: Buffer;
}> {
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        host: "localhost",
        port: PORT,
        path: opts.path,
        method: opts.method ?? "GET",
        headers: opts.headers
      },
      (res) => {
        const chunks: Buffer[] = [];
        res.on("data", (c) => chunks.push(c));
        res.on("end", () =>
          resolve({
            statusCode: res.statusCode!,
            headers: res.headers,
            body: Buffer.concat(chunks)
          })
        );
        res.on("error", reject);
      }
    );
    req.on("error", reject);
    req.end();
  });
}

describe("compression middleware", function () {
  let server: Cpeak;

  before(function (done) {
    server = cpeak({ compression: true });

    server.route(
      "get",
      "/large-json",
      (_req: CpeakRequest, res: CpeakResponse) => {
        res.status(200).json({ data: LARGE });
      }
    );

    server.route(
      "get",
      "/small-json",
      (_req: CpeakRequest, res: CpeakResponse) => {
        res.status(200).json({ data: SMALL });
      }
    );

    server.route(
      "get",
      "/png",
      async (_req: CpeakRequest, res: CpeakResponse) => {
        res.statusCode = 200;
        await res.compress("image/png", Buffer.alloc(2048));
      }
    );

    server.route(
      "get",
      "/no-transform",
      (_req: CpeakRequest, res: CpeakResponse) => {
        res.setHeader("Cache-Control", "no-transform");
        res.status(200).json({ data: LARGE });
      }
    );

    const preGzipped = zlib.gzipSync(crypto.randomBytes(2048));
    server.route(
      "get",
      "/already-encoded",
      (_req: CpeakRequest, res: CpeakResponse) => {
        res.setHeader("Content-Type", "application/json");
        res.setHeader("Content-Encoding", "gzip");
        res.statusCode = 200;
        res.end(preGzipped);
      }
    );

    server.route(
      "get",
      "/with-vary",
      (_req: CpeakRequest, res: CpeakResponse) => {
        res.setHeader("Vary", "Cookie");
        res.status(200).json({ data: LARGE });
      }
    );

    server.route(
      "get",
      "/streamed-large",
      async (_req: CpeakRequest, res: CpeakResponse) => {
        res.statusCode = 200;
        const payload = Buffer.from("a".repeat(100_000), "utf8");
        await res.compress(
          "text/css",
          Readable.from([payload]),
          payload.length
        );
      }
    );

    server.route(
      "get",
      "/streamed-huge",
      async (_req: CpeakRequest, res: CpeakResponse) => {
        // 4 MB in 16 KB random-byte chunks to exercise backpressure.
        res.statusCode = 200;
        const chunkSize = 16 * 1024;
        const chunks = Array.from({ length: 256 }, () =>
          crypto.randomBytes(chunkSize)
        );
        await res.compress(
          "application/javascript",
          Readable.from(chunks),
          chunkSize * 256
        );
      }
    );

    server.listen(PORT, done);
  });

  after(function (done) {
    server.close(done);
  });

  it("should compress with gzip and use chunked transfer encoding", async function () {
    const res = await rawRequest({
      path: "/large-json",
      headers: { "Accept-Encoding": "gzip" }
    });
    assert.strictEqual(res.statusCode, 200);
    assert.strictEqual(res.headers["content-encoding"], "gzip");
    assert.strictEqual(res.headers["content-length"], undefined);
    assert.strictEqual(res.headers["transfer-encoding"], "chunked");
    assert.deepStrictEqual(
      JSON.parse(zlib.gunzipSync(res.body).toString("utf8")),
      { data: LARGE }
    );
  });

  it("should prefer brotli over gzip when both are accepted", async function () {
    const res = await rawRequest({
      path: "/large-json",
      headers: { "Accept-Encoding": "br, gzip" }
    });
    assert.strictEqual(res.headers["content-encoding"], "br");
    assert.deepStrictEqual(
      JSON.parse(zlib.brotliDecompressSync(res.body).toString("utf8")),
      { data: LARGE }
    );
  });

  it("should fall back to deflate when client only accepts deflate", async function () {
    const res = await rawRequest({
      path: "/large-json",
      headers: { "Accept-Encoding": "deflate" }
    });
    assert.strictEqual(res.headers["content-encoding"], "deflate");
    assert.deepStrictEqual(
      JSON.parse(zlib.inflateSync(res.body).toString("utf8")),
      { data: LARGE }
    );
  });

  it("should append Accept-Encoding to an existing Vary header", async function () {
    const res = await rawRequest({
      path: "/with-vary",
      headers: { "Accept-Encoding": "gzip" }
    });
    assert.strictEqual(res.headers["content-encoding"], "gzip");
    const vary = String(res.headers["vary"]);
    assert.ok(/Cookie/i.test(vary), `expected Cookie in Vary, got: ${vary}`);
    assert.ok(
      /Accept-Encoding/i.test(vary),
      `expected Accept-Encoding in Vary, got: ${vary}`
    );
  });

  it("should send identity response when client only accepts identity", async function () {
    const res = await rawRequest({
      path: "/large-json",
      headers: { "Accept-Encoding": "identity" }
    });
    assert.strictEqual(res.headers["content-encoding"], undefined);
    assert.deepStrictEqual(JSON.parse(res.body.toString("utf8")), {
      data: LARGE
    });
  });

  it("should respect q=0 to explicitly disable all encodings", async function () {
    const res = await rawRequest({
      path: "/large-json",
      headers: { "Accept-Encoding": "gzip;q=0, deflate;q=0, br;q=0" }
    });
    assert.strictEqual(res.headers["content-encoding"], undefined);
  });

  it("should skip compression but still set Vary when body is below threshold", async function () {
    const res = await rawRequest({
      path: "/small-json",
      headers: { "Accept-Encoding": "gzip" }
    });
    assert.strictEqual(res.headers["content-encoding"], undefined);
    const vary = String(res.headers["vary"] ?? "");
    assert.ok(
      /Accept-Encoding/i.test(vary),
      `expected Accept-Encoding in Vary, got: ${vary}`
    );
  });

  it("should skip compression for HEAD requests", async function () {
    const res = await rawRequest({
      path: "/large-json",
      method: "HEAD",
      headers: { "Accept-Encoding": "gzip" }
    });
    assert.strictEqual(res.headers["content-encoding"], undefined);
  });

  it("should respect Cache-Control: no-transform", async function () {
    const res = await rawRequest({
      path: "/no-transform",
      headers: { "Accept-Encoding": "gzip" }
    });
    assert.strictEqual(res.headers["content-encoding"], undefined);
  });

  it("should not double-encode a response that is already compressed", async function () {
    const res = await rawRequest({
      path: "/already-encoded",
      headers: { "Accept-Encoding": "gzip" }
    });
    assert.strictEqual(res.headers["content-encoding"], "gzip");
    assert.strictEqual(zlib.gunzipSync(res.body).length, 2048);
  });

  it("should not compress or set Vary for non-compressible content types", async function () {
    const res = await rawRequest({
      path: "/png",
      headers: { "Accept-Encoding": "gzip" }
    });
    assert.strictEqual(res.headers["content-encoding"], undefined);
    const vary = String(res.headers["vary"] ?? "");
    assert.ok(
      !/Accept-Encoding/i.test(vary),
      `did not expect Accept-Encoding in Vary, got: ${vary}`
    );
  });

  it("should compress a large streamed body via res.compress", async function () {
    this.timeout(5000);
    const res = await rawRequest({
      path: "/streamed-large",
      headers: { "Accept-Encoding": "gzip" }
    });
    assert.strictEqual(res.statusCode, 200);
    assert.strictEqual(res.headers["content-encoding"], "gzip");
    const decoded = zlib.gunzipSync(res.body).toString("utf8");
    assert.strictEqual(decoded.length, 100_000);
    assert.strictEqual(decoded[0], "a");
  });

  it("should handle backpressure on a huge streamed body without hanging", async function () {
    this.timeout(15000);
    const res = await rawRequest({
      path: "/streamed-huge",
      headers: { "Accept-Encoding": "gzip" }
    });
    assert.strictEqual(res.statusCode, 200);
    assert.strictEqual(res.headers["content-encoding"], "gzip");
    assert.strictEqual(zlib.gunzipSync(res.body).length, 16 * 1024 * 256);
  });
});
