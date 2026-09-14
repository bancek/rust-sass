# Module: `compile_context.rs`

The identity token for a single compilation.

## `CompileContext`

```rust
pub type CompileContext = Rc<()>;
pub fn new_compile_context() -> CompileContext;   // Rc::new(())
```

`SassFunction`/`SassMixin` values capture the context at creation;
`meta.call`/`meta.apply` assert it matches the current compilation before
invoking the wrapped callable (same-compilation guard). Comparison is
`Rc::ptr_eq` — matching Go's `any`-holding-`&struct{}{}` (interface identity)
and Dart's `final Object _compileContext = Object()` (object identity;
`dart-source: lib/src/visitor/evaluate.dart`).

## Why `Rc<()>`, not arena

The context must be comparable across value lifetimes (`Value<'parse>` is
invariant, so a `SassFunction` from one lifetime is compared against another)
and must outlive any single arena borrow — including across the `'static`
embedded boundary. An arena `&'parse ()` would tie identity to a borrow;
`Rc<()>` gives a cheap cloneable token with pointer identity. This is one of
the sanctioned `Rc` uses (see `critical-invariants.md` "Rc, not Arc").

## Working here

- New cross-compilation guards go through `assert_compile_context` on the
  value types (`value/function.rs`, `value/mixin.rs`), not through direct
  `Rc::ptr_eq` at call sites.
- Unit tests live in-file (`compile_context.rs:27-44`); identity is
  covered by `test_same_context_is_ptr_eq` /
  `test_different_contexts_are_not_ptr_eq`.
