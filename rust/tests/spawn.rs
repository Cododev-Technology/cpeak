// Running something in the background, with cpeak::spawn()
mod common;

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
            cpeak::spawn(async {
                cpeak::sleep(Duration::from_millis(200)).await;
                DONE.fetch_add(1, Ordering::SeqCst);
            });
            res.json(json!({ "done": DONE.load(Ordering::SeqCst) }))
        });

        // Unless the route awaits it
        server.route("get", "/sum", async |_req, res| {
            let sum = cpeak::spawn(async {
                cpeak::sleep(Duration::from_millis(20)).await;
                1 + 2
            })
            .await?;
            res.json(json!({ "sum": sum }))
        });

        server.route("get", "/panic", async |_req, res| {
            let result = cpeak::spawn(async {
                if true {
                    panic!("Something went wrong in the background");
                }
            })
            .await;
            res.json(json!({ "failed": result.is_err() }))
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
fn should_give_back_a_handle_that_can_be_awaited() {
    let res = request(server(), "GET", "/sum").send();

    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "sum": 3 }));
}

#[test]
fn should_keep_the_server_going_when_a_background_job_panics() {
    let res = request(server(), "GET", "/panic").send();
    assert_eq!(res.json(), json!({ "failed": true }));

    let res = request(server(), "GET", "/sum").send();
    assert_eq!(res.json(), json!({ "sum": 3 }));
}

// No server needed: it's the same background event loop
#[test]
fn should_run_outside_of_a_server_too() {
    let job = cpeak::spawn(async {
        cpeak::sleep(Duration::from_millis(20)).await;
        "done"
    });

    assert_eq!(cpeak::run(job).unwrap(), "done");
}
