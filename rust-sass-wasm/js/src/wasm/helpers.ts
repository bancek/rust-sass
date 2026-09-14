// Test-only helpers for the wasm-backed behavior tests in this directory.
// These tests exercise the public API (`js/src/index.ts`) against the built
// wasm artifacts and, for parity cases, the official `sass` package.

import * as fs from 'node:fs';
import * as os from 'node:os';
import * as p from 'node:path';

/** The `rust-sass-wasm/` package root (works under vitest and plain CJS). */
function wasmRoot(): string {
  if (typeof __dirname === 'string')
    return p.resolve(__dirname, '..', '..', '..');
  return p.resolve(process.cwd());
}

/** Whether both wasm artifacts are built (`js/dist/pkg-sync`/`pkg-async`). */
export const artifactsPresent =
  fs.existsSync(
    p.join(wasmRoot(), 'js', 'dist', 'pkg-sync', 'rust_sass_wasm.js'),
  ) &&
  fs.existsSync(
    p.join(wasmRoot(), 'js', 'dist', 'pkg-async', 'rust_sass_wasm.js'),
  );

/** The `embedded-host-node-rust/` package root (repo-root sibling). */
function wrapperRoot(): string {
  if (typeof __dirname === 'string')
    return p.resolve(
      __dirname,
      '..',
      '..',
      '..',
      '..',
      'embedded-host-node-rust',
    );
  return p.resolve(process.cwd(), '..', 'embedded-host-node-rust');
}

/** Current platform triple, mirroring the host's musl detection. */
function currentTriple(): string {
  let plat: string = process.platform;
  if (plat === 'linux') {
    const report = (
      process as unknown as {
        report?: {
          getReport?: () => { header?: { glibcVersionArray?: unknown } };
        };
      }
    ).report?.getReport?.();
    const glibc = report?.header?.glibcVersionArray;
    if (!Array.isArray(glibc) || glibc.length === 0) plat = 'linux-musl';
  }
  return `${plat}-${process.arch}`;
}

/** Whether the assembled wrapper + its local platform binary are present
 * (requires `npm run build` in `embedded-host-node-rust` first). */
export const wrapperPresent =
  fs.existsSync(p.join(wrapperRoot(), 'dist', 'lib', 'index.mjs')) &&
  fs.existsSync(
    p.join(
      wrapperRoot(),
      'platform',
      currentTriple(),
      'dart-sass',
      process.platform === 'win32' ? 'sass.bat' : 'sass',
    ),
  );

/** Creates a throwaway directory and returns its absolute path. */
export function tempDir(): string {
  return fs.mkdtempSync(p.join(os.tmpdir(), 'sass-wasm-test-'));
}
