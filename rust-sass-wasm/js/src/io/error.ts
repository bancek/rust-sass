// IoError-shaped errors thrown (or rejected with) by the JS io delegates.
// Mirrors `rust_sass::io::IoError {message, kind, path?}` (rust-sass/src/io/
// mod.rs). Rust `JsIo` maps this shape back onto `IoError`, preserving the
// original message/kind/path (docs/ref/wasm.md, "Io bridge").
//
// dart-source: lib/src/io/js.dart (error normalization)

export type IoErrorKind = 'NotFound' | 'Permission' | 'AlreadyExists' | 'Other';

export interface IoError {
  message: string;
  kind: IoErrorKind;
  path?: string;
}

/**
 * Normalizes a caught error (e.g. a `node:fs` error with a `code`/`path`) into
 * an `IoError`-shaped object. The original message and path are preserved; the
 * kind is derived from the error code.
 */
export function toIoError(e: unknown, path?: string): IoError {
  const err = e as { code?: unknown; message?: unknown; path?: unknown };
  const code = typeof err.code === 'string' ? err.code : undefined;
  const kind: IoErrorKind =
    code === 'ENOENT'
      ? 'NotFound'
      : code === 'EACCES' || code === 'EPERM'
        ? 'Permission'
        : code === 'EEXIST'
          ? 'AlreadyExists'
          : 'Other';
  const message = typeof err.message === 'string' ? err.message : String(e);
  const errPath = typeof err.path === 'string' ? err.path : path;
  return errPath === undefined
    ? { message, kind }
    : { message, kind, path: errPath };
}

/** Throws an `IoError`-shaped error for a failing io operation. */
export function throwIoError(e: unknown, path?: string): never {
  throw toIoError(e, path);
}
