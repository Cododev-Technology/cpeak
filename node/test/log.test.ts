import assert from "node:assert";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import type { AddressInfo } from "node:net";

import cpeak, { log } from "../lib/";

import type { Cpeak, LogOptions, RequestLogOptions } from "../lib/";

// A folder of its own for each test
function folder(name: string) {
  const dir = path.join(os.tmpdir(), `cpeak-log-${process.pid}-${name}`);
  fs.rmSync(dir, { recursive: true, force: true });
  return dir;
}

// What's in the file, once it has at least `count` lines (or after 3 seconds)
async function lines(file: string, count: number): Promise<string[]> {
  const started = Date.now();
  while (true) {
    let content = "";
    try {
      content = fs.readFileSync(file, "utf8");
    } catch {}
    const found = content.split("\n").filter(Boolean);
    if (found.length >= count || Date.now() - started > 3000) return found;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

const servers: Cpeak[] = [];

async function listen(server: Cpeak): Promise<string> {
  servers.push(server);
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  return `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
}

// An app with the `log` option, and log.requests() in front of its routes (unless `requests`
// is false)
async function start(
  options: LogOptions,
  requests: RequestLogOptions | false = {}
): Promise<string> {
  const server = cpeak({ log: options });

  if (requests !== false) server.beforeEach(log.requests(requests));

  server.route("get", "/hello", (req, res) => {
    res.end("hello");
  });
  server.route("get", "/missing", (req, res) => {
    res.status(404).end();
  });
  server.route("get", "/moved", (req, res) => res.redirect("/hello"));
  server.route("get", "/slow", async (req, res) => {
    await new Promise((resolve) => setTimeout(resolve, 100));
    res.end();
  });
  server.route("get", "/stream", (req, res) => {
    res.write("one ");
    res.write("two ");
    res.end("three");
  });
  server.route("get", "/text", (req, res) => {
    log("hello from a route");
    res.end();
  });
  server.route("get", "/error", (req, res) => {
    log(new Error("something broke"));
    res.end();
  });

  return listen(server);
}

async function get(url: string, headers: Record<string, string> = {}) {
  const res = await fetch(url, { headers, redirect: "manual" });
  await res.arrayBuffer();
}

describe("The log", function () {
  after(function () {
    for (const server of servers) server.close();
  });

  it("writes a line for every request", async function () {
    const dir = folder("default");
    const url = await start({ output: { file: path.join(dir, "info.log") } });

    await get(url + "/hello?name=joe");
    const found = await lines(path.join(dir, "info.log"), 1);
    assert.strictEqual(found.length, 1);

    // 2026-10-15 14:00:00 -- ::ffff:127.0.0.1 -- GET /hello?name=joe 200 OK -- 0.21 ms
    assert.match(
      found[0],
      /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} -- 127\.0\.0\.1 -- GET \/hello\?name=joe 200 OK -- \d+\.\d{2} ms$/
    );
  });

  it("writes the format it is given", async function () {
    const dir = folder("format");
    const url = await start(
      { output: { file: path.join(dir, "info.log") } },
      {
        format:
          "{ip} {method} {url} {path} {status} {status_message} {bytes} {header:User-Agent} {header:x-missing}"
      }
    );

    await get(url + "/hello?a=1", { "User-Agent": "tester/1.0" });
    assert.deepStrictEqual(await lines(path.join(dir, "info.log"), 1), [
      "127.0.0.1 GET /hello?a=1 /hello 200 OK 5 tester/1.0 -"
    ]);

    await get(url + "/moved", { "User-Agent": "" });
    await get(url + "/nowhere", { "User-Agent": "" });
    await get(url + "/stream", { "User-Agent": "" });
    const found = await lines(path.join(dir, "info.log"), 4);
    assert.ok(found.includes("127.0.0.1 GET /moved /moved 302 Found 0 - -"));
    // Cpeak's own 404 is {"error":"Cannot GET /nowhere"}
    assert.ok(
      found.includes("127.0.0.1 GET /nowhere /nowhere 404 Not Found 31 - -")
    );
    // Every chunk of a streamed body counts
    assert.ok(found.includes("127.0.0.1 GET /stream /stream 200 OK 13 - -"));
  });

  it("reads the ip from a header", async function () {
    const dir = folder("ip");
    const url = await start(
      { output: { file: path.join(dir, "info.log") } },
      { format: "{ip}", ipHeader: "X-Forwarded-For" }
    );

    await get(url + "/hello", { "X-Forwarded-For": "203.0.113.7" });
    assert.deepStrictEqual(await lines(path.join(dir, "info.log"), 1), [
      "203.0.113.7"
    ]);

    // Without the header, it's the address of the connection
    await get(url + "/hello");
    assert.deepStrictEqual(await lines(path.join(dir, "info.log"), 2), [
      "203.0.113.7",
      "127.0.0.1"
    ]);
  });

  it("only errors", async function () {
    const dir = folder("only-errors");
    const url = await start(
      { output: { file: path.join(dir, "info.log") } },
      { format: "{url} {status}", onlyErrors: true }
    );

    await get(url + "/hello");
    await get(url + "/missing");
    await get(url + "/moved");
    assert.deepStrictEqual(await lines(path.join(dir, "info.log"), 1), [
      "/missing 404"
    ]);
  });

  it("slower than", async function () {
    const dir = folder("slower-than");
    const url = await start(
      { output: { file: path.join(dir, "info.log") } },
      { format: "{url}", slowerThan: 50 }
    );

    await get(url + "/hello");
    await get(url + "/slow");
    await get(url + "/hello");
    assert.deepStrictEqual(await lines(path.join(dir, "info.log"), 1), [
      "/slow"
    ]);
  });

  it("skip", async function () {
    const dir = folder("skip");
    const url = await start(
      { output: { file: path.join(dir, "info.log") } },
      { format: "{url}", skip: (req) => req.url === "/hello" }
    );

    await get(url + "/hello");
    await get(url + "/missing");
    assert.deepStrictEqual(await lines(path.join(dir, "info.log"), 1), [
      "/missing"
    ]);
  });

  it("sample", async function () {
    const dir = folder("sample");
    const url = await start(
      { output: { file: path.join(dir, "info.log") } },
      { format: "{url}", sample: 3 }
    );

    for (let i = 0; i < 7; i++) await get(url + "/hello?" + i);
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.deepStrictEqual(await lines(path.join(dir, "info.log"), 3), [
      "/hello?0",
      "/hello?3",
      "/hello?6"
    ]);
  });

  it("log() in a route", async function () {
    const dir = folder("in-a-route");
    const url = await start(
      {
        output: { file: path.join(dir, "info.log") },
        errors: { file: path.join(dir, "errors.log") }
      },
      false
    );

    await get(url + "/text");
    const found = await lines(path.join(dir, "info.log"), 1);
    assert.deepStrictEqual(
      found.map((line) => line.slice(19)),
      [" -- hello from a route"]
    );

    await get(url + "/error");
    const errors = (await lines(path.join(dir, "errors.log"), 6)).join("\n");
    assert.match(
      errors,
      /^-{39}\n\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\nError Name: Error\nError Message: something broke\nError Stack:\nError: something broke\n {4}at [^]+\n-{39}$/
    );
  });

  it("a file for every day", async function () {
    const dir = folder("days");
    const url = await start({
      output: { file: path.join(dir, "{day}", "info.log") }
    });

    await get(url + "/hello");
    const started = Date.now();
    while (!fs.existsSync(dir) && Date.now() - started < 3000) {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    const days = fs.readdirSync(dir);
    assert.strictEqual(days.length, 1);
    // Named after the day of the line in it (2026-10-15)
    assert.match(days[0], /^\d{4}-\d{2}-\d{2}$/);
    const found = await lines(path.join(dir, days[0], "info.log"), 1);
    assert.ok(found[0].startsWith(days[0]));
  });

  it("iso dates", async function () {
    const dir = folder("iso");
    let url = await start(
      { output: { file: path.join(dir, "utc.log") } },
      { format: "{iso}" }
    );
    await get(url + "/hello");
    // 2026-10-15T14:00:00.123Z
    assert.match(
      (await lines(path.join(dir, "utc.log"), 1))[0],
      /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/
    );

    url = await start(
      { output: { file: path.join(dir, "local.log") }, timeZone: "local" },
      { format: "{iso}" }
    );
    await get(url + "/hello");
    // 2026-10-15T10:00:00.123-04:00
    assert.match(
      (await lines(path.join(dir, "local.log"), 1))[0],
      /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}[+-]\d{2}:\d{2}$/
    );
  });

  it("waits in the buffer until flushEvery", async function () {
    const dir = folder("flush");
    const url = await start({
      output: { file: path.join(dir, "info.log") },
      flushEvery: 300
    });

    await get(url + "/hello");
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.ok(!fs.existsSync(path.join(dir, "info.log")));
    assert.strictEqual((await lines(path.join(dir, "info.log"), 1)).length, 1);
  });

  it("a full buffer goes right away", async function () {
    const dir = folder("full-buffer");
    const url = await start({
      output: { file: path.join(dir, "info.log") },
      bufferSize: 1,
      flushEvery: 60_000
    });

    await get(url + "/hello");
    assert.strictEqual((await lines(path.join(dir, "info.log"), 1)).length, 1);
  });

  it("everything is written when the server closes", async function () {
    const dir = folder("close");
    const url = await start({
      output: { file: path.join(dir, "info.log") },
      flushEvery: 60_000
    });

    for (let i = 0; i < 5; i++) await get(url + "/hello");
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.ok(!fs.existsSync(path.join(dir, "info.log")));

    const server = servers.pop()!;
    await new Promise((resolve) => {
      server.close(resolve);
      server.server.closeAllConnections();
    });
    assert.strictEqual((await lines(path.join(dir, "info.log"), 5)).length, 5);
  });

  it("everything is written when the program exits", function () {
    this.timeout(20_000);
    const dir = folder("exit");
    const lib = new URL("../lib/index.ts", import.meta.url).pathname;
    const program = `
      import cpeak, { log } from ${JSON.stringify(lib)};
      cpeak({ log: { output: { file: ${JSON.stringify(path.join(dir, "info.log"))} }, flushEvery: 60000 } });
      log("one");
      log("two");
      process.exit(0);
    `;
    const result = spawnSync(
      process.execPath,
      ["--import", "tsx", "--input-type=module", "-e", program],
      { encoding: "utf8" }
    );
    assert.strictEqual(result.status, 0, result.stderr);
    const found = fs
      .readFileSync(path.join(dir, "info.log"), "utf8")
      .split("\n")
      .filter(Boolean)
      .map((line) => line.slice(19));
    assert.deepStrictEqual(found, [" -- one", " -- two"]);
  });

  it("only the requests that reach it", async function () {
    const dir = folder("position");
    const server = cpeak({
      log: { output: { file: path.join(dir, "info.log") } }
    });
    // Like serveStatic(): it sends the response itself, so the request stops here
    server.beforeEach((req, res, next) => {
      if (req.url === "/early") return res.end("sent before the log");
      next();
    });
    server.beforeEach(log.requests({ format: "{url}" }));
    server.route("get", "/hello", (req, res) => {
      res.end("hello");
    });
    const url = await listen(server);

    await get(url + "/early");
    await get(url + "/hello");
    // (more than flushEvery, so a line for /early would be there by now)
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.deepStrictEqual(await lines(path.join(dir, "info.log"), 1), [
      "/hello"
    ]);
  });

  it("on a route of its own", async function () {
    const dir = folder("route");
    const server = cpeak({
      log: { output: { file: path.join(dir, "info.log") } }
    });
    server.route(
      "get",
      "/logged",
      log.requests({ format: "{url}" }),
      (req, res) => {
        res.end();
      }
    );
    server.route("get", "/hello", (req, res) => {
      res.end("hello");
    });
    const url = await listen(server);

    await get(url + "/hello");
    await get(url + "/logged");
    await get(url + "/hello");
    // (more than flushEvery, so the lines for /hello would be there by now)
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.deepStrictEqual(await lines(path.join(dir, "info.log"), 1), [
      "/logged"
    ]);
  });

  it("one line for a request that goes through it twice", async function () {
    const dir = folder("twice");
    const server = cpeak({
      log: { output: { file: path.join(dir, "info.log") } }
    });
    server.beforeEach(log.requests({ format: "all {url}" }));
    server.route(
      "get",
      "/hello",
      log.requests({ format: "route {url}" }),
      (req, res) => {
        res.end("hello");
      }
    );
    const url = await listen(server);

    await get(url + "/hello");
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.deepStrictEqual(await lines(path.join(dir, "info.log"), 1), [
      "route /hello"
    ]);
  });

  it("a mistyped name throws at startup", function () {
    assert.throws(
      () => log.requests({ format: "{ip} {nope}" }),
      /Unknown name \{nope\} in the request log format/
    );
  });
});
