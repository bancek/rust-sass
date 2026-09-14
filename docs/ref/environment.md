# Modules: `environment/`, `module/`, `configuration/`

The scoping and module system: `Environment` (variable/function/mixin scopes),
`Module` (the compiled result of a stylesheet), `Configuration` (`@use ... with`
values). Warning-span resolution (`import_span`/`callable_span`/
`default_warn_span`) lives directly on `EvalState` (see `ref/eval.md`) —
there is no `EvalContext` struct.

## Environment

```rust
struct Environment<'compile, 'parse>(&'parse RefCell<EnvironmentInner<'compile, 'parse>>);
// EnvironmentInner<'compile, 'parse> {
    variables: Vec<&'parse RefCell<IndexMap<String, Value<'parse>>>>,
    variable_nodes: Vec<&'parse RefCell<IndexMap<String, FileSpan<'parse>>>>,
    functions: Vec<&'parse RefCell<IndexMap<String, Callable<'compile, 'parse>>>>,
    mixins: Vec<&'parse RefCell<IndexMap<String, Callable<'compile, 'parse>>>>,
    modules: &'parse RefCell<IndexMap<String, Module<'compile, 'parse>>>,
    // ... namespace_nodes, global/imported modules, forwarded (Option<&RefCell>),
    // nested-forwarded, all_modules (&RefCell), index caches, last_variable_*,
    // content, configurable_variables (&RefCell), in_mixin, in_semi_global_scope
// }
```

No arena is stored (the `&'parse` refs already tie frames to it — an `Rc<Bump>`
could never yield `&'parse Bump`; see `architecture.md` §5).

- **`closure()`** shallow-copies the frame `Vec`s (the arena refs are `Copy`),
  so parent and closure share
  frame 0 — a `!global` write to frame 0 is visible to the caller (and resets
  the index caches). The module maps (`modules`/`namespace_nodes`/
  `global_modules`/`imported_modules`/`forwarded_modules`/`nested_forwarded_modules`/
  `all_modules`) are shared by reference (arena `&RefCell`, like Dart's shared
  `Map` objects); the configurable set is detached (fresh empty — closures are
  always in nested contexts where configurables are never added); and
  `in_mixin` resets to `false`. **`for_import()`** shares the scope chains,
  `_importedModules`/`_nestedForwardedModules`, and the configurable set, but
  isolates the remaining module maps (fresh `modules`/`namespace_nodes`/
  `global_modules`/`all_modules`, `forwarded_modules = None`); `in_mixin`
  resets to `false`.
- **`AstNode → FileSpan`:** every stored `AstNode` (`variable_nodes`,
  `callable_span`, `namespace_nodes`, the module maps) was only ever `.span()`-ed,
  so the fields store `FileSpan` (Copy) directly.
- **`scope()`** saves/restores `in_semi_global_scope`, optionally pushes/pops a
  frame, and invalidates the `last_variable_name` index cache on exit.
- **`content`** holds the `@content` block (`with_content`/`as_mixin` set and
  restore it around mixin application).

## Module

```rust
enum ModuleKind<'compile, 'parse> { BuiltIn, Forwarded, Shadowed, Environment(Box<EnvironmentModule>) }
// Copy handle with address identity:
struct Module<'compile, 'parse>(&'parse ModuleKind<'compile, 'parse>);
```

Identity is `std::ptr::eq`. **Why no counter:** Dart and Go have no `ModuleID`/
`CallableID`; identity is the allocation. `Module` and `Callable` are immutable
value objects, so they do **not** need `RefCell` (unlike `Environment` frames
and `Configuration` inners, which use arena `&RefCell` for shared mutation).

- `ForwardedModuleView` / `ShadowedModuleView` hold an inner module and cache
  precomputed, prefix/show/hide-filtered maps; `variable_identity(name)`
  delegates through the view to the originating inner module.
  `ShadowedModuleView` filters `variable_nodes` too (namespaced node lookup),
  queries its own (filtered) keys in `could_have_been_configured` (the
  length+containment check stands in for Dart's map-identity comparison),
  and counts CSS in `is_empty` (a memberless-but-CSS-carrying view is kept,
  not dropped — otherwise the environment shadowing path loses CSS).
- `EnvironmentModule` (`to_module()`/`to_dummy_module()`) merges
  `environment.variables[0]` with forwarded members, and maps each variable to
  its originating module via `modules_by_variable`. `forwarded` travels as an
  order-preserving `&[Module]` slice (Dart iterates its `LinkedHashSet` in
  insertion order — a `HashSet` would nondeterministically reorder
  `meta.module-variables`); nested forwarded modules are flattened like
  Dart's `_makeModulesByVariable`.

## Configuration

```rust
enum Configuration<'parse> {
    Implicit { inner: &'parse RefCell<ConfigurationInner<'parse>>, filters: Vec<Filter> },
    Explicit { inner: &'parse RefCell<ConfigurationInner<'parse>>, filters: Vec<Filter>, node_span: FileSpan<'parse> },
}
enum Filter { Prefix(String), Safelist(HashSet<String>), Blocklist(HashSet<String>) }
struct ConfiguredValue<'parse> { value: Value<'parse>, configuration_span: Option<FileSpan>, assignment_span: FileSpan }
```

`SameOriginal` is address equality on the shared inner — `through_forward`
clones the arena ref, so all configs derived from one base share an identity,
while two independently created configs do not. Go/Dart express the
explicit/implicit split as a subtype; Rust flattens it to an enum (safe —
the `node_span` presence marks explicitness, no subtype assertions exist).

## Warning-span resolution (no `EvalContext` struct)

There is no `EvalContext`/`evalcontext.rs` — the fields live directly on
`EvalState`:

```rust
// EvalState<'compile, 'parse> {
    import_span: Option<FileSpan<'parse>>,   // span of the @import being evaluated
    callable_span: Option<FileSpan<'parse>>, // span of the executing callable
    default_warn_span: FileSpan<'parse>,     // fallback (set by with_evaluation_context)
// }
```

`warn_span()` falls back through `import_span → callable_span →
default_warn_span`, matching Dart's `_EvaluationContext` chain. Deprecation
warnings route through the evaluator's dedup/quiet-deps/stack-trace handling
(see `eval/warn.rs`); otherwise they fall back to `Logger::warn_deprecation`.

## `from_one_module`

Two free functions (not one generic) because the identity type differs:

- `from_one_module_variable` — identity is `module.variable_identity(name)`
  (a `Module`, address equality).
- `from_one_module_callable` — identity is the `Callable` itself (address equality).

Two functions avoid runtime type checks and generics on identity: the
alternatives (a single generic with `any` identity needing `dyn Any`, an
`OpaqueId` raw-pointer burden, or an `Identity` enum the caller must construct)
are all rejected. Both search `nestedForwardedModules → importedModules → globalModules`, with
identity-based conflict detection only in the `globalModules` phase (a differing
identity produces a `MultiSpan` "available from multiple global modules" error).

## `importForwards`

At root, `importForwards` shadows conflicting members and copies forwarded
modules into `importedModules`/`forwardedModules`; when nested, it appends to
`nestedForwardedModules`; in both cases it removes now-shadowed locals.

## Frame-scoped readers (C-API seam)

Two additive readers exist for host seams that need libsass-style
frame-addressed access (seeded by `rust-sass-libsass` step 8; both
semantics-preserving, both `&self`):

- `get_local_variable(name)` — current frame only (`variables.last()`).
  Unlike `get_variable`, outer scopes and modules are never consulted, and
  unlike libsass `Env::get_local` a miss inserts nothing (reads never
  mutate the environment).
- `get_global_variable(name)` — frame 0, then the global-module fallback
  (same lookup shape as `global_variable_exists`, returning the value).
- Writes need no new API: `set_variable(..., global=false)` already mirrors
  libsass `set_lexical` (found frame, else current; never frame 0 outside
  the root), `set_local_variable` is the current-frame write, and
  `set_variable(..., global=true)` is `set_global`. C-origin values use
  `BOGUS_SPAN` for their declaration span.

## File mapping

| Dart                                                  | Go                                     | Rust                                                     |
| ----------------------------------------------------- | -------------------------------------- | -------------------------------------------------------- |
| `lib/src/environment.dart`                            | `go/sassenv/environment.go`            | `src/environment/mod.rs`                                 |
| `lib/src/module.dart`, `module/*.dart`                | `go/sassmodule/*.go`                   | `src/module/*.rs`                                        |
| `lib/src/configuration.dart`, `configured_value.dart` | `go/configuration/*.go`                | `src/configuration.rs`                                   |
| `lib/src/evaluation_context.dart`                     | `go/evalcontext/evaluation_context.go` | (no counterpart — fields live on `EvalState`; see above) |
