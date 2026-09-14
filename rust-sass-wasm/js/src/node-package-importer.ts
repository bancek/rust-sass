// The `NodePackageImporter` class, exported for API fidelity. Computing the
// entrypoint mirrors `lib/src/js/compile.dart` (the `nodePackageImporterClass`
// factory) + `lib/src/js/utils.dart` (`entrypointFilename`). Compilation
// support (passing the entrypoint to the Rust `NodePackageImporter`) lands in
// a later phase; the class shape and constructor behavior are complete.

import * as p from 'node:path';

/** The entrypoint filename, if it can be located (utils.dart entrypointFilename). */
function defaultEntrypointFilename(): string | null {
  const main = (require as { main?: { filename?: string } }).main;
  if (main?.filename) return main.filename;
  const firstArg = process.argv[1];
  if (firstArg) return require.resolve(firstArg);
  return null;
}

let entrypointFilenameOverride: (() => string | null) | undefined;

/**
 * @internal
 *
 * Test-only hook that replaces the entrypoint-filename resolution. Not part of
 * the public API; used because `require.main` is not controllable under the
 * test runner (vitest). Passing `undefined` restores the default behavior.
 */
export function _setEntrypointFilename(
  fn: (() => string | null) | undefined,
): void {
  entrypointFilenameOverride = fn;
}

/** An importer that loads `pkg:` URLs from `node_modules` packages. */
export class NodePackageImporter {
  /** The absolute directory of the package entrypoint. */
  readonly entryPointDirectory: string;

  /**
   * @param entryPointDirectory - If set, the importer will resolve relative to
   * this directory instead of the Node.js entrypoint.
   */
  constructor(entryPointDirectory?: string) {
    if (entryPointDirectory !== undefined) {
      this.entryPointDirectory = p.resolve(entryPointDirectory);
      return;
    }
    const filename = entrypointFilenameOverride
      ? entrypointFilenameOverride()
      : defaultEntrypointFilename();
    if (filename === null) {
      throw 'The Node package importer cannot determine an entry point because `require.main.filename` is not defined. Please provide an `entryPointDirectory` to the `NodePackageImporter`.';
    }
    this.entryPointDirectory = p.dirname(filename);
  }
}
