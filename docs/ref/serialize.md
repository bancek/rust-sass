# Module: `serialize/`

The serializer: CSS output tree (or a value) → a CSS string, with an optional
source map.

## Structure and dispatch

```rust
#[derive(Clone, Copy)]
pub struct SerializeState { indentation: u32, style: OutputStyle, inspect: bool, quote: bool, line_feed: LineFeed, indent_char: char, indent_width: u32 }

pub struct SerializeVisitor<'parse> { buffer: SourceMapBuffer<'parse>, inner: SerializeState }
```

All visitor logic lives in free functions `(buf: &mut SourceMapBuffer, state:
&mut SerializeState, ...)`. `for_node` is a free function that runs a callback
and associates every byte it writes with the node's span via
`SourceMapBuffer::for_span` (mirroring Go's `forNode` / Dart's `_for`). The
serializer uses **two-level dispatch**:

1. Free `visit_*_impl(buf, state, node)` functions hold the logic.
2. The `ValueVisitor`/`SelectorVisitor`/`CssVisitor` trait impls are thin
   wrappers delegating to those functions.
3. Inside `for_node`/`visit_children`/`with_indent` callbacks you only hold
   `&mut SourceMapBuffer` + `&mut SerializeState`, so you cannot call
   `accept()` (which needs `&mut SerializeVisitor`); manual `match` helpers
   (`visit_value_impl`, `visit_css_node_impl`, `visit_selector_impl`,
   `visit_simple_selector_impl`) dispatch directly to the `_impl` functions.

This restores full Go/Dart serialization parity by removing the borrow workarounds
a single `&mut self` visitor would need.

## SourceMapBuffer

```rust
enum SourceMapBuffer<'parse> { Plain(String), Mapping { buf, line, col, builder, in_span, current_span } }
```

A `Plain` buffer has no source map; a `Mapping` buffer tracks the current output
position and records mappings. `SourceMapBuffer` implements `fmt::Write`, so
`write!(buf, ...)` works uniformly.

## Number formatting (Dragon4 emulation)

Numbers are serialized in three stages:

1. `ryu` produces the shortest round-tripping decimal — but ryu switches to
   e-notation at `|v| ≥ 1e6`, while Dart's Dragon4 switches at `|v| ≥ 1e21`.
2. `remove_exponent()` therefore expands any e-notation to plain decimal.
3. `write_rounded_to()` rounds to 10 fractional digits with ripple-carry
   (propagates through the integer part, handles negatives, trims trailing
   zeros).

A `FuzzyAsInt` fast path (`|v − round(v)| ≤ 1e-11`) writes integers without a
`.0` suffix; compressed output strips leading zeros (`0.5 → .5`, sign kept so
`-0.5` stays `-0.5`). The int64-extreme f64 sentinels from the math builtins
print as the exact digit strings (see `ref/math.md`).

## Color

The color serializer is a decision tree over `ColorSpace` (17 variants) and
`ColorFormat` (`RgbFunction | Preserved`):

```
legacy space (rgb/hsl/hwb) with all channels present
  → named color, hex, or rgb()/hsl()/hwb() (compressed picks the shortest)
legacy out-of-gamut (inspect=false) → write_hsl()
inspect && hwb → write_hwb(); format==RgbFunction → write_rgb(); Preserved → the stored text
Lab/Lch/Oklab/Oklch → lab-style function, or color-mix() when out of gamut
  (Lab/Lch: c0 ∉ [0,100]; Oklab/Oklch: c0 ∉ [0,1]; Lch/Oklch: negative chroma c1)
  out-of-gamut with missing channels → prefixed "from red/black"
other modern spaces → color(space c0 c1 c2 / α)
```

Compressed `tryIntegerRgb()` shortens hexable colors to a 3-digit hex or a named
color (comparing named-vs-hex-vs-short-hex and rgb-vs-hsl text lengths). The
compressed rgb-vs-hsl tiebreak compares (and emits) **compressed** number
spellings (Dart's `_writeNumberToString` renders through the current visitor
state: `0.5 → .5`), never uncompressed ones. `ColorFormat::RgbFunction` forces `rgb()`; `Preserved` re-emits the
original source text (see `value.md`). The Dart→Rust accessor mapping —
`isChannel0Missing → is_channel0_missing()`, `channel0OrNull → channel0_or_nil()`,
`isInGamut → is_in_gamut()`, `channel('red') → channel_by_name("red")`, `toSpace →
to_space`, `hexCharFor → character::hex_char_for`, `namesByColor →
color_names::color_name_for_sass_color`, `fuzzy_equals/fuzzy_is_int/fuzzy_in_range
→ number::fuzzy_*` — is the back-port index for the color path.

## Strings, lists, calc

- **Quoting:** auto-detect a quote character; if both `'` and `"` appear, escape
  `"` and keep `'` (force-double mode). Escape `\\`, control chars, and
  private-use chars (compressed output passes private-use chars through).
  Supplementary-plane PUA (`U+F0000–U+FFFFD`, `U+100000–U+10FFFD`) escapes as
  scalars (`\f0000`), matching Dart's UTF-16 high-surrogate test.
- **Lists:** separator and bracket handling; an empty unbracketed list in CSS
  mode errors (`() isn't a valid CSS value.`).
- **Calc:** serializes `CalcArgument` (number / string / interpolation /
  operation / nested calculation).

## `inspect`

`inspect` controls the error surface: in CSS mode (`inspect = false`) maps,
functions, mixins, and empty unbracketed lists raise `SassError::Script`;
`inspect = true` serializes all types without error.

## CSS output formatting

Two C-API-seeded extensions live here (no Dart counterpart; see
`ref/libsass.md`): the `SerializeOptions`/`SerializeState`
`source_comments` flag emitting libsass-style `/* line N, path */`
comments in the style-rule visitor, and the `LINE_FEED_CRLF/CR/LFCR`
constants alongside `LINE_FEED_LF` (`LineFeed.text` is `&'static`, so
only these four byte shapes are expressible).

| Feature                    | Expanded        | Compressed                | Nested (libsass-compat, no Dart counterpart)                                                                                                                     | Compact (libsass-compat, no Dart counterpart)                                                                                                      |
| -------------------------- | --------------- | ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| Semicolon after last child | yes             | no                        | yes (glued closer: `; }`)                                                                                                                                        | yes (glued closer: `; }`)                                                                                                                          |
| Spaces around `{`/`}`      | yes             | no                        | yes, except the closer                                                                                                                                           | yes, except the closer                                                                                                                             |
| `rgb(1, 2, 3)`             | `rgb(1, 2, 3)`  | `rgb(1,2,3)`              | `rgb(1, 2, 3)`                                                                                                                                                   | `rgb(1, 2, 3)`                                                                                                                                     |
| `0.5`                      | `0.5`           | `.5`                      | `0.5`                                                                                                                                                            | `0.5`                                                                                                                                              |
| `@media` query spaces      | spaces          | minimal                   | spaces                                                                                                                                                           | spaces                                                                                                                                             |
| Hoisted indent (`tabs`)    | n/a (0)         | n/a (0)                   | evaluator-stamped `+1` per enclosing props-bearing style rule (transparent through `@at-root`, blocked by media/supports/at-rule boundaries; keyframes excluded) | n/a (0)                                                                                                                                            |
| Block children separator   | LF              | none                      | LF                                                                                                                                                               | space (`write_block_break`); supports/keyframe/non-`@font-face` at-rule children LF (`write_special_linefeed`, `output.cpp:197,230,293`)           |
| Top-level blank lines      | `is_group_end`  | none                      | libsass emitter rule (block closed at indent 0 → blank unless next renders indented; comments never collapse/schedule) — `is_group_end` ignored                  | `is_group_end` (same as Expanded)                                                                                                                  |
| Comment placement          | trailing inline | hidden                    | always own line (no trailing inlining)                                                                                                                           | space-joined (never inline via `is_trailing_comment`, same bytes); multi-line comments flattened (`comment_to_compact_string`, `util.cpp:224-251`) |
| Declaration values         | reindented      | folded                    | reindented                                                                                                                                                       | reindented (continuation indent emitted even though all other indentation is suppressed)                                                           |
| Multi-line selectors       | kept            | collapsed (`,`, no space) | kept (line break preserved)                                                                                                                                      | collapsed to `, ` (`hasPostLineBreak` suppressed, `inspect.cpp:1101`)                                                                              |
| Charset                    | `@charset`      | BOM                       | `@charset`                                                                                                                                                       | `@charset`                                                                                                                                         |

The `CssVisitor` impl covers all 9 CSS node types; the `SelectorVisitor` impl
covers all 11 selector types.

`write_with_indent` (custom-property values, loud comments) walks a
`CharScanner` over `char`s; the scan-forward unread steps back a full char
(`unread_char`), never mid-codepoint — Dart's `LineScanner` (UTF-16 units)
never splits a code point either.

## Source-map integration

`for_span` maps the current output position to the source span start. While
`in_span`, each `\n` auto-maps the output line to the source span; BOM/charset
are handled via prefix offsets. See `source-maps.md` for the builder.

## File mapping

| Dart                                          | Go                              | Rust                                                                  |
| --------------------------------------------- | ------------------------------- | --------------------------------------------------------------------- |
| `lib/src/visitor/serialize.dart`              | `go/value/visitor_serialize.go` | `src/serialize/mod.rs`                                                |
| `lib/src/visitor/*` (value, color, number, …) | `go/value/visitor_*.go`         | `src/serialize/{value,color,number,string,list,calc,css,selector}.rs` |
