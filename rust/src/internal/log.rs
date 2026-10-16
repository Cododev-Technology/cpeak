// The log: cpeak::log(), and cpeak::log_requests(), the middleware that writes a line for
// every request that reaches it.
//
// A busy server can handle hundreds of thousands of requests a second, so a line can't cost
// much. Each worker thread writes its lines into a buffer of its own (nothing shared, no
// locks), and hands the full buffer over to one thread that writes it out. That thread does
// all the waiting on the disk, so the requests never do.
//
// If the disk can't keep up, the buffers that don't fit in line are dropped instead of piling
// up in memory, and the log says how many lines were lost once it catches up.

use std::borrow::Cow;
use std::cell::RefCell;
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::internal::errors::CpeakError;
use crate::internal::types::{LogOptions, LogOutput, LogTimeZone, RequestLogOptions};
use crate::types::Next;
use crate::{status_message, CpeakRequest, CpeakResponse, Middleware};

// How many full buffers can wait for the writing thread before we start dropping them
const WAITING_BUFFERS: usize = 64;

// How often we check that the log file is still there (someone may delete or move it while
// the server runs, and on Unix, writing to a deleted file doesn't fail)
const FILE_CHECK_INTERVAL: Duration = Duration::from_secs(5);

// What can be logged: some text, or an error
pub enum LogMessage<'a> {
    Text(Cow<'a, str>),
    Error(&'a CpeakError),
}

impl<'a> From<&'a str> for LogMessage<'a> {
    fn from(text: &'a str) -> Self {
        LogMessage::Text(Cow::Borrowed(text))
    }
}

impl<'a> From<&'a String> for LogMessage<'a> {
    fn from(text: &'a String) -> Self {
        LogMessage::Text(Cow::Borrowed(text))
    }
}

impl From<String> for LogMessage<'_> {
    fn from(text: String) -> Self {
        LogMessage::Text(Cow::Owned(text))
    }
}

impl<'a> From<&'a CpeakError> for LogMessage<'a> {
    fn from(error: &'a CpeakError) -> Self {
        LogMessage::Error(error)
    }
}

// Write a line to the log, with the date in front of it:
//
//     cpeak::log("The server has started");
//
// Or an error, which goes to the log of errors:
//
//     cpeak::log(&error);
//
// It goes where the `log` option of your app says. In a route or a middleware function, the
// line waits in the worker's buffer like the request lines do. Anywhere else (or in an app
// without the option or cpeak::log_requests()), it's written before cpeak::log() returns.
// With no app that has the option, the text goes to stdout and the errors to stderr.
pub fn log<'a>(message: impl Into<LogMessage<'a>>) {
    let message = message.into();

    let on_a_worker = THREAD_LOG.with(|thread_log| match thread_log.try_borrow_mut() {
        Ok(mut thread_log) => match thread_log.as_mut() {
            Some(thread_log) => {
                thread_log.message(&message);
                true
            }
            None => false,
        },
        Err(_) => false,
    });

    if !on_a_worker {
        let logger = lock(&CURRENT).clone().unwrap_or_else(|| DEFAULT.clone());
        let mut clock = Clock::new(logger.time_zone);
        clock.now();
        let mut bytes = Vec::new();
        let sink = match &message {
            LogMessage::Text(text) => {
                write_text(&mut bytes, &clock, text);
                &logger.output
            }
            LogMessage::Error(error) => {
                write_error(&mut bytes, &clock, error);
                &logger.errors
            }
        };
        sink.write(&clock.day(), &bytes);
    }
}

// The `log` option of an app, ready to use
pub(crate) struct Logger {
    output: Arc<Sink>,
    errors: Arc<Sink>,
    time_zone: LogTimeZone,
    buffer_size: usize,
    flush_every: Duration,
}

// The options of a cpeak::log_requests(), ready to use
pub(crate) struct RequestLog {
    // Which cpeak::log_requests() it is (0, 1, 2, ...), for the counts of `sample`
    id: usize,
    parts: Vec<Part>,
    ip_header: Option<String>,
    only_errors: bool,
    slower_than: Option<Duration>,
    sample: u32,
    skip: Option<fn(&CpeakRequest, &CpeakResponse) -> bool>,
}

// The pieces of a request line, from the format
enum Part {
    Text(Box<[u8]>),
    Date,
    Iso,
    Ip,
    Method,
    Url,
    Path,
    Status,
    StatusMessage,
    Ms,
    Us,
    Bytes,
    Header(String),
}

// What cpeak::log() uses outside of a worker: the `log` option of the last app made with one
static CURRENT: Mutex<Option<Arc<Logger>>> = Mutex::new(None);
// And before there is one (and what an app without the option uses for its request lines)
static DEFAULT: LazyLock<Arc<Logger>> =
    LazyLock::new(|| Arc::new(Logger::new(LogOptions::default())));

// How many cpeak::log_requests() were made. While there are none, the requests don't read the
// clock for their {ms}.
static REQUEST_LOGS: AtomicUsize = AtomicUsize::new(0);

// A middleware function that writes a line for every request that reaches it, once the
// response is sent:
//
//     server.before_each(serve_static("./public", None));
//     server.before_each(cpeak::log_requests(RequestLogOptions::default()));
//
// Where it is decides which requests get one. Above, the files that serve_static() sends never
// reach it, so they don't. It can go on a route of its own too (server.route_with()).
//
// The lines go where the `log` option of your app says (stdout without one). The {ms} of a
// line is from when the request came in, not from when it reached this middleware.
pub fn log_requests(options: RequestLogOptions) -> impl Middleware {
    // Made once, and kept for as long as the program runs, so the requests can point at it
    // without counting references to it
    let request_log: &'static RequestLog = Box::leak(Box::new(RequestLog::new(options)));

    async move |_req: &mut CpeakRequest, res: &mut CpeakResponse, next: Next| {
        res.request_log = Some(request_log);
        next()
    }
}

// Whether there is a cpeak::log_requests() (the server asks once, when a worker starts)
pub(crate) fn logs_requests() -> bool {
    REQUEST_LOGS.load(Ordering::Relaxed) > 0
}

// What an app without the `log` option writes its request lines with
pub(crate) fn default_logger() -> Arc<Logger> {
    DEFAULT.clone()
}

// Called by Cpeak::new() when the app has a `log` option
pub(crate) fn start(options: LogOptions) -> Arc<Logger> {
    let logger = Arc::new(Logger::new(options));
    *lock(&CURRENT) = Some(logger.clone());
    logger
}

impl Logger {
    fn new(options: LogOptions) -> Self {
        Self {
            output: Arc::new(Sink::new(options.output, options.time_zone)),
            errors: Arc::new(Sink::new(options.errors, options.time_zone)),
            time_zone: options.time_zone,
            buffer_size: options.buffer_size,
            flush_every: options.flush_every.max(Duration::from_millis(1)),
        }
    }
}

impl RequestLog {
    fn new(options: RequestLogOptions) -> Self {
        Self {
            id: REQUEST_LOGS.fetch_add(1, Ordering::Relaxed),
            parts: parse_format(&options.format),
            ip_header: options.ip_header.map(|header| header.to_lowercase()),
            only_errors: options.only_errors,
            slower_than: options.slower_than,
            sample: options.sample.unwrap_or(1).max(1),
            skip: options.skip,
        }
    }
}

// Turn the format into its pieces once, at startup. A mistyped name panics right here, instead
// of showing up in every line.
fn parse_format(format: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut rest = format;

    while let Some(open) = rest.find('{') {
        text.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            panic!("[cpeak] The request log format has a {{ without a }}: \"{format}\"");
        };
        let name = &rest[open + 1..open + close];
        let part = match name {
            "date" => Part::Date,
            "iso" => Part::Iso,
            "ip" => Part::Ip,
            "method" => Part::Method,
            "url" => Part::Url,
            "path" => Part::Path,
            "status" => Part::Status,
            "status_message" => Part::StatusMessage,
            "ms" => Part::Ms,
            "us" => Part::Us,
            "bytes" => Part::Bytes,
            _ => match name.strip_prefix("header:") {
                Some(header) if !header.is_empty() => Part::Header(header.to_lowercase()),
                _ => panic!(
                    "[cpeak] Unknown name {{{name}}} in the request log format. The names are {{date}} {{iso}} {{ip}} {{method}} {{url}} {{path}} {{status}} {{status_message}} {{ms}} {{us}} {{bytes}} and {{header:NAME}}."
                ),
            },
        };
        if !text.is_empty() {
            parts.push(Part::Text(std::mem::take(&mut text).into_bytes().into()));
        }
        parts.push(part);
        rest = &rest[open + close + 1..];
    }
    text.push_str(rest);
    if !text.is_empty() {
        parts.push(Part::Text(text.into_bytes().into()));
    }
    parts
}

// ------------------------------------------------------------------------------------------
// On the worker threads

// The buffer of the worker thread (None on the other threads, and in apps without the option
// and cpeak::log_requests())
struct ThreadLog {
    logger: Arc<Logger>,
    buffer: Vec<u8>,
    lines: u64,
    // The day the lines in the buffer are from (so a file that starts every day gets the right ones)
    day: [u8; 10],
    clock: Clock,
    // How many requests passed the other rules, for `sample` (one count for each
    // cpeak::log_requests(), by its id)
    sampled: Vec<u32>,
}

thread_local! {
    static THREAD_LOG: RefCell<Option<ThreadLog>> = const { RefCell::new(None) };
}

// Called by every worker thread of an app with a `log` option or a cpeak::log_requests() as it
// starts (see run_worker() in server.rs). It also hands the buffer over every flush_every, so
// the lines of a quiet server don't wait in it.
pub(crate) fn start_worker(runtime: &tokio::runtime::Runtime, logger: Arc<Logger>) {
    let flush_every = logger.flush_every;
    THREAD_LOG.set(Some(ThreadLog {
        buffer: Vec::with_capacity(logger.buffer_size),
        clock: Clock::new(logger.time_zone),
        logger,
        lines: 0,
        day: [0; 10],
        sampled: Vec::new(),
    }));

    // Outside of the worker's LocalSet, so it doesn't keep the worker from stopping (the
    // worker's own runtime only runs on this thread)
    runtime.spawn(async move {
        loop {
            tokio::time::sleep(flush_every).await;
            flush_thread();
        }
    });
}

// Called by every worker thread as it stops, so what's left in its buffer gets written
pub(crate) fn stop_worker() {
    flush_thread();
}

fn flush_thread() {
    THREAD_LOG.with(|thread_log| {
        if let Ok(mut thread_log) = thread_log.try_borrow_mut() {
            if let Some(thread_log) = thread_log.as_mut() {
                thread_log.hand_off();
            }
        }
    });
}

// A line for a request that is done and went through a cpeak::log_requests() (called by
// serve() in server.rs)
pub(crate) fn request(
    req: &CpeakRequest,
    res: &CpeakResponse,
    started: Instant,
    requests: &RequestLog,
) {
    THREAD_LOG.with(|thread_log| {
        if let Ok(mut thread_log) = thread_log.try_borrow_mut() {
            if let Some(thread_log) = thread_log.as_mut() {
                thread_log.request(req, res, started.elapsed(), requests);
            }
        }
    });
}

impl ThreadLog {
    fn request(
        &mut self,
        req: &CpeakRequest,
        res: &CpeakResponse,
        took: Duration,
        requests: &RequestLog,
    ) {
        if requests.only_errors && res.status_code < 400 {
            return;
        }
        if requests
            .slower_than
            .is_some_and(|slower_than| took <= slower_than)
        {
            return;
        }
        if requests.skip.is_some_and(|skip| skip(req, res)) {
            return;
        }
        if requests.sample > 1 {
            if self.sampled.len() <= requests.id {
                self.sampled.resize(requests.id + 1, 0);
            }
            let sampled = &mut self.sampled[requests.id];
            *sampled = sampled.wrapping_add(1);
            if (*sampled - 1) % requests.sample != 0 {
                return;
            }
        }

        let millis = self.start_line();
        let buffer = &mut self.buffer;
        for part in &requests.parts {
            match part {
                Part::Text(text) => buffer.extend_from_slice(text),
                Part::Date => buffer.extend_from_slice(self.clock.date()),
                Part::Iso => self.clock.write_iso(buffer, millis),
                Part::Ip => {
                    let from_header = requests
                        .ip_header
                        .as_ref()
                        .and_then(|header| req.headers.get(header))
                        .filter(|ip| !ip.is_empty());
                    match from_header {
                        Some(ip) => buffer.extend_from_slice(ip.as_bytes()),
                        None => write_ip(buffer, &req.remote_address),
                    }
                }
                Part::Method => buffer.extend_from_slice(req.method.as_bytes()),
                // The URL the request came in with, whatever a middleware function did to req.url
                Part::Url => match req.uri.path_and_query() {
                    Some(path_and_query) if req.uri.scheme().is_none() => {
                        buffer.extend_from_slice(path_and_query.as_str().as_bytes())
                    }
                    _ => {
                        let _ = write!(buffer, "{}", req.uri);
                    }
                },
                Part::Path => buffer.extend_from_slice(req.uri.path().as_bytes()),
                Part::Status => {
                    let _ = write!(buffer, "{}", res.status_code);
                }
                Part::StatusMessage => {
                    buffer.extend_from_slice(status_message(res.status_code).as_bytes())
                }
                Part::Ms => {
                    let micros = took.as_micros();
                    let _ = write!(buffer, "{}.{:02}", micros / 1000, micros % 1000 / 10);
                }
                Part::Us => {
                    let _ = write!(buffer, "{}", took.as_micros());
                }
                Part::Bytes => {
                    let _ = write!(buffer, "{}", res.body_bytes);
                }
                Part::Header(name) => match req.headers.get(name) {
                    Some(value) if !value.is_empty() => buffer.extend_from_slice(value.as_bytes()),
                    _ => buffer.push(b'-'),
                },
            }
        }
        self.end_line();
    }

    fn message(&mut self, message: &LogMessage) {
        match message {
            LogMessage::Text(text) => {
                self.start_line();
                self.buffer.extend_from_slice(self.clock.date());
                self.buffer.extend_from_slice(b" -- ");
                self.buffer.extend_from_slice(text.as_bytes());
                self.end_line();
            }
            // Errors don't wait in the buffer. They go to be written right away.
            LogMessage::Error(error) => {
                self.clock.now();
                let mut bytes = Vec::new();
                write_error(&mut bytes, &self.clock, error);
                send(
                    Job::Write {
                        sink: self.logger.errors.clone(),
                        day: self.clock.day(),
                        bytes,
                    },
                    true,
                );
            }
        }
    }

    // Reads the clock for a new line. If the day changed, the lines of the day before go first.
    fn start_line(&mut self) -> u32 {
        let millis = self.clock.now();
        let day = self.clock.day();
        if day != self.day {
            self.hand_off();
            self.day = day;
        }
        millis
    }

    fn end_line(&mut self) {
        self.buffer.push(b'\n');
        self.lines += 1;
        if self.buffer.len() >= self.logger.buffer_size {
            self.hand_off();
        }
    }

    // Hand the buffer over to be written. If too many are waiting already, its lines are dropped.
    fn hand_off(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        let bytes = std::mem::replace(
            &mut self.buffer,
            Vec::with_capacity(self.logger.buffer_size),
        );
        let lines = std::mem::take(&mut self.lines);
        let sink = self.logger.output.clone();
        let job = Job::Write {
            sink: sink.clone(),
            day: self.day,
            bytes,
        };
        if !send(job, false) {
            sink.dropped.fetch_add(lines, Ordering::Relaxed);
        }
    }
}

fn write_ip(buffer: &mut Vec<u8>, address: &SocketAddr) {
    let _ = write!(buffer, "{}", address.ip());
}

// A line of text, the way cpeak::log("...") writes it
fn write_text(bytes: &mut Vec<u8>, clock: &Clock, text: &str) {
    bytes.extend_from_slice(clock.date());
    bytes.extend_from_slice(b" -- ");
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(b'\n');
}

// An error, the way cpeak::log(&error) writes it. Rust has no stack trace for an error, so
// the "stack" is everything the error knows.
fn write_error(bytes: &mut Vec<u8>, clock: &Clock, error: &CpeakError) {
    let message = &error.message;
    bytes.extend_from_slice(b"---------------------------------------\n");
    bytes.extend_from_slice(clock.date());
    bytes.extend_from_slice(b"\nError Name: Error");
    if !message.is_empty() {
        let _ = write!(bytes, "\nError Message: {message}");
    }
    let _ = write!(bytes, "\nError Stack:\nError: {message}\n    {error:?}");
    bytes.extend_from_slice(b"\n---------------------------------------\n");
}

// ------------------------------------------------------------------------------------------
// The thread that writes

enum Job {
    Write {
        sink: Arc<Sink>,
        day: [u8; 10],
        bytes: Vec<u8>,
    },
    // Lets the one waiting know that everything sent before it is written
    Written(tokio::sync::oneshot::Sender<()>),
}

static WRITER: OnceLock<SyncSender<Job>> = OnceLock::new();

// `wait`: wait for a place in line instead of dropping the job (for errors). Returns false if
// the job was dropped.
fn send(job: Job, wait: bool) -> bool {
    let writer = WRITER.get_or_init(|| {
        let (writer, jobs) = mpsc::sync_channel(WAITING_BUFFERS);
        std::thread::Builder::new()
            .name("cpeak-log".to_string())
            .spawn(move || {
                for job in jobs {
                    match job {
                        Job::Write { sink, day, bytes } => sink.write(&day, &bytes),
                        Job::Written(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })
            .unwrap_or_else(|error| panic!("[cpeak] Failed to start the log: {error}"));
        writer
    });

    if wait {
        return writer.send(job).is_ok();
    }
    match writer.try_send(job) {
        Ok(()) => true,
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
    }
}

// Resolves once everything the workers handed over is written (when the server stops)
pub(crate) async fn written() {
    let Some(writer) = WRITER.get() else {
        return;
    };
    let (done, wait) = tokio::sync::oneshot::channel();
    if writer.send(Job::Written(done)).is_ok() {
        let _ = wait.await;
    }
}

// Where the lines end up: stdout, stderr or a file
struct Sink {
    output: LogOutput,
    time_zone: LogTimeZone,
    file: Mutex<OpenFile>,
    // Lines that were dropped because the disk didn't keep up
    dropped: AtomicU64,
}

#[derive(Default)]
struct OpenFile {
    path: String,
    file: Option<fs::File>,
    checked: Option<Instant>,
    // So a file that can't be opened says so once, not on every write
    failing: bool,
}

impl Sink {
    fn new(output: LogOutput, time_zone: LogTimeZone) -> Self {
        Self {
            output,
            time_zone,
            file: Mutex::new(OpenFile::default()),
            dropped: AtomicU64::new(0),
        }
    }

    fn write(&self, day: &[u8; 10], bytes: &[u8]) {
        self.write_bytes(day, bytes);

        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            let mut clock = Clock::new(self.time_zone);
            clock.now();
            let mut note = Vec::new();
            write_text(
                &mut note,
                &clock,
                &format!("[cpeak] {dropped} log lines were dropped, because the log couldn't be written as fast as they came"),
            );
            self.write_bytes(&clock.day(), &note);
        }
    }

    fn write_bytes(&self, day: &[u8; 10], bytes: &[u8]) {
        let pattern = match &self.output {
            LogOutput::Stdout => {
                let _ = io::stdout().lock().write_all(bytes);
                return;
            }
            LogOutput::Stderr => {
                let _ = io::stderr().lock().write_all(bytes);
                return;
            }
            LogOutput::File(pattern) => pattern,
        };

        let path = if pattern.contains("{day}") {
            pattern.replace("{day}", std::str::from_utf8(day).unwrap_or_default())
        } else {
            pattern.clone()
        };

        let mut open = lock(&self.file);
        let check = open
            .checked
            .is_none_or(|checked| checked.elapsed() >= FILE_CHECK_INTERVAL);
        if open.path != path || open.file.is_none() || (check && !Path::new(&path).exists()) {
            open.file = None;
            let opened = Path::new(&path)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| fs::OpenOptions::new().append(true).create(true).open(&path));
            match opened {
                Ok(file) => {
                    open.file = Some(file);
                    open.failing = false;
                }
                Err(error) => {
                    if !open.failing {
                        eprintln!("[cpeak] Failed to open the log file {path}: {error}");
                        open.failing = true;
                    }
                }
            }
            open.path = path;
        }
        if check {
            open.checked = Some(Instant::now());
        }

        if let Some(file) = &mut open.file {
            if let Err(error) = file.write_all(bytes) {
                if !open.failing {
                    eprintln!(
                        "[cpeak] Failed to write to the log file {}: {error}",
                        open.path
                    );
                    open.failing = true;
                }
                // Open it again next time
                open.file = None;
            }
        }
    }
}

// ------------------------------------------------------------------------------------------
// The dates

// The date of the current second, written out once a second instead of on every line
struct Clock {
    time_zone: LogTimeZone,
    second: i64,
    // 2026-10-15 14:00:00
    date: [u8; 19],
    // Z, or the offset from UTC like -04:00
    offset: [u8; 6],
    offset_len: usize,
}

impl Clock {
    fn new(time_zone: LogTimeZone) -> Self {
        Self {
            time_zone,
            second: i64::MIN,
            date: [b'0'; 19],
            offset: [0; 6],
            offset_len: 0,
        }
    }

    // Reads the time, and gives back the milliseconds of the current second
    fn now(&mut self) -> u32 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let second = now.as_secs() as i64;
        if second != self.second {
            self.set(second);
        }
        now.subsec_millis()
    }

    fn set(&mut self, second: i64) {
        self.second = second;
        let offset = match self.time_zone {
            LogTimeZone::Utc => 0,
            LogTimeZone::Local => local_offset(second),
        };
        let time = second + offset;
        let (year, month, day) = civil_from_days(time.div_euclid(86_400));
        let seconds_of_day = time.rem_euclid(86_400);

        let mut date = &mut self.date[..];
        let _ = write!(
            date,
            "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
            seconds_of_day / 3600,
            seconds_of_day % 3600 / 60,
            seconds_of_day % 60
        );

        let mut written = &mut self.offset[..];
        if self.time_zone == LogTimeZone::Utc {
            let _ = written.write_all(b"Z");
            self.offset_len = 1;
        } else {
            let sign = if offset < 0 { '-' } else { '+' };
            let minutes = offset.abs() / 60;
            let _ = write!(written, "{sign}{:02}:{:02}", minutes / 60, minutes % 60);
            self.offset_len = 6;
        }
    }

    fn date(&self) -> &[u8] {
        &self.date
    }

    fn day(&self) -> [u8; 10] {
        let mut day = [0; 10];
        day.copy_from_slice(&self.date[..10]);
        day
    }

    // 2026-10-15T14:00:00.123Z
    fn write_iso(&self, buffer: &mut Vec<u8>, millis: u32) {
        buffer.extend_from_slice(&self.date[..10]);
        buffer.push(b'T');
        buffer.extend_from_slice(&self.date[11..]);
        let _ = write!(buffer, ".{millis:03}");
        buffer.extend_from_slice(&self.offset[..self.offset_len]);
    }
}

// The year, month and day of a number of days since 1970-01-01 (Howard Hinnant's algorithm)
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

// How far the machine's time zone is from UTC at that moment, in seconds
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "freebsd"
))]
fn local_offset(second: i64) -> i64 {
    let time = second as libc::time_t;
    // SAFETY: localtime_r only writes into the tm we give it
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&time, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
}

// Elsewhere we can't ask, so the dates stay in UTC
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "freebsd"
)))]
fn local_offset(_second: i64) -> i64 {
    0
}

// (a thread that panicked while it held the lock is no reason for this one to panic too)
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
