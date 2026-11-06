# Porting new Dart Sass changes — Rust port

How this port stays in sync with the Dart Sass reference implementation
(dart-sass). This is a **manual** procedure — there is no sync automation.
Run the commands below from the repository root.

## Start here: an `upgrade to <version>` request

A request like **“upgrade to 1.106.1”** is the complete instruction. It means:
discover the release span, port every functional unit, commit the release,
push the feature branch, wait for branch CI, fast-forward and push `main`,
wait for main CI, tag the release, push the tag, and wait for tag CI.

No plan file is needed. This document plus the repository state are
authoritative; do not wait for a separate plan. “Stop before main” or “port
the units only” in the request withholds the corresponding release steps.

The first commands are:

```sh
cd /home/luka/projects/rust-sass
git fetch origin
git status --short --branch
git switch main && git pull --ff-only
git switch -c upgrade/sass-<version> origin/main
```

Then follow [Target discovery and pin resolution](#target-discovery-and-pin-resolution),
[Sync procedure](#sync-procedure), and
[Release checkpoint](#release-checkpoint-every-upstream-tag-including-patch-releases).
Every commit—unit or release—stops for review first; see
[Review protocol](#review-protocol).

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

## Target discovery and pin resolution

`TARGET` below is the upstream release tag, for example `1.106.1`. Run from
the repository root.

```sh
git -C dart-sass fetch origin --tags
git -C sass-spec fetch origin
git -C embedded-host-node fetch --tags origin
git -C sass fetch --tags origin

BASE=$(cat PORTED_FROM)
git submodule status dart-sass embedded-host-node sass sass-spec
git -C dart-sass rev-parse "$TARGET^{commit}"
git -C dart-sass log --oneline "$BASE..$TARGET" -- lib/ CHANGELOG.md
```

Classify the commits with [Sync procedure](#sync-procedure) step 1. Every
user-visible `CHANGELOG.md` entry must map to a functional unit or to an
explicitly skipped unit. A release span with no `lib/` changes is a patch
release with no port units.

Resolve the release pins as follows:

- **dart-sass**: the commit the release tag points at
  (`git -C dart-sass rev-parse "$TARGET^{commit}"`). `PORTED_FROM` must equal
  it.
- **embedded-host-node**: the **peeled commit** of the matching host tag
  (`git -C embedded-host-node rev-parse "$TARGET^{commit}"`), not the
  annotated tag object. Host tags are `Update Dart Sass version and release`
  commits.
- **sass**: the commit for the embedded protocol version. Read the version
  from the published `sass-embedded@$TARGET` package (`protocol-version`) or
  from `sass/spec/EMBEDDED_PROTOCOL_VERSION`, then
  `git -C sass rev-parse "embedded-protocol-<version>"`. Move this pin only
  when the protocol version changes.
- **sass-spec**: the spec commit belonging to the ported units. Inspect
  `git -C sass-spec log --oneline <current-pin>..origin/main -- spec/` and
  match commits to the units' tests. Advance per unit and never past an
  unported unit; a release has no single upstream “spec tag”.
- **compiler/protocol**: `$TARGET` and the protocol version above.
- **dart2js**: the version printed by the published package:
  `npx -y sass@$TARGET --version`.

Record the resolved pins in the release ledger. If a pin already matches,
leave it untouched.

## Commit-by-commit sync and release checkpoints (mandatory)

Do not batch a release span into one bump commit. Create one Rust commit per
upstream functional commit, in upstream order, and stop at every upstream
release tag.

### Per-unit commit

1. Check out the upstream commit and port only its `lib/` diff.
2. For Dart sync/async twin files, read the async file only (the sync file is
   generated).
3. Port line-by-line: same control flow, helpers, messages, ordering, spans,
   and error paths. No behavioral or structural shortcuts.
4. Add failing-first internal tests in the touched module as `#[maybe_test]`
   (sync and async legs), covering the new behavior and its exact messages and
   error spans.
5. Advance the `sass-spec` pin to that unit's spec commit, never past an
   unported unit. If the unit has no spec commit, leave the pin unchanged.
6. Verify locally:
   - `cargo test -p rust-sass`
   - `cargo test -p rust-sass --features async`
   - the affected `sass-spec` directories
   - differential A/B against the target dart-sass binary
   - clippy with the CI/MSRV toolchain (currently Rust 1.99), sync and async
7. Fold any pre-existing port bug exposed by this unit into this same commit.
   Do not create a standalone fix commit before the branch has been pushed.
8. Advance `PORTED_FROM` and the `dart-sass` submodule to the ported commit.
9. Commit only that unit. Record skipped upstream commits in the ledger.

The full spec suite is run one top-level directory per process locally because
the runner's peak memory grows beyond a single-process run; CI runs it whole.

### Release checkpoint (every upstream tag, including patch releases)

A patch release with no `lib/` changes still gets a release commit and tag.

1. Resolve and check out the release pins from
   [Target discovery and pin resolution](#target-discovery-and-pin-resolution):
   `dart-sass`/`PORTED_FROM` to the tag commit, `embedded-host-node` to the
   peeled host tag commit, `sass` to the protocol tag commit (usually
   unchanged), and `sass-spec` to the last spec commit for the ported units.
2. Run `tools/bump-version.sh <version>`.
3. Set `DART2JS_VERSION` from `npx -y sass@<version> --version` and update the
   wasm `--version` assertion in `rust-sass-wasm/js/src/wasm/cli.test.ts`.
4. Sweep the current version and update the release surfaces:
   - `git grep -n -F '<old-version>'`; current references move, historical
     CHANGELOG sections stay.
   - `README.md`, `docs/upstream.md` (commit **and date**),
     `docs/review.md` (`PORTED_FROM` hash), `docs/ref/sass-embedded-rust.md`
     (host tag/hash), `docs/ci.md`, `docs/release.md`,
     `docs/architecture.md`, `docs/divergences.md`,
     `docs/critical-invariants.md`, and the remaining `docs/ref/*` version
     references.
   - npm oracle pins: `rust-sass-wasm/package.json` (`sass` and
     `sass-embedded`) and `embedded-host-node-rust/package.json`
     (`sass-embedded`), then refresh both lockfiles with
     `npm install --package-lock-only --ignore-scripts` in those directories.
   - `embedded-host-node-rust/package.dist.json`: wrapper version and the
     exact optional platform dependencies (the bump script updates these;
     verify).
   - `docs/CHANGELOG.md`: add the section from upstream. Keep historical
     sections unchanged.
5. Run the full local battery with the CI/MSRV toolchain (Rust 1.99), including
   the wasm release build and JS tests:
   ```sh
   cargo test -p rust-sass && cargo test -p rust-sass --features async
   cargo test -p rust-sass-macros && cargo test -p rust-sass-macros --features async
   cargo test -p rust-sass-embedded && cargo test -p rust-sass-embedded --features async
   cargo clippy --workspace --all-targets -- -D warnings
   cargo clippy --workspace --all-targets --features async -- -D warnings
   # spec suite, one top-level directory per process
   cd rust-sass-wasm
   RUSTUP_TOOLCHAIN=1.99.1 npm run build:rust:release
   npm run build:js
   npm run test:js
   ```
   Run the full spec suite as described below and differential A/B against
   `sass@<version>` for the release's behavior changes.
6. Stage the release surfaces explicitly, run the
   [Review protocol](#review-protocol), and commit only after approval.
7. Push the short-lived feature branch (for example
   `upgrade/sass-<version>`) and wait for branch CI. `ci.yml` runs on every
   branch push.
8. After branch CI is green, fast-forward `main` to the feature branch tip and
   push it unless the request said “stop before main”. Wait for main CI. If it
   fails:
   - never amend a pushed commit,
   - add fix commits, re-run the gates, and wait for the next CI run.
9. After main CI is green, create the local annotated tag `v<version>` and
   push it.
10. Wait for the tag/release CI (`crates` and `Release` for a `v*` tag). Only
    then start the next upstream unit.

### Push and tag ownership

An explicit **“upgrade to `<version>`”** authorizes the full release flow:
feature-branch push, branch CI, fast-forwarding `main`, main push, main CI,
tag push, and tag CI. A request that says “stop before main” or “port the
units only” withholds the corresponding steps.

The agent never force-pushes `main` and never moves or recreates a published
tag. Tags are created only after main CI is green, so they include any CI fix
commits.

### Branch policy

`ci.yml` runs on every branch push. Use a short-lived branch (for example
`upgrade/sass-<version>`) for each release checkpoint, fast-forward `main` to
it after branch CI is green, and delete the branch when the upgrade is
complete.
`crates.yml`, `release.yml` and `playground.yml` stay `main`/tag-only; they
have packaging, release or deployment side effects.

### Rebuilding a commit stack

If the history must be rebuilt before release:

1. Keep the old stack on a backup branch.
2. Create the new branch from `origin/main`.
3. Recreate the commits, folding fixes into the relevant commit.
4. Delete and recreate local `v1.10X.*` tags on the rebuilt release commits.
5. Follow the normal review/push/tag checkpoints above.

### Wasm

Wasm is a local release gate, built with the CI/MSRV toolchain:

```sh
RUSTUP_TOOLCHAIN=1.99.1 npm run build:rust:release
npm run build:js
npm run test:js
```

Run focused `js-api-spec` and a scoped wasm spec subset when the harness is
available. CI remains the final full-wasm gate.

## Review protocol

Every unit commit and every release commit is reviewed before it is created.

1. Stage explicit paths (`git add <paths>`, never `git add -A`).
2. Run the unit or release gates: fmt, tests in both builds, clippy in both
   builds, focused `sass-spec` directories, and differential A/B where the
   behavior is observable.
3. Show the staged summary (`git diff --cached --stat`), the pins being moved,
   the new/changed internal tests, the focused spec results, and any
   differential output.
4. Stop and wait for approval. Do not `git commit` until the user approves.
5. After the commit, continue with the next unit or the release steps.

For release commits, the same review applies after the full battery, before
the branch push.

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
   git diff --name-only $(cat PORTED_FROM)..<target> -- sass/spec/embedded_sass.proto
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
   `#[maybe_test]` tests (both sync and async legs), the affected spec
   directories, and the differential A/B for that behavior. Release
   checkpoints run the full battery described in
   [Commit-by-commit sync and release checkpoints](#commit-by-commit-sync-and-release-checkpoints-mandatory).
   Run the spec suite one top-level directory per process locally; CI runs the
   full suite in one process.

   Plus `cargo clippy --workspace --all-targets -- -D warnings` in both
   default and `--features async` builds. Wasm is a local release gate (build
   and JS tests); CI remains the final full-wasm gate.

7. **Record and commit.** Update `PORTED_FROM` to the new commit hash and
   `upstream.md` (commit, version, any external package pins). The release
   commit message is the **ledger**: list each functional unit ported
   (`<sha> <subject>`), each unit explicitly skipped and why, and the
   submodule pins moved (`dart-sass`, `sass-spec`, `sass`,
   `embedded-host-node`). Keep it free-form — a plain list is enough, no
   template. Commit each functional unit separately; commit the release
   changes at the release checkpoint. Then follow the version-bump checklist
   in `CONTRIBUTING.md` (`COMPILER_VERSION`/`SASS_VERSION`, wasm
   `version.ts`, crate/npm versions, CHANGELOG entry).

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
