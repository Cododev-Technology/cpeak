// cpeak::layer() and cpeak::service(): tower Layers as middleware functions, and tower Services
// as route handlers.
//
// A layer wraps a Service. Here, that service is the rest of the request (Rest): the middleware
// functions after the layer, and the route. A cpeak::layer() middleware function leaves its
// layer on the request, and the runner (lib.rs) calls the layer with the rest of the request.
// What the rest writes to its res comes back to the layer as an http::Response.
//
// To a layer, Rest looks like an axum Router: it takes and gives axum's Body, it never fails,
// it owns what it uses, and it is Send and Sync. It still only runs on the worker's thread
// (SameThread), so handlers don't need to be Send.

use std::cell::RefCell;
use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use axum_core::body::Body;
use bytes::{Buf, Bytes};
use http_body::{Body as HttpBody, Frame, SizeHint};
use tower_layer::Layer;
use tower_service::Service;

use crate::internal::body::BoxError;
use crate::internal::log::RequestLog;
use crate::internal::server::{capture_response, CapturedHead, ResponseBody};
use crate::types::LocalBoxFuture;
use crate::{
    CpeakError, CpeakRequest, CpeakResponse, Handler, Middleware, Next, RequestBody, RestOfChain,
};

// A tower Layer as a middleware function: server.before_each(cpeak::layer(TimeoutLayer::new(...))).
// It wraps the middleware functions added after it and the route. In a route_with() tuple, it
// wraps the route middleware after it and the handler.
pub fn layer<L, B>(layer: L) -> impl Middleware
where
    L: Layer<Rest> + Send + Sync + 'static,
    L::Service: Service<http::Request<Body>, Response = http::Response<B>>,
    <L::Service as Service<http::Request<Body>>>::Error: Into<BoxError>,
    B: HttpBody + 'static,
    B::Data: Buf,
    B::Error: Into<BoxError>,
{
    // Leaked, so a request can point at it without a reference count
    let layer: &'static dyn DynLayer = Box::leak(Box::new(LayerAdapter {
        layer,
        body: PhantomData::<fn() -> B>,
    }));

    async move |req: &mut CpeakRequest, _res: &mut CpeakResponse, next: Next| {
        req.pending_layer = Some(layer);
        next()
    }
}

// A tower Service as a route handler: server.route("post", "/graphql", cpeak::service(...)).
// The service gets the request as an http::Request, and its http::Response is sent.
pub fn service<S, B>(service: S) -> impl Handler
where
    S: Service<http::Request<RequestBody>, Response = http::Response<B>>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Error: Into<BoxError>,
    B: HttpBody + 'static,
    B::Data: Buf,
    B::Error: Into<BoxError>,
{
    async move |req: &mut CpeakRequest, res: &mut CpeakResponse| {
        let mut service = service.clone();
        std::future::poll_fn(|cx| service.poll_ready(cx))
            .await
            .map_err(|error| CpeakError::from_boxed(error.into()))?;
        let response = service
            .call(req.take_http())
            .await
            .map_err(|error| CpeakError::from_boxed(error.into()))?;
        res.send_response(response)
    }
}

// The rest of the request, as the Service that a cpeak::layer() wraps. It can be called once.
pub struct Rest(SameThread<Rc<LayerContext>>);

impl Clone for Rest {
    fn clone(&self) -> Self {
        Rest(SameThread::new(self.0.get().clone()))
    }
}

impl<B> Service<http::Request<B>> for Rest
where
    B: HttpBody + Send + 'static,
    B::Data: Buf,
    B::Error: Into<BoxError>,
{
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = RestFuture;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<B>) -> RestFuture {
        let context = self.0.get().clone();
        let running = Box::pin(context.call(request.map(RequestBody::new)));
        RestFuture(SameThread::new(running))
    }
}

// What Rest gives back when it's called
pub struct RestFuture(SameThread<LocalBoxFuture<'static, http::Response<Body>>>);

impl Future for RestFuture {
    type Output = Result<http::Response<Body>, Infallible>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().0.get_mut().as_mut().poll(cx).map(Ok)
    }
}

// What the runner calls for the layer that a cpeak::layer() left on the request
pub(crate) trait DynLayer: Send + Sync {
    fn run<'a>(
        &'a self,
        req: &'a mut CpeakRequest,
        res: &'a mut CpeakResponse,
        rest: RestOfChain,
    ) -> LocalBoxFuture<'a, Result<(), CpeakError>>;
}

struct LayerAdapter<L, B> {
    layer: L,
    body: PhantomData<fn() -> B>,
}

impl<L, B> DynLayer for LayerAdapter<L, B>
where
    L: Layer<Rest> + Send + Sync + 'static,
    L::Service: Service<http::Request<Body>, Response = http::Response<B>>,
    <L::Service as Service<http::Request<Body>>>::Error: Into<BoxError>,
    B: HttpBody + 'static,
    B::Data: Buf,
    B::Error: Into<BoxError>,
{
    fn run<'a>(
        &'a self,
        req: &'a mut CpeakRequest,
        res: &'a mut CpeakResponse,
        rest: RestOfChain,
    ) -> LocalBoxFuture<'a, Result<(), CpeakError>> {
        Box::pin(async move {
            let request = req.take_http().map(Body::new);
            // The rest of the request writes to a res of its own, whose response goes to the layer
            let (sender, head) = capture_response();
            let inner_res = res.for_layer(sender);
            let context = Rc::new(LayerContext {
                rest,
                parts: RefCell::new(Some((req.take_whole(), inner_res))),
                head,
                remaining: RefCell::new(None),
                finished: RefCell::new(None),
            });

            let mut service = self.layer.layer(Rest(SameThread::new(context.clone())));
            let response = async {
                std::future::poll_fn(|cx| service.poll_ready(cx)).await?;
                service.call(request).await
            }
            .await;
            drop(service);

            let result = match response {
                Ok(response) => {
                    let sent = res.send_response(response);
                    // The rest of the request may still be writing the body
                    let remaining = context.remaining.borrow_mut().take();
                    if let Some(remaining) = remaining {
                        let finished = remaining.await;
                        *context.finished.borrow_mut() = Some(finished);
                    }
                    sent
                }
                Err(error) => Err(CpeakError::from_boxed(error.into())),
            };

            // The request comes back, for the request log and for what's left of its body
            let finished = context.finished.borrow_mut().take();
            if let Some(finished) = finished {
                *req = finished.req;
                if finished.request_log.is_some() {
                    res.request_log = finished.request_log;
                }
            } else if let Some((inner_req, _)) = context.parts.borrow_mut().take() {
                *req = inner_req;
            }
            result
        })
    }
}

// What the rest of a request needs while a layer has it
struct LayerContext {
    rest: RestOfChain,
    // Taken when the layer calls the rest of the request
    parts: RefCell<Option<(CpeakRequest, CpeakResponse)>>,
    head: CapturedHead,
    // The rest of the request, if it's still writing the body when its head is sent
    remaining: RefCell<Option<LocalBoxFuture<'static, Finished>>>,
    finished: RefCell<Option<Finished>>,
}

struct Finished {
    req: CpeakRequest,
    request_log: Option<&'static RequestLog>,
}

impl LayerContext {
    async fn call(self: Rc<Self>, request: http::Request<RequestBody>) -> http::Response<Body> {
        let Some((mut req, mut res)) = self.parts.borrow_mut().take() else {
            eprintln!("[cpeak] A tower layer called the rest of the request a second time");
            return failed();
        };
        req.put_http(request);
        let rest = self.rest.clone();
        let chain: LocalBoxFuture<'static, Finished> = Box::pin(async move {
            rest.run(&mut req, &mut res).await;
            res.finish();
            Finished {
                request_log: res.request_log,
                req,
            }
        });

        // Run the rest of the request until it sends its head
        let mut chain = Some(chain);
        let head = std::future::poll_fn(|cx| {
            if let Some(running) = &mut chain {
                if let Poll::Ready(finished) = running.as_mut().poll(cx) {
                    chain = None;
                    *self.finished.borrow_mut() = Some(finished);
                }
            }
            self.head.poll_head(cx)
        })
        .await;
        if let Some(chain) = chain {
            *self.remaining.borrow_mut() = Some(chain);
        }

        match head {
            Ok(response) => response.map(|body| Body::new(RestBody(SameThread::new(body)))),
            // The response was destroyed before its head was sent
            Err(_) => failed(),
        }
    }
}

fn failed() -> http::Response<Body> {
    let mut response = http::Response::new(Body::empty());
    *response.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
    response
}

// The body of the rest of the request's response
struct RestBody(SameThread<ResponseBody>);

impl HttpBody for RestBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        Pin::new(self.get_mut().0.get_mut()).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.0.get().is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.0.get().size_hint()
    }
}

// A value that stays on the thread that made it, but is Send and Sync, so tower layers that
// require that accept it. Using it on another thread panics, and dropping it there leaks it.
struct SameThread<T> {
    thread: usize,
    value: ManuallyDrop<T>,
}

// SAFETY: the value is only used and dropped on the thread that made it (see check())
unsafe impl<T> Send for SameThread<T> {}
unsafe impl<T> Sync for SameThread<T> {}

impl<T> SameThread<T> {
    fn new(value: T) -> Self {
        Self {
            thread: this_thread(),
            value: ManuallyDrop::new(value),
        }
    }

    fn get(&self) -> &T {
        self.check();
        &self.value
    }

    fn get_mut(&mut self) -> &mut T {
        self.check();
        &mut self.value
    }

    fn check(&self) {
        if this_thread() != self.thread {
            panic!(
                "[cpeak] A tower layer moved the request to another thread. Cpeak runs each request on the thread that took it."
            );
        }
    }
}

impl<T> Drop for SameThread<T> {
    fn drop(&mut self) {
        if this_thread() == self.thread {
            // SAFETY: dropped once, here, on its own thread
            unsafe { ManuallyDrop::drop(&mut self.value) }
        }
    }
}

// Tells the threads apart: the address of a thread-local is different on each running thread
fn this_thread() -> usize {
    thread_local! {
        static MARK: u8 = const { 0 };
    }
    MARK.with(|mark| mark as *const u8 as usize)
}
