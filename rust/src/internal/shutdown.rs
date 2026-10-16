// Stopping the program the gentle way. A process manager (PM2, systemd, Docker, Kubernetes, ...)
// stops a program by sending it SIGTERM, and a program that does nothing about it is gone
// right there, in the middle of whatever its requests were doing.
//
// So when SIGTERM arrives, the servers stop accepting new connections, the requests in
// progress get to finish, the functions passed to cpeak::on_shutdown() run, and then the
// program exits. In Node.js you'd write this yourself, with process.on("SIGTERM", ...).

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::internal::server::CpeakServer;

// A function that was passed to cpeak::on_shutdown(), ready to be called
type CleanUp = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()>>> + Send>;

static CLEAN_UPS: Mutex<Vec<CleanUp>> = Mutex::new(Vec::new());
// The servers that were started with server.listen()
static SERVERS: Mutex<Vec<CpeakServer>> = Mutex::new(Vec::new());
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

// Run a function when the program is asked to stop (SIGTERM), to close what your app has
// open, like the connections to a database:
//
//     cpeak::on_shutdown(async || {
//         pool.close().await;
//     });
//
// By the time it runs, the servers have stopped accepting connections and the requests in
// progress are done. You can add as many as you want. They run in the order you added them,
// and the program exits once the last one is done.
pub fn on_shutdown<F>(clean_up: F)
where
    F: AsyncFnOnce() + Send + 'static,
{
    lock(&CLEAN_UPS).push(Box::new(move || -> Pin<Box<dyn Future<Output = ()>>> {
        Box::pin(clean_up())
    }));
    listen_for_sigterm();
}

// Every server that starts listening gets closed when the program is asked to stop
pub(crate) fn add_server(server: CpeakServer) {
    lock(&SERVERS).push(server);
    listen_for_sigterm();
}

// Whether the program is on its way out
pub(crate) fn is_shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::SeqCst)
}

// SIGTERM is how a Unix system asks a program to stop. Elsewhere there is nothing to listen for.
#[cfg(unix)]
fn listen_for_sigterm() {
    use std::sync::Once;

    use tokio::signal::unix::{signal, SignalKind};

    use crate::internal::event_loop;

    static LISTENING: Once = Once::new();

    LISTENING.call_once(|| {
        // From here on, SIGTERM doesn't stop the program by itself anymore: we do
        let sigterm = {
            let _event_loop = event_loop::enter();
            signal(SignalKind::terminate())
        };
        let mut sigterm = match sigterm {
            Ok(sigterm) => sigterm,
            Err(error) => {
                eprintln!("[cpeak] Failed to listen for SIGTERM: {error}");
                return;
            }
        };

        event_loop::spawn(async move {
            sigterm.recv().await;

            // On a thread of its own, so the clean-up functions can hold on to anything
            // while they wait, like the routes can
            std::thread::Builder::new()
                .name("cpeak-shutdown".to_string())
                .spawn(|| event_loop::run(shut_down()))
                .unwrap_or_else(|error| panic!("[cpeak] Failed to shut down: {error}"));
        });
    });
}

#[cfg(not(unix))]
fn listen_for_sigterm() {}

#[cfg(unix)]
async fn shut_down() {
    let _exit_on_panic = ExitOnPanic;
    SHUTTING_DOWN.store(true, Ordering::SeqCst);

    // Stop accepting new connections, and let the requests in progress finish
    let servers = std::mem::take(&mut *lock(&SERVERS));
    for server in &servers {
        server.close();
    }
    for server in &servers {
        server.stopped().await;
    }

    let clean_ups = std::mem::take(&mut *lock(&CLEAN_UPS));
    for clean_up in clean_ups {
        clean_up().await;
    }

    std::process::exit(0);
}

// A clean-up function that panics must not leave the program running, half stopped
#[cfg(unix)]
struct ExitOnPanic;

#[cfg(unix)]
impl Drop for ExitOnPanic {
    fn drop(&mut self) {
        if std::thread::panicking() {
            std::process::exit(1);
        }
    }
}

// (a thread that panicked while it held the lock is no reason for this one to panic too)
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
