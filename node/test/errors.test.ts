import assert from "node:assert";
import supertest from "supertest";

import cpeak from "../lib/";

import type { Cpeak, CpeakRequest, CpeakResponse } from "../lib/types";

const PORT = 7543;
const request = supertest(`http://localhost:${PORT}`);

describe("Error handling with handleErr", function () {
  let server: Cpeak;

  before(function (done) {
    server = cpeak();

    const mid1 = (req: CpeakRequest, res: CpeakResponse, next: () => void) => {
      const value = req.query.value;

      if (value === "random")
        throw { status: 401, message: "another error msg" };

      next();
    };

    server.route(
      "patch",
      "/foo/:bar",
      mid1,
      (req: CpeakRequest, res: CpeakResponse) => {
        const bar = req.params?.bar;

        if (bar === "random") {
          throw { status: 403, message: "an error msg" };
        }

        return res.status(200).json({ bar });
      }
    );

    // Returning an async response method's promise lets the framework
    // route the rejection to handleErr.
    server.route(
      "get",
      "/sendfile-returned",
      (req: CpeakRequest, res: CpeakResponse) => {
        return res.sendFile("./test/files/test.unknownext");
      }
    );

    server.handleErr((error: any, req: CpeakRequest, res: CpeakResponse) => {
      if (error?.code) {
        return res.status(500).json({ code: error.code });
      }
      return res.status(error.status).json({ error: error.message });
    });

    server.route(
      "get",
      "/handle-err-broken-json",
      (req: CpeakRequest, res: CpeakResponse) => {
        // Mimics res.json rejecting under compression (zlib failure, socket closed mid-stream, etc.)
        res.json = () => Promise.reject(new Error("simulated json rejection"));
        throw { status: 500, message: "trigger handleErr" };
      }
    );

    server.listen(PORT, done);
  });

  after(function (done) {
    server.close(done);
  });

  it("should route a thrown error from a handler to handleErr", async function () {
    const res = await request.patch("/foo/random");
    assert.strictEqual(res.status, 403);
    assert.deepStrictEqual(res.body, { error: "an error msg" });
  });

  it("should route a thrown error from a middleware to handleErr", async function () {
    const res = await request.patch("/foo/random?value=random");
    assert.strictEqual(res.status, 401);
    assert.deepStrictEqual(res.body, { error: "another error msg" });
  });

  it("should funnel async errors from a returned res.sendFile to handleErr", async function () {
    const res = await request.get("/sendfile-returned");
    assert.strictEqual(res.status, 500);
    assert.strictEqual(res.body.code, "CPEAK_ERR_MISSING_MIME");
  });

  it("should still respond with 500 when res.json inside handleErr rejects", async function () {
    const res = await request.get("/handle-err-broken-json");
    assert.strictEqual(res.status, 500);
  });
});
