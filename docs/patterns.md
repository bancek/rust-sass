# Patterns and conventions

This document records the translation patterns used across the port and the
conventions that every contributor should follow. It is written from the
perspective of the code as it exists — "what" the conventions are and "why"
they exist — not as a porting tutorial.

## 1. Design philosophy

Two principles govern the whole codebase:

- **Dart is the source of truth.** Where Dart, Go, and Rust disagree, Dart wins.
  The Go port is a structural reference (it flattened Dart's class hierarchy
  into interfaces); the Rust port re-adds ownership on top of that flattening.
- **Structural fidelity.** The Rust code mirrors the Dart/Go structure, not
  merely its output. If Go uses the serializer for `to_css_string()`, Rust does
  too — no inlined simplifications. This keeps the port auditable against
  upstream and keeps `// dart-source:` annotations meaningful.

## 2. Source annotations

Every ported file carries header annotations naming its origins:

```rust
// dart-source: lib/src/exception.dart
// go-source: go/sasscommon/exception.go
```

For external packages or multiple sources:

```rust
// dart-source: (external) package:source_span/lib/src/file.dart + lib/src/util/span.dart
```

`// Matches Dart:` marks a specific line or block that was cross-checked against
Dart. `mod.rs`, `lib.rs`, and `Cargo.toml` are not annotated.

Where a file's mapping is not a bare 1:1 path, a **parenthetical qualifier**
documents the relationship in place, e.g.
`// go-source: go/sasscommon/core_errors.go (errors) + go/sasscommon/exception.go (SassError wrapping)`
or `(external) package:source_span/lib/src/file.dart`. The qualifiers used in
the codebase include `(errors)`, `(inside EvaluateVisitor)`, `(interface only)`,
`(test helper)`, `(not present in Dart/Go)`, and `(standard) dart:core`.

**Why:** these annotations are the reverse index used to back-port upstream
dart-sass changes. `porting.md` builds a changed-Dart-file → Rust-file mapping
from them, and the license-header policy relies on every annotation resolving to
a real Dart source. They are a live, maintained part of the code — not
archaeology.

Files ported from libsass instead of Dart carry `// libsass-source:` with the
same shape (paths are relative to the `libsass/` submodule checkout), e.g.
`// libsass-source: src/sass_context.cpp (option surface) +
include/sass/context.h`, plus the upstream copyright header
(`Sass Open Source Foundation`, MIT — see `libsass/LICENSE`). Only
`rust-sass-libsass` uses this annotation today; its `lib.rs` stays
unannotated like every other crate root.

## 3. Type-system translation

Dart's sealed class hierarchy and Go's interface system both become Rust enums:

| Dart / Go                               | Rust                                                                      |
| --------------------------------------- | ------------------------------------------------------------------------- |
| `Value` interface + `v.(*SassMap)`      | `Value<'parse>` enum + `match`                                            |
| `Statement` interface + type assertion  | `Statement<'parse>` enum + `match`                                        |
| `Expression` interface + type assertion | `Expression<'parse>` enum + `match`                                       |
| `Selector` interface + type assertion   | `Selector<'parse>` enum + `match`                                         |
| `CssNode` interface + type assertion    | `CssNode` enum + `match`                                                  |
| `interface{}` / `any`                   | a typed enum or concrete struct                                           |
| `nil` interface                         | `Option<T>`                                                               |
| `nil, nil` not-found                    | `SassResult<Option<T>>`                                                   |
| `nil []Statement` vs `[]Statement{}`    | `Option<Vec<Statement>>` (`None` = no block, `Some(vec![])` = empty `{}`) |

The one place a Dart `any` return survives is `visitCalculationExpression`,
which in Go returns `any` (a `Value` or a `CalculationOperation`); Rust models it
as the `EvalResult::Value | EvalResult::CalcOp` enum, so Go's `sprint_any()`
duck-typing helper is unnecessary and deliberately not ported.

## 4. Dispatch

| Go                                                        | Rust                                                               |
| --------------------------------------------------------- | ------------------------------------------------------------------ |
| `stmt.AcceptValue(v)` / `AcceptBool(v)` / `AcceptVoid(v)` | `stmt.accept(&mut v)` — one method, `V::Output` selects the result |
| `expr.AcceptExpr(v)` (ReplaceExpressionVisitor)           | `expr.accept(&mut v)` with `V::Output = Expression`                |
| `switch v.(type)`                                         | `match` on enum (exhaustive)                                       |

Concrete `Output` types in use (the evaluator itself is not a visitor —
it dispatches through free `fn(config, state, arena, …)` + `match`):

| Visitor                                          | `Output`             |
| ------------------------------------------------ | -------------------- |
| `RecursiveAstVisitor`, `FindDependenciesVisitor` | `()`                 |
| `SerializeVisitor` (all impls)                   | `()`                 |
| `ReplaceExpressionVisitor`                       | `Expression<'parse>` |
| `IsPlainCssVisitor`, `IsCalculationSafeVisitor`  | `bool`               |

## 5. Ownership and lifetimes

| Go (GC-managed)                                | Rust                                                                                                                                         |
| ---------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| String slices `file.text[start:end]`           | `&'parse str` from the bumpalo arena                                                                                                         |
| Shared mutable references (Environment frames) | arena `&'parse RefCell<IndexMap>` (`Copy` refs)                                                                                              |
| `reflect.ValueOf(ref).Pointer()` for identity  | `Callable::identity_eq` / `identity_hash` (address of the `CallableKind` allocation); `CompileContext = Rc<()>` for compile-context identity |
| `defer` save/restore                           | `SavedContext` struct or manual field assignment                                                                                             |
| Closure capturing `v *EvaluateVisitor`         | free `fn(config: &EvalConfig, state: &mut EvalState, ...)` (`&self` splits into `&self.config` + `&mut self.state` at the call site)         |
| `*int` nil-pointer lazy hash cache             | `Cell<Option<usize>>`                                                                                                                        |
| Shallow map copy (GC pointers)                 | arena `&'parse` references                                                                                                                   |

Data structures:

| Go                | Rust                                    |
| ----------------- | --------------------------------------- |
| `linkedhashmap`   | `indexmap::IndexMap`                    |
| `map[T]bool` set  | `HashSet<T>`                            |
| `strings.Builder` | `write!(buf, ...).unwrap()` on `String` |

Buffer writes are the only place a `.unwrap()` is permitted — writing to a
`String` can never fail, matching Go's `_, _ = sb.WriteString()`.

## 6. Value serialization naming

| Go                                                   | Rust                                    |
| ---------------------------------------------------- | --------------------------------------- |
| `v.String()` (Dart `toString()`, list parens)        | `v.to_display_string()`                 |
| `value.SerializeValueInspect(v)` (== `meta.inspect`) | `serialize::serialize_value_inspect(v)` |
| `v.ToCssString(quote)`                               | `v.to_css_string(quote)`                |

These three are distinct and must not be conflated: `to_display_string()` adds
Dart's list parens, `serialize_value_inspect` is the raw serializer, and
`to_css_string` errors on non-CSS values (see §7).

## 7. Error handling

| Go                                       | Rust                                                                                                                                             |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `addExceptionSpan(v, node, func(){...})` | `add_exception_span(config, state, span, add_stack_frame, async \|config, state\| { ... })` (dual `sync_impl`/`async_impl` in `eval/helpers.rs`) |
| `SassScriptException` (unspanned)        | `SassError::Script`                                                                                                                              |
| `SassRuntimeException`                   | `SassError::Runtime { message, span, trace, .. }`                                                                                                |
| `throwWithTrace(err, cause)`             | `SassError::Runtime` with `trace` from `stack_trace()`                                                                                           |
| `errors.AsType[*T](err)`                 | `match err` on the `SassError` enum                                                                                                              |

Errors are asserted **exactly** — never `is_err()`/`is_ok()` without checking the
message or variant. Rust tests match `SassError`/`ParseError` enum fields rather
than string-matching `Display` output, which is more precise and does not depend
on box-drawing characters or line numbers.

## 8. Testing

### Golden values

For behavior whose correctness is a specific number, capture the real output and
hardcode it — never recompute the expected value from the same formula as the
code under test. The workflow: print the value at `%.16g`, copy it into the
test, assert `abs(got - want) < 1e-12`, and delete the printer.

Weak assertions fall into four classes, each a signal that a golden value is
missing:

- **COMPUTED** — the expected re-derives the same formula (passes even with
  wrong constants).
- **RANGE** — only checks `> 0` / `!= input` (passes with any value).
- **MISSING** — checks the type/space but no numeric channel values.
- **METADATA** — string/bool checks (`name`, `isBounded`); acceptable as-is.

### Coverage

Implementation and tests should have comparable line counts — a 1000-line
implementation needs roughly 1000 lines of test code. Every happy path, error
path, and syntax variant needs explicit coverage.

## 9. Logger and deprecation

`Deprecation` constants are defined once in `deprecation.rs`; parsers that need
deprecation warnings store two handles:

```rust
pub type WarnDeprecationFn<'parse> =
    Box<dyn Fn(&str, &'static Deprecation) -> ParseResult<'parse, ()> + 'parse>;
```

- `logger: Option<&dyn Logger>` — for standalone use.
- `warn_deprecation_fn: Option<&WarnDeprecationFn>` — set by the evaluator for
  stack-trace-aware warnings; it wins when present.

These handles are threaded as explicit `Option` parameters through the parser
free functions. Nested parser calls that do not themselves introduce a
deprecation pass `(None, None)` — the deprecation only fires in the outermost
selector.

The `Logger` trait has **no `'parse` parameter** — its methods are generic
(`fn warn<'a>(&self, ..., span: Option<&Span<'a>>, trace: Option<&Trace>)`).
Only `Span` borrows; the `Trace` it carries owns its frames, so this lifetime
erasure is what makes `Rc<dyn Logger>` usable in `EvalConfig` without threading
the arena through every logger call site. Code that cannot reach
`EvalConfig`/`EvalState` (importers, `value/` helpers) buffers deprecation
warnings through `BufferedWarnLogger` and flushes them later; see
`ref/warn-logger.md`.

## 10. Importer and module shape

| Go                                        | Rust                                                                                                                                        |
| ----------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `Importer` interface                      | `enum ImporterKind { Filesystem, NoOp, User(Rc<dyn UserImporter>), Package, NodePackage }` + `Copy` handle `Importer(&'parse ImporterKind)` |
| Built-in + user impls behind an interface | built-in arms inline; only the `User` arm boxes                                                                                             |
| `Module` interface (pointer identity)     | `enum ModuleKind { BuiltIn, Forwarded, Shadowed, Environment(Box<…>) }` + `Copy` handle `Module(&'parse ModuleKind)`                        |

Only the genuinely open, host-supplied seams are `dyn`: `Logger`,
`UserImporter` (plus `Io`/`PackageConfig` at the seams). Everything else is
a closed enum with address identity (`std::ptr::eq`).

## 11. Import style

All imports live at module tops as absolute paths; bodies never qualify:

- `use crate::…` — never `super::`/`self::` and never function-local `use`
  (including `use std::fmt::Write;` inside `fn fmt` and per-test-fn imports,
  which hoist to the `mod tests` top). Absolute paths keep every reference
  greppable (`rg 'use crate::value::Value'`) and make file moves mechanical.
- A child module never sees its parent's `use` imports, so each `mod tests`
  carries its own (a `use super::*` glob does re-import them — verified by
  experiment — but explicit imports are preferred for greppability).
- `std::` utility namespaces stay qualified: `std::mem::replace`,
  `std::ptr::eq`, `std::fs::write` (standard Rust idiom; bare verbs pollute
  scope). Types, consts, and traits are imported instead (`HashSet`, `Path`,
  `PI`, `Rc`, `fmt::Write` — either directly or via the parent module import
  as in `use std::fmt;`).
- Never import a prelude name (`Result`, `Option`, `Box`, `String`, `Vec`,
  …): the import silently shadows the prelude with no compiler error, so
  `std::fmt::Result` stays qualified wherever it appears.
- On a genuine same-name/different-item collision, alias the narrower side
  (`std::io::Write as IoWrite`, proto `SourceLocation as ProtoSourceLocation`)
  rather than leaving either site qualified.
- `#[cfg]`-gated code keeps gated imports (`#[cfg(test)]`,
  `#[cfg(not(feature = "async"))]`, …); never hoist a gated `use` to an
  ungated position, and never move an import above another item's attributes.
- Exempt: intra-doc links (`/// [`Io`]: crate::io::Io`), comments, and
  generated code (`rust-sass-embedded-pb`, rewritten by its generator).
