# Rust ↔ Dart review guide

Detailed per-file review of the Rust port against Dart Sass. One file,
Rust ↔ Dart only — Go is ignored (it is a separate frozen port, not a
porting path; see `porting.md`).

## 1. Goal and non-goals

**Goal:** find porting bugs where Rust diverges from Dart in observable
behavior: CSS output, source maps, error messages, spans, traces,
warnings, import resolution, and evaluation order/scope effects.

**Non-goals:**

- No literal structural parity. Rust's type system forces more
  restructuring than Go needed (see §3). Review _boundary behavior_,
  not statement counts.
- No Go cross-check. Read Dart in `dart-sass/` directly (`porting.md`).
  `// go-source:` annotations are provenance only.
- No style review. If behavior matches, structure is fine even when it
  looks nothing like Dart.

**Scope:** the `rust-sass` library crate vs `dart-sass/lib/src`. Other
crates map elsewhere: `rust-sass-embedded*` ↔ `lib/src/embedded/*` +
`build/language/spec/embedded_sass.proto`, `rust-sass-cli` ↔
`lib/src/executable/*`, `rust-sass-wasm` ↔ `lib/src/js/*`. Review those
only when the finding lives at the seam (e.g. error → proto mapping).

Pin under review: `PORTED_FROM` (`e01e268c6f6826ae309bf3105765d4c93024ebbc`,
v1.104.0 per `upstream.md`) must agree with the `dart-sass/` submodule
pin. Record both hashes in every finding log (§7).

## 2. File map (Dart → Rust)

Built from `// dart-source:` in `rust-sass/src`. Read the Dart file in
`dart-sass/lib/src/...`, find Rust via:

```sh
rg -l "dart-source:.*$(basename <dart-file>)" --glob '*.rs' rust-sass/src
```

Groups in risk-first review order (§5). The evaluator group is first
because most porting bugs live there.

| #   | Dart                                                                                                                                                                                                                                                                                    | Rust                                                                                                                                                                                            |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| E1  | `lib/src/visitor/evaluate.dart` (EvaluateVisitor, ~all visit methods)                                                                                                                                                                                                                   | `rust-sass/src/eval/{mod,statement,expression,css,calc,helpers,meta,compat,init,warn,result}.rs` + `rust-sass/src/compile_context.rs`                                                           |
| E2  | `lib/src/visitor/async_evaluate.dart`                                                                                                                                                                                                                                                   | `rust-sass/src/eval/result.rs`                                                                                                                                                                  |
| E3  | `lib/src/environment.dart`, `lib/src/async_environment.dart`                                                                                                                                                                                                                            | `rust-sass/src/environment/mod.rs` + `rust-sass/src/module/environment_module.rs`                                                                                                               |
| E4  | `lib/src/importer.dart`, `lib/src/importer/async.dart`, `lib/src/importer/*.dart`, `lib/src/import_cache.dart`, `lib/src/async_import_cache.dart`                                                                                                                                       | `rust-sass/src/eval/importer/{mod,filesystem,package,node_package,resolve_import_path,result,no_op,utils}.rs` + `rust-sass/src/eval/import_cache.rs`                                            |
| E5  | `lib/src/functions/*.dart`, `lib/src/functions.dart`                                                                                                                                                                                                                                    | `rust-sass/src/functions/*.rs`                                                                                                                                                                  |
| E6  | `lib/src/compile.dart`, `lib/src/async_compile.dart`, `lib/src/compile_result.dart`                                                                                                                                                                                                     | `rust-sass/src/compile/{mod,options,result}.rs`                                                                                                                                                 |
| E7  | `lib/src/callable*.dart`, `lib/src/callable/*.dart`, `lib/src/evaluation_context.dart`, `lib/src/configuration.dart`, `lib/src/configured_value.dart`, `lib/src/module*.dart`, `lib/src/module/*.dart`                                                                                  | `rust-sass/src/callable.rs`, `rust-sass/src/compile_context.rs`, `rust-sass/src/configuration.rs`, `rust-sass/src/module/*.rs`                                                                  |
| A1  | `lib/src/ast/sass/*.dart`, `lib/src/ast/sass/**/*.dart`                                                                                                                                                                                                                                 | `rust-sass/src/ast/sass/**/*.rs`                                                                                                                                                                |
| A2  | `lib/src/ast/css/*.dart`, `lib/src/ast/css/modifiable/*.dart`, `lib/src/visitor/clone_css.dart`, `lib/src/visitor/*css*.dart`                                                                                                                                                           | `rust-sass/src/ast/css/*.rs`                                                                                                                                                                    |
| A3  | `lib/src/value*.dart`, `lib/src/value/*.dart`                                                                                                                                                                                                                                           | `rust-sass/src/value/*.rs`                                                                                                                                                                      |
| P1  | `lib/src/parse/*.dart`                                                                                                                                                                                                                                                                  | `rust-sass/src/parse/*.rs`                                                                                                                                                                      |
| S1  | `lib/src/visitor/serialize.dart`                                                                                                                                                                                                                                                        | `rust-sass/src/serialize/*.rs`                                                                                                                                                                  |
| S2  | `lib/src/ast/selector/*.dart`, `lib/src/extend/*.dart`, `lib/src/visitor/*selector*.dart`, `lib/src/visitor/replace_expression.dart`, `lib/src/visitor/*plain*`, `lib/src/visitor/*calculation*`, `lib/src/visitor/find_dependencies.dart`, `lib/src/visitor/source_interpolation.dart` | `rust-sass/src/selector/*.rs`, `rust-sass/src/extend/*.rs`                                                                                                                                      |
| C1  | `lib/src/exception.dart`, `lib/src/util/span.dart`, `lib/src/util/lazy_file_span.dart`, `lib/src/util/multi_span.dart`, external `source_span`/`string_scanner`/`term_glyph`/`source_maps`/`path`/`dart:core`                                                                           | `rust-sass/src/common/*.rs`, `rust-sass/src/termglyph.rs`, `rust-sass/src/sourcemap/*.rs`, `rust-sass/src/source_map_buffer.rs`, `rust-sass/src/common/core_errors.rs` (see `upstream.md` pins) |
| C2  | `lib/src/logger*.dart`, `lib/src/logger/*.dart`, `lib/src/deprecation.dart`, `lib/src/syntax.dart`, `lib/src/utils.dart`, `lib/src/util/*.dart`, `lib/src/color_names.dart`, `lib/src/interpolation_*.dart`                                                                             | `rust-sass/src/logger/*.rs`, `rust-sass/src/deprecation.rs`, `rust-sass/src/eval/syntax.rs` + `warn.rs`, `rust-sass/src/unvendor.rs`, `rust-sass/src/util/*.rs`, `rust-sass/src/math*.rs`       |

Notes:

- `evaluate.dart` is a 1:N split. `eval/mod.rs` holds state/config/frames,
  `eval/statement.rs` the statement visitors, `eval/expression.rs` the
  expression visitors, `eval/css.rs` the CSS-output phase,
  `eval/{calc,meta,compat,warn}.rs` the sections named in their headers,
  `eval/helpers.rs` the `_exception`/`_addExceptionSpan` machinery,
  `eval/init.rs` the constructor. Never expect one Rust file to mirror the
  whole Dart file.
- `mod.rs`/`lib.rs`/`Cargo.toml` carry no `dart-source:` by policy
  (`patterns.md` §2). Qualifiers like `(inside EvaluateVisitor)`,
  `(interface only)`, `(external)` disambiguate non-1:1 mappings — trust
  them, then verify.
- Stale/missing annotations are findings: a changed Dart file with no Rust
  counterpart (`porting.md` step 2 recipe) means a missing port, not a pass.

## 3. Accepted Rust adaptations (not bugs)

Check these first so review time goes to real bugs. Each is documented in
`patterns.md` / `architecture.md` / `critical-invariants.md` /
`divergences.md` / `ref/*` — the refs below are entry points, not proofs.

| Dart                            | Rust                                                                                                                                                           | Why                                                                                     |
| ------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| sealed class + `is`/`as`        | enum + exhaustive `match` (`Value`, `Statement`, `Expression`, `Selector`, `CssNode`)                                                                          | closed type set; see `architecture.md` §2–3, `patterns.md` §3                           |
| `stmt.acceptX(v)` family        | single `accept(&mut v)` with `V::Output` (`()` eval/serialize, `bool` predicates, `Expression` replace)                                                        | one traversal, many results; `architecture.md` §4, `critical-invariants.md` accept rule |
| `throw` / `try on T catch`      | `SassResult<T> = Result<T, SassError>` + `?` + `match err`                                                                                                     | no exceptions; every fallible visitor returns `SassResult`                              |
| `null` / `nil` returns          | `Option<T>`; `nil []Stmt` vs `[]` → `Option<Vec<Statement>>` (`None` = no block, `Some(vec![])` = empty `{}`)                                                  | `critical-invariants.md` nil-slice rule                                                 |
| GC sharing                      | arena `&'parse` refs (`Bump` owned by caller) + `'compile: 'parse` / `'parse: 'compile` bounds; `Value = &'parse ValueInner` (`Copy`)                          | `architecture.md` §5, `critical-invariants.md` lifetime rules                           |
| shared mutable frames           | arena `&'parse RefCell<IndexMap>` env stacks (`Copy`), `Rc<RefCell>` + `Weak` CSS parents, `Rc<dyn Logger>` / `Rc<dyn Io>` / `Rc<dyn UserImporter>` seams only | single-threaded `!Send`; `architecture.md` §9–10, `critical-invariants.md` Rc rule      |
| `Map`/`Set` with value `==`     | `IndexMap<Value, Value>`, `HashSet<T>` (value equality, O(1))                                                                                                  | `patterns.md` §5                                                                        |
| class fields + `this` closures  | `EvalConfig` (shared) + `EvalState` (mutable) + free `fn(config, state, …)` (no visitor-trait impls on the evaluator)                                          | borrowck; `architecture.md` §7, `ref/eval.md` borrow patterns                           |
| `Future`/`async` evaluator      | one tree, two builds via `rust-sass-macros` (`maybe_async`, `sync_impl`/`async_impl`, `maybe_block_on!`, `box_rec!`); sync build has zero futures              | `architecture.md` §6, `ref/macros.md`                                                   |
| `buffer.write(x)` void          | `write!(buf, …).unwrap()` on `String` — the only allowed `unwrap`                                                                                              | infallible; `patterns.md` §5, `critical-invariants.md`                                  |
| `toString()` / inspect / css    | `to_display_string()` (list parens) vs `serialize_value_inspect` vs `to_css_string(quote)` (errors on non-CSS values) — never conflate                         | `patterns.md` §6                                                                        |
| `for_span` closure over visitor | callback gets only `&mut SourceMapBuffer`; precompute outside, manual `match` inside                                                                           | borrowck; recorded in `divergences.md` §3, `ref/serialize.md`                           |
| `Module`/`Callable` identity    | `Rc::ptr_eq` / address identity (`Callable::identity_eq/hash`, `CompileContext = Rc<()>`)                                                                      | no ID counter in Dart; `patterns.md` §5, `architecture.md` §10                          |
| `Importer`/`Module` interface   | closed enums (`ImporterKind`, `Module::{BuiltIn,Forwarded,Shadowed,Environment}`); only `Logger`/`UserImporter` stay `dyn`                                     | closed set; `patterns.md` §10                                                           |
| float formatting edge           | `(m[i]*v) as f64` per-product casts (no FMA); `glibc-math` feature on wasm                                                                                      | bit parity; `critical-invariants.md` FMA rule, `ref/math.md`                            |

If a difference is not in this table (or in `divergences.md`), treat it as
a suspect, not an adaptation.

## 4. Per-file review procedure

Apply to each map row in §2, in §5 order. Keep the Dart file open beside
the Rust file(s); judge behavior, not shape.

1. **Locate and size.** Resolve Rust counterpart(s) via the `rg` recipe in
   §2. Note splits/merges/qualifiers. If Dart changed since `PORTED_FROM`
   with no Rust counterpart, file a missing-port finding and stop.
2. **Entry points and names.** For each public Dart function/method, find
   the Rust fn. Names convert mechanically (`isBogus` → `is_bogus`,
   `toSpace` → `to_space`); any rename beyond case/shape, moved logic
   between files, or dropped/added parameter is a finding unless §3 covers
   it (e.g. added `config`/`state`/`arena` params are expected).
3. **Control flow and guards.** Walk the Dart body in order: every guard,
   branch, loop bound, early return, and error site must have a Rust
   counterpart producing the same outcome in the same order. A missing
   `if`, swapped branch order, or combined/split check that changes which
   error fires first is a bug — even if each branch looks right alone.
4. **Errors: variant, message, span, trace.** For every Dart `throw`:
   variant (`SassError::{Script,Runtime,Format,Sass,MultiSpan,MultiSpanScript}`
   in `common/exception.rs:22`), exact message text, span source node, and
   trace/cause/`loaded_urls`. Key refs: `exception(state, msg, span)` in
   `eval/helpers.rs:35` (span falls back to stack top), `stack_trace` at
   `:61`, `add_exception_span` dual impls at `:272`, `with_member_use_span`
   / `with_additional_span` in `common/exception.rs:225,292`. Gruff rule:
   `Script` = unspanned value/type error inside a wrappable callback;
   anything escaping to the user must be `Runtime`/`Format`/`Sass`/`MultiSpan`
   with span + trace.
5. **Wrapping, scope, and effects.** Check per-visit `add_exception_span` /
   `add_error_span` presence (must match Dart per-type, never a blanket
   wrapper in `accept()`); `scope(arena, cb, semi_global, when)` in
   `environment/mod.rs:970,1011` gets `when = has_declarations(children)`
   (`eval/statement.rs:3599`), not `len > 0` or hardcoded bool; `at_root`,
   `closure()`/`with_content()` sharing, `!global` visibility, and CSS
   `with_parent` `through` filters preserved.
6. **Types and data.** `Option` vs missing, `None` vs `Some(vec![])`,
   `IndexMap` ordering, `HashSet` membership, string-form choice (§3 row),
   number/color-space handling, `InterpolationPart` boxing (sole recursive
   box). Value-equality must stay value-equality; `ptr_eq` outside
   `Module`/`Callable` is a bug.
7. **Dual-build parity.** Every `#[maybe_async]` fn compiles both ways;
   `sync_impl`/`async_impl` pairs share a name and logic; `box_rec!` never
   wraps `.await`; no consumer-local `cfg` gates library shape
   (`ref/macros.md`). Diff the pair when a finding touches async code.

Record the outcome per file using §7 before moving on.

## 5. Suggested order (risk-first)

1. E1–E2 evaluator core (`statement.rs`, `expression.rs`, `css.rs`, `helpers.rs`, `mod.rs`, `init.rs`).
2. E3–E4 environment/module/scope + importer/import-cache.
3. E5 built-in functions (argument handling + error variants).
4. E6–E7 compile entry points, callables, evaluation context, configuration.
5. A1–A3 AST + values (usually faithful; focus on new/changed nodes).
6. P1 parser, S1 serializer, S2 selectors/extend.
7. C1–C2 spans/errors/source-maps/logger/deprecation/utils/math.

Within a group, start with files touched since `PORTED_FROM`
(`git -C dart-sass diff --name-only $(cat PORTED_FROM)..origin/main -- lib/`
then §2 recipe per `porting.md` step 2).

## 6. Pattern scans (Rust `rg` recipes)

Use after each file (or group) to catch what line-by-line reading misses.
Every hit needs a Dart-side check — scans suggest, files decide.

```sh
# error-variant suspects: Script escaping where Dart raises a spanned error
rg -n 'SassError::Script' rust-sass/src/eval rust-sass/src/compile rust-sass/src/functions
# Runtime constructed without trace (should come from exception()/stack_trace())
rg -n 'SassError::Runtime\s*\{' rust-sass/src --after 6 | grep -v 'trace:'
# direct Runtime construction on a path Dart routes through _exception()
rg -n 'SassError::(Runtime|Format|Sass|MultiSpan)\s*\{' rust-sass/src/eval
# wrapping: visits missing add_exception_span / add_error_span
rg -n 'fn visit_.*\(' rust-sass/src/eval rust-sass/src/serialize
rg -n 'add_exception_span|add_error_span|with_evaluation_context' rust-sass/src/eval
# scope: hardcoded when or len-based checks instead of has_declarations
rg -n 'has_declarations' rust-sass/src/eval rust-sass/src/environment
rg -n '\.scope\(|scope_when|in_semi_global|at_root' rust-sass/src/eval rust-sass/src/environment
# blocks: None vs Some(vec![]) confusion
rg -n 'Option<Vec<.*Statement>>|Some\(vec!\[\]\)' rust-sass/src/ast rust-sass/src/eval
# string forms conflated
rg -n 'to_display_string|serialize_value_inspect|to_css_string' rust-sass/src
# identity used outside Module/Callable
rg -n 'ptr_eq|identity_eq|identity_hash' rust-sass/src
# sync/async divergence
rg -n 'maybe_async|sync_impl|async_impl|maybe_block_on|box_rec' rust-sass/src rust-sass-macros/src
# float parity hazards
rg -n 'powf|powi| as f64' rust-sass/src/value rust-sass/src/math.rs
# forbidden in compiler code
rg -n 'unsafe|\.unwrap\(\)|\.expect\(' rust-sass/src --glob '!*test*'
# annotation hygiene
rg -n 'dart-source:' rust-sass/src | wc -l
```

`.unwrap()` hits are allowed only for `write!(buf, …)` on `String`;
`expect()` and any other `unwrap` in non-test compiler code is a finding.
`unsafe` in `rust-sass`/`cli`/`embedded`/`wasm` is a finding
(`critical-invariants.md`).

## 7. Verification and finding log

Verify after every fix, not at the end (both builds are one tree compiled
twice — a sync-only pass hides async regressions). Per fix commit:

```sh
cargo test -p rust-sass
cargo test -p rust-sass --features async
cargo clippy -p <touched-crate>
SASS_SPEC=<real/path/under/sass-spec/spec> cargo test -p rust-sass-spec --test runner_test --release -- --ignored
```

Per plan-file exit the bar rises to the full battery (see
`CONTRIBUTING.md`): macros, embedded sync+async, whole `sass-spec`,
`cargo clippy --workspace`, byte-identity spot-check.

Byte-identity (`CONTRIBUTING.md`): bootstrap / `huge` / `huge10` Rust == Dart.
Spot-check CLI parity while reviewing eval changes:

```sh
echo '<scss>' | dart run bin/sass.dart --stdin   # from dart-sass/
echo '<scss>' | cargo run -p rust-sass-cli -- --stdin
```

Log each file (or finding) in the same shape so later passes can skip
clean files without re-reading them:

```md
### `<dart-path>` ↔ `<rust-path(s)>`

- Pins: dart `PORTED_FROM` + submodule short hash, rust commit.
- Verdict: clean | bug | accepted-adaptation (§3 row + why).
- Bugs: Dart line(s) vs Rust line(s), expected vs actual (message/span/trace/order/scope), repro (`SASS_SPEC=` group or stdin snippet).
- Gates: unit sync/async, spec subset, clippy — pass/fail.
```

Finding severity: (a) wrong output/message/span/trace, (b) wrong scope/
ordering/import effect, (c) missing port, (d) annotation drift. Cosmetic
structure with identical behavior is not a finding — if tempted to file
one, re-read §1 and §3 first.

## 8. Reference files

- `porting.md` — delta → file mapping, header/license rules, full battery.
- `patterns.md` — translation conventions (§3–7 error/dispatch/ownership).
- `architecture.md` — pipeline, arena lifetimes, eval/serialize splits.
- `critical-invariants.md` — never-violate rules (FMA, nil-slice, no
  `unwrap`, lifetimes, `Rc` not `Arc`, accept pattern, byte-identity).
- `divergences.md` — intentional differences; do not re-file these.
- `ref/` — per-module detail (`eval.md` borrow patterns, `macros.md` dual
  build, `visitors.md` dispatch, `environment.md`, `common.md`, `math.md`).
- `upstream.md` + `PORTED_FROM` — tracked commit/version/package pins.
- `CONTRIBUTING.md` — build/test/bench/lint procedure.
