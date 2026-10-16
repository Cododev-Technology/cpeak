// The HTTP server plumbing. In the Node.js version, all of this comes from the node:http
// module. Here we build it on top of hyper and tokio.
//
// Like Node.js, each worker thread runs its own single-threaded event loop. Unlike Node.js,
// we start one worker per CPU core, so your app uses all the cores of the machine.
//
// On Linux, every worker listens on the server's address with a socket of its own, and the
// operating system spreads the new connections across them (the way nginx and Drogon do it).
// Elsewhere, one more thread accepts the connections and hands them out to the workers in turn.
//
// Everything a worker does stays on its own thread: nothing is shared between the workers
// while requests are being handled, so the cores never wait on each other.

use std::cell::{Cell, RefCell};
use std::collections::hash_map::Entry;
use std::collections::VecDeque;
use std::future::{Future, IntoFuture};
use std::io;
use std::net::{
    IpAddr, Ipv6Addr, SocketAddr, TcpListener as StdTcpListener, TcpStream as StdTcpStream,
    ToSocketAddrs,
};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{ready, Context, Poll, Waker};
use std::time::{Duration, Instant};

use bytes::Bytes;
use hyper::body::{Frame, Incoming, SizeHint};
use hyper::header::HeaderMap;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use socket2::{Domain, Socket, Type};
use tokio::sync::{mpsc, watch};

use crate::internal::event_loop;
use crate::internal::log;
use crate::internal::shutdown;
use crate::internal::socket;
use crate::internal::timers;
use crate::internal::tls;
use crate::internal::types::ResolvedCompressionConfig;
use crate::types::StringMap;
use crate::{Cpeak, CpeakRequest, CpeakResponse};

// The body we hand over to hyper. It's either the whole body at once (res.end(), res.json(), ...)
// or a stream of chunks that the handlers keep writing with res.write().
pub(crate) enum ResponseBody {
    Full(Option<Bytes>),
    Stream(Rc<BodyStream>),
}

impl hyper::body::Body for ResponseBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        match &mut *self {
            ResponseBody::Full(body) => Poll::Ready(body.take().map(|body| Ok(Frame::data(body)))),
            ResponseBody::Stream(chunks) => chunks
                .poll_next(cx)
                .map(|chunk| chunk.map(|chunk| chunk.map(Frame::data))),
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(self, ResponseBody::Full(None))
    }

    fn size_hint(&self) -> SizeHint {
        match self {
            ResponseBody::Full(Some(body)) => SizeHint::with_exact(body.len() as u64),
            ResponseBody::Full(None) => SizeHint::with_exact(0),
            ResponseBody::Stream(_) => SizeHint::default(),
        }
    }
}

impl Drop for ResponseBody {
    fn drop(&mut self) {
        // hyper is done with the body. If the stream isn't over, the client went away.
        if let ResponseBody::Stream(chunks) = self {
            chunks.receiver_dropped();
        }
    }
}

// How many bytes of a streamed body can wait in line for the client before res.write() waits
// (backpressure), so the memory usage stays low when the client is slower than your app
const STREAM_HIGH_WATER_MARK: usize = 64 * 1024;

// How much of an unread request body we read and throw away after the response is sent
const MAX_DISCARDED_BODY: usize = 1024 * 1024;

// The chunks of a streamed response, on their way from res.write() to hyper
pub(crate) struct BodyStream {
    state: RefCell<StreamState>,
}

#[derive(Default)]
struct StreamState {
    chunks: VecDeque<Bytes>,
    queued_bytes: usize,
    // Some(Ok) once the response has ended, Some(Err) when it was destroyed
    end: Option<io::Result<()>>,
    // hyper dropped the body before the end, most likely because the client went away
    receiver_dropped: bool,
    // hyper, waiting for the next chunk
    receiver: Option<Waker>,
    // res.write(), waiting for the client to catch up
    sender: Option<Waker>,
}

impl BodyStream {
    fn new() -> Self {
        Self {
            state: RefCell::new(StreamState::default()),
        }
    }

    fn poll_next(&self, cx: &mut Context<'_>) -> Poll<Option<io::Result<Bytes>>> {
        let mut state = self.state.borrow_mut();
        if let Some(chunk) = state.chunks.pop_front() {
            state.queued_bytes -= chunk.len();
            if let Some(sender) = state.sender.take() {
                sender.wake();
            }
            return Poll::Ready(Some(Ok(chunk)));
        }
        match state.end.take() {
            Some(Ok(())) => {
                state.end = Some(Ok(()));
                Poll::Ready(None)
            }
            Some(Err(error)) => {
                state.end = Some(Ok(()));
                Poll::Ready(Some(Err(error)))
            }
            None => {
                set_waker(&mut state.receiver, cx);
                Poll::Pending
            }
        }
    }

    // Queue a chunk, waiting while the client is slower than us. Fails if the client went away.
    async fn send(&self, chunk: Bytes) -> Result<(), ()> {
        std::future::poll_fn(|cx| {
            let mut state = self.state.borrow_mut();
            if state.receiver_dropped {
                return Poll::Ready(Err(()));
            }
            if state.queued_bytes < STREAM_HIGH_WATER_MARK {
                return Poll::Ready(Ok(()));
            }
            set_waker(&mut state.sender, cx);
            Poll::Pending
        })
        .await?;

        let mut state = self.state.borrow_mut();
        state.queued_bytes += chunk.len();
        state.chunks.push_back(chunk);
        if let Some(receiver) = state.receiver.take() {
            receiver.wake();
        }
        Ok(())
    }

    // End the stream, with a last chunk (or with an error, to abort the response)
    fn finish(&self, last: io::Result<Bytes>) {
        let mut state = self.state.borrow_mut();
        if state.end.is_some() {
            return;
        }
        match last {
            Ok(chunk) => {
                if !chunk.is_empty() {
                    state.queued_bytes += chunk.len();
                    state.chunks.push_back(chunk);
                }
                state.end = Some(Ok(()));
            }
            Err(error) => state.end = Some(Err(error)),
        }
        if let Some(receiver) = state.receiver.take() {
            receiver.wake();
        }
    }

    fn receiver_dropped(&self) {
        let mut state = self.state.borrow_mut();
        state.receiver_dropped = true;
        if let Some(sender) = state.sender.take() {
            sender.wake();
        }
    }
}

// Where the status line and the headers wait for hyper, once the handlers send them.
// hyper handles one request at a time on a connection, so each connection needs just one.
struct ResponseSlot {
    response: RefCell<Option<Response<ResponseBody>>>,
    // The response was destroyed before its head was sent
    destroyed: Cell<bool>,
    // hyper let go of the request before it got the head, because the client went away
    abandoned: Cell<bool>,
    // hyper, waiting for the head
    waiting: RefCell<Option<Waker>>,
    // The handlers are running as part of hyper's own turn right now (see serve()). hyper
    // looks for the head as soon as they return, so there's no need to wake it up for it.
    in_hypers_turn: Cell<bool>,
}

impl ResponseSlot {
    fn new() -> Self {
        Self {
            response: RefCell::new(None),
            destroyed: Cell::new(false),
            abandoned: Cell::new(false),
            waiting: RefCell::new(None),
            in_hypers_turn: Cell::new(false),
        }
    }

    fn reset(&self) {
        *self.response.borrow_mut() = None;
        self.destroyed.set(false);
    }

    fn put(&self, response: Response<ResponseBody>) {
        // Nobody is left to send it. Letting go of it right here is how a streamed body finds
        // out that the client went away, so res.write() fails instead of waiting forever for
        // someone to read what it wrote.
        if self.abandoned.get() {
            return;
        }
        *self.response.borrow_mut() = Some(response);
        self.wake();
    }

    fn destroy(&self) {
        self.destroyed.set(true);
        self.wake();
    }

    fn wake(&self) {
        if self.in_hypers_turn.get() {
            return;
        }
        if let Some(waiting) = self.waiting.borrow_mut().take() {
            waiting.wake();
        }
    }

    fn poll_response(&self, cx: &mut Context<'_>) -> Poll<io::Result<Response<ResponseBody>>> {
        if let Some(response) = self.response.borrow_mut().take() {
            return Poll::Ready(Ok(response));
        }
        // No head means the response was destroyed, so hyper closes the connection
        if self.destroyed.get() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "the response was destroyed",
            )));
        }
        set_waker(&mut self.waiting.borrow_mut(), cx);
        Poll::Pending
    }
}

fn set_waker(slot: &mut Option<Waker>, cx: &Context<'_>) {
    match slot {
        Some(waker) if waker.will_wake(cx.waker()) => {}
        _ => *slot = Some(cx.waker().clone()),
    }
}

// Sends the response over to hyper: first the status and headers, then the body.
pub(crate) struct ResponseSender {
    slot: Rc<ResponseSlot>,
    head_sent: bool,
    stream: Option<Rc<BodyStream>>,
}

impl ResponseSender {
    fn new(slot: Rc<ResponseSlot>) -> Self {
        Self {
            slot,
            head_sent: false,
            stream: None,
        }
    }

    pub(crate) fn is_streaming(&self) -> bool {
        self.stream.is_some()
    }

    // Send the status line and the headers along with the body (or the receiving end of the body stream)
    pub(crate) fn send_head(&mut self, status_code: u16, headers: HeaderMap, body: ResponseBody) {
        if self.head_sent {
            return;
        }
        self.head_sent = true;
        let mut response = Response::new(body);
        *response.status_mut() =
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        *response.headers_mut() = headers;
        self.slot.put(response);
    }

    // Send the head with a streaming body, so the handlers can keep writing chunks
    pub(crate) fn start_stream(&mut self, status_code: u16, headers: HeaderMap) {
        let stream = Rc::new(BodyStream::new());
        self.stream = Some(stream.clone());
        self.send_head(status_code, headers, ResponseBody::Stream(stream));
    }

    // Send a chunk, waiting while the client is slower than us. Fails if the client went away.
    pub(crate) async fn send_chunk(&mut self, chunk: Bytes) -> Result<(), ()> {
        match &self.stream {
            Some(stream) => stream.send(chunk).await,
            None => Err(()),
        }
    }

    // End the stream, with a last chunk (or with an error, to abort the response)
    pub(crate) fn close(&mut self, last: io::Result<Bytes>) {
        if let Some(stream) = self.stream.take() {
            stream.finish(last);
        }
    }

    // Drop the connection without finishing the response, like req.socket.destroy() in Node.js
    pub(crate) fn abort(&mut self) {
        // Without a head, hyper closes the connection instead of sending a response
        if !self.head_sent {
            self.head_sent = true;
            self.slot.destroy();
        }
        self.close(Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "the response was destroyed",
        )));
    }
}

impl Drop for ResponseSender {
    fn drop(&mut self) {
        // The handlers went away without finishing the response (they panicked out of our
        // reach). Don't leave the client hanging.
        if !self.head_sent || self.stream.is_some() {
            self.abort();
        }
    }
}

// Anything you can pass to server.listen() as the address:
// a port (3000), or anything that resolves to a socket address ("127.0.0.1:3000", ("localhost", 3000), ...)
pub trait ListenAddress {
    fn bind(self) -> io::Result<StdTcpListener>;
}

impl ListenAddress for u16 {
    fn bind(self) -> io::Result<StdTcpListener> {
        // Like Node.js, listen on all the interfaces, over IPv6 and IPv4 when the machine supports it
        let any_v6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), self);
        bind_address(any_v6, true)
            .or_else(|_| bind_address(SocketAddr::from(([0, 0, 0, 0], self)), false))
    }
}

impl ListenAddress for SocketAddr {
    fn bind(self) -> io::Result<StdTcpListener> {
        bind_address(self, false)
    }
}

impl ListenAddress for (IpAddr, u16) {
    fn bind(self) -> io::Result<StdTcpListener> {
        bind_address(SocketAddr::from(self), false)
    }
}

impl ListenAddress for ([u8; 4], u16) {
    fn bind(self) -> io::Result<StdTcpListener> {
        bind_address(SocketAddr::from(self), false)
    }
}

impl ListenAddress for (&str, u16) {
    fn bind(self) -> io::Result<StdTcpListener> {
        bind_first(self.to_socket_addrs()?)
    }
}

impl ListenAddress for &str {
    fn bind(self) -> io::Result<StdTcpListener> {
        bind_first(self.to_socket_addrs()?)
    }
}

impl ListenAddress for String {
    fn bind(self) -> io::Result<StdTcpListener> {
        self.as_str().bind()
    }
}

fn bind_first(addresses: impl Iterator<Item = SocketAddr>) -> io::Result<StdTcpListener> {
    let mut last_error = io::Error::new(io::ErrorKind::InvalidInput, "no address to listen on");
    for address in addresses {
        match bind_address(address, false) {
            Ok(listener) => return Ok(listener),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn bind_address(address: SocketAddr, dual_stack: bool) -> io::Result<StdTcpListener> {
    // The sockets of our workers share the address (see one_per_worker), and the operating
    // system lets any program of the same user join them. So a second copy of your app
    // wouldn't fail to start, it would quietly take a share of the connections. To still
    // fail the way Node.js does (EADDRINUSE), we first try the address with a socket that
    // doesn't share. A port of 0 gets a free port, which nobody listens on yet.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if address.port() != 0 {
        new_socket(address, dual_stack)?.bind(&address.into())?;
    }

    let socket = new_socket(address, dual_stack)?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    socket.set_reuse_port(true)?;
    socket.bind(&address.into())?;
    socket.listen(BACKLOG)?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

fn new_socket(address: SocketAddr, dual_stack: bool) -> io::Result<Socket> {
    let socket = Socket::new(Domain::for_address(address), Type::STREAM, None)?;
    if dual_stack {
        socket.set_only_v6(false)?;
    }
    // Like Node.js on Unix, so restarting your server doesn't fail while old connections are closing
    #[cfg(not(windows))]
    socket.set_reuse_address(true)?;
    Ok(socket)
}

// A listening socket for every worker, all on the address of the one we have. The operating
// system then hands every new connection to one of them, so the workers take their connections
// in themselves: no thread in between that all of them have to go through, which is what
// holds a server back on a machine with many cores when connections come and go quickly.
//
// Only Linux spreads the connections across such sockets (SO_REUSEPORT). Elsewhere, and when
// this doesn't work out, we get the one socket back, and an acceptor thread hands out its connections.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn one_per_worker(
    listener: StdTcpListener,
    workers: usize,
) -> Result<Vec<StdTcpListener>, StdTcpListener> {
    let more = || -> io::Result<Vec<StdTcpListener>> {
        let first = socket2::SockRef::from(&listener);
        if !first.reuse_port()? {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let address = listener.local_addr()?;
        let dual_stack = address.is_ipv6() && !first.only_v6()?;

        let mut listeners = Vec::with_capacity(workers);
        for _ in 1..workers {
            let socket = new_socket(address, dual_stack)?;
            socket.set_reuse_port(true)?;
            socket.bind(&address.into())?;
            socket.listen(BACKLOG)?;
            socket.set_nonblocking(true)?;
            listeners.push(socket.into());
        }
        Ok(listeners)
    };

    match more() {
        Ok(mut listeners) => {
            listeners.push(listener);
            Ok(listeners)
        }
        Err(_) => Err(listener),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn one_per_worker(
    listener: StdTcpListener,
    _workers: usize,
) -> Result<Vec<StdTcpListener>, StdTcpListener> {
    Err(listener)
}

// How many connections can wait to be accepted. When more than that arrive at once, the
// operating system drops the rest, and each of those clients only tries again a second
// later. Node.js asks for 511. We ask for as many as the operating system allows (it lowers
// this to its own limit: net.core.somaxconn on Linux, kern.ipc.somaxconn on macOS), so a
// burst of thousands of new connections gets in on a server that is set up for it.
const BACKLOG: i32 = 65535;

// The running server that server.listen() gives back (the http.Server in Node.js).
// Call wait() on it (or await it in async code) to keep your program running for as
// long as the server does.
#[derive(Clone)]
#[must_use = "the server runs in the background, call .wait() on it to keep your program running"]
pub struct CpeakServer {
    address: SocketAddr,
    closed: Arc<watch::Sender<bool>>,
    running_workers: watch::Receiver<usize>,
}

impl CpeakServer {
    // The address the server is listening on (useful if you listened on port 0)
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    // Stop accepting new connections. The requests in progress get to finish, and once
    // every connection is closed, waiting on (or awaiting) the server returns.
    pub fn close(&self) {
        self.closed.send_replace(true);
    }

    // Block the current thread for as long as the server runs. The workers bring their own
    // event loops, so a plain `fn main` is all you need, with no async runtime of your own.
    // Inside async code, await the server instead, since this would block the event loop.
    pub fn wait(self) {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap_or_else(|error| panic!("[cpeak] Failed to wait for the server: {error}"))
            .block_on(self.into_future());
    }

    // Resolves once every connection is closed and the workers have stopped
    pub(crate) async fn stopped(&self) {
        let mut running_workers = self.running_workers.clone();
        let _ = running_workers.wait_for(|running| *running == 0).await;
    }
}

impl IntoFuture for CpeakServer {
    type Output = ();
    type IntoFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.stopped().await;
            // What the workers logged on their way out
            log::written().await;

            // The program was asked to stop (SIGTERM). It exits by itself once the functions
            // passed to cpeak::on_shutdown() are done, and until then whoever waits for the
            // server keeps waiting, so a `fn main` doesn't return before they have run.
            if shutdown::is_shutting_down() {
                std::future::pending::<()>().await;
            }
        })
    }
}

// A connection on its way from the acceptor to a worker
type Accepted = (StdTcpStream, SocketAddr);

// Where a worker gets its connections from
enum Connections {
    // Its own listening socket, until the server closes
    Listener(StdTcpListener, watch::Receiver<bool>),
    // The acceptor thread
    Acceptor(mpsc::UnboundedReceiver<Accepted>),
}

pub(crate) fn listen(
    app: Cpeak,
    address: impl ListenAddress,
    callback: impl FnOnce(),
) -> CpeakServer {
    let listener = address
        .bind()
        .unwrap_or_else(|error| panic!("[cpeak] Failed to listen: {error}"));
    let local_address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("[cpeak] Failed to listen: {error}"));
    let app = Arc::new(app);

    let workers = std::thread::available_parallelism().map_or(1, |cores| cores.get());
    let (closed, closed_receiver) = watch::channel(false);
    let (running_workers, running_workers_receiver) = watch::channel(workers);
    let running_workers = Arc::new(running_workers);

    let start_worker = |connections: Connections| {
        let app = app.clone();
        let running_workers = running_workers.clone();
        std::thread::Builder::new()
            .name("cpeak-worker".to_string())
            .spawn(move || {
                // Counts the worker as stopped when the thread ends, even if it panicked
                let _stopped = WorkerStopped(running_workers);
                run_worker(app, connections);
            })
            .unwrap_or_else(|error| panic!("[cpeak] Failed to start a worker: {error}"));
    };

    match one_per_worker(listener, workers) {
        Ok(listeners) => {
            for listener in listeners {
                start_worker(Connections::Listener(listener, closed_receiver.clone()));
            }
        }
        Err(listener) => {
            let mut worker_queues = Vec::with_capacity(workers);
            for _ in 0..workers {
                let (queue, connections) = mpsc::unbounded_channel();
                worker_queues.push(queue);
                start_worker(Connections::Acceptor(connections));
            }
            std::thread::Builder::new()
                .name("cpeak-acceptor".to_string())
                .spawn(move || run_acceptor(listener, worker_queues, closed_receiver))
                .unwrap_or_else(|error| panic!("[cpeak] Failed to start the acceptor: {error}"));
        }
    }

    callback();

    let server = CpeakServer {
        address: local_address,
        closed: Arc::new(closed),
        running_workers: running_workers_receiver,
    };
    // So the server closes when the program is asked to stop (SIGTERM)
    shutdown::add_server(server.clone());
    server
}

struct WorkerStopped(Arc<watch::Sender<usize>>);

impl Drop for WorkerStopped {
    fn drop(&mut self) {
        self.0.send_modify(|running| *running -= 1);
    }
}

// Accepts the connections and hands them out to the workers in turn
fn run_acceptor(
    listener: StdTcpListener,
    worker_queues: Vec<mpsc::UnboundedSender<Accepted>>,
    mut closed: watch::Receiver<bool>,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("[cpeak] Failed to start the acceptor: {error}"));

    runtime.block_on(async move {
        let mut next_worker = 0;
        let accepting = accept_connections(listener, |stream, remote_address| {
            // The worker registers the connection with its own event loop
            let _ = worker_queues[next_worker].send((stream, remote_address));
            next_worker = (next_worker + 1) % worker_queues.len();
        });

        // Stop listening right away (this drops the listening socket). The workers let the
        // requests in progress finish, and stop once their connections are closed.
        tokio::select! {
            _ = accepting => {}
            _ = until_closed(&mut closed) => {}
        }
    });
}

// Takes the new connections off a listening socket, one after the other, for as long as it runs
async fn accept_connections(
    listener: StdTcpListener,
    mut connection: impl FnMut(StdTcpStream, SocketAddr),
) {
    let listener = socket::Listener::new(listener)
        .unwrap_or_else(|error| panic!("[cpeak] Failed to listen: {error}"));

    loop {
        match listener.accept().await {
            Ok((stream, remote_address)) => connection(stream, remote_address),
            // The client gave up before we accepted it, nothing to worry about
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset
                ) => {}
            Err(error) => {
                // Most likely we ran out of file descriptors. Wait a bit for some to free up.
                eprintln!("[cpeak] Failed to accept a connection: {error}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

// Resolves once server.close() is called. If the CpeakServer was dropped without closing it,
// nobody can close the server anymore, so just like in Node.js, it keeps running.
async fn until_closed(closed: &mut watch::Receiver<bool>) {
    if closed.wait_for(|closed| *closed).await.is_err() {
        std::future::pending::<()>().await;
    }
}

// What a worker shares with its connections. It never leaves the worker's thread.
struct Worker {
    app: Arc<Cpeak>,
    // res.compression, one copy per worker, so the requests don't all bump the same counter
    compression: Option<Rc<ResolvedCompressionConfig>>,
    // There is a cpeak::log_requests(), so the requests need the time they came in
    log_requests: bool,
    closing: Cell<bool>,
    // The connections to wake up once the server starts closing (by connection id)
    connections: RefCell<Vec<Option<Waker>>>,
    free_ids: RefCell<Vec<usize>>,
}

impl Worker {
    fn new(app: Arc<Cpeak>) -> Self {
        let compression = app
            .compression
            .as_ref()
            .map(|config| Rc::new(config.clone()));
        let log_requests = log::logs_requests();
        Self {
            app,
            compression,
            log_requests,
            closing: Cell::new(false),
            connections: RefCell::new(Vec::new()),
            free_ids: RefCell::new(Vec::new()),
        }
    }

    fn add_connection(&self) -> usize {
        let mut connections = self.connections.borrow_mut();
        match self.free_ids.borrow_mut().pop() {
            Some(id) => id,
            None => {
                connections.push(None);
                connections.len() - 1
            }
        }
    }

    fn remove_connection(&self, id: usize) {
        self.connections.borrow_mut()[id] = None;
        self.free_ids.borrow_mut().push(id);
    }

    // The server is closing: close the idle connections, and let the others finish their request
    fn start_closing(&self) {
        self.closing.set(true);
        for waker in self.connections.borrow_mut().iter_mut() {
            if let Some(waker) = waker.take() {
                waker.wake();
            }
        }
    }
}

// Resolves once the worker's server starts closing
struct Closing<'a> {
    worker: &'a Worker,
    id: usize,
}

impl Future for Closing<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.worker.closing.get() {
            return Poll::Ready(());
        }
        set_waker(&mut self.worker.connections.borrow_mut()[self.id], cx);
        Poll::Pending
    }
}

fn run_worker(app: Arc<Cpeak>, connections: Connections) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("[cpeak] Failed to start a worker: {error}"));
    // For cpeak::spawn_local()
    event_loop::mark_as_worker();
    // For cpeak::sleep()
    timers::start(&runtime);
    keep_timer_driver_set(&runtime);
    // For cpeak::log() and the request lines
    if app.log.is_some() || log::logs_requests() {
        let logger = app.log.clone().unwrap_or_else(log::default_logger);
        log::start_worker(&runtime, logger);
    }
    let local = tokio::task::LocalSet::new();

    local.spawn_local(async move {
        let worker = Rc::new(Worker::new(app));
        let serve = {
            let worker = worker.clone();
            move |stream, remote_address| {
                tokio::task::spawn_local(serve_connection(worker.clone(), stream, remote_address));
            }
        };

        match connections {
            Connections::Listener(listener, mut closed) => {
                // In a task of its own, so that taking a connection in never has to look at
                // anything the workers share (like whether the server was closed)
                let accepting = tokio::task::spawn_local(accept_connections(listener, serve));
                until_closed(&mut closed).await;
                // Stop listening right away: the listening socket goes with the task
                accepting.abort();
                let _ = accepting.await;
            }
            Connections::Acceptor(mut connections) => {
                // Until the acceptor stops, which is when the server closes
                while let Some((stream, remote_address)) = connections.recv().await {
                    serve(stream, remote_address);
                }
            }
        }
        // Let the requests in progress finish, and close the connections
        worker.start_closing();
    });

    // Runs until every task is done: the connections, and their requests
    runtime.block_on(local);
    log::stop_worker();
}

// The timer hyper uses for the 30 seconds it gives a client to send the headers of a request.
// hyper asks for a new one for every request, but only waits on it when the request doesn't
// arrive in one piece. So ours becomes a real timer the first time hyper waits on it, and
// until then it's just the time it's due.
#[derive(Clone)]
struct HeaderTimer;

impl hyper::rt::Timer for HeaderTimer {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn hyper::rt::Sleep>> {
        self.sleep_until(Instant::now() + duration)
    }

    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn hyper::rt::Sleep>> {
        Box::pin(HeaderSleep {
            deadline,
            sleep: None,
        })
    }
}

struct HeaderSleep {
    deadline: Instant,
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl Future for HeaderSleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let deadline = self.deadline;
        self.sleep
            .get_or_insert_with(|| Box::pin(tokio::time::sleep_until(deadline.into())))
            .as_mut()
            .poll(cx)
    }
}

impl hyper::rt::Sleep for HeaderSleep {}

// hyper gives every connection that waits for a request 30 seconds to send its headers, with a
// tokio timer. When tokio gets a timer that is due before the moment its event loop planned
// to wake up, it wakes the event loop up to plan again, and that is a system call. An event
// loop that went to sleep without any timer has no such moment, so every one of those
// timers costs that system call: one per request, whenever the handlers of all the requests
// are waiting for something (a database, cpeak::sleep(), ...) while the event loop sleeps.
//
// This timer is always due sooner than hyper's, so the event loop always has a moment it
// planned to wake up, and hyper's timers never move it. It wakes the worker up once every
// 10 seconds, to do nothing.
fn keep_timer_driver_set(runtime: &tokio::runtime::Runtime) {
    runtime.spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

async fn serve_connection(worker: Rc<Worker>, stream: StdTcpStream, remote_address: SocketAddr) {
    let Ok(socket) = socket::Socket::new(stream) else {
        return;
    };

    if let Some(https) = &worker.app.https {
        // An HTTPS server: hyper reads and writes through the TLS layer
        if let Ok(socket) = tls::TlsSocket::new(socket, https) {
            serve_requests(worker, socket, remote_address).await;
        }
        return;
    }

    serve_requests(worker, socket, remote_address).await;
}

// Serves the requests of a connection, one after the other, until it closes
async fn serve_requests<T>(worker: Rc<Worker>, socket: T, remote_address: SocketAddr)
where
    T: hyper::rt::Read + hyper::rt::Write + Unpin + 'static,
{
    let socket = Rc::new(RefCell::new(HeldBack::new(socket)));

    let slot = Rc::new(ResponseSlot::new());
    let service = {
        let worker = worker.clone();
        service_fn(move |request: Request<Incoming>| {
            serve(worker.clone(), slot.clone(), request, remote_address)
        })
    };
    let connection = http1::Builder::new()
        .timer(HeaderTimer)
        .serve_connection(HoldingBack(socket.clone()), service);
    tokio::pin!(connection);

    let id = worker.add_connection();
    let closing = Closing {
        worker: &worker,
        id,
    };
    tokio::pin!(closing);
    let mut closing_started = false;

    std::future::poll_fn(|cx| {
        if !closing_started && closing.as_mut().poll(cx).is_ready() {
            // Let the request in progress finish, then close the connection
            closing_started = true;
            connection.as_mut().graceful_shutdown();
        }
        // hyper handles all the requests it has, and then we send out everything it wrote
        let done = connection.as_mut().poll(cx);
        socket.borrow_mut().send(cx);
        done.map(|_| ())
    })
    .await;

    worker.remove_connection(id);
}

// When a client pipelines its requests (sends the next ones without waiting for the responses),
// hyper writes each response as soon as it's ready: one write to the socket per response. So
// we hold back the small writes, and send everything hyper wrote while handling the requests it
// had at once, with a single write (serve_connection calls send() after each of hyper's turns).
// That's what Drogon does with the responses to the requests it got in one read.
//
// (hyper has a pipeline_flush option for this, but with it on, hyper also takes in a streamed
// body as fast as we give it, so a slow client would make the whole response pile up in memory.)
const HOLD_BACK_LIMIT: usize = 64 * 1024;

// After sending, a connection keeps a buffer of up to this size for next time
const KEPT_BUFFER: usize = 16 * 1024;

struct HeldBack<T> {
    io: T,
    // What hyper wrote that hasn't gone out yet, and how much of it already has
    held_back: Vec<u8>,
    sent: usize,
    // The socket failed while we were sending what we held back. hyper gets the error next time.
    error: Option<io::Error>,
}

impl<T: hyper::rt::Write + Unpin> HeldBack<T> {
    fn new(io: T) -> Self {
        Self {
            io,
            held_back: Vec::new(),
            sent: 0,
            error: None,
        }
    }

    // Send what we held back, after one of hyper's turns. If the client can't take it all right
    // now, the socket wakes the connection up once it can.
    fn send(&mut self, cx: &mut Context<'_>) {
        if let Poll::Ready(Err(error)) = self.poll_send(cx) {
            self.error = Some(error);
        }
    }

    fn poll_send(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(error) = self.error.take() {
            return Poll::Ready(Err(error));
        }
        while self.sent < self.held_back.len() {
            let written =
                ready!(Pin::new(&mut self.io).poll_write(cx, &self.held_back[self.sent..]))?;
            if written == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.sent += written;
        }
        self.sent = 0;
        if self.held_back.capacity() > KEPT_BUFFER {
            self.held_back = Vec::new();
        } else {
            self.held_back.clear();
        }
        // Over HTTPS, all of it is encrypted by now, and this is where it goes out
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_write_vectored(
        &mut self,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let len: usize = bufs.iter().map(|buf| buf.len()).sum();
        if !self.held_back.is_empty() && self.held_back.len() + len > HOLD_BACK_LIMIT {
            ready!(self.poll_send(cx))?;
        }
        if let Some(error) = self.error.take() {
            return Poll::Ready(Err(error));
        }
        // Big writes (like the chunks of a file) go out right away, without a copy
        if self.held_back.is_empty() && len >= HOLD_BACK_LIMIT {
            return Pin::new(&mut self.io).poll_write_vectored(cx, bufs);
        }
        for buf in bufs {
            self.held_back.extend_from_slice(buf);
        }
        Poll::Ready(Ok(len))
    }
}

// The socket as hyper sees it
struct HoldingBack<T>(Rc<RefCell<HeldBack<T>>>);

impl<T: hyper::rt::Read + Unpin> hyper::rt::Read for HoldingBack<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0.borrow_mut().io).poll_read(cx, buf)
    }
}

impl<T: hyper::rt::Write + Unpin> hyper::rt::Write for HoldingBack<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.0
            .borrow_mut()
            .poll_write_vectored(cx, &[io::IoSlice::new(buf)])
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.0.borrow_mut().poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    // What hyper flushes goes out at the end of its turn (see serve_connection)
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.0.borrow_mut().error.take() {
            Some(error) => Poll::Ready(Err(error)),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut socket = self.0.borrow_mut();
        ready!(socket.poll_send(cx))?;
        Pin::new(&mut socket.io).poll_shutdown(cx)
    }
}

// Handles one request. Cpeak runs your middleware and handlers right here, and as soon as they
// send the status and headers, we hand the response over to hyper. If they stream the body (or
// just take a while), they keep going in their own task while hyper sends what they write.
async fn serve(
    worker: Rc<Worker>,
    slot: Rc<ResponseSlot>,
    request: Request<Incoming>,
    remote_address: SocketAddr,
) -> io::Result<Response<ResponseBody>> {
    slot.reset();

    let (mut parts, incoming) = request.into_parts();
    let mut req = CpeakRequest::new(
        parts.method.as_str().to_string(),
        parts.uri,
        incoming_headers(&parts.headers),
        remote_address,
        incoming,
    );
    // The request's header map becomes the response's (emptied). hyper hands it back to us
    // with the next request once it has sent the response, so a connection never allocates another one.
    parts.headers.clear();
    let mut res = CpeakResponse::new(ResponseSender::new(slot.clone()), &req, parts.headers);
    res.compression = worker.compression.clone();
    let started = worker.log_requests.then(Instant::now);

    let handlers: Pin<Box<dyn Future<Output = ()>>> = Box::pin(async move {
        worker.app.handle_request(&mut req, &mut res).await;
        res.finish();
        if let (Some(started), Some(request_log)) = (started, res.request_log) {
            log::request(&req, &res, started, request_log);
        }

        // Like Node.js does once the response is finished, read and throw away whatever is left
        // of the request body (if your handlers didn't read it), so the connection can be reused
        // for the next request. We only do it up to a limit, and close the connection after that.
        let mut discarded = 0;
        while discarded <= MAX_DISCARDED_BODY {
            match req.chunk().await {
                Ok(Some(chunk)) => discarded += chunk.len(),
                _ => break,
            }
        }
    });
    let mut handling = Handling(Some(handlers), slot.clone());

    // The handlers run right here, as part of the connection (so reading the request body
    // doesn't need any hopping between tasks), until they send the status and headers. If
    // they're not done by then (they stream the body, or keep going after res.end()), they
    // carry on in a task of their own while hyper sends the response.
    let response = std::future::poll_fn(|cx| {
        if let Some(future) = &mut handling.0 {
            let done = {
                let _in_hypers_turn = InHypersTurn::new(&slot);
                future.as_mut().poll(cx).is_ready()
            };
            if done {
                handling.0 = None;
            }
        }
        slot.poll_response(cx)
    })
    .await;
    handling.detach();
    response
}

// Marks the slot while the handlers run as part of hyper's turn, also when they panic
struct InHypersTurn<'a>(&'a ResponseSlot);

impl<'a> InHypersTurn<'a> {
    fn new(slot: &'a ResponseSlot) -> Self {
        slot.in_hypers_turn.set(true);
        Self(slot)
    }
}

impl Drop for InHypersTurn<'_> {
    fn drop(&mut self) {
        self.0.in_hypers_turn.set(false);
    }
}

// The handlers of a request while they run as part of the connection. If hyper drops them before
// they're done (the client went away), they still get to finish in a task of their own, just like
// a request handler in Node.js runs to the end even when the client is gone.
struct Handling(Option<Pin<Box<dyn Future<Output = ()>>>>, Rc<ResponseSlot>);

impl Handling {
    fn detach(&mut self) {
        if let Some(handling) = self.0.take() {
            tokio::task::spawn_local(handling);
        }
    }
}

impl Drop for Handling {
    fn drop(&mut self) {
        if self.0.is_some() {
            // Whatever they still send has nowhere to go
            self.1.abandoned.set(true);
        }
        // Outside of a running worker (it's shutting down), there's nothing to finish them on
        if self.0.is_some() && !std::thread::panicking() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.detach()));
        }
    }
}

// Build req.headers the way Node.js does: lower-cased names, and when a header is sent more
// than once, the values are joined together (with "; " for cookie and ", " for the rest).
// Duplicates of the headers below are discarded, just like Node.js does.
fn incoming_headers(headers: &HeaderMap) -> StringMap {
    let mut result = StringMap::with_capacity(headers.keys_len());

    for (name, value) in headers {
        let name = name.as_str();
        let value = latin1_string(value.as_bytes());

        match result.entry(name.to_string()) {
            Entry::Vacant(header) => {
                header.insert(value);
            }
            Entry::Occupied(mut header) => match name {
                "age"
                | "authorization"
                | "content-length"
                | "content-type"
                | "etag"
                | "expires"
                | "from"
                | "host"
                | "if-modified-since"
                | "if-unmodified-since"
                | "last-modified"
                | "location"
                | "max-forwards"
                | "proxy-authorization"
                | "referer"
                | "retry-after"
                | "server"
                | "user-agent" => {}
                "cookie" => {
                    let existing = header.get_mut();
                    existing.push_str("; ");
                    existing.push_str(&value);
                }
                _ => {
                    let existing = header.get_mut();
                    existing.push_str(", ");
                    existing.push_str(&value);
                }
            },
        }
    }

    result
}

// Header values are Latin-1 (ISO-8859-1) strings
fn latin1_string(bytes: &[u8]) -> String {
    if bytes.is_ascii() {
        // SAFETY: ASCII is valid UTF-8
        return unsafe { String::from_utf8_unchecked(bytes.to_vec()) };
    }
    bytes.iter().map(|&byte| byte as char).collect()
}
