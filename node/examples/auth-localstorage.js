import cpeak, { parseJSON, cookieParser, auth } from "cpeak";

const app = cpeak();

// In-memory storage for tokens and users. In a real application, you would use a database for them.
const tokens = {};
const users = [];

app.beforeEach(parseJSON());
app.beforeEach(cookieParser());

// This middleware will check for the token and set the user if the token is valid, but it will throw an error
// if the token is missing or invalid.
export const requireAuth = async (req, res, next) => {
  const token = req.headers["authorization"];
  if (!token) throw { status: 401, message: "Unauthorized." };

  const result = await req.verifyToken(token);
  if (!result) throw { status: 401, message: "Unauthorized." };

  req.user = { id: result.userId };
  next();
};

// This middleware will check for the token and set the user if the token is valid, but it won't throw an error if the token is missing or invalid.
// This can be useful for routes that can be accessed by both authenticated and unauthenticated users.
export const optionalAuth = async (req, _res, next) => {
  const token = req.headers["authorization"];
  if (token) {
    const result = await req.verifyToken(token);
    if (result) req.user = { id: result.userId };
  }
  next();
};

app.beforeEach(
  auth({
    secret: "Soeofjeowjgogru9876%TRFYGHVJBH",
    saveToken: async (tokenId, userId, expiresAt) => {
      tokens[tokenId] = { userId, expiresAt };
    },
    findToken: async (tokenId) => {
      const row = tokens[tokenId];
      return row ? { userId: row.userId, expiresAt: row.expiresAt } : null;
    },
    revokeToken: async (tokenId) => {
      // This revokes the specific token for the current session. This can be useful for logout from a specific device,
      // for example, user logs out from their phone but they want to stay logged in on their laptop.
      delete tokens[tokenId];

      // If you want to revoke all tokens for a user upon logout, look at the logout route below for an example.
    },

    /** For finer control you can specify all these below, but all are optional and have sensible defaults: */
    // Password hashing (PBKDF2)
    iterations: 210_000, // Number of PBKDF2 iterations — higher = slower brute-force
    keylen: 64, // Derived key length in bytes
    digest: "sha512", // Hash algorithm used by PBKDF2
    saltSize: 32, // Random salt length in bytes

    // Token signing
    hmacAlgorithm: "sha256", // Algorithm used to sign/verify token IDs
    tokenIdSize: 20, // Random token ID length in bytes before signing
    tokenExpiry: 7 * 24 * 60 * 60 * 1000 // Token lifetime in ms (default: 7 days)
  })
);

app.route("post", "/register", async (req, res) => {
  const { username, password } = req.body;

  const hash = await req.hashPassword({ password });

  const user = { id: String(users.length + 1), username, password: hash };
  users.push(user);

  const token = await req.login({
    password,
    hashedPassword: hash,
    userId: String(user.id)
  });
  return res.status(201).json({ token });
});

app.route("post", "/login", async (req, res) => {
  const { username, password } = req.body;

  const user = users.find((u) => u.username === username);
  if (!user) {
    throw { status: 401, message: "Invalid username or password" };
  }

  // The client will then save this token and send it in the Authorization header for subsequent requests to protected routes.
  // You can optionally set this as a cookie as well. See the auth-cookies.js example.
  const token = await req.login({
    password,
    hashedPassword: user.password,
    userId: String(user.id)
  });
  return res.json({ token });
});

app.route("get", "/profile", requireAuth, async (req, res) => {
  return res.json({ user: req.user });
});

app.route("delete", "/logout", requireAuth, async (req, res) => {
  const token = req.headers["authorization"];
  if (token) await req.logout(token);
  return res.status(200).json({ message: "logged out" });

  // If you want to revoke all tokens for a user, you can do it like this if using SQL:
  // DELETE FROM tokens WHERE user_id = $1;
});

// Global error handler
app.handleErr((error, req, res) => {
  if (error && error.status) {
    return res.status(error.status).json({ error: error.message });
  } else {
    console.error(error);
    return res.status(500).json({
      error: "Sorry, something unexpected happened from our side."
    });
  }
});

app.listen(9000, () => {
  console.log("Server has started on port 9000");
});
