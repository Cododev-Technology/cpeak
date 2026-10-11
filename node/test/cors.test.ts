import assert from "node:assert";
import supertest from "supertest";
import cpeak, { cors } from "../lib/index";
import type { Cpeak, CpeakRequest, CpeakResponse } from "../lib/types";

const PORT = 7543;
const request = supertest(`http://localhost:${PORT}`);

describe("CORS middleware", function () {
  let server: Cpeak;

  before(function (done) {
    server = cpeak();

    server.beforeEach(
      cors({
        origin: "http://localhost:3000",
        credentials: true
      })
    );

    server.route(
      "get",
      "/test-cors",
      (req: CpeakRequest, res: CpeakResponse) => {
        res.status(200).json({ message: "CORS test successful" });
      }
    );

    server.listen(PORT, done);
  });

  after(function (done) {
    server.close(done);
  });

  it("should add CORS headers to the response", async function () {
    const res = await request
      .get("/test-cors")
      .set("Origin", "http://localhost:3000");

    assert.strictEqual(res.status, 200);
    assert.strictEqual(
      res.headers["access-control-allow-origin"],
      "http://localhost:3000"
    );
    assert.strictEqual(res.headers["access-control-allow-credentials"], "true");
  });

  it("should handle OPTIONS preflight requests", async function () {
    const res = await request
      .options("/test-cors")
      .set("Origin", "http://localhost:3000")
      .set("Access-Control-Request-Method", "GET")
      .set("Access-Control-Request-Headers", "Content-Type, Authorization");

    assert.strictEqual(res.status, 204);
    assert.strictEqual(
      res.headers["access-control-allow-origin"],
      "http://localhost:3000"
    );
    assert.strictEqual(
      res.headers["access-control-allow-methods"],
      "GET,HEAD,PUT,PATCH,POST,DELETE"
    );
    assert.strictEqual(
      res.headers["access-control-allow-headers"],
      "Content-Type, Authorization"
    );
  });
});

// These tests verify that each CorsOptions field is actually reflected in the
// preflight (OPTIONS) and regular responses
describe("CORS preflight option fidelity", function () {
  let server: Cpeak;
  const PORT2 = 7544;
  const request2 = supertest(`http://localhost:${PORT2}`);

  before(function (done) {
    server = cpeak();

    server.beforeEach(
      cors({
        origin: "http://localhost:3000",
        methods: ["GET"],
        allowedHeaders: ["X-Custom-Token"],
        exposedHeaders: ["X-Request-Id"],
        maxAge: 30
      })
    );

    server.route("get", "/test", (req: CpeakRequest, res: CpeakResponse) => {
      res.setHeader("X-Request-Id", "abc123");
      res.status(200).json({});
    });

    server.listen(PORT2, done);
  });

  after(function (done) {
    server.close(done);
  });

  it("preflight Access-Control-Allow-Methods reflects the methods option", async function () {
    const res = await request2
      .options("/test")
      .set("Origin", "http://localhost:3000")
      .set("Access-Control-Request-Method", "POST");

    assert.strictEqual(res.status, 204);
    assert.strictEqual(res.headers["access-control-allow-methods"], "GET");
  });

  it("preflight Access-Control-Allow-Headers reflects the allowedHeaders option", async function () {
    const res = await request2
      .options("/test")
      .set("Origin", "http://localhost:3000")
      .set("Access-Control-Request-Method", "GET")
      .set("Access-Control-Request-Headers", "X-Custom-Token");

    assert.strictEqual(res.status, 204);
    assert.strictEqual(
      res.headers["access-control-allow-headers"],
      "X-Custom-Token"
    );
  });

  it("preflight Access-Control-Max-Age reflects the maxAge option", async function () {
    const res = await request2
      .options("/test")
      .set("Origin", "http://localhost:3000")
      .set("Access-Control-Request-Method", "GET");

    assert.strictEqual(res.status, 204);
    assert.strictEqual(res.headers["access-control-max-age"], "30");
  });

  it("response Access-Control-Expose-Headers reflects the exposedHeaders option", async function () {
    const res = await request2
      .get("/test")
      .set("Origin", "http://localhost:3000");

    assert.strictEqual(res.status, 200);
    assert.strictEqual(
      res.headers["access-control-expose-headers"],
      "X-Request-Id"
    );
  });
});
