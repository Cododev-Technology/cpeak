use std::sync::Arc;

use crate::types::{Middleware, Next};
use crate::utils::types::{CorsOptions, OriginInput};
use crate::{CpeakRequest, CpeakResponse};

// Append a value to an existing header without overwriting prior entries
// (e.g. compression already sets `Vary: Accept-Encoding`).
fn append_vary(res: &mut CpeakResponse, value: &str) {
    let Some(existing) = res
        .get_header("Vary")
        .filter(|existing| !existing.is_empty())
    else {
        res.set_header("Vary", value);
        return;
    };
    let mut current: Vec<String> = existing
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if current.iter().any(|v| v == "*" || v == value) {
        return;
    }
    current.push(value.to_string());
    res.set_header("Vary", current.join(", "));
}

// Determine if the given origin is allowed based on the rule.
// Examples of what developers can specify for the rule:
// - `true` or `"*"`: allow all origins
// - `false`: disallow all origins
// - `"https://example.com"`: allow only this origin
// - `vec!["https://example.com", "https://foo.com"]`: allow these origins
// - `Regex::new(r"\.example\.com$")`: allow origins that match this regex
// - `OriginInput::function(async |origin| origin == "https://example.com")`: custom function to determine if the origin is allowed
async fn is_allowed(origin: &str, rule: &OriginInput) -> bool {
    match rule {
        OriginInput::Bool(true) => true,
        OriginInput::String(rule) if rule == "*" => true,
        OriginInput::Bool(false) => false,
        _ if origin.is_empty() => false,
        OriginInput::String(rule) => rule == origin,
        OriginInput::List(rule) => rule.iter().any(|allowed| allowed == origin),
        OriginInput::RegExp(rule) => rule.is_match(origin),
        OriginInput::Function(rule) => rule.0.call(origin).await,
    }
}

pub fn cors(options: impl Into<Option<CorsOptions>>) -> impl Middleware {
    let CorsOptions {
        origin,
        methods,
        allowed_headers,
        exposed_headers,
        credentials,
        max_age,
        preflight_continue,
        options_success_status,
    } = options.into().unwrap_or_default();

    let origin = Arc::new(origin);
    let is_any_origin = matches!(&*origin, OriginInput::String(origin) if origin == "*");

    async move |req: &mut CpeakRequest, res: &mut CpeakResponse, next: Next| {
        let request_origin = req.headers.get("origin").cloned().unwrap_or_default();

        // Not a CORS request, nothing to do.
        if request_origin.is_empty() {
            return next();
        }

        let allowed = is_allowed(&request_origin, &origin).await;
        if !allowed {
            return next();
        }

        // We cannot combine Access-Control-Allow-Origin: * with
        // Access-Control-Allow-Credentials: true. Browsers will flat-out reject it.
        // Instead we'll reflect the origin.
        let allow_origin_value = if is_any_origin && !credentials {
            "*"
        } else {
            request_origin.as_str()
        };
        res.set_header("Access-Control-Allow-Origin", allow_origin_value);
        if allow_origin_value != "*" {
            append_vary(res, "Origin");
        }

        if credentials {
            res.set_header("Access-Control-Allow-Credentials", "true");
        }
        if !exposed_headers.is_empty() {
            res.set_header("Access-Control-Expose-Headers", &exposed_headers);
        }

        let is_preflight =
            req.method == "OPTIONS" && req.headers.contains_key("access-control-request-method");

        if !is_preflight {
            return next();
        }

        res.set_header("Access-Control-Allow-Methods", &methods);

        if !allowed_headers.is_empty() {
            res.set_header("Access-Control-Allow-Headers", &allowed_headers);
        } else if is_any_origin {
            // If origin is *, just act like an echo chamber for requested headers. Give back whatever the browser asks for.
            if let Some(requested) = req.headers.get("access-control-request-headers") {
                res.set_header("Access-Control-Allow-Headers", requested);
            }
        } else {
            res.set_header(
                "Access-Control-Allow-Headers",
                "Content-Type, Authorization",
            );
        }

        res.set_header("Access-Control-Max-Age", max_age);

        if preflight_continue {
            return next();
        }

        res.status_code = options_success_status;
        res.end("")
    }
}
