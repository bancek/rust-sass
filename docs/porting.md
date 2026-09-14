# Porting new Dart Sass changes — Rust port

How this port stays in sync with the Dart Sass reference implementation
(dart-sass). This is a **manual** procedure — there is no sync automation.
Run the commands below from the repository root.

## The model

- The port tracks one dart-sass commit at a time: `PORTED_FROM` (hash
  only, must agree with the `dart-sass/` submodule pin) + `upstream.md`
  (human-readable: commit, version, external package pins).
- Porting is **dart → rust directly** — read the Dart source in the
  `dart-sass/` submodule; there is no cross-port chaining. (Byte-identical
  output against Dart is verified via the oracles in the test battery — a
  cross-check, not a porting path.)
- Cadence: sync in **functional units** (see step 1). A release span
  contains mostly mechanical noise (Dart syntax modernization,
  analyzer/lint/reformats, CI/dependabot/version bumps) with zero behavior
  change. Each functional PR is ported in isolation from its own diff —
  never from the whole-span range diff (which buries behavior under
  churn), and never by walking every commit.
- Every change must land in **both** builds of this crate: sync (default) and
  async (`--features async`), which are one source tree compiled twice via
  `rust-sass-macros` (`maybe_async`). Items for only one build are `#[cfg]`-gated
  (see `ref/macros.md`). In practice most units need only one port: Dart's
  sync/async twin files (e.g. `evaluate.dart` / `async_evaluate.dart`) map
  to a single Rust file — and within the pair, only the async file is
  source of truth: dart-sass auto-generates the sync file from the async
  one (`tool/grind/synchronize.dart`: `async_evaluate→evaluate`,
  `async_compile→compile`, `async_environment→environment`,
  `async_import_cache→import_cache`). Read the async diff; ignore the sync
  twin's diff entirely.

## Sync procedure

1. **Fetch and locate the functional units.** `dart-sass/` is a submodule pinned at the
   tracked commit (`PORTED_FROM` must agree — see `upstream.md`).

   ```sh
   git -C dart-sass fetch origin
   git -C dart-sass log --oneline $(cat PORTED_FROM)..<target> -- lib/
   ```

   dart-sass lands work as **squash commits on main with the PR number as a
   message suffix** (`<title> (#NNNN)`); release spans contain almost no
   merge commits, so merge topology carries no information — group by those
   squash commits, not by merges. Classify each commit by subject + stat;
   mechanical commits are recognizable by subject alone (`shorthand`,
   `constructor`, `lint`, `reformat`, `analyzer`, `Bump …`, CI/actions,
   release plumbing):

   ```sh
   git -C dart-sass show --stat --oneline <sha> | head -30
   git -C dart-sass show <sha> -- lib/ test/ | head -100
   ```

   A mechanical commit's `lib/` diff is syntax/style churn only; a
   functional one changes behavior and usually tests. Each surviving commit
   is one **functional unit**, ported in isolation from its own
   `git show <sha> -- lib/ test/` diff.

   Subject-based filtering is **not sufficient**: mechanical commits can
   smuggle functional changes (observed: a "prefer interpolation" lint
   commit that also swapped two deprecation-message branches and added an
   empty-args guard). The CHANGELOG cross-check below is the backstop —
   any user-visible entry without a matching functional unit means the
   filter missed something: find it with `git log -S"< distinctive
string>" -- lib/` on the entry's behavior, not by re-reading subjects.

   Cross-check against the `CHANGELOG.md` entries in the span: every
   user-visible entry must map to a unit. If
   `git -C dart-sass log --oneline $(cat PORTED_FROM)..<target> --grep="<keyword>"`
   for a changelog entry returns empty, it predates `PORTED_FROM` and is
   already ported — move on.

   CLI-only/tooling-only units with no Rust counterpart are **recorded as
   skipped** in the bump ledger (step 7), not ported and not silently
   dropped. Standing skip: `--watch` mode handling — the Rust CLI has no
   file watcher, so `--watch` fixes (e.g. `8abfa4d2`, unreleased `72446f80`)
   are always skipped. Re-evaluate only if the CLI ever gains watching.

   To advance the pin: `git -C dart-sass checkout <new-commit>` (full clone —
   any commit is inspectable).

2. **Find the affected ported files (per unit).** Each ported file carries a
   `// dart-source: lib/src/...` annotation. Map the unit's changed Dart
   files to Rust counterparts, matching on the **full `lib/src/...` path**,
   not the basename (basenames over-match — e.g. `color.dart` appears in
   many directories):

   ```sh
   git -C dart-sass show --name-only --format= <sha> -- lib/ \
     | while read d; do
         rg -l "dart-source:.*$d" --glob '*.rs' . || echo "NO RUST PORT: $d"
       done
   ```

   A `NO RUST PORT` hit is usually a **false alarm, not a missing file**.
   The most common cause: Dart keeps separate sync/async twin files, and
   the sync twin is auto-generated from the async one
   (`tool/grind/synchronize.dart`: `async_evaluate→evaluate`,
   `async_compile→compile`, `async_environment→environment`,
   `async_import_cache→import_cache`). Rust covers both builds from one
   source file via `maybe_async`, so both twins resolve to the same Rust
   file (e.g. `evaluate.dart` and `async_evaluate.dart` both map into
   `rust-sass/src/eval/`) — and when reading the diff, the async twin is
   the only one that matters; the sync twin's diff is generated output.
   Only if neither the file nor its twin resolves is it a genuinely new
   file (add a Rust file) or a skipped unit (step 1) — decide explicitly,
   never ignore.

   Handle Dart renames/additions/deletions: add or drop the Rust file, and
   update the `dart-source:` annotations (never leave a stale path — the header
   sweep in the release plan relies on them resolving).

   When touching a `docs/ref/*.md` File-mapping table, re-verify the Go
   column against the live `go-sass` tree (`bancek/go-sass`). The Go column
   is informational, but a stale one sends the next reader to a file that
   does not exist.

3. **Check dependency bumps.** `pubspec.yaml` pins the external packages
   (`source_span`, `string_scanner`, `source_maps`, `path`, the Dart SDK). If a
   runtime pin moved, re-port the files listed in `upstream.md` from the new
   release and update that table. Dev-only moves (analyzer, lints, grinder,
   CI actions) and SDK-floor moves are ignored unless they change language
   semantics the port relies on. `maybe-async` is a Rust dependency pinned
   directly (`rust-sass-macros`); re-check it against upstream only if it needs
   re-vendoring.

4. **Regenerate protocol bindings if the proto changed.**

   ```sh
   git -C dart-sass diff --name-only $(cat PORTED_FROM)..<target> -- build/language/spec/embedded_sass.proto sass/spec/embedded_sass.proto
   # if non-empty:
   cargo run -p rust-sass-embedded-pb-gen --features gen
   ```

   Bump `PROTOCOL_VERSION`/`COMPILER_VERSION` in `rust-sass-embedded/` in
   lockstep with the dart-sass version. (Purely mechanical
   `protofier`/`proto_extensions` churn with an unchanged `.proto` needs no
   regen.)

5. **Port one unit at a time.** Read the unit's Dart diff; apply the equivalent change to the Rust files
   from step 2. Keep `// Matches Dart:` / `// dart-source:` annotations honest. New files get the standard header: the Dart source's
   license lines plus `// Ported and rearchitected for Rust by Luka Zakrajsek.`
   Per AGENTS.md: repro first (differential A/B vs `dart-sass/` on the
   unit's behavior), fix narrowly, add the regression test before moving to
   the next unit.

6. **Verify per unit, then globally.** Per unit: the touched file's in-module
   `#[maybe_test]` tests (both sync and async legs), the **full** spec suite
   (`cargo test -p rust-sass-spec --test runner_test --release -- --ignored`;
   at ~9s release there is no reason to scope it — `SASS_SPEC=<subpath>` is
   only a debug aid for iterating on a failure), and the differential A/B
   for that behavior. After all units: the full battery —

   ```sh
   cargo test -p rust-sass                     # unit, sync build
   cargo test -p rust-sass --features async    # unit, async build
   cargo test -p rust-sass-macros && cargo test -p rust-sass-macros --features async
   cargo test -p rust-sass-embedded            # 56 + golden + differential + interactive
   cargo test -p rust-sass-embedded --features async
   cargo test -p rust-sass-spec --test runner_test --release -- --ignored   # spec suite
   # byte-identity rust==dart on bootstrap/huge/huge10 (see CONTRIBUTING.md)
   # embedded round-trip harness: embedded-host-node-rust/ (drive the assembled host against target/release/rust-sass)
   ```

   Plus `cargo clippy --workspace --all-targets -- -D warnings` in both
   default and `--features async` builds. If the wasm bridge is
   affected, the wasm build + js-api-spec (`rust-sass-wasm`, see
   `ref/wasm.md`).

7. **Record and commit.** Update `PORTED_FROM` to the new commit hash and
   `upstream.md` (commit, version, any external package pins). The bump
   commit message is the **ledger**: list each functional unit ported
   (`<sha> <subject>`), each unit explicitly skipped and why, and the
   submodule pins moved (`dart-sass`, `sass-spec`, `sass`,
   `embedded-host-node`). Keep it free-form — a plain list is enough, no
   template. Commit the port changes and the tracking update together. Then
   follow the version-bump checklist in `CONTRIBUTING.md`
   (`COMPILER_VERSION`/`SASS_VERSION`, wasm `version.ts`, crate/npm
   versions, CHANGELOG entry).

## Ecosystem pins (checked in step 1, moved per unit — not once at the end)

dart-sass does **not** pin sass-spec: its CI clones floating `main` HEAD
(`sass/clone-linked-repo`: main by default, a linked spec PR's head only
while testing the matching implementation PR). So there are no upstream
pins to copy — define per-unit pins instead. Each behavior in the span has
a matching spec-side commit, and every dart-sass behavior commit predates
(or is same-day-earlier than) its spec commit, so chronological lockstep
keeps every step green:

- Work the units in **chronological** order (this satisfies code
  dependencies like "serialize change A before serialize change B" as long
  as the order respects them — check before committing to it).
- After porting each unit, advance the `sass-spec` submodule pin **to**
  that unit's spec commit — never past an unported unit's spec commit, or
  the suite carries expectations for behavior you haven't ported yet.
- Then the full suite must be green before moving on. Shared files touched
  by several spec commits (e.g. `mix/missing.hrx`) resolve themselves:
  each pin's version was authored against dart-sass main at that date,
  which contained exactly the behaviors released to date — the same set
  you have ported.
- Units whose spec delta is js-api-spec-only (no `.hrx` change) still
  advance the pin for monotonicity, but are verified by fixtures +
  in-module tests + differential A/B; their `.node.test.ts` files become a
  Phase-9 js-api-spec gate. Conversely, if a unit HAS a dedicated
  `.node.test.ts` file (e.g. importer/logger behavior), run that file
  per-unit too — it can catch gaps the `.hrx` suite and unit tests miss
  (observed: the index-fallback import-only case). This needs a current
  wasm build (`rust-sass-wasm`: `build:rust` + `build:js`); the legacy
  `render`/`renderSync` failures are documented exclusions of the
  modern-only build.
- `sass` (language spec): moves when a unit is spec-backed (check for
  flushed proposals in the span).
- `embedded-host-node`: moves to the host tag matching the new dart-sass
  version (once, at the end — host releases track versions, not units).

## License/header rules when porting

- Header = the ported Dart file's exact 3-line license header + `//\n// Ported
and rearchitected for Rust by Luka Zakrajsek.`
- Files ported from external packages carry that package's BSD header (see
  `upstream.md`), never a Google MIT header.
- Original Rust code (no Dart source) gets no Google header.
