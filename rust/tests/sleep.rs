// Waiting, with cpeak::sleep()
mod common;

use std::time::{Duration, Instant};

use common::{request, serve};
use cpeak::{cpeak, json};

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.route("get", "/wait/:ms", async |req, res| {
            let ms: u64 = req.params["ms"].parse().unwrap();
            let started = Instant::now();
            cpeak::sleep(Duration::from_millis(ms)).await;
            res.json(json!({ "waited": started.elapsed().as_micros() as u64 }))
        });

        // The short wait is over first, and the long one is dropped before its time has come
        server.route("get", "/race", async |_req, res| {
            tokio::select! {
                _ = cpeak::sleep(Duration::from_millis(20)) => res.json(json!({ "first": "short" })),
                _ = cpeak::sleep(Duration::from_secs(30)) => res.json(json!({ "first": "long" })),
            }
        });

        server
    })
}

// How long the handler says it waited, in microseconds
fn wait(ms: u64) -> u64 {
    let res = request(server(), "GET", &format!("/wait/{ms}")).send();
    assert_eq!(res.status, 200);
    res.json()["waited"].as_u64().unwrap()
}

#[test]
fn should_wait_for_the_given_time() {
    let waited = wait(50);
    assert!(waited >= 50_000, "waited only {waited} microseconds");
    assert!(waited < 250_000, "waited {waited} microseconds");
}

#[test]
fn should_not_wait_at_all_for_a_time_of_zero() {
    assert!(wait(0) < 50_000);
}

#[test]
fn should_let_many_requests_wait_at_once_each_for_its_own_time() {
    let started = Instant::now();
    let requests: Vec<_> = (0..32u64)
        .map(|i| {
            let ms = 100 + i * 10;
            std::thread::spawn(move || (ms, wait(ms)))
        })
        .collect();

    let mut all_waits = 0;
    for request in requests {
        let (ms, waited) = request.join().unwrap();
        assert!(
            waited >= ms * 1000,
            "asked for {ms}ms, waited {waited} microseconds"
        );
        assert!(
            waited < ms * 1000 + 250_000,
            "asked for {ms}ms, waited {waited} microseconds"
        );
        all_waits += ms;
    }

    // They waited at the same time, not one after the other (that would take 8 seconds)
    assert!(all_waits > 8000);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn should_keep_waiting_right_after_a_wait_was_dropped() {
    let res = request(server(), "GET", "/race").send();
    assert_eq!(res.json(), json!({ "first": "short" }));

    for _ in 0..5 {
        let waited = wait(10);
        assert!(waited >= 10_000, "waited only {waited} microseconds");
        assert!(waited < 250_000, "waited {waited} microseconds");
    }
}

// Outside of a Cpeak server, any tokio runtime with timers will do
#[tokio::test]
async fn should_wait_outside_of_a_server_too() {
    let started = Instant::now();
    cpeak::sleep(Duration::from_millis(30)).await;
    assert!(started.elapsed() >= Duration::from_millis(30));
}
