// Wire types shared between the shim and the wasm artifacts (serde-wasm-bindgen
// outgoing; manual js_sys incoming). See docs/ref/wasm.md, "Value marshalling wire format".

import type { NodeFs } from './io/node-fs';
import type { NodeFsAsync } from './io/node-fs-async';
import type { WrappedFunction } from './adapter';

/** The options object handed to the wasm compile entry points (already
 * normalized by `options.ts`). The `io`/`ascii`/`color`/`consoleWarn`/
 * `consoleDebug` keys are rust-sass extensions (docs/ref/wasm.md, "Extension options"),
 * not part of the public `sass` option surface. */
export interface WasmOptions {
  style?: 'expanded' | 'compressed';
  sourceMap?: boolean;
  sourceMapIncludeSources?: boolean;
  charset?: boolean;
  quietDeps?: boolean;
  verbose?: boolean;
  alertColor?: boolean;
  alertAscii?: boolean;
  syntax?: 'scss' | 'indented' | 'css';
  loadPaths?: string[];
  url?: string;
  functions?: Record<string, WrappedFunction>;
  importers?: unknown[];
  importer?: unknown;
  logger?: unknown;
  fatalDeprecations?: unknown[];
  silenceDeprecations?: unknown[];
  futureDeprecations?: unknown[];
  /** rust-sass extension: an explicit filesystem delegate (docs/ref/wasm.md, "Io bridge"). The
   * shim injects the Node `node:fs` delegate (or the browser throwing
   * delegate) when absent; a user may pass a custom one (e.g. http fs). */
  io?: NodeFs | NodeFsAsync;
  /** rust-sass extension: `ascii: true` → compile everything ASCII (sets
   * `CompileOptions.unicode = false`, so eval-time selector/extend baking is
   * ASCII) and renders errors ASCII too. */
  ascii?: boolean;
  /** rust-sass extension: render ANSI color in fallback warning blocks and
   * thrown error `formatted` text. */
  color?: boolean;
  /** rust-sass extension: sink for the default-logger fallback's rendered
   * warning/deprecation block when no `Logger.warn` is provided. */
  consoleWarn?: (block: string) => void;
  /** rust-sass extension: sink for the default-logger fallback's rendered
   * debug block when no `Logger.debug` is provided. */
  consoleDebug?: (block: string) => void;
}

/** The result object returned by the wasm compile entry points. `sourceMap` is
 * present only when source maps were requested. */
export interface WasmCompileResult {
  css: string;
  loadedUrls: string[];
  sourceMap?: unknown;
}

/** Common to both artifacts. */
interface WasmModuleBase {
  /** Primes the wasm wall clock (`set_time_now(Date.now())`); see docs/ref/wasm.md, "Node CLI and export surface". */
  set_time_now?: (millis: number) => void;
}

/** The sync artifact (`pkg-sync`, zero futures): compile/compileString return
 * the result directly (docs/ref/wasm.md D2). */
export interface SyncWasmModule extends WasmModuleBase {
  compileString(source: string, options: WasmOptions): WasmCompileResult;
  compile(inputPath: string, options: WasmOptions): WasmCompileResult;
  compile_bytes(source: Uint8Array, options: WasmOptions): WasmCompileResult;
}

/** The async artifact (`pkg-async`): compile/compileString return Promises. */
export interface AsyncWasmModule extends WasmModuleBase {
  compileString(
    source: string,
    options: WasmOptions,
  ): Promise<WasmCompileResult>;
  compile(inputPath: string, options: WasmOptions): Promise<WasmCompileResult>;
  compile_bytes(
    source: Uint8Array,
    options: WasmOptions,
  ): Promise<WasmCompileResult>;
}

/** The data object Rust throws (a plain JS object); the shim builds `Exception`
 * from it (docs/ref/wasm.md, "Approach" D8). `formatted`/`errorCss`/`exitCode`-free: it is the
 * shared error shape for every compile entry, including `compile_bytes` (the
 * CLI writes `errorCss` for `--error-css` and `formatted` to stderr). */
export interface ExceptionData {
  sassMessage: string;
  sassStack?: string;
  span?: unknown;
  loadedUrls?: string[];
  formatted?: string;
  errorCss?: string;
}
