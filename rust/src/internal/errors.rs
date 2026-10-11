use std::any::Any;
use std::fmt;
use std::future::Future;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use serde::{Serialize, Serializer};

// The error that flows through Cpeak and ends up in your handleErr.
// In the Node.js version you can throw anything, like `throw { status: 400, message: "Title too long." }`.
// In Rust, you return it instead: `return Err(CpeakError::new(400, "Title too long."));`
#[derive(Clone)]
pub struct CpeakError {
    pub status: Option<u16>,
    pub message: String,
    pub code: Option<ErrorCode>,
    // True for the errors created by the framework itself
    pub cpeak_err: bool,
    // True when the error happened because the client went away (closed the connection)
    pub client_disconnect: bool,
    source: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl CpeakError {
    // The same as `throw { status, message }` in the Node.js version
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status: Some(status),
            message: message.into(),
            code: None,
            cpeak_err: false,
            client_disconnect: false,
            source: None,
        }
    }

    // An error without a status, the same as `throw new Error(message)` in the Node.js version.
    // Your handleErr treats these as unexpected errors.
    pub fn other(message: impl Into<String>) -> Self {
        Self {
            status: None,
            message: message.into(),
            code: None,
            cpeak_err: false,
            client_disconnect: false,
            source: None,
        }
    }

    // The original error, if this one was created from another error (for example with the `?` operator)
    pub fn source(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        self.source.as_deref()
    }
}

// Any error can be turned into a CpeakError, so you can use the `?` operator in your
// routes and middleware. These errors have no status, so handleErr sees them as unexpected.
impl<E> From<E> for CpeakError
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn from(error: E) -> Self {
        let client_disconnect = (&error as &dyn Any)
            .downcast_ref::<io::Error>()
            .is_some_and(|io_error| CLIENT_DISCONNECT_CODES.contains(&io_error.kind()));

        Self {
            status: None,
            message: error.to_string(),
            code: None,
            cpeak_err: false,
            client_disconnect,
            source: Some(Arc::new(error)),
        }
    }
}

impl fmt::Display for CpeakError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for CpeakError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("CpeakError");
        debug
            .field("status", &self.status)
            .field("message", &self.message)
            .field("code", &self.code)
            .field("cpeak_err", &self.cpeak_err)
            .field("client_disconnect", &self.client_disconnect);
        if let Some(source) = &self.source {
            debug.field("source", source);
        }
        debug.finish()
    }
}

// A utility function to create an error for the framework itself
pub fn framework_error(
    message: impl Into<String>,
    code: ErrorCode,
    status: Option<u16>,
    client_disconnect: bool,
) -> CpeakError {
    CpeakError {
        status,
        message: message.into(),
        code: Some(code),
        cpeak_err: true,
        client_disconnect,
        source: None,
    }
}

// Rust has no exceptions to throw, so for mistakes in how the framework is set up (like
// registering the same route twice) we panic with the error code and the message.
pub(crate) fn throw(error: CpeakError) -> ! {
    match error.code {
        Some(code) => panic!("{code}: {}", error.message),
        None => panic!("{}", error.message),
    }
}

const CLIENT_DISCONNECT_CODES: [io::ErrorKind; 3] = [
    io::ErrorKind::BrokenPipe,
    io::ErrorKind::ConnectionReset,
    io::ErrorKind::ConnectionAborted,
];

pub fn is_client_disconnect(err: &CpeakError) -> bool {
    err.client_disconnect
        || err
            .source
            .as_deref()
            .and_then(|source| source.downcast_ref::<io::Error>())
            .is_some_and(|io_error| CLIENT_DISCONNECT_CODES.contains(&io_error.kind()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    MissingMime,
    FileNotFound,
    NotAFile,
    SendFileFail,
    InvalidJson,
    PayloadTooLarge,
    WeakSecret,
    CompressionNotEnabled,
    RenderNotEnabled,
    AuthNotEnabled,
    // For router:
    DuplicateRoute,
    InvalidRoute,
    DuplicateFallback,
    RenderFail,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MissingMime => "CPEAK_ERR_MISSING_MIME",
            Self::FileNotFound => "CPEAK_ERR_FILE_NOT_FOUND",
            Self::NotAFile => "CPEAK_ERR_NOT_A_FILE",
            Self::SendFileFail => "CPEAK_ERR_SEND_FILE_FAIL",
            Self::InvalidJson => "CPEAK_ERR_INVALID_JSON",
            Self::PayloadTooLarge => "CPEAK_ERR_PAYLOAD_TOO_LARGE",
            Self::WeakSecret => "CPEAK_ERR_WEAK_SECRET",
            Self::CompressionNotEnabled => "CPEAK_ERR_COMPRESSION_NOT_ENABLED",
            Self::RenderNotEnabled => "CPEAK_ERR_RENDER_NOT_ENABLED",
            Self::AuthNotEnabled => "CPEAK_ERR_AUTH_NOT_ENABLED",
            Self::DuplicateRoute => "CPEAK_ERR_DUPLICATE_ROUTE",
            Self::InvalidRoute => "CPEAK_ERR_INVALID_ROUTE",
            Self::DuplicateFallback => "CPEAK_ERR_DUPLICATE_FALLBACK",
            Self::RenderFail => "CPEAK_ERR_RENDER_FAIL",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// So `json!({ "code": error.code })` gives you the same "CPEAK_ERR_..." strings as the Node.js version
impl Serialize for ErrorCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

// A panic is the closest thing Rust has to an unexpected thrown error. We catch panics
// coming from your routes and middleware and hand them to handleErr (as errors without a
// status), just like the Node.js version does with anything that gets thrown.
pub(crate) struct CatchUnwind<F>(pub(crate) F);

impl<F> Future for CatchUnwind<F>
where
    F: Future<Output = Result<(), CpeakError>> + Unpin,
{
    type Output = Result<(), CpeakError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match catch_unwind(AssertUnwindSafe(|| Pin::new(&mut self.0).poll(cx))) {
            Ok(poll) => poll,
            Err(payload) => Poll::Ready(Err(panic_to_error(payload))),
        }
    }
}

fn panic_to_error(payload: Box<dyn Any + Send>) -> CpeakError {
    let message = if let Some(message) = payload.downcast_ref::<&str>() {
        message.to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "A route or middleware function panicked".to_string()
    };
    CpeakError::other(message)
}
