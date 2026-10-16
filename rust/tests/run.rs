// Running async code without a server, with cpeak::run()

use std::cell::Cell;
use std::io::Write;
use std::net::TcpStream;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

#[test]
fn should_run_async_code_to_the_end_and_give_back_its_result() {
    let started = Instant::now();

    let result = cpeak::run(async {
        cpeak::sleep(Duration::from_millis(30)).await;
        "the result"
    });

    assert_eq!(result, "the result");
    assert!(started.elapsed() >= Duration::from_millis(30));
}

// Like the code of a route, it can hold on to anything while it waits
#[test]
fn should_run_code_that_is_not_send() {
    let counter = Rc::new(Cell::new(0));

    cpeak::run(async {
        let counter = counter.clone();
        cpeak::sleep(Duration::from_millis(1)).await;
        counter.set(counter.get() + 1);
    });

    assert_eq!(counter.get(), 1);
}

// The event loop is still there after a run, with everything that was started on it. That's
// what lets a test suite call run() once per test and share a database pool between them.
#[test]
fn should_keep_what_was_started_for_the_next_run() {
    let listener = cpeak::run(async { TcpListener::bind("127.0.0.1:0").await.unwrap() });
    let port = listener.local_addr().unwrap().port();

    let job = cpeak::spawn(async {
        cpeak::sleep(Duration::from_millis(50)).await;
        "still running"
    });

    let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
    client.write_all(b"hello").unwrap();
    drop(client);

    let received = cpeak::run(async {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut received = String::new();
        socket.read_to_string(&mut received).await.unwrap();
        received
    });

    assert_eq!(received, "hello");
    assert_eq!(cpeak::run(job).unwrap(), "still running");
}

#[test]
fn should_run_on_many_threads_at_once() {
    let threads: Vec<_> = (0..8u64)
        .map(|i| {
            std::thread::spawn(move || {
                cpeak::run(async move {
                    cpeak::sleep(Duration::from_millis(10 + i)).await;
                    i * 2
                })
            })
        })
        .collect();

    let results: Vec<u64> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(results, [0, 2, 4, 6, 8, 10, 12, 14]);
}

// Whatever uses rustls in the program has a crypto library to work with (see tests/crypto.rs)
#[test]
fn should_pick_the_crypto_library_of_the_program() {
    cpeak::run(async {});

    assert!(rustls::crypto::CryptoProvider::get_default().is_some());
}
