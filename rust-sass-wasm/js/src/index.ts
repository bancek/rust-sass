// Public compile API for the wasm Sass build: the four top-level compile
// functions + the export surface (value classes, Version, Logger, deprecations,
// NodePackageImporter, Compiler/AsyncCompiler, Exception).
//
// dart-source: lib/src/js/compile.dart + lib/src/js/exception.dart
//
// The sync entry points (`compile`, `compileString`) use the sync wasm artifact
// + the sync `node:fs` delegate; the async entry points use the async artifact +
// the async (always-Promise) delegate (docs/ref/wasm.md, "Io bridge").

import { Adapter } from './adapter';
import { createNodeFs, NodeFs } from './io/node-fs';
import { createNodeFsAsync, NodeFsAsync } from './io/node-fs-async';
import { createBrowserFs, createBrowserFsAsync } from './io/browser-fs';
import { loadSyncWasm, loadAsyncWasm } from './wasm-loader';
import { normalizeOptions, CompileOptions } from './options';
import { normalizeResult, CompileResult } from './result';
import { wrapException } from './exception';
import { activeDeprecationOptions } from './deprecations';
import type { WasmOptions } from './types';

// ==== value class re-exports =================================================

export { Value } from './value/index';
export { SassArgumentList } from './value/argument-list';
export { SassBoolean, sassFalse, sassTrue } from './value/boolean';
export {
  CalculationInterpolation,
  CalculationOperation,
  SassCalculation,
} from './value/calculations';
export { SassColor } from './value/color';
export { SassFunction } from './value/function';
export { ListSeparator, SassList } from './value/list';
export { SassMap } from './value/map';
export { SassMixin } from './value/mixin';
export { SassNumber } from './value/number';
export { sassNull } from './value/null';
export { SassString } from './value/string';

export { Version } from './version';
export { Logger } from './logger';
export { deprecations } from './deprecations';
export { NodePackageImporter } from './node-package-importer';
export {
  AsyncCompiler,
  Compiler,
  initAsyncCompiler,
  initCompiler,
} from './compiler';
export { Exception } from './exception';

export type { CompileOptions, CompileStringOptions } from './options';
export type { CompileResult } from './result';

/** Version and implementation metadata for this Sass build.
 * Single source of truth: `./version`. */
export { INFO_TEXT as info } from './version';

// ==== filesystem delegates (cached singletons) ===============================

// The Node delegate is the default; the browser throwing delegate is used when
// `process` is unavailable (browser bundlers). Both satisfy the same shape.
const isNode = typeof process !== 'undefined' && process.versions?.node;

let syncDelegate: NodeFs;
let asyncDelegate: NodeFsAsync;

function getSyncDelegate(): NodeFs {
  if (syncDelegate === undefined) {
    syncDelegate = isNode ? createNodeFs() : createBrowserFs();
  }
  return syncDelegate;
}

function getAsyncDelegate(): NodeFsAsync {
  if (asyncDelegate === undefined) {
    asyncDelegate = isNode ? createNodeFsAsync() : createBrowserFsAsync();
  }
  return asyncDelegate;
}

// ==== public API =============================================================

/** Registers a compilation's deprecation options for host-side deprecations
 * (e.g. legacy color channel getters), mirroring the embedded host's
 * `activeDeprecationOptions` (compiler/sync.ts + async.ts). */
function withDeprecationOptions<T>(
  options: CompileOptions | undefined,
  run: () => T,
): T {
  const key = Symbol();
  activeDeprecationOptions.set(key, options ?? {});
  try {
    return run();
  } finally {
    activeDeprecationOptions.delete(key);
  }
}

/** Normalizes the public options and injects the filesystem delegate into
 * `options.io` when the user didn't provide one: Node entries use the real
 * `node:fs` delegate; browser entries the throwing delegate (browser: fs
 * access throws Dart-exact "… only supported on Node.js" messages). io is a
 * wasm-only option (wasm can't call `node:fs` directly) — invisible to the
 * public API; a user may pass a custom io (docs/ref/wasm.md, "Extension options"). */
function wireOptions(
  options: CompileOptions | undefined,
  adapter: Adapter,
  async: boolean,
): WasmOptions {
  const o = normalizeOptions(options, adapter, !async);
  if (o.io === undefined) o.io = async ? getAsyncDelegate() : getSyncDelegate();
  return o;
}

export function compileString(
  source: string,
  options?: CompileOptions,
): CompileResult {
  const adapter = new Adapter();
  const wasm = loadSyncWasm();
  try {
    return withDeprecationOptions(options, () =>
      normalizeResult(
        wasm.compileString(source, wireOptions(options, adapter, false)),
      ),
    );
  } catch (e) {
    wrapException(e);
  }
}

export async function compileStringAsync(
  source: string,
  options?: CompileOptions,
): Promise<CompileResult> {
  const adapter = new Adapter();
  const wasm = loadAsyncWasm();
  try {
    return await withDeprecationOptions(options, async () =>
      normalizeResult(
        await wasm.compileString(source, wireOptions(options, adapter, true)),
      ),
    );
  } catch (e) {
    wrapException(e);
  }
}

export function compile(
  inputPath: string,
  options?: CompileOptions,
): CompileResult {
  const adapter = new Adapter();
  const wasm = loadSyncWasm();
  try {
    return withDeprecationOptions(options, () =>
      normalizeResult(
        wasm.compile(inputPath, wireOptions(options, adapter, false)),
      ),
    );
  } catch (e) {
    wrapException(e);
  }
}

export async function compileAsync(
  inputPath: string,
  options?: CompileOptions,
): Promise<CompileResult> {
  const adapter = new Adapter();
  const wasm = loadAsyncWasm();
  try {
    return await withDeprecationOptions(options, async () =>
      normalizeResult(
        await wasm.compile(inputPath, wireOptions(options, adapter, true)),
      ),
    );
  } catch (e) {
    wrapException(e);
  }
}
