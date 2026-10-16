// The log: cpeak::log_requests(), and cpeak::log() in the routes
mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{request, start};
use cpeak::{
    json, Cpeak, CpeakError, CpeakOptions, LogOptions, LogOutput, LogTimeZone, RequestLogOptions,
};

// A folder of its own for each test
fn folder(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("cpeak-log-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    folder
}

fn file(folder: &Path, name: &str) -> LogOutput {
    LogOutput::File(folder.join(name).to_string_lossy().into_owned())
}

// What's in the file, once it has at least `lines` lines (or after 5 seconds)
fn lines(path: PathBuf, lines: usize) -> Vec<String> {
    let started = Instant::now();
    loop {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let found: Vec<String> = content.lines().map(String::from).collect();
        if found.len() >= lines || started.elapsed() > Duration::from_secs(5) {
            return found;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// An app with the `log` option, and cpeak::log_requests() in front of its routes (when
// `requests` is Some)
fn app(options: LogOptions, requests: Option<RequestLogOptions>) -> Cpeak {
    let mut server = Cpeak::new(CpeakOptions {
        log: Some(options),
        ..Default::default()
    });

    if let Some(requests) = requests {
        server.before_each(cpeak::log_requests(requests));
    }

    server.route("get", "/hello", async |_req, res| res.end("hello"));
    server.route("get", "/json", async |_req, res| {
        res.json(json!({ "message": "hi" }))
    });
    server.route("get", "/missing", async |_req, res| res.status(404).end(""));
    server.route("get", "/moved", async |_req, res| res.redirect("/hello"));
    server.route("get", "/slow", async |_req, res| {
        cpeak::sleep(Duration::from_millis(100)).await;
        res.end("")
    });
    server.route("get", "/text", async |_req, res| {
        cpeak::log("hello from a route");
        cpeak::log(format!("with {} words", 3));
        res.end("")
    });
    server.route("get", "/error", async |_req, res| {
        cpeak::log(&CpeakError::other("something broke"));
        res.end("")
    });
    server.route("get", "/stream", async |_req, res| {
        res.write("one ").await?;
        res.write("two ").await?;
        res.end("three")
    });
    server
}

// Only the format of the request lines changes
fn with_format(format: &str) -> Option<RequestLogOptions> {
    Some(RequestLogOptions {
        format: format.to_string(),
        ..Default::default()
    })
}

#[test]
fn writes_a_line_for_every_request() {
    let folder = folder("default");
    let port = start(|| {
        app(
            LogOptions {
                output: file(&folder, "info.log"),
                ..Default::default()
            },
            Some(RequestLogOptions::default()),
        )
    });

    request(port, "GET", "/hello?name=joe").send();
    let found = lines(folder.join("info.log"), 1);
    assert_eq!(found.len(), 1);

    // 2026-10-15 14:00:00 -- 127.0.0.1 -- GET /hello?name=joe 200 OK -- 0.21 ms
    let line = &found[0];
    let (date, rest) = line.split_at(19);
    assert_eq!(date.len(), 19);
    assert_eq!(&date[4..5], "-");
    assert_eq!(&date[10..11], " ");
    assert_eq!(&date[13..14], ":");
    assert!(
        rest.starts_with(" -- 127.0.0.1 -- GET /hello?name=joe 200 OK -- "),
        "{line}"
    );
    assert!(rest.ends_with(" ms"), "{line}");
    let ms = rest
        .trim_start_matches(" -- 127.0.0.1 -- GET /hello?name=joe 200 OK -- ")
        .trim_end_matches(" ms");
    assert!(ms.parse::<f64>().is_ok(), "{line}");
    assert_eq!(ms.split('.').nth(1).map(str::len), Some(2), "{line}");
}

#[test]
fn writes_the_format_it_is_given() {
    let folder = folder("format");
    let port = start(|| {
        app(
            LogOptions {
                output: file(&folder, "info.log"),
                ..Default::default()
            },
            with_format("{ip} {method} {url} {path} {status} {status_message} {bytes} {header:User-Agent} {header:x-missing}"),
        )
    });

    request(port, "GET", "/hello?a=1")
        .header("User-Agent", "tester/1.0")
        .send();
    assert_eq!(
        lines(folder.join("info.log"), 1),
        ["127.0.0.1 GET /hello?a=1 /hello 200 OK 5 tester/1.0 -"]
    );

    request(port, "GET", "/moved").send();
    request(port, "GET", "/nowhere").send();
    request(port, "GET", "/stream").send();
    let found = lines(folder.join("info.log"), 4);
    assert!(found.contains(&"127.0.0.1 GET /moved /moved 302 Found 0 - -".to_string()));
    // Cpeak's own 404 is {"error":"Cannot GET /nowhere"}
    assert!(found.contains(&"127.0.0.1 GET /nowhere /nowhere 404 Not Found 31 - -".to_string()));
    // Every chunk of a streamed body counts
    assert!(found.contains(&"127.0.0.1 GET /stream /stream 200 OK 13 - -".to_string()));
}

#[test]
fn reads_the_ip_from_a_header() {
    let folder = folder("ip");
    let port = start(|| {
        app(
            LogOptions {
                output: file(&folder, "info.log"),
                ..Default::default()
            },
            Some(RequestLogOptions {
                format: "{ip}".to_string(),
                ip_header: Some("X-Forwarded-For".to_string()),
                ..Default::default()
            }),
        )
    });

    request(port, "GET", "/hello")
        .header("X-Forwarded-For", "203.0.113.7")
        .send();
    assert_eq!(lines(folder.join("info.log"), 1), ["203.0.113.7"]);

    // Without the header, it's the address of the connection
    request(port, "GET", "/hello").send();
    assert_eq!(
        lines(folder.join("info.log"), 2),
        ["203.0.113.7", "127.0.0.1"]
    );
}

#[test]
fn only_errors() {
    let folder = folder("only-errors");
    let port = start(|| {
        app(
            LogOptions {
                output: file(&folder, "info.log"),
                ..Default::default()
            },
            Some(RequestLogOptions {
                format: "{url} {status}".to_string(),
                only_errors: true,
                ..Default::default()
            }),
        )
    });

    request(port, "GET", "/hello").send();
    request(port, "GET", "/missing").send();
    request(port, "GET", "/moved").send();
    assert_eq!(lines(folder.join("info.log"), 1), ["/missing 404"]);
}

#[test]
fn slower_than() {
    let folder = folder("slower-than");
    let port = start(|| {
        app(
            LogOptions {
                output: file(&folder, "info.log"),
                ..Default::default()
            },
            Some(RequestLogOptions {
                format: "{url}".to_string(),
                slower_than: Some(Duration::from_millis(50)),
                ..Default::default()
            }),
        )
    });

    request(port, "GET", "/hello").send();
    request(port, "GET", "/slow").send();
    request(port, "GET", "/hello").send();
    assert_eq!(lines(folder.join("info.log"), 1), ["/slow"]);
}

#[test]
fn skip() {
    let folder = folder("skip");
    let port = start(|| {
        app(
            LogOptions {
                output: file(&folder, "info.log"),
                ..Default::default()
            },
            Some(RequestLogOptions {
                format: "{url}".to_string(),
                skip: Some(|req, _res| req.url == "/hello"),
                ..Default::default()
            }),
        )
    });

    request(port, "GET", "/hello").send();
    request(port, "GET", "/json").send();
    assert_eq!(lines(folder.join("info.log"), 1), ["/json"]);
}

#[test]
fn sample() {
    let folder = folder("sample");
    let port = start(|| {
        app(
            LogOptions {
                output: file(&folder, "info.log"),
                ..Default::default()
            },
            Some(RequestLogOptions {
                format: "{url}".to_string(),
                sample: Some(1_000_000),
                ..Default::default()
            }),
        )
    });

    // Each worker counts its own requests, and writes the first of every 1,000,000
    let workers = std::thread::available_parallelism().unwrap().get();
    for _ in 0..workers * 3 {
        request(port, "GET", "/hello").send();
    }
    // (more than flush_every, so every worker has handed its lines over)
    std::thread::sleep(Duration::from_millis(500));
    let found = lines(folder.join("info.log"), 1);
    assert!(!found.is_empty());
    assert!(found.len() <= workers, "{} lines", found.len());
}

#[test]
fn only_the_requests_that_reach_it() {
    let folder = folder("position");
    let port = start(|| {
        let mut server = Cpeak::new(CpeakOptions {
            log: Some(LogOptions {
                output: file(&folder, "info.log"),
                ..Default::default()
            }),
            ..Default::default()
        });
        // Like serve_static(): it sends the response itself, so the request stops here
        server.before_each(async |req, res, next| {
            if req.url == "/early" {
                return res.end("sent before the log");
            }
            next()
        });
        server.before_each(cpeak::log_requests(RequestLogOptions {
            format: "{url}".to_string(),
            ..Default::default()
        }));
        server.route("get", "/early", async |_req, res| res.end(""));
        server.route("get", "/hello", async |_req, res| res.end("hello"));
        server
    });

    request(port, "GET", "/early").send();
    request(port, "GET", "/hello").send();
    // (more than flush_every, so a line for /early would be there by now)
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(lines(folder.join("info.log"), 1), ["/hello"]);
}

#[test]
fn on_a_route_of_its_own() {
    let folder = folder("route");
    let port = start(|| {
        let mut server = Cpeak::new(CpeakOptions {
            log: Some(LogOptions {
                output: file(&folder, "info.log"),
                ..Default::default()
            }),
            ..Default::default()
        });
        let log_requests = cpeak::log_requests(RequestLogOptions {
            format: "{url}".to_string(),
            ..Default::default()
        });
        server.route_with("get", "/logged", log_requests, async |_req, res| {
            res.end("")
        });
        server.route("get", "/hello", async |_req, res| res.end("hello"));
        server
    });

    request(port, "GET", "/hello").send();
    request(port, "GET", "/logged").send();
    request(port, "GET", "/hello").send();
    // (more than flush_every, so the lines for /hello would be there by now)
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(lines(folder.join("info.log"), 1), ["/logged"]);
}

#[test]
fn one_line_for_a_request_that_goes_through_it_twice() {
    let folder = folder("twice");
    let port = start(|| {
        let mut server = Cpeak::new(CpeakOptions {
            log: Some(LogOptions {
                output: file(&folder, "info.log"),
                ..Default::default()
            }),
            ..Default::default()
        });
        server.before_each(cpeak::log_requests(RequestLogOptions {
            format: "all {url}".to_string(),
            ..Default::default()
        }));
        let on_the_route = cpeak::log_requests(RequestLogOptions {
            format: "route {url}".to_string(),
            ..Default::default()
        });
        server.route_with("get", "/hello", on_the_route, async |_req, res| {
            res.end("hello")
        });
        server
    });

    // The last one it reaches has the say
    request(port, "GET", "/hello").send();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(lines(folder.join("info.log"), 1), ["route /hello"]);
}

#[test]
fn cpeak_log_in_a_route() {
    let folder = folder("in-a-route");
    let port = start(|| {
        app(
            LogOptions {
                output: file(&folder, "info.log"),
                errors: file(&folder, "errors.log"),
                ..Default::default()
            },
            None,
        )
    });

    request(port, "GET", "/text").send();
    let found = lines(folder.join("info.log"), 2);
    assert_eq!(found.len(), 2);
    assert_eq!(&found[0][19..], " -- hello from a route");
    assert_eq!(&found[1][19..], " -- with 3 words");

    request(port, "GET", "/error").send();
    let found = lines(folder.join("errors.log"), 7);
    let date = &found[1];
    assert_eq!(date.len(), 19);
    assert_eq!(
        found,
        [
            "---------------------------------------",
            date,
            "Error Name: Error",
            "Error Message: something broke",
            "Error Stack:",
            "Error: something broke",
            "    CpeakError { status: None, message: \"something broke\", code: None, cpeak_err: false, client_disconnect: false }",
            "---------------------------------------",
        ]
    );
}

#[test]
fn a_file_for_every_day() {
    let folder = folder("days");
    let port = start(|| {
        app(
            LogOptions {
                output: LogOutput::File(
                    folder
                        .join("{day}")
                        .join("info.log")
                        .to_string_lossy()
                        .into_owned(),
                ),
                ..Default::default()
            },
            Some(RequestLogOptions::default()),
        )
    });

    request(port, "GET", "/hello").send();
    let started = Instant::now();
    let day = loop {
        let days: Vec<String> = std::fs::read_dir(&folder)
            .map(|days| {
                days.map(|day| day.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        if !days.is_empty() || started.elapsed() > Duration::from_secs(5) {
            assert_eq!(days.len(), 1, "{days:?}");
            break days[0].clone();
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    // Named after the day of the line in it (2026-10-15)
    assert_eq!(day.len(), 10);
    let found = lines(folder.join(&day).join("info.log"), 1);
    assert!(found[0].starts_with(&day), "{found:?}");
}

#[test]
fn iso_dates() {
    let folder = folder("iso");
    let port = start(|| {
        app(
            LogOptions {
                output: file(&folder, "utc.log"),
                ..Default::default()
            },
            with_format("{iso}"),
        )
    });
    request(port, "GET", "/hello").send();
    // 2026-10-15T14:00:00.123Z
    let utc = &lines(folder.join("utc.log"), 1)[0];
    assert_eq!(utc.len(), 24, "{utc}");
    assert_eq!(&utc[10..11], "T");
    assert_eq!(&utc[19..20], ".");
    assert!(utc.ends_with('Z'), "{utc}");

    let port = start(|| {
        app(
            LogOptions {
                output: file(&folder, "local.log"),
                time_zone: LogTimeZone::Local,
                ..Default::default()
            },
            with_format("{iso}"),
        )
    });
    request(port, "GET", "/hello").send();
    // 2026-10-15T10:00:00.123-04:00
    let local = &lines(folder.join("local.log"), 1)[0];
    assert_eq!(local.len(), 29, "{local}");
    assert!(matches!(&local[23..24], "+" | "-"), "{local}");
    assert_eq!(&local[26..27], ":");
}

#[test]
fn waits_in_the_buffer_until_flush_every() {
    let folder = folder("flush");
    let port = start(|| {
        app(
            LogOptions {
                output: file(&folder, "info.log"),
                flush_every: Duration::from_millis(300),
                ..Default::default()
            },
            Some(RequestLogOptions::default()),
        )
    });

    request(port, "GET", "/hello").send();
    std::thread::sleep(Duration::from_millis(100));
    assert!(std::fs::read_to_string(folder.join("info.log"))
        .unwrap_or_default()
        .is_empty());
    assert_eq!(lines(folder.join("info.log"), 1).len(), 1);
}

#[test]
fn a_full_buffer_goes_right_away() {
    let folder = folder("full-buffer");
    let port = start(|| {
        app(
            LogOptions {
                output: file(&folder, "info.log"),
                buffer_size: 1,
                flush_every: Duration::from_secs(60),
                ..Default::default()
            },
            Some(RequestLogOptions::default()),
        )
    });

    request(port, "GET", "/hello").send();
    assert_eq!(lines(folder.join("info.log"), 1).len(), 1);
}

#[test]
fn everything_is_written_when_the_server_stops() {
    let folder = folder("stop");
    let server = app(
        LogOptions {
            output: file(&folder, "info.log"),
            flush_every: Duration::from_secs(60),
            ..Default::default()
        },
        Some(RequestLogOptions::default()),
    )
    .listen(("127.0.0.1", 0), || {});

    for _ in 0..5 {
        request(server.address().port(), "GET", "/hello").send();
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(std::fs::read_to_string(folder.join("info.log"))
        .unwrap_or_default()
        .is_empty());

    server.close();
    server.wait();
    let content = std::fs::read_to_string(folder.join("info.log")).unwrap();
    assert_eq!(content.lines().count(), 5);
}

#[test]
#[should_panic(expected = "Unknown name {nope} in the request log format")]
fn a_mistyped_name_panics_at_startup() {
    let _ = cpeak::log_requests(RequestLogOptions {
        format: "{ip} {nope}".to_string(),
        ..Default::default()
    });
}
