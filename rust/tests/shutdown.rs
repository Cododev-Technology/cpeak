// Shutting down when the program is asked to stop (SIGTERM), and cpeak::on_shutdown()
#![cfg(unix)]
mod common;

use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

use common::request;
use cpeak::{cpeak, json};

// SIGTERM stops the whole program, so every test here needs a program of its own to stop.
// It gets one by running this test file once more as another process, which only runs the
// `program` test below, and CPEAK_TEST_PROGRAM tells that one which program to be.
const PROGRAM: &str = "CPEAK_TEST_PROGRAM";

#[test]
fn program() {
    let Ok(program) = std::env::var(PROGRAM) else {
        // A regular run of the tests: nothing to do here
        return;
    };

    // Whatever happens, the program doesn't outlive its test by much
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(30));
        std::process::exit(3);
    });

    match program.as_str() {
        "server" => server(),
        "plain server" => plain_server(),
        "worker" => worker(),
        "plain worker" => plain_worker(),
        "panicking worker" => panicking_worker(),
        _ => panic!("no such program: {program}"),
    }
}

// What a program tells its test (the test runner prints lines of its own in between)
fn say(text: impl std::fmt::Display) {
    println!("> {text}");
}

// A server with a slow route, and two things to clean up
fn server() {
    let mut server = cpeak();

    server.route("get", "/slow", async |_req, res| {
        cpeak::sleep(Duration::from_millis(400)).await;
        say("request done");
        res.json(json!({ "done": true }))
    });

    cpeak::on_shutdown(async || {
        // It can wait for things, like a database pool that closes its connections
        cpeak::sleep(Duration::from_millis(50)).await;
        say("first clean-up");
    });
    cpeak::on_shutdown(async || say("second clean-up"));

    let server = server.listen(("127.0.0.1", 0), || {});
    say(format!("ready {}", server.address().port()));

    server.wait();
    say("wait returned");
}

// A server with nothing to clean up
fn plain_server() {
    let server = cpeak().listen(("127.0.0.1", 0), || {});
    say(format!("ready {}", server.address().port()));

    server.wait();
    say("wait returned");
}

// A program without a server, like a worker that takes jobs off a queue
fn worker() {
    cpeak::on_shutdown(async || say("cleaned up"));
    say("ready");

    cpeak::run(async {
        loop {
            cpeak::sleep(Duration::from_millis(10)).await;
        }
    });
}

fn plain_worker() {
    say("ready");
    cpeak::run(std::future::pending::<()>());
}

fn panicking_worker() {
    cpeak::on_shutdown(async || panic!("Something went wrong while cleaning up"));
    cpeak::on_shutdown(async || say("cleaned up"));
    say("ready");

    cpeak::run(std::future::pending::<()>());
}

// A program that is running for a test
struct Program {
    process: Child,
    output: BufReader<ChildStdout>,
}

// Starts a program, and waits until it says that it's ready (with its port, if it has one)
fn start(program: &str) -> (Program, String) {
    let mut process = Command::new(std::env::current_exe().unwrap())
        .args(["program", "--exact", "--nocapture"])
        .env(PROGRAM, program)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut program = Program {
        output: BufReader::new(process.stdout.take().unwrap()),
        process,
    };

    loop {
        let said = program
            .next_said()
            .expect("the program ended before it was ready");
        if let Some(ready) = said.strip_prefix("ready") {
            return (program, ready.trim().to_string());
        }
    }
}

impl Program {
    // The next thing the program says, or None once it has ended
    fn next_said(&mut self) -> Option<String> {
        loop {
            let mut line = String::new();
            if self.output.read_line(&mut line).unwrap() == 0 {
                return None;
            }
            if let Some((_, said)) = line.split_once("> ") {
                return Some(said.trim_end().to_string());
            }
        }
    }

    fn sigterm(&self) {
        let killed = Command::new("kill")
            .args(["-TERM", &self.process.id().to_string()])
            .status()
            .unwrap();
        assert!(killed.success());
    }

    // Waits for the program to end. Gives back how it ended, and what it said before it did.
    fn end(mut self) -> (std::process::ExitStatus, Vec<String>) {
        let mut said = Vec::new();
        while let Some(line) = self.next_said() {
            said.push(line);
        }
        let mut rest = String::new();
        let _ = self.output.read_to_string(&mut rest);

        (self.process.wait().unwrap(), said)
    }
}

#[test]
fn should_let_the_requests_in_progress_finish_and_then_run_the_clean_up_functions() {
    let (program, port) = start("server");
    let port: u16 = port.parse().unwrap();

    let slow = std::thread::spawn(move || request(port, "GET", "/slow").send());
    std::thread::sleep(Duration::from_millis(100));
    program.sigterm();

    // While that request is still in progress, the server doesn't accept new connections
    std::thread::sleep(Duration::from_millis(100));
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());

    let res = slow.join().unwrap();
    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "done": true }));

    // server.wait() doesn't return: the program exits once the clean-up functions are done
    let (status, said) = program.end();
    assert_eq!(status.code(), Some(0));
    assert_eq!(said, ["request done", "first clean-up", "second clean-up"]);
}

#[test]
fn should_stop_a_server_that_has_nothing_to_clean_up() {
    let (program, port) = start("plain server");
    let port: u16 = port.parse().unwrap();
    assert_eq!(request(port, "GET", "/").send().status, 404);

    program.sigterm();

    let (status, said) = program.end();
    assert_eq!(status.code(), Some(0));
    assert!(said.is_empty(), "{said:?}");
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
}

#[test]
fn should_run_the_clean_up_functions_of_a_program_without_a_server() {
    let (program, _) = start("worker");

    program.sigterm();

    let (status, said) = program.end();
    assert_eq!(status.code(), Some(0));
    assert_eq!(said, ["cleaned up"]);
}

// Without a server and without clean-up functions, SIGTERM stops the program like it
// stops any other
#[test]
fn should_leave_a_program_alone_that_has_nothing_to_shut_down() {
    let (program, _) = start("plain worker");

    program.sigterm();

    let (status, said) = program.end();
    assert_eq!(status.code(), None);
    assert_eq!(status.signal(), Some(15));
    assert!(said.is_empty(), "{said:?}");
}

#[test]
fn should_still_stop_when_a_clean_up_function_panics() {
    let (program, _) = start("panicking worker");

    program.sigterm();

    let (status, said) = program.end();
    assert_eq!(status.code(), Some(1));
    assert!(said.is_empty(), "{said:?}");
}
