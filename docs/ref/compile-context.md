# Module: `compile_context.rs`

The identity token for a single compilation.

## `CompileContext`

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CompileContext(u64);

static NEXT_COMPILE_CONTEXT: AtomicU64 = AtomicU64::new(1);

pub fn new_compile_context() -> CompileContext   // NEXT.fetch_add(1, Relaxed)
```

`SassFunction`/`SassMixin`/`SassModule` values capture the context at creation;
`meta.call`/`meta.apply` assert it matches the current compilation before
invoking the wrapped callable (same-compilation guard). Comparison is `==` on
the `u64`. This matches Go's `any`-holding-`&struct{}{}` (interface identity) and
Dart's `final Object _compileContext = Object()` (object identity;
`dart-source: lib/src/visitor/evaluate.dart`).

## Why a counter, not a pointer or `Rc`

The token must be comparable across value lifetimes (`Value<'parse>` is
invariant, so a `SassFunction` from one lifetime is compared against another)
and must stay valid after the arena is gone. The values that hold it live in the
arena, and `bumpalo` never runs `Drop`, so an `Rc<()>` handle stored there would
never be released — it leaked one 16-byte allocation per compilation.

A monotonic `u64` gives the same identity semantics as an object pointer while
remaining a plain `Copy` value. Ids are never reused, unlike heap addresses,
and `target_has_atomic = "64"` holds on wasm32. `Relaxed` ordering is enough:
only uniqueness matters. This is the codebase's only process-global
mutable state, and it is deliberate (`critical-invariants.md`, "No static
caches").

## Working here

- New cross-compilation guards go through `assert_compile_context` on the
  value types (`value/function.rs`, `value/mixin.rs`, `value/module.rs`), not
  through direct comparison at call sites.
- Unit tests live in-file; identity is covered by `test_same_context_is_equal`
  / `test_different_contexts_are_not_equal`.
