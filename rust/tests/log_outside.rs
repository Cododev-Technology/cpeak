// cpeak::log() outside of the routes (at startup, in a script, on a thread of your own).
// Its own test file, because it writes where the last app made with a `log` option says.
use std::time::Duration;

use cpeak::{Cpeak, CpeakError, CpeakOptions, LogOptions, LogOutput};

#[test]
fn cpeak_log_outside_of_the_routes() {
    // Before there's an app with the option, it prints (to stdout and stderr)
    cpeak::log("before any app");
    cpeak::log(&CpeakError::other("an error before any app"));

    let folder = std::env::temp_dir().join(format!("cpeak-log-{}-outside", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    let _app = Cpeak::new(CpeakOptions {
        log: Some(LogOptions {
            output: LogOutput::File(folder.join("info.log").to_string_lossy().into_owned()),
            errors: LogOutput::File(folder.join("errors.log").to_string_lossy().into_owned()),
            flush_every: Duration::from_secs(60),
            ..Default::default()
        }),
        ..Default::default()
    });

    // Written before cpeak::log() returns, without waiting for a buffer
    cpeak::log("The server has started");
    let info = std::fs::read_to_string(folder.join("info.log")).unwrap();
    assert_eq!(&info[19..], " -- The server has started\n");

    cpeak::log(&CpeakError::from(std::io::Error::other("disk on fire")));
    let errors = std::fs::read_to_string(folder.join("errors.log")).unwrap();
    assert!(
        errors.contains("\nError Name: Error\nError Message: disk on fire\nError Stack:\nError: disk on fire\n    CpeakError {"),
        "{errors}"
    );
    assert!(errors.ends_with("\n---------------------------------------\n"));
}
