import http from "node:http";
import fs from "node:fs/promises";
import { createReadStream } from "node:fs";
import { pipeline } from "node:stream/promises";

import type net from "node:net";
import type { Readable } from "node:stream";
import type { Buffer } from "node:buffer";

import {
  resolveCompressionOptions,
  compressAndSend
} from "./internal/compression";
import { MIME_TYPES } from "./internal/mimeTypes";
import { Router, EMPTY_OBJECT } from "./internal/router";
import {
  frameworkError,
  ErrorCode,
  isClientDisconnect
} from "./internal/errors";

export { frameworkError, ErrorCode, isClientDisconnect };

import type {
  StringMap,
  CpeakHttpServer,
  CpeakOptions,
  CpeakRequest,
  CpeakResponse,
  Middleware,
  RouteMiddleware,
  Handler
} from "./types";

import type { ResolvedCompressionConfig } from "./internal/types";

export class CpeakIncomingMessage extends http.IncomingMessage {
  // We define body and params here for better V8 optimization (not changing the shape of the object at runtime)
  public body: any = undefined;
  // users shouldn't be able to mutate params.
  public params: StringMap = EMPTY_OBJECT;

  #query?: StringMap;

  // Parse the URL parameters (like /users?key1=value1&key2=value2)
  // We will call this query to be more familiar with other node.js frameworks.
  // This is a getter method (accessed like a property)
  get query(): StringMap {
    // This way if a developer writes req.query multiple times, we don't parse it multiple times
    if (this.#query) return this.#query;

    const url = this.url || "";
    const qIndex = url.indexOf("?");

    if (qIndex === -1) {
      this.#query = EMPTY_OBJECT;
    } else {
      const searchParams = new URLSearchParams(url.substring(qIndex + 1));
      this.#query = Object.fromEntries(searchParams.entries());
    }

    return this.#query;
  }
}

export class CpeakServerResponse extends http.ServerResponse<CpeakIncomingMessage> {
  // Set per-request from the Cpeak instance. Undefined when compression isn't enabled.
  _compression?: ResolvedCompressionConfig;

  // Send a file back to the client
  async sendFile(path: string, mime?: string) {
    if (this.headersSent) return;
    if (!mime) {
      const dotIndex = path.lastIndexOf(".");
      const fileExtension = dotIndex >= 0 ? path.slice(dotIndex + 1) : "";
      mime = MIME_TYPES[fileExtension];
      if (!mime) {
        throw frameworkError(
          `MIME type is missing for "${path}". Pass it as the second argument or register the extension via cpeak({ mimeTypes: { ${fileExtension || "ext"}: "..." } }).`,
          this.sendFile,
          ErrorCode.MISSING_MIME
        );
      }
    }

    try {
      const stat = await fs.stat(path);
      if (!stat.isFile()) {
        throw frameworkError(
          `Not a file: ${path}`,
          this.sendFile,
          ErrorCode.NOT_A_FILE
        );
      }

      if (this._compression) {
        await compressAndSend(
          this,
          mime,
          createReadStream(path),
          this._compression,
          stat.size
        );
        return;
      }

      this.setHeader("Content-Type", mime);
      this.setHeader("Content-Length", String(stat.size));

      // Properly propagate stream errors and respect backpressure
      await pipeline(createReadStream(path), this);
    } catch (err: any) {
      if (err?.code === "ENOENT") {
        throw frameworkError(
          `File not found: ${path}`,
          this.sendFile,
          ErrorCode.FILE_NOT_FOUND
        );
      }

      throw frameworkError(
        `Failed to send file: ${path}`,
        this.sendFile,
        ErrorCode.SEND_FILE_FAIL,
        undefined,
        isClientDisconnect(err)
      );
    }
  }

  // Set the status code of the response
  status(code: number) {
    this.statusCode = code;
    return this;
  }

  // Set the Content-Disposition header to prompt the user to download a file
  attachment(filename?: string) {
    const contentDisposition = filename
      ? `attachment; filename="${filename}"`
      : "attachment";
    this.setHeader("Content-Disposition", contentDisposition);
    return this;
  }

  // Redirects to a new URL
  redirect(location: string) {
    this.writeHead(302, { Location: location });
    this.end();
  }

  // Send a json data back to the client.
  // This is only good for bodies that their size is less than the highWaterMark value.
  json(data: any): Promise<void> {
    if (this.headersSent) return Promise.resolve();
    const body = JSON.stringify(data);
    if (this._compression) {
      return compressAndSend(this, "application/json", body, this._compression);
    }
    this.setHeader("Content-Type", "application/json");
    this.end(body);
    return Promise.resolve();
  }

  render(): Promise<void> {
    throw frameworkError(
      "render middleware not registered. Add render() via app.beforeEach(render()) to use res.render.",
      this.render,
      ErrorCode.RENDER_NOT_ENABLED
    );
  }

  // Explicit compression entry point. A developer can use this in any custom handler to compress arbitrary responses
  compress(
    mime: string,
    body: Buffer | string | Readable,
    size?: number
  ): Promise<void> {
    if (this.headersSent) return Promise.resolve();
    if (!this._compression) {
      throw frameworkError(
        "compression is not enabled. Pass `compression` to cpeak({ compression: true | { ... } }) to use res.compress.",
        this.compress,
        ErrorCode.COMPRESSION_NOT_ENABLED
      );
    }
    return compressAndSend(this, mime, body, this._compression, size);
  }
}

export class Cpeak {
  #server: CpeakHttpServer;
  #router: Router;
  #middleware: Middleware[];
  #handleErr?: (err: unknown, req: CpeakRequest, res: CpeakResponse) => void;
  #fallback?: Handler;
  #compression?: ResolvedCompressionConfig;

  constructor(options: CpeakOptions = {}) {
    this.#server = http.createServer({
      IncomingMessage: CpeakIncomingMessage,
      ServerResponse: CpeakServerResponse
    });
    this.#router = new Router();
    this.#middleware = [];

    // Resolve compression options once at app startup.
    if (options.compression) {
      this.#compression = resolveCompressionOptions(options.compression);
    }

    // Merge developer-supplied mime types with the defaults once at startup
    if (options.mimeTypes) Object.assign(MIME_TYPES, options.mimeTypes);

    this.#server.on(
      "request",
      async (req: CpeakRequest, res: CpeakResponse) => {
        res._compression = this.#compression;

        // Get the url without the URL parameters (query strings)
        const qIndex = req.url?.indexOf("?");
        const urlWithoutQueries =
          qIndex === -1 ? req.url || "" : req.url?.substring(0, qIndex);

        await this.#runMiddleware(
          req,
          res,
          this.#middleware,
          0,
          urlWithoutQueries
        );
      }
    );
  }

  // Run all the specific middleware functions for that router only and then run the handler
  async #runHandler(
    req: CpeakRequest,
    res: CpeakResponse,
    middleware: RouteMiddleware[],
    cb: Handler,
    index: number
  ) {
    // Our exit point...
    if (index === middleware.length) {
      // Call the route handler with the modified req and res objects.
      // Also handle the promise errors by passing them to handleErr to save developers from having to manually wrap every handler in try/catch.
      try {
        await cb(req, res);
      } catch (error) {
        this.#dispatchError(error, req, res);
      }
    } else {
      // Handle the promise errors by passing them to handleErr to save developers from having to manually wrap every route middleware in try/catch.
      try {
        await middleware[index](req, res, async (error?: unknown) => {
          // this function only accepts an error argument to be more compatible with NPM modules that are built for express
          if (error) {
            return this.#dispatchError(error, req, res);
          }
          await this.#runHandler(req, res, middleware, cb, index + 1);
        });
      } catch (error) {
        this.#dispatchError(error, req, res);
      }
    }
  }

  // Run all the middleware functions (beforeEach functions) before we run the corresponding route
  async #runMiddleware(
    req: CpeakRequest,
    res: CpeakResponse,
    middleware: Middleware[],
    index: number,
    urlWithoutQueries?: string
  ) {
    // Our exit point...
    if (index === middleware.length) {
      const method = req.method?.toLowerCase() || "";
      const found = this.#router.find(method, urlWithoutQueries || "");

      if (found) {
        req.params = found.params;
        return await this.#runHandler(
          req,
          res,
          found.middleware,
          found.handler,
          0
        );
      }

      // If a fallback handler is registered, run it before falling back to the default 404
      if (this.#fallback) {
        try {
          return await this.#fallback(req, res);
        } catch (error) {
          return this.#dispatchError(error, req, res);
        }
      }

      // If the requested route dose not exist, and developer has not registered the fallback handler, return 404
      return res
        .status(404)
        .json({ error: `Cannot ${req.method} ${urlWithoutQueries}` });
    } else {
      try {
        await middleware[index](req, res, async (err?: unknown) => {
          if (err) {
            return this.#dispatchError(err, req, res);
          }
          await this.#runMiddleware(
            req,
            res,
            middleware,
            index + 1,
            urlWithoutQueries
          );
        });
      } catch (error) {
        this.#dispatchError(error, req, res);
      }
    }
  }

  // Routes every error path through the registered handleErr. Awaits
  // handleErr so its own async work (or a rejecting res.json under
  // compression) is caught. If handleErr itself fails, we log and send a
  // bare 500 so the client never gets a hung socket. Returns a Promise
  // that never rejects to avoid unhandled promise rejections in case of errors in handleErr.
  async #dispatchError(error: unknown, req: CpeakRequest, res: CpeakResponse) {
    if (res.headersSent) {
      req.socket?.destroy();
    } else {
      res.setHeader("Connection", "close");
    }

    if (isClientDisconnect(error) && !(error as any).clientDisconnect) {
      (error as any).clientDisconnect = true;
    }

    try {
      await this.#handleErr?.(error, req, res);
    } catch (handlerFailure) {
      console.error(
        "[cpeak] handleErr failed while processing:",
        error,
        "\nReason:",
        handlerFailure
      );

      if (!res.headersSent) {
        try {
          res.statusCode = 500;
          res.end();
        } catch {}
      }
    }
  }

  route(method: string, path: string, ...args: (RouteMiddleware | Handler)[]) {
    // The last argument should always be our handler
    const cb = args.pop() as Handler;

    if (!cb || typeof cb !== "function") {
      throw new Error("Route definition must include a handler");
    }

    // Rest will be our middleware functions
    const middleware = args.flat() as RouteMiddleware[];

    this.#router.add(method, path, middleware, cb);
  }

  beforeEach(cb: Middleware) {
    this.#middleware.push(cb);
  }

  handleErr(cb: (err: unknown, req: CpeakRequest, res: CpeakResponse) => void) {
    this.#handleErr = cb;
  }

  // This will handle any request that doesn't match any of the routes and middleware functions
  fallback(cb: Handler) {
    if (this.#fallback) {
      throw frameworkError(
        "Fallback handler is already registered. Only one fallback can be set per app.",
        this.fallback,
        ErrorCode.DUPLICATE_FALLBACK
      );
    }
    this.#fallback = cb;
  }

  // The first 3 listens are just TS overloads for better type inference and editor autocompletion. The last one is the actual implementation.
  listen(port: number, cb?: () => void): CpeakHttpServer;
  listen(port: number, host: string, cb?: () => void): CpeakHttpServer;
  listen(options: net.ListenOptions, cb?: () => void): CpeakHttpServer;
  listen(...args: any[]) {
    return this.#server.listen(...args);
  }

  address() {
    return this.#server.address();
  }

  close(cb?: (err?: Error) => void) {
    return this.#server.close(cb);
  }

  // A getter for developers who want to access the underlying http server instance for advanced use cases that aren't covered by Cpeak
  get server() {
    return this.#server;
  }
}

// Util functions
export {
  serveStatic,
  parseJSON,
  render,
  swagger,
  auth,
  hashPassword,
  verifyPassword,
  cookieParser,
  cors
} from "./utils";

export type {
  AuthOptions,
  PbkdfOptions,
  CookieOptions,
  CorsOptions
} from "./utils/types";

export type { CompressionOptions } from "./internal/types";

export type {
  CpeakHttpServer,
  CpeakOptions,
  CpeakRequest,
  CpeakResponse,
  Next,
  Middleware,
  RouteMiddleware,
  Handler
} from "./types";

export default function cpeak(options?: CpeakOptions): Cpeak {
  return new Cpeak(options);
}
