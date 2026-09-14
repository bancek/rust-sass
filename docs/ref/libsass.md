# Module: `rust-sass-libsass`

The libsass C API implemented on `rust-sass`: a drop-in `libsass.{so,dylib,a}`
plus `include/sass/*.h` tree built from the modern Sass engine, so existing
native consumers (sassc, node-sass, language bindings) link unmodified.

## What it is

**Contract: links like libsass, behaves like Dart Sass.** Same symbols,
signatures, ownership rules, and error shapes (ABI-compatible); modern language
semantics underneath (semantics-modernized). Diffs vs real libsass output are
expected and documented, not bugs.

- `rust-sass-libsass` — the adapter: `#[no_mangle] extern "C"` functions
  over the `rust-sass` sync API. `[lib] name = "sass"`,
  `crate-type = ["cdylib", "staticlib", "rlib"]`, so the artifact is
  literally `libsass.so` / `libsass.dylib` / `libsass.a`. Sync-only: no
  `maybe_async` (consumers parallelize themselves, cf. node-sass
  `uv_queue_work`). The `async` cargo feature is a graph-consistency stub —
  see §Async feature below.
- `rust-sass-libsass-tests` — integration tests through the C ABI, run
  identically against our library (`default` features) and upstream libsass
  (`--no-default-features --features upstream`).

Windows ships too (reversal 2026-09-13: release demand; x64/arm64 only,
where `extern "C"` is unambiguous): MSVC cargo emits `sass.dll` +
`sass.dll.lib` (import library — embeds the DLL name, never rename) +
`sass.lib` (static archive), with no filename collision. Still out of scope:
plugin loading (accepted-but-empty stub); async builds. The `sass2scss()`
converter ships (`sass2scss.rs` line-for-line port + `sass2scss_version()`).
See §9 for the deferred list.

## Release binaries

`release.yml` publishes one self-contained
`rust-sass-libsass-<version>-<triple>.zip` per triple (linux-x64/arm64,
linux-musl-x64/arm64, darwin-x64/arm64, win32-x64/arm64; `<version>` is the
workspace train, not the 3.6.6 ABI fiction): `include/` (pinned upstream
headers; `sass/version.h` generated from `version.h.in` stamped with the ABI
version from `base.rs`, never the submodule's `[NA]` placeholder) + `lib/`
(cargo's native filenames, never renamed). musl triples ship build-gated
only — a gnu runner cannot execute or link-test musl binaries — and
static-only: rustc drops `cdylib` for musl targets (warning, not error), so
no `libsass.so` exists there.

## Layout

| Module         | Implements                                                                  |
| -------------- | --------------------------------------------------------------------------- |
| `base.rs`      | `sass/base.h`: alloc/copy/free, quote helpers, versions                     |
| `sass2scss.rs` | `sass2scss.h`: indented-Sass→SCSS converter + option constants              |
| `options.rs`   | `Sass_Options`: all setters/getters, path lists, `c_*` slots                |
| `context.rs`   | file/data contexts, staged compiler, take_* transfer, error/source-map JSON |
| `values.rs`    | `Sass_Value` union lifecycle (make/get/set/clone/delete)                    |
| `functions.rs` | custom functions: entries, signature parsing, value bridge                  |
| `importers.rs` | custom importers: entries, results, filesystem fallback                     |
| `env.rs`       | callee snapshots and `Sass_Env` variable access                             |

**Safety boundary:** this is the only workspace crate allowed `unsafe` code
and a `libc` dependency — raw C pointers, NUL-terminated strings,
malloc/free ownership, and `catch_unwind` at every `extern "C"` boundary
live here so the core stays `unsafe`-free. Each public function documents
its pointer contract under `# Safety`.

## Decisions

| #   | Decision                                                                         | Rationale                                                                                                                                                                                             |
| --- | -------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| D1  | ABI-compatible, semantics-modernized                                             | Emulating frozen libsass quirks would be a second compiler and contradicts byte-identity-to-Dart                                                                                                      |
| D2  | `libsass_version()` → `"3.6.6"`                                                  | The known-max release: passes every parser/comparison style (`split('.').map(int)`, `>= 3.x`, lexicographic, prefix, upper-bound caps). Answers "which libsass is this ABI compatible with"           |
| D3  | `libsass_language_version()` → `"3.5"`                                           | Zero parser risk; the engine is a superset                                                                                                                                                            |
| D4  | Precision accepted, ignored                                                      | Dart `PRECISION=10` is a language constant woven through fuzzy eval + serializer; parameterizing forks byte-identity. sassc sets 10 (zero divergence); node-sass defaults to 5 (one divergences line) |
| D5  | NESTED→Nested (exact), EXPANDED→Expanded, COMPACT→Compact, COMPRESSED→Compressed | `OutputStyle::Nested` matches upstream byte-for-byte via evaluator-stamped `tabs`; `OutputStyle::Compact` renders one block per line (see `divergences.md`)                                           |
| D6  | `source_comments` honored                                                        | Spans were retained all along — one emission site in the style-rule visitor                                                                                                                           |
| D7  | Plugins accepted-but-empty                                                       | Liability, not a feature                                                                                                                                                                              |
| D8  | `sass2scss()` shipped                                                            | Line-for-line port of `sass2scss.cpp` in `sass2scss.rs` (std-only text machine; `parse+serialize` stays wrong for converters)                                                                         |
| D9  | Sync only                                                                        | Consumers self-parallelize; `Rc`/`!Send` core is fine                                                                                                                                                 |
| D10 | All cross-boundary pointers from libc `malloc`                                   | Consumers `free()` our pointers and hand us `malloc`'d buffers. EXCEPTION: `indent`/`linefeed` are borrowed, never freed                                                                              |
| D11 | Contract tests identical on both legs; goldens gate ours only                    | Cross-leg CSS byte-compare is wrong by design                                                                                                                                                         |
| D12 | Committed bindgen output                                                         | Headers frozen (upstream archived 2021): generate once, no libclang in CI                                                                                                                             |
| D13 | Upstream via submodule + env-var override                                        | Consistent with existing submodules                                                                                                                                                                   |
| D14 | `error_json` field-fidelity is load-bearing                                      | node-sass `JSON.parse`s it: `{status,file,line,column,message,formatted}`                                                                                                                             |
| D15 | `c_headers` stored, ignored                                                      | Only in-tree producer is plugins (stubbed); zero surveyed consumers                                                                                                                                   |
| D16 | Bridged `c_importers` stable-sorted by descending `priority`                     | Mirrors `sort_importers`; load-bearing for multi-importer bindings                                                                                                                                    |

## Option mapping

| C setter                                                            | Rust target                   | Rule                                                                                                    |
| ------------------------------------------------------------------- | ----------------------------- | ------------------------------------------------------------------------------------------------------- |
| output_style                                                        | `CompileOptions.style`        | Per D5 (constants 0–3 are ABI — node-sass casts ints)                                                   |
| precision                                                           | stored, returned, ignored     | D4                                                                                                      |
| indent                                                              | `use_spaces` + `indent_width` | all-spaces→`(true,count)`; all-tabs→`(false,count)`; else `(true,2)`; clamp 10                          |
| linefeed                                                            | `LineFeed` const              | exact bytes for `lf/crlf/cr/lfcr`, else LF fallback (`text` is `&'static`)                              |
| source_comments                                                     | serializer flag               | D6 (1-based lines, `stdin` fallback, absolute file paths)                                               |
| source_map_embed/contents/file_urls, omit_source_map_url, file/root | map JSON + comments           | `file`/`sources`/comment go through `abs2rel`; embed emits base64 data-URL; omit suppresses the comment |
| input/output_path                                                   | URL base + map links          | Links only — libsass never writes output itself                                                         |
| include_path (+push) / SASS_PATH                                    | `load_paths` (`:`-split)      | NULL-safe                                                                                               |
| plugin_path (+push)                                                 | stored                        | D7 stub                                                                                                 |
| is_indented_syntax_src                                              | `Syntax::Sass`                | Native parser, no converter detour                                                                      |
| c_functions/c_importers                                             | bridges (see Callbacks)       | D16 sort                                                                                                |
| c_headers                                                           | stored, ignored               | D15 stub                                                                                                |
| find_file/find_include (+compiler variants)                         | import-path probing           | Best-effort subset (exact + extensions + `_partial`); miss returns an empty-string copy, never NULL     |

## Ownership across the boundary

- Makers copy, and fail NULL on NULL input (`make_number(v,NULL)`→NULL;
  unitless is `""`); setters take ownership (`set_unit/set_value/
set_message`, `list/map_set_*` adopt, freeing the old value first —
  upstream leaks there, unobservable except to leak checkers).
- `set_options` moves (emptied shell stays deletable); `take_*` returns +
  nulls; deletes are idempotent and NULL-safe (hardened past upstream UB —
  ours must not crash where libsass UBs).
- Staged compilers borrow their context (`sass_delete_compiler` frees the
  handle only; the context must outlive the compiler — deleting it first is
  upstream-UB too).
- Callback argv is borrowed (read, never delete); callback returns transfer
  to the compiler (never delete). C error/warning values are the failure
  idiom (never NULL): `sass_value_op`/`stringify` failures, env misses.
- Callee/import pointers are borrowed for the call (snapshot the strings —
  they point into AST storage); `get_last_*` on an empty stack returns
  NULL instead of crashing.
- Output and `error_message` strings carry libsass's trailing newlines
  (byte-parity for direct getter consumers; the harness strips them).

## Callbacks

**Staged vs callback handles.** Upstream uses one `Sass_Compiler` struct for
both jobs; the adapter splits them: `StagedCompilerBox` drives the
make→parse→execute lifecycle only, while each callback receives an ephemeral
`CompilerBox` snapshot token (never the staged handle). Consequently the
`sass_compiler_get_*_import/callee` getters take the token type and
`get_state/context/options` take the staged type — same C names as upstream,
different Rust types per role.

**Custom functions.** Strict signature pre-parse (embedded-style
`Invalid signature` errors, never raw strings into the callable builder —
with one carve-out: bare `...`, which node-sass generates for
signature-less functions, parses as rest-only). Argv marshals to a C comma
list; results convert back with ownership to the compiler (alias-guarded
cleanup); C error/warning returns raise `"error in C function {name}:
{msg}"` at the call site; NULL returns harden to an error.

**Custom importers.** The callback return maps onto
canonicalize (`Some` absolute / `None` fallthrough — empty lists fall
through too; upstream's hide-import quirk is deliberately not mirrored) +
load (`ImporterResult` / `None` / `Err(Script)`). A `srcmap` payload
becomes a `data:` URL (the core models locations, not bytes). Path-only
returns resolve via the normal filesystem flow; error entries abort with
message (+line/col when not `-1`); buffers are copied out before the
wrapper list is deleted. One C call is one canonicalization: multi-entry
lists fail fast. `prev` reports the plain path the importer returned,
never a `file:` URL. Dispatch is customs-first for `@import` (opt-in
raw+`prev` consultation); non-opt-in hosts keep Dart's base-first order.

**Callee/env.** Each callback gets a compiler token snapshotting the
callee chain (name/path/1-based line-col/type from the declaration;
import/load frames skipped — upstream has none) plus the live env. Reads:
lexical (no default-insert — safer than upstream), current-frame-only,
global (frame 0 + module fallback); `$`-prefixed and bare names both
accepted. Writes: lexical writes through to the caller frame (probed
upstream behavior — no fresh scope), global writes frame 0, C-origin
values use `BOGUS_SPAN` (module-conflict setter errors have no channel
and are swallowed). All callee envs of one callback alias the live
env (core frames carry no per-frame envs).

## Verification

```sh
cargo test -p rust-sass-libsass                          # adapter unit tests
cargo test -p rust-sass-libsass-tests                    # contract suite, our leg
cargo test -p rust-sass-libsass-tests --no-default-features --features upstream  # upstream leg
cargo clippy -p rust-sass-libsass -p rust-sass-libsass-tests --all-targets      # zero warnings
./rust-sass-libsass-tests/tool/build-sassc.sh [--run [subpath]]  # sassc gate (wrapper forces `-t expanded`: the harness passes no --style flag, so the default governs — nested itself is covered by the node-sass gate + contracts)
./rust-sass-libsass-tests/tool/build-node-sass.sh [--run]        # node-sass gate (api.js + non-watch CLI subset)
```

Standing results: sassc suite matches the native baseline (minus the
permanent NUL-byte C-ABI set); node-sass api.js fails only on
engine-semantics goldens (whitespace eliminated by real NESTED) and the
proven-unfixable items below; contract suites green on both legs
(77 tests: base/options/context/staged/values/functions/importers/callee-env/sass2scss).

Ecosystem (third-party bindings): `rust-sass-libsass-tests/ecosystem/Dockerfile`
is one multi-stage image — `libsass-builder` (Ubuntu 24.04, release libsass)
→ `base` (installed `.so`/`.a` + headers + `libsass.pc`) → one `<lang>-smoke`
stage per binding, each ending in a `$color`-variable compile-string
assertion (`/tmp/ok` receipt; the build is the gate). `ecosystem/run.sh`
builds all targets (`--target <lang>` for one); smoke helpers live in
`ecosystem/smokes/<lang>/`. Standing: python, go×2 (`wellington/go-libsass`,
`bep/golibsass`), ruby, perl, C#, java, lua, php (`sensational/sassphp`),
rust (`sass-rs`), nim, node-sass all smoke-green, plus the lua/nim/rust
suites pass unmodified; a `sassc` target compiles the reference CLI against
our lib (complementing `tool/build-sassc.sh`).

### Async feature

The adapter stays sync-only, but it no longer breaks the workspace-wide
async gate. Both `rust-sass-libsass` and `rust-sass-libsass-tests` define an
`async` feature that is a **graph-consistency stub**:

- `rust-sass-libsass`: `async = ["rust-sass/async"]`, and `src/lib.rs` opens
  with `#![cfg(not(feature = "async"))]`. Under `--features async` every
  module is compiled out, so the crate is an empty `libsass` artifact and
  none of its sync `Importer`/`Io` impls meet the async core.
- `rust-sass-libsass-tests`: `async = ["rust-sass-libsass/async"]`, so the
  `[build-dependencies]` edge also resolves `rust-sass` as async. Without
  this the build-dep graph built a _second_, sync `rust-sass` while the
  shared `rust-sass-macros` proc-macro was async — the macro/cfg mismatch in
  `macros.md` ("consumer-crate cfg blindness") that produced E0412/E0733.

The forwarding is what preserves the invariant "every crate aliases the same
feature … no mixing hazard" (`macros.md` §Mode selection): a no-op `async`
would leave the build-dep instance of `rust-sass` sync. Consequently
`cargo check/clippy --workspace --all-targets --features async` passes, and
the supported artifact is always the sync build. An async build produces an
empty `libsass` — deliberately unsupported, not a working async adapter.

Clippy `-D warnings` denies the rustc lints too: an unimported `UPPER_CASE`
name in a `match` arm binds silently (only `unused_variables` hints),
so every new warning is a build break. Lifecycle contract tests must
free each allocation exactly once — a double-free aborts the whole test
binary. Census runs start from a clean tree (failed CLI tests skip
their `unlink`, leaving fixture outputs that poison later tests). For
ABI-honesty probes outside the harness, a plain C program links the
staticlib with include + archive path only.

## Known divergences

Framed like `divergences.md` — do not fix; each names what would change it:

- **Engine semantics** (never, by D1): error text/traces, `quote(0)`-style
  type strictness, best-quote string shape, source-map `mappings`
  granularity, `@import` deprecation warnings on stderr (breaks
  parse-once consumers).
- **Precision-5 output width** (node-sass default): parameterizing forks
  byte-identity; our digits are strictly more accurate.
- **Dispatch order for non-opt-in hosts**: base-first (Dart-canonical);
  opt-in `prefers_raw_load_paths` importers get customs-first raw+prev
  consultation for `@import`.
- **`file:`-vs-path shapes** (historical): fixed — `prev`,
  `includedFiles`, map `file`/`sources`/comment all report paths.
- **NUL bytes**: substituted with U+FFFD at the boundary (upstream
  `strlen`-truncates instead) — unpassable through any `char*`; the
  native runner preserves them.
- **COMPACT, non-`lf/crlf/cr/lfcr` linefeeds, file-entry
  comment paths**: non-`lf/crlf/cr/lfcr` linefeeds stay deferred, see
  §9. (COMPACT shipped — `SASS_STYLE_COMPACT` maps to the unit-tested
  core `Compact`, byte-identical to upstream `sassc -t compact`.)

## Deferred / future work

Each names its revisit trigger:

- **`sass2scss()` port**: SHIPPED 2026-09-08 (`sass2scss.rs`:
  line-for-line behavioral port of `src/sass2scss.cpp`, differential-clean
  vs the standalone C++ oracle on 417 corpus files + 27 edge files × 16
  option combos; invalid-Sass `@import` shapes that abort upstream are
  hardened in the port). Trigger history: a consumer needs the converter
  symbol, not just the version.
- **Staged compiler** (`sass_make_*_compiler`, `parse`/`execute`,
  `sass_compiler_get_state/context/options`, `take_*` transfer): SHIPPED —
  lazy parse (syntax parse + import-closure discovery into `included_files`,
  no CSS) with upstream's exact state machine (NULL→1, idempotent
  re-parse/re-execute→0, wrong-phase→-1, parse-returns-0-on-error,
  pre-existing errors returned without advancing), full compile at
  `execute` through the shared one-shot path. Three documented
  divergences: (1) the parse-phase file list is the _static_ subset
  (`find_dependencies` + real `ImportCache` canonicalization, transitively;
  interpolated/conditional/dynamic imports appear only after `execute`) —
  upstream's parser loads imports eagerly so its list is exact; (2) error
  timing: upstream surfaces evaluation errors (e.g. undefined variables)
  already at `parse`, ours at `execute` — identical state after `execute`;
  (3) custom-importer callbacks may fire at both `parse` (discovery) and
  `execute` (compile); upstream fires once. Revisit trigger: a consumer
  depending on parse being side-effect-free (would need a core
  seeded-cache entry point) or on exact parse-phase lists for dynamic
  imports. Trigger history: wellington/go-libsass's staged flow
  (make→parse→execute); census after shipping shows all 83 `sass_*`
  symbols it references present (`nm` over the built artifact).
- **Third consumer gaps** (python/ruby/go/nim/`sass-sys`, plus perl/C#/java/lua/php/rust surveyed since): SURVEYED — the ecosystem gate (`rust-sass-libsass-tests/ecosystem/Dockerfile`, one smoke stage per binding, §Verification) ran the needed-vs-exported census for each: python/ruby/nim/perl/C#/java/lua/php/rust/`sass-sys`/node-sass needed nothing beyond the shipped surface; both Go bindings (`wellington/go-libsass`, `bep/golibsass`) needed exactly the staged quintet, which shipped in the staged-compiler entry above (census: all 83 `sass_*` symbols go-libsass references present in the built artifact). Trigger history: the Go link failure against the pre-staged adapter. Revisit trigger: a new consumer whose census shows a gap.
- **Binding version-parser survey**: DONE — D2 (`"3.6.6"`) confirmed through python, ruby, perl, C#, java, nim, and node-sass `info`; D3 (`"3.5"`) through C# (`SassInfo.SassLanguageVersion`) and nim (`language_version()`); zero version-gated misbehavior across all twelve ecosystem targets. Revisit trigger: any version-gated misbehavior report.

## File mapping

| Adapter        | libsass sources (paths relative to `libsass/`)                                                                                                           |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `base.rs`      | `src/sass.cpp` (alloc) + `src/util.cpp` (quote) + `include/sass/base.h`                                                                                  |
| `sass2scss.rs` | `src/sass2scss.cpp` (converter) + `include/sass2scss.h` (option constants)                                                                               |
| `options.rs`   | `src/sass_context.cpp` + `include/sass/context.h` (options)                                                                                              |
| `context.rs`   | `src/sass_context.cpp` + `src/context.cpp` (pipeline) + `include/sass/context.h`                                                                         |
| `values.rs`    | `src/sass_values.cpp` + `src/sass_values.hpp` + `include/sass/values.h`                                                                                  |
| `functions.rs` | `src/sass_functions.cpp` + `src/context.cpp` (registration) + `src/fn_utils.cpp` (signatures) + `src/eval.cpp` (invocation) + `include/sass/functions.h` |
| `importers.rs` | `src/sass_functions.cpp` + `src/context.cpp` (`call_loader`) + `src/sass.cpp` (find) + `include/sass/functions.h`                                        |
| `env.rs`       | `src/sass_functions.cpp` (env) + `src/eval.cpp`/`src/expand.cpp` (callee sites) + `include/sass/functions.h`                                             |

Per-file `// libsass-source:` annotations carry the precise mapping (see
`patterns.md` §2); early build history lives in git history.
