# Module: `parse/`

The parser: source text in, `Stylesheet<'parse>` out.

## The `Syntax` enum

```rust
pub enum Syntax { Scss, Sass(SassIndentState), Css(CssState) }

pub struct SassIndentState { current_indentation: usize, next_indentation: Option<usize>, next_indentation_end: Option<LineScannerState>, indent_spaces: Option<bool> }
pub struct CssState { disallowed_function_names: HashSet<String> }
```

The single `Syntax` enum **replaces all 14 Go function-pointer fields** that Go
used to dispatch between SCSS, indented Sass, and plain CSS. Rust free functions
`match state.syntax` instead — no `Box<dyn FnMut>`, no trait objects, no
allocations. Go's counts confirm the shape: Sass sets 10 fn fields, CSS 13, SCSS
9, and the only cross-cutting state is `SassIndentState` (4 fields) and
`CssState.disallowed_function_names` (1 field).

The mapping is Dart's OOP model (7 abstract `StylesheetParser` methods plus
`if (indented)`/`if (plainCss)` checks) flattened into a match: `CssParser
extends ScssParser` in Dart becomes `Css` match arms delegating to the shared
SCSS free functions where behavior matches.

## Struct split + free functions

Every parser type is split into a scanner-carrying wrapper and a state struct,
with all logic in free functions:

```rust
pub struct ParserState<'parse> { syntax: Syntax, interpolation_map: Option<&'parse InterpolationMap<'parse>> }
pub struct Parser<'parse> { scanner: SpanScanner<'parse>, state: ParserState<'parse> }

// free function: the logic
pub(crate) fn identifier_impl(scanner: &mut SpanScanner, state: &ParserState, normalize: bool, unit: bool) -> ParseResult<String>;

// thin wrapper
impl Parser<'_> { pub fn identifier(&mut self, ...) -> ParseResult<String> { identifier_impl(&mut self.scanner, &self.state, ...) } }
```

`StylesheetParser<'parse>` nests `ParserState` inside a `StylesheetState`
(selectors-allowed, in-mixin, in-control-directive, global-variables, and other
stylesheet-level flags), so stylesheet free functions can call the parser-level
functions directly. Standalone parsers (`AtRootQueryParser`, `SelectorParser`,
`KeyframeSelectorParser`, `CssMediaQueryParser`) wrap a `Parser` and always use
`Syntax::Scss`.

The split exists because closure-heavy methods (like `raw_text`) need a consumer
that parses more input; the consumer receives `(&mut SpanScanner, &mut State)` —
two disjoint mutable references — rather than re-borrowing `&mut self` twice.

## Files

| #   | Rust file              | Purpose                                                                                                                            |
| --- | ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| 1   | `parser.rs`            | base `Parser`: whitespace, identifiers, strings, numbers, escapes                                                                  |
| 2   | `import_url.rs`        | `parse_import_url` — converts Windows-absolute paths to `file:` URIs and returns the URL **unchanged** on success (no re-encoding) |
| 3   | `stylesheet.rs`        | `Syntax`, `SassIndentState`, `CssState`, `StylesheetState`                                                                         |
| 4   | `identifier.rs`        | interpolated identifier scanning (`#{}` inside identifiers)                                                                        |
| 5   | `util.rs`              | lookahead predicates, `with_children`, `url_string`, `assert_public`                                                               |
| 6   | `at_root_query.rs`     | `@at-root` query parser                                                                                                            |
| 7   | `keyframe_selector.rs` | keyframe selector parser                                                                                                           |
| 8   | `media_query.rs`       | CSS media query parser                                                                                                             |
| 9   | `media_style.rs`       | stylesheet-level media queries                                                                                                     |
| 10  | `selector_parse.rs`    | resolution-level selector parser                                                                                                   |
| 11  | `selector.rs`          | stylesheet-level interpolated selector parser                                                                                      |
| 12  | `supports.rs`          | `@supports` condition parsing — operators (`and`/`or`/`not`) are matched **case-insensitively** per the CSS spec                   |
| 13  | `anyvalue.rs`          | `almost_any_value`, `interpolated_declaration_value`                                                                               |
| 14  | `expression.rs`        | the Pratt expression parser                                                                                                        |
| 15  | `atrule.rs`            | all 22 `@`-rule parsers + `parameterList` + configuration                                                                          |
| 16  | `stylesheet_parse.rs`  | `parse`, statement, style-rule                                                                                                     |
| 17  | `scss.rs`              | SCSS comment nodes + default syntax behavior                                                                                       |
| 18  | `sass.rs`              | indentation logic, Sass comments                                                                                                   |
| 19  | `css.rs`               | CSS-specific `@`-rules, forbidden at-rules                                                                                         |

## The Pratt expression parser

`expression.rs` is a Pratt parser using two sub-structs plus free functions:

```rust
pub(crate) struct Acc<'parse> { comma_exprs, space_exprs, single, allow_slash }
pub(crate) struct Op<'parse>  { operators: Vec<BinaryOperator>, operands: Vec<Expression<'parse>> }
```

All logic takes `(scanner, state, ops: &mut Op, acc: &mut Acc, ...)` — four
disjoint mutable references. **Why the split:** Go's `_expression` uses six
nested closures that capture mutable locals and call each other; Rust closures
cannot simultaneously borrow scanner + state while sharing mutable state between
themselves. The `Acc` + `Op` split makes each free function's borrows disjoint.

`resolve_one` distinguishes `1/2` (division) from `1 / 2` (a space list with a
slash) using `Acc.allow_slash` plus `state.in_parentheses` — which is why
`resolve_one` takes `&StylesheetState` in addition to `&mut Op`/`&mut Acc`.

Span discipline (all mirroring Dart's `stylesheet.dart`/`selector.dart`/
`sass.dart`/`parser.dart`/`css.dart` call shapes):

- `unary()`: `+`/`-` in plain CSS error `Operators aren't allowed in plain
CSS.` (`/` is exempt so calculations still parse; evaluation rejects the
  use). `parse_single`'s `!` arm spans the full `!important`
  (`span_from(bang)`), the `%` arm spans just the `%` character, and the
  loop `!` arm likewise starts at the `!` rather than the whole-expression
  start.
- `unmatched "}"` reports a length-1 span (Dart's `length: 1`); the
  interpolated attribute-operator `Expected "]".` fallback is
  zero-length at the offending char (Dart's `position: start` with no length).
  (`selector_parse.rs:598`'s plain attribute-operator path still uses length 1.)
- `@use … as` namespaces and namespaced variable-declaration namespaces use
  plain `identifier()` with the default `normalize: false`, so `_bar` stays
  `_bar` (at-rule names, `@forward … as … *` prefixes, and `show`/`hide`
  member lists normalize with `true` — as do plain at-rule names via
  `plain_at_rule_name_impl` (an internal dispatch helper; at-rule _lookup_
  itself uses the uninterpolated `interpolated_identifier_impl`, which never
  normalizes).
- Indented Sass: mixed tabs/spaces report Dart's `Tabs and spaces may not be
mixed.` with the whole-indent span (`position - column .. position`); the
  `Expected spaces, was tabs.` / `Expected tabs, was spaces.` errors use the
  same span; a document-start indent reports `position: 0, length: indent`.
  `url(`/`Url(` scan case-**in**sensitively in the indented-syntax import
  path (`scanIdentifier` default `caseSensitive: false`), routing
  `URL(foo.scss)` to the url path — the shared `try_url_impl` already did.
- Plain-CSS `FunctionExpression` argument lists span from after the `(`
  (Dart's `spanFrom(beforeArguments)`), not from the function name.
- `SelectorParser` carries both a `Logger` and a `WarnLogger`: the
  adjacent-compounds deprecation passes `None` through the buffered logger so
  the eval flush resolves the call-site span (threading the inner
  parse span would attribute the warning to the parsed string's file, as the
  `selector.parse("[c]d")` spec case proves).
- `consume_escaped_character` returns `0xFFFD` on EOF (only newlines throw),
  and `matches_identifier` decodes backslash escapes (Dart's
  `_consumeIdentifier` via `scanIdentChar`) instead of comparing raw chars.

## Errors and span adjustment

The parser returns `ParseError` (not `SassError`), so zero-length "expected X"
errors can be repositioned to the preceding newline before conversion.
`wrap_span_format_exception_impl` is three-phase:

1. **Map** — apply the interpolation map at the `FileSpan` level
   (`map_file_span` + `map_exception`).
2. **Adjust** — for `"expected"` messages, `adjust_exception_span` moves a
   zero-length span to the preceding newline (`first_newline_before`); this
   fires for all three `FileSpan`-carrying variants (`Format`, `MultiSpan`,
   `Scan`).
3. **Convert** — `FileSpan → SourceSpanWithContext`, yielding a `SassError`.

See `common.md` for the error type layering.

## File mapping

| Dart                                                               | Go                                                                | Rust                                                                                                                  |
| ------------------------------------------------------------------ | ----------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `lib/src/parse/parser.dart`                                        | `go/value/parse_parser.go`                                        | `src/parse/parser.rs`                                                                                                 |
| `lib/src/parse/stylesheet.dart` (4886 lines)                       | `go/value/parse_stylesheet_*.go`                                  | `src/parse/{stylesheet,identifier,util,anyvalue,supports,media_style,expression,atrule,stylesheet_parse,selector}.rs` |
| `lib/src/parse/{scss,sass,css}.dart`                               | `go/value/parse_{scss,sass,css}.go`                               | `src/parse/{scss,sass,css}.rs`                                                                                        |
| `lib/src/parse/selector.dart`                                      | `go/value/parse_selector.go`                                      | `src/parse/selector_parse.rs`                                                                                         |
| `lib/src/parse/{media_query,keyframe_selector,at_root_query}.dart` | `go/value/parse_{media_query,keyframe_selector,at_root_query}.go` | `src/parse/{media_query,keyframe_selector,at_root_query}.rs`                                                          |
