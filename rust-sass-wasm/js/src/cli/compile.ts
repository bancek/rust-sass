// The compile runner: mirrors rust-sass-cli/src/compile.rs (compile_all /
// compile_one / write_source_map) onto the unified wasm `compile_bytes` export
// (docs/ref/wasm.md, "Node CLI and export surface"). File I/O (reading inputs, writing
// destinations, source maps) stays here in node:fs; the filesystem delegate and
// the `consoleWarn`/`consoleDebug` sinks are passed in `options.io`/`options`,
// so warnings stream byte-exactly to `process.stderr` during the compile and
// errors arrive as the shared enriched data object (`formatted` +
// `errorCss`).
//
// dart-source: lib/src/executable/compile_stylesheet.dart + concurrent.dart

import * as fs from 'node:fs';
import * as p from 'node:path';
import { pathToFileURL } from 'node:url';

import { loadSyncWasm } from '../wasm-loader';
import { createNodeFs } from '../io/node-fs';
import type { ExceptionData, WasmCompileResult, WasmOptions } from '../types';
import { CliOptions } from './args';

/** A CLI I/O error carrying the process exit code (66). */
class CliIoError extends Error {
  constructor(
    readonly exitCode: number,
    message: string,
  ) {
    super(message);
    this.name = 'CliIoError';
  }
}

/** A tiny "pretty path" for messages: relative to cwd (Dart `p.relative`),
 * matching `sass.js` (`Error reading ../../x.scss: ...`). */
function prettyPath(path: string): string {
  return p.relative(process.cwd(), path);
}

/** The clean OS message for a node:fs error (Dart shows the strerror, not
 * Node's `CODE: message, open '...'` form). */
function osErrorMessage(err: NodeJS.ErrnoException): string {
  switch (err.code) {
    case 'ENOENT':
      return 'no such file or directory';
    case 'EACCES':
      return 'permission denied';
    case 'EPERM':
      return 'operation not permitted';
    case 'EISDIR':
      return 'is a directory';
    case 'ENOTDIR':
      return 'not a directory';
    case 'ELOOP':
      return 'too many levels of symbolic links';
    case 'EEXIST':
      return 'file exists';
    default:
      return err.message;
  }
}

/** Reads the input file as raw bytes via node:fs so missing files produce
 * Dart's clean `Error reading <pretty>: <os message>.` and exit 66 (sass.js
 * parity), and so invalid UTF-8 reaches the compiler (which renders Dart's
 * `Invalid UTF-8.` error instead of a lossy parse error). */
function readInputFile(source: string): Buffer {
  try {
    return fs.readFileSync(source);
  } catch (e) {
    const err = e as NodeJS.ErrnoException;
    throw new CliIoError(
      66,
      `Error reading ${prettyPath(source)}: ${osErrorMessage(err)}.`,
    );
  }
}

/** The syntax implied by [path]'s extension (mirrors `syntax_for_path`). */
function syntaxForPath(path: string): 'scss' | 'indented' | 'css' {
  const ext = p.extname(path);
  if (ext === '.sass') return 'indented';
  if (ext === '.css') return 'css';
  return 'scss';
}

/** Builds the wasm wire options from the CLI options for one compilation.
 * The `ascii` knob maps `--unicode` (`--no-unicode` → ascii), `color` mirrors
 * the resolved `-c`/terminal color, and the sinks route the default logger's
 * rendered blocks straight to `process.stderr` (no console newline). */
function buildOptions(
  opts: CliOptions,
  url: string | undefined,
  syntax: 'scss' | 'indented' | 'css' | undefined,
): WasmOptions {
  const o: WasmOptions = {};
  if (opts.loadPaths.length > 0) o.loadPaths = opts.loadPaths;
  if (opts.style === 'compressed') o.style = 'compressed';
  if (!opts.charset) o.charset = false;
  if (opts.quietDeps) o.quietDeps = true;
  if (opts.verbose) o.verbose = true;
  if (opts.emitSourceMap) o.sourceMap = true;
  if (opts.embedSources) o.sourceMapIncludeSources = true;
  if (!opts.unicode) o.ascii = true;
  if (opts.alertColor) o.color = true;
  if (opts.silenceDeprecations.length > 0) {
    o.silenceDeprecations = opts.silenceDeprecations;
  }
  if (opts.fatalDeprecations.length > 0) {
    o.fatalDeprecations = opts.fatalDeprecations;
  }
  if (opts.futureDeprecations.length > 0) {
    o.futureDeprecations = opts.futureDeprecations;
  }
  if (opts.silent) o.logger = { __sassSilent: true };
  if (opts.nodePackageImporter) {
    o.importers = [{ __sassNodePackageImporter: process.cwd() }];
  }
  if (url !== undefined) o.url = url;
  if (syntax !== undefined) o.syntax = syntax;
  return o;
}

/** The restricted percent-encoding Dart's `Uri.dataFromString` uses (unreserved
 * + `!$&'()*+,/:;=?@`), applied to UTF-8 bytes. `sass.js` embeds the source-map
 * JSON in a data URL with exactly this encoding. */
const DATA_URL_SAFE = /[A-Za-z0-9\-_.~!$&'()*+,/:;=?@]/;

function encodeDataUrl(s: string): string {
  let out = '';
  const bytes = new TextEncoder().encode(s);
  for (const b of bytes) {
    const c = String.fromCharCode(b);
    out += DATA_URL_SAFE.test(c)
      ? c
      : `%${b.toString(16).toUpperCase().padStart(2, '0')}`;
  }
  return out;
}

/** Writes the source map given by [result] to disk (if necessary) according to
 * [opts], returning the source map comment to append to the CSS. Port of
 * `_writeSourceMap` (rust-sass-cli compile.rs / Dart compile_stylesheet.dart).
 * Only `file:` sources are remapped (Dart leaves `data:` URLs untouched). */
export function writeSourceMap(
  opts: CliOptions,
  result: WasmCompileResult,
  dest: string | null,
): string {
  const sourceMap = result.sourceMap as
    { sources: string[]; file?: string } | undefined;
  if (sourceMap === undefined) return '';

  for (let i = 0; i < sourceMap.sources.length; i++) {
    const url = sourceMap.sources[i];
    if (url.startsWith('file://')) {
      sourceMap.sources[i] = opts.sourceMapUrl(url, dest);
    }
  }
  if (dest !== null) sourceMap.file = p.basename(dest);

  const text = JSON.stringify(sourceMap);

  let url: string;
  if (opts.embedSourceMap) {
    // Dart embeds the JSON percent-encoded with the restricted `data:` set
    // (which re-encodes the `%` of any inner data: URLs in `sources`).
    url = `data:application/json;charset=utf-8,${encodeDataUrl(text)}`;
  } else {
    // [dest] can't be null here because --embed-source-map is incompatible
    // with writing to stdout.
    const mapPath = `${dest}.map`;
    fs.mkdirSync(p.dirname(mapPath), { recursive: true });
    fs.writeFileSync(mapPath, text);
    url = opts.sourceMapUrl(mapPath, dest).replace(/^file:\/\//, '');
  }

  const escaped = url.replace(/\*\//g, '%2A/');
  return `${opts.style === 'compressed' ? '' : '\n\n'}/*# sourceMappingURL=${escaped} */`;
}

/** Ensures the parent directory of [dest] exists. */
function ensureDir(dest: string): void {
  const parent = p.dirname(dest);
  if (parent !== '') fs.mkdirSync(parent, { recursive: true });
}

/** Compiles a single source (stdin or file) to a destination (stdout or file).
 * Writes errors to stderr itself and returns the process exit code. */
function compileOne(
  opts: CliOptions,
  source: string | null,
  dest: string | null,
): number {
  let input: Buffer;
  let url: string | undefined;
  let syntax: 'scss' | 'indented' | 'css' | undefined;
  try {
    if (source === null) {
      input = fs.readFileSync(0);
      syntax = opts.indented ? 'indented' : 'scss';
    } else {
      input = readInputFile(source);
      url = pathToFileURL(p.resolve(source)).href;
      syntax = opts.indented ? 'indented' : syntaxForPath(source);
    }
  } catch (e) {
    if (e instanceof CliIoError) {
      process.stderr.write(`${e.message}\n`);
      return e.exitCode;
    }
    throw e;
  }

  const wasm = loadSyncWasm();
  const options = buildOptions(opts, url, syntax);
  // The filesystem delegate and the default-logger sinks ride in the options
  // (docs/ref/wasm.md, "Extension options"): warnings/debug stream byte-exactly to
  // stderr during the compile.
  options.io = createNodeFs();
  options.consoleWarn = (block) => process.stderr.write(block);
  options.consoleDebug = (block) => process.stderr.write(block);

  let result;
  try {
    result = wasm.compile_bytes(input, options);
  } catch (e) {
    const err = e as ExceptionData;
    if (typeof err.formatted !== 'string') throw e;
    if (opts.emitErrorCss) {
      const css = err.errorCss ?? '';
      if (dest !== null) {
        try {
          ensureDir(dest);
          fs.writeFileSync(dest, `${css}\n`);
        } catch (ioErr) {
          process.stderr.write(
            `${new CliIoError(66, `Error writing ${dest}: ${osErrorMessage(ioErr as NodeJS.ErrnoException)}.`).message}\n`,
          );
          return 66;
        }
      } else {
        process.stdout.write(`${css}\n`);
      }
    } else if (dest !== null) {
      fs.rmSync(dest, { force: true });
    }
    process.stderr.write(`${err.formatted}\n`);
    return 65;
  }

  const css = result.css + writeSourceMap(opts, result, dest);
  if (dest !== null) {
    try {
      ensureDir(dest);
      fs.writeFileSync(dest, `${css}\n`);
    } catch (e) {
      const err = e as NodeJS.ErrnoException;
      process.stderr.write(`Error writing ${dest}: ${osErrorMessage(err)}.\n`);
      return 66;
    }
  } else if (css !== '') {
    process.stdout.write(`${css}\n`);
  }
  return 0;
}

/** Compiles every source in [opts]. Returns the max exit code (0 on success).
 * Port of `compile_all` (rust-sass-cli compile.rs). Note: the official
 * `sass.js` CLI prints no "Compiled X to Y." success line (verified 1.100.0),
 * so neither do we — byte parity for dest-mode differentials. */
export function compileAll(opts: CliOptions): number {
  let exitCode = 0;
  for (const entry of opts.sources) {
    const code = compileOne(opts, entry.source, entry.dest);
    if (code !== 0) {
      if (code > exitCode) exitCode = code;
      if (opts.stopOnError) break;
    }
  }
  return exitCode;
}
