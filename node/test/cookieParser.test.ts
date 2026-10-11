import assert from "node:assert";
import supertest from "supertest";
import cpeak, { Cpeak, cookieParser } from "../lib/";

import type { CpeakRequest, CpeakResponse } from "../lib/types";

const PORT = 7543;
const request = supertest(`http://localhost:${PORT}`);

describe("Parsing cookies with cookieParser", function () {
  let server: Cpeak;

  before(function (done) {
    server = cpeak();

    server.beforeEach(cookieParser());

    server.route(
      "get",
      "/get-cookies",
      (req: CpeakRequest, res: CpeakResponse) => {
        res.status(200).json({ receivedCookies: req.cookies });
      }
    );

    server.listen(PORT, done);
  });

  after(function (done) {
    server.close(done);
  });

  it("should handle Basic cookies", async function () {
    const res = await request
      .get("/get-cookies")
      .set("Cookie", "foo=bar; baz=qux");
    assert.deepStrictEqual(res.body.receivedCookies, {
      foo: "bar",
      baz: "qux"
    });
  });

  it("should handle Quoted values (stripping double quotes)", async function () {
    const res = await request
      .get("/get-cookies")
      .set("Cookie", 'id="12345"; user=jane');
    assert.deepStrictEqual(res.body.receivedCookies, {
      id: "12345",
      user: "jane"
    });
  });

  it("should handle Encoded values (UTF-8/URL decoding)", async function () {
    const res = await request
      .get("/get-cookies")
      .set("Cookie", "city=New%20York; fav=%E2%9C%93");
    assert.deepStrictEqual(res.body.receivedCookies, {
      city: "New York",
      fav: "✓"
    });
  });

  it("should handle Malformed percent-encoding gracefully", async function () {
    const res = await request
      .get("/get-cookies")
      .set("Cookie", "bad=%zz; good=yes");
    // Should return raw string if decoding fails to avoid crashing
    assert.deepStrictEqual(res.body.receivedCookies, {
      bad: "%zz",
      good: "yes"
    });
  });

  it("should handle extreme Whitespace and empty pairs", async function () {
    const res = await request
      .get("/get-cookies")
      .set("Cookie", "  space = cadet ; ");
    assert.deepStrictEqual(res.body.receivedCookies, { space: "cadet" });
  });

  it("should handle Valueless cookies", async function () {
    const res = await request
      .get("/get-cookies")
      .set("Cookie", "flag=; session=1");
    assert.deepStrictEqual(res.body.receivedCookies, {
      flag: "",
      session: "1"
    });
  });

  it("should handle Duplicates by honoring the FIRST occurrence", async function () {
    // RFC 6265: Servers SHOULD use the first appearance
    const res = await request
      .get("/get-cookies")
      .set("Cookie", "pref=light; pref=dark");
    assert.deepStrictEqual(res.body.receivedCookies, { pref: "light" });
  });

  it("should treat a lone double-quote as a literal value, not strip it to empty string", async function () {
    const res = await request.get("/get-cookies").set("Cookie", 'x="');
    assert.deepStrictEqual(res.body.receivedCookies, { x: '"' });
  });

  it("should not silently drop a cookie whose name is __proto__", async function () {
    const res = await request
      .get("/get-cookies")
      .set("Cookie", "__proto__=secret; legit=yes");
    assert.strictEqual(res.body.receivedCookies["legit"], "yes");
    assert.strictEqual(res.body.receivedCookies["__proto__"], "secret");
  });

  it("should not drop cookies whose names match Object.prototype methods", async function () {
    const res = await request
      .get("/get-cookies")
      .set(
        "Cookie",
        "toString=a; hasOwnProperty=b; valueOf=c; constructor=d; legit=ok"
      );
    assert.strictEqual(res.body.receivedCookies.legit, "ok");
    assert.strictEqual(res.body.receivedCookies.toString, "a");
    assert.strictEqual(res.body.receivedCookies.hasOwnProperty, "b");
    assert.strictEqual(res.body.receivedCookies.valueOf, "c");
    assert.strictEqual(res.body.receivedCookies.constructor, "d");
  });

  it("should treat cookie names as case-sensitive", async function () {
    const res = await request
      .get("/get-cookies")
      .set("Cookie", "Foo=upper; foo=lower");
    assert.deepStrictEqual(res.body.receivedCookies, {
      Foo: "upper",
      foo: "lower"
    });
  });

  it("should preserve '=' inside cookie values (base64 / JWT padding)", async function () {
    const res = await request
      .get("/get-cookies")
      .set("Cookie", "token=YWJjPT0=; jwt=eyJhbGciOiJIUzI1NiJ9.payload.sig=");
    assert.deepStrictEqual(res.body.receivedCookies, {
      token: "YWJjPT0=",
      jwt: "eyJhbGciOiJIUzI1NiJ9.payload.sig="
    });
  });

  it("should ignore pairs that lack an '=' character", async function () {
    const res = await request
      .get("/get-cookies")
      .set("Cookie", "noequalshere; real=ok; alsoBare");
    assert.deepStrictEqual(res.body.receivedCookies, { real: "ok" });
  });

  it("should preserve '+' as a literal character (not decode it to space)", async function () {
    const res = await request
      .get("/get-cookies")
      .set("Cookie", "q=a+b+c; mixed=hello+world%20foo");
    assert.deepStrictEqual(res.body.receivedCookies, {
      q: "a+b+c",
      mixed: "hello+world foo"
    });
  });

  it("should tolerate multiple consecutive ';' separators", async function () {
    const res = await request
      .get("/get-cookies")
      .set("Cookie", ";;a=1;;;b=2;;");
    assert.deepStrictEqual(res.body.receivedCookies, { a: "1", b: "2" });
  });
});

describe("Setting cookies with cookieParser", function () {
  let server2: Cpeak;
  const SECRET = "test-secret-cpeak-cookie-signing!!";
  const PORT2 = 7544;
  const request2 = supertest(`http://localhost:${PORT2}`);

  before(function (done) {
    server2 = cpeak();
    server2.beforeEach(cookieParser({ secret: SECRET }));

    server2.route(
      "get",
      "/set-cookie",
      (req: CpeakRequest, res: CpeakResponse) => {
        res
          .cookie("session", "abc123", {
            domain: "example.com",
            maxAge: 86400000,
            expires: new Date("2026-12-31T00:00:00.000Z"),
            httpOnly: true,
            secure: true,
            sameSite: "Strict",
            path: "/dashboard"
          })
          .cookie("token", "secret-val", {
            signed: true,
            httpOnly: true,
            path: "/"
          })
          .status(200)
          .json({ ok: true });
      }
    );

    server2.listen(PORT2, done);
  });

  after(function (done) {
    server2.close(done);
  });

  it("should set a plain cookie with all options and a signed cookie", async function () {
    const res = await request2.get("/set-cookie");
    const headers = res.headers["set-cookie"] as any;

    // Plain cookie: all attributes present
    assert.ok(headers[0].startsWith("session=abc123"));
    assert.ok(headers[0].includes("Path=/dashboard"));
    assert.ok(headers[0].includes("Domain=example.com"));
    assert.ok(headers[0].includes("Max-Age=86400"));
    assert.ok(headers[0].includes("Expires=Thu, 31 Dec 2026 00:00:00 GMT"));
    assert.ok(headers[0].includes("HttpOnly"));
    assert.ok(headers[0].includes("Secure"));
    assert.ok(headers[0].includes("SameSite=Strict"));

    // Signed cookie: value is s:<original>.<hmac> URL-encoded, attributes intact
    assert.ok(headers[1].startsWith("token=s%3Asecret-val."));
    assert.ok(headers[1].includes("HttpOnly"));
    assert.ok(headers[1].includes("Path=/"));
  });
});
