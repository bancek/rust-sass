// Option normalization: the public `CompileOptions`/`CompileStringOptions` →
// the wire `WasmOptions` handed to Rust. Functions/importers/logger are wrapped
// by the per-compile `Adapter` (js handles kept alive for the duration of the
// call); deprecation lists are normalized (Version → `{__sassVersion}` marker).

import {
  Adapter,
  CustomFunction,
  JsImporter,
  WrappedFunction,
} from './adapter';
import type { NodeFs } from './io/node-fs';
import type { NodeFsAsync } from './io/node-fs-async';
import { NodePackageImporter } from './node-package-importer';
import { Logger } from './logger';
import { Version } from './version';
import type { Deprecation } from './deprecations';
import type { WasmOptions } from './types';

export interface CompileOptions {
  style?: 'expanded' | 'compressed';
  sourceMap?: boolean;
  sourceMapIncludeSources?: boolean;
  charset?: boolean;
  quietDeps?: boolean;
  verbose?: boolean;
  alertColor?: boolean;
  alertAscii?: boolean;
  syntax?: 'scss' | 'indented' | 'css';
  url?: URL | string;
  loadPaths?: string[];
  functions?: Record<string, CustomFunction>;
  importers?: (JsImporter | NodePackageImporter)[];
  logger?: { warn?: Function; debug?: Function };
  fatalDeprecations?: (string | Deprecation | Version)[];
  silenceDeprecations?: (string | Deprecation)[];
  futureDeprecations?: (string | Deprecation)[];
  /** rust-sass extensions (docs/ref/wasm.md, "Extension options"); not part of the
   * public `sass` surface — do not add them to export-surface/js-api-doc. */
  /** Explicit filesystem delegate (default: Node `node:fs`, or the browser
   * throwing delegate). */
  io?: NodeFs | NodeFsAsync;
  /** `ascii: true` compiles everything ASCII (eval-time glyph baking
   * included); `color: true` renders fallback warning/error text in ANSI. */
  ascii?: boolean;
  color?: boolean;
  /** Sinks for the default-logger fallback when no `Logger.warn`/`debug`. */
  consoleWarn?: (block: string) => void;
  consoleDebug?: (block: string) => void;
}

export interface CompileStringOptions extends CompileOptions {
  syntax?: 'scss' | 'indented' | 'css';
  url?: URL | string;
  importer?: JsImporter | NodePackageImporter;
}

const scalarKeys = [
  'style',
  'sourceMap',
  'sourceMapIncludeSources',
  'charset',
  'quietDeps',
  'verbose',
  'alertColor',
  'alertAscii',
  'ascii',
  'color',
  'syntax',
  'loadPaths',
] as const;

const deprecationKeys = [
  'fatalDeprecations',
  'silenceDeprecations',
  'futureDeprecations',
] as const;

/** Converts `Version` entries (fatal only) to the `{__sassVersion}` wire
 * marker; strings and `Deprecation` objects pass through unchanged. */
function normalizeDeprecationList(
  list: (string | Deprecation | Version)[],
): unknown[] {
  return list.map((entry) =>
    entry instanceof Version ? { __sassVersion: entry.toString() } : entry,
  );
}

export function normalizeOptions(
  options: CompileStringOptions | undefined,
  adapter: Adapter,
  sync: boolean,
): WasmOptions {
  if (!options) return {};
  const out: WasmOptions = {};

  for (const key of scalarKeys) {
    const value = options[key];
    if (value !== undefined) (out as Record<string, unknown>)[key] = value;
  }
  for (const key of deprecationKeys) {
    const value = options[key];
    if (value !== undefined) out[key] = normalizeDeprecationList(value);
  }
  if (options.url !== undefined) {
    out.url = options.url instanceof URL ? options.url.toString() : options.url;
  }
  if (options.functions) {
    const functions: Record<string, WrappedFunction> = {};
    for (const [signature, fn] of Object.entries(options.functions)) {
      functions[signature] = adapter.wrapFunction(fn, sync);
    }
    out.functions = functions;
  }
  const wrapImporter = (
    importer: JsImporter | NodePackageImporter | null | undefined,
  ) =>
    importer === null || importer === undefined
      ? importer
      : importer instanceof NodePackageImporter
        ? { __sassNodePackageImporter: importer.entryPointDirectory }
        : adapter.wrapImporter(importer, sync);
  if (options.importers) {
    out.importers = options.importers.map(wrapImporter);
  }
  if (options.importer !== undefined) {
    out.importer = wrapImporter(options.importer);
  }
  if (options.logger === Logger.silent) {
    out.logger = { __sassSilent: true };
  } else if (options.logger) {
    out.logger = adapter.wrapLogger(options.logger, sync);
  }
  // rust-sass extension pass-throughs (io is filled in by index.ts when the
  // user didn't provide one; the sinks forward rendered fallback blocks).
  if (options.io !== undefined) out.io = options.io;
  if (options.consoleWarn !== undefined) out.consoleWarn = options.consoleWarn;
  if (options.consoleDebug !== undefined) {
    out.consoleDebug = options.consoleDebug;
  }
  return out;
}
