# AGENTS.md — read this first, then read the docs it points to

This codebase is large (~137k lines of Rust across 8 crates) and tightly
constrained: it must produce **byte-identical output to Dart Sass**, so most
"obvious" simplifications are bugs. Do **not** dive into code head-first.
Follow the reading map below before touching anything.

## Mandatory starting reads (in order)

1. **`docs/CONTRIBUTING.md`** — how to build, test, and verify. Non-negotiable.
2. **`docs/architecture.md`** — the pipeline and why the design looks the way
   it does (arena lifetimes, free-function evaluator, enum dispatch).
3. **`docs/ref/pipeline.md`** — end-to-end walkthrough of one compilation:
   arena ownership, entries, stages.

## Before ANY code change, also read

- **`docs/critical-invariants.md`** — rules that must never be violated
  (no `unsafe`, no `unwrap` except `write!` on `String`, error-variant
  discipline, sync+async parity). Violating one means revert.
- **`docs/patterns.md`** — translation conventions (Dart → Rust) and the
  `// dart-source:` annotation system.
- **The relevant `docs/ref/` page** for the module you are touching
  (index at `docs/ref/README.md`) — it documents the exact struct shapes,
  invariants, and gotchas. Doc snippets drift; verify signatures against
  code with `rg` before copying them.

## Rules for every change

- **Tests are mandatory.** Every fix needs a regression test that fails
  without it, in the touched file's in-module `mod tests` as `#[maybe_test]`
  (both sync and async legs). See `CONTRIBUTING.md` Testing.
- **Both builds must pass.** Sync (`cargo test -p <crate>`) and async
  (`--features async`) — one source tree compiled twice; a sync-only pass
  hides regressions.
- **Clippy must be clean.** `cargo clippy --workspace --all-targets -- -D warnings`
  must report zero warnings, in both default and `--features async` builds
  (async-only lints exist — a sync-only pass hides them). New allows are
  per-case with justification; see `CONTRIBUTING.md` Lint.
- **Iterate locally, verify globally.** Write fast in-module unit tests first
  and iterate on those; at the end of every change the full spec suite must
  also pass:
  `cargo test -p rust-sass-spec --test runner_test --release -- --ignored`
  (use `SASS_SPEC=<subpath>` for a focused subset during development —
  it must be a real path under `sass-spec/spec/`).
- **Match Dart's observable behavior exactly** — messages, spans, traces,
  ordering, scope effects (see `docs/review.md` for the checklist). Never
  "improve" Dart; record deliberate divergences in `docs/divergences.md`
  using its entry template.
- **Keep annotations honest.** Every ported file carries `// dart-source:` —
  update them when code moves; never leave a stale path.
- **Verify per change, not at the end.** Repro first (differential A/B vs
  `dart-sass/`), fix narrowly, run the gates in `CONTRIBUTING.md`.

## Task-specific entry points

| Task                         | Start with                                                            |
| ---------------------------- | --------------------------------------------------------------------- |
| Porting a dart-sass change   | `docs/porting.md` + `docs/upstream.md`                                |
| Evaluator/importer/functions | `docs/ref/eval.md`, `docs/ref/importer.md`, `docs/ref/environment.md` |
| Parser/serializer/values     | `docs/ref/parse.md`, `docs/ref/serialize.md`, `docs/ref/value.md`     |
| Sync/async build issues      | `docs/ref/macros.md` (triage table)                                   |
| Embedded protocol / wasm     | `docs/ref/embedded.md` / `docs/ref/wasm.md`                           |
