# Documentation

The project overview, status, and usage live in the [root
README](../README.md). This directory holds the maintainer documentation;
start with `architecture.md`, then `patterns.md`, then
`critical-invariants.md`.

| Document                                           | Contents                                                             |
| -------------------------------------------------- | -------------------------------------------------------------------- |
| [`architecture.md`](architecture.md)               | The pipeline and the design decisions behind it.                     |
| [`patterns.md`](patterns.md)                       | Translation conventions and the annotation system.                   |
| [`critical-invariants.md`](critical-invariants.md) | Rules that must never be violated.                                   |
| [`ref/`](ref/README.md)                            | Per-module reference (evaluator, parser, values, …).                 |
| [`divergences.md`](divergences.md)                 | Intentional implementation differences (identical output); not bugs. |
| [`porting.md`](porting.md)                         | How to port new dart-sass changes.                                   |
| [`upstream.md`](upstream.md)                       | Tracked commit and external-package pins.                            |
| [`CONTRIBUTING.md`](CONTRIBUTING.md)               | Build, test, and contribution guide.                                 |
| [`ci.md`](ci.md)                                   | CI workflows, release tags, and roadmap.                             |
| [`release.md`](release.md)                       | Release runbook (bump script, alpha, tag flow).                      |
| [`CHANGELOG.md`](CHANGELOG.md)                     | Release history.                                                     |
| [`review.md`](review.md)                           | Rust↔Dart review guide (how to check a port for bugs).               |

Upstream tracking (mirroring a specific dart-sass commit by hand):

- [`../PORTED_FROM`](../PORTED_FROM) — the machine-readable tracked commit.
- [`upstream.md`](upstream.md) — the human-readable picture (commit, version,
  external-package pins).
- [`porting.md`](porting.md) — the procedure for porting new dart-sass changes.

Every ported source file carries a `// dart-source: lib/src/...` annotation
naming the Dart file it reimplements; this is the reverse index used when
back-porting upstream changes.
