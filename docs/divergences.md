# Intentional divergences

Every _behavioral_ divergence between this port and Dart Sass has been resolved:
the Rust port (and the Go port) produce byte-identical output to Dart Sass
`1.104.0`. The resolved divergences are recorded only in git history, not here.

This document records the differences that remain **on purpose**: places where
Dart, Go, and Rust differ in implementation while producing identical results,
plus two deliberate Rust design choices. Do not "fix" any of these — they are
not bugs.

## Implementation differences with identical output

### 1. `PseudoSelector` boolean sense (Go)

Go's `pseudo.IsSyntacticClass` is the inverse of Dart's
`pseudo.isSyntacticElement` (single-colon `:` vs double-colon `::` sense). The
naming is inverted but the serialized output is identical.

### 2. `WriteString` return discarded (Go)

Go's `sb.WriteString()` returns `(int, error)`, discarded with `_, _ =`; Dart's
`buffer.write()` is void; Rust uses `write!(buf, ...).unwrap()` (infallible for
`String`). A mechanical difference with identical output.

### 3. `for_span` closure capture (Rust)

Go/Dart closures capture the full visitor (`this`/`sv`); Rust's `for_span`
callback receives only `&mut SourceMapBuffer`, so visitor-state methods must be
called outside the closure with precomputed results, and dispatch inside the
callback uses a manual `match` rather than `accept()`. A Rust borrowing
difference only; output is identical. (See `ref/serialize.md`.)

### 4. `utf8Decode` (Go)

Go needs a manual `utf8Decode` because Go strings are byte-indexed. Dart and
Rust iterate code points natively (`string.codeUnitAt`, `s.chars()`), so no such
helper exists in Rust. No output difference.

## Deliberate Rust divergences from Dart

### wasm libm and `glibc-math`

On `wasm32-unknown-unknown` there is no libm: `f64::powf` lowers to
`compiler_builtins`' fdlibm-style `pow`, which rounds the wrong way on ~28
last-ULP color-conversion inputs. The `glibc-math` feature instead uses a
faithful Rust port of glibc's `e_pow.c`, which bit-matches native macOS/glibc
libm. Native builds keep `f64::powf`. This is a deliberate, wasm-only divergence
(see `ref/math.md`).

### Imported-CSS folded into the single `CssVisitor`

Go uses a separate `importedCssVisitor` struct that downcasts every incoming
node to a modifiable type. Dart's `_ImportedCssVisitor` implements
`ModifiableCssVisitor<void>` directly, and Rust folds the same logic into the
single `CssVisitor` impl on the evaluator via context-dispatched free functions
— no separate struct, no downcasts (see `ref/eval.md`).

### `max_recursion_depth` retained but unwired

`EvalConfig.max_recursion_depth` (250) has no Dart counterpart and is never
read outside `Debug` — the real bound is the machine stack (see
`critical-invariants.md` "Stack safety"). It is kept so `EvalConfig`
construction sites stay stable. Do not wire it up.

### `Importer::canonicalize` convenience wrapper keeps `NoOpWarnLogger`

The `Importer::canonicalize` wrapper (for non-eval callers: tests, host
importers) drops the caller's `WarnLogger` in favor of a fresh
`NoOpWarnLogger`. The eval pipeline threads the real logger through
`ImportCache::canonicalize_with` directly, so this is unobservable in
compilation. Do not "fix" the wrapper.

### Explicit-`Scss` syntax cannot be distinguished from default

Dart's `compile()` honors an explicit `syntax` verbatim and only infers from
the path when `syntax` is `None`. Rust's `CompileOptions.syntax` is a plain
`Syntax` value (default `Scss`), so "explicit Scss for a `.sass` file" is
inexpressible — only default-`Scss` is path-inferred. The CLI always sets
syntax explicitly, so all expressible paths match Dart.

### Complex `hasPossiblyCompatibleUnits` is lenient instead of throwing

Dart: `complex.dart:48-54` throws `UnimplementedError` for complex-unit
comparisons.
Rust: `value/number.rs:has_possibly_compatible_units` returns
`is_comparable_to` for complex units instead.
Why accepted: crashing to conform is not a fix; on reachable inputs
(`min(1px*s, 2px*s)`) both sides emit the unsimplified calculation
byte-identically, so the leniency is unobservable in output.
Pins: dart 5fd18c75, rust d3a1faf.

### `InterpolatedAttributeSelector.toString` keeps the closing `]`

Dart: `interpolated_selector/attribute.dart:toString` returns without the
closing `]` (upstream typo).
Rust: `ast/sass/interpolated_selector/attribute.rs:to_display_string` appends
`]` like every other selector display.
Why accepted: the Display is internal-only (evaluation parses selectors via
`SelectorList.parse`, never via this string) and the port's own parse
round-trip tests lock `[href]`; conforming would break them with no
observable Dart behavior to match.
Pins: dart 5fd18c75, rust a29cf4c.

### `OutputStyle::Nested` (libsass-compat, no Dart counterpart)

Dart: `lib/src/visitor/serialize.dart` has only expanded/compressed output.
Rust: `rust-sass/src/serialize/mod.rs:OutputStyle::Nested` renders libsass
NESTED style (`a {\n  b: 3; }` — glued `; }` closer, source-depth
indentation via evaluator-stamped `tabs`, libsass top-level separation,
own-line comments), sharing the Expanded arm everywhere except the block
closer, the `tabs` shift, separation, and comment placement.
Why accepted: opt-in style only (default output byte-identical —
Expanded/Compressed ignore `tabs` entirely); Dart has no NESTED to conform
to, and the adapter needs it for node-sass whitespace parity. Known
upstream content bugs (dropped nested-`@media` content, unmerged
same-query medias) are deliberately not replicated — byte-identity to Dart
governs content.
Pins: dart 5fd18c75, rust 4444047.

### `OutputStyle::Compact` (libsass-compat, no Dart counterpart)

Dart: `lib/src/visitor/serialize.dart` has only expanded/compressed output.
Rust: `rust-sass/src/serialize/mod.rs:OutputStyle::Compact` renders libsass
COMPACT style (`a { b: 3; }` — one top-level block per line, declarations
space-separated, glued `; }` closer, blank line between top-level blocks,
zero indentation), sharing the Expanded arm for value spellings and the
Compressed arm for indentation suppression, plus `write_block_break`
(space intra-block, LF inter-block) and `comment_to_compact_string`
flattening (`libsass/src/util.cpp:224-251`). Supports/keyframe/
non-`@font-face` at-rule children split across lines
(`libsass/src/output.cpp:197,230,293`); declaration values are reindented
like Expanded.
Why accepted: opt-in style only (Expanded/Compressed output byte-identical —
verified by the unchanged full spec suite); Dart has no COMPACT to conform
to. Two measured upstream shapes are deliberately not replicated because
they are Dart-governed content, not layout: top-level siblings flattened
from one nest join with a single LF (evaluator `is_group_end` grouping,
like Expanded, where libsass blanks), and custom-property continuation
indents follow Dart reindentation (like Expanded, where libsass keeps raw
text).
Pins: dart 5fd18c75, rust c9247c7.

## Adding an entry

Conscious accepts land here instead of conforming to
Dart. Append a `### <short title>` section in this shape:

```md
### <Title>

Dart: <file:line> does X.
Rust: <file:line> does Y instead.
Why accepted: <one-line reason — unreachable input, internal-only,
defensive leniency, or documented quirk preserved deliberately>.
Pins: dart <short>, rust <short>.
```

Do not file message/span/trace/output divergences here — those are bugs.
This file is for behavior-neutral (or deliberately lenient) differences only.
