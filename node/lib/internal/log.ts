// The log: log(), and log.requests(), the middleware that writes a line for every request
// that reaches it.
//
// A busy server handles thousands of requests a second, so a line can't cost much. The lines
// are collected in a buffer and written out together, without waiting on the disk (the writes
// go through libuv's thread pool), so the requests never do.
//
// If the disk can't keep up, the lines that don't fit in line are dropped instead of piling up
// in memory, and the log says how many were lost once it catches up.

import fs from "node:fs";
import path from "node:path";
import { STATUS_CODES } from "node:http";
import { Buffer } from "node:buffer";
import { performance } from "node:perf_hooks";
import type {
  LogOptions,
  LogOutput,
  LogTimeZone,
  RequestLogOptions
} from "./types";
import type { CpeakRequest, CpeakResponse, Middleware } from "../types";

// How many full buffers can wait to be written before we start dropping lines
const WAITING_BUFFERS = 64;

// How often we check that the log file is still there (someone may delete or move it while
// the server runs, and on Unix, writing to a deleted file doesn't fail)
const FILE_CHECK_INTERVAL = 5000;

const DASHES = "---------------------------------------";

type Part = { text: string } | { name: string } | { header: string };

const NAMES = [
  "date",
  "iso",
  "ip",
  "method",
  "url",
  "path",
  "status",
  "status_message",
  "ms",
  "us",
  "bytes"
];

// Turn the format into its pieces once, at startup. A mistyped name throws right here,
// instead of showing up in every line.
function parseFormat(format: string): Part[] {
  const parts: Part[] = [];
  let text = "";
  let rest = format;

  let open = rest.indexOf("{");
  while (open !== -1) {
    text += rest.slice(0, open);
    const close = rest.indexOf("}", open);
    if (close === -1) {
      throw new Error(
        `[cpeak] The request log format has a { without a }: "${format}"`
      );
    }
    const name = rest.slice(open + 1, close);
    let part: Part;
    if (NAMES.includes(name)) {
      part = { name };
    } else if (name.startsWith("header:") && name.length > 7) {
      part = { header: name.slice(7).toLowerCase() };
    } else {
      throw new Error(
        `[cpeak] Unknown name {${name}} in the request log format. The names are {date} {iso} {ip} {method} {url} {path} {status} {status_message} {ms} {us} {bytes} and {header:NAME}.`
      );
    }
    if (text) parts.push({ text });
    text = "";
    parts.push(part);
    rest = rest.slice(close + 1);
    open = rest.indexOf("{");
  }
  text += rest;
  if (text) parts.push({ text });
  return parts;
}

// The date of the current second, written out once a second instead of on every line
class Clock {
  #second = -1;
  // 2026-10-15 14:00:00
  date = "";
  // Z, or the offset from UTC like -04:00
  offset = "Z";
  millis = 0;

  constructor(private timeZone: LogTimeZone) {}

  // Reads the time
  now() {
    const now = Date.now();
    const second = Math.floor(now / 1000);
    this.millis = now - second * 1000;
    if (second !== this.#second) this.#set(second);
  }

  get day() {
    return this.date.slice(0, 10);
  }

  // 2026-10-15T14:00:00.123Z
  get iso() {
    return (
      this.date.slice(0, 10) +
      "T" +
      this.date.slice(11) +
      "." +
      String(this.millis).padStart(3, "0") +
      this.offset
    );
  }

  #set(second: number) {
    this.#second = second;
    const d = new Date(second * 1000);
    const two = (n: number) => (n < 10 ? "0" : "") + n;

    if (this.timeZone === "local") {
      this.date =
        String(d.getFullYear()).padStart(4, "0") +
        "-" +
        two(d.getMonth() + 1) +
        "-" +
        two(d.getDate()) +
        " " +
        two(d.getHours()) +
        ":" +
        two(d.getMinutes()) +
        ":" +
        two(d.getSeconds());
      const offset = -d.getTimezoneOffset();
      const minutes = Math.abs(offset);
      this.offset =
        (offset < 0 ? "-" : "+") +
        two(Math.floor(minutes / 60)) +
        ":" +
        two(minutes % 60);
    } else {
      this.date =
        String(d.getUTCFullYear()).padStart(4, "0") +
        "-" +
        two(d.getUTCMonth() + 1) +
        "-" +
        two(d.getUTCDate()) +
        " " +
        two(d.getUTCHours()) +
        ":" +
        two(d.getUTCMinutes()) +
        ":" +
        two(d.getUTCSeconds());
      this.offset = "Z";
    }
  }
}

// A line of text, the way log("...") writes it
function textLine(clock: Clock, text: string) {
  return clock.date + " -- " + text + "\n";
}

// An error, the way log(error) writes it: its name, its message and its stack trace
function errorBlock(clock: Clock, error: any) {
  const name = error?.name;
  const message = error?.message;
  const stack = error?.stack;
  return (
    DASHES +
    "\n" +
    clock.date +
    (name ? "\nError Name: " + name : "") +
    (message ? "\nError Message: " + message : "") +
    (stack ? "\nError Stack:\n" + stack : "") +
    "\n" +
    DASHES +
    "\n"
  );
}

// Where the lines end up: stdout, stderr or a file
class Sink {
  // Lines that were dropped because the disk didn't keep up
  dropped = 0;

  #fd: number | null = null;
  #path = "";
  #checked = 0;
  // So a file that can't be opened says so once, not on every write
  #failing = false;
  // What waits to be written, with the day it's from
  #queue: { day: string; text: string }[] = [];
  #queued = 0;
  #writing = false;

  constructor(
    private output: LogOutput,
    private timeZone: LogTimeZone,
    private limit: number
  ) {}

  // Returns false if the text was dropped. `mayDrop` is false for errors.
  write(day: string, text: string, lines: number, mayDrop: boolean): boolean {
    if (typeof this.output === "string") {
      const stream = this.output === "stderr" ? process.stderr : process.stdout;
      if (mayDrop && stream.writableLength > this.limit) {
        this.dropped += lines;
        return false;
      }
      stream.write(text);
      this.#noteDropped();
      return true;
    }

    if (mayDrop && this.#queued + text.length > this.limit) {
      this.dropped += lines;
      return false;
    }
    this.#queue.push({ day, text });
    this.#queued += text.length;
    this.#next();
    return true;
  }

  // Write everything that waits, and then `last`, before the program exits (only sync calls
  // work at that point). The write that is already on its way finishes on its own.
  writeSync(last?: { day: string; text: string }) {
    const waiting = this.#queue.splice(this.#writing ? 1 : 0);
    if (last) waiting.push(last);
    for (const { day, text } of waiting) {
      this.#queued -= text.length;
      if (typeof this.output === "string") {
        (this.output === "stderr" ? process.stderr : process.stdout).write(
          text
        );
        continue;
      }
      const fd = this.#open(day);
      if (fd === null) continue;
      try {
        fs.writeSync(fd, text);
      } catch {}
    }
  }

  #next(): void {
    if (this.#writing || !this.#queue.length) return;
    const { day, text } = this.#queue[0];
    const fd = this.#open(day);
    if (fd === null) {
      this.#queue.shift();
      this.#queued -= text.length;
      this.#next();
      return;
    }

    this.#writing = true;
    fs.write(fd, text, (error) => {
      this.#writing = false;
      this.#queue.shift();
      this.#queued -= text.length;
      if (error) {
        if (!this.#failing) {
          console.error(
            `[cpeak] Failed to write to the log file ${this.#path}:`,
            error
          );
          this.#failing = true;
        }
        // Open it again next time
        this.#close();
      } else {
        this.#noteDropped();
      }
      this.#next();
    });
  }

  #noteDropped() {
    if (!this.dropped) return;
    const dropped = this.dropped;
    this.dropped = 0;
    const clock = new Clock(this.timeZone);
    clock.now();
    this.write(
      clock.day,
      textLine(
        clock,
        `[cpeak] ${dropped} log lines were dropped, because the log couldn't be written as fast as they came`
      ),
      0,
      false
    );
  }

  // The file of that day, opened if it isn't yet (or isn't there anymore)
  #open(day: string): number | null {
    const file = (this.output as { file: string }).file;
    const filePath = file.includes("{day}")
      ? file.replaceAll("{day}", day)
      : file;

    const now = Date.now();
    const check = now - this.#checked >= FILE_CHECK_INTERVAL;
    if (check) this.#checked = now;

    if (
      this.#path !== filePath ||
      this.#fd === null ||
      (check && !fs.existsSync(filePath))
    ) {
      this.#close();
      this.#path = filePath;
      try {
        const folder = path.dirname(filePath);
        if (folder) fs.mkdirSync(folder, { recursive: true });
        this.#fd = fs.openSync(filePath, "a");
        this.#failing = false;
      } catch (error) {
        if (!this.#failing) {
          console.error(
            `[cpeak] Failed to open the log file ${filePath}:`,
            error
          );
          this.#failing = true;
        }
      }
    }
    return this.#fd;
  }

  #close() {
    if (this.#fd === null) return;
    // A write in the thread pool may still use it, so it closes after that
    const fd = this.#fd;
    this.#fd = null;
    fs.close(fd, () => {});
  }
}

// The options of a log.requests(), ready to use
export class RequestLog {
  parts: Part[];
  ipHeader?: string;
  onlyErrors: boolean;
  slowerThan?: number;
  sample: number;
  skip?: (req: CpeakRequest, res: CpeakResponse) => boolean;
  // How many requests passed the other rules, for `sample`
  sampled = 0;

  constructor(options: RequestLogOptions) {
    this.parts = parseFormat(
      options.format ??
        "{date} -- {ip} -- {method} {url} {status} {status_message} -- {ms} ms"
    );
    this.ipHeader = options.ipHeader?.toLowerCase();
    this.onlyErrors = options.onlyErrors ?? false;
    this.slowerThan = options.slowerThan;
    this.sample = Math.max(options.sample ?? 1, 1);
    this.skip = options.skip;
    requestLogs++;
  }
}

// The `log` option of an app, ready to use
export class Logger {
  #output: Sink;
  #errors: Sink;
  #clock: Clock;
  #bufferSize: number;

  #buffer = "";
  #lines = 0;
  // The day the lines in the buffer are from (so a file that starts every day gets the right ones)
  #day = "";

  constructor(options: LogOptions) {
    const timeZone = options.timeZone ?? "utc";
    this.#bufferSize = options.bufferSize ?? 64 * 1024;
    const limit = Math.max(this.#bufferSize, 1024) * WAITING_BUFFERS;
    this.#output = new Sink(options.output ?? "stdout", timeZone, limit);
    this.#errors = new Sink(options.errors ?? "stderr", timeZone, limit);
    this.#clock = new Clock(timeZone);

    // Hand the buffer over every flushEvery, so the lines of a quiet server don't wait in it
    setInterval(
      () => this.flush(),
      Math.max(options.flushEvery ?? 100, 1)
    ).unref();
    LOGGERS.add(this);
  }

  // A line for a request that is done and went through a log.requests(). `url` is the URL it
  // came in with.
  request(
    requests: RequestLog,
    req: CpeakRequest,
    res: CpeakResponse,
    url: string,
    started: number
  ) {
    const took = performance.now() - started;

    if (requests.onlyErrors && res.statusCode < 400) return;
    if (requests.slowerThan !== undefined && took <= requests.slowerThan)
      return;
    if (requests.skip?.(req, res)) return;
    if (requests.sample > 1 && requests.sampled++ % requests.sample !== 0)
      return;

    this.#startLine();
    let line = "";
    for (const part of requests.parts) {
      if ("text" in part) {
        line += part.text;
      } else if ("header" in part) {
        line += headerValue(req, part.header) || "-";
      } else {
        switch (part.name) {
          case "date":
            line += this.#clock.date;
            break;
          case "iso":
            line += this.#clock.iso;
            break;
          case "ip":
            line +=
              (requests.ipHeader && headerValue(req, requests.ipHeader)) ||
              req.socket?.remoteAddress ||
              "-";
            break;
          case "method":
            line += req.method;
            break;
          case "url":
            line += url;
            break;
          case "path": {
            const q = url.indexOf("?");
            line += q === -1 ? url : url.slice(0, q);
            break;
          }
          case "status":
            line += res.statusCode;
            break;
          case "status_message":
            line += res.statusMessage || STATUS_CODES[res.statusCode] || "";
            break;
          case "ms":
            line += took.toFixed(2);
            break;
          case "us":
            line += Math.round(took * 1000);
            break;
          case "bytes":
            line += res._bodyBytes;
            break;
        }
      }
    }
    this.#endLine(line);
  }

  message(message: unknown) {
    this.#clock.now();
    if (
      typeof message === "string" ||
      typeof message !== "object" ||
      message === null
    ) {
      this.#startLine();
      this.#endLine(this.#clock.date + " -- " + String(message));
      return;
    }
    // Errors don't wait in the buffer. They go to be written right away.
    this.#errors.write(
      this.#clock.day,
      errorBlock(this.#clock, message),
      1,
      false
    );
  }

  // Hand the buffer over to be written
  flush() {
    if (!this.#buffer) return;
    const text = this.#buffer;
    const lines = this.#lines;
    this.#buffer = "";
    this.#lines = 0;
    this.#output.write(this.#day, text, lines, true);
  }

  // Before the program exits: the buffer, and everything that waits to be written
  flushSync() {
    const text = this.#buffer;
    this.#buffer = "";
    this.#lines = 0;
    this.#output.writeSync(text ? { day: this.#day, text } : undefined);
    this.#errors.writeSync();
  }

  // Reads the clock for a new line. If the day changed, the lines of the day before go first.
  #startLine() {
    this.#clock.now();
    const day = this.#clock.day;
    if (day !== this.#day) {
      this.flush();
      this.#day = day;
    }
  }

  #endLine(line: string) {
    this.#buffer += line + "\n";
    this.#lines++;
    if (this.#buffer.length >= this.#bufferSize) this.flush();
  }
}

function headerValue(req: CpeakRequest, name: string): string | undefined {
  const value = req.headers[name];
  return Array.isArray(value) ? value.join(", ") : value;
}

// How many bytes a chunk of the body is (for {bytes})
export function byteLength(chunk: unknown, encoding?: unknown): number {
  if (typeof chunk === "string") {
    return Buffer.byteLength(
      chunk,
      typeof encoding === "string" ? (encoding as BufferEncoding) : "utf8"
    );
  }
  if (chunk instanceof Uint8Array) return chunk.byteLength;
  return 0;
}

// Every app with the option, so what they have left is written when the program exits
const LOGGERS = new Set<Logger>();
// What log() uses: the `log` option of the last app made with one
let current: Logger | undefined;
// What the request lines of an app without the option are written with (made when needed)
let fallback: Logger | undefined;

// How many log.requests() were made. While there are none, the requests don't read the clock
// for their {ms}.
let requestLogs = 0;

// Whether there is a log.requests() (cpeak() asks on every request)
export function logsRequests() {
  return requestLogs > 0;
}

// What an app without the `log` option writes its request lines with
export function defaultLogger(): Logger {
  return (fallback ??= new Logger({}));
}

process.on("exit", () => {
  for (const logger of LOGGERS) logger.flushSync();
});

// Called by cpeak() when the app has a `log` option
export function startLog(options: LogOptions): Logger {
  const logger = new Logger(options);
  current = logger;
  return logger;
}

// A middleware function that writes a line for every request that reaches it, once the
// response is sent:
//
//     server.beforeEach(serveStatic("./public"));
//     server.beforeEach(log.requests());
//
// Where it is decides which requests get one. Above, the files that serveStatic() sends never
// reach it, so they don't. It can go on a route of its own too.
//
// The lines go where the `log` option of your app says (stdout without one). The {ms} of a line
// is from when the request came in, not from when it reached this middleware.
function logRequests(options: RequestLogOptions = {}): Middleware {
  const requests = new RequestLog(options);

  return (req, res, next) => {
    // One line for a request, even when it goes through more than one of these (the last one
    // it reaches has the say)
    if (!res._requestLog) {
      res.once("close", () =>
        (res._log ?? defaultLogger()).request(
          res._requestLog!,
          req,
          res,
          res._url!,
          res._started!
        )
      );
    }
    res._requestLog = requests;
    next();
  };
}

// Write a line to the log, with the date in front of it:
//
//     log("The server has started");
//
// Or an error, which goes to the log of errors:
//
//     log(error);
//
// It goes where the `log` option of your app says. With no app that has the option, the text
// goes to stdout and the errors to stderr.
//
// log.requests() is the middleware that writes a line for every request (see logRequests()).
export const log = Object.assign(writeLog, { requests: logRequests });

function writeLog(message: unknown): void {
  if (current) return current.message(message);

  const clock = new Clock("utc");
  clock.now();
  if (
    typeof message === "string" ||
    typeof message !== "object" ||
    message === null
  ) {
    process.stdout.write(textLine(clock, String(message)));
  } else {
    process.stderr.write(errorBlock(clock, message));
  }
}
