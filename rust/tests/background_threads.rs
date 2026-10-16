// How many threads cpeak::spawn() runs things on, when the app doesn't say. The background
// event loop is started once per program, so this has a test binary of its own.

use std::collections::HashSet;
use std::time::Duration;

#[test]
fn should_run_on_four_threads_by_default() {
    let _server = cpeak::cpeak();

    // Jobs that hold on to their thread, so each thread can only take one at a time
    let jobs: Vec<_> = (0..12)
        .map(|_| {
            cpeak::spawn(async {
                std::thread::sleep(Duration::from_millis(100));
                std::thread::current().id()
            })
        })
        .collect();

    let threads: HashSet<_> = cpeak::run(async {
        let mut threads = HashSet::new();
        for job in jobs {
            threads.insert(job.await.unwrap());
        }
        threads
    });

    assert_eq!(threads.len(), 4);
}
