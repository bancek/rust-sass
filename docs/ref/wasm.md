# Module: `rust-sass-wasm`

The modern Sass JS API compiled to WebAssembly: `compile`, `compileString`,
`compileAsync`, `Compiler`, importers, custom functions, the value classes, and
the Node CLI.

## Approach

- **Bridge:** direct `wasm-bindgen` + `serde-wasm-bindgen` — **not** the
  embedded protocol (protobuf). Data crosses as tagged plain-JS objects.
- **Two artifacts** from one source: a zero-future **sync** build (`pkg-sync`)
  and an **async** build (`pkg-async`), selected by `rust-sass/async`; plus a web
  target (`pkg-web`). No `block_on` anywhere — the sync entry points cannot block.
- **Value classes** (`SassNumber`, `SassColor`, …) and `deprecations`/`version`
  are reused from the vendored `embedded-host-node` package (pure JS, MIT), not
  reimplemented.

## Decisions

| #   | Decision                                                                                                         |
| --- | ---------------------------------------------------------------------------------------------------------------- |
| D1  | Direct wasm-bindgen + serde-wasm-bindgen; no protobuf, no varint framing.                                        |
| D2  | Two artifacts from one source; no `block_on`.                                                                    |
| D3  | Reuse `embedded-host-node` value classes, deprecations, and version.                                             |
| D4  | A `marshaller.rs` mirroring `protofier.rs` case-for-case.                                                        |
| D5  | Modern API only — no legacy `render`/`renderSync`/`sass.types.*`, no `sass-parser`.                              |
| D6  | Browser = `compileString*` + JS importers (fs throws); Node adds `compile*`, `loadPaths`, `NodePackageImporter`. |
| D7  | JS callbacks are plain `js_sys::Function` extracted from the options object.                                     |
| D8  | Rust throws a plain data object; the shim builds the `Exception` class.                                          |
| D9  | The shim accepts the documented options; Rust parses them manually via `js_sys`.                                 |

D2 (the two-artifact split) is the intended end state, not a workaround. The
sync build must be genuinely zero-future: driving an async pipeline with
`block_on` measures ~30% slower, and a wasm sync entry point _cannot_ block (no
executor, no microtask pumping, no SharedArrayBuffer). Sync and async `Value`
are **structurally different types** (async `Value` carries async callables), so
a single binary would need the marshaller and JS glue written twice. Packaging
alternatives were rejected: wrapper crates recompile all files twice _and_ force
wasm-glue duplication; `Value<Fn>` generics drag `'compile` into `Value`; a
`FunctionRef` trait needs `dyn Any`; and parse/value/serialize can't be split
out (one SCC).

## Value marshalling wire format

Values cross as tagged objects:

```
{type: "boolean", value: bool}
{type: "null"}
{type: "string", text: string, quoted: bool}
{type: "number", value, numeratorUnits: [], denominatorUnits: []}
{type: "color", space, channel1?, channel2?, channel3?, alpha?, missing: bool[]}
{type: "list", separator: "space"|"comma"|"slash"|"undecided", hasBrackets, contents}
{type: "argumentList", id, separator, contents, keywords: {name: value}}
{type: "map", entries: [{key, value}]}
{type: "calculation", name, arguments: CalcArg[]}
{type: "function"|"mixin", id}
{type: "hostFunction", signature, callback}
```

`CalcArg` is `{type: "number"|"string"|"interpolation"|"operation"|"calculation",
...}`, with operations carrying `operator: "plus"|"minus"|"times"|"dividedBy"`.

Semantics to preserve: an empty list keeps its separator/brackets; `Undecided`
with >1 element errors; `argumentList.id` is 1-based (0 = built fresh), and a
non-zero id on the way in returns the stored clone (shared `keywordsAccessed`
flag); function/mixin ids are per-compilation and an unknown id errors;
`hostFunction` is JS→Rust only (Rust never emits it); missing color channels
mirror `channelN == null` (alpha absent ⇒ missing alpha); calculation names
validate arities. The options/result/error/importer-result/canonicalize-context
objects use analogous plain-object shapes.

## Io bridge

The compile-path `Io` is minimal (`read_file`, `file_exists`, `dir_exists`,
`link_exists`, `canonicalize`, `current_dir`, `is_macos`, `is_windows`,
`supports_ansi_escapes`); CLI/spec-only operations moved to `IoExt`. The JS
delegate is read from `options.io` (wasm-only): the Node shim injects a
`node:fs` delegate; the browser uses throwing delegates. `real_case_path` and
`clean_path` are shared in Rust — there is no `canonicalize` in the JS delegate;
`JsIo` owns it via `read_dir(path): string[]`, which feeds the _sync_
`real_case_path` recursion (the reason it lives in Rust is to preserve symlink
directory names on case-insensitive filesystems; `node_package.rs` uses lexical
`clean_path` for exports/root-values).

The delegates split sync/async: the async delegate uses `fs.promises` (methods
always return `Promise<T>`); `Io`'s own sync methods (`current_dir`, `is_macos`,
…) stay sync. `file_exists`/`dir_exists`/`link_exists` return `Result<bool,
IoError>` (ENOENT → `Ok(false)`, other errors rethrown); errors are
`IoError`-shaped `{message, kind, path?}` with node `code` mapped to
`NotFound`/`Permission`/`AlreadyExists`/`Other`.

## Extension options

Four rust-sass extensions (additive, ignored by `sass.js`, never affect CSS or
source maps):

- **`io`** — an explicit filesystem delegate. It is wasm-only because `sass.js`
  is a single dart2js build with runtime env detection and **no io argument**.
- **`ascii`** — sets `CompileOptions.unicode = false` (eval-time glyph baking is
  ASCII).
- **`color`** — renders ANSI at error-render time.
- **`consoleWarn`** / **`consoleDebug`** — fallback-only sinks: when a user
  `Logger` exists its methods still receive plain messages; the sinks replace
  only the `console.warn`/`console.error` fallback.

`alertColor`/`alertAscii` are **render-time only** — they never write
`CompileOptions.alert_color`/`alert_ascii` (which would change eval-time
message baking). Effective flags: `ascii_effective = ascii || alertAscii`,
`color_effective = color || alertColor`.

## Node CLI and export surface

The wasm exports are `compileString` / `compile` / `compile_bytes` (all
`(input, options)`; the old positional io and the `compile_string`/`compile_string_cli`
prototype exports were removed). `compile_bytes` is the sole CLI export — the
Node CLI reads file/stdin bytes itself and passes them through (invalid UTF-8 →
Rust `Invalid UTF-8.`). The shim provides `bin/sass.js` (which the 14k runner
spawns directly, so it is `chmod +x`), mirroring `sass.js` parity: no
`Compiled X to Y.` line on success, `process.exitCode` (not `process.exit`), the
`%2A/` `sourceMappingURL` escape, `--embed-source-map` percent-encoding with the
restricted `data:` set, `--source-map-urls relative` remapping only `file:`, and
`--pkg-importer node` → `{__sassNodePackageImporter: cwd}`. `--version` prints
`1.104.0 compiled with dart2js 3.13.3` (the reference `sass` info string, single-sourced in `version.ts`);
`--quiet` → `Logger.silent` → `QuietLogger`; a Sass error exits `65` (writes
`errorCss` for `--error-css`, `formatted` + `"\n"` to stderr); a file-read
failure exits `66`. The sync artifact has no `--watch`.

## wasm-specific seams

- **`SassTime`** — `SystemTime::now()` panics on wasm32, so the clock is a
  host-set seam (`set_time_now`, an i64-ms value).
- **`glibc-math`** is forwarded so wasm `pow` bit-matches native libm (see
  `math.md`).

## Source-map parity

A url-less `compileString` emits `sources` as `data:;charset=utf-8,` with a
restricted `data:` encode set (unreserved + `!$&'()*+,/:;=?@`), and `mappings`
are byte-identical to `sass`. `sourcesContent` is emitted only with
`sourceMapIncludeSources` (default off, matching `sass`).

## Gotchas (selected)

- No `block_on`; the sync artifact must contain zero futures.
- Promise detection in sync entry points throws Dart's exact
  `"can't return a Promise for synchronous compile functions"`.
- Every `Value<'parse>` returned to Sass must be built into the compile arena.
- `SassUrl → JS` uses `Display`, never `as_str()` (which leaks the
  `sass-relative:` prefix).
- `cfg!` is not conditional compilation — use `#[cfg]`/`#[async_impl]`/
  `#[sync_impl]` to gate two variants.
- `canonicalize` must mark `containing_url` accessed when present (the
  dart-sass#2208 caching fix).
- Error fidelity: `SassError::to_error_string(io)` produces `formatted`; strip
  the `Error: ` prefix for the JS `message`.
- `loadedUrls` must be JS `URL` objects (`new URL(url)`).
- Source-map JSON: `SingleMapping::json()` → `JSON.parse` to a plain object;
  omit the field when `sourceMap: false`.
- `Logger.silent` maps to `QuietLogger`.
- Importer results cross the wire RAW; `String::from(JsString)` panics on
  non-strings — stringify via the JS `String(v)` constructor.
- Exact importer/function messages (including the `findFileUrl` "synchron"
  typo and `nonCanonicalScheme`/scheme validation) are Dart-exact.
- `alertColor`/`alertAscii` color the thrown `Exception.formatted`; a
  user-provided `Logger.warn`/`debug` always receives the plain message.

## Build artifacts

The published package (`js/dist`, the package root) ships two wasm bundles:

- `pkg-sync/` — `wasm-pack build --target nodejs` (sync entry points;
  zero futures).
- `pkg-async/` — `wasm-pack build --target nodejs --features async`
  (async entry points; JS callbacks may return Promises).

`pkg-web/` (`wasm-pack build --target web`, sync build) is dev-only and never
shipped — browser packaging is deferred. The rest of `js/dist/` is the
compiled shim: `index.js` (CJS), `index.mjs` (ESM wrapper), `index.d.ts`,
`bin/sass.js` (CLI).

`build:rust` mirrors each assembled `js/dist/pkg-*` bundle down to
`js/src/pkg-*` as plain copies (symlinks don't work on Windows), so the
vitest suite resolves `./pkg-*/…` with no manual step. The build/test command
sequence lives in `CONTRIBUTING.md` (`rust-sass-wasm`); both builds are
required before testing — a stale `js/dist` (no `pkg-sync/`) fails every
harness test with `MODULE_NOT_FOUND`.

## Playground

A dependency-free single-page playground exercises the browser artifact:
`playground/index.html` compiles the source `<textarea>` (debounced) on every
change via the public `compileString` on `pkg-web`, with a syntax/style/
source-map/`alertColor`/`alertAscii` options panel, a CSS output pane, and a
colored warnings-and-errors console. Dogfooding: the page's own stylesheet is
Sass — an inlined `#sass-chrome` block (with `sass:color` palette derivation)
that the page compiles with the same wasm after `init()` and injects as a
`<style>`; a tiny hand-written splash stylesheet shows "Loading…" until that
finishes.

```sh
npm run build:rust        # regenerates js/dist/pkg-web (and pkg-sync/pkg-async)
npm run build:playground  # copies js/dist/pkg-web's rust_sass_wasm.js + .wasm into playground/
npx serve playground      # or caddy, python http.server, etc.
```

`npm run build:playground:release` builds the same chain with
`wasm-pack --release` (what the Pages publish workflow deploys).

The two copied wasm artifacts in `playground/` are gitignored; `index.html` is
tracked. There is no automated browser runtime test suite — the live browser
surface is this playground.

## File mapping

| Dart                | Go             | Rust                                                    |
| ------------------- | -------------- | ------------------------------------------------------- |
| `lib/src/js/*.dart` | — (no Go wasm) | `rust-sass-wasm/src/*.rs`, `rust-sass-wasm/js/src/*.ts` |
