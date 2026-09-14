// Browser io delegates for the wasm bridge. Mirror Dart's browser behavior
// (`lib/src/io/browser.dart` / `lib/src/io/js.dart` UnsupportedError): every
// filesystem operation throws (sync delegate) or rejects (async delegate) with
// an `IoError`-shaped error. Messages are Dart-exact per method
// (`lib/src/io/js.dart`: `"<method>() is only supported on Node.js"`).
// `compileString*` works everywhere; filesystem access is expected to go
// through custom JS importers instead.
//
// The async delegate keeps this shape so browser io could be implemented over
// HTTP later (not now) — the methods are still always-Promise.

import type { NodeFs } from './node-fs';
import type { NodeFsAsync } from './node-fs-async';

function unsupportedSync(method: string): (_p: string) => never {
  return (_p: string): never => {
    throw {
      message: `${method}() is only supported on Node.js`,
      kind: 'Other' as const,
    };
  };
}

function unsupportedAsync(method: string): (_p: string) => Promise<never> {
  return (_p: string): Promise<never> => {
    return Promise.reject({
      message: `${method}() is only supported on Node.js`,
      kind: 'Other' as const,
    });
  };
}

export function createBrowserFs(): NodeFs {
  return {
    readFile: unsupportedSync('readFile'),
    fileExists: unsupportedSync('fileExists'),
    dirExists: unsupportedSync('dirExists'),
    linkExists: unsupportedSync('linkExists'),
    readDir: unsupportedSync('listDir'),
    currentDir: () => '/',
    isWindows: () => false,
    isMacOS: () => false,
    supportsAnsiEscapes: () => true,
  };
}

export function createBrowserFsAsync(): NodeFsAsync {
  return {
    readFile: unsupportedAsync('readFile'),
    fileExists: unsupportedAsync('fileExists'),
    dirExists: unsupportedAsync('dirExists'),
    linkExists: unsupportedAsync('linkExists'),
    readDir: unsupportedSync('listDir'),
    currentDir: () => '/',
    isWindows: () => false,
    isMacOS: () => false,
    supportsAnsiEscapes: () => true,
  };
}
