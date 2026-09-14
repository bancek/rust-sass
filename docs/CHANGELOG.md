# Changelog

## Unreleased

(Add an entry here with every user-visible change. Crate versions stay
`0.1.0` until the pre-release re-sync — see `Cargo.toml` — so entries
accumulate here until the first version bump, which renames this section.)

### Sync to dart-sass 1.104.0

Tracks dart-sass `e01e268c` (embedded protocol unchanged at `3.2.0`).
Behavior ports, oldest first:

- Node package importer loads import-only files for `@import` (#2772).
- Stack traces humanize only URLs that have a scheme (#2778).
- Plain-CSS `if()` values serialize as CSS, not inspected values (#2808).
- Fixed vendor-prefixed `expression()` deprecation direction (#2148, found
  smuggled inside a lint refactor).
- Legacy `rgb()`/`rgba()` with non-integer channels emit percentages
  (#2800).
- rec2020 uses the 2.4 gamma transfer function (#2729).
- Conversions preserve analogous sets of missing channels (#2810).
- Degenerate colors: `NaN`/negative zero (and polar-hue infinities)
  normalize to `0`; negative zero serializes as `-0` (#2840).

Skipped: `--watch` handling (CLI-only, no Rust counterpart) and the
unreleased `72446f80` watch fix.

- Full `sass-spec` suite: 14263/14263.
- Research-loop note: the 1.104 tree needs a dev-channel Dart SDK (3.14
  beta tested; stable 3.13.3 cannot compile it); `dart2js` 3.13.3.

## 1.100.0

Initial release of the Rust port of Dart Sass, tracking dart-sass `1.100.0`
(embedded protocol `3.2.0`).

- Byte-identical output to Dart Sass on the bootstrap, `huge`, and `huge10`
  workloads (CSS, source maps, warnings, and errors).
- Full `sass-spec` suite: 13904/13904.
- `rust-sass` library, `rust-sass-cli` command-line binary, `rust-sass-embedded`
  protocol server, and `rust-sass-wasm` JS bridge.
- Sync (zero-future) and async builds from a single source tree.
