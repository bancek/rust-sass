# Contributing

How to build, test, and maintain `rust-sass`. This is a maintenance and
contribution guide; the procedure for porting upstream dart-sass changes lives
in [`porting.md`](porting.md).

## Prerequisites

- Rust 1.92 (async closures and the `AsyncFn*` traits are required).
- Dart SDK (for the differential research loop: piping the same SCSS through
  `dart run bin/sass.dart --stdin` from `dart-sass/` vs the Rust CLI).
  Constraint from [`upstream.md`](upstream.md): Dart `>=3.13.0 <4.0.0`,
  dev channel required (3.14 beta tested — stable 3.13.3 cannot compile
  the 1.104 tree); check with `dart --version`.
- Dart SDK install (macOS/homebrew, as done for the 1.104 sync — the stock
  `dart` formula tracks stable, which is insufficient):
  ```sh
  brew trust dart-lang/dart
  brew install dart-lang/dart/dart-beta
  brew unlink dart   # if stable is also installed: only the beta may own
                     # `dart` on PATH, or `dart run` silently compiles with
                     # the wrong SDK
  dart --version     # must report the beta, e.g. 3.14.0-211.1.beta
  ```
  Any SDK that compiles the pinned tree works — re-verify with
  `printf 'a { b: c; }\n' | dart run bin/sass.dart --stdin` from
  `dart-sass/` after (re)installing.
- `protoc` plus the Dart protobuf plugin (to generate the embedded-protocol
  Dart bindings — see below). The plugin ships as a dev-dependency of
  dart-sass itself: `dart pub global activate protoc_plugin` (ensure
  `~/.pub-cache/bin` is on `PATH`). `buf` is an accepted alternative driver
  (`buf --version`), but the documented command uses `protoc` directly so no
  extra config files are needed.
- For the wasm bridge: `wasm-pack` and Node.js (for the `js-api-spec` gate:
  sass-spec's `npm run js-api-spec` in `sass-spec/`, pointed at the
  `rust-sass-wasm` build — see `ref/wasm.md`).
- Submodules: clone with `git clone --recurse-submodules` (or run
  `git submodule update --init` afterwards). Five pins: `dart-sass`,
  `sass-spec`, `bootstrap-main`, `embedded-host-node`, `sass` — see
  [`upstream.md`](upstream.md) for what each feeds.

### Submodules

Submodules are pinned, read-only reference trees: the build and test gates
consume them but never modify tracked files. Every entry in `.gitmodules`
therefore sets `ignore = dirty`, so changes to a submodule's working tree do
not show up in the superproject's `git status` or the diff family. A change
to the pinned **commit** (the gitlink moving) still shows; only work-tree
dirt is suppressed. `ignore = dirty` covers modified tracked files and
untracked files alike (e.g. the generated protobuf bindings written into
`dart-sass/`).

This is required for `sass-spec` at the pinned commit. Its `.gitattributes`
normalizes HRX to LF (`*.hrx text eol=lf`), but several committed fixtures
under `spec/` intentionally contain CRLF — they are the CRLF-handling tests,
e.g. `spec/libsass-closed-issues/issue_100.hrx`. The working-tree bytes match
the committed blobs, so there is no real edit, yet Git's forced normalization
disagrees with them: once a file's index stat cache is invalidated (a
`touch`, a checkout, or a fresh `git submodule add`), it is re-read and
reported as modified — hundreds of files at once. `ignore = dirty` keeps the
superproject clean without changing the pin; `git -C sass-spec status` still
reports the raw submodule state.

Trade-off: because dirt is hidden, local edits inside a submodule (for
example while experimenting with `dart-sass` for the differential loop) do
not appear in the parent's `git status`. Work inside the submodule directory
to see them.

#### Clean `git status` inside a submodule (local)

`ignore = dirty` only silences the superproject; `git -C sass-spec status`
(or `cd sass-spec && git status`) still shows the spurious `.hrx` files. To
clean that too, override the attribute locally instead of editing the pinned
`.gitattributes`. `$GIT_DIR/info/attributes` outranks every `.gitattributes`
file, so one line is enough for `sass-spec`:

```sh
mkdir -p .git/modules/sass-spec/info
printf '*.hrx -text\n' > .git/modules/sass-spec/info/attributes
git -C sass-spec status --short   # clean
```

Or run it from inside the submodule, where `--absolute-git-dir` already
points at the right directory:

```sh
printf '*.hrx -text\n' > "$(git rev-parse --absolute-git-dir)/info/attributes"
git status --short                # clean
```

`-text` disables CRLF↔LF conversion for those paths, so Git compares raw
bytes — which match the committed blobs — rather than the normalized form;
genuine content edits still show. This must be `$GIT_DIR/info/attributes`:
`core.attributesFile` and other config cannot override a tracked
`.gitattributes` because they sit at lower precedence. The file is local to
your clone and disappears if the submodule is deinitialized
(`git submodule deinit sass-spec`), so re-run the command after that.

### Dart embedded bindings (fresh checkout)

`dart run bin/sass.dart` fails to compile on a fresh checkout: the protobuf
bindings `dart-sass/lib/src/embedded/embedded_sass.{pb,pbenum,pbjson}.dart`
are gitignored build artifacts. Generate them from the pinned `sass`
language-spec submodule (never copy them from another checkout — the proto
version must match the `dart-sass` pin):

```sh
dart pub global activate protoc_plugin   # once; needs ~/.pub-cache/bin on PATH
export PATH="$PATH:$HOME/.pub-cache/bin"
protoc -I sass/spec \
  --dart_out=dart-sass/lib/src/embedded \
  --plugin=protoc-gen-dart=$HOME/.pub-cache/bin/protoc-gen-dart \
  embedded_sass.proto
```

Verify: `printf 'a { b: c; }\n' | dart run bin/sass.dart --stdin` (from
`dart-sass/`) must print the compiled CSS. Regenerate whenever the `sass` or
`dart-sass` pins move.

## Building

```sh
cargo build -p rust-sass-libsass        # first on a fresh checkout (see below)
cargo build                              # whole workspace
cargo build --release -p rust-sass-cli   # the production `rust-sass` binary
```

On a fresh checkout, build `rust-sass-libsass` before the workspace:
`rust-sass-libsass-tests` links the prebuilt `libsass.so` from
`target/<profile>/`, so the library must exist before that crate's build
script runs (incremental rebuilds already have it).

The CLI binary is named `rust-sass` (the `[[bin]]` name in `rust-sass-cli`).
It defaults to the **sync** build; `--features async` produces the async build.
`mimalloc` is the default global allocator (feature `mimalloc`).

## Testing

Every code change to `rust-sass` (or any workspace crate) must add or update
tests covering it: a bug fix needs a regression test that fails without the
fix, a behavior change needs tests locking the new behavior. Tests assert
**exact** variant/message/span (`match err`, never bare `is_err()`); golden
values are hardcoded, never recomputed from the code under test (see
`patterns.md` §8 and `critical-invariants.md` Test standards).

Run these from the repository root. Both sync and async modes must pass —
they are one source tree compiled twice; always run both legs (a sync-only
regression can otherwise land silently).

```sh
# Unit tests, sync build
cargo test -p rust-sass

# Unit tests, async build
cargo test -p rust-sass --features async

# The macro crate
cargo test -p rust-sass-macros && cargo test -p rust-sass-macros --features async

# Embedded protocol server
cargo test -p rust-sass-embedded
cargo test -p rust-sass-embedded --features async

# The official sass-spec suite (definitive correctness gate)
cargo test -p rust-sass-spec --test runner_test --release -- --ignored
```

The release gate takes ~8s of test time once built (debug works too at ~33s —
useful when a release build isn't handy, but prefer release: debug's
unoptimized frames also shrink the stack margin on deeply nested inputs).

How tests are laid out, what `.hrx` files and `options.yml` mean, how the
runner compares output, and the debug loop for a failure: see
[`ref/sass-spec.md`](ref/sass-spec.md). That page also documents the faster
full-suite cross-check (`npm run sass-spec -- --command
../target/release/rust-sass --impl dart-sass`, ≈44s) and the wasm runners.

The `sass-spec` suite requires the `sass-spec` submodule checkout at the
repository root (`git clone --recurse-submodules`). To run a subset, set
`SASS_SPEC` to a **relative path under `sass-spec/spec/`** (it is appended
verbatim — use a real directory or `.hrx` file, e.g. `directives/use`,
`css/media`, `core_functions/color`; a nonexistent path runs 0 tests):

```sh
SASS_SPEC=directives/use cargo test -p rust-sass-spec --test runner_test --release -- --ignored
```

`SASS_SPEC_PATH` is different: an absolute path used _instead of_ the
default `sass-spec/spec` root (if both are set, `SASS_SPEC` is appended
under the resolved root — see `rust-sass-spec/tests/runner_test.rs`).

### rust-sass-cli

```sh
cargo build --release -p rust-sass-cli   # target/release/rust-sass
# fast full-suite cross-check through the upstream harness (≈44s):
cd sass-spec && npm run sass-spec -- --command ../target/release/rust-sass --impl dart-sass
```

The harness run exercises the release binary end to end (CLI arg handling,
real process I/O) — a useful second gate after the in-process runner when a
change touches CLI behavior, I/O, or exit codes. Append a subpath (as with
`SASS_SPEC=`) to scope it. See [`ref/sass-spec.md`](ref/sass-spec.md) for
all runners and when to use each.

### rust-sass-wasm

```sh
cd rust-sass-wasm
npm run build:rust   # pkg-sync/pkg-async/pkg-web (with glibc-math) — required first;
                     # a stale js/dist (no pkg-sync/) fails every test with MODULE_NOT_FOUND
npm run build:js     # tsc -> js/dist, including bin/sass.js
npm run test:js      # vitest unit/parity suite
# wasm spec through the upstream harness (≈16min full; scope with a subpath):
cd ../sass-spec && npm run sass-spec -- --command ../rust-sass-wasm/js/dist/bin/sass.js --impl dart-sass [subpath]
# JS API surface (omit the two legacy-render .node.test.ts files):
npm run js-api-spec -- --sassPackage <abs-path-to>/rust-sass-wasm/js/dist --sassSassRepo <abs-path-to>/sass [files…]
```

Run the wasm gates when the wasm adapter/shim changes, or after large
refactorings — not per core change (the native gates above cover that).
Details and expected counts: [`ref/sass-spec.md`](ref/sass-spec.md)
("Ways to run") and [`ref/wasm.md`](ref/wasm.md).

### Byte-identity

After any core change, verify byte-identical output vs Dart Sass on the
bootstrap, `huge`, and `huge10` workloads. The `rust-sass-spec` `expected`
files are the Dart goldens; the benchmark harness is described below.
`bench/huge.scss` is a tracked workload (`huge10` is generated from it at bench
time: 10× concatenation); `bootstrap-main/` is a pinned submodule.

### Benchmarking (native A/B)

Measure with interleaved A/B runs comparing **user CPU** (not wall — background
threads inflate `%cpu`, so wall time is misleading): warm up once,
report the median of 5, and re-measure CPU and wall after every change. For
faithful ratios rebuild with `CARGO_INCREMENTAL=0` (incremental codegen adds
noise). The standing results are ~0.83× Dart Sass CPU time on bootstrap.
(The wasm bench below uses a different estimator — minimum, not median —
because memory drift makes medians misleading there.)

### Lint

Clippy must be clean — zero warnings, not just zero errors:

```sh
cargo clippy --workspace --all-targets -- -D warnings
# async leg (async-only lints exist — a sync-only pass hides them):
cargo clippy --workspace --all-targets --features async -- -D warnings
```

New `#[allow(clippy::…)]` must be per-case (on the flagged fn, field, enum,
or static — never module- or workspace-level) with a one-line justification
comment. The four systemic by-design lints (`mutable_key_type`,
`too_many_arguments`, `type_complexity`, `large_enum_variant`) carry per-case
allows for the documented reasons; everything else gets fixed. CI runs
these commands as the deny gate (`.github/workflows/ci.yml`) — see
[`ci-todo.md`](ci-todo.md).

## Protocol codegen

The embedded-protocol bindings are committed generated code. Regenerate them
only when the `.proto` changes:

```sh
cargo run -p rust-sass-embedded-pb-gen --features gen
```

This rewrites `rust-sass-embedded-pb/src/embedded_sass.rs` from
`sass/spec/embedded_sass.proto` (in the `sass` language-spec submodule). The generated code is prost 0.14 and
depends only on `prost` so it compiles once and stays cached.

## Version-bump checklist

The compiler version and protocol version track the pinned dart-sass version.
When bumping:

1. Update `PROTOCOL_VERSION` / `COMPILER_VERSION` in
   `rust-sass-embedded/src/lib.rs` (and `SASS_VERSION` in
   `rust-sass-cli/src/options.rs`) to match dart-sass.
2. Update [`../PORTED_FROM`](../PORTED_FROM) and [`upstream.md`](upstream.md)
   (commit, version, any external-package pins) — see [`porting.md`](porting.md).
   The `dart-sass/` submodule pin moves to the same commit (both must agree).
   The `libsass/` pin feeds the release archives (headers), not just the
   upstream-leg contract tests — move it deliberately and re-verify the
    header set (`release.yml` fails on unreviewed additions).
3. Update `SASS_VERSION` / `DART2JS_VERSION` in
   `rust-sass-wasm/js/src/version.ts` (derived `VERSION_TEXT`/`INFO_TEXT`
   follow; `DART2JS_VERSION` is the `dart2js` line of the published `sass`
   release's `info` output), `rust-sass-wasm/js/package.json` + publishable crate versions, and add a
   `CHANGELOG.md` entry. Bump the npm oracle devDeps to the new release
   (`sass` + `sass-embedded` in `rust-sass-wasm/package.json` and
   `sass-embedded` in `embedded-host-node-rust/package.json`; `npm install`
   to refresh the lockfiles) and the wrapper version in
   `embedded-host-node-rust/package.dist.json` — the single version that
   matters (source `package.json` stays `0.0.0`; the 8 platform manifests and
   the tag guard derive from the dist manifest) — plus the 8 exact
   `optionalDependencies` pins there. The lockfile carries no platform entries
   (they live only in the dist manifest), so no lockfile re-sync is needed for
   platform changes.
   Update version assertions in tests (e.g. the wasm `--version` test),
   then sweep the repo for the old version — stragglers hide in test
   fixtures, doc tables, and hardcoded strings. Triage each hit: our docs
   and code move to the new version, historical changelog sections and
   real past-release markers (e.g. `deprecated_in`) stay, submodule
   contents and lockfiles are upstream's or generated (verify, don't
   hand-edit).
4. Bump the `embedded-host-node` submodule pin to the matching host tag,
   rebuild `embedded-host-node-rust` (new platform binaries), and re-run its
   harness + coexistence + tarball validation.
5. Re-run the full battery above, in both sync and async modes, plus the
   embedded differential and byte-identity checks.

## Release

Releases are tag-based, only after a re-sync (Phase 4) with all gates green.
Runbook (bump script, alpha/first-release procedure, tag flow):
[`release.md`](release.md).

1. `cargo publish` in dependency order: `rust-sass-macros`,
   `rust-sass-embedded-pb`, `rust-sass`, `rust-sass-libsass`,
   `rust-sass-embedded`, `rust-sass-cli` — the `crates.yml` CI job lists
   packaged files per crate on every push and publishes in order (with
   index-propagation retries) on tags, via OIDC trusted
   publishing (no token secret) — see [`ci.md`](ci.md).
2. `npm publish` for `rust-sass-wasm`: the `release.yml` CI pipeline builds
   the release bundles (`build-wasm` job), runs vitest, and publishes
   `js/dist` (`publish` job; exact-pinned `dependencies`, `pkg-web`
   dev-only); validate with `npm pack` of `js/dist`
   + scratch-dir install + smoke compile. Publishing uses npm trusted
   publishing (OIDC, no token secret), under `next` on prerelease tags —
   see [`ci.md`](ci.md).
3. `npm publish` for `sass-embedded-rust`: the `release.yml` CI matrix builds all 8
   platform binaries natively (`build-native` job), the `publish` job publishes the 8
   `sass-embedded-rust-<platform>-<arch>` packages first, then the wrapper
   (exact-pinned `optionalDependencies` via `package.dist.json`, which is the
   version authority — source `package.json` stays `0.0.0`); validate with
   `npm pack` of both + scratch-dir install + smoke compile. Publishing uses
   npm trusted publishing (OIDC, no token secret), under `next` on
   prerelease tags — see [`ci.md`](ci.md).
4. GitHub release attachments for `rust-sass-libsass`: the `release.yml` CI
   matrix builds the adapter on all 8 triples natively, the `publish` job
   packs `rust-sass-libsass-<version>-<triple>.zip` (pinned upstream headers
   + native libs) and attaches them on tags — see [`ci.md`](ci.md) and
   [`ref/libsass.md`](ref/libsass.md) §Release binaries.

## Synchronizing with dart-sass

See [`porting.md`](porting.md). The short version: diff `lib/` between the
tracked commit and `origin/main`, map changed Dart files to Rust files via the
`// dart-source:` annotations, port each change, then re-run the full battery.

## Tooling

### ast-grep

[ast-grep](https://ast-grep.github.io/) drives large mechanical refactors. The
three commands that matter:

```sh
# search (read-only)
ast-grep run -p 'PATTERN' -l rust rust-sass/src

# inline rewrite (apply all)
ast-grep run -p 'PATTERN' -r 'REPLACEMENT' -l rust -U rust-sass/src

# rule-file rewrite (applies `fix:`)
ast-grep scan --rule /tmp/rule.yml -U rust-sass/src
```

- `scan -U` applies rewrites; the rule file must use `fix:` (not `rewrite:`).
- Metavariables: `$V` (one node), `$$$A` (zero or more), `$_` (non-capturing).
- The tree-sitter-rust node kinds that matter: `call_expression` (construction),
  `match_pattern` → `tuple_struct_pattern` / `tuple_pattern` (match arms),
  `let_condition` (`if let` patterns), `scoped_identifier` (unit variants, e.g.
  `ValueKind::Null`). Unit variants need `not: { inside: { kind: match_pattern } }`
  and `let_condition` exclusions because `scoped_identifier` appears in both
  positions.
- `--debug-query=cst` shows how a pattern parses; `run -k` _replaces_ the
  pattern (not a filter); without `-U` the command prints a diff only.

### Profiling

```sh
cargo instruments --profile release-with-debug -p rust-sass-cli -t cpu --no-open \
  --output /tmp/trace.trace -- bootstrap-main/scss/bootstrap.scss
```

Use the CPU Profiler template (`-t cpu`); its `cpu-profile` export is
pre-symbolized. The command needs a real terminal (it fails under a non-TTY
shell), so run it manually and analyze `scripts/cpu_profile.py` on the exported
XML.

### wasm benchmarking

Bench prerequisites, in order: submodules (above) → `npm run build:rust` in
`rust-sass-wasm` (wasm artifacts under `js/dist/pkg-*`) → `npm run build` in
`embedded-host-node-rust` (assembled wrapper + local platform binary;
`huge10` is generated from tracked `bench/huge.scss` at bench time).
Suites without their prerequisites skip cleanly (`artifactsPresent`,
`wrapperPresent`) — a skip is not a pass; check the report output.

The wasm bench harness is `rust-sass-wasm`'s `npm run test:bench` (or
`js/src/wasm/perf.test.ts`). Report **minimum**, not median (memory drift makes
medians misleading). Prime the clock once per process (`set_time_now` —
`SystemTime::now()` panics on wasm32), and run the benches through the real
`compile`/`compileString` over node:fs.
