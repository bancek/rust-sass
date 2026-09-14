// The async Node filesystem delegate for the wasm bridge (used by the async
// compile entry points: compileAsync/compileStringAsync). Same contract as
// `NodeFs` but the fs methods are ALWAYS `Promise<T>` (never `T | Promise<T>`)
// so the bridge stays non-blocking and browser io could be HTTP-based later.
// On failure they REJECT with an `IoError`-shaped error (see `./error`).
//
// The `Io` trait's sync methods stay sync here (`currentDir`, `isWindows`,
// `isMacOS`, `supportsAnsiEscapes`, and `readDir` — the latter feeds the
// synchronous `real_case_path` recursion).
//
// dart-source: lib/src/io/js.dart (Node behavior)

import * as fs from 'node:fs';
import { promises as fsp } from 'node:fs';
import * as path from 'node:path';

import { toIoError } from './error';

export interface NodeFsAsync {
  readFile(p: string): Promise<Uint8Array>;
  fileExists(p: string): Promise<boolean>;
  dirExists(p: string): Promise<boolean>;
  linkExists(p: string): Promise<boolean>;
  /** All entries (files and dirs) of `p` as full paths. SYNC (see above). */
  readDir(p: string): string[];
  currentDir(): string;
  isWindows(): boolean;
  isMacOS(): boolean;
  supportsAnsiEscapes(): boolean;
}

export function createNodeFsAsync(): NodeFsAsync {
  return {
    async readFile(p: string): Promise<Uint8Array> {
      try {
        return await fsp.readFile(p);
      } catch (e) {
        throw toIoError(e, p);
      }
    },
    async fileExists(p: string): Promise<boolean> {
      try {
        return (await fsp.stat(p)).isFile();
      } catch (e) {
        if ((e as { code?: unknown }).code === 'ENOENT') return false;
        throw toIoError(e, p);
      }
    },
    async dirExists(p: string): Promise<boolean> {
      try {
        return (await fsp.stat(p)).isDirectory();
      } catch (e) {
        if ((e as { code?: unknown }).code === 'ENOENT') return false;
        throw toIoError(e, p);
      }
    },
    async linkExists(p: string): Promise<boolean> {
      try {
        return (await fsp.lstat(p)).isSymbolicLink();
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
