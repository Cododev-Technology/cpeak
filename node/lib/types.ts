import { IncomingMessage, ServerResponse, type Server } from "node:http";
import type { Readable } from "node:stream";
import type { Buffer } from "node:buffer";
import type { CompressionOptions } from "./internal/types";
import type { CookieOptions } from "./utils/types";
import type { CpeakIncomingMessage, CpeakServerResponse } from "./index";

export type { Cpeak } from "./index";

export type CpeakHttpServer = Server<
  typeof CpeakIncomingMessage,
  typeof CpeakServerResponse
>;

// For constructor options passed to `cpeak()`
export interface CpeakOptions {
  compression?: boolean | CompressionOptions;
  mimeTypes?: StringMap;
}

// Extending Node.js's Request and Response objects to add our custom properties
export type StringMap = Record<string, string>;

export interface CpeakRequest<
  ReqBody = any,
  ReqQueries = any
> extends IncomingMessage {
  params: StringMap;
  query: ReqQueries;
  body?: ReqBody;
  cookies?: StringMap;
  signedCookies?: Record<string, string | false>;
  [key: string]: any; // allow developers to add their onw extensions (e.g. req.test)
}

export interface CpeakResponse<ResBody = any> extends ServerResponse {
  sendFile: (path: string, mime?: string) => Promise<void>;
  status: (code: number) => CpeakResponse<ResBody>;
  attachment: (filename?: string) => CpeakResponse<ResBody>;
  cookie: (
    name: string,
    value: string,
    options?: CookieOptions
  ) => CpeakResponse<ResBody>;
  redirect: (location: string) => void;
  json: (data: ResBody) => Promise<void>;
  compress: (
    mime: string,
    body: Buffer | string | Readable,
    size?: number
  ) => Promise<void>;
  render: (
    filePath: string,
    data: Record<string, unknown>,
    mime?: string
  ) => Promise<void>;
  [key: string]: any; // allow developers to add their onw extensions (e.g. res.test)
}

export type Next = (err?: any) => void;

// beforeEach middleware: (req, res, next)
export type Middleware<ReqBody = any, ReqParams = any> = (
  req: CpeakRequest<ReqBody, ReqParams>,
  res: CpeakResponse,
  next: Next
) => unknown;

// Route middleware: (req, res, next)
export type RouteMiddleware<ReqBody = any, ReqParams = any> = (
  req: CpeakRequest<ReqBody, ReqParams>,
  res: CpeakResponse,
  next: Next
) => unknown;

// Route handlers: (req, res)
export type Handler<ReqBody = any, ReqParams = any, ResBody = any> = (
  req: CpeakRequest<ReqBody, ReqParams>,
  res: CpeakResponse<ResBody>
) => unknown;

// Represents a single registered route.
export interface Route {
  path: string;
  middleware: RouteMiddleware[];
  cb: Handler;
}
