import assert from "node:assert";
import supertest from "supertest";
import cpeak, { ErrorCode } from "../lib/";

import type { Cpeak, CpeakRequest, CpeakResponse } from "../lib/types";

const PORT = 7544;
const request = supertest(`http://localhost:${PORT}`);

describe("Fallback handler", function () {
  let server: Cpeak;

  before(function (done) {
    server = cpeak();

    server.route(
      "get",
      "/health-check",
      (req: CpeakRequest, res: CpeakResponse) => {
        res.status(200).json({ matched: "static" });
      }
    );

    // A GET-scoped wildcard so we can verify fallback only fires for methods the wildcard does not cover
    server.route("get", "*", (req: CpeakRequest, res: CpeakResponse) => {
      res.status(200).json({ matched: "wildcard" });
    });

    server.handleErr((error: any, req: CpeakRequest, res: CpeakResponse) => {
      res.status(500).json({ error: error.message });
    });

    server.fallback(async (req: CpeakRequest, res: CpeakResponse) => {
      if (req.url === "/boom") throw new Error("fallback boom");
      return res.status(418).json({ matched: "fallback", method: req.method });
    });

    server.listen(PORT, done);
  });

  after(function (done) {
    server.close(done);
  });

  it("should fire for unmatched requests across methods", async function () {
    const post = await request.post("/anything");
    assert.strictEqual(post.status, 418);
    assert.deepStrictEqual(post.body, { matched: "fallback", method: "POST" });

    const del = await request.delete("/x/y/z");
    assert.strictEqual(del.status, 418);
    assert.deepStrictEqual(del.body, { matched: "fallback", method: "DELETE" });
  });

  it("should not preempt static or wildcard route matches", async function () {
    const staticHit = await request.get("/health-check");
    assert.strictEqual(staticHit.status, 200);
    assert.deepStrictEqual(staticHit.body, { matched: "static" });

    const wildHit = await request.get("/anything");
    assert.strictEqual(wildHit.status, 200);
    assert.deepStrictEqual(wildHit.body, { matched: "wildcard" });
  });

  it("should route a thrown error to handleErr", async function () {
    const res = await request.post("/boom");
    assert.strictEqual(res.status, 500);
    assert.deepStrictEqual(res.body, { error: "fallback boom" });
  });

  it("should throw DUPLICATE_FALLBACK when registered twice", function () {
    const app = cpeak();
    const handler = (req: CpeakRequest, res: CpeakResponse) =>
      res.status(404).end();

    app.fallback(handler);

    let err: any;
    try {
      app.fallback(handler);
    } catch (e) {
      err = e;
    }

    assert.strictEqual(err?.code, ErrorCode.DUPLICATE_FALLBACK);
  });
});
