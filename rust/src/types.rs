use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use crate::internal::errors::CpeakError;
use crate::internal::types::{CompressionOptions, HttpsOptions};
use crate::{CpeakRequest, CpeakResponse};

// For constructor options passed to `Cpeak::new()`
#[derive(Debug, Clone, Default)]
pub struct CpeakOptions {
    pub compression: Option<CompressionOptions>,
    pub mime_types: StringMap,
    // Serve the app over HTTPS, with this key and certificate
    pub https: Option<HttpsOptions>,
}

pub type StringMap = HashMap<String, String>;

// Calling `next()` lets the next middleware function run (or the route handler if there's none left).
// It returns `Ok(())`, so you can write `return next();` just like in the Node.js version.
pub type Next = Box<dyn FnOnce() -> Result<(), CpeakError>>;

// beforeEach middleware: (req, res, next)
pub trait Middleware:
    AsyncFn(&mut CpeakRequest, &mut CpeakResponse, Next) -> Result<(), CpeakError>
    + Send
    + Sync
    + 'static
{
}

impl<F> Middleware for F where
    F: AsyncFn(&mut CpeakRequest, &mut CpeakResponse, Next) -> Result<(), CpeakError>
        + Send
        + Sync
        + 'static
{
}

// Route middleware: (req, res, next)
pub use self::Middleware as RouteMiddleware;

// Route handlers: (req, res)
pub trait Handler:
    AsyncFn(&mut CpeakRequest, &mut CpeakResponse) -> Result<(), CpeakError> + Send + Sync + 'static
{
}

impl<F> Handler for F where
    F: AsyncFn(&mut CpeakRequest, &mut CpeakResponse) -> Result<(), CpeakError>
        + Send
        + Sync
        + 'static
{
}

// The error handler: (error, req, res)
pub trait ErrorHandler:
    AsyncFn(CpeakError, &mut CpeakRequest, &mut CpeakResponse) -> Result<(), CpeakError>
    + Send
    + Sync
    + 'static
{
}

impl<F> ErrorHandler for F where
    F: AsyncFn(CpeakError, &mut CpeakRequest, &mut CpeakResponse) -> Result<(), CpeakError>
        + Send
        + Sync
        + 'static
{
}

// Rust doesn't have variadic functions, so `server.route_with()` takes the route middleware
// as a single function, or as a tuple of functions: `(require_auth, another_function, one_more)`.
pub trait IntoRouteMiddleware<Marker> {
    #[doc(hidden)]
    fn into_route_middleware(self) -> Vec<BoxedMiddleware>;
}

#[doc(hidden)]
pub struct SingleMiddleware;

impl<M: RouteMiddleware> IntoRouteMiddleware<SingleMiddleware> for M {
    fn into_route_middleware(self) -> Vec<BoxedMiddleware> {
        vec![BoxedMiddleware(Box::new(self))]
    }
}

macro_rules! impl_into_route_middleware_for_tuples {
    ($($middleware:ident),+) => {
        impl<$($middleware: RouteMiddleware),+> IntoRouteMiddleware<($($middleware,)+)> for ($($middleware,)+) {
            #[allow(non_snake_case)]
            fn into_route_middleware(self) -> Vec<BoxedMiddleware> {
                let ($($middleware,)+) = self;
                vec![$(BoxedMiddleware(Box::new($middleware))),+]
            }
        }
    };
}

impl_into_route_middleware_for_tuples!(A);
impl_into_route_middleware_for_tuples!(A, B);
impl_into_route_middleware_for_tuples!(A, B, C);
impl_into_route_middleware_for_tuples!(A, B, C, D);
impl_into_route_middleware_for_tuples!(A, B, C, D, E);
impl_into_route_middleware_for_tuples!(A, B, C, D, E, F);
impl_into_route_middleware_for_tuples!(A, B, C, D, E, F, G);
impl_into_route_middleware_for_tuples!(A, B, C, D, E, F, G, H);
impl_into_route_middleware_for_tuples!(A, B, C, D, E, F, G, H, I);
impl_into_route_middleware_for_tuples!(A, B, C, D, E, F, G, H, I, J);
impl_into_route_middleware_for_tuples!(A, B, C, D, E, F, G, H, I, J, K);
impl_into_route_middleware_for_tuples!(A, B, C, D, E, F, G, H, I, J, K, L);

// ---------------------------------------------------------------------------------------
// Internals: `AsyncFn` closures can't be stored as trait objects directly, so we wrap them
// in these object-safe traits that box up the returned futures.

pub(crate) type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[doc(hidden)]
pub struct BoxedMiddleware(pub(crate) Box<dyn DynMiddleware>);

pub(crate) trait DynMiddleware: Send + Sync {
    fn call<'a>(
        &'a self,
        req: &'a mut CpeakRequest,
        res: &'a mut CpeakResponse,
        next: Next,
    ) -> LocalBoxFuture<'a, Result<(), CpeakError>>;
}

impl<M: Middleware> DynMiddleware for M {
    fn call<'a>(
        &'a self,
        req: &'a mut CpeakRequest,
        res: &'a mut CpeakResponse,
        next: Next,
    ) -> LocalBoxFuture<'a, Result<(), CpeakError>> {
        Box::pin(self(req, res, next))
    }
}

pub(crate) trait DynHandler: Send + Sync {
    fn call<'a>(
        &'a self,
        req: &'a mut CpeakRequest,
        res: &'a mut CpeakResponse,
    ) -> LocalBoxFuture<'a, Result<(), CpeakError>>;
}

impl<H: Handler> DynHandler for H {
    fn call<'a>(
        &'a self,
        req: &'a mut CpeakRequest,
        res: &'a mut CpeakResponse,
    ) -> LocalBoxFuture<'a, Result<(), CpeakError>> {
        Box::pin(self(req, res))
    }
}

pub(crate) trait DynErrorHandler: Send + Sync {
    fn call<'a>(
        &'a self,
        error: CpeakError,
        req: &'a mut CpeakRequest,
        res: &'a mut CpeakResponse,
    ) -> LocalBoxFuture<'a, Result<(), CpeakError>>;
}

impl<E: ErrorHandler> DynErrorHandler for E {
    fn call<'a>(
        &'a self,
        error: CpeakError,
        req: &'a mut CpeakRequest,
        res: &'a mut CpeakResponse,
    ) -> LocalBoxFuture<'a, Result<(), CpeakError>> {
        Box::pin(self(error, req, res))
    }
}
