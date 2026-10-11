import { randomBytes, pbkdf2, createHmac, timingSafeEqual } from "node:crypto";
import { promisify } from "node:util";
import type { Middleware } from "../types";
import { frameworkError, ErrorCode } from "../index";
import type { AuthOptions, PbkdfOptions } from "./types";

const pbkdf2Async = promisify(pbkdf2);

const DEFAULTS = {
  iterations: 210_000,
  keylen: 64,
  digest: "sha512",
  saltSize: 32,
  hmacAlgorithm: "sha256",
  tokenIdSize: 20,
  tokenExpiry: 7 * 24 * 60 * 60 * 1000 // 7 days in ms
} as const;

export async function hashPassword(
  password: string,
  options?: PbkdfOptions
): Promise<string> {
  const iterations = options?.iterations ?? DEFAULTS.iterations;
  const keylen = options?.keylen ?? DEFAULTS.keylen;
  const digest = options?.digest ?? DEFAULTS.digest;
  const saltSize = options?.saltSize ?? DEFAULTS.saltSize;
  const salt = randomBytes(saltSize);
  const hash = await pbkdf2Async(password, salt, iterations, keylen, digest);
  return `pbkdf2:${iterations}:${keylen}:${digest}:${salt.toString("hex")}:${hash.toString("hex")}`;
}

export async function verifyPassword(
  password: string,
  stored: string
): Promise<boolean> {
  // When argon2 is added, dispatch on the prefix here.
  const withoutPrefix = stored.slice(stored.indexOf(":") + 1);
  const parts = withoutPrefix.split(":");
  if (parts.length !== 5) return false;
  const [itersStr, keylenStr, digest, saltHex, hashHex] = parts;
  const iterations = parseInt(itersStr, 10);
  const keylen = parseInt(keylenStr, 10);
  if (!digest || !saltHex || !hashHex || isNaN(iterations) || isNaN(keylen))
    return false;
  const salt = Buffer.from(saltHex, "hex");
  const hash = await pbkdf2Async(password, salt, iterations, keylen, digest);
  const storedHash = Buffer.from(hashHex, "hex");
  if (storedHash.length !== hash.length) return false;
  return timingSafeEqual(hash, storedHash);
}

function signToken(tokenId: string, secret: string, algorithm: string): string {
  const sig = createHmac(algorithm, secret).update(tokenId).digest("hex");
  return `${tokenId}.${sig}`;
}

function extractTokenId(
  token: string,
  secret: string,
  algorithm: string
): string | null {
  const dot = token.indexOf(".");
  if (dot === -1) return null;
  const tokenId = token.slice(0, dot);
  const sig = token.slice(dot + 1);
  const expected = createHmac(algorithm, secret).update(tokenId).digest("hex");
  const expectedBuf = Buffer.from(expected, "hex");
  const actualBuf = Buffer.from(sig, "hex");
  if (expectedBuf.length !== actualBuf.length) return null;
  if (!timingSafeEqual(expectedBuf, actualBuf)) return null;
  return tokenId;
}

export function auth(options: AuthOptions): Middleware {
  if (!options.secret || options.secret.length < 32) {
    throw frameworkError(
      "Secret must be at least 32 characters. HMAC security is only as strong as the key.",
      auth,
      ErrorCode.WEAK_SECRET
    );
  }

  const {
    secret,
    saveToken,
    findToken,
    revokeToken,
    tokenExpiry = DEFAULTS.tokenExpiry,
    hmacAlgorithm = DEFAULTS.hmacAlgorithm,
    tokenIdSize = DEFAULTS.tokenIdSize
  } = options;

  const pbkdfOpts: PbkdfOptions = {
    iterations: options.iterations,
    keylen: options.keylen,
    digest: options.digest,
    saltSize: options.saltSize
  };

  const _hashPassword = ({ password }: { password: string }) =>
    hashPassword(password, pbkdfOpts);

  const login = async ({
    password,
    hashedPassword,
    userId
  }: {
    password: string;
    hashedPassword: string;
    userId: string;
  }): Promise<string | null> => {
    const isMatch = await verifyPassword(password, hashedPassword);
    if (!isMatch) return null;
    const tokenId = randomBytes(tokenIdSize).toString("hex");
    const token = signToken(tokenId, secret, hmacAlgorithm);
    await saveToken(tokenId, userId, new Date(Date.now() + tokenExpiry));
    return token;
  };

  const verifyToken = async (
    token: string
  ): Promise<{ userId: string } | null> => {
    if (!token) return null;
    const tokenId = extractTokenId(token, secret, hmacAlgorithm);
    if (!tokenId) return null;
    const record = await findToken(tokenId);
    if (!record) return null;
    if (new Date(record.expiresAt) < new Date()) return null;
    return { userId: record.userId };
  };

  const logout = revokeToken
    ? async (token: string): Promise<boolean> => {
        const tokenId = extractTokenId(token, secret, hmacAlgorithm);
        if (!tokenId) return false;
        await revokeToken(tokenId);
        return true;
      }
    : undefined;

  return (req, _res, next) => {
    req.hashPassword = _hashPassword;
    req.login = login;
    req.verifyToken = verifyToken;
    if (logout) req.logout = logout;
    next();
  };
}
