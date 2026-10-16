// Node.js has one event loop, and everything runs on it. Each of Cpeak's worker threads has
// an event loop of its own, but those are only there to handle the requests.
//
// This is one more event loop, on a few threads of its own (4, unless the app sets
// CpeakOptions::background_threads), that starts the first time it's needed. cpeak::spawn()
// runs things on it that have to keep going whatever the requests do, and cpeak::run() uses
// it for the programs that have no server at all.

use std::cell::Cell;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::LazyLock;

use tokio::runtime::{Builder, Runtime};
use tokio::task::JoinHandle;

use crate::internal::tls;

// How many threads the background event loop gets when the app doesn't say
const DEFAULT_BACKGROUND_THREADS: usize = 4;

// What the app asked for in CpeakOptions::background_threads (0 if it didn't)
static BACKGROUND_THREADS: AtomicUsize = AtomicUsize::new(0);
// How many threads the background event loop started with (0 until it has started)
static STARTED_WITH: AtomicUsize = AtomicUsize::new(0);

static EVENT_LOOP: LazyLock<Runtime> = LazyLock::new(|| {
    let threads = match BACKGROUND_THREADS.load(Ordering::SeqCst) {
        0 => DEFAULT_BACKGROUND_THREADS,
        threads => threads,
    };
    STARTED_WITH.store(threads, Ordering::SeqCst);

    Builder::new_multi_thread()
        .worker_threads(threads)
        .thread_name("cpeak-background")
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            panic!("[cpeak] Failed to start the background event loop: {error}")
        })
});

// Called by Cpeak::new() when the app sets CpeakOptions::background_threads. The background
// event loop picks it up when it starts, so if something already started it (a call to
// cpeak::spawn() or cpeak::run() before Cpeak::new()), it keeps the threads it has.
pub(crate) fn set_background_threads(threads: usize) {
    let threads = threads.max(1);
    BACKGROUND_THREADS.store(threads, Ordering::SeqCst);

    let started_with = STARTED_WITH.load(Ordering::SeqCst);
    if started_with != 0 && started_with != threads {
        eprintln!(
            "[cpeak] background_threads is {threads}, but the background event loop had already started with {started_with} threads, so it keeps those. Call Cpeak::new() before cpeak::spawn() and cpeak::run()."
        );
    }
}

// Run something in the background, like calling an async function without awaiting it in
// Node.js:
//
//     cpeak::spawn(async { send_the_welcome_email().await });
//
// It keeps running after your route has sent its response. Await what spawn() gives back if
// you want to wait until it's done.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    EVENT_LOOP.spawn(future)
}

thread_local! {
    // True on the threads of the workers, the only ones with an event loop for spawn_local()
    static ON_A_WORKER: Cell<bool> = const { Cell::new(false) };
}

// Called by every worker thread as it starts (see run_worker() in server.rs)
pub(crate) fn mark_as_worker() {
    ON_A_WORKER.set(true);
}

// Run something in the background on the worker thread that is handling the request,
// without making the response wait for it. It's for small things that happen on many
// requests (an analytics event, filling a cache): it never leaves the thread, so it costs
// less than cpeak::spawn(), and what you give it doesn't have to be Send.
//
//     cpeak::spawn_local(async move { record_the_view(view).await });
//
// It only works in a route or a middleware function, where there is a worker thread to
// run on. Anywhere else (in cpeak::run(), or in what you gave to cpeak::spawn()) it panics.
pub fn spawn_local<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    if !ON_A_WORKER.get() {
        panic!(
            "[cpeak] cpeak::spawn_local() only works in a route or a middleware function. Use cpeak::spawn() here instead."
        );
    }
    tokio::task::spawn_local(future)
}

// Run blocking code (a heavy computation, a library that has no async version) on a pool of
// threads made for it, so it doesn't stop an event loop. It's what the thread pool of Node.js
// (libuv's) is for, the one that runs crypto.pbkdf2() and fs.readFile() for you:
//
//     let hash = cpeak::spawn_blocking(move || hash_the_password(&password)).await?;
//
// The pool belongs to the background event loop, so every worker shares it, and it works
// in cpeak::run() too. Threads are added as they're needed and go away once idle.
pub fn spawn_blocking<F, R>(function: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    EVENT_LOOP.spawn_blocking(function)
}

// Run async code to the end from a regular function, for a program that has no server
// (a script, a worker, a seed, a test). It's what the top-level await is in Node.js:
//
//     fn main() {
//         cpeak::run(async {
//             // your async code...
//         });
//     }
pub fn run<F: Future>(future: F) -> F::Output {
    tls::install_crypto_provider();
    EVENT_LOOP.block_on(future)
}

// Makes this the event loop that the calling thread sets things up on, for as long as what
// it returns is around
#[cfg(unix)]
pub(crate) fn enter() -> tokio::runtime::EnterGuard<'static> {
    EVENT_LOOP.enter()
}
