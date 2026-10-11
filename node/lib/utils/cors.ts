import type { CpeakRequest, CpeakResponse, Next } from "../types";
import type { CorsOptions, OriginInput } from "./types";

// Append a value to an existing header without overwriting prior entries
// (e.g. compression already sets `Vary: Accept-Encoding`).
function appendVary(res: CpeakResponse, value: string) {
  const existing = res.getHeader("Vary");
  if (!existing) return res.setHeader("Vary", value);
  const current = String(existing)
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  if (current.includes("*") || current.includes(value)) return;
  res.setHeader("Vary", [...current, value].join(", "));
}

// Determine if the given origin is allowed based on the rule.
// Examples of what developers can specify for the rule:
// - `true` or `*`: allow all origins
// - `false`: disallow all origins
// - `"https://example.com"`: allow only this origin
// - `["https://example.com", "https://foo.com"]`: allow these origins
// - `/\.example\.com$/`: allow origins that match this regex
// - `(origin) => origin === "https://example.com"`: custom function to determine if the origin is allowed
async function isAllowed(
  origin: string | undefined,
  rule: OriginInput
): Promise<boolean> {
  if (rule === true || rule === "*") return true;
  if (rule === false || !origin) return false;
  if (typeof rule === "string") return rule === origin;
  if (Array.isArray(rule)) return rule.includes(origin);
  if (rule instanceof RegExp) return rule.test(origin);
  if (typeof rule === "function") return await rule(origin);
  return false;
}

const cors = (options: CorsOptions = {}) => {
  const {
    origin = "*",
    methods = "GET,HEAD,PUT,PATCH,POST,DELETE",
    allowedHeaders,
    exposedHeaders,
    credentials = false,
    maxAge = 86400,
    preflightContinue = false,
    optionsSuccessStatus = 204
  } = options;

  const methodsStr = Array.isArray(methods) ? methods.join(",") : methods;
  const allowedHeadersStr = Array.isArray(allowedHeaders)
    ? allowedHeaders.join(",")
    : allowedHeaders;
  const exposedHeadersStr = Array.isArray(exposedHeaders)
    ? exposedHeaders.join(",")
    : exposedHeaders;

  return async (req: CpeakRequest, res: CpeakResponse, next: Next) => {
    const requestOrigin = req.headers.origin;

    // Not a CORS request, nothing to do.
    if (!requestOrigin) return next();

    const allowed = await isAllowed(requestOrigin, origin);
    if (!allowed) return next();

    // We cannot combine Access-Control-Allow-Origin: * with
    // Access-Control-Allow-Credentials: true. Browsers will flat-out reject it.
    // Instead we'll reflect the origin.
    const allowOriginValue =
      origin === "*" && !credentials ? "*" : requestOrigin;
    res.setHeader("Access-Control-Allow-Origin", allowOriginValue);
    if (allowOriginValue !== "*") appendVary(res, "Origin");

    if (credentials) res.setHeader("Access-Control-Allow-Credentials", "true");
    if (exposedHeadersStr)
      res.setHeader("Access-Control-Expose-Headers", exposedHeadersStr);

    const isPreflight =
      req.method === "OPTIONS" &&
      req.headers["access-control-request-method"] !== undefined;

    if (!isPreflight) return next();

    res.setHeader("Access-Control-Allow-Methods", methodsStr);

    if (allowedHeadersStr) {
      res.setHeader("Access-Control-Allow-Headers", allowedHeadersStr);
    } else if (origin === "*") {
      // If origin is *, just act like an echo chamber for requested headers. Give back whatever the browser asks for.
      const requested = req.headers["access-control-request-headers"];
      if (requested) res.setHeader("Access-Control-Allow-Headers", requested);
    } else {
      res.setHeader(
        "Access-Control-Allow-Headers",
        "Content-Type, Authorization"
      );
    }

    res.setHeader("Access-Control-Max-Age", String(maxAge));

    if (preflightContinue) return next();

    res.statusCode = optionsSuccessStatus;
    res.end();
  };
};

export { cors };
