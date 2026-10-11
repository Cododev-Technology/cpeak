import assert from "node:assert";
import supertest from "supertest";
import cpeak from "../lib/";

import type { Cpeak, CpeakRequest, CpeakResponse } from "../lib/types";

const PORT = 7543;
const request = supertest(`http://localhost:${PORT}`);

describe("Redirecting to a new URL with res.redirect function", function () {
  let server: Cpeak;

  before(function (done) {
    server = cpeak();

    server.route(
      "get",
      "/old-route",
      (req: CpeakRequest, res: CpeakResponse) => {
        res.redirect("/new-route");
      }
    );

    server.listen(PORT, done);
  });

  after(function (done) {
    server.close(done);
  });

  it("should redirect to the new route", async function () {
    const res = await request.get("/old-route");
    assert.strictEqual(res.status, 302);
    assert.strictEqual(res.headers["location"], "/new-route");
  });
});
