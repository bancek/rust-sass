# Deferred deprecation warnings

The pattern for code that cannot reach `EvalConfig` + `EvalState` but must emit
a deprecation warning.

## Why the pattern exists

Three implementations face the same problem — getting a deprecation warning from
utility code to the logger, with dedup, quiet-deps, and stack trace:

| Lang | Mechanism                                                                                         |
| ---- | ------------------------------------------------------------------------------------------------- |
| Dart | Zone-based ambient `EvaluationContext.current`; utility code calls `warnForDeprecation()`.        |
| Go   | An explicit `*EvaluationContext` threaded everywhere, or a stored `warnFn` field on the importer. |
| Rust | `BufferedWarnLogger` — buffer the warning, return, flush through the eval pipeline.               |

**Why Rust can't store a `warn_fn` closure (the core reason):** importers live
inside `ImportCache`, which lives inside `EvalState`. A stored `warn_fn` closure
would need to capture `&mut EvalState` while the importer is already transitively
borrowed from `state` — a self-referential borrow the checker rejects.

**Why Rust can't use Dart's Zone:** there is no global/thread-local ambient
context anywhere in the codebase.

**Why even passing `warn` as a parameter fails:** a caller that builds
`|msg, dep| warn_deprecation(config, state, msg, dep)` borrows `state` mutably
through the closure; if the caller needs `state` again after the call, the
borrow is rejected (e.g. `sass_index_to_list_index`).

## Types

```rust
pub trait WarnLogger<'parse> { fn warn_deprecation(&self, message: &str, deprecation: &'static Deprecation, span: Option<FileSpan<'parse>>); }

pub struct BufferedWarnLogger<'parse> { pending: &'parse RefCell<Vec<(String, &'static Deprecation, Option<FileSpan<'parse>>)>> }
pub struct NoOpWarnLogger;   // discards, used when no eval context is available
```

`WarnLogger` takes `&self` (implementors use interior mutability) and returns
`()` — no error propagation, no eval dependency. `flush_buffered_warnings`
drains the buffer and routes each entry through `warn_deprecation`.

## Three tiers (and three signatures)

1. **Direct** — code with `EvalConfig` + `EvalState` calls the free functions in
   `eval/warn.rs` (`warn`, `warn_deprecation`, `warn_deprecation_span`,
   `warn_deprecation_multi_span`).
2. **Deferred** — importers and `value/` helpers take `&dyn WarnLogger`, write
   into a `BufferedWarnLogger`, and the caller flushes it afterward.
3. **Selector bridge** — `selector::WarnLogger` (a separate trait with a `&mut
self` live-borrow signature) is bridged to the eval pipeline via
   `WarnLoggerAdapter`.

The three `warn` surfaces have distinct signatures: `logger::WarnLogger` (`&self`,
interior mutability), `selector::WarnLogger` (`&mut self`, live borrow), and
`eval::warn::warn_deprecation` (a free function taking `&EvalConfig, &mut
EvalState`).

## File mapping

| Dart                                     | Go                                                | Rust                                            |
| ---------------------------------------- | ------------------------------------------------- | ----------------------------------------------- |
| `lib/src/evaluation_context.dart` (zone) | `go/evalcontext/evaluation_context.go` (`warnFn`) | `src/logger/warn_logger.rs`, `src/eval/warn.rs` |
