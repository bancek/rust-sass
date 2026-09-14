# rust-sass-wasm

Sass compiled to WebAssembly via the Rust port, exposing the modern JavaScript
API (`compile`, `compileString`, `compileAsync`, `compileStringAsync`,
`initCompiler`/`initAsyncCompiler`, all `Value` classes, `Logger`,
`deprecations`, `NodePackageImporter`, `Exception`, …) — no Sass Embedded
Protocol, no protobuf.

See `../docs/ref/wasm.md` for the design, wire format, and gotchas.

## Layout

- `js/dist/pkg-sync/` — `wasm-pack build --target nodejs` (sync entry points;
  zero futures).
- `js/dist/pkg-async/` — `wasm-pack build --target nodejs --features async`
  (async entry points; JS callbacks may return Promises).
- `js/dist/pkg-web/` — `wasm-pack build --target web` (browser artifact; sync
  build; dev-only, not shipped — browser packaging is deferred).
- `js/dist/` — the compiled shim (package root): `index.js` (CJS), `index.mjs`
  (ESM wrapper), `index.d.ts`, `bin/sass.js` (CLI).

## Build

```sh
npm run build:rust   # rebuild pkg-sync, pkg-async, pkg-web
npm run build:js     # compile js/src -> js/dist (tsc) + package.json + index.mjs + chmod bin
```

Both are required before testing: a stale `js/dist` (no `pkg-sync/`) fails
every harness test with `MODULE_NOT_FOUND` — rebuild before trusting a red run.

No setup needed for the artifacts: `build:rust` mirrors each assembled
`js/dist/pkg-*` bundle down to `js/src/pkg-*` as plain copies (symlinks
don't work on Windows), so the vitest suite resolves `./pkg-*/…` with no
manual step.

## Verification

```sh
npm run test:js      # vitest unit/parity suite
```

Full gates go through the upstream `sass-spec` harness (from `sass-spec/`):

```sh
# wasm spec (≈16min full; append a subpath to scope):
npm run sass-spec -- --command ../rust-sass-wasm/js/dist/bin/sass.js --impl dart-sass [subpath]
# JS API surface (omit the two legacy-render .node.test.ts files):
npm run js-api-spec -- --sassPackage <abs-path-to>/rust-sass-wasm/js/dist --sassSassRepo <abs-path-to>/sass [files…]
```

Expected: spec 14263 runs / 14255 passing / 8 todo / 0 errors; `js-api-spec`
197/204 executed / 0 failures / 7 pending. Run these when the wasm
adapter/shim changes, or after large refactorings — not per core change
(the native `cargo test` + `rust-sass-spec` gates cover that). See
`../docs/ref/sass-spec.md` for all runners and the debug loop.

## Node usage

The package (`js/dist`) has a dual entry point via the `exports` map:

```js
// ESM
import { compileString } from 'sass-wasm';
// CommonJS
const { compileString } = require('sass-wasm');
```

All filesystem access (including `compile(path)`, `loadPaths`, and
`NodePackageImporter`) is Node-only, backed by `node:fs` via the io delegates
in `js/src/io/`. In a browser, filesystem operations throw (Dart parity); use
`compileString`/`compileStringAsync` with custom JS importers for browser I/O.

## CLI

The `bin` field exposes `sass` (`bin/sass.js`), a full Node CLI mirroring
`sass.js` (stdin/file/directory mode, source maps, `--style`, `-I`,
deprecation flags, exit codes 0/64/65/66):

```sh
node bin/sass.js input.scss output.css
```

## Browser

Bundle `js/dist/pkg-web/rust_sass_wasm.js` with your bundler, call its async `init()`
with the `.wasm` URL, then use the same exports from the shim:

```js
import init from 'js/dist/pkg-web/rust_sass_wasm.js';
import { compileString } from 'rust-sass-wasm';
await init(); // e.g. fetch('rust_sass_wasm_bg.wasm')
const result = compileString('a { b: c; }');
```

Browser I/O goes through the throwing `browser-fs.ts` delegates: any
filesystem-backed resolution (load paths, `compile(path)`,
`NodePackageImporter`) throws Dart's `"… is only supported on Node.js"` error.
Custom JS importers (`canonicalize`/`load`/`findFileUrl`) are the portable I/O
path in both environments.

The `pkg-web` artifact is the sync build. For async entry points in the
browser, build the `--features async` variant (`wasm-pack build --target web
--features async`) and bundle it the same way. There is no automated browser
runtime test suite — the live browser surface is the **playground** below.

## Playground

A dependency-free single-page playground exercises the browser artifact:
`playground/index.html` compiles the source `<textarea>` (debounced) on every change
via the public `compileString` on `pkg-web`, with a syntax/style/source-map/
`alertColor`/`alertAscii` options panel, a CSS output pane, and a colored
warnings-and-errors console. Dogfooding: the page's own stylesheet is Sass —
an inlined `#sass-chrome` block (with `sass:color` palette derivation) that
the page compiles with the same wasm after `init()` and injects as a `<style>`;
a tiny hand-written splash stylesheet shows "Loading…" until that finishes.

```sh
npm run build:rust      # regenerates js/dist/pkg-web (and pkg-sync/pkg-async)
npm run build:playground  # copies js/dist/pkg-web's rust_sass_wasm.js + .wasm into playground/
npx serve playground           # or caddy, python http.server, etc.
```

`npm run build:playground:release` builds the same chain with
`wasm-pack --release` (what the Pages publish workflow deploys).

The two copied wasm artifacts in `playground/` are gitignored; `index.html` is
tracked.

## Colors in messages (`alertColor` / `alertAscii`)

Mirroring the reference `sass` API (explicit-only; no tty detection):

- `alertColor: true` renders the thrown exception's `formatted`/`message` with
  ANSI color codes, and renders the **default-logger** warning/deprecation/debug
  blocks (used when a user `Logger` leaves `warn`/`debug` undefined, or none is
  given) in color — forwarded through `console.warn`/`console.error` (stderr in
  Node).
- `alertAscii: true` switches the source-frame glyphs to ASCII.
- Messages passed to a user-provided `Logger.warn`/`debug` stay plain, matching
  `sass`.

## License

MIT.
