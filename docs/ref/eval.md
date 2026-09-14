# Module: `eval/`

The evaluator: AST in, CSS output tree out. It resolves variables, imports,
modules, mixins, functions, and `@extend`.

## `EvalConfig` + `EvalState`

The evaluator is split into an immutable config and a mutable state, with all
logic in free functions (see `architecture.md` §7 for the rationale):

```rust
pub struct EvalConfig<'compile, 'parse> {
    pub built_in_modules: IndexMap<String, Module<'compile, 'parse>>,
    // Shared mutable via arena &'parse RefCell (Copy) — meta callbacks capture it:
    pub built_in_functions: &'parse RefCell<IndexMap<String, Callable<'compile, 'parse>>>,
    pub global_functions: &'parse RefCell<Vec<Callable<'compile, 'parse>>>,
    pub logger: Rc<dyn Logger>,
    pub compile_context: CompileContext,
    pub io: Rc<dyn Io>,
    // ... quiet_deps, source_map, max_recursion_depth, node_importer,
    // unicode, alert_color, alert_ascii
}

pub struct EvalState<'compile, 'parse> {
    pub env: Environment<'compile, 'parse>,
    pub stack: Option<Box<StackFrame<'compile, 'parse>>>,
    pub root: Option<ModifiableCssNode<'parse>>,
    pub parent: Option<ModifiableCssNode<'parse>>,
    pub import_cache: Option<ImportCache<'compile, 'parse>>,
    pub importer: Importer<'parse>,
    pub loaded_urls: Vec<String>,
    pub warnings_emitted: HashSet<WarnKey>,
    pub modules: IndexMap<String, Module<'compile, 'parse>>,
    // ... member, import/callable/default_warn spans, module tables,
    // media/at-root context, declaration flags, stylesheet, etc. (see eval/mod.rs)
}
```

The `built_in_functions` registry is `&'parse RefCell<…>` so meta-function
closures (which outlive `&mut self`) can access it; `logger`/`io` are
`Rc<dyn …>` host seams. There is no `EvalContext` struct — the transient
`default_warn_span` lives directly on state (see `with_evaluation_context`
in `helpers.rs`).

The `ImportCache` flows by **ownership, not borrowing**: it is created in
`compile_string`, passed into `evaluate()`, stored in `EvalState`, taken via
`state.import_cache.take()`, and returned on `EvaluateResult.import_cache` —
an `Option` (`Some` on success, `None` when evaluation fails before the
take) — since the compile pipeline always provides one.

### `StackFrame` and `WarnKey`

```rust
pub struct StackFrame<'compile, 'parse> { name: String, span: FileSpan<'parse>, callable: Callable<'compile, 'parse>, parent: Option<Box<StackFrame<'compile, 'parse>>> }

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct WarnKey { message: String, source_url: Option<Rc<str>>, start_offset: usize, file_id: usize }
```

The stack is a `Box`-linked list (owned exclusively by the evaluator). `WarnKey`
is the warning-dedup key; it uses `source_url + start_offset` instead of a
`FileSpan` because `FileSpan` cannot derive `Hash` (its `file` field holds
references), plus `file_id` (the `FileSource` address) to disambiguate
same-offset spans from different sources.

`StackFrame.callable` is production-unread (traces render `name`/`span`
only): user-defined invocations store the real callable so host seams (the
libsass C ABI) can classify mixin vs function frames from the declaration
and skip `dummy` frames (import/load machinery passes `None`), which have
no libsass counterpart.

## Borrow patterns

The free-function + config/state split supports 19 borrow patterns, each proven
by the evaluator's design. The ones that recur throughout the code:

- **Split borrow** at call sites: `&v.config` (shared) + `&mut v.state`
  (mutable) passed as separate function arguments.
- **Save/restore** combinators (`with_stack_frame`, `with_environment`,
  `with_parent`, `with_media_queries`, `with_css_style_rule`): take ownership of
  the old value, call the callback, restore.
- **Post-hoc wrapping** (`add_exception_span`, `add_error_span`,
  `add_exception_trace`): call the callback, then wrap a `Script` error
  with span + trace. `add_exception_trace` additionally dedups adjacent
  coincident trace frames (`dedup_trace_frames`, keeping the outermost) so
  load-site `@use`/`@forward` errors report one frame, matching Dart's trace.
- **`run_user_defined_callable`**: five nested closure levels, each reborrowing
  `state` through the chain.
- **Long-lived closures** (warn_fn, meta callbacks, import_cache deprecation):
  capture arena `&'parse RefCell<T>` handles (or `Rc` host handles), never
  `&mut state`.

## Function taxonomy

1. **Free functions** — the real logic: `fn(config, state, arena, ...) -> SassResult<T>`,
   dispatched via `match` (the evaluator implements no Sass-AST visitor traits).
2. **Methods on `EvaluateVisitor`** — constructors and entry points
   (`new`, `evaluate`, `set_variable`) only. `Evaluator::new` takes
   `Option<ImportCache>` directly (there is no `ImportCache.none()`
   sentinel — "no cache" is `None`, U13); it has `evaluate`/`set_variable`
   for REPL-style use but no `use` method (Dart REPL-only, no Rust caller —
   accepted gap, U13).

## Evaluation semantics

- **`@return` short-circuit:** `evaluate_block -> SassResult<Option<Value>>`
  stops iterating once a child returns `Ok(Some(_))`; `evaluate_return_rule`
  produces that `Some` (Go's `handleReturn`). No `return_value` field on state.
- **`@at-root` scoping** saves/restores six fields — `parent`,
  `style_rule_ignoring_at_root`, `at_root_excluding_style_rule`, `media_queries`,
  `media_query_sources`, `in_keyframes`, `in_unknown_at_rule` — in
  `scope_for_at_root` (mirroring Dart's `_scopeForAtRoot`). Media exclusion
  clears queries **and** sources (`_withMediaQueries(null, null)`); the
  unknown-at-rule check reads the post-`trim_included` list. The query itself
  comes from `perform_interpolation_with_map(..., warn_for_color: true)` plus
  `AtRootQuery::parse` with the threaded interpolation map.
- **`!global`** writes to frame 0 of the shared arena refs (`Vec<&'parse
RefCell<IndexMap>>`), so the
  write is visible to the caller (the `closure()` shared-frame semantics).
- **Rest-argument dispatch:** the evaluated rest value is type-dispatched —
  `SassMap` → named args (`add_rest_map`), `SassArgumentList` → positional +
  keywords, `SassList` → positional, anything else → a single positional value;
  a non-map keyword-rest errors at the **use-site** span
  (`keywordRestArgs.span`, not the resolved value node). `bind_arguments` then wraps
  the remaining positional and named args into a comma-separated
  `SassArgumentList`.
- **Parameter verification** (`verify_parameter_list`) reports source spellings
  (`Parameter::original_name`, e.g. `$foo_bar` not `$foo-bar`) for
  missing/both-position errors, and includes `"positional "` in the over-arity
  message when named arguments are present.
- **Function calls without `@return`** throw a spanned `Runtime` at the
  declaration span (`run_user_defined_callable` builds it via `exception()`,
  not `Script`), so the declaration span and trace survive outer wrappers.
- **Namespaced callables** resolve through `env.get_mixin`/`get_function`
  inside `add_exception_span(node.span)`, so a missing namespace reports
  Dart's `There is no module with the namespace "ns".` (a wrapped `Script`)
  at the call-site span.
- **Calculation arguments** (`expression_to_calc_argument`): no unary arm —
  every `UnaryOperation` hits the rejection default; unquoted strings pass the
  `is_calculation_safe` gate; a parenthesized `CalculationOperation` inside a
  space-separated list is rewrapped as `"(…)"` before joining. The
  `BinaryOperation` arm wraps `operate_internal` in `add_exception_span` with
  the operation span (mirroring Dart's `_addExceptionSpan` in
  `_visitCalculationExpression`): `calc()` incompatibilities report the
  whole-operation span, while `_visitCalculation` re-verifies multi-arg calls
  against the original nodes for per-operand `MultiSpan` labels (e.g.
  `min(1px, 1s)`).
- **`withoutSlash`:** a `/`-as-division `SassNumber` emits a deprecation warning
  and `without_slash()` strips the slash (recursing into `SassArgumentList`).
  It applies per destructured `@each` sub-item (and to the whole item first),
  and to variable-declaration values — but **not** to modern `if()` branch
  values, which return raw like Dart's `visitIfExpression`.
- **Rule-child scoping:** declaration/media/style-rule/keyframe/at-rule children
  scope with `has_declarations(children)` (never a hardcoded bool); `@each` loop
  variables are defined at the list expression node, `@for` bounds evaluate
  inside `add_exception_span` and define the loop variable at the `from`
  expression node; plain `@warn` evaluates its expression inside
  `add_exception_span(node.span)`; and variable-declaration RHS values evaluate
  bare (only `setVariable` is span-wrapped).
- **`@media` queries** evaluate via `perform_interpolation_with_map` and parse
  with the threaded map (`parse_list_with_map` →
  `CssMediaQueryParser::new_with_map`), mirroring Dart's `_visitMediaQueries`
  — errors inside `@media #{}` locate the interpolation site (with the
  "error in interpolated output" secondary span).
- **`load_module` returns `(Module, bool)`** rather than taking a callback: a
  callback closure would need to capture `&mut EvalState`, conflicting with
  `load_module`'s own `&mut state` borrow. `@import`, by contrast, bypasses
  module caching entirely (`load_stylesheet` in the current context).
- **`EvalResult`:** `visit_calculation_expression` returns
  `EvalResult::Value | EvalResult::CalcOp` — the enum that replaces Go's `any`
  return (and `sprint_any()`).
- **Reentrant meta functions:** `meta.call`/`meta.apply`/`meta.load-css` re-enter
  the evaluator through `pub(crate)` entry points — `call → invoke_callable`,
  `apply → apply_mixin` (extracted from `evaluate_include_rule`),
  `load-css → ImportCache::canonicalize`. Config-parsing errors from
  `evaluate_module`/`$with` must be wrapped with `add_exception_span` (otherwise
  a bare `SassError::Script` leaks through). `load-css` maps
  `$with: null → Configuration::empty()` and `$with: ()` to an explicitly
  empty config (empty lists count as empty maps per `SassList.assertMap`),
  never the caller's ambient configuration. The meta mixins (`load-css`,
  `apply`) are module-only — they are registered on `sass:meta` but not in
  `global_functions`, so `meta.function-exists("apply")` is `false`
  (`function-exists` itself is `env.exists || builtInFunctions[raw]` with no
  extra scan).

## CssVisitor bubbling

The evaluator's `CssVisitor` impl walks the parent chain via a `through`
predicate: style rules always bubble, but media rules pass through **only when
all their queries are in the merged source set** (`has_been_merged`). When a
target parent has a following sibling, `copy_without_children()` clones the
node first so the sibling is not corrupted. The CSS at-rule keyframes check
unvendors (`unvendor(name) == "keyframes"`, matching the statement path), so
vendor-prefixed keyframes take the keyframes path. `has_css_nesting` starts
from the style-rule node but returns `false` while
`at_root_excluding_style_rule` is set (mirroring Dart's `_styleRule` getter,
which is null in that state). On the imported-CSS path,
`visit_css_keyframe_block` returns "should never be called".

## Error flow

1. Type-check failures → `SassError::Script`.
2. `add_exception_span()` wraps a `Script` error with span + trace →
   `SassError::Runtime`.
3. `add_exception_span()` / `add_error_span()` / `add_exception_trace()`
   wrap a callback's `Script` errors with span + trace. All wrapping is
   callback-based (`eval/helpers.rs`); there is no helper that wraps an
   already-built `SassResult`.
4. At the API boundary, `emitErrorCss` decides whether to render the error as CSS
   (the four spanned variants `Sass`/`Runtime`/`Format`/`MultiSpan` — never
   the unspanned `Script`/`MultiSpanScript`).

## File mapping

| Dart                                                   | Go                                                   | Rust                                                                                   |
| ------------------------------------------------------ | ---------------------------------------------------- | -------------------------------------------------------------------------------------- |
| `lib/src/visitor/evaluate.dart`, `async_evaluate.dart` | `go/eval/evaluate*.go`                               | `src/eval/mod.rs`, `statement.rs`, `expression.rs`, `helpers.rs`, `meta.rs`, `init.rs` |
| `lib/src/import_cache.dart`                            | `go/eval/import_cache.go`                            | `src/eval/import_cache.rs`                                                             |
| `lib/src/visitor/evaluate.dart` (CSS)                  | `go/eval/evaluate_css.go`, `imported_css_visitor.go` | `src/eval/css.rs`, `imported_css.rs`                                                   |
| `lib/src/visitor/evaluate.dart` (meta)                 | `go/eval/evaluate_meta.go`                           | `src/eval/meta.rs`                                                                     |
