# Module: `rust-sass-macros`

The `maybe_async` proc-macro crate behind the sync/async dual build. A trimmed,
patched fork of [`maybe-async`](https://github.com/fMeow/maybe-async-rs) 0.2.11
(MIT, © 2020 Guoli Lyu).

## Quick usage

```rust
use rust_sass_macros::{maybe_async, maybe_block_on, maybe_test, sync_impl, async_impl};

#[maybe_async]                 // async build: unchanged; sync build: async/await stripped
async fn compile(...);

#[sync_impl]                   // same-name pairs for AsyncFnOnce-bounded helpers
fn helper(...);                // (async twin under #[async_impl])

#[maybe_test]                  // runs as #[test] (sync) or #[futures_test::test] (async)
async fn test_thing() { ... }

maybe_block_on!(some_fn());    // drives a future in async builds; identity in sync
```

## Mode selection

Sync is the default; `async` is the only real feature:

```toml
# rust-sass/Cargo.toml
[features]
default = ["sync"]                        # documentary marker only
sync = []                                 # the default (absence of async)
async = ["rust-sass-macros/async"]        # the real switch
```

Every crate aliases the same feature (`async = ["rust-sass/async"]`), so unified
workspace graphs expand consistently and there is no mixing hazard. **Never gate
library-call shaping on a consumer-local `cfg`** — a crate cannot see a
dependency's resolved features, so use the macros (which read the mode from the
macro crate's feature) instead.

## Macro surface

| macro                                   | async build             | sync build                                                                                                                                       |
| --------------------------------------- | ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `#[maybe_async]`                        | identity                | strip `async`/`await`; rewrite `-> LocalBoxFuture<'a, T>` (and `Pin<Box<dyn Future>>`, `impl Future`) to `-> T`; clear async-closure `asyncness` |
| `#[must_be_sync]`                       | —                       | same conversion, unconditional                                                                                                                   |
| `#[sync_impl]`                          | item deleted            | item kept                                                                                                                                        |
| `#[async_impl]`                         | item kept               | item deleted                                                                                                                                     |
| `#[maybe_test]`                         | `#[futures_test::test]` | `#[test]`                                                                                                                                        |
| `maybe_block_on!(expr)`                 | `block_on(expr)`        | `expr`                                                                                                                                           |
| `box_rec!(f)` / `box_rec_in!(f, arena)` | boxed recursion edge    | identity                                                                                                                                         |

`sync_impl`/`async_impl` pairs share a name and express things the keyword
stripper cannot convert (e.g. `AsyncFnOnce`-bounded combinators). `box_rec!`
boxes a recursion edge; it must never wrap `.await` — extract the body into a
`#[maybe_async]` fn and write `box_rec!(impl_fn(...))`.

## The two patches

1. **Async closures** — syn 2's `ExprClosure` has no `Signature`, so
   `visit_signature_mut` never fires for closures; the fork clears `asyncness`
   **and** rewrites `output: ReturnType` via `extract_future_output`.
2. **`LocalBoxFuture` alias** — `extract_future_output` matches the last path
   segment `"LocalBoxFuture"` (in addition to `Pin`/`Box`); for the alias the
   output type is the last generic argument (the first is a lifetime).

Deleted from upstream: `async-trait` integration, `ReplaceGenericType`,
arbitrary-condition `test` parser. Added: `maybe_test` (hardcoded conditions),
`maybe_block_on!`.

## Pitfalls

- **Macro tokens are opaque.** `.await` inside any macro invocation survives
  conversion (E0728): `assert!(x.await)`, `vec![f().await]`,
  `box_rec!(async move { g().await })`. Hoist to a `let` binding, or extract the
  body into a marked fn.
- **`?` in an async block changes meaning.** In async mode `?` returns from the
  block; after conversion it would return from the enclosing function. Extract
  such blocks into a marked fn.
- **`AsyncFnOnce`/`async |..|` bounds are never rewritten** — use
  `sync_impl`/`async_impl` pairs.
- **Lifetime invariance** — test helpers that call maybe_async helpers need
  `'compile: 'parse, 'parse: 'compile`.
- **Consumer-crate `cfg` blindness** — `cfg(feature = "sync")` in
  cli/embedded/spec/wasm checks _that_ crate's feature, not rust-sass's mode;
  mode-dependent library-call code uses the macros.
- **Recursion edges** — async recursion requires boxing (E0733); use
  `box_rec!`/`box_rec_in!`, never raw `Box::pin` (unconvertible in sync mode).
- **Embedded sync-primary mailbox** — the embedded server is sync-primary; its
  per-compilation mailbox is cfg-paired (`futures::channel::mpsc` + `.await` vs
  `std::sync::mpsc` + blocking `recv()`), and `send_and_wait` is an
  `async_impl`/`sync_impl` pair.

## Sync-build failure triage

| error                                           | cause                                         | fix                                                |
| ----------------------------------------------- | --------------------------------------------- | -------------------------------------------------- |
| E0728 `await` only in async fn                  | unmarked fn / await in macro tokens           | mark the fn, or hoist the await                    |
| E0277 `X is not a future`                       | await stripped but callee not converted       | mark the callee `#[maybe_async]`                   |
| E0308 mismatched future return                  | unmarked fn/closure returning a future        | mark it                                            |
| E0599 no `unwrap`/`unwrap_err` on `impl Future` | same as E0308                                 | same                                               |
| "lifetime may not live long enough"             | missing `'parse: 'compile` on a test helper   | add both bounds                                    |
| no associated item `Sync`/`Async`               | sync build: `BuiltInCallback` is a type alias | call directly, or use the `invoke_callback` helper |
| both-features `compile_error`                   | `async` + `sync` enabled in one graph         | pick one (sync is the default)                     |
