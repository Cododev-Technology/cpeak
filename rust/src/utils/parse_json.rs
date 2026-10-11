use bytes::{Bytes, BytesMut};

use crate::internal::errors::{framework_error, ErrorCode};
use crate::internal::json;
use crate::types::{Middleware, Next};
use crate::{CpeakRequest, CpeakResponse};

// Check if Content-Type is JSON
fn is_json(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return false;
    };
    if content_type == "application/json" {
        return true;
    }
    content_type.starts_with("application/json") || content_type.contains("+json")
}

#[derive(Debug, Clone)]
pub struct ParseJsonOptions {
    // The maximum JSON body size in bytes that your server will accept
    pub limit: usize,
}

impl Default for ParseJsonOptions {
    fn default() -> Self {
        Self { limit: 1024 * 1024 }
    }
}

// Parsing JSON
pub fn parse_json(options: impl Into<Option<ParseJsonOptions>>) -> impl Middleware {
    // Default limit to 1MB
    let limit = options
        .into()
        .map(|options| options.limit)
        .filter(|limit| *limit > 0)
        .unwrap_or(1024 * 1024);

    async move |req: &mut CpeakRequest, _res: &mut CpeakResponse, next: Next| {
        if !is_json(req.headers.get("content-type").map(String::as_str)) {
            return next();
        }

        let mut chunks: Vec<Bytes> = Vec::new();
        let mut bytes_received = 0;

        while let Some(chunk) = req.chunk().await? {
            bytes_received += chunk.len();

            // To prevent Denial of Service (DoS) attacks, enforce a maximum body size
            if bytes_received > limit {
                // Stop reading the body and let handleErr respond
                return Err(framework_error(
                    "JSON body too large",
                    ErrorCode::PayloadTooLarge,
                    Some(413), // HTTP 413 Payload Too Large
                    false,
                ));
            }

            chunks.push(chunk);
        }

        // For better performance, we concat buffers once, then parse
        // Optimization: If only one chunk exists, avoid the memory copy of concat
        let raw_body = if chunks.len() == 1 {
            chunks.remove(0)
        } else {
            let mut raw_body = BytesMut::with_capacity(bytes_received);
            for chunk in chunks {
                raw_body.extend_from_slice(&chunk);
            }
            raw_body.freeze()
        };

        // Handle empty body case
        req.body = if raw_body.is_empty() {
            serde_json::json!({})
        } else {
            // Handle Invalid JSON without crashing
            json::parse(&raw_body).map_err(|_| {
                framework_error(
                    "Invalid JSON format",
                    ErrorCode::InvalidJson,
                    Some(400), // HTTP 400 Bad Request
                    false,
                )
            })?
        };

        next()
    }
}
