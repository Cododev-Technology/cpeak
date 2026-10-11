import assert from "node:assert";
import supertest from "supertest";
import cpeak, { Cpeak, parseJSON } from "../lib/";

import type { CpeakRequest, CpeakResponse } from "../lib/types";

const PORT = 7543;
const request = supertest(`http://localhost:${PORT}`);

describe("Parsing request bodies with parseJSON", function () {
  let server: Cpeak;

  before(function (done) {
    server = cpeak();

    server.beforeEach(parseJSON());

    server.route(
      "post",
      "/do-something",
      (req: CpeakRequest, res: CpeakResponse) => {
        res.status(205).json({ receivedData: req.body });
      }
    );

    server.listen(PORT, done);
  });

  after(function (done) {
    server.close(done);
  });

  it("should return the same data that was sent in request body as JSON", async function () {
    const obj = {
      key1: "value1",
      key2: 42,
      key3: {
        nestedKey1: "nestedValue1",
        nestedKey2: ["arrayValue1", "arrayValue2", 1000]
      },
      key4: true
    };

    const res = await request.post("/do-something").send(obj);

    assert.strictEqual(res.status, 205);
    assert.deepStrictEqual(res.body.receivedData, obj);
  });
});
