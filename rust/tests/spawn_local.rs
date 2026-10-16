// Running something in the background on the request's own worker thread, with cpeak::spawn_local()
mod common;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use common::{request, serve};
use cpeak::{cpeak, json};

// How many of the background jobs of /later are done
static DONE: AtomicU32 = AtomicU32::new(0);

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        // The response doesn't wait for what was spawned
        server.route("get", "/later", async |_req, res| {
            cpeak::spawn_local(async {
                cpeak::sleep(Duration::from_millis(200)).await;
                DONE.fetch_add(1, Ordering::SeqCst);
            });
            res.json(json!({ "done": DONE.load(Ordering::SeqCst) }))
        });

        // It runs on the thread of the route, and can hold on to what isn't Send
        server.route("get", "/same-thread", async |_req, res| {
            let route_thread = std::thread::current().id();
            let counter = Rc::new(Cell::new(0));

            let job_thread = cpeak::spawn_local({
                let counter = counter.clone();
                async move {
                    cpeak::sleep(Duration::from_millis(20)).await;
                    counter.set(counter.get() + 1);
                    std::thread::current().id()
                }
            })
            .await?;

            res.json(json!({
                "sameThread": job_thread == route_thread,
                "counter": counter.get(),
            }))
        });

        server
    })
}

#[test]
fn should_keep_running_after_the_response_was_sent() {
    let started = Instant::now();
    let res = request(server(), "GET", "/later").send();

    assert_eq!(res.json(), json!({ "done": 0 }));
    assert!(started.elapsed() < Duration::from_millis(200));

    // The job that the route started is done a bit later
    for _ in 0..100 {
        if DONE.load(Ordering::SeqCst) == 1 {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the background job never finished");
}

#[test]
fn should_run_on_the_thread_of_the_route() {
    let res = request(server(), "GET", "/same-thread").send();

    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "sameThread": true, "counter": 1 }));
}

// There's no worker thread to run on outside of a route
#[test]
fn should_panic_outside_of_a_route() {
    let spawned = std::panic::catch_unwind(|| {
        cpeak::spawn_local(async {});
    });
    assert!(spawned.is_err());

    let in_spawn = cpeak::run(cpeak::spawn(async {
        cpeak::spawn_local(async {});
    }));
    assert!(in_spawn.is_err());
}
