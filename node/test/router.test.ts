import assert from "node:assert";
import supertest from "supertest";
import cpeak, { ErrorCode } from "../lib/";

import type { Cpeak, CpeakRequest, CpeakResponse } from "../lib/types";

const PORT = 7543;
const request = supertest(`http://localhost:${PORT}`);

describe("Routing logic & URL & query parameters", function () {
  let server: Cpeak;

  before(function (done) {
    server = cpeak();

    server.route(
      "get",
      "/document/:title/more/:another/final",
      (req: CpeakRequest, res: CpeakResponse) => {
        const title = req.params?.title;
        const another = req.params?.another;
        const query = req.query;
        res.status(200).json({ title, another, query });
      }
    );

    // Static beats param at the same depth, regardless of registration order.
    server.route(
      "get",
      "/users/:id",
      (req: CpeakRequest, res: CpeakResponse) => {
        res.status(200).json({ matched: "param", id: req.params.id });
      }
    );
    server.route(
      "get",
      "/users/me",
      (req: CpeakRequest, res: CpeakResponse) => {
        res.status(200).json({ matched: "static-me" });
      }
    );

    // Static beats wildcard at the same depth.
    server.route("get", "/api/*", (req: CpeakRequest, res: CpeakResponse) => {
      res.status(200).json({ matched: "api-wildcard" });
    });
    server.route(
      "get",
      "/api/health",
      (req: CpeakRequest, res: CpeakResponse) => {
        res.status(200).json({ matched: "api-health" });
      }
    );

    // Static is preferred, but the param branch must still match on a dead end.
    server.route("get", "/a/b/c", (req: CpeakRequest, res: CpeakResponse) => {
      res.status(200).json({ matched: "abc-static" });
    });
    server.route("get", "/a/:x/d", (req: CpeakRequest, res: CpeakResponse) => {
      res.status(200).json({ matched: "axd-param", x: req.params.x });
    });

    // Same-position param names across different methods. We have a separate tree per method, so no conflict.
    server.route(
      "post",
      "/comments/:pageId",
      (req: CpeakRequest, res: CpeakResponse) => {
        res.status(200).json({ method: "post", params: req.params });
      }
    );
    server.route(
      "put",
      "/comments/:id",
      (req: CpeakRequest, res: CpeakResponse) => {
        res.status(200).json({ method: "put", params: req.params });
      }
    );

    // Same param slot at /lookup/:_, but the leaves diverge so each route gets
    // its own param name on the captured value.
    server.route(
      "get",
      "/lookup/:userId",
      (req: CpeakRequest, res: CpeakResponse) => {
        res.status(200).json({ matched: "lookup-user", params: req.params });
      }
    );
    server.route(
      "get",
      "/lookup/:slug/posts",
      (req: CpeakRequest, res: CpeakResponse) => {
        res
          .status(200)
          .json({ matched: "lookup-slug-posts", params: req.params });
      }
    );

    // Final fallback for any unmatched GET.
    server.route("get", "*", (req: CpeakRequest, res: CpeakResponse) => {
      res.status(200).json({ matched: "root-wildcard" });
    });

    server.listen(PORT, done);
  });

  after(function (done) {
    server.close(done);
  });

  it("should extract multiple URL params alongside the query string", async function () {
    const expected = {
      title: "some-title",
      another: "thisISsome__more-text",
      query: {
        filter: "comments-date",
        page: "2",
        sortBy: "date-desc",
        tags: JSON.stringify(["nodejs", "express", "url-params"]),
        author: JSON.stringify({ name: "John Doe", id: 123 }),
        isPublished: "true",
        metadata: JSON.stringify({ version: "1.0.0", language: "en" })
      }
    };

    const res = await request
      .get("/document/some-title/more/thisISsome__more-text/final")
      .query({
        filter: "comments-date",
        page: 2,
        sortBy: "date-desc",
        tags: JSON.stringify(["nodejs", "express", "url-params"]),
        author: JSON.stringify({ name: "John Doe", id: 123 }),
        isPublished: true,
        metadata: JSON.stringify({ version: "1.0.0", language: "en" })
      });

    assert.strictEqual(res.status, 200);
    assert.deepStrictEqual(res.body, expected);
  });

  it("should pick static over param at the same depth, fall to param otherwise, and decode encoded values", async function () {
    const staticHit = await request.get("/users/me");
    assert.strictEqual(staticHit.status, 200);
    assert.deepStrictEqual(staticHit.body, { matched: "static-me" });

    const paramHit = await request.get("/users/42");
    assert.strictEqual(paramHit.status, 200);
    assert.deepStrictEqual(paramHit.body, { matched: "param", id: "42" });

    // 'a b/c' encoded becomes 'a%20b%2Fc' which is a single segment since '/' is encoded.
    const encoded = await request.get("/users/" + encodeURIComponent("a b/c"));
    assert.strictEqual(encoded.status, 200);
    assert.deepStrictEqual(encoded.body, { matched: "param", id: "a b/c" });
  });

  it("should pick static over wildcard, and fall to wildcard for unclaimed paths", async function () {
    const staticHit = await request.get("/api/health");
    assert.strictEqual(staticHit.status, 200);
    assert.deepStrictEqual(staticHit.body, { matched: "api-health" });

    const wildHit = await request.get("/api/v1/users/123");
    assert.strictEqual(wildHit.status, 200);
    assert.deepStrictEqual(wildHit.body, { matched: "api-wildcard" });
  });

  it("should keep the static-only match, but backtrack into the param branch on a dead end", async function () {
    const exact = await request.get("/a/b/c");
    assert.strictEqual(exact.status, 200);
    assert.deepStrictEqual(exact.body, { matched: "abc-static" });

    const backtracked = await request.get("/a/b/d");
    assert.strictEqual(backtracked.status, 200);
    assert.deepStrictEqual(backtracked.body, { matched: "axd-param", x: "b" });
  });

  it("should fall through to the bare-* root wildcard for unmatched GETs", async function () {
    const res = await request.get("/nothing/registered/here");
    assert.strictEqual(res.status, 200);
    assert.deepStrictEqual(res.body, { matched: "root-wildcard" });
  });

  it("should return a 404 with body for a method that has no registered routes", async function () {
    const res = await request.patch("/random");
    assert.strictEqual(res.status, 404);
    assert.deepStrictEqual(res.body, { error: "Cannot PATCH /random" });
  });

  it("should allow different param names at the same position when the paths diverge", async function () {
    const single = await request.get("/lookup/abc");
    assert.strictEqual(single.status, 200);
    assert.deepStrictEqual(single.body, {
      matched: "lookup-user",
      params: { userId: "abc" }
    });

    const deeper = await request.get("/lookup/abc/posts");
    assert.strictEqual(deeper.status, 200);
    assert.deepStrictEqual(deeper.body, {
      matched: "lookup-slug-posts",
      params: { slug: "abc" }
    });
  });
});
