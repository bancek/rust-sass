# Architecture

This document describes how `rust-sass` is structured and why the design
decisions were made. It is organized from the compiler's core data structures
outward.

## 1. The pipeline

Compilation is a three-stage pipeline, each stage a distinct set of modules:

```
source text ──parse──▶ Stylesheet<'parse> ──eval──▶ ModifiableCssNode ──serialize──▶ CSS string
```

1. **Parse** (`parse/`) turns SCSS, indented Sass, or plain CSS into an AST
   (`Stylesheet<'parse>`) of statements and expressions.
2. **Eval** (`eval/`) walks that AST, resolving variables, imports, mixins,
   functions, and `@extend`, and produces a mutable CSS output tree
   (`ModifiableCssNode`).
3. **Serialize** (`serialize/`) walks the output tree and writes the final CSS
   string (and an optional source map).

Everything allocated during a single compilation — source text, AST nodes,
spans, and Sass values — lives in one [`bumpalo::Bump`](https://crates.io/crates/bumpalo)
arena owned by the caller. The arena is **not** reference-counted: `compile_*`
takes `&'compile Bump` and threads it through parse, eval, and serialize. This
makes every heap value that would otherwise be `Rc`-clone-heavy a simple
borrowed reference.

## 2. `Value`: an 11-variant arena enum

The Sass value type set is closed (sealed in Dart), so it is modeled as a Rust
enum, not a trait object:

```rust
enum ValueKind<'parse> {
    Boolean(SassBoolean),
    Null,
    String(SassString<'parse>),
    Number(SassNumber),
    Color(SassColor),
    List(SassList<'parse>),
    ArgumentList(SassArgumentList<'parse>),
    Map(SassMap<'parse>),
    Calculation(Box<SassCalculation>),
    Function(SassFunction<'parse>),
    Mixin(SassMixin<'parse>),
}

// The handle: an arena reference. Copy.
struct Value<'parse>(&'parse ValueInner<'parse>);
```

`Value` is `&'parse ValueInner` — a `Copy` arena reference, not a heap-owning
`Rc`. This is what removes the deep-clone tax that a GC would hide: cloning a
`Value` is a pointer copy. `SassMap` is an `IndexMap<Value, Value>`, so map keys
work by value equality with no allocation per lookup.

**Why an enum, not `dyn Value`:** the type set is finite and known at compile
time; an enum gives static dispatch, exhaustive `match`, `derive(Clone, Debug)`,
and zero allocations for return values. **Why arena:** Dart shares values by
reference (GC); the arena reproduces that sharing without a collector. The two
designs are documented in `ref/value.md`.

## 3. AST: three enums

| Enum                 | Variants | Dispatch                      |
| -------------------- | -------- | ----------------------------- |
| `Statement<'parse>`  | 27       | `match` → `StatementVisitor`  |
| `Expression<'parse>` | 18       | `match` → `ExpressionVisitor` |
| `Value<'parse>`      | 11       | `match` → `ValueVisitor`      |

No `dyn` for AST nodes. Recursive positions are boxed (`BinaryOperation`
operands, `InterpolationPart` expressions) and child lists live in `Vec`s;
the CSS AST is a separate pair of enums (see §9). Why enums over trait objects:
no vtable, no `Box<dyn>` for child vectors, and compiler-enforced exhaustiveness
when a new node type is added — the exact properties Dart's sealed classes give,
without a hierarchy.

## 4. Visitors: one `accept()` per enum

Each AST enum has a single generic `accept()` method whose return type is an
associated type on the visitor trait:

```rust
impl<'parse> Statement<'parse> {
    pub fn accept<V: StatementVisitor<'parse> + ?Sized>(
        &self, visitor: &mut V,
    ) -> SassResult<V::Output> {
        match self { Statement::Stylesheet(s) => visitor.visit_stylesheet(s), /* ... */ }
    }
}

trait StatementVisitor<'parse> {
    type Output;
    fn visit_stylesheet(&mut self, node: &Stylesheet<'parse>) -> SassResult<Self::Output>;
    // ... one method per Statement variant
}
```

`V::Output` selects the result type. Real examples: `RecursiveAstVisitor`
and `FindDependenciesVisitor` use `Output = ()`, `IsPlainCssVisitor` uses
`bool`, `ReplaceExpressionVisitor` uses `Expression<'parse>`, and the
`SerializeVisitor` impls use `()` (`serialize/value.rs`, `serialize/css.rs`,
`serialize/selector.rs`). **Why one method
with an associated type, rather than Go's `AcceptValue`/`AcceptBool`/`AcceptVoid`
explosion:** Rust expresses "same traversal, different result" through a single
generic method, so adding a visitor does not require adding a new `accept`
family. Note the evaluator itself implements **none** of these traits: it
dispatches exclusively through free functions (`evaluate_statement(config,
state, arena, stmt)`, `evaluate_expression(…)`, `evaluate_css_*(…)`), each a
`match` over the enum. `EvaluateVisitor` is just the `{config, state, arena}`
bundle passed (split) into those functions. Visitor traits
are object-safe but are never used as `dyn` —
dispatch is always a `match`. See `ref/visitors.md` for the
full trait index.

## 5. Lifetimes: `'parse` and `'compile`

Two lifetime parameters pervade the codebase.

- **`'parse`** is the lifetime of data allocated into the compile arena:
  `&'parse str` source slices, `Stylesheet<'parse>`, `FileSpan<'parse>`, and
  `Value<'parse> = &'parse ValueInner`. A `SassString.text` is either a slice of
  the source file or a temporary string built into the arena at eval time; the
  arena reclaims neither until the compile ends, so a `&'parse str` can point at
  a transient buffer without outliving it.
- **`'compile`** is the lifetime of the arena itself (`&'compile Bump`). The
  evaluator holds `arena: &'compile Bump`; the bound `'compile: 'parse` states
  that the arena outlives everything allocated into it.

Core evaluator types carry **both** lifetimes (`EvalConfig<'compile, 'parse>`,
`EvalState<'compile, 'parse>`, `Callable<'compile, 'parse>`), with the bound
`'compile: 'parse` — and, where `Callable` invariance leaks (its callback
mentions `&mut EvalState` in argument position, a contravariant use that
strips covariance), additionally `'parse: 'compile`. Most eval-level free
functions need only `'compile: 'parse` (e.g. `eval/calc.rs`); the double
bound appears where a `Callable` is stored or coerced (e.g. `eval/mod.rs`
`evaluate`, `callable.rs`). At the concrete call site inside
`compile_string`, both lifetimes unify to the same arena lifetime, satisfying
both bounds trivially.

**Why two distinct lifetimes instead of one:** a single `'parse` would force the
arena reference and the arena-allocated AST data to share one lifetime, which
fails when the data outlives the evaluator (in the compile pipeline the
evaluator is dropped before the arena). The split also existed to replace
`arena: Rc<Bump>`: `Rc` deref ties a borrow to the _access path_ (`config` is
passed by reference), never to the owner's lifetime, so `Rc<Bump>` could never
yield `&'parse Bump`. A plain `&'compile Bump` does.

The `'compile: 'parse` bound appears **only at construction sites** — the
serialize visitors never construct a value and therefore take no arena
(value _constructors_ such as `Value::new_with_arena` and
`BuiltInCallable::function(..., arena)` do take one).

**`Value` is invariant in `'parse`** for the same reason (`SassFunction`/
`SassMixin` carry a `Callable`). The value API consequently decouples the
reference lifetime from the content lifetime: assertion functions are
`fn assert_*<'v, 'parse>(v: &'v Value<'parse>)`, never
`fn f(v: &'parse Value<'parse>)`.

The port is **`unsafe`-free**: the sole pre-existing `unsafe` (a `Box::leak` in
`parse_parameter_list`) was removed in favor of passing the arena through, and
identity — needed for address-based comparisons — is `std::ptr::eq`
(not raw-pointer dereference).

## 6. Sync/async duality

The crate compiles in two modes from **one source tree**, selected by the single
`async` cargo feature (`rust-sass/async → rust-sass-macros/async`):

- **sync** (default): `async fn` becomes a plain `fn`, `.await` is stripped,
  `LocalBoxFuture<'a, T>` return types are rewritten to `T`. There are **no
  futures at all** in this build.
- **async**: the code is unchanged; callbacks return futures that hosts can
  drive.

The `rust-sass-macros` attributes (`maybe_async`, `sync_impl`, `async_impl`,
`maybe_test`, plus the `maybe_block_on!`/`box_rec!`/`box_rec_in!` helpers)
perform this transformation. See `ref/macros.md` for the mechanism.

**Why two builds, not one:** the sync build must be genuinely zero-future —
driving an async pipeline with `block_on` measures ~30% slower, and a wasm sync
entry point cannot block at all. Rust has no "same traversal, sync or async"
primitive, and sync/async `Value` are structurally different types (async
`Value` carries async callables), so a single unified build is impossible. The
value/ast/parse/serialize/callable/module cluster is one strongly connected
component that cannot be split or made generic over the callable. The ~30%
async tax is an **accepted architectural floor**: it is paid only by the wasm
build; native artifacts compile the sync evaluator and have no futures. The only
full fix would be an explicit-stack (worklist) evaluator rewrite, which was
rejected for maintainability and dart-sass back-porting.

## 7. Evaluator: `EvalConfig` + `EvalState`

The evaluator is split into an immutable config and a mutable state, with all
logic in **free functions** taking separate references:

```rust
pub struct EvalConfig<'compile, 'parse> {
    pub built_in_modules: IndexMap<String, Module<'compile, 'parse>>,
    // Shared mutable via arena &'parse RefCell — closures capture the Copy ref:
    pub built_in_functions: &'parse RefCell<IndexMap<String, Callable<'compile, 'parse>>>,
    pub global_functions: &'parse RefCell<Vec<Callable<'compile, 'parse>>>,
    pub logger: Rc<dyn Logger>,
    pub compile_context: CompileContext,
    pub io: Rc<dyn Io>,
    // ... plus quiet_deps, source_map, max_recursion_depth, node_importer,
    // unicode, alert_color, alert_ascii (see eval/mod.rs)
}

pub struct EvalState<'compile, 'parse> {
    pub env: Environment<'compile, 'parse>,
    pub stack: Option<Box<StackFrame<'compile, 'parse>>>,
    pub root: Option<ModifiableCssNode<'parse>>,
    pub import_cache: Option<ImportCache<'compile, 'parse>>,
    // ... plus member, import/callable/default_warn spans, module tables,
    // media-query + style-rule context, declaration flags, loaded_urls, etc.
    // (see eval/mod.rs)
}
```

All evaluation logic lives in `fn(config: &EvalConfig, state: &mut EvalState,
arena: &'compile Bump, …)` free functions. There are no visitor-trait impls on
`EvaluateVisitor` — call sites split-borrow `&v.config` (shared) +
`&mut v.state` (mutable) directly, e.g.
`statement::evaluate_stylesheet(&v.config, &mut v.state, arena, …)`:

**Why the split:** a single `&mut self` visitor cannot express the deep closure
nesting the eval code inherits from Dart (e.g. `run_user_defined_callable` has
five nested closure levels, each needing evaluator state). Splitting config and
state into separate references lets nested closures reborrow `state` through the
chain without conflict. `ref/eval.md` documents the 19 borrow patterns this
enables.

## 8. Serializer: `SerializeState` + `for_node`

The serializer follows the same split-borrow pattern:

```rust
#[derive(Clone, Copy)]
pub struct SerializeState { indentation, style, inspect, quote, line_feed, indent_char, indent_width }

pub struct SerializeVisitor<'parse> {
    pub buffer: SourceMapBuffer<'parse>,
    pub inner: SerializeState,
}
```

All visitor logic lives in free functions `(buf: &mut SourceMapBuffer, state:
&mut SerializeState, ...)`. Inside `for_node` callbacks only the buffer and
state are available, so dispatch is done with manual `match` helpers
(`visit_value_impl`, `visit_css_node_impl`, `visit_simple_selector_impl`)
rather than `node.accept(self)` — this **two-level dispatch** (free `_impl`
functions + thin trait wrappers + manual match where `accept()` is unavailable)
is what makes the struct split viable. See `ref/serialize.md`.

## 9. CSS output tree: `ModifiableCssNode` inner indirection

The CSS tree produced by evaluation is mutable and supports parent↔child
navigation without `unsafe`:

```rust
pub struct ModifiableCssNode<'parse> {
    inner: Rc<RefCell<ModifiableCssNodeInner<'parse>>>,
}
struct ModifiableCssNodeInner<'parse> {
    kind: ModifiableCssNodeKind<'parse>,       // 9-variant enum
    parent: Weak<RefCell<ModifiableCssNodeInner<'parse>>>,
    index_in_parent: usize,
    is_group_end: bool,
}
```

Parents own children via `Vec<ModifiableCssNode>` (each an `Rc` clone); children
hold a `Weak` back-pointer, so there is no reference cycle. `ModifiableCssNodeKind`
is unaware of the parent chain; all parent logic lives on the outer struct.

**Why indirection:** the parent↔child cycle cannot use plain borrowing (parents
move on `Vec` reallocation). Alternatives — raw pointers (rejected: `unsafe`),
`Rc<RefCell<>>` on each of 9 structs (too invasive), a HashMap side-table
(API mismatch) — are documented in `ref/ast.md`. The chosen design is one
wrapper with one `RefCell` for all mutable parent-chain state.

`is_group_end` is set by the evaluator on the last child of a root-level group,
read by the serializer to emit an extra blank line between groups, and
transferred at freeze via `to_css_node()`.

**Why a typed `ModifiableCssNode` (not an interface):** Dart uses a covariant
`parent` return so parent-chain walks stay in modifiable-land — its evaluator
needs exactly one downcast; Go's interface-based port needs 15+. Rust eliminates
them entirely by making the mutable node a concrete enum type. At the end of
evaluation, `to_css_node()` performs a one-time deep conversion to the frozen
`CssNode` enum that the serializer walks.

## 10. Module system and environment

The Sass module system is an enum with identity-by-allocation:

```rust
pub enum ModuleKind<'compile, 'parse> { BuiltIn(..), Forwarded(..), Shadowed(..), Environment(Box<EnvironmentModule>) }
// The handle: arena-allocated, Copy. Equality/hash are address identity:
pub struct Module<'compile, 'parse>(&'parse ModuleKind<'compile, 'parse>);
```

`Module` and `Callable` compare by address (`std::ptr::eq` /
`Callable::identity_eq`) — Dart and Go have no `ModuleID`/`CallableID`
counter, so identity is the allocation itself.
`Environment` keeps parallel scope stacks as `Vec<&'parse RefCell<IndexMap>>`
(arena refs, `Copy`), which
gives `!global` shared-frame semantics: `closure()` shares frame 0 via the
arena refs so a `!global` write is visible to the caller. See `ref/environment.md`.

Modules store the **modifiable** CSS tree, not a frozen one: there is no
shared borrow accessor (the variants need different lifetimes/arena access),
so CSS leaves a module via `clone_css(arena)`, and freezing to `CssNode`
happens only at the serializer boundary (`to_css_node(is_group_end)`). The tree is never
freeze→clone→freeze'd internally.

## 11. Error system

`SassError` is a 6-variant enum (`Script`, `Runtime`, `Format`, `Sass`,
`MultiSpan`, `MultiSpanScript` — the last carries no primary span yet; the
eval call site attaches it via `with_member_use_span`). Internal errors are
zero-copy (they borrow `FileSpan<'parse>`);
at the public boundary `SourceSpanWithContext::from_span()` copies the source
text, so `'parse` never leaks into a public error type. The parser layers its
own errors — `ScanError → SpanScannerError → ParseError{Sass|Format|MultiSpan|Scan} → SassError` — so
that "expected" zero-length spans can be adjusted to point at the preceding
newline before conversion. See `ref/common.md` and `ref/parse.md`.

## 12. I/O and importers

`Io` is the host-abstraction seam: an async trait (`read_file`, `file_exists`,
`canonicalize`, …) with a **sync** `current_dir()` (the only `Io` call in the
error-formatting path; keeping it sync keeps the whole error path sync).
`IoError` is `{ message, kind, path }` with `IoErrorKind { NotFound, Permission,
AlreadyExists, Other }` — no `std::io`, so it is wasm-safe.

Four threading principles govern I/O: use `Io::current_dir()` (never
`std::env::current_dir()`); build `file:` URLs via `from_file_path` (never string
formatting); thread `Io` through `EvalConfig` (not per-function); and `Frame`
does not store `Io` — `Frame::location(io)` (and `Trace::format(io)`) receives
it from the caller.
`UserImporter` is the extension point a JS/wasm host implements.
`NodePackageImporter` is stored as `Option<&'parse NodePackageImporter>` (a
presence flag — the real importer lives in `ImporterKind::NodePackage`). See
`ref/io.md`.

## 13. Embedded, wasm, and libsass

Three consumer-facing surfaces are built on the compiler:

- **`rust-sass-embedded`** serves the Sass embedded protocol (version 3.2.0,
  compiler version 1.104.0, implementation name `"dart-sass"`) over stdio. See
  `ref/embedded.md`.
- **`rust-sass-wasm`** exposes the modern JS API via wasm-bindgen and
  serde-wasm-bindgen, from the same source tree as two artifacts (a zero-future
  sync build and an async build). See `ref/wasm.md`.
- **`rust-sass-libsass`** builds a drop-in `libsass.{so,dylib,a}` plus the
  pinned `sass/*.h` headers from the sync API, so existing native consumers
  (sassc, node-sass, language bindings) link unmodified. The only `unsafe`/
  C-ABI crate in the workspace. See `ref/libsass.md`.

## 14. Workspace layout

| Crate                                                        | Purpose                                                              |
| ------------------------------------------------------------ | -------------------------------------------------------------------- |
| [`rust-sass`](../rust-sass/)                                 | The compiler library (parse → eval → serialize).                     |
| [`rust-sass-cli`](../rust-sass-cli/)                         | The `rust-sass` command-line binary.                                 |
| [`rust-sass-embedded`](../rust-sass-embedded/)               | The `sass-embedded` protocol server (spawned with `--embedded`).     |
| [`rust-sass-wasm`](../rust-sass-wasm/)                       | The modern JS Sass API compiled to WebAssembly.                      |
| [`rust-sass-libsass`](../rust-sass-libsass/)                 | The libsass C API on `rust-sass` (drop-in `libsass.so`/`.a`).        |
| [`rust-sass-embedded-pb`](../rust-sass-embedded-pb/)         | Prost bindings for the embedded protocol (committed generated code). |
| [`rust-sass-embedded-pb-gen`](../rust-sass-embedded-pb-gen/) | Build tool that regenerates the proto bindings.                      |
| [`rust-sass-macros`](../rust-sass-macros/)                   | The `maybe_async` proc-macro behind the sync/async dual build (§6).  |
| [`rust-sass-spec`](../rust-sass-spec/)                       | The runner for the official `sass-spec` test suite.                  |
| [`rust-sass-libsass-tests`](../rust-sass-libsass-tests/)     | C-ABI contract tests plus the ecosystem binding gate.                |

Only the first five are public surfaces; the rest is build tooling and test
harness (`-embedded-pb` is committed generated code, regenerated by
`-pb-gen`).
