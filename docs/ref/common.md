# Module: `common/`

Errors, source locations, and the span scanner. This is the foundation every
other module builds on.

## Errors

`SassError` is the single public error type:

```rust
pub enum SassError {
    Script   { message: String, argument_name: Option<String> },
    Runtime  { message, span: SourceSpanWithContext, trace: Trace, cause, loaded_urls },
    Format   { message, span, original_source, cause, loaded_urls },
    Sass     { message, span, cause, loaded_urls },
    MultiSpan { message, span, primary_label: Option<String>, secondary, original_source, cause, loaded_urls, trace: Trace },
    MultiSpanScript { message, primary_label: Option<String>, secondary, cause, loaded_urls },
    // (MultiSpanScript carries no primary span yet — the eval call site
    // attaches it via with_member_use_span.)
}
```

- `Script` is the unspanned error built-ins raise; the evaluator wraps it into
  `Runtime` (adding span + trace) at the boundary.
- `Format` is the parse error (it carries the span and source context);
  `MultiSpan` carries secondary spans for multi-location errors (e.g. conflicting
  `@forward`s).
- Every variant except `Script` carries `cause` (a chained error) and
  `loaded_urls: Vec<SassUrl>` (the canonical URLs loaded so far — the embedded layer reports
  these even on failure). `Runtime` and `MultiSpan` also carry `trace: Trace` (an
  empty `MultiSpan.trace` means "derive the frame from the span").
- `loaded_urls` is stamped at the evaluate boundary on **all** spanned variants
  (`with_loaded_urls`, mirroring Dart's `error.withLoadedUrls` on every
  `SassException`), and `with_additional_span` preserves it when rebuilding a
  `MultiSpan`.
- `MultiSpan.original_source` exists because the `source` field name is reserved
  by `thiserror`.

`SassResult<T> = Result<T, SassError>` is the crate-wide result alias.

### Frames and traces

```rust
pub struct Frame { uri: Option<SassUrl>, line: usize, column: usize, member: String }
pub struct Trace { frames: Vec<Frame> }   // Deref to [Frame]
impl Trace { fn new(Vec<Frame>); fn is_empty(&self) -> bool; fn format(&self, io: &dyn Io) -> String; }
pub fn frame_for_span(span, member) -> Frame;      // Dart frameForSpan
pub fn trace_for_span(span, member) -> Trace;      // Dart SassException.trace getter
```

`Trace` is Dart's `package:stack_trace` `Trace`: an ordered (outermost →
innermost) list of frames. It owns its frames, never borrows the `'parse`
arena, and is carried structurally through the evaluator and the `Logger`
seam; stringification happens only at the output boundary via `Trace::format`
(Dart's `Trace.toString()`), which needs the `Io` seam for `prettyUri` because
Rust has no ambient cwd. `frame_for_span`/`trace_for_span` replace the old
`frames_for_span`; there is no string→trace parser (Dart has none).

### Error layering

The parser uses its own internal error types so that spans can be adjusted
before they become `SassError`:

```
ScanError → SpanScannerError → ParseError{Sass | Format(ParseFormatError) | MultiSpan(ParseMultiSpanError) | Scan(ScanError)} → SassError
```

- `ScanError { message, span, cause }` — a scanner-level error carrying a
  `FileSpan` (mirrors Dart's `StringScannerException`).
- `SpanScannerError { Sass(SassError), Scan(ScanError) }` — what the scanner
  returns.
- `ParseError` — what the parser returns; the three span-carrying variants
  (`Format`, `MultiSpan`, `Scan`) each hold a `FileSpan` plus
  `cause: Option<Box<SassError>>`.
- `SpanError { Argument(String), Range(String), Sass(SassError) }` — the internal
  error from span utility methods. `Argument` is a URL mismatch/containment
  failure/wrong order; `Range` is a subspan out of bounds; `Sass` comes from a
  `LazyFileSpan` builder or a scan error. It is `Debug`-only (matched on and
  converted, never displayed), with `From<ArgumentError>`/`From<RangeError>`/
  `From<SassError>` impls. At evaluator/parser boundaries, `Argument`/`Range`
  become `SassError::Runtime` (with the current stack span) and `Sass` passes
  through; `From<SpanError> for SassError` maps them to `SassError::Script` or
  the pass-through.

The reason for the layering is **span adjustment**: a zero-length "expected X"
error must be repositioned to point at the preceding newline, which requires the
original `FileSpan` (and source text) to still be available. See `parse.md`.

## Spans

```rust
pub struct FileSource<'parse> { url: Option<SassUrl>, text: &'parse str, line_starts: &'parse [usize] /* + cached_line: Cell, url_str_cache: RefCell (memoization) */ }

#[derive(Clone, Copy)] pub struct SourceLocation { offset, line, column }  // 0-based

#[derive(Clone, Copy)] pub struct FileSpan<'parse> { file: Option<&'parse FileSource<'parse>>, start: usize, end: usize }
pub const BOGUS_SPAN: FileSpan<'static>;  // file: None

pub struct SourceSpanWithContext { source_url, start, end, text: String, context: String }
```

- `FileSource` is arena-allocated (`FileSource::new_in(&arena, text, url)`),
  infallible, and holds precomputed `line_starts` for binary-search line lookups.
  `FileSource::identical(a, b)` is address identity (`ptr::eq`, `(None, None) →
true`), matching Dart's `identical()` on `SourceFile` (which defines no
  `operator ==`, so `==` on files is identity too) — while `PartialEq` stays
  structural (matching Dart's value-only `_FileSpan ==`). All provenance
  checks (`InterpolationMap::is_mapped`, `map_file_span`, the
  `map_exception` no-op check, `BinaryOperationExpression.operator_span`,
  the four `wrap_span_format_exception_impl` change-detection sites) use
  `identical`: two distinct allocations with equal content are `==` but
  never `identical`, and only identity answers "already mapped here".
- `FileSpan` is a 24-byte `Copy` value; `BOGUS_SPAN` has `file: None`. Its core
  accessors are infallible (handle `None` gracefully); the utility methods
  (`subspan`, `expand`, `before`, `after`, `between`, `contains`) return
  `Result<_, SpanError>`.
- `SourceSpanWithContext` is the **owned boundary type**: `from_span()` copies
  the source text at the public API boundary so `'parse` never leaks. It
  replaces the older `SpanInfo` (which dropped `end` and `text`). Fields:
  `source_url`, `start`/`end` (0-based `SourceLocation`s), `text` (the span's
  own text), and `context` (the surrounding source, ~5 lines). Convenience
  methods `line()`/`column()` (1-based) and `length()`/`is_multiline()`; a
  `from_file_span()` convenience constructor; and `message()`/`highlight()`/
  `highlight_multiple()`/`message_multiple()` which delegate to the highlighter.
  It is `Clone + Debug`.
- The **highlighter** (`source_span_highlighter.rs`) slices and counts in
  **chars, not bytes**: highlight/underline/arrow spans snap to
  `char_boundary_at_or_before`, caret widths and whole-line checks use
  `chars().count()`, and `last_line_length` counts chars — matching Dart's
  UTF-16-unit `substring` (identical for BMP text). Byte-based slicing
  panics mid-codepoint and undercounts carets on non-ASCII lines.
- `FileSpan` cannot derive `Hash` (its `file` field holds `&str`/`&[usize]`
  references). Where a `HashMap<FileSpan, _>` was wanted but only iterated, a
  `Vec<(FileSpan, _)>` is used instead. `BOGUS_SPAN` reports `file() == None`,
  `text() == ""`, and `len() == 0`.

### Lazy and multi spans

```rust
pub struct LazyFileSpan<'parse> { builder: Cell<Option<Rc<dyn Fn() -> SassResult<FileSpan<'parse>> + 'parse>>>, cached: Cell<Option<FileSpan<'parse>>> }

pub struct MultiSpan<'parse> { primary: Box<Span<'parse>>, primary_label: String, secondary_spans: Vec<(FileSpan<'parse>, String)> }

pub enum Span<'parse> { File(FileSpan<'parse>), Lazy(LazyFileSpan<'parse>), Multi(MultiSpan<'parse>) }
```

`LazyFileSpan` defers span resolution: the builder is `Rc<dyn Fn>` (shared,
not consumed on success) and the cache is a `Cell` (because `FileSpan` is `Copy`, `get()`
returns by value with no borrow guard). `Clone` shares the `Rc`; `Debug`
resolves via `get()`. Error-path caveat: if the builder _fails_, it is not
restored — a retry reports `"LazyFileSpan builder already consumed…"` instead
of re-running (Dart retries; no production path hits this). `Span` is the union; it implements `Clone` (manual) and
`Debug` (manual for `LazyFileSpan`/`MultiSpan`). `MultiSpan`'s `primary` is
`Box<Span>` (recursion), and `secondary_spans` is a `Vec` (not a `HashMap`)
because maps are small and `Span` has no `Hash + Eq`. Utility methods return
`Result<_, SpanError>` because the `Lazy` builder can fail. `file_span()`
resolves `Lazy → FileSpan` on demand. `Interpolation.span` is the one AST-node
span typed as `Span<'parse>` rather than `FileSpan<'parse>` — matching Dart's
`LazyFileSpan` stored inside `Interpolation`; its constructors take
`impl Into<Span<'parse>>` via `From<FileSpan> for Span`.

## Scanner

`SpanScanner` is the character-level scanner used by the parser:

```rust
pub struct SpanScanner<'parse> { source: &'parse FileSource<'parse>, pos: usize, line: usize, column: usize }
pub struct LineScannerState { position: usize, line: usize, column: usize }  // Copy
```

`pos` is a **byte** offset (not a character index); multi-byte UTF-8 advances
`pos` by the byte length but `line`/`column` by one. Key API: `text()`,
`source_url()`, `is_done()`, `pos()`, `rest()`, `peek_char` (returns `i32`, `-1`
at EOF; `peek_char(-1)` decodes backwards in O(1)), `read_char` (→
`SpanScannerResult<char>`), `scan_char`/`expect_char`, `scan`/`expect`,
`expect_done`, `substring`, `state`/`set_state`, `set_position`,
`span_from`/`span_from_to`/`span_from_pos`, `empty_span`, `location`, `error`.
Scanner errors are `SpanScannerError` (not `SassError`), so the parser can
adjust spans.

## AstNode and CssValue

```rust
pub trait AstNode<'parse> { fn span(&self) -> SassResult<FileSpan<'parse>>; }
pub struct FakeAstNode<'parse> { callback: Box<dyn Fn() -> SassResult<FileSpan<'parse>> + 'parse> }
pub struct CssValue<'parse, T> { pub value: T, span: FileSpan<'parse> }
```

`AstNode` is not used for visitor dispatch (that is enums) — it exists so
heterogeneous node types can report their span. `FakeAstNode` provides a span
from a callback. `CssValue` pairs a value with a span; equality ignores the
span (matching Go/Dart).

## File mapping

| Dart                                          | Go                                             | Rust                                                         |
| --------------------------------------------- | ---------------------------------------------- | ------------------------------------------------------------ |
| `lib/src/exception.dart`                      | `go/sasscommon/exception.go`, `core_errors.go` | `src/common/exception.rs`, `core_errors.rs`, `span_error.rs` |
| `(external) package:source_span`              | `go/sasscommon/source_span_*.go`               | `src/common/source_span_*.rs`, `file_span.rs`, `span.rs`     |
| `(external) package:string_scanner`           | `go/sasscommon/string_scanner_span_scanner.go` | `src/common/span_scanner.rs`                                 |
| `(external) package:path` (`prettyUri`)       | `go/sasscommon/pretty_uri.go`                  | `src/common/pretty_uri.rs`                                   |
| `(external) dart:core` (`ArgumentError` etc.) | `go/sasscommon/core_errors.go`                 | `src/common/core_errors.rs`                                  |
| `lib/src/ast_node.dart`                       | `go/sasscommon/ast_node.go`                    | `src/common/ast_node.rs`                                     |
