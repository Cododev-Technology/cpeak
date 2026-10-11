import zlib from "node:zlib";
import { Readable } from "node:stream";
import { Buffer } from "node:buffer";
import { pipeline } from "node:stream/promises";
import type { Transform } from "node:stream";
import type { ServerResponse } from "node:http";
import type { CompressionOptions, ResolvedCompressionConfig } from "./types";

type Encoding = "br" | "gzip" | "deflate";

const COMPRESSIBLE_TYPE = /text|json|javascript|css|xml|svg/i;
const NO_TRANSFORM = /(?:^|,)\s*no-transform\s*(?:,|$)/i;

// Parse Accept-Encoding and pick a compression algorithm the server supports.
// Handles q=0 to disable an algorithm. Cpeak preference is fixed: br > gzip > deflate.
function pickEncoding(header: string): Encoding | null {
  if (!header) return null;

  const accepted: Record<string, number> = {};
  let wildcard: number | undefined;

  for (const part of header.split(",")) {
    const [rawName, ...params] = part.trim().split(";");
    const name = rawName.trim().toLowerCase();
    if (!name) continue;

    let q = 1;
    for (const p of params) {
      const m = p.trim().match(/^q=([\d.]+)$/i);
      if (m) q = Number(m[1]);
    }
    if (Number.isNaN(q)) q = 0;

    if (name === "*") wildcard = q;
    else accepted[name] = q;
  }

  const tryPick = (enc: Encoding): boolean => {
    const q = enc in accepted ? accepted[enc] : wildcard;
    return q !== undefined && q > 0;
  };

  if (tryPick("br")) return "br";
  if (tryPick("gzip")) return "gzip";
  if (tryPick("deflate")) return "deflate";
  return null;
}

// Handling the Vary HTTP header
function appendVary(res: ServerResponse, value: string) {
  const existing = res.getHeader("Vary");
  if (!existing) return res.setHeader("Vary", value);
  const current = String(existing)
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  if (
    current.includes("*") ||
    current.some((v) => v.toLowerCase() === value.toLowerCase())
  )
    return;
  res.setHeader("Vary", [...current, value].join(", "));
}

// Brotli options. Zlib uses 11 (max), which is really slow for live
// responses. We go with 4 unless the developer specifies otherwise.
function brotliOptsFor(config: ResolvedCompressionConfig): zlib.BrotliOptions {
  const userBrotli = config.brotli || {};
  return {
    ...userBrotli,
    params: {
      [zlib.constants.BROTLI_PARAM_QUALITY]: 4,
      ...(userBrotli.params || {})
    }
  };
}

function createCompressorStream(
  encoding: Encoding,
  config: ResolvedCompressionConfig
): Transform {
  if (encoding === "br")
    return zlib.createBrotliCompress(brotliOptsFor(config));
  if (encoding === "gzip") return zlib.createGzip(config.gzip);
  return zlib.createDeflate(config.deflate);
}

// Decides what to do with this response
function negotiate(
  res: ServerResponse,
  mime: string,
  size: number,
  config: ResolvedCompressionConfig
): { encoding: Encoding | null; eligible: boolean } {
  // Whether this content type is worth trying to compress at all.
  // Some types are already compressed and don't compress well.
  if (!COMPRESSIBLE_TYPE.test(mime)) return { encoding: null, eligible: false };

  if (res.req?.method === "HEAD") return { encoding: null, eligible: false };

  // RFC specification: don't transform responses that ask not to be transformed.
  const cc = res.getHeader("Cache-Control");
  if (cc && NO_TRANSFORM.test(String(cc)))
    return { encoding: null, eligible: false };

  const existing = res.getHeader("Content-Encoding");
  if (existing && existing !== "identity")
    return { encoding: null, eligible: false };

  if (size < config.threshold) return { encoding: null, eligible: true };

  const encoding = pickEncoding(
    String(res.req?.headers["accept-encoding"] || "")
  );
  return { encoding, eligible: true };
}

// Converts into a Readable stream
function bodyAsReadable(body: Buffer | string | Readable): Readable {
  if (Buffer.isBuffer(body)) return Readable.from([body]);
  if (typeof body === "string") return Readable.from([Buffer.from(body)]);
  return body;
}

// Resolves compression options (or 'true' for defaults) into a
// complete config. Called once at Cpeak construction.
export function resolveCompressionOptions(
  input: true | CompressionOptions
): ResolvedCompressionConfig {
  const options: CompressionOptions = input === true ? {} : input;
  return {
    threshold: options.threshold ?? 1024,
    brotli: options.brotli ?? {},
    gzip: options.gzip ?? {},
    deflate: options.deflate ?? {}
  };
}

// The final point used by res.compress, res.json, res.sendFile and res.render
// when compression is enabled by the developer.
//
// Compression always goes through createGzip/createBrotliCompress/createDeflate
// streams which are async and run on libuv's thread pool.
export async function compressAndSend(
  res: ServerResponse,
  mime: string,
  body: Buffer | string | Readable,
  config: ResolvedCompressionConfig,
  size?: number
): Promise<void> {
  res.setHeader("Content-Type", mime);

  const knownSize: number = Buffer.isBuffer(body)
    ? body.length
    : typeof body === "string"
      ? Buffer.byteLength(body)
      : (size ?? Infinity);

  const { encoding, eligible } = negotiate(res, mime, knownSize, config);

  if (!encoding) {
    if (eligible) appendVary(res, "Accept-Encoding");
    if (Buffer.isBuffer(body) || typeof body === "string") {
      res.setHeader("Content-Length", String(knownSize));
      res.end(body);
      return;
    }
    if (size !== undefined) res.setHeader("Content-Length", String(size));
    await pipeline(body, res);
    return;
  }

  res.setHeader("Content-Encoding", encoding);
  appendVary(res, "Accept-Encoding");
  await pipeline(
    bodyAsReadable(body),
    createCompressorStream(encoding, config),
    res
  );
}
