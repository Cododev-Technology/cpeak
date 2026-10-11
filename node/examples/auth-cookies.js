import cpeak, { parseJSON, cookieParser, auth } from "cpeak";

const app = cpeak();

// In-memory storage for tokens and users. In a real application, you would use a database for them.
const tokens = {};
const users = [];

app.beforeEach(parseJSON());
app.beforeEach(cookieParser());

export const requireAuth = async (req, res, next) => {
  const token = req.cookies.session;
  if (!token) throw { status: 401, message: "Unauthorized." };

  const result = await req.verifyToken(token);
  if (!result) throw { status: 401, message: "Unauthorized." };

  req.user = { id: result.userId };
  next();
};

export const optionalAuth = async (req, _res, next) => {
  const token = req.cookies.session;
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
      delete tokens[tokenId];
    }
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

  // The token is stored in an httpOnly cookie so it's never accessible via JavaScript, which protects against XSS attacks.
  res.cookie("session", token, {
    httpOnly: true,
    secure: true,
    sameSite: "lax"
  });
  return res.status(201).json({ ok: true });
});

app.route("post", "/login", async (req, res) => {
  const { username, password } = req.body;

  const user = users.find((u) => u.username === username);
  if (!user) {
    throw { status: 401, message: "Invalid username or password" };
  }

  const token = await req.login({
    password,
    hashedPassword: user.password,
    userId: String(user.id)
  });

  if (!token) {
    throw { status: 401, message: "Invalid username or password" };
  }

  // The token is stored in an httpOnly cookie so it's never accessible via JavaScript, which protects against XSS attacks.
  res.cookie("session", token, {
    httpOnly: true,
    secure: true,
    sameSite: "lax"
  });
  return res.json({ ok: true });
});

app.route("get", "/profile", requireAuth, async (req, res) => {
  return res.json({ user: req.user });
});

app.route("delete", "/logout", requireAuth, async (req, res) => {
  const token = req.cookies.session;
  if (token) await req.logout(token);

  // It is your responsibility to clear the token from the client side as well.
  res.clearCookie("session");
  return res.status(200).json({ message: "logged out" });
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

app.listen(9001, () => {
  console.log("Server has started on port 9001");
});
