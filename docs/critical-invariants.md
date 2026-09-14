# Critical invariants

Rules that must not be violated anywhere in the port. Each rule states what it
is and why it exists.

## FMA prevention

Every `a*b + c` in color-conversion matrices must round each multiply
explicitly:

```rust
// CORRECT — each product rounds before the addition
(m[0] * v0) as f64 + (m[1] * v1) as f64 + (m[2] * v2) as f64

// WRONG — LLVM may fuse into one FMA instruction and diverge by 1 ULP
m[0] * v0 + m[1] * v1 + m[2] * v2
```

**Why:** when compiled with `-Ctarget-cpu=native`, LLVM fuses multiply-add into
a single FMA that rounds once instead of twice, diverging from Dart/Go (and the
spec goldens) at the last ULP on ARM64 vs x86-64. The Go port uses `float64()`
casts for the same reason. `ref/math.md` documents the full floating-point
parity model, including the `glibc-math` feature that handles wasm's lack of a
libm.

## Nil-slice vs empty-slice

Go distinguishes `nil []Statement` (no block, ends with `;`) from
`[]Statement{}` (an empty `{}` block). Rust must too, via
`Option<Vec<Statement>>`: `None` = no block, `Some(vec![])` = empty block. This
affects `Declaration`, `AtRule`, `StyleRule`, `MediaRule`, `SupportsRule`, and
`IncludeRule`'s content argument.

## No silent error discarding

All nine visitor traits return `SassResult`. No visitor method is infallible.
The only operation that may `.unwrap()` is writing to a `String` buffer
(`write!(buf, ...).unwrap()`), because `impl fmt::Write for String` never fails.
`to_css_string()` and `to_display_string()` return `SassResult<String>` — in CSS
mode (`inspect = false`) maps, functions, mixins, and empty unbracketed lists
error via `SassError::Script`.

## `'parse` never appears in public types

Internal errors are zero-copy (`FileSpan<'parse>`). At the public API boundary
`SourceSpanWithContext::from_span()` copies the source text, so `'parse` never
leaks into a public error type or `CompileResult`.

## `'compile: 'parse` lifetime split

Core evaluator types (`EvalConfig`, `EvalState`, `EvaluateVisitor`,
`StackFrame`, `Callable`) carry both `'compile` and `'parse`, with the bound
`'compile: 'parse` (the arena outlives its allocations); add `'parse:
'compile` only where `Callable`'s invariance leaks (storing or coercing a
`Callable` — most free functions need just `'compile: 'parse`). The
`'compile: 'parse` bound appears **only at construction sites**
— the serialize visitors never construct a value, so they take no arena
(value constructors do).

**Why two lifetimes, not one:** a single `'parse` would force the arena
reference and the AST data to share one lifetime, which fails when arena
allocations outlive the evaluator (the compile pipeline drops the evaluator
before the arena).

## No `unsafe`

`rust-sass`, `rust-sass-cli`, `rust-sass-embedded`, and `rust-sass-wasm` contain
no `unsafe` code. The sole pre-existing `unsafe` (a `Box::leak` in
`parse_parameter_list`) was removed by threading the arena through instead.
Identity is address-based (`std::ptr::eq`) and is correct only
because values are constructed once per identity.

## Rc, not Arc

The compiler is single-threaded (`!Send`), so shared ownership uses `Rc`,
never `Arc`. Environment frames are arena refs (`Vec<&'parse
RefCell<IndexMap>>`, `Copy`); `Rc` remains only where shared ownership is
genuine: the CSS parent chain (`Rc<RefCell<…>>` + `Weak` back-pointers),
callable/overload sharing (`Rc<Vec<BuiltInOverload>>`,
`SyncBuiltInCallback = Rc<dyn Fn…>`), extend-store boxes
(`Rc<RefCell>` `MutableBox`/`StoreBox`), and the host-service seams (`Logger`,
`Io`, `PackageConfig`, `UserImporter`, plus `HostContext`/`CompileContext =
Rc<()>` for the `'static` embedded boundary that cannot be arena-ref'd).
Never reach for `RefCell`/`Rc` as a borrow workaround in visitor
implementations — split into free functions taking separate references
instead (see "Free functions for borrow ergonomics" below).

**No-reentry contract.** The whole codebase is `!Send`/`!Sync`, and an `Io`
implementation must never call back into Rust while processing an I/O request.
Given that, a `RefCell` guard held across an `.await` can never conflict — while
the future is suspended, nothing else can touch the same `RefCell`. Do not add
defensive guard-dropping to "protect" against re-entry; it is unsound by
contract, not by guard discipline.

## The accept pattern

All AST enums expose a single generic `accept()` with an associated `Output`
type. No Go-style `AcceptValue`/`AcceptBool`/`AcceptVoid` explosion.

## Module is an enum (+ arena handle)

`ModuleKind` is `enum ModuleKind { BuiltIn, Forwarded, Shadowed,
Environment(Box<EnvironmentModule>) }`, not a trait object — zero vtable;
`Module` is a `Copy` arena handle (`&'parse ModuleKind`) with identity via
`std::ptr::eq`.

## `Value` invariance

`Value<'parse>` is **invariant** in `'parse` (because `SassFunction`/`SassMixin`
carry a `Callable`, whose callback mentions `'parse` in argument position). Do
not write `fn f(v: &'parse Value<'parse>)`; use two lifetime parameters
(`fn assert_foo<'v, 'parse>(v: &'v Value<'parse>)`) so the reference lifetime
is decoupled from the content lifetime.

## Stack safety

Evaluation is recursive. The `max_recursion_depth: 250` field is **not** a live
limit — the real bound is the machine stack. Errors are boxed
(`SassResult<T> = Result<T, Box<SassError>>`): the `Err` variant is 8 bytes,
so every fallible return slot stays small even on the `Ok` path, and the
allocation happens only on the (cold) error path. Boxed async futures move the
recursion state machine to the heap, so only active poll frames nest on the
stack; deep nesting passes in release builds and is more stack-safe than the
equivalent sync recursion. A synchronous `.await` still polls the child inline
(Rust has no tail-call elimination), so unbounded recursion eventually
overflows the machine stack regardless.

## Free functions for borrow ergonomics

When `&mut self` methods create borrow conflicts with nested closures, split the
visitor into a state struct and a thin visitor struct, and convert all logic to
**free functions** taking separate references (serializer trait impls become
one-line wrappers; the evaluator has no trait impls at all — only free
functions). Do **not** use `RefCell`/`Cell` as a borrow workaround in visitor
implementations.

The distinction is deliberate: `RefCell` is correct for _ownership_ semantics
(arena `&'parse RefCell<IndexMap>` environment frames,
`Rc<RefCell<ModifiableCssNodeInner>>`
for the CSS parent chain — these model GC-shared mutable references); free
functions are correct for _borrow ergonomics_ (temporary conflicts in
single-owner visitors). This rule covers the serializer (`SerializeState` +
`for_node`) and the evaluator (`EvalConfig` + `EvalState`).

## Byte-identity, spec-green, protocol alignment

- Output is **byte-identical** to Dart Sass on the bootstrap,
  `huge`, and `huge10` workloads — CSS, source maps, warnings, and errors.
- The `sass-spec` suite must stay green: 14263/14263.
- `compilerVersion`/`protocolVersion` must track the pinned dart-sass version
  (`1.104.0` / `3.2.0`) in lockstep — see `CONTRIBUTING.md` version-bump
  checklist.

## Test standards

- **Exact error assertions.** Never assert `is_err()`/`is_ok()` without checking
  the exact message or variant.
- **Golden values.** Hardcode captured output; never recompute from the SUT
  formula (see `patterns.md` §8).
- **Coverage.** Implementation and test line counts should be comparable.
