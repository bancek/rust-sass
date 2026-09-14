# Modules: `logger/`, `deprecation.rs`

Logging and deprecation handling.

## The `Logger` trait

```rust
pub trait Logger: fmt::Debug {
    fn warn<'a>(&self, message: &str, span: Option<&Span<'a>>, trace: Option<&Trace>);
    fn debug<'a>(&self, message: &str, span: Option<&Span<'a>>);
    fn warn_deprecation<'a>(&self, message: &str, span: Option<&Span<'a>>, deprecation: &'static Deprecation, trace: Option<&Trace>) -> SassResult<()>;
}
```

- No `Send`/`Sync` — the compiler is single-threaded (`!Send`), so `Rc`
  (not `Arc`) is used throughout.
- The stack trace is carried **structurally** as `Option<&Trace>` (Dart's
  `Trace?`), not as a pre-formatted string. `Trace` owns its `Frame`s, so it
  never borrows the `'parse` arena; the trait methods are generic over the span
  lifetime only (`fn warn<'a>(...)`), which is what makes `Rc<dyn Logger>`
  usable in `EvalConfig` without threading the arena through every call site.
  Loggers stringify via `Trace::format(io)` — Dart's `Trace.toString()` — only
  at the render boundary (`logger/stderr.dart`, embedded, wasm, libsass).
- `Logger` is one of several `dyn` seams (`UserImporter`, `Io`,
  `PackageConfig`, callbacks); it is the only _host-supplied_ one besides
  `UserImporter`.

### Implementations

- `QuietLogger` — a no-op (used by `--quiet` / `Logger.silent`).
- `StderrLogger` — writes to an internal `write_fn` callback (defaults to stderr).
- `TrackingLogger<I>` — decorates a logger with `Cell<bool>` emitted flags.
- `DeprecationProcessingLogger<I>` — decorates a logger, applying the
  silence/fatal/future deprecation lists and a repetition limit.

`new_default_logger(unicode)` returns a `StderrLogger` that detects ANSI color
support from `TERM`/`COLORTERM`/`is_terminal()`.

## Deprecation

```rust
pub struct Deprecation {
    pub id: &'static str,
    pub deprecated_in: Option<&'static str>,
    pub description: Option<&'static str>,
    pub obsolete_in: Option<&'static str>,
    pub is_future: bool,
}
```

~31 `pub const` instances (`CALL_STRING`, `SLASH_DIV`, `IMPORT`, …).
`Deprecation` derives `Hash + Eq` so `DeprecationProcessingLogger` can use it as
a map key. `from_id(id)` is a 31-arm `match` (no `HashMap`);
`for_version(version)` iterates the `ALL` slice. `Deprecation` is the type
threaded through parsers via `WarnDeprecationFn` (see `patterns.md` §9).

## File mapping

| Dart                       | Go                              | Rust                 |
| -------------------------- | ------------------------------- | -------------------- |
| `lib/src/logger/*.dart`    | `go/sasslogger/*.go`            | `src/logger/*.rs`    |
| `lib/src/deprecation.dart` | `go/deprecation/deprecation.go` | `src/deprecation.rs` |
