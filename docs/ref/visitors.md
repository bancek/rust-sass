# Visitor traits

The nine visitor traits and the `accept()` dispatch that ties them to the AST
enums.

## The traits

| #   | Trait                                  | Methods | `Output` types in use                                                                           |
| --- | -------------------------------------- | ------- | ----------------------------------------------------------------------------------------------- |
| 1   | `StatementVisitor<'parse>`             | 27      | `()` (eval, search)                                                                             |
| 2   | `ExpressionVisitor<'parse>`            | 18      | `()` (eval), `bool` (IsPlainCss, IsCalcSafe), `Expression` (ReplaceExpr)                        |
| 3   | `ValueVisitor<'parse>`                 | 10      | `()` (serialize)                                                                                |
| 4   | `CssVisitor<'parse>`                   | 9       | `()` (serialize), `bool` (EveryCss)                                                             |
| 5   | `SelectorVisitor<'parse>`              | 11      | `()` (serialize), `bool` (AnySelector, Search)                                                  |
| 6   | `IfConditionExpressionVisitor<'parse>` | 6       | `Value` (eval), `()` (RecursiveAst), `IfConditionExpression` (ReplaceExpr), `bool` (IsPlainCss) |
| 7   | `InterpolatedSelectorVisitor<'parse>`  | 11      | `()` (RecursiveAst)                                                                             |
| 8   | `ModifiableCssVisitor<'parse>`         | 9       | `()` (serialize/eval)                                                                           |
| 9   | `CloneCssVisitor<'parse>`              | 9       | `ModifiableCssNode` (clone)                                                                     |

All traits use `&mut self` methods with `type Output`, and all return
`SassResult` — no visitor is infallible.

## `accept()` dispatch

```rust
impl<'parse> Statement<'parse> {
    pub fn accept<V: StatementVisitor<'parse> + ?Sized>(&self, visitor: &mut V) -> SassResult<V::Output> {
        match self { Statement::Stylesheet(s) => visitor.visit_stylesheet(s), /* ... */ }
    }
}
```

One generic `accept()` per AST enum, dispatching via `match` to the concrete
`visit_*` method. `V::Output` selects the result type, so a single method
replaces Go's `AcceptValue`/`AcceptBool`/`AcceptVoid`/`AcceptExpr` families.

## Object safety

All nine traits are object-safe (no `Self` returns, no method-level generics) —
but none are used as `dyn`. Dispatch is always the `match` in `accept()`, so the
visitors are never trait objects at runtime.

## `ArgumentList → visit_list`

Go's `VisitList(ListValue)` accepts both `SassList` and `SassArgumentList`; Rust
collapses them by having `Value::ArgumentList` dispatch to `visit_list` with
`&arg_list.list` (the embedded `SassList`), so `ValueVisitor` needs a single
list method (`visit_list(&ListValue<'_, 'parse>)`).

## Concrete implementations

| Visitor                                        | Implements                                               | `Output`            |
| ---------------------------------------------- | -------------------------------------------------------- | ------------------- |
| `SerializeVisitor`                             | Css, Value, Selector                                     | `()`                |
| `RecursiveAstVisitor`                          | Statement, Expression, IfCondition, InterpolatedSelector | `()`                |
| `ReplaceExpressionVisitor`                     | Expression, IfCondition                                  | `Expression`        |
| `IsPlainCssVisitor`                            | Expression, IfCondition                                  | `bool`              |
| `IsCalculationSafeVisitor`                     | Expression                                               | `bool`              |
| `FindDependenciesVisitor`                      | Statement                                                | `()`                |
| `StatementSearchVisitor`                       | Statement                                                | `bool`              |
| `AnySelectorVisitor` / `SelectorSearchVisitor` | Selector                                                 | `bool`              |
| `EveryCssVisitor`                              | Css                                                      | `bool`              |
| `CloneCssVisitor`                              | CloneCss                                                 | `ModifiableCssNode` |

There is no `EvaluateVisitor` row: the evaluator implements none of these
traits and dispatches through free `fn(config, state, arena, …)` + `match`
(see `architecture.md` §4, `ref/eval.md`). There is no
`SourceInterpolationVisitor` either: Dart's is inlined as per-node
`source_interpolation() -> Option<&Interpolation>` stubs on `Expression`.
Two Dart-exact edges: `FindDependenciesVisitor` only records a `load-css`
call with exactly one positional argument (Dart's `case [StringExpression]`
— extra args are not statically analyzable); `IsCalculationSafeVisitor`'s
string checks index `chars` (Dart's UTF-16 `codeUnitAtOrNull(1/3)`), so
non-ASCII text like `é+foo` disagrees with byte indexing. `ReplaceExpressionVisitor`'s
unknown-`SupportsCondition` arm returns `Err(SassError::Sass)` (Dart throws a
catchable `SassException`), never panics.

## File mapping

| Dart                               | Go                     | Rust                                   |
| ---------------------------------- | ---------------------- | -------------------------------------- |
| `lib/src/visitor/interface/*.dart` | `go/value/visitor*.go` | `src/ast/*/visitor.rs`, `src/visitor/` |
