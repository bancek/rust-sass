// CLI argument parsing + option resolution, mirroring
// rust-sass-cli/src/options.rs (the `ExecutableOptions` port): the full flag
// surface, the positional grammar, directory mode, source-map gating, and
// deprecation resolution. Pure module (no wasm); the only I/O is sync
// `node:fs` for directory/file detection.
//
// dart-source: lib/src/executable/options.dart

import * as fs from 'node:fs';
import * as p from 'node:path';

import { deprecations } from '../deprecations';
import { SASS_VERSION, Version } from '../version';

/** The Sass version reported by `--version` and used as the cap for
 * `--fatal-deprecation` version arguments. Matches the `info` version.
 * Single source of truth: `../version`. */
export { SASS_VERSION, VERSION_TEXT } from '../version';

/** A usage error (exit code 64), mirroring Dart's `UsageException`. */
export class UsageError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'UsageError';
  }
}

export type Style = 'expanded' | 'compressed';

/** A resolved source → destination entry (null source = stdin, null dest = stdout). */
export interface SourceMapEntry {
  source: string | null;
  dest: string | null;
}

// ==== flag registry ==========================================================

interface ValueFlag {
  short?: string;
  append?: boolean;
  /** Returns an error message when [v] is invalid, else undefined. */
  validate?: (v: string) => string | undefined;
}

const VALUE_FLAGS: Record<string, ValueFlag> = {
  'load-path': { short: 'I', append: true },
  'pkg-importer': {
    short: 'p',
    append: true,
    validate: (v) =>
      v === 'node' ? undefined : `Invalid value "${v}" for --pkg-importer.`,
  },
  style: {
    short: 's',
    validate: (v) =>
      v === 'expanded' || v === 'compressed'
        ? undefined
        : `Invalid value "${v}" for --style.`,
  },
  'source-map-urls': {
    validate: (v) =>
      v === 'relative' || v === 'absolute'
        ? undefined
        : `Invalid value "${v}" for --source-map-urls.`,
  },
  'fatal-deprecation': { append: true },
  'silence-deprecation': { append: true },
  'future-deprecation': { append: true },
  // Hidden no-op, accepted for sass-spec compatibility.
  precision: {},
};

interface BoolFlag {
  short?: string;
  default?: boolean;
  /** Plain `SetTrue` with no `--no-` pair (the hidden `--async` no-op). */
  noNegation?: boolean;
}

const BOOL_FLAGS: Record<string, BoolFlag> = {
  stdin: {},
  indented: {},
  charset: { default: true },
  'error-css': {},
  'source-map': { default: true },
  'embed-sources': {},
  'embed-source-map': {},
  quiet: { short: 'q' },
  'quiet-deps': {},
  verbose: {},
  'stop-on-error': {},
  trace: {},
  color: { short: 'c' },
  unicode: { default: true },
  // Hidden no-op, accepted for sass-spec compatibility.
  async: { noNegation: true },
};

// ==== parsing ================================================================

interface ParsedFlags {
  bools: Map<string, { value: boolean; explicit: boolean }>;
  values: Map<string, { value: string; explicit: boolean }>;
  appends: Map<string, string[]>;
  positionals: string[];
}

/**
 * Tokenizes [argv] into a flat flag state. Boolean flags record their last
 * occurrence (so `--x --no-x` ends false and `--no-x --x` ends true, matching
 * clap's `overrides_with`); each flag also records whether it was explicitly
 * passed (needed for the default-resolving `parsed` checks).
 */
export function parseFlags(argv: string[]): ParsedFlags {
  const bools = new Map<string, { value: boolean; explicit: boolean }>();
  const values = new Map<string, { value: string; explicit: boolean }>();
  const appends = new Map<string, string[]>();
  const positionals: string[] = [];
  let i = 0;
  let onlyPositional = false;

  const setBool = (name: string, value: boolean) =>
    bools.set(name, { value, explicit: true });
  const setValue = (name: string, value: string) =>
    values.set(name, { value, explicit: true });

  while (i < argv.length) {
    const arg = argv[i];
    if (onlyPositional || arg === '-' || !arg.startsWith('-')) {
      positionals.push(arg);
      i++;
      continue;
    }
    if (arg === '--') {
      onlyPositional = true;
      i++;
      continue;
    }
    if (arg.startsWith('--')) {
      let body = arg.slice(2);
      let inlineValue: string | undefined;
      const eq = body.indexOf('=');
      if (eq !== -1) {
        inlineValue = body.slice(eq + 1);
        body = body.slice(0, eq);
      }
      if (body.startsWith('no-')) {
        const name = body.slice(3);
        const flag = BOOL_FLAGS[name];
        if (flag && !flag.noNegation) {
          setBool(name, false);
          i++;
          continue;
        }
        throw new UsageError(`Could not find an option named "--${body}".`);
      }
      const bool = BOOL_FLAGS[body];
      if (bool) {
        setBool(body, true);
        i++;
        continue;
      }
      const value = VALUE_FLAGS[body];
      if (value) {
        let v = inlineValue;
        if (v === undefined) {
          v = argv[i + 1];
          if (v === undefined) {
            throw new UsageError(`A value is required for "--${body}".`);
          }
          i++;
        }
        const err = value.validate?.(v);
        if (err) throw new UsageError(err);
        if (value.append) {
          appends.set(body, [...(appends.get(body) ?? []), v]);
        } else {
          setValue(body, v);
        }
        i++;
        continue;
      }
      throw new UsageError(`Could not find an option named "--${body}".`);
    }
    // Short flag: -I VALUE | -IVALUE | -I=VALUE | -q | -c | -h
    const short = arg[1];
    const shortValue = Object.entries(VALUE_FLAGS).find(
      ([, f]) => f.short === short,
    );
    const shortBool = Object.entries(BOOL_FLAGS).find(
      ([, f]) => f.short === short,
    );
    if (arg.length > 2) {
      const rest = arg[2] === '=' ? arg.slice(3) : arg.slice(2);
      if (!shortValue) {
        throw new UsageError(`Could not find an option named "-${short}".`);
      }
      const [name, flag] = shortValue;
      const err = flag.validate?.(rest);
      if (err) throw new UsageError(err);
      if (flag.append) {
        appends.set(name, [...(appends.get(name) ?? []), rest]);
      } else {
        setValue(name, rest);
      }
      i++;
      continue;
    }
    if (shortValue) {
      const [name, flag] = shortValue;
      const v = argv[i + 1];
      if (v === undefined) {
        throw new UsageError(`A value is required for "-${short}".`);
      }
      const err = flag.validate?.(v);
      if (err) throw new UsageError(err);
      if (flag.append) {
        appends.set(name, [...(appends.get(name) ?? []), v]);
      } else {
        setValue(name, v);
      }
      i += 2;
      continue;
    }
    if (shortBool) {
      setBool(shortBool[0], true);
      i++;
      continue;
    }
    throw new UsageError(`Could not find an option named "-${short}".`);
  }

  return { bools, values, appends, positionals };
}

function boolFlag(flags: ParsedFlags, name: string): boolean {
  return flags.bools.get(name)?.value ?? BOOL_FLAGS[name].default ?? false;
}

/** Whether `--<name>` or `--no-<name>` was explicitly passed on the command line. */
function parsed(flags: ParsedFlags, name: string): boolean {
  return flags.bools.get(name)?.explicit ?? false;
}

/** Whether a non-negatable value option was explicitly passed. */
function parsedValue(flags: ParsedFlags, name: string): boolean {
  return flags.values.get(name)?.explicit ?? false;
}

// ==== path helpers ===========================================================

function isWindowsPath(string: string, index: number): boolean {
  return (
    string.length > index + 2 &&
    /[a-zA-Z]/.test(string[index]) &&
    string[index + 1] === ':'
  );
}

function isDir(path: string): boolean {
  try {
    return fs.statSync(path).isDirectory();
  } catch {
    return false;
  }
}

function isFile(path: string): boolean {
  try {
    return fs.statSync(path).isFile();
  } catch {
    return false;
  }
}

/** Returns whether [argument] contains a colon separator (ignoring Windows
 * drive letters), mirroring `contains_colon` (options.rs:730). */
function containsColon(argument: string): boolean {
  return (
    argument.includes(':') &&
    (!isWindowsPath(argument, 0) || argument.slice(2).includes(':'))
  );
}

/** Splits `source:destination`, mirroring `split_source_and_destination`
 * (options.rs:330) including the Windows drive-letter guards. */
export function splitSourceAndDestination(argument: string): [string, string] {
  for (let i = 0; i < argument.length; i++) {
    if (i === 1 && isWindowsPath(argument, i - 1)) continue;
    if (argument[i] !== ':') continue;
    const nextColon = argument.indexOf(':', i + 1);
    if (
      nextColon !== -1 &&
      nextColon === i + 2 &&
      isWindowsPath(argument, i + 1)
    ) {
      continue;
    }
    if (nextColon !== -1) {
      throw new UsageError(`"${argument}" may only contain one ":".`);
    }
    return [argument.slice(0, i), argument.slice(i + 1)];
  }
  throw new UsageError(`Expected "${argument}" to contain a colon.`);
}

/** Port of `p.relative` (options.rs `relative_path`): strip the common prefix
 * components, then `..` up the remaining `from` components and append the
 * remaining `to` components. */
export function relativePath(from: string, to: string): string {
  const fromParts = from.split('/').filter((s) => s !== '');
  const toParts = to.split('/').filter((s) => s !== '');
  while (fromParts.length > 0 && fromParts[0] === toParts[0]) {
    fromParts.shift();
    toParts.shift();
  }
  if (toParts.length === 0) return '.';
  return [...fromParts.map(() => '..'), ...toParts].join('/');
}

// ==== source resolution ======================================================

/** Port of `list_source_directory` + `collect_files` + `is_entrypoint`
 * (options.rs:800-865): list the entrypoint sources under [source], mapped to
 * destinations under [destination]. */
function listSourceDirectory(
  source: string,
  destination: string,
): [string, string][] {
  const out: [string, string][] = [];
  const entries: string[] = [];
  collectFiles(source, entries);
  for (const pathStr of entries) {
    if (!isEntrypoint(pathStr)) continue;
    // Don't compile a CSS file to its own location.
    if (source === destination && p.extname(pathStr) === '.css') continue;
    const rel = p.relative(source, pathStr);
    const dot = rel.lastIndexOf('.');
    const destRel = (dot === -1 ? rel : rel.slice(0, dot)) + '.css';
    out.push([pathStr, p.join(destination, destRel)]);
  }
  return out;
}

function collectFiles(dir: string, out: string[]): void {
  let children: fs.Dirent[];
  try {
    children = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const child of children) {
    const pathStr = p.join(dir, child.name);
    if (child.isDirectory()) {
      collectFiles(pathStr, out);
    } else if (child.isFile()) {
      out.push(pathStr);
    }
  }
}

/** Returns whether [path] is a Sass entrypoint (that is, not a partial). */
function isEntrypoint(path: string): boolean {
  const base = p.basename(path);
  if (base.startsWith('_')) return false;
  const ext = p.extname(path);
  return ext === '.scss' || ext === '.sass' || ext === '.css';
}

/** Port of `resolve_sources` (options.rs:678): classify the positional
 * arguments into a source→destination map, with the exact usage errors. */
function resolveSources(rest: string[], stdin: boolean): SourceMapEntry[] {
  if (rest.length === 0 && !stdin) {
    throw new UsageError('Compile Sass to CSS.');
  }

  const directories = new Set<string>();
  let colonArgs = false;
  let positionalArgs = false;
  for (const argument of rest) {
    if (argument === '') {
      throw new UsageError(`Invalid argument "".`);
    }
    if (containsColon(argument)) {
      colonArgs = true;
    } else if (isDir(argument)) {
      directories.add(argument);
    } else {
      positionalArgs = true;
    }
  }

  if (positionalArgs || rest.length === 0) {
    if (colonArgs) {
      throw new UsageError(
        'Positional and ":" arguments may not both be used.',
      );
    }
    if (stdin) {
      if (rest.length > 1) {
        throw new UsageError('Only one argument is allowed with --stdin.');
      }
      return [{ source: null, dest: rest[0] ?? null }];
    }
    if (rest.length > 2) {
      throw new UsageError('Only two positional args may be passed.');
    }
    if (directories.size > 0) {
      const first = [...directories][0];
      let message = `Directory "${first}" may not be a positional arg.`;
      const target = rest[rest.length - 1] ?? '';
      if (rest[0] === first && !isFile(target)) {
        message += `\nTo compile all CSS in "${first}" to "${target}", use \`sass ${first}:${target}\`.`;
      }
      throw new UsageError(message);
    }
    const source = rest[0] === '-' ? null : (rest[0] ?? null);
    const destination = rest.length === 1 ? null : (rest[1] ?? null);
    return [{ source, dest: destination }];
  }

  if (stdin) {
    throw new UsageError('--stdin may not be used with ":" arguments.');
  }

  const seen = new Set<string>();
  const sources: SourceMapEntry[] = [];
  for (const argument of rest) {
    if (directories.has(argument)) {
      if (seen.has(argument)) {
        throw new UsageError(`Duplicate source "${argument}".`);
      }
      seen.add(argument);
      for (const [s, d] of listSourceDirectory(argument, argument)) {
        if (!seen.has(s)) {
          seen.add(s);
          sources.push({ source: s, dest: d });
        }
      }
      continue;
    }

    const [source, destination] = splitSourceAndDestination(argument);
    if (seen.has(source)) {
      throw new UsageError(`Duplicate source "${source}".`);
    }
    seen.add(source);
    if (source === '-') {
      sources.push({ source: null, dest: destination });
    } else if (isDir(source)) {
      for (const [s, d] of listSourceDirectory(source, destination)) {
        if (!seen.has(s)) {
          seen.add(s);
          sources.push({ source: s, dest: d });
        }
      }
    } else {
      sources.push({ source, dest: destination });
    }
  }
  return sources;
}

// ==== deprecations / versions ================================================

/** Minimal semver parsing: (major, minor, patch); patch defaults to 0. */
function parseVersion(v: string): [number, number, number] | null {
  const parts = v.split('.');
  const major = Number(parts[0]);
  if (parts.length < 2 || !Number.isInteger(major) || parts[0] === '')
    return null;
  const minor = Number(parts[1]);
  if (!Number.isInteger(minor) || parts[1] === '') return null;
  const patch = parts.length > 2 ? Number(parts[2]) : 0;
  if (!Number.isInteger(patch) || (parts.length > 2 && parts[2] === ''))
    return null;
  return [major, minor, patch];
}

function versionIsValidId(id: string): boolean {
  return id in deprecations;
}

/** Port of `resolve_deprecations` (options.rs:627): ids only; unknown → usage error. */
function resolveDeprecations(flags: ParsedFlags, name: string): string[] {
  const out: string[] = [];
  for (const id of flags.appends.get(name) ?? []) {
    if (!versionIsValidId(id)) {
      throw new UsageError(`Invalid deprecation "${id}".`);
    }
    out.push(id);
  }
  return out;
}

function versionGt(
  a: [number, number, number],
  b: [number, number, number],
): boolean {
  for (let i = 0; i < 3; i++) {
    if (a[i] !== b[i]) return a[i] > b[i];
  }
  return false;
}

/** Port of `resolve_fatal_deprecations` (options.rs:643): ids or a Sass version. */
function resolveFatalDeprecations(flags: ParsedFlags): (string | Version)[] {
  const out = new Map<string, string | Version>();
  for (const id of flags.appends.get('fatal-deprecation') ?? []) {
    if (versionIsValidId(id)) {
      out.set(id, id);
      continue;
    }
    const version = parseVersion(id);
    if (version === null) {
      throw new UsageError(`Invalid deprecation "${id}".`);
    }
    const current = parseVersion(SASS_VERSION)!;
    if (versionGt(version, current)) {
      throw new UsageError(
        `Invalid version ${id}. --fatal-deprecation requires a version less than or equal to the current Dart Sass version.`,
      );
    }
    out.set(id, new Version(version[0], version[1], version[2]));
  }
  return [...out.values()];
}

// ==== source maps ============================================================

/** Port of `resolve_emit_source_map` (options.rs:870). */
function resolveEmitSourceMap(
  flags: ParsedFlags,
  sources: SourceMapEntry[],
): boolean {
  const sourceMap = boolFlag(flags, 'source-map');
  const sourceMapUrls =
    flags.values.get('source-map-urls')?.value ?? 'relative';
  const embedSourceMap = boolFlag(flags, 'embed-source-map');

  if (!sourceMap) {
    if (parsedValue(flags, 'source-map-urls')) {
      throw new UsageError(
        "--source-map-urls isn't allowed with --no-source-map.",
      );
    }
    if (parsed(flags, 'embed-sources')) {
      throw new UsageError(
        "--embed-sources isn't allowed with --no-source-map.",
      );
    }
    if (parsed(flags, 'embed-source-map')) {
      throw new UsageError(
        "--embed-source-map isn't allowed with --no-source-map.",
      );
    }
  }

  const writeToStdout = sources.length === 1 && sources[0].dest === null;
  if (!writeToStdout) return sourceMap;

  if (parsedValue(flags, 'source-map-urls') && sourceMapUrls === 'relative') {
    throw new UsageError(
      "--source-map-urls=relative isn't allowed when printing to stdout.",
    );
  }
  if (embedSourceMap) return sourceMap;
  if (parsed(flags, 'source-map') && sourceMap) {
    throw new UsageError(
      'When printing to stdout, --source-map requires --embed-source-map.',
    );
  }
  if (parsedValue(flags, 'source-map-urls')) {
    throw new UsageError(
      'When printing to stdout, --source-map-urls requires --embed-source-map.',
    );
  }
  if (parsed(flags, 'embed-sources')) {
    throw new UsageError(
      'When printing to stdout, --embed-sources requires --embed-source-map.',
    );
  }
  return false;
}

// ==== resolved options =======================================================

export class CliOptions {
  readonly sources: SourceMapEntry[];
  readonly emitSourceMap: boolean;
  readonly silent: boolean;
  readonly verbose: boolean;
  readonly quietDeps: boolean;
  readonly style: Style;
  readonly charset: boolean;
  readonly emitErrorCss: boolean;
  readonly unicode: boolean;
  readonly alertColor: boolean;
  readonly trace: boolean;
  readonly stopOnError: boolean;
  readonly indented: boolean;
  readonly embedSources: boolean;
  readonly embedSourceMap: boolean;
  readonly loadPaths: string[];
  readonly nodePackageImporter: boolean;
  readonly silenceDeprecations: string[];
  readonly fatalDeprecations: (string | Version)[];
  readonly futureDeprecations: string[];
  private readonly sourceMapUrls: string;

  private constructor(
    flags: ParsedFlags,
    sources: SourceMapEntry[],
    emitSourceMap: boolean,
    supportsAnsiEscapes: boolean,
    indented: boolean,
    embedSources: boolean,
    embedSourceMap: boolean,
  ) {
    this.sources = sources;
    this.emitSourceMap = emitSourceMap;
    this.silent = boolFlag(flags, 'quiet');
    this.verbose = boolFlag(flags, 'verbose');
    this.quietDeps = boolFlag(flags, 'quiet-deps');
    this.style =
      flags.values.get('style')?.value === 'compressed'
        ? 'compressed'
        : 'expanded';
    this.charset = boolFlag(flags, 'charset');
    this.unicode = boolFlag(flags, 'unicode');
    this.alertColor = parsed(flags, 'color')
      ? boolFlag(flags, 'color')
      : supportsAnsiEscapes;
    this.trace = boolFlag(flags, 'trace');
    this.stopOnError = boolFlag(flags, 'stop-on-error');
    this.indented = indented;
    this.embedSources = embedSources;
    this.embedSourceMap = embedSourceMap;
    this.loadPaths = flags.appends.get('load-path') ?? [];
    this.nodePackageImporter =
      (flags.appends.get('pkg-importer') ?? []).length > 0;
    this.silenceDeprecations = resolveDeprecations(
      flags,
      'silence-deprecation',
    );
    this.futureDeprecations = resolveDeprecations(flags, 'future-deprecation');
    this.fatalDeprecations = resolveFatalDeprecations(flags);
    this.sourceMapUrls =
      flags.values.get('source-map-urls')?.value ?? 'relative';
    this.emitErrorCss = parsed(flags, 'error-css')
      ? boolFlag(flags, 'error-css')
      : sources.some((entry) => entry.dest !== null);
  }

  static fromParsed(
    flags: ParsedFlags,
    supportsAnsiEscapes: boolean,
  ): CliOptions {
    const stdin = boolFlag(flags, 'stdin');
    const sources = resolveSources(flags.positionals, stdin);
    const emitSourceMap = resolveEmitSourceMap(flags, sources);
    return new CliOptions(
      flags,
      sources,
      emitSourceMap,
      supportsAnsiEscapes,
      boolFlag(flags, 'indented'),
      boolFlag(flags, 'embed-sources'),
      boolFlag(flags, 'embed-source-map'),
    );
  }

  writeToStdout(): boolean {
    return this.sources.length === 1 && this.sources[0].dest === null;
  }

  /** Port of `source_map_url` (options.rs:582): make [path] absolute or
   * relative (to the directory containing [destination]) per
   * `--source-map-urls`. */
  sourceMapUrl(path: string, destination: string | null): string {
    const stripped = path.replace(/^file:\/\//, '');
    const abs = p.resolve(stripped);
    if (this.sourceMapUrls === 'relative' && !this.writeToStdout()) {
      if (destination !== null) {
        const dir = p.dirname(p.resolve(destination));
        return relativePath(dir, abs);
      }
      return stripped;
    }
    return abs;
  }
}

/** Parses the CLI argv (excluding the node + script args) into `CliOptions`.
 * Throws `UsageError` for usage errors (exit 64). */
export function parseArgs(
  argv: string[],
  supportsAnsiEscapes: boolean = process.stdout.isTTY === true,
): CliOptions {
  return CliOptions.fromParsed(parseFlags(argv), supportsAnsiEscapes);
}

// ==== help text ==============================================================

export const HELP_TEXT = `Usage: sass <input.scss> [output.css]
       sass <input.scss>:<output.css> <input/>:<output/> <dir/>

━━━ Input and Output ━━━━━━━━━━━━━━━━━━━
    --[no-]stdin               Read the stylesheet from stdin.
    --[no-]indented            Use the indented syntax for input from stdin.
-I, --load-path=<PATH>         A path to use when resolving imports.
                               May be passed multiple times.
-p, --pkg-importer=<TYPE>      Built-in importer(s) to use for pkg: URLs.
                               [node]
-s, --style=<NAME>             Output style.
                               [expanded (default), compressed]
    --[no-]charset             Emit a @charset or BOM for CSS with non-ASCII characters.
                               (defaults to on)
    --[no-]error-css           When an error occurs, emit a stylesheet describing it.
                               Defaults to true when compiling to a file.

━━━ Source Maps ━━━━━━━━━━━━━━━━━━━━━━━━
    --[no-]source-map          Whether to generate source maps.
                               (defaults to on)
    --source-map-urls          How to link from source maps to source files.
                               [relative (default), absolute]
    --[no-]embed-sources       Embed source file contents in source maps.
    --[no-]embed-source-map    Embed source map contents in CSS.

━━━ Warnings ━━━━━━━━━━━━━━━━━━━━━━━━━━━
-q, --[no-]quiet               Don't print warnings.
    --[no-]quiet-deps          Don't print compiler warnings from dependencies.
                               Stylesheets imported through load paths count as dependencies.
    --[no-]verbose             Print all deprecation warnings even when they're repetitive.
    --fatal-deprecation        Deprecations to treat as errors. You may also pass a Sass
                               version to include any behavior deprecated in or before it.
    --silence-deprecation      Deprecations to ignore.
    --future-deprecation       Opt in to a deprecation early.

━━━ Other ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    --[no-]stop-on-error       Don't compile more files once an error is encountered.
-c, --[no-]color               Whether to use terminal colors for messages.
    --[no-]unicode             Whether to use Unicode characters for messages.
    --[no-]trace               Print full stack traces for exceptions.
-h, --help                     Print this usage information.
    --version                  Print the version of Dart Sass.
`;
