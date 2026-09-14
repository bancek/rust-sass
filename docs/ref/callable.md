# Module: `callable.rs`

The runtime representation of Sass functions and mixins: the `Callable` type,
its three variants, overload resolution, and the invocation callback.

## `Callable`

```rust
enum CallableKind<'compile, 'parse> {
    UserDefined(UserDefinedCallable<'compile, 'parse>),
    BuiltIn(BuiltInCallable<'compile, 'parse>),
    PlainCss(PlainCssCallable),
}

struct Callable<'compile, 'parse>(&'parse CallableKind<'compile, 'parse>);
```

Identity is address equality (`std::ptr::eq` / `Callable::identity_eq`,
`identity_hash` = address of the `CallableKind` allocation — cf. Go
`reflect.ValueOf(ref).Pointer()`). The handle is arena-allocated and `Copy`.
Callables are stored in string-keyed maps (by name)
in the environment and the built-in registry, never as map keys themselves.
Identity is compared exactly once: `assert_no_conflicts()` during `@forward`
deduplication.

## The three variants

### `BuiltInCallable`

```rust
struct BuiltInCallable<'compile, 'parse> { name: String, overloads: Rc<Vec<BuiltInOverload<'compile, 'parse>>>, accepts_content: bool, deprecation_warning: Option<(String, String)> }
struct BuiltInOverload<'compile, 'parse> { params: ParameterList<'parse>, callback: BuiltInCallback<'compile, 'parse> }
```

The callback is a `Sync | Async` enum:

```rust
pub type SyncBuiltInCallback<'c, 'p> = Rc<dyn Fn(
    &EvalConfig<'c, 'p>, &mut EvalState<'c, 'p>, Vec<Value<'p>>, &'c Bump,
) -> SassResult<Value<'p>> + 'p>;

pub type AsyncBuiltInCallback<'c, 'p> = Rc<dyn for<'a> Fn(
    &'a EvalConfig<'c, 'p>, &'a mut EvalState<'c, 'p>, Vec<Value<'p>>, &'a Bump,
) -> LocalBoxFuture<'a, SassResult<Value<'p>>> + 'p>;

pub enum BuiltInCallback<'c, 'p> { Sync(SyncBuiltInCallback<'c, 'p>), Async(AsyncBuiltInCallback<'c, 'p>) }
```

The callback carries the full evaluator (`&EvalConfig` + `&mut EvalState`), not
just `EvalContext`, so the `meta.call`/`meta.apply`/`meta.load-css` built-ins can
re-enter the evaluator (`call → invoke_callable`, `apply → apply_mixin`,
`load-css → ImportCache::canonicalize`). This makes `callable` and `eval`
mutually `use` each other — legal because the cycle is only references
(`&EvalConfig`/`&mut EvalState`), not owned types, so no recursive type
definition is created. The `Sync`/`Async` split exists so that ~380 pure
built-ins never box a future: the invocation sites match the enum **inline**, so
a `Sync` callback runs with zero async overhead while an `Async` callback is
awaited. In the sync build the enum collapses to the `Sync` alias
(`#[sync_impl] pub type BuiltInCallback = SyncBuiltInCallback`). Callback
values are arena-allocated `Value<'parse>` per compilation (built in the
`&'compile Bump` threaded through the callback).

### `UserDefinedCallable`

```rust
struct UserDefinedCallable<'compile, 'parse> {
    declaration: CallableDeclaration<'parse>,   // Mixin(MixinRule) | Function(FunctionRule) | ContentBlock(ContentBlock)
    environment: Environment<'compile, 'parse>,
    in_dependency: bool,
}
```

`name()` and `parameters()` are derived from the `declaration` (Dart-exact; Go's
flattened `name`/`arguments`/`isMixin` copies were not ported). There is no
`is_mixin` field — call sites match the declaration variant. `has_content()`
(lazy) lives on `MixinRule` only.

### `PlainCssCallable`

A bare `{ name: String }` constructed on the fly when no user-defined or
built-in function matches, and serialized as plain CSS `name(args...)`. It is
never stored persistently.

## Overload resolution and argument flow

`CallbackFor(positional, named)` resolves overloads:

1. **Exact match** — the first overload whose `ParameterList` matches the
   positional count and named keys wins.
2. **Fuzzy fallback** — pick the overload with the smallest
   `|param_count − positional|`, preferring more parameters on ties.
3. **No match** — error.

The full argument flow is five steps: parse the `ArgumentList` → evaluate
positional and named expressions → resolve the overload → verify the parameters
→ resolve defaults and pack the rest argument into an `SassArgumentList`.

## Lookup priority

When the evaluator resolves a function or mixin name:

1. Environment scope chain → `UserDefinedCallable`.
2. Built-in registry (`builtInFunctions[normalized_name]`) → `BuiltInCallable`.
3. Plain-CSS context → a fresh `PlainCssCallable`.

## File mapping

| Dart                                       | Go                                                          | Rust              |
| ------------------------------------------ | ----------------------------------------------------------- | ----------------- |
| `lib/src/callable.dart`, `callable/*.dart` | `go/sasscallable/callable.go`, `go/functions/callable_*.go` | `src/callable.rs` |
