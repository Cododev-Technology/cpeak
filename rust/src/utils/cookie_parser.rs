use std::collections::HashMap;
use std::time::UNIX_EPOCH;

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::internal::crypto::base64url_encode;
use crate::internal::errors::{framework_error, throw, ErrorCode};
use crate::internal::uri::{decode_uri_component, encode_uri_component};
use crate::types::{Middleware, Next, StringMap};
use crate::utils::types::CookieOptions;
use crate::{CpeakRequest, CpeakResponse};

// This will sign the cookie value with HMAC with the secret.
// Ideal for data like user IDs or session IDs, where you want to ensure the integrity of the cookie value without encryption.
// So this way, imagine you save a user ID in the cookie. By signing it, you can detect if the client has tampered with the cookie value
// (e.g., changing the user ID to impersonate another user).
// However, since it's not encrypted, the actual user ID is still visible to the client.
// This is a common approach for session cookies where you want to prevent tampering but don't mind if the value is visible.
pub fn sign(value: &str, secret: &str) -> String {
    let sig = base64url_encode(hmac_sha256(secret, value));
    format!("s:{value}.{sig}")
}

fn unsign(signed: &str, secret: &str) -> Option<String> {
    let without_prefix = signed.strip_prefix("s:")?;
    let last_dot = without_prefix.rfind('.')?;
    let value = &without_prefix[..last_dot];
    let sig = &without_prefix[last_dot + 1..];
    let expected = base64url_encode(hmac_sha256(secret, value));
    let expected_buf = expected.as_bytes();
    let actual_buf = sig.as_bytes();
    if expected_buf.len() != actual_buf.len() {
        return None;
    }
    if !timing_safe_equal(expected_buf, actual_buf) {
        return None;
    }
    Some(value.to_string())
}

// Parses the raw value of an HTTP `Cookie` request header into a name->value map
// This should be compatible with the RFC 6265 HTTP specification
fn parse_raw_cookies(header: &str) -> StringMap {
    let mut cookies = StringMap::new();
    if header.is_empty() {
        return cookies;
    }

    for pair in header.split(';') {
        // RFC 6265: cookie-pair requires '='. Pairs without one (e.g. a
        // bare flag like `Cookie: foo`) are not valid cookie-pairs and we skip them.
        // Note we use the FIRST '=' only. So values like base64 padding (`token=YWJjPT0=`) must keep trailing '='s.
        let Some(equal_sign_index) = pair.find('=') else {
            continue;
        };

        let key = pair[..equal_sign_index].trim();
        // Drop empty names and honour the FIRST occurrence on duplicates (Specs say servers SHOULD NOT rely on order.
        // We pick first-wins for stability).
        if key.is_empty() || cookies.contains_key(key) {
            continue;
        }

        let mut val = pair[equal_sign_index + 1..].trim();

        // Cookie values are sometimes sent wrapped in double quotes (like name="hello world"), so we strip the outer "
        // characters to get the actual value hello world.
        // The val.len() > 1 guard handles the edge case where the value is literally just a single " — without it, that one
        // character would match both the "starts with quote" and "ends with quote" checks, and slicing would wipe it out
        // to an empty string.
        if val.len() > 1 && val.starts_with('"') && val.ends_with('"') {
            val = &val[1..val.len() - 1];
        }

        // Percent-decoding cookie values is a server-side convention (not part of
        // RFC 6265 itself), but it's what Express and most ecosystem libraries do,
        // so we follow suit for compatibility. Skip the decode entirely when there's no
        // '%' to save work on the common case, and fall back to the raw value if
        // decoding fails on malformed input rather than failing the whole request.
        let decoded = if val.contains('%') {
            decode_uri_component(val).unwrap_or_else(|| val.to_string())
        } else {
            val.to_string()
        };
        cookies.insert(key.to_string(), decoded);
    }
    cookies
}

// One example output: "session=abc123; Path=/dashboard; Domain=example.com; Max-Age=86400; Expires=Thu, 31 Dec 2026 00:00:00 GMT; HttpOnly; Secure; SameSite=Strict"
fn build_set_cookie_header(name: &str, value: &str, options: &CookieOptions) -> String {
    let mut parts: Vec<String> = vec![format!("{name}={}", encode_uri_component(value))];
    let path = options.path.as_deref().unwrap_or("/");
    parts.push(format!("Path={path}"));
    if let Some(domain) = options
        .domain
        .as_deref()
        .filter(|domain| !domain.is_empty())
    {
        parts.push(format!("Domain={domain}"));
    }
    if let Some(max_age) = options.max_age {
        parts.push(format!("Max-Age={}", max_age.div_euclid(1000)));
    }
    if let Some(expires) = options.expires {
        // Dates before 1970 can't be represented in an HTTP date, so we use the earliest one
        let expires = if expires < UNIX_EPOCH {
            UNIX_EPOCH
        } else {
            expires
        };
        parts.push(format!("Expires={}", httpdate::fmt_http_date(expires)));
    }
    if options.http_only {
        parts.push("HttpOnly".to_string());
    }
    if options.secure {
        parts.push("Secure".to_string());
    }
    if let Some(same_site) = options.same_site {
        parts.push(format!("SameSite={}", same_site.as_str()));
    }
    parts.join("; ")
}

// Without this helper, calling res.cookie("a", "1") then res.cookie("b", "2") would overwrite the first cookie instead
// of sending both.
fn append_set_cookie(res: &mut CpeakResponse, header: String) {
    res.append_header("Set-Cookie", header);
}

#[derive(Debug, Clone, Default)]
pub struct CookieParserOptions {
    pub secret: Option<String>,
}

pub fn cookie_parser(options: impl Into<Option<CookieParserOptions>>) -> impl Middleware {
    let secret = options.into().unwrap_or_default().secret;

    if let Some(secret) = &secret {
        if secret.chars().count() < 32 {
            throw(framework_error(
                "Secret must be at least 32 characters. HMAC security is only as strong as the key.",
                ErrorCode::WeakSecret,
                None,
                false,
            ));
        }
    }

    // The secret stays around for as long as the program runs (like a static), so every request
    // can use it without touching a counter that all the worker threads share
    let secret: Option<&'static str> = secret.map(|secret| &*Box::leak(secret.into_boxed_str()));

    async move |req: &mut CpeakRequest, res: &mut CpeakResponse, next: Next| {
        let raw_header = req.headers.get("cookie").map_or("", String::as_str);
        let raw = parse_raw_cookies(raw_header);

        let mut cookies = StringMap::new();
        let mut signed_cookies: HashMap<String, Option<String>> = HashMap::new();

        for (key, val) in raw {
            // The "s:" prefix is the marker we add in `sign()` for HMAC-signed
            // cookies. Route those through unsign so the handler sees the original
            // value (or None if the signature didn't verify).
            match &secret {
                Some(secret) if val.starts_with("s:") => {
                    signed_cookies.insert(key, unsign(&val, secret));
                }
                _ => {
                    cookies.insert(key, val);
                }
            }
        }

        // The separation is intentional signal: "these were signed and verified, trust them more."
        req.cookies = cookies;
        req.signed_cookies = signed_cookies;

        // res.cookie() needs the secret to sign cookies
        res.cookie_secret = secret;

        next()
    }
}

// res.cookie() and res.clear_cookie()
impl CpeakResponse {
    // Set a cookie on the response. Just like in the Node.js version, signing a cookie
    // without giving a secret to cookie_parser throws (panics), which ends up in your handleErr.
    pub fn cookie(
        &mut self,
        name: &str,
        value: &str,
        options: impl Into<Option<CookieOptions>>,
    ) -> &mut Self {
        let options = options.into().unwrap_or_default();
        let final_value = if options.signed {
            let Some(secret) = self.cookie_secret else {
                panic!("cookieParser: secret is required to use signed cookies");
            };
            sign(value, &secret)
        } else {
            value.to_string()
        };
        append_set_cookie(self, build_set_cookie_header(name, &final_value, &options));
        self
    }

    pub fn clear_cookie(
        &mut self,
        name: &str,
        options: impl Into<Option<CookieOptions>>,
    ) -> &mut Self {
        let options = CookieOptions {
            max_age: Some(0),
            expires: Some(UNIX_EPOCH),
            ..options.into().unwrap_or_default()
        };
        append_set_cookie(self, build_set_cookie_header(name, "", &options));
        self
    }
}

fn hmac_sha256(secret: &str, value: &str) -> Vec<u8> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any size");
    mac.update(value.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

// A constant-time comparison, so attackers can't learn anything from how long it takes
fn timing_safe_equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}
