# Upstream tracking — Rust port

This port mirrors a specific revision of the Dart Sass reference
implementation. `../PORTED_FROM` holds the machine-readable dart-sass commit
the port currently matches (read by sync tooling). This file records the
human-readable picture.

## Tracked upstream

|                     |                                                                        |
| ------------------- | ---------------------------------------------------------------------- |
| dart-sass commit    | `e01e268c6f6826ae309bf3105765d4c93024ebbc` (2026-09-02, `PORTED_FROM`) |
| dart-sass version   | 1.104.0                                                                |
| Embedded protocol   | 3.2.0                                                                  |
| Dart SDK constraint | `>=3.13.0 <4.0.0` (dart-sass `pubspec.yaml`)                           |

Before the first release the ports are re-synced to current `origin/main`
(procedure: `porting.md`). Update `../PORTED_FROM` and the table below together.

## External-package-derived files

Most files are ports of dart-sass `lib/` sources (see each file's
`// dart-source:` annotation and the Google MIT header). A small set instead
ports third-party Dart packages that dart-sass depends on; their license
headers carry the Dart-project-authors BSD text. Because these packages are not
part of dart-sass's tree, the exact version ported is recorded here. All
versions below are compatible with the ranges in dart-sass `pubspec.yaml` at
the tracked commit (dart-sass carries no committed lockfile).

| Ported file(s)                                                                                                                      | Source package                                   | Version         |
| ----------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ | --------------- |
| `rust-sass/src/common/source_span_file_source.rs`, `source_span_highlighter.rs`, `source_span_span_with_context.rs`, `file_span.rs` | `source_span`                                    | 1.10.2          |
| `rust-sass/src/common/span_scanner.rs`                                                                                              | `string_scanner`                                 | 1.4.1           |
| `rust-sass/src/termglyph.rs`                                                                                                        | `term_glyph`                                     | 1.2.2           |
| `rust-sass/src/sourcemap/vlq.rs`                                                                                                    | `source_maps`                                    | 0.10.13         |
| `rust-sass/src/common/pretty_uri.rs`                                                                                                | `path`                                           | 1.9.1           |
| `rust-sass/src/common/core_errors.rs`                                                                                               | Dart SDK (`dart:core`)                           | Dart `>=3.13.0` |
| `rust-sass-macros/` (all)                                                                                                           | `maybe-async` (Rust crate, MIT © 2020 Guoli Lyu) | 0.2.11          |

When syncing (`porting.md`), re-port any file whose package range changed in
dart-sass's `pubspec.yaml`, and copy the package's actual header from the
pinned release (pub cache / GitHub), not dart-sass.
