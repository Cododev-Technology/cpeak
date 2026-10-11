import type { CpeakRequest, CpeakResponse, Next } from "../types";
import { Buffer } from "node:buffer";
import { frameworkError, ErrorCode } from "../index";

// Check if Content-Type is JSON
function isJSON(contentType: string | undefined) {
  if (!contentType) return false;
  if (contentType === "application/json") return true;
  return (
    contentType.startsWith("application/json") || contentType.includes("+json")
  );
}

// Parsing JSON
const parseJSON = (options: { limit?: number } = {}) => {
  // Default limit to 1MB
  const limit = options.limit || 1024 * 1024;

  return (req: CpeakRequest, res: CpeakResponse, next: Next) => {
    if (!isJSON(req.headers["content-type"])) return next();

    const chunks: Buffer[] = [];
    let bytesReceived = 0;

    const onData = (chunk: Buffer) => {
      bytesReceived += chunk.length;

      // To prevent Denial of Service (DoS) attacks, enforce a maximum body size
      if (bytesReceived > limit) {
        // Stop listening to data
        req.pause();

        // Remove listeners so we don't trigger 'end' or more 'data'
        req.removeListener("data", onData);
        req.removeListener("end", onEnd);

        next(
          frameworkError(
            "JSON body too large",
            onData,
            ErrorCode.PAYLOAD_TOO_LARGE,
            413 // HTTP 413 Payload Too Large
          )
        );

        return;
      }

      chunks.push(chunk);
    };

    const onEnd = () => {
      try {
        // For better performance, we concat buffers once, then convert to string
        // Optimization: If only one chunk exists, avoid the memory copy of concat
        const rawBody =
          chunks.length === 1
            ? chunks[0].toString("utf-8")
            : Buffer.concat(chunks).toString("utf-8");

        // Handle empty body case
        req.body = rawBody ? JSON.parse(rawBody) : {};

        next();
      } catch (err) {
        // Handle Invalid JSON without crashing
        next(
          frameworkError(
            "Invalid JSON format",
            onEnd,
            ErrorCode.INVALID_JSON,
            400 // HTTP 400 Bad Request
          )
        );
      }
    };

    req.on("data", onData);
    req.on("end", onEnd);
  };
};

export { parseJSON };
