export interface PbkdfOptions {
  iterations?: number;
  keylen?: number;
  digest?: string;
  saltSize?: number;
}

export interface AuthOptions extends PbkdfOptions {
  secret: string;
  saveToken: (
    tokenId: string,
    userId: string,
    expiresAt: Date
  ) => Promise<void>;
  findToken: (
    tokenId: string
  ) => Promise<{ userId: string; expiresAt: Date } | null>;
  tokenExpiry?: number;
  hmacAlgorithm?: string;
  tokenIdSize?: number;
  revokeToken?: (tokenId: string) => Promise<void>;
}

export interface CookieOptions {
  signed?: boolean;
  httpOnly?: boolean;
  secure?: boolean;
  sameSite?: "strict" | "lax" | "none";
  maxAge?: number; // ms
  expires?: Date;
  path?: string;
  domain?: string;
}

export type OriginInput =
  | string
  | string[]
  | RegExp
  | boolean
  | ((origin: string | undefined) => boolean | Promise<boolean>);

export interface CorsOptions {
  origin?: OriginInput;
  methods?: string | string[];
  allowedHeaders?: string | string[];
  exposedHeaders?: string | string[];
  credentials?: boolean;
  maxAge?: number;
  preflightContinue?: boolean;
  optionsSuccessStatus?: number;
}
