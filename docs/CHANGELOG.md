# Changelog

Release history. Tags track the pinned dart-sass version (`v1.105.1`); the
procedure lives in [`docs/release.md`](docs/release.md).

## Unreleased

## 1.105.1

- Improved `@extend` error messages across media queries, with multi-span
  labels matching dart-sass 1.105.1.
- Span-boundary fixes for selectors and media queries, including interpolation
  spans that could swallow an entire media block on error.

## 1.105.0

- First-class modules, matching dart-sass 1.105.0: `meta.load()`,
  `meta.get-module()`, `meta.css()`, module references in the `$module`
  argument of `meta`'s introspection functions, and `meta.type-of()` returning
  `module`.
- Port the 1.104.1 parser fixes (phantom newline in indented selectors,
  `expected end of rule.` / `unrecognized syntax`) and the duplicate
  pre-module loud-comment fix.
- Embedded protocol 3.3.0 (`CompilerModule`) and JS API `SassModule` /
  `Value.assertModule()`.
- Byte-identical output to Dart Sass 1.105.0 (14350/14350 spec tests passing).

## 1.104.0

- Baseline: port complete against dart-sass `1.104.0` — `rust-sass` CLI,
  `compile` API, embedded protocol server (`compilerVersion` 1.104.0,
  protocol 3.2.0), and the `sass-embedded-rust` npm host. Byte-identical
  output to Dart Sass 1.104.0 (14246/14252 spec tests passing).
