# Changelog

Release history. Tags track the pinned dart-sass version (`v1.108.2`); the
procedure lives in [`docs/release.md`](docs/release.md).

## Unreleased

## 1.108.2

Covers dart-sass 1.108.0 through 1.108.2. The intermediate 1.108.0 and 1.108.1
tags were not released separately: 1.108.1 fixed only upstream's embedded-host
packaging (OS-specific packages missing their Dart Sass snapshots), and 1.108.2
has no user-visible changes (only a `sass_api`-facing interface annotation).

- Undo some unintentional breaking changes added in 1.106.0 relating to how
  escaped identifiers interact with `sass:meta` functions and named rest
  arguments. Reflection lookups (`meta.global-variable-exists`,
  `variable-exists`, `function-exists`, `mixin-exists`, `get-function`,
  `get-mixin`, `get-module`, `module-variables`/`functions`/`mixins`) and
  `@use … with` configuration again resolve identifiers in canonical form, and
  serialized `get-function()`/`get-mixin()`/`get-module()` names are escaped.
- Fix a long-standing bug where a namespace declared with an escaped
  identifier (`@use "sass:math" as \$`) was unreachable through `sass:meta`.
- Embedded protocol 3.4.0 (unchanged).
- Byte-identical output to Dart Sass 1.108.2 (14513/14513 spec tests passing).

## 1.107.1

- Report the embedded protocol version as 3.4.0. The `fatal_deprecation_except`
  field was already supported in 1.107.0, but the protocol version was not
  advertised until dart-sass 1.107.1.
- Byte-identical output to Dart Sass 1.107.1 (14400/14400 spec tests passing).

## 1.107.0

- `--fatal-deprecation` accepts `-<name>` exclusions (for example
  `--fatal-deprecation=1.107.0,-import`) and the special value `dart-sass-2`,
  which marks fatal every deprecation that will become fatal in Dart Sass
  2.0.0.
- The JS API's `Options.fatalDeprecations` accepts a
  `DeprecationSelectorDifference` object (`{include, except}`; `except` wins)
  and `sass.deprecations.dartSass2`. The Dart API gains
  `Deprecation.dartSass2`. Passing a version beyond the current Dart Sass
  version is now deprecated.
- Embedded protocol 3.3.0 (unchanged).
- Byte-identical output to Dart Sass 1.107.0 (14400/14400 spec tests passing).

## 1.106.1

- Fix crashes when serializing an attribute selector with an empty value
  (such as `[class=""]`) and a number with an empty unit (reachable through
  the JS/Dart APIs and the embedded protocol).
- Represent Dart `ArgumentError` distinctly from `SassScriptException`, and
  convert argument errors from host-provided values into `PARAMS` protocol
  errors instead of compile failures.
- Embedded protocol 3.3.0 (unchanged).
- Byte-identical output to Dart Sass 1.106.1 (14400/14400 spec tests passing).

## 1.106.0

- Match dart-sass 1.106.0's CSS escape handling: escapes are preserved in
  statement contexts and normalized only where SassScript observes them,
  fixing escaped identifiers, variables, at-rules, and attribute values.
- Fix keyframe selectors that use escaped `e` (`1\65 1%`), units that look
  like numeric exponents (`1\65 1`), and duplicate rest/non-rest parameters
  with multi-span errors.
- Trim comments when computing span boundaries, and fix indented first
  statements and interpolation spans containing `// */`.
- Embedded protocol 3.3.0 (unchanged).
- Byte-identical output to Dart Sass 1.106.0 (14399/14399 spec tests passing).

## 1.105.2

- Match dart-sass 1.105.2's embedded-host release; the compiler is unchanged
  from 1.105.1.

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
