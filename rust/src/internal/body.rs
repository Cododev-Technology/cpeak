// The bodies exchanged with other crates: the request's (RequestBody), and any body given to
// res.send_response() (HttpResponseBody).

use std::any::Any;
use std::error::Error;
use std::fmt;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Buf, Bytes};
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::BodyExt;
use hyper::body::Incoming;

pub(crate) type BoxError = Box<dyn Error + Send + Sync>;

// The body of a request, read by req.chunk(). It's the body coming off the connection, or one
// that a tower layer put in its place (like a body with a size limit).
pub struct RequestBody {
    kind: Kind,
}

enum Kind {
    Incoming(Incoming),
    Other(UnsyncBoxBody<Bytes, BoxError>),
    // No body, or all of it was read
    Done,
}

impl RequestBody {
    pub(crate) fn incoming(incoming: Incoming) -> Self {
        Self {
            kind: Kind::Incoming(incoming),
        }
    }

    // A body without anything in it
    pub fn empty() -> Self {
        Self { kind: Kind::Done }
    }

    // Any other body, to put in place of the one the request came with
    pub fn new<B>(body: B) -> Self
    where
        B: HttpBody + Send + 'static,
        B::Data: Buf,
        B::Error: Into<BoxError>,
    {
        // Not wrapped again if it's one already
        let mut body = Some(body);
        if let Some(ours) = (&mut body as &mut dyn Any).downcast_mut::<Option<RequestBody>>() {
            if let Some(ours) = ours.take() {
                return ours;
            }
        }
        let Some(body) = body else {
            return Self::empty();
        };
        let body = body
            .map_frame(|frame| frame.map_data(|mut data| data.copy_to_bytes(data.remaining())))
            .map_err(Into::into);
        Self {
            kind: Kind::Other(body.boxed_unsync()),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        matches!(self.kind, Kind::Done)
    }

    pub(crate) fn set_done(&mut self) {
        self.kind = Kind::Done;
    }
}

impl Default for RequestBody {
    fn default() -> Self {
        Self::empty()
    }
}

impl HttpBody for RequestBody {
    type Data = Bytes;
    type Error = BodyError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BodyError>>> {
        match &mut self.get_mut().kind {
            Kind::Incoming(incoming) => Pin::new(incoming)
                .poll_frame(cx)
                .map_err(|error| BodyError(ErrorKind::Hyper(error))),
            Kind::Other(body) => Pin::new(body)
                .poll_frame(cx)
                .map_err(|error| BodyError(ErrorKind::Other(error))),
            Kind::Done => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        match &self.kind {
            Kind::Incoming(incoming) => incoming.is_end_stream(),
            Kind::Other(body) => body.is_end_stream(),
            Kind::Done => true,
        }
    }

    fn size_hint(&self) -> SizeHint {
        match &self.kind {
            Kind::Incoming(incoming) => incoming.size_hint(),
            Kind::Other(body) => body.size_hint(),
            Kind::Done => SizeHint::with_exact(0),
        }
    }
}

impl fmt::Debug for RequestBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RequestBody")
    }
}

// Why a request body couldn't be read: the error of hyper, or of the body a tower layer put in,
// with its message and source() unchanged
pub struct BodyError(ErrorKind);

enum ErrorKind {
    Hyper(hyper::Error),
    Other(BoxError),
}

impl BodyError {
    fn inner(&self) -> &(dyn Error + 'static) {
        match &self.0 {
            ErrorKind::Hyper(error) => error,
            ErrorKind::Other(error) => error.as_ref(),
        }
    }
}

impl fmt::Display for BodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.inner(), f)
    }
}

impl fmt::Debug for BodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.inner(), f)
    }
}

impl Error for BodyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.inner().source()
    }
}

// A body from another crate, on its way to hyper (res.send_response())
pub(crate) struct HttpResponseBody(Pin<Box<dyn HttpBody<Data = Bytes, Error = io::Error>>>);

impl HttpResponseBody {
    pub(crate) fn new<B>(body: B) -> Self
    where
        B: HttpBody + 'static,
        B::Data: Buf,
        B::Error: Into<BoxError>,
    {
        let body = body
            .map_frame(|frame| frame.map_data(|mut data| data.copy_to_bytes(data.remaining())))
            .map_err(|error| io::Error::other(error.into()));
        Self(Box::pin(body))
    }

    pub(crate) fn poll_frame(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        self.0.as_mut().poll_frame(cx)
    }

    pub(crate) fn is_end_stream(&self) -> bool {
        self.0.is_end_stream()
    }

    pub(crate) fn size_hint(&self) -> SizeHint {
        self.0.size_hint()
    }
}
