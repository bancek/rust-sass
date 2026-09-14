# The `sass-spec` suite: layout, `.hrx` format, and debugging failures

The definitive correctness gate: thousands of small compilations with expected
outputs. There are **three ways to run it** (next section) — the native Rust
runner is the per-change gate; the wasm runners verify the JS bridge. This page
explains all three, the spec layout, the `.hrx` archive format,
`options.yml`, how the native runner compares, and the debug loop for a
failing test. For commands, see `CONTRIBUTING.md` Testing.

## Ways to run (and when)

| #   | Runner                           | Command (cwd)                                                                                                                                                     | What it exercises                                                          | Run when                                                                                                     |
| --- | -------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| 1   | Native Rust (`rust-sass-spec`)   | `cargo test -p rust-sass-spec --test runner_test --release -- --ignored` (repo root; `SASS_SPEC=<subpath>` for a subset)                                          | `rust-sass` compiled in-process (`VirtualIo`, `cwd` = test dir)            | after **every** core change — the per-change gate                                                            |
| 2   | Upstream harness → native binary | `npm run sass-spec -- --command ../target/release/rust-sass --impl dart-sass [subpath]` (from `sass-spec/`; needs `cargo build --release -p rust-sass-cli` first) | the release CLI end to end, incl. CLI arg handling and real process I/O    | fast full-suite cross-check after larger changes (full suite ≈44s)                                           |
| 3   | Upstream harness → wasm CLI      | `npm run sass-spec -- --command ../rust-sass-wasm/js/dist/bin/sass.js --impl dart-sass [subpath]` (from `sass-spec/`)                                             | the built wasm `bin/sass.js` end to end (JS shim + wasm bridge + compiler) | when the **wasm adapter/shim changes**, or after **large refactorings** — not per change (full suite ≈16min) |
| 4   | `js-api-spec` → wasm build       | `npm run js-api-spec -- --sassPackage <rust-sass-wasm/js/dist> --sassSassRepo <sass-repo> [files…]` (from `sass-spec/`)                                           | the JS API surface (`compile`, values, importers, logger, …)               | same as 3, plus JS API changes                                                                               |

Runners 2–4 go through the upstream `sass-spec` harness (`sass-spec.ts`,
`--command` + `--impl` + optional positional subpath filter); runner 1 is
ours. Prerequisites for 3 and 4: `npm run build:rust` in `rust-sass-wasm/`
(builds `pkg-sync`/`pkg-async`/`pkg-web` with `glibc-math`), then `npm run
build:js` (`tsc` → `js/dist`, including `bin/sass.js`). A stale `js/dist`
(without `pkg-sync/`) fails every test with `MODULE_NOT_FOUND` — rebuild
before trusting a red run. Scripts live in `rust-sass-wasm/package.json`,
not `js/package.json`. Expected results: 14263 runs / 14255 passing /
8 todo / 0 errors (verified identically via runners 2 and 3);
`js-api-spec` 197/204 executed / 0 failures / 7 pending (omit the two
legacy-`render` `.node.test.ts` files — legacy API is out of scope).

`--impl dart-sass` matters: it selects the expectation files and the
`:todo:`/`:ignore_for:` matching used for Dart Sass. A trailing subpath
(e.g. `spec/core_functions/color/to_space`) scopes the run like
`SASS_SPEC=` does for the native runner.

Cleanup note: sass-spec tooling can materialize stray untracked dirs (e.g.
`spec/core_functions/color/to_space/xyz/oklch/`); remove them before full
runs so they don't pollute results.

## Layout

`sass-spec/spec/` is a tree of _test directories_ and `.hrx` files:

- A **test directory** is any directory containing `input.scss` (or
  `input.sass`). Siblings provide expectations: `output.css` for success,
  `error` for failure, `warning` for expected warnings. Extra files
  (`_partial.scss`, subdirectories) are importable fixtures.
- An **`.hrx` file** packs several tests (or fixtures) into one file; each
  archived test dir inside it works exactly like a physical test directory.
- `options.yml` files at any level adjust behavior for everything below
  (inherited down, parent + child merged — see below).

Top-level groups (use as `SASS_SPEC=<path>`, which appends to `sass-spec/spec/`):

| Path                                 | Contents                                                                                                                        |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------- |
| `core_functions/`                    | built-in modules (`color/`, `math/`, `map/`, `list`, …)                                                                         |
| `css/`                               | plain-CSS parsing, `@media`, `@supports`, comments, `plain/`                                                                    |
| `directives/`                        | `@use`, `@forward`, `@import`, `@extend`, `@media`, `@at-root`, … (single-case `.hrx` files like `each.hrx` live here directly) |
| `expressions/`, `operators/`         | expression semantics, arithmetic                                                                                                |
| `values/`                            | `calculation/`, `colors/`, `lists/`, `maps/`, `numbers/`                                                                        |
| `variables/`, `callable/`, `parser/` | scoping, callables, syntax edge cases                                                                                           |
| `libsass*`, `non_conformant/`        | legacy / known-divergent (mostly `ignore_for` / `todo`)                                                                         |

## The `.hrx` format

A multi-file archive: sections delimited by `<===> path`, one file per
section (parsed by `rust-sass-spec/src/hrx.rs`, a port of node-hrx):

```
<===> leading/input.scss
a {b: c}
> d {@extend a}

<===> leading/output.css
a, > d {
  b: c;
}

<===> leading/warning
DEPRECATION WARNING [bogus-combinators]: ...
```

Rules:

- `<===>` + whitespace + relative path starts a section; a trailing single
  newline of each section is stripped.
- A bare `<===>` line (or `<===> ===…`) is a comment/separator and carries
  no file — used with a `====…` rule line between archived tests.
- Nested paths (`leading/input.scss`) become nested test dirs; each dir with
  an `input.scss`/`input.sass` is an independent test.
- An `options.yml` section inside the archive applies to that archive, and a
  per-test-dir `options.yml` section applies to just that test.
- Sibling files on disk next to the `.hrx` are visible to its tests (they do
  not override same-named archived files).

## `options.yml`

```yaml
:todo:
  - dart-sass
:warning_todo:
  - dart-sass
:ignore_for:
  - libsass
:precision: 10
```

Parsed by `rust-sass-spec/src/options.rs`:

| Key              | Meaning for our runner (`IMPL_NAMES = ["dart-sass-rust", "dart-sass"]`)                          |
| ---------------- | ------------------------------------------------------------------------------------------------ |
| `:todo:`         | test counts as **passed** without running (known failure; the wasm line reports these as "todo") |
| `:warning_todo:` | CSS is compared but the `warning` file is **not**                                                |
| `:ignore_for:`   | test is **skipped** entirely (not counted as passed)                                             |
| `:precision:`    | child overrides parent when nonzero; lists concatenate parent + child                            |

Matching is **substring**: an entry `dart-sass-rust` matches impl `dart-sass`.
Options merge down the directory tree (and archive → test dir inside `.hrx`).

## How our runner compares (`rust-sass-spec/src/runner.rs`)

1. **Isolation** — each test compiles in a `VirtualIo` preloaded with the
   test dir's files, `cwd` set to the test dir, `DefaultIo` as fallback for
   unknown paths, and `load_paths = [spec_root]`. Logger is a capturing
   `TestLogger`; options are `verbose: true, unicode: false, charset: true`.
2. **Expectation lookup** — impl-specific overrides first:
   `output-dart-sass-rust.css`, then `output-dart-sass.css`, then
   `output.css` (same scheme for `error`/`warning`).
3. **Normalization** (`normalize_output`): `\r\n` → `\n`, consecutive
   newlines collapsed to one, full input paths replaced with basenames
   (`…/input.scss` → `input.scss`), surrounding whitespace trimmed. Both
   sides go through it — trailing-newline and path differences never fail.
4. **CSS path** — compile must succeed; normalized actual vs expected must
   match byte-for-byte, else `CSS mismatch:` with both texts. Then, if a
   `warning` file exists (and no `:warning_todo:`), normalized captured
   warnings must match, else `warning mismatch:`.
5. **Error path** — compile must fail; compared text is
   **warnings + error message** (deprecations emitted before the error count),
   rendered with ASCII glyphs (`unicode: false`, like the reference
   `--no-unicode` run). Mismatch → `error mismatch:`; success → `expected
error but compilation succeeded`.
6. **Neither file** — `test has no expected output` failure (the test itself
   is malformed).
7. **Panics** are caught (`catch_unwind`, hook silenced) and reported as
   `…: panic: <msg>` failures, never aborting the run. Progress lines
   (`[n] <path> ... ok/FAIL/todo/ignored`) go to stderr; the summary prints
   `passed/total` plus warnings/errors-compared counts, and asserts both
   warning and error comparisons actually ran (guards against a corpus
   change silently dropping them).

## Debugging a failure

### Step 1: identify the failure type

| Runner output                              | Meaning                                              | `runner.rs` |
| ------------------------------------------ | ---------------------------------------------------- | ----------- |
| `…: panic: …`                              | the compiler panicked (bug — panics are never valid) | `:594`      |
| `unexpected error: …`                      | threw an error; test expected CSS                    | `:639`      |
| `CSS mismatch:`                            | both produced CSS but differ                         | `:616`      |
| `warning mismatch:`                        | CSS matches, warnings differ                         | `:629`      |
| `error mismatch:`                          | both errored but the text differs                    | `:669`      |
| `expected error but compilation succeeded` | threw nothing; test expected `error`                 | `:651`      |

### Step 2: reproduce in isolation

Run the single test, then shrink to stdin on both compilers:

```sh
SASS_SPEC=<path/to/test-dir-or.hrx> cargo test -p rust-sass-spec --test runner_test --release -- --ignored
echo '<minimal scss>' | cargo run -p rust-sass-cli -- --stdin 2>&1
echo '<minimal scss>' | dart run bin/sass.dart --stdin 2>&1   # from dart-sass/
```

The CLI reads the same code path as the runner except for the harness
(`VirtualIo`, `cwd`, `unicode: false`, `verbose`); if stdin and spec
disagree, suspect `cwd`-relative paths, `load_paths`, or warning capture —
not the compiler.

### Step 3: find the code

| Failure smells like…                   | Look in                                                              |
| -------------------------------------- | -------------------------------------------------------------------- |
| wrong error text/span, missing trace   | `eval/helpers.rs` (`exception`, `add_exception_span`)                |
| wrong CSS for a construct              | `eval/statement.rs` / `expression.rs` / `css.rs` per construct       |
| warning text/span/dedup                | `eval/warn.rs`, `logger/`                                            |
| `@use`/`@forward`/`@import` resolution | `eval/import_cache.rs`, `eval/importer/`, `docs/ref/importer.md`     |
| built-in function behavior             | `functions/<module>.rs`, `docs/ref/functions.md`                     |
| parse error or shape                   | `parse/`, `docs/ref/parse.md`                                        |
| number/color output                    | `value/`, `docs/ref/value.md`                                        |
| selector/extend output                 | `selector/`, `extend/`, `docs/ref/selector.md`, `docs/ref/extend.md` |

Every Rust file carries a `// dart-source:` annotation — open the Dart file
and compare. For the full port-review procedure, see `docs/review.md`.

### Step 4: fix and verify

Fix narrowly against Dart's behavior, add a unit regression test
(`CONTRIBUTING.md` Testing), then re-run the single spec test, the group,
and — at the end of the change — the full suite. A spec failure that
requires changing an expectation file (`output.css`/`error`/`warning` upstream)
is almost certainly a wrong fix: expectations are Dart's output.

## `:todo:` vs `:ignore_for:` vs `warning_todo`

- `:todo:` containing our impl name → counted passed, never run. Used for
  known open bugs (the wasm spec line reports these as "todo"). Fixing the
  bug means the test starts running — no bookkeeping needed.
- `:ignore_for:` → skipped before counting. Used for out-of-scope
  implementations (mostly `libsass`).
- `:warning_todo:` → CSS compared, warnings not. Used while warning text
  lags behind correct output.

## File mapping

| Concern                | Code                                                                      |
| ---------------------- | ------------------------------------------------------------------------- |
| test walk + comparison | `rust-sass-spec/src/runner.rs`                                            |
| `.hrx` parsing         | `rust-sass-spec/src/hrx.rs` (port of node-hrx)                            |
| `options.yml`          | `rust-sass-spec/src/options.rs` (port of `lib/spec-directory/options.ts`) |
| test entry             | `rust-sass-spec/tests/runner_test.rs` (`SASS_SPEC`/`SASS_SPEC_PATH`)      |
| fixtures               | `sass-spec/spec/` (submodule)                                             |
