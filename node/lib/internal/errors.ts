// A utility function to create an error with a custom stack trace
export function frameworkError(
  message: string,
  skipFn: Function,
  code?: string,
  status?: number,
  clientDisconnect?: boolean
) {
  const err = new Error(message) as Error & {
    code?: string;
    cpeak_err?: boolean;
    clientDisconnect?: boolean;
  };
  Error.captureStackTrace(err, skipFn);

  err.cpeak_err = true;

  if (code) err.code = code;
  if (status) (err as any).status = status;
  if (clientDisconnect) err.clientDisconnect = true;

  return err;
}

const CLIENT_DISCONNECT_CODES = new Set([
  "ERR_STREAM_PREMATURE_CLOSE",
  "ERR_STREAM_DESTROYED",
  "ECONNRESET",
  "EPIPE"
]);

export function isClientDisconnect(err: unknown): boolean {
  return CLIENT_DISCONNECT_CODES.has((err as any)?.code);
}

export enum ErrorCode {
  MISSING_MIME = "CPEAK_ERR_MISSING_MIME",
  FILE_NOT_FOUND = "CPEAK_ERR_FILE_NOT_FOUND",
  NOT_A_FILE = "CPEAK_ERR_NOT_A_FILE",
  SEND_FILE_FAIL = "CPEAK_ERR_SEND_FILE_FAIL",
  INVALID_JSON = "CPEAK_ERR_INVALID_JSON",
  PAYLOAD_TOO_LARGE = "CPEAK_ERR_PAYLOAD_TOO_LARGE",
  WEAK_SECRET = "CPEAK_ERR_WEAK_SECRET",
  COMPRESSION_NOT_ENABLED = "CPEAK_ERR_COMPRESSION_NOT_ENABLED",
  RENDER_NOT_ENABLED = "CPEAK_ERR_RENDER_NOT_ENABLED",
  // For router:
  DUPLICATE_ROUTE = "CPEAK_ERR_DUPLICATE_ROUTE",
  INVALID_ROUTE = "CPEAK_ERR_INVALID_ROUTE",
  DUPLICATE_FALLBACK = "CPEAK_ERR_DUPLICATE_FALLBACK",
  RENDER_FAIL = "CPEAK_ERR_RENDER_FAIL"
}
