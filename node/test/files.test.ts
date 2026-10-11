import assert from "node:assert";
import supertest from "supertest";
import fs from "node:fs/promises";
import cpeak from "../lib/";

import type { Cpeak, CpeakRequest, CpeakResponse } from "../lib/types";

const PORT = 7543;
const request = supertest(`http://localhost:${PORT}`);

describe("Returning files with sendFile", function () {
  let server: Cpeak;

  before(function (done) {
    server = cpeak();

    server.route("get", "/file", (req: CpeakRequest, res: CpeakResponse) => {
      res.status(200).sendFile("./test/files/test.txt", "text/plain");
    });

    server.route(
      "get",
      "/file-inferred",
      (req: CpeakRequest, res: CpeakResponse) => {
        res.status(200).sendFile("./test/files/test.txt");
      }
    );

    server.handleErr((error: any, req: CpeakRequest, res: CpeakResponse) => {
      res.status(500).json({ code: error?.code, message: error?.message });
    });

    server.listen(PORT, done);
  });

  after(function (done) {
    server.close(done);
  });

  it("should get a file as the response with the correct MIME type", async function () {
    const res = await request.get("/file");

    const fileContent = await fs.readFile("./test/files/test.txt", "utf-8");

    assert.strictEqual(res.status, 200);
    assert.strictEqual(res.headers["content-type"], "text/plain");
    assert.strictEqual(res.text, fileContent);
  });

  it("should infer the MIME type from the file extension when omitted", async function () {
    const res = await request.get("/file-inferred");

    const fileContent = await fs.readFile("./test/files/test.txt", "utf-8");

    assert.strictEqual(res.status, 200);
    assert.strictEqual(res.headers["content-type"], "text/plain");
    assert.strictEqual(res.text, fileContent);
  });
});
