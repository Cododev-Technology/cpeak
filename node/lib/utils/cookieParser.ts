import { createHmac, timingSafeEqual } from "node:crypto";
import type { CpeakRequest, CpeakResponse, Next } from "../types";
import { frameworkError, ErrorCode } from "../index";
import type { CookieOptions } from "./types";

// This will sign the cookie value with HMAC with the secret.
// Ideal for data like user IDs or session IDs, where you want to ensure the integrity of the cookie value without encryption.
// So this way, imagine you save a user ID in the cookie. By signing it, you can detect if the client has tampered with the cookie value
// (e.g., changing the user ID to impersonate another user).
// However, since it's not encrypted, the actual user ID is still visible  to the client.
// This is a common approach for session cookies where you want to prevent tampering but don't mind if the value is visible.
function sign(value: string, secret: string): string {
  const sig = createHmac("sha256", secret).update(value).digest("base64url");
  return `s:${value}.${sig}`;
}

function unsign(signed: string, secret: string): string | false {
  if (!signed.startsWith("s:")) return false;
  const withoutPrefix = signed.slice(2);
  const lastDot = withoutPrefix.lastIndexOf(".");
  if (lastDot === -1) return false;
  const value = withoutPrefix.slice(0, lastDot);
  const sig = withoutPrefix.slice(lastDot + 1);
  const expected = createHmac("sha256", secret)
    .update(value)
    .digest("base64url");
  const expectedBuf = Buffer.from(expected);
  const actualBuf = Buffer.from(sig);
  if (expectedBuf.length !== actualBuf.length) return false;
  if (!timingSafeEqual(expectedBuf, actualBuf)) return false;
  return value;
}

// Parses the raw value of an HTTP `Cookie` request header into a name->value
// This should be compatible with the RFC 6265 HTTP specification
function parseRawCookies(header: string): Record<string, string> {
  // Use a null-prototype object to prevent prototype pollution attacks when assigning cookie names like "__proto__" or "constructor".
  const cookies: Record<string, string> = Object.create(null);
  if (!header) return cookies;

  const pairs = header.split(";");

  for (let i = 0; i < pairs.length; i++) {
    const pair = pairs[i];
    const equalSignIndex = pair.indexOf("=");

    // RFC 6265: cookie-pair requires '='. Pairs without one (e.g. a
    // bare flag like `Cookie: foo`) are not valid cookie-pairs and we skip them.
    // Note we use the FIRST '=' only. So values like base64 padding (`token=YWJjPT0=`) must keep trailing '='s.
    if (equalSignIndex === -1) continue;

    const key = pair.slice(0, equalSignIndex).trim();
    // Drop empty names and honour the FIRST occurrence on duplicates (Specs say servers SHOULD NOT rely on order.
    // We pick first-wins for stability).
    if (!key || cookies[key] !== undefined) continue;

    let val = pair.slice(equalSignIndex + 1).trim();

    // Cookie values are sometimes sent wrapped in double quotes (like name="hello world"), so we strip the outer "
    // characters to get the actual value hello world.
    // The val.length > 1 guard handles the edge case where the value is literally just a single " — without it, that one
    // character would match both the "starts with quote" and "ends with quote" checks, and slice(1, -1) would wipe it out
    // to an empty string.
    if (val.length > 1 && val[0] === '"' && val[val.length - 1] === '"') {
      val = val.slice(1, -1);
    }

    // Percent-decoding cookie values is a server-side convention (not part of
    // RFC 6265 itself), but it's what Express and most ecosystem libraries do,
    // so we follow suit for compatibility. Skip the decode entirely when there's no
    // '%' to save work on the common case, and fall back to the raw value if
    // decodeURIComponent throws on malformed input rather than crashing the
    // whole request.
    try {
      cookies[key] = val.indexOf("%") !== -1 ? decodeURIComponent(val) : val;
    } catch (e) {
      cookies[key] = val;
    }
  }
  return cookies;
}

// One example output: "session=abc123; Path=/dashboard; Domain=example.com; Max-Age=86400; Expires=Thu, 31 Dec 2026 00:00:00 GMT; HttpOnly; Secure; SameSite=Strict"
function buildSetCookieHeader(
  name: string,
  value: string,
  options: CookieOptions
): string {
  const parts: string[] = [`${name}=${encodeURIComponent(value)}`];
  const path = options.path ?? "/";
  parts.push(`Path=${path}`);
  if (options.domain) parts.push(`Domain=${options.domain}`);
  if (options.maxAge !== undefined)
    parts.push(`Max-Age=${Math.floor(options.maxAge / 1000)}`);
  if (options.expires) parts.push(`Expires=${options.expires.toUTCString()}`);
  if (options.httpOnly) parts.push("HttpOnly");
  if (options.secure) parts.push("Secure");
  if (options.sameSite) parts.push(`SameSite=${options.sameSite}`);
  return parts.join("; ");
}

// Without this helper, calling res.cookie("a", "1") then res.cookie("b", "2") would overwrite the first cookie instead
// of sending both.
function appendSetCookie(res: CpeakResponse, header: string) {
  const existing = res.getHeader("Set-Cookie");
  if (!existing) {
    res.setHeader("Set-Cookie", [header]);
  } else if (Array.isArray(existing)) {
    res.setHeader("Set-Cookie", [...existing, header]);
  } else {
    res.setHeader("Set-Cookie", [String(existing), header]);
  }
}

export function cookieParser(options: { secret?: string } = {}) {
  const { secret } = options;

  if (secret !== undefined && secret.length < 32) {
    throw frameworkError(
      "Secret must be at least 32 characters. HMAC security is only as strong as the key.",
      cookieParser,
      ErrorCode.WEAK_SECRET
    );
  }

  return (req: CpeakRequest, res: CpeakResponse, next: Next) => {
    const rawHeader = req.headers["cookie"] || "";
    const raw = parseRawCookies(rawHeader);

    // Mirror parseRawCookies and use null-prototype maps here too. If we used
    // a regular `{}`, the assignment below would invoke Object.prototype's
    // __proto__ setter (no-op for string values), silently dropping any
    // cookie literally named __proto__ — undoing the fix in parseRawCookies.
    const cookies: Record<string, string> = Object.create(null);
    const signedCookies: Record<string, string | false> = Object.create(null);

    for (const [key, val] of Object.entries(raw)) {
      // The "s:" prefix is the marker we add in `sign()` for HMAC-signed
      // cookies. Route those through unsign so the handler sees the original
      // value (or `false` if the signature didn't verify).
      if (val.startsWith("s:") && secret) {
        signedCookies[key] = unsign(val, secret);
      } else {
        cookies[key] = val;
      }
    }

    // The separation is intentional signal: "these were signed and verified, trust them more."
    req.cookies = cookies;
    req.signedCookies = signedCookies;

    res.cookie = (name: string, value: string, options: CookieOptions = {}) => {
      let finalValue = value;
      if (options.signed) {
        if (!secret)
          throw new Error(
            "cookieParser: secret is required to use signed cookies"
          );
        finalValue = sign(value, secret);
      }
      appendSetCookie(res, buildSetCookieHeader(name, finalValue, options));
      return res;
    };

    res.clearCookie = (name: string, options: CookieOptions = {}) => {
      appendSetCookie(
        res,
        buildSetCookieHeader(name, "", {
          ...options,
          maxAge: 0,
          expires: new Date(0)
        })
      );
      return res;
    };

    next();
  };
}
