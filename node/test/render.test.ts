import assert from "node:assert";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import supertest from "supertest";
import cpeak, { render } from "../lib/";
import { MAX_PATTERN } from "../lib/utils/render";

import type { Cpeak, CpeakRequest, CpeakResponse } from "../lib/types";

const PORT = 7543;
const request = supertest(`http://localhost:${PORT}`);

describe("Rendering a template with render middleware", function () {
  let server: Cpeak;

  before(function (done) {
    server = cpeak();
    server.beforeEach(render());

    server.route("get", "/", (req: CpeakRequest, res: CpeakResponse) => {
      return res.render(
        `./test/files/index.html`,
        {
          title: "Home",
          body: "Welcome to the Home Page"
        },
        "text/html"
      );
    });

    server.route(
      "get",
      "/inferred",
      (req: CpeakRequest, res: CpeakResponse) => {
        return res.render(`./test/files/index.html`, {
          title: "Home",
          body: "Welcome to the Home Page"
        });
      }
    );

    server.route("get", "/escape", (req: CpeakRequest, res: CpeakResponse) => {
      return res.render(
        `./test/files/escape.html`,
        {
          content: "<script>alert('xss')</script>",
          raw: "<strong>bold</strong>"
        },
        "text/html"
      );
    });

    server.route("get", "/include", (req: CpeakRequest, res: CpeakResponse) => {
      return res.render(
        `./test/files/with-include.html`,
        {
          title: "With Include",
          navTitle: "My Nav",
          body: "Page body"
        },
        "text/html"
      );
    });

    server.listen(PORT, done);
  });

  after(function (done) {
    server.close(done);
  });

  it("should render the HTML file with variables injected and infer MIME type", async function () {
    const explicit = await request.get("/");
    const inferred = await request.get("/inferred");

    assert.equal(explicit.status, 200);
    assert.match(explicit.headers["content-type"] ?? "", /^text\/html\b/);
    assert.ok(explicit.text.includes("<title>Home</title>"));
    assert.ok(explicit.text.includes("<p>Welcome to the Home Page</p>"));

    assert.equal(inferred.status, 200);
    assert.match(inferred.headers["content-type"] ?? "", /^text\/html\b/);
    assert.ok(inferred.text.includes("<title>Home</title>"));
  });

  it("should escape HTML in {{ }} and output raw HTML in <cpeak html={} />", async function () {
    const res = await request.get("/escape");

    assert.equal(res.status, 200);
    assert.ok(
      res.text.includes("&lt;script&gt;alert(&#39;xss&#39;)&lt;/script&gt;")
    );
    assert.ok(!res.text.includes("<script>"));
    assert.ok(res.text.includes("<strong>bold</strong>"));
  });

  it("should resolve and inline <cpeak include> partials", async function () {
    const res = await request.get("/include");

    assert.equal(res.status, 200);
    assert.ok(res.text.includes("<title>With Include</title>"));
    assert.ok(res.text.includes("<nav>My Nav</nav>"));
    assert.ok(res.text.includes("<main>Page body</main>"));
    assert.ok(!res.text.includes("<cpeak"));
  });

  it("should interpolate template tags that straddle the safe/tail chunk boundary", async function () {
    // Place {{ name }} so it starts 5 bytes before the safe/tail boundary
    // (str.length - MAX_PATTERN) and ends 5 bytes inside the tail.
    const PAD = 5;
    const TOTAL = MAX_PATTERN + 72;
    const content =
      "A".repeat(TOTAL - MAX_PATTERN - PAD) +
      "{{ name }}" +
      "B".repeat(MAX_PATTERN - PAD);

    const tmpFile = path.join(
      os.tmpdir(),
      `cpeak-render-boundary-${Date.now()}.html`
    );
    await fs.writeFile(tmpFile, content, "utf-8");

    const PORT3 = 7545;
    const tmpServer = cpeak();
    tmpServer.beforeEach(render());
    tmpServer.route(
      "get",
      "/boundary",
      (req: CpeakRequest, res: CpeakResponse) => {
        return res.render(tmpFile, { name: "World" }, "text/html");
      }
    );
    await new Promise<void>((resolve) => tmpServer.listen(PORT3, resolve));
    const request3 = supertest(`http://localhost:${PORT3}`);

    try {
      const res = await request3.get("/boundary");
      assert.equal(res.status, 200);
      assert.ok(
        !res.text.includes("{{"),
        "raw template syntax should not appear in output"
      );
      assert.ok(res.text.includes("World"), "variable should be substituted");
    } finally {
      await new Promise<void>((resolve) => tmpServer.close(() => resolve()));
      await fs.unlink(tmpFile).catch(() => {});
    }
  });

  it("should re-read the file on every request reflecting any changes", async function () {
    const tmpFile = path.join(
      os.tmpdir(),
      `cpeak-render-test-${Date.now()}.html`
    );
    await fs.writeFile(tmpFile, "<p>{{ value }}</p>", "utf-8");

    const tmpServer = cpeak();
    tmpServer.beforeEach(render());
    tmpServer.route("get", "/live", (req: CpeakRequest, res: CpeakResponse) => {
      return res.render(tmpFile, { value: "original" }, "text/html");
    });

    const PORT2 = 7544;
    await new Promise<void>((resolve) => tmpServer.listen(PORT2, resolve));
    const request2 = supertest(`http://localhost:${PORT2}`);

    try {
      const first = await request2.get("/live");
      assert.ok(first.text.includes("<p>original</p>"));

      await fs.writeFile(
        tmpFile,
        "<p>{{ value }}</p><span>updated</span>",
        "utf-8"
      );

      const second = await request2.get("/live");
      assert.ok(second.text.includes("<span>updated</span>"));
    } finally {
      await new Promise<void>((resolve) => tmpServer.close(() => resolve()));
      await fs.unlink(tmpFile).catch(() => {});
    }
  });
});
