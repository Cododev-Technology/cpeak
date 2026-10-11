import path from "node:path";
import { createReadStream } from "node:fs";
import { readFile } from "node:fs/promises";
import { Transform } from "node:stream";
import { pipeline } from "node:stream/promises";
import type { TransformCallback } from "node:stream";
import { frameworkError, ErrorCode } from "../";
import { isClientDisconnect } from "../internal/errors";
import { compressAndSend } from "../internal/compression";
import { MIME_TYPES } from "../internal/mimeTypes";
import type { CpeakRequest, CpeakResponse, Next } from "../types";

export const MAX_PATTERN = 128;

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

class TemplateTransform extends Transform {
  private tail = "";

  constructor(
    private readonly data: Record<string, unknown>,
    private readonly baseDir: string
  ) {
    super();
  }

  _transform(
    chunk: Buffer,
    _: BufferEncoding,
    callback: TransformCallback
  ): void {
    const str = this.tail + chunk.toString("utf8");
    if (str.length <= MAX_PATTERN) {
      this.tail = str;
      callback();
      return;
    }

    let boundary = str.length - MAX_PATTERN;

    // Prevent cutting a tag in two
    for (const [opener, closer] of [
      ["{{", "}}"],
      ["<cpeak", ">"]
    ]) {
      const last = str.lastIndexOf(opener, boundary - 1);
      if (last === -1) continue;
      const closeIdx = str.indexOf(closer, last + opener.length);
      if (closeIdx === -1 || closeIdx >= boundary)
        boundary = Math.min(boundary, last);
    }

    this.tail = str.slice(boundary);
    const safe = str.slice(0, boundary);
    if (safe)
      this.process(safe)
        .then(() => callback())
        .catch(callback);
    else callback();
  }

  _flush(callback: TransformCallback): void {
    if (this.tail)
      this.process(this.tail)
        .then(() => callback())
        .catch(callback);
    else callback();
  }

  private async process(str: string): Promise<void> {
    const RE =
      /<cpeak\s+include="([^"]+)"\s*\/?>|<cpeak\s+html=\{([^}]+)\}\s*\/?>|\{\{([^}]+)\}\}/g;
    let last = 0;

    for (const match of str.matchAll(RE)) {
      const idx = match.index!;
      if (idx > last) this.push(str.slice(last, idx));

      const [, includeSrc, rawKey, escapedKey] = match;

      if (includeSrc !== undefined) {
        const includePath = path.resolve(this.baseDir, includeSrc);
        const content = await readFile(includePath, "utf8");
        const chunks: Buffer[] = [];
        const nested = new TemplateTransform(
          this.data,
          path.dirname(includePath)
        );
        await new Promise<void>((resolve, reject) => {
          nested.on("data", (c: Buffer) => chunks.push(c));
          nested.on("end", resolve);
          nested.on("error", reject);
          nested.end(Buffer.from(content, "utf8"));
        });
        this.push(Buffer.concat(chunks));
      } else if (rawKey !== undefined) {
        const val = this.data[rawKey.trim()];
        if (val !== undefined) this.push(String(val));
      } else {
        const val = this.data[escapedKey.trim()];
        if (val !== undefined) this.push(escapeHtml(String(val)));
      }

      last = idx + match[0].length;
    }

    if (last < str.length) this.push(str.slice(last));
  }
}

function render() {
  return function (req: CpeakRequest, res: CpeakResponse, next: Next): void {
    res.render = async (
      filePath: string,
      data: Record<string, unknown>,
      mime?: string
    ) => {
      if (res.headersSent) return;
      if (!mime) {
        const dotIndex = filePath.lastIndexOf(".");
        const fileExtension = dotIndex >= 0 ? filePath.slice(dotIndex + 1) : "";
        mime = MIME_TYPES[fileExtension];
        if (!mime) {
          throw frameworkError(
            `MIME type is missing for "${filePath}". Pass it as the third argument or register the extension via cpeak({ mimeTypes: { ${fileExtension || "ext"}: "..." } }).`,
            res.render,
            ErrorCode.MISSING_MIME
          );
        }
      }

      const resolved = path.resolve(filePath);

      try {
        if (res._compression) {
          const readStream = createReadStream(resolved);
          const transform = new TemplateTransform(data, path.dirname(resolved));
          pipeline(readStream, transform).catch(() => {});
          await compressAndSend(res, mime, transform, res._compression);
          return;
        }

        res.setHeader("Content-Type", mime);
        await pipeline(
          createReadStream(resolved),
          new TemplateTransform(data, path.dirname(resolved)),
          res
        );
      } catch (err: any) {
        throw frameworkError(
          `Failed to render "${filePath}." Error: ${err as Error}`,
          res.render,
          ErrorCode.RENDER_FAIL,
          undefined,
          isClientDisconnect(err)
        );
      }
    };

    next();
  };
}

render.string = async function (
  filePath: string,
  data: Record<string, unknown>
): Promise<string> {
  const resolved = path.resolve(filePath);
  const chunks: Buffer[] = [];
  await new Promise<void>((resolve, reject) => {
    const transform = new TemplateTransform(data, path.dirname(resolved));
    transform.on("data", (c: Buffer) => chunks.push(c));
    transform.on("end", resolve);
    transform.on("error", reject);
    createReadStream(resolved).pipe(transform);
  });
  return Buffer.concat(chunks).toString("utf8");
};

// TODO: implement stream engine
render.stream = function (
  _filePath: string,
  _data: Record<string, unknown>
): never {
  throw new Error("render.stream is not yet implemented");
};

export { render };
