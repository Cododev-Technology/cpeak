import assert from "node:assert";
import supertest from "supertest";
import cpeak from "../lib/";

import type { Cpeak, CpeakRequest, CpeakResponse } from "../lib/types";

const PORT = 7543;
const request = supertest(`http://localhost:${PORT}`);

describe("Route middleware functions", function () {
  let server: Cpeak;

  before(function (done) {
    server = cpeak();

    const mid1 = (req: CpeakRequest, res: CpeakResponse, next: () => void) => {
      const value = req.query.value;

      if (value === "random")
        return res.status(400).json({ error: "an error msg" });

      next();
    };

    const mid2 = (req: CpeakRequest, res: CpeakResponse, next: () => void) => {
      req.foo = "text";
      next();
    };

    const mid3 = (req: CpeakRequest, res: CpeakResponse, next: () => void) => {
      res.unauthorized = () => {
        res.statusCode = 401;
        return res;
      };
      next();
    };

    server.route(
      "get",
      "/bar",
      mid1,
      mid2,
      (req: CpeakRequest, res: CpeakResponse) => {
        res.status(200).json({ message: req.foo });
      }
    );

    server.route(
      "get",
      "/bar-more",
      mid3,
      (req: CpeakRequest, res: CpeakResponse) => {
        res.unauthorized().json({});
      }
    );

    server.listen(PORT, done);
  });

  after(function (done) {
    server.close(done);
  });

  it("should modify the req object with a new property", async function () {
    const res = await request.get("/bar");
    assert.strictEqual(res.status, 200);
    assert.strictEqual(res.body.message, "text");
  });

  it("should modify the res object with a new method", async function () {
    const res = await request.get("/bar-more");
    assert.strictEqual(res.status, 401);
  });

  it("should exit the middleware and route chain if a middleware wants to", async function () {
    const res = await request.get("/bar?value=random");
    assert.strictEqual(res.status, 400);
    assert.strictEqual(res.body.message, undefined);
    assert.deepStrictEqual(res.body, { error: "an error msg" });
  });
});
