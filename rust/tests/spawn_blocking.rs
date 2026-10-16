// Running blocking code on a pool of threads, with cpeak::spawn_blocking()
mod common;

use std::time::{Duration, Instant};

use common::{request, serve};
use cpeak::{cpeak, json};

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        // Blocking code, which would stop the worker thread if the route ran it itself
        server.route("get", "/slow-sum", async |_req, res| {
            let sum = cpeak::spawn_blocking(|| {
                std::thread::sleep(Duration::from_millis(300));
                1 + 2
            })
            .await?;
            res.json(json!({ "sum": sum }))
        });

        // Runs on a thread of the pool, not on the worker thread of the route
        server.route("get", "/thread", async |_req, res| {
            let route_thread = std::thread::current().id();
            let pool_thread = cpeak::spawn_blocking(|| std::thread::current().id()).await?;
            res.json(json!({ "otherThread": pool_thread != route_thread }))
        });

        server
    })
}

#[test]
fn should_give_back_what_the_function_returns() {
    let res = request(server(), "GET", "/slow-sum").send();

    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "sum": 3 }));
}

#[test]
fn should_run_on_another_thread_than_the_route() {
    let res = request(server(), "GET", "/thread").send();
    assert_eq!(res.json(), json!({ "otherThread": true }));
}

// Many at once each get a thread of their own, instead of waiting for each other
#[test]
fn should_run_many_at_the_same_time() {
    let started = Instant::now();
    let jobs: Vec<_> = (0..8)
        .map(|_| cpeak::spawn_blocking(|| std::thread::sleep(Duration::from_millis(200))))
        .collect();

    cpeak::run(async {
        for job in jobs {
            job.await.unwrap();
        }
    });

    assert!(started.elapsed() < Duration::from_millis(800));
}
