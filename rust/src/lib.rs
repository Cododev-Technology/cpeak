mod internal;
mod types;
pub mod utils;

// The `mimalloc` feature (see Cargo.toml): a faster memory allocator for the whole program
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::any::Any;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::fmt::{Display, Write as _};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::{StatusCode, Uri};
use serde::Serialize;

use crate::internal::compression::{
    compress_and_send, compress_and_send_buffer, pipe, resolve_compression_options,
    send_precompressed,
};
use crate::internal::errors::CatchUnwind;
use crate::internal::files::{self, File};
use crate::internal::json;
use crate::internal::log::{Logger, RequestLog};
use crate::internal::mime_types::{get_mime, register_mime_types};
use crate::internal::router::Router;
use crate::internal::server::{ResponseBody, ResponseSender};
use crate::internal::tls::{resolve_https_options, ResolvedHttpsConfig};
use crate::internal::types::ResolvedCompressionConfig;
use crate::internal::uri::parse_search_params;
use crate::types::{DynErrorHandler, DynHandler, DynMiddleware};
use crate::utils::auth::AuthState;

pub use crate::internal::compression::{Body, Readable};
pub use crate::internal::errors::{framework_error, is_client_disconnect, CpeakError, ErrorCode};
pub use crate::internal::event_loop::{run, spawn, spawn_blocking, spawn_local};
pub use crate::internal::log::{log, log_requests, LogMessage};
pub use crate::internal::server::{CpeakServer, ListenAddress};
pub use crate::internal::shutdown::on_shutdown;
pub use crate::internal::timers::sleep;
pub use crate::internal::types::{
    BrotliOptions, CompressionOptions, HttpsOptions, LogOptions, LogOutput, LogTimeZone,
    RequestLogOptions, ZlibOptions,
};
pub use crate::types::{
    CpeakOptions, ErrorHandler, Handler, IntoRouteMiddleware, Middleware, Next, RouteMiddleware,
    StringMap,
};
// So `use cpeak::{json, Value};` is all you need to read and send JSON, without adding
// serde_json yourself. from_value() turns a Value (like req.body) into a struct of your own.
pub use serde_json::{from_value, json, Value};
// What cpeak::spawn(), cpeak::spawn_local() and cpeak::spawn_blocking() give back
pub use tokio::task::JoinHandle;

// What the crypto module is in Node.js: random bytes, a hash, and bytes written as text
pub mod crypto {
    pub use crate::internal::crypto::{base64url_encode, hex_encode, random_bytes, sha256};
}

pub struct CpeakRequest {
    pub method: String,
    // The path and the query string, like /users?filter=active
    pub url: String,
    // The request headers. Just like in Node.js, the header names are lower-cased.
    pub headers: StringMap,
    pub remote_address: SocketAddr,
    pub body: serde_json::Value,
    pub params: StringMap,
    // The URL parameters (like /users?key1=value1&key2=value2)
    // We will call this query to be more familiar with other Node.js frameworks.
    pub query: StringMap,
    pub cookies: StringMap,
    // None means the signature was invalid, or the value was tampered with
    pub signed_cookies: HashMap<String, Option<String>>,

    incoming: Option<Incoming>,
    // The URL as hyper parsed it. We route on this one, like the Node.js version routes on
    // the URL the request came in with, whatever a middleware function does to req.url.
    uri: Uri,
    // Set by the auth() middleware
    pub(crate) auth: Option<&'static AuthState>,
    // Allow developers to add their own extensions (e.g. req.set("test", ...))
    extensions: HashMap<String, Box<dyn Any>>,
}

impl CpeakRequest {
    pub(crate) fn new(
        method: String,
        uri: Uri,
        headers: StringMap,
        remote_address: SocketAddr,
        incoming: Incoming,
    ) -> Self {
        // Parse the URL parameters (like /users?key1=value1&key2=value2)
        let query = match uri.query() {
            Some(query) => parse_search_params(query),
            None => StringMap::new(),
        };

        Self {
            method,
            url: request_url(&uri),
            headers,
            remote_address,
            body: serde_json::Value::Null,
            params: StringMap::new(),
            query,
            cookies: StringMap::new(),
            signed_cookies: HashMap::new(),
            incoming: Some(incoming),
            uri,
            auth: None,
            extensions: HashMap::new(),
        }
    }

    // Attach your own value to the request, like `req.custom = "This is some string"` in the Node.js version
    pub fn set<T: Any>(&mut self, key: impl Into<String>, value: T) {
        self.extensions.insert(key.into(), Box::new(value));
    }

    // Read a value you attached before, like `req.custom` in the Node.js version
    pub fn get<T: Any>(&self, key: &str) -> Option<&T> {
        self.extensions.get(key)?.downcast_ref()
    }

    pub fn get_mut<T: Any>(&mut self, key: &str) -> Option<&mut T> {
        self.extensions.get_mut(key)?.downcast_mut()
    }

    // Read the next chunk of the request body, or None once the whole body has been read.
    // This is how you consume the request as a stream, like `req.on("data", ...)` in Node.js.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, CpeakError> {
        loop {
            let Some(incoming) = self.incoming.as_mut() else {
                return Ok(None);
            };
            let frame = std::future::poll_fn(|cx| {
                hyper::body::Body::poll_frame(Pin::new(&mut *incoming), cx)
            })
            .await;

            match frame {
                None => {
                    self.incoming = None;
                    return Ok(None);
                }
                Some(Ok(frame)) => {
                    // We skip the trailers, if there are any
                    if let Ok(data) = frame.into_data() {
                        if !data.is_empty() {
                            return Ok(Some(data));
                        }
                    }
                }
                Some(Err(error)) => {
                    self.incoming = None;
                    let mut error = CpeakError::from(error);
                    error.client_disconnect = true;
                    return Err(error);
                }
            }
        }
    }
}

// The path and the query string, as the client sent them (like /users?filter=active)
fn request_url(uri: &Uri) -> String {
    match uri.path_and_query() {
        Some(path_and_query) if uri.scheme().is_none() => path_and_query.as_str().to_owned(),
        _ => uri.to_string(),
    }
}

pub struct CpeakResponse {
    pub status_code: u16,
    headers: HeaderMap,
    headers_sent: bool,
    finished: bool,
    sender: ResponseSender,
    // The functions to run once the response is sent (res.on_finish())
    finish_listeners: Vec<Box<dyn FnOnce(&FinishedResponse)>>,
    // How many bytes of body went out, for the {bytes} of the request log
    pub(crate) body_bytes: u64,

    // Set per-request from the Cpeak instance (the worker's own copy of it). None when compression isn't enabled.
    pub(crate) compression: Option<Rc<ResolvedCompressionConfig>>,
    // What we need to know about the request (res.req in Node.js)
    pub(crate) req_is_head: bool,
    pub(crate) req_accept_encoding: String,
    // Set by the render() middleware
    pub(crate) render_enabled: bool,
    // Set by the cookie_parser() middleware
    pub(crate) cookie_secret: Option<&'static str>,
    // Set by the cpeak::log_requests() middleware
    pub(crate) request_log: Option<&'static RequestLog>,
}

impl CpeakResponse {
    // `headers` is an empty map to fill in (the server reuses the one of the request, see server.rs)
    pub(crate) fn new(sender: ResponseSender, req: &CpeakRequest, headers: HeaderMap) -> Self {
        Self {
            status_code: 200,
            headers,
            headers_sent: false,
            finished: false,
            sender,
            finish_listeners: Vec::new(),
            body_bytes: 0,
            compression: None,
            req_is_head: req.method == "HEAD",
            req_accept_encoding: req
                .headers
                .get("accept-encoding")
                .cloned()
                .unwrap_or_default(),
            render_enabled: false,
            cookie_secret: None,
            request_log: None,
        }
    }

    // Send a file back to the client
    pub async fn send_file<'a>(
        &mut self,
        path: &str,
        mime: impl Into<Option<&'a str>>,
    ) -> Result<(), CpeakError> {
        self.send_file_or_copy(path, mime.into(), true).await
    }

    // `precompressed`: send the .br or .gz copy of the file instead, if cpeak::precompress()
    // made one and the client takes it
    pub(crate) async fn send_file_or_copy(
        &mut self,
        path: &str,
        mime: Option<&str>,
        precompressed: bool,
    ) -> Result<(), CpeakError> {
        if self.headers_sent {
            return Ok(());
        }
        let mime = match mime {
            Some(mime) => mime.to_string(),
            None => {
                let file_extension = path
                    .rfind('.')
                    .map_or("", |dot_index| &path[dot_index + 1..]);
                match get_mime(file_extension) {
                    Some(mime) => mime,
                    None => {
                        return Err(framework_error(
                            format!(
                                "MIME type is missing for \"{path}\". Pass it as the second argument or register the extension via Cpeak::new(CpeakOptions {{ mime_types: ... }}), like (\"{}\", \"...\").",
                                if file_extension.is_empty() { "ext" } else { file_extension }
                            ),
                            ErrorCode::MissingMime,
                            None,
                            false,
                        ))
                    }
                }
            }
        };

        let result = async {
            if precompressed && send_precompressed(self, path, &mime).await? {
                return Ok(());
            }

            let size = match files::read(path)? {
                File::NotAFile => {
                    return Err(framework_error(
                        format!("Not a file: {path}"),
                        ErrorCode::NotAFile,
                        None,
                        false,
                    ));
                }
                // Small files come from memory (see internal/files.rs)
                File::Content(content, _) => {
                    if let Some(compression) = self.compression.clone() {
                        let size = content.len() as u64;
                        return compress_and_send(
                            self,
                            &mime,
                            Body::Buffer(content),
                            &compression,
                            Some(size),
                        )
                        .await;
                    }
                    self.set_header("Content-Type", &mime);
                    self.set_header("Content-Length", content.len());
                    return self.end(content);
                }
                File::TooBig(size) => size,
            };
            let file = tokio::fs::File::open(path).await?;

            if let Some(compression) = self.compression.clone() {
                return compress_and_send(
                    self,
                    &mime,
                    Body::Readable(Readable::new(file)),
                    &compression,
                    Some(size),
                )
                .await;
            }

            self.set_header("Content-Type", &mime);
            self.set_header("Content-Length", size);

            // Properly propagate stream errors and respect backpressure
            pipe(Readable::new(file), self).await
        }
        .await;

        result.map_err(|err| {
            if err.code == Some(ErrorCode::NotAFile) {
                return err;
            }

            let not_found = err
                .source()
                .and_then(|source| source.downcast_ref::<io::Error>())
                .is_some_and(|io_error| io_error.kind() == io::ErrorKind::NotFound);
            if not_found {
                return framework_error(
                    format!("File not found: {path}"),
                    ErrorCode::FileNotFound,
                    None,
                    false,
                );
            }

            framework_error(
                format!("Failed to send file: {path}"),
                ErrorCode::SendFileFail,
                None,
                is_client_disconnect(&err),
            )
        })
    }

    // Set the status code of the response
    pub fn status(&mut self, code: u16) -> &mut Self {
        if !(100..=999).contains(&code) {
            panic!("Invalid status code: {code}");
        }
        self.status_code = code;
        self
    }

    // Set the Content-Disposition header to prompt the user to download a file
    pub fn attachment<'a>(&mut self, filename: impl Into<Option<&'a str>>) -> &mut Self {
        let content_disposition = match filename.into() {
            Some(filename) => format!("attachment; filename=\"{filename}\""),
            None => "attachment".to_string(),
        };
        self.set_header("Content-Disposition", content_disposition);
        self
    }

    // Redirects to a new URL
    pub fn redirect(&mut self, location: &str) -> Result<(), CpeakError> {
        self.write_head(302, &[("Location", location)]);
        self.end(Bytes::new())
    }

    // Send a json data back to the client.
    pub fn json(&mut self, data: impl Serialize) -> Result<(), CpeakError> {
        if self.headers_sent {
            return Ok(());
        }
        let body = json::stringify(&data)?;
        if let Some(compression) = self.compression.clone() {
            return compress_and_send_buffer(self, "application/json", body, &compression);
        }
        self.set_header("Content-Type", "application/json");
        self.end(body)
    }

    // Explicit compression entry point. A developer can use this in any custom handler to compress arbitrary responses
    pub async fn compress(
        &mut self,
        mime: &str,
        body: impl Into<Body>,
        size: impl Into<Option<u64>>,
    ) -> Result<(), CpeakError> {
        if self.headers_sent {
            return Ok(());
        }
        let Some(compression) = self.compression.clone() else {
            return Err(framework_error(
                "compression is not enabled. Pass `compression` to Cpeak::new(CpeakOptions { compression: Some(...), .. }) to use res.compress.",
                ErrorCode::CompressionNotEnabled,
                None,
                false,
            ));
        };
        compress_and_send(self, mime, body.into(), &compression, size.into()).await
    }

    // ------------------------------------------------------------------------------------
    // The rest of this block is what Node.js gives us through http.ServerResponse.
    // Just like in Node.js, setting a header after it was sent, or setting an invalid one,
    // throws (panics), and that ends up in your handleErr.

    // Set a header, replacing it if it's already set
    pub fn set_header(&mut self, name: &str, value: impl Display) -> &mut Self {
        self.assert_headers_not_sent();
        let (name, value) = header_pair(name, value);
        self.headers.insert(name, value);
        self
    }

    // Add a header without replacing the existing ones with the same name (useful for Set-Cookie)
    pub fn append_header(&mut self, name: &str, value: impl Display) -> &mut Self {
        self.assert_headers_not_sent();
        let (name, value) = header_pair(name, value);
        self.headers.append(name, value);
        self
    }

    pub fn get_header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }

    pub fn has_header(&self, name: &str) -> bool {
        self.headers.contains_key(name)
    }

    pub fn remove_header(&mut self, name: &str) -> &mut Self {
        self.assert_headers_not_sent();
        self.headers.remove(name);
        self
    }

    pub fn headers_sent(&self) -> bool {
        self.headers_sent
    }

    // Set the status code and the headers in one go. After this, the headers count as sent.
    pub fn write_head(&mut self, status_code: u16, headers: &[(&str, &str)]) -> &mut Self {
        self.status(status_code);
        for (name, value) in headers {
            self.set_header(name, value);
        }
        self.headers_sent = true;
        self
    }

    // Write a chunk of the body. The status and headers are sent along with the first chunk.
    // It waits while the client is slower than your app, so the memory usage stays low.
    pub async fn write(&mut self, chunk: impl Into<Bytes>) -> Result<(), CpeakError> {
        if self.finished {
            return Err(CpeakError::other("write after end"));
        }
        let chunk = chunk.into();
        self.headers_sent = true;

        // The response to a HEAD request has no body
        if self.req_is_head {
            return Ok(());
        }

        if !self.sender.is_streaming() {
            // The headers go to hyper as they are, instead of being copied
            let headers = std::mem::take(&mut self.headers);
            self.sender.start_stream(self.status_code, headers);
        }
        if chunk.is_empty() {
            return Ok(());
        }
        self.body_bytes += chunk.len() as u64;
        self.sender.send_chunk(chunk).await.map_err(|()| {
            let mut error = CpeakError::other("The client closed the connection");
            error.client_disconnect = true;
            error
        })
    }

    // Finish the response, with a last chunk of the body (pass "" or Bytes::new() for none).
    // It returns Ok(()), so you can write `return res.end(...)` in your handlers.
    pub fn end(&mut self, data: impl Into<Bytes>) -> Result<(), CpeakError> {
        if self.finished {
            return Ok(());
        }
        let data = data.into();
        self.finished = true;
        self.headers_sent = true;
        if !self.req_is_head {
            self.body_bytes += data.len() as u64;
        }

        if self.sender.is_streaming() {
            self.sender.close(Ok(data));
        } else {
            // The headers go to hyper as they are, instead of being copied
            let headers = std::mem::take(&mut self.headers);
            self.sender
                .send_head(self.status_code, headers, ResponseBody::Full(Some(data)));
        }
        self.emit_finish();
        Ok(())
    }

    // Run a function once the response is sent, like res.on("finish", ...) in Node.js.
    // It gets the status that the response went out with, so add it before the response ends
    // (in a middleware function, for example). It doesn't run for a response that was
    // destroyed instead of finished.
    pub fn on_finish(&mut self, listener: impl FnOnce(&FinishedResponse) + 'static) -> &mut Self {
        self.finish_listeners.push(Box::new(listener));
        self
    }

    // Run the on_finish() functions, in the order they were added
    fn emit_finish(&mut self) {
        if self.finish_listeners.is_empty() {
            return;
        }
        let finished = FinishedResponse {
            status_code: self.status_code,
            status_message: status_message(self.status_code),
        };
        for listener in std::mem::take(&mut self.finish_listeners) {
            // The response is already out, so a panic in here has nowhere to go
            if catch_unwind(AssertUnwindSafe(|| listener(&finished))).is_err() {
                eprintln!("[cpeak] A function passed to res.on_finish() panicked");
            }
        }
    }

    // Drop the connection without finishing the response, like req.socket.destroy() in Node.js
    pub(crate) fn destroy(&mut self) {
        self.finished = true;
        self.headers_sent = true;
        self.sender.abort();
    }

    // Cpeak calls this once your middleware and handlers are done. If they left the response
    // open, we end it here (without a body) so the client never gets a hung socket.
    pub(crate) fn finish(&mut self) {
        if !self.finished {
            let _ = self.end(Bytes::new());
        }
    }

    fn assert_headers_not_sent(&self) {
        if self.headers_sent {
            panic!("Cannot set headers after they are sent to the client");
        }
    }
}

// What a function passed to res.on_finish() gets: the status that the response went out with
// (res.statusCode and res.statusMessage in Node.js)
#[derive(Debug, Clone, Copy)]
pub struct FinishedResponse {
    pub status_code: u16,
    pub status_message: &'static str,
}

// The reason phrase that hyper sends along with a status code, like "Not Found" for 404
fn status_message(status_code: u16) -> &'static str {
    StatusCode::from_u16(status_code)
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
        .canonical_reason()
        .unwrap_or("<none>")
}

fn header_pair(name: &str, value: impl Display) -> (HeaderName, HeaderValue) {
    let header_name = static_header_name(name).unwrap_or_else(|| {
        HeaderName::from_bytes(name.as_bytes())
            .unwrap_or_else(|_| panic!("Header name must be a valid HTTP token [\"{name}\"]"))
    });

    thread_local! {
        // The value is formatted here first, so setting a header doesn't allocate a String
        static FORMATTED: RefCell<String> = const { RefCell::new(String::new()) };
    }

    let header_value = FORMATTED.with(|formatted| {
        let mut own_buffer = String::new();
        let mut shared_buffer = formatted.try_borrow_mut();
        let formatted = match &mut shared_buffer {
            Ok(formatted) => &mut **formatted,
            Err(_) => &mut own_buffer,
        };
        formatted.clear();
        let _ = write!(formatted, "{value}");

        // The values that Cpeak itself sets all the time don't need to be copied
        if let Some(value) = static_header_value(formatted) {
            return Some(value);
        }
        if formatted.is_ascii() {
            return HeaderValue::from_str(formatted).ok();
        }
        // Header values are Latin-1 (ISO-8859-1) strings, like in Node.js
        let bytes: Option<Vec<u8>> = formatted
            .chars()
            .map(|c| u8::try_from(u32::from(c)).ok())
            .collect();
        bytes.and_then(|bytes| HeaderValue::from_bytes(&bytes).ok())
    });
    let header_value =
        header_value.unwrap_or_else(|| panic!("Invalid character in header content [\"{name}\"]"));

    (header_name, header_value)
}

// The header names that are set all the time, the way they're usually written. These don't
// have to be checked and looked up letter by letter.
fn static_header_name(name: &str) -> Option<HeaderName> {
    Some(match name {
        "Content-Type" => header::CONTENT_TYPE,
        "Content-Length" => header::CONTENT_LENGTH,
        "Content-Encoding" => header::CONTENT_ENCODING,
        "Vary" => header::VARY,
        "Location" => header::LOCATION,
        "Set-Cookie" => header::SET_COOKIE,
        "Cache-Control" => header::CACHE_CONTROL,
        "Connection" => header::CONNECTION,
        _ => return None,
    })
}

// The header values that Cpeak sets itself (content types, Vary, CORS, compression...)
fn static_header_value(value: &str) -> Option<HeaderValue> {
    let value = match value {
        "application/json" => "application/json",
        "text/plain" => "text/plain",
        "text/html" => "text/html",
        "text/css" => "text/css",
        "application/javascript" => "application/javascript",
        "close" => "close",
        "true" => "true",
        "Origin" => "Origin",
        "Accept-Encoding" => "Accept-Encoding",
        "gzip" => "gzip",
        "br" => "br",
        "deflate" => "deflate",
        _ => return None,
    };
    Some(HeaderValue::from_static(value))
}

pub struct Cpeak {
    router: Router,
    middleware: Vec<Box<dyn DynMiddleware>>,
    handle_err: Option<Box<dyn DynErrorHandler>>,
    fallback: Option<Box<dyn DynHandler>>,
    // Each worker thread makes its own copy of it, for res.compression
    pub(crate) compression: Option<ResolvedCompressionConfig>,
    // Set when the app is served over HTTPS
    pub(crate) https: Option<ResolvedHttpsConfig>,
    // Set when the app has a `log` option
    pub(crate) log: Option<Arc<Logger>>,
}

impl Cpeak {
    pub fn new(options: CpeakOptions) -> Self {
        // The crypto library for everything in the program that uses rustls (see internal/tls.rs)
        internal::tls::install_crypto_provider();

        // How many threads cpeak::spawn() gets, if the app wants another number than the default
        if let Some(threads) = options.background_threads {
            internal::event_loop::set_background_threads(threads);
        }

        // Resolve compression options once at app startup.
        let compression = options.compression.map(resolve_compression_options);

        // Merge developer-supplied mime types with the defaults once at startup
        if !options.mime_types.is_empty() {
            register_mime_types(options.mime_types);
        }

        // Read the key and the certificate once at app startup.
        let https = options.https.map(resolve_https_options);

        // Where the lines of the log go, once at app startup.
        let log = options.log.map(internal::log::start);

        Self {
            router: Router::new(),
            middleware: Vec::new(),
            handle_err: None,
            fallback: None,
            compression,
            https,
            log,
        }
    }

    // Every request starts here (the server has already set res.compression, from its own copy
    // of this Cpeak's option)
    pub(crate) async fn handle_request(&self, req: &mut CpeakRequest, res: &mut CpeakResponse) {
        // Get the url without the URL parameters (query strings). Cloning the Uri doesn't copy
        // its text (it's counted), so we get to keep the path while the middleware has req.
        let uri = req.uri.clone();
        match uri.path_and_query() {
            Some(path_and_query) if uri.scheme().is_none() => {
                self.run_middleware(req, res, path_and_query.path()).await
            }
            // A full URL as the request target (like "GET http://example.com/users"). Rare.
            _ => {
                let url_without_queries = match req.url.find('?') {
                    Some(q_index) => req.url[..q_index].to_string(),
                    None => req.url.clone(),
                };
                self.run_middleware(req, res, &url_without_queries).await
            }
        }
    }

    // Run all the specific middleware functions for that router only and then run the handler
    async fn run_handler(
        &self,
        req: &mut CpeakRequest,
        res: &mut CpeakResponse,
        middleware: &[Box<dyn DynMiddleware>],
        cb: &dyn DynHandler,
        next_called: &NextCalled,
    ) {
        for route_middleware in middleware {
            // Handle the errors by passing them to handleErr to save developers from having to manually handle every error in every route middleware.
            if let Err(error) =
                CatchUnwind(next_called.run(route_middleware.call(req, res, next_fn()))).await
            {
                return self.dispatch_error(error, req, res).await;
            }
            // The middleware didn't call next(), so this is the end of the line for this request
            if !next_called.get() {
                return;
            }
        }

        // Our exit point...
        // Call the route handler with the modified req and res objects.
        // Also handle the errors by passing them to handleErr to save developers from having to manually handle every error in every handler.
        if let Err(error) = CatchUnwind(cb.call(req, res)).await {
            self.dispatch_error(error, req, res).await;
        }
    }

    // Run all the middleware functions (before_each functions) before we run the corresponding route
    async fn run_middleware(
        &self,
        req: &mut CpeakRequest,
        res: &mut CpeakResponse,
        url_without_queries: &str,
    ) {
        let next_called = NextCalled::default();

        for middleware in &self.middleware {
            if let Err(error) =
                CatchUnwind(next_called.run(middleware.call(req, res, next_fn()))).await
            {
                return self.dispatch_error(error, req, res).await;
            }
            // The middleware didn't call next(), so this is the end of the line for this request
            if !next_called.get() {
                return;
            }
        }

        // Our exit point...
        // (the router matches the method without caring about its case, so no need to lower-case it)
        if let Some(found) = self.router.find(&req.method, url_without_queries) {
            req.params = found.params;
            return self
                .run_handler(req, res, found.middleware, found.handler, &next_called)
                .await;
        }

        // If a fallback handler is registered, run it before falling back to the default 404
        if let Some(fallback) = &self.fallback {
            if let Err(error) = CatchUnwind(fallback.call(req, res)).await {
                self.dispatch_error(error, req, res).await;
            }
            return;
        }

        // If the requested route does not exist, and developer has not registered the fallback handler, return 404
        let _ = res.status(404).json(serde_json::json!({
            "error": format!("Cannot {} {}", req.method, url_without_queries)
        }));
    }

    // Routes every error path through the registered handleErr. Awaits
    // handleErr so its own async work (or a failing res.json under
    // compression) is caught. If handleErr itself fails, we log and send a
    // bare 500 so the client never gets a hung socket.
    async fn dispatch_error(
        &self,
        mut error: CpeakError,
        req: &mut CpeakRequest,
        res: &mut CpeakResponse,
    ) {
        if res.headers_sent() {
            res.destroy();
        } else {
            res.set_header("Connection", "close");
        }

        if is_client_disconnect(&error) && !error.client_disconnect {
            error.client_disconnect = true;
        }

        let Some(handle_err) = &self.handle_err else {
            eprintln!("[cpeak] Unhandled error (register an error handler with server.handle_err): {error:?}");
            if !res.headers_sent() {
                res.status_code = 500;
                let _ = res.end(Bytes::new());
            }
            return;
        };

        if let Err(handler_failure) = CatchUnwind(handle_err.call(error.clone(), req, res)).await {
            eprintln!(
                "[cpeak] handleErr failed while processing: {error:?}\nReason: {handler_failure:?}"
            );

            if !res.headers_sent() {
                res.status_code = 500;
                let _ = res.end(Bytes::new());
            }
        }
    }

    pub fn route<H: Handler>(&mut self, method: &str, path: &str, cb: H) {
        self.router.add(method, path, Vec::new(), Box::new(cb));
    }

    // Rust doesn't have variadic functions, so the route middleware functions go in as the
    // third argument: a single function, or a tuple like (require_auth, another_function, one_more)
    pub fn route_with<M, Marker, H>(&mut self, method: &str, path: &str, middleware: M, cb: H)
    where
        M: IntoRouteMiddleware<Marker>,
        H: Handler,
    {
        let middleware = middleware
            .into_route_middleware()
            .into_iter()
            .map(|middleware| middleware.0)
            .collect();
        self.router.add(method, path, middleware, Box::new(cb));
    }

    pub fn before_each<M: Middleware>(&mut self, cb: M) {
        self.middleware.push(Box::new(cb));
    }

    pub fn handle_err<E: ErrorHandler>(&mut self, cb: E) {
        self.handle_err = Some(Box::new(cb));
    }

    // This will handle any request that doesn't match any of the routes and middleware functions
    pub fn fallback<H: Handler>(&mut self, cb: H) {
        if self.fallback.is_some() {
            internal::errors::throw(framework_error(
                "Fallback handler is already registered. Only one fallback can be set per app.",
                ErrorCode::DuplicateFallback,
                None,
                false,
            ));
        }
        self.fallback = Some(Box::new(cb));
    }

    // Start listening. The first argument is a port (3000), or anything that resolves to
    // a socket address, like "127.0.0.1:3000". The callback runs once the server is listening.
    pub fn listen(self, address: impl ListenAddress, cb: impl FnOnce()) -> CpeakServer {
        internal::server::listen(self, address, cb)
    }
}

// Creates the `next` function that we hand to a middleware. Calling it lets the next one run.
// It doesn't hold any state (so creating one doesn't allocate): it sets the flag of the
// middleware that's running on this thread right now.
fn next_fn() -> Next {
    Box::new(|| {
        RUNNING_MIDDLEWARE.with(|running| {
            let flag = running.take();
            if let Some(flag) = &flag {
                flag.set(true);
            }
            running.set(flag);
        });
        Ok(())
    })
}

thread_local! {
    // The flag of the middleware function that's running on this thread right now
    static RUNNING_MIDDLEWARE: Cell<Option<Rc<Cell<bool>>>> = const { Cell::new(None) };
}

// Whether the last middleware function of a request called next(). The flag is only made
// when a middleware function runs, so a request that has none doesn't allocate it.
#[derive(Default)]
struct NextCalled(OnceCell<Rc<Cell<bool>>>);

impl NextCalled {
    fn get(&self) -> bool {
        self.0.get().is_some_and(|flag| flag.get())
    }

    // Run a middleware function, with next() pointing at this flag while it runs
    fn run<F>(&self, middleware: F) -> WithNext<'_, F> {
        let flag = self.0.get_or_init(Default::default);
        flag.set(false);
        WithNext { middleware, flag }
    }
}

struct WithNext<'a, F> {
    middleware: F,
    flag: &'a Rc<Cell<bool>>,
}

impl<F: Future + Unpin> Future for WithNext<'_, F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let previous = RUNNING_MIDDLEWARE.with(|running| running.replace(Some(self.flag.clone())));
        // Put back what was running before, even if the middleware function panics
        let _restore = RestoreRunning(previous);
        Pin::new(&mut self.middleware).poll(cx)
    }
}

struct RestoreRunning(Option<Rc<Cell<bool>>>);

impl Drop for RestoreRunning {
    fn drop(&mut self) {
        let previous = self.0.take();
        RUNNING_MIDDLEWARE.with(|running| running.set(previous));
    }
}

// Util functions
pub use crate::utils::{
    auth, cookie_parser, cors, hash_password, parse_json, precompress, render, serve_static,
    sign_cookie, swagger, verify_password,
};

pub use crate::utils::types::{
    AuthOptions, CookieOptions, CorsOptions, FindToken, OriginInput, PbkdfOptions, RevokeToken,
    SameSite, SaveToken, TokenRecord, VerifiedToken,
};
pub use crate::utils::{CookieParserOptions, ParseJsonOptions, ServeStaticOptions};

pub fn cpeak() -> Cpeak {
    Cpeak::new(CpeakOptions::default())
}
