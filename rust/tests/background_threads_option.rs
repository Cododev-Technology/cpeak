// How many threads cpeak::spawn() runs things on, when the app sets background_threads. The
// background event loop is started once per program, so this has a test binary of its own.

use std::collections::HashSet;
use std::time::Duration;

use cpeak::{Cpeak, CpeakOptions};

#[test]
fn should_run_on_as_many_threads_as_the_app_asked_for() {
    let _server = Cpeak::new(CpeakOptions {
        background_threads: Some(2),
        ..Default::default()
    });

    // Jobs that hold on to their thread, so each thread can only take one at a time
    let jobs: Vec<_> = (0..12)
        .map(|_| {
            cpeak::spawn(async {
                std::thread::sleep(Duration::from_millis(50));
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

    assert_eq!(threads.len(), 2);
}
