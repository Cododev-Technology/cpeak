import assert from "node:assert";
import supertest from "supertest";
import fs from "node:fs/promises";
import cpeak, { serveStatic } from "../lib/";
import type { Cpeak } from "../lib/types";

const PORT = 7543;
const request = supertest(`http://localhost:${PORT}`);

describe("Serving static files with serveStatic", function () {
  let server: Cpeak;

  before(function (done) {
    server = cpeak({ mimeTypes: { m4a: "audio/mp4" } });

    server.beforeEach(serveStatic("./test/files"));

    server.listen(PORT, done);
  });

  after(function (done) {
    server.close(done);
  });

  it("should return the correct file with the correct MIME type", async function () {
    const textRes = await request.get("/test.txt");
    const cssRes = await request.get("/styles.css");

    const fileTextContent = await fs.readFile("./test/files/test.txt", "utf-8");
    const fileCssContent = await fs.readFile(
      "./test/files/styles.css",
      "utf-8"
    );

    assert.strictEqual(textRes.status, 200);
    assert.strictEqual(textRes.headers["content-type"], "text/plain");
    assert.strictEqual(textRes.text, fileTextContent);

    assert.strictEqual(cssRes.status, 200);
    assert.strictEqual(cssRes.headers["content-type"], "text/css");
    assert.strictEqual(cssRes.text, fileCssContent);
  });

  it("should return the correct file with the specified MIME type by the developer", async function () {
    const res = await request.get("/audio.m4a");

    // read the file as binary
    const fileBuffer = await fs.readFile("./test/files/audio.m4a");

    assert.strictEqual(res.status, 200);
    assert.strictEqual(res.headers["content-type"], "audio/mp4");
    assert.deepStrictEqual(res.body, fileBuffer);
  });
});
