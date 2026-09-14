// The sync Node filesystem delegate for the wasm bridge (used by the sync
// compile entry points). This is a purpose-built typed contract — NOT a mirror
// of the Rust `Io` trait (docs/ref/wasm.md, "Io bridge"): only the primitives the bridge needs.
//
// Every operation may fail; on failure the delegate THROWS an `IoError`-shaped
// error (see `./error`) — never returns null/undefined/false-on-error.
// `fileExists`/`dirExists`/`linkExists` return `false` only on ENOENT and throw
// on real errors (Dart rethrow semantics). There is NO `canonicalize` method —
// Rust `JsIo` owns canonicalize, using the `readDir` primitive (all entries,
// files AND dirs) for its case-correction.
//
// `readDir` is intentionally SYNC on both the sync and async delegates: it
// feeds the synchronous `real_case_path` recursion.
//
// dart-source: lib/src/io/js.dart (Node behavior)

import * as fs from 'node:fs';
import * as path from 'node:path';

import { toIoError } from './error';

export interface NodeFs {
  readFile(p: string): Uint8Array;
  fileExists(p: string): boolean;
  dirExists(p: string): boolean;
  linkExists(p: string): boolean;
  /** All entries (files and dirs) of `p` as full paths. */
  readDir(p: string): string[];
  currentDir(): string;
  isWindows(): boolean;
  isMacOS(): boolean;
  supportsAnsiEscapes(): boolean;
}

export function createNodeFs(): NodeFs {
  return {
    readFile(p: string): Uint8Array {
      try {
        return fs.readFileSync(p);
      } catch (e) {
        throw toIoError(e, p);
      }
    },
    fileExists(p: string): boolean {
      try {
        return fs.statSync(p).isFile();
      } catch (e) {
        if ((e as { code?: unknown }).code === 'ENOENT') return false;
        throw toIoError(e, p);
      }
    },
    dirExists(p: string): boolean {
      try {
        return fs.statSync(p).isDirectory();
      } catch (e) {
        if ((e as { code?: unknown }).code === 'ENOENT') return false;
        throw toIoError(e, p);
      }
    },
    linkExists(p: string): boolean {
      try {
        return fs.lstatSync(p).isSymbolicLink();
      } catch (e) {
        if ((e as { code?: unknown }).code === 'ENOENT') return false;
        throw toIoError(e, p);
      }
    },
    readDir(p: string): string[] {
      try {
        return fs.readdirSync(p).map((child) => path.join(p, child));
      } catch (e) {
        throw toIoError(e, p);
      }
    },
    currentDir(): string {
      return process.cwd();
    },
    isWindows(): boolean {
      return process.platform === 'win32';
    },
    isMacOS(): boolean {
      return process.platform === 'darwin';
    },
    supportsAnsiEscapes(): boolean {
      return true;
    },
  };
}
