import assert from "node:assert";
import supertest from "supertest";
import cpeak, {
  Cpeak,
  parseJSON,
  auth,
  hashPassword,
  verifyPassword,
  ErrorCode
} from "../lib/";
import type { CpeakRequest, CpeakResponse, Middleware } from "../lib/types";

const PORT = 7543;
const request = supertest(`http://localhost:${PORT}`);

describe("Authentication with the utility", function () {
  this.timeout(30000);

  let server: Cpeak;
  const tokens: Record<string, { userId: string; expiresAt: Date }> = {};
  const users: Array<{ id: string; username: string; password: string }> = [];

  before(function (done) {
    server = cpeak();
    server.beforeEach(parseJSON());

    const requireAuth: Middleware = async (req, res, next) => {
      const token = req.headers["authorization"];
      if (!token) throw { status: 401, message: "Unauthorized." };
      const result = await req.verifyToken(token);
      if (!result) throw { status: 401, message: "Unauthorized." };
      req.user = { id: result.userId };
      next();
    };

    server.beforeEach(
      auth({
        secret: "Soeofjeowjgogru9876%TRFYGHVJBH##",
        saveToken: async (tokenId, userId, expiresAt) => {
          tokens[tokenId] = { userId, expiresAt };
        },
        findToken: async (tokenId) => {
          const row = tokens[tokenId];
          return row ? { userId: row.userId, expiresAt: row.expiresAt } : null;
        },
        revokeToken: async (tokenId) => {
          delete tokens[tokenId];
        },
        iterations: 250_000,
        keylen: 128,
        digest: "sha256",
        saltSize: 64,
        hmacAlgorithm: "sha256",
        tokenIdSize: 40,
        tokenExpiry: 3 * 24 * 60 * 60 * 1000
      })
    );

    server.route(
      "post",
      "/register",
      async (req: CpeakRequest, res: CpeakResponse) => {
        const { username, password } = req.body;
        const hash = await req.hashPassword({ password });
        const user = { id: String(users.length + 1), username, password: hash };
        users.push(user);
        const token = await req.login({
          password,
          hashedPassword: hash,
          userId: user.id
        });
        res.status(201).json({ token });
      }
    );

    server.route(
      "post",
      "/login",
      async (req: CpeakRequest, res: CpeakResponse) => {
        const { username, password } = req.body;
        const user = users.find((u) => u.username === username);
        if (!user)
          throw { status: 401, message: "Invalid username or password" };
        const token = await req.login({
          password,
          hashedPassword: user.password,
          userId: user.id
        });
        if (!token)
          throw { status: 401, message: "Invalid username or password" };
        res.json({ token });
      }
    );

    server.route(
      "get",
      "/profile",
      requireAuth,
      async (req: CpeakRequest, res: CpeakResponse) => {
        res.json({ user: req.user });
      }
    );

    server.route(
      "delete",
      "/logout",
      requireAuth,
      async (req: CpeakRequest, res: CpeakResponse) => {
        const token = req.headers["authorization"];
        if (token) await req.logout(token);
        res.status(200).json({ message: "logged out" });
      }
    );

    server.handleErr((err: any, _req: CpeakRequest, res: CpeakResponse) => {
      res
        .status(err?.status || 500)
        .json({ error: err?.message || "Internal server error" });
    });

    server.listen(PORT, done);
  });

  after(function (done) {
    server.close(done);
  });

  // ─── hashPassword() ──────────────────────────────────────────────────────────

  describe("hashPassword()", function () {
    it("returns a string in pbkdf2:iter:keylen:digest:saltHex:hashHex format", async function () {
      const hash = await hashPassword("mypassword", { iterations: 1000 });
      const parts = hash.split(":");
      assert.strictEqual(parts.length, 6);
      assert.strictEqual(parts[0], "pbkdf2");
    });

    it("embeds the correct custom options in the hash string", async function () {
      const hash = await hashPassword("mypassword", {
        iterations: 1000,
        keylen: 32,
        digest: "sha256"
      });
      const parts = hash.split(":");
      assert.strictEqual(parts[1], "1000");
      assert.strictEqual(parts[2], "32");
      assert.strictEqual(parts[3], "sha256");
    });

    it("produces different hashes for the same password due to random salt", async function () {
      const hash1 = await hashPassword("mypassword", { iterations: 1000 });
      const hash2 = await hashPassword("mypassword", { iterations: 1000 });
      assert.notStrictEqual(hash1, hash2);
    });

    it("resulting hash verifies correctly with verifyPassword()", async function () {
      const hash = await hashPassword("mypassword", { iterations: 1000 });
      assert.ok(await verifyPassword("mypassword", hash));
    });
  });

  // ─── verifyPassword() ────────────────────────────────────────────────────────

  describe("verifyPassword()", function () {
    it("returns true for the correct password", async function () {
      const hash = await hashPassword("correctpassword", { iterations: 1000 });
      assert.strictEqual(await verifyPassword("correctpassword", hash), true);
    });

    it("returns false for the wrong password", async function () {
      const hash = await hashPassword("correctpassword", { iterations: 1000 });
      assert.strictEqual(await verifyPassword("wrongpassword", hash), false);
    });
  });

  // ─── auth() initialization ───────────────────────────────────────────────────

  describe("auth() initialization", function () {
    const minimalOptions = {
      saveToken: async () => {},
      findToken: async () => null
    };

    it("throws WEAK_SECRET when secret is missing", function () {
      assert.throws(
        () => auth({ ...minimalOptions, secret: undefined as any }),
        (err: any) => err.code === ErrorCode.WEAK_SECRET
      );
    });

    it("throws WEAK_SECRET when secret is 31 characters", function () {
      assert.throws(
        () => auth({ ...minimalOptions, secret: "a".repeat(31) }),
        (err: any) => err.code === ErrorCode.WEAK_SECRET
      );
    });

    it("does not throw when secret is exactly 32 characters", function () {
      const result = auth({ ...minimalOptions, secret: "a".repeat(32) });
      assert.strictEqual(typeof result, "function");
    });
  });

  // ─── req.logout presence ─────────────────────────────────────────────────────

  describe("req.logout presence", function () {
    it("is undefined when revokeToken is not provided", function () {
      const middleware = auth({
        secret: "a".repeat(32),
        saveToken: async () => {},
        findToken: async () => null
      });
      const req: any = {};
      middleware(req, {} as any, () => {});
      assert.strictEqual(req.logout, undefined);
    });

    it("is a function when revokeToken is provided", function () {
      const middleware = auth({
        secret: "a".repeat(32),
        saveToken: async () => {},
        findToken: async () => null,
        revokeToken: async () => {}
      });
      const req: any = {};
      middleware(req, {} as any, () => {});
      assert.strictEqual(typeof req.logout, "function");
    });
  });

  // ─── POST /register ──────────────────────────────────────────────────────────

  describe("POST /register", function () {
    it("returns 201 with a signed token in tokenId.signature format", async function () {
      const res = await request
        .post("/register")
        .send({ username: "alice", password: "password123" });
      assert.strictEqual(res.status, 201);
      assert.strictEqual(typeof res.body.token, "string");
      assert.ok(res.body.token.length > 0);
      assert.strictEqual(res.body.token.split(".").length, 2);
    });

    it("produces unique tokens for different registrations", async function () {
      const res1 = await request
        .post("/register")
        .send({ username: "bob", password: "bobpass" });
      const res2 = await request
        .post("/register")
        .send({ username: "carol", password: "carolpass" });
      assert.notStrictEqual(res1.body.token, res2.body.token);
    });
  });

  // ─── POST /login ─────────────────────────────────────────────────────────────

  describe("POST /login", function () {
    before(async function () {
      await request
        .post("/register")
        .send({ username: "dave", password: "davepass" });
    });

    it("returns 200 with a signed token for correct credentials", async function () {
      const res = await request
        .post("/login")
        .send({ username: "dave", password: "davepass" });
      assert.strictEqual(res.status, 200);
      assert.strictEqual(typeof res.body.token, "string");
      assert.strictEqual(res.body.token.split(".").length, 2);
    });

    it("returns 401 for wrong password", async function () {
      const res = await request
        .post("/login")
        .send({ username: "dave", password: "wrongpass" });
      assert.strictEqual(res.status, 401);
    });
  });

  // ─── GET /profile (verifyToken) ──────────────────────────────────────────────

  describe("GET /profile (verifyToken)", function () {
    let validToken: string;
    let validUserId: string;

    before(async function () {
      const res = await request
        .post("/register")
        .send({ username: "eve", password: "evepass" });
      validToken = res.body.token;
      const profile = await request
        .get("/profile")
        .set("authorization", validToken);
      validUserId = profile.body.user.id;
    });

    it("allows access and returns the correct userId with a valid token", async function () {
      const res = await request
        .get("/profile")
        .set("authorization", validToken);
      assert.strictEqual(res.status, 200);
      assert.strictEqual(res.body.user.id, validUserId);
    });

    it("returns 401 when no Authorization header is present", async function () {
      const res = await request.get("/profile");
      assert.strictEqual(res.status, 401);
    });

    it("returns 401 for a token without a dot separator", async function () {
      const res = await request
        .get("/profile")
        .set("authorization", "badtokennodot");
      assert.strictEqual(res.status, 401);
    });

    it("returns 401 for a tampered token (modified signature)", async function () {
      const tampered = validToken.slice(0, -4) + "0000";
      const res = await request.get("/profile").set("authorization", tampered);
      assert.strictEqual(res.status, 401);
    });

    it("returns 401 when the token has been removed from the store", async function () {
      const regRes = await request
        .post("/register")
        .send({ username: "frank", password: "frankpass" });
      const token = regRes.body.token;
      const profile = await request.get("/profile").set("authorization", token);
      const frankUserId = profile.body.user.id;

      const frankTokenId = Object.keys(tokens).find(
        (id) => tokens[id].userId === frankUserId
      );
      assert.ok(
        frankTokenId,
        "frank's token should exist in the store before deletion"
      );
      delete tokens[frankTokenId];

      const res = await request.get("/profile").set("authorization", token);
      assert.strictEqual(res.status, 401);
    });

    it("returns 401 when the token has expired", async function () {
      const regRes = await request
        .post("/register")
        .send({ username: "grace", password: "gracepass" });
      const token = regRes.body.token;
      const profile = await request.get("/profile").set("authorization", token);
      const graceUserId = profile.body.user.id;

      const graceTokenId = Object.keys(tokens).find(
        (id) => tokens[id].userId === graceUserId
      );
      assert.ok(
        graceTokenId,
        "grace's token should exist in the store before expiry"
      );
      tokens[graceTokenId].expiresAt = new Date(Date.now() - 1000);

      const res = await request.get("/profile").set("authorization", token);
      assert.strictEqual(res.status, 401);
    });
  });

  // ─── DELETE /logout ──────────────────────────────────────────────────────────

  describe("DELETE /logout", function () {
    let logoutToken: string;

    before(async function () {
      const res = await request
        .post("/register")
        .send({ username: "henry", password: "henrypass" });
      logoutToken = res.body.token;
    });

    it("returns 200 on successful logout", async function () {
      const res = await request
        .delete("/logout")
        .set("authorization", logoutToken);
      assert.strictEqual(res.status, 200);
      assert.deepStrictEqual(res.body, { message: "logged out" });
    });

    it("subsequent requests with the revoked token return 401", async function () {
      const res = await request
        .get("/profile")
        .set("authorization", logoutToken);
      assert.strictEqual(res.status, 401);
    });

    it("returns 401 when no Authorization header is provided", async function () {
      const res = await request.delete("/logout");
      assert.strictEqual(res.status, 401);
    });

    it("returns 401 when a tampered token is provided", async function () {
      const regRes = await request
        .post("/register")
        .send({ username: "irene", password: "irenepass" });
      const tampered = regRes.body.token.slice(0, -4) + "0000";
      const res = await request
        .delete("/logout")
        .set("authorization", tampered);
      assert.strictEqual(res.status, 401);
    });
  });
});
