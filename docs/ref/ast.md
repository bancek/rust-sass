# Module: `ast/`

The Sass abstract syntax tree (statements and expressions), the CSS output
tree, and the visitors that walk them. See also `visitors.md` for the visitor
traits themselves.

## Design: enums, not trait objects

Three type hierarchies, each a closed enum with a matching visitor trait:

```
Statement<'parse>             (27 variants) → StatementVisitor
Expression<'parse>            (18 variants) → ExpressionVisitor
IfConditionExpression<'parse> (6 variants)  → IfConditionExpressionVisitor
SupportsCondition<'parse>     (6 variants)  → no visitor (methods via match)
CssNode<'parse>               (9 variants)  → CssVisitor
ModifiableCssNode             (9 variants)  → ModifiableCssVisitor
```

**Why enums over `dyn`:** `Vec<CssNode>` is stack-sized (no `Box<dyn>` for
children), `match` is a jump table (no vtable), and adding a variant forces
every `match` to be updated — the properties Dart's sealed classes give, without
a class hierarchy. Recursive positions are boxed (`BinaryOperation` operands,
`InterpolationPart` expressions) with child lists in `Vec`s.

## Sass AST

- **Statements (27)** span the whole language: `Stylesheet`, `StyleRule`,
  `AtRule`, `AtRootRule`, `Declaration`, `VariableDeclaration`, the import/module
  rules (`ImportRule`, `UseRule`, `ForwardRule`), callable rules (`IncludeRule`,
  `FunctionRule`, `MixinRule`, `ContentRule`, `ContentBlock`), control flow
  (`IfRule`, `EachRule`, `ForRule`, `WhileRule`), `MediaRule`, `SupportsRule`,
  `ExtendRule`, `ErrorRule`/`WarnRule`/`DebugRule`/`ReturnRule`, and comments.
- **Expressions (18)** include `BinaryOperation`, `Function`, `IfExpression`,
  `LegacyIfExpression`, `List`, `Map`, `String`, `Number`, `Color`, `Boolean`,
  `Null`, `Value`, `Variable`, `Selector`, `Parenthesized`, `UnaryOperation`,
  `SupportsExpression`, `InterpolatedFunction`.
- **If conditions (6)**: `Parenthesized`, `Negation`, `Operation`, `Function`,
  `Sass`, `Raw`.
- **Supports conditions (6)**: `SupportsAnything`, `SupportsDeclaration`,
  `SupportsFunction`, `SupportsInterpolation`, `SupportsNegation`,
  `SupportsOperation`. `SupportsOperation` is **binary** (`left`/`right`, not a
  `Vec`); `SupportsExpression.condition` is a `Box<SupportsCondition>` to break
  the recursion cycle with the `Expression` enum.

### Support and wrapper types

- `Parameter`, `ParameterList`, `ConfiguredVariable`, `AtRootQuery`, and
  `Import`. `AtRootQuery` has **no span and no lifetime** — it is pure owned
  data (`names`, `include`, plus `_all`/`_rule`). Parsing threads through the
  eval caller's arena and interpolation map (`AtRootQuery::parse(arena,
contents, Some(map))` → `AtRootQueryParser::new(source, map)`), so
  query-parse spans resolve against the original interpolation sites.
  `ParameterList.span_with_name` expands the span over a preceding identifier
  (Dart's `spanWithName`); `ParseTimeWarning` carries optional
  `primary_label`/`secondary` beyond Dart's 3-field record (always `None`/empty
  at construction sites).
- `IncludeRule.name_span` strips the `+`/at-rule prefix **and** the namespace
  (`withoutNamespace`), so `@include ns.foo` covers `foo`;
  `namespace_span` shares the same prefix-stripping and covers `ns`
  (excluding the `+` in indented `+ns.foo`). `MixinRule.name_span`
  `trimLeft`s after the `=` subspan (indented `= foo` starts at `foo`).
- `DynamicImport::url` is infallible: residual parse failures fall back to
  `SassUrl::parse_relative_fallback` (raw string as a relative URL) instead
  of panicking, matching Dart's never-throwing `Uri.parse`.
- `BinaryOperationExpression.operator_span` trims the between-span only when
  both operands share one `FileSource` **allocation**
  (`FileSource::identical`, matching Dart's `SourceFile ==` identity);
  separate-but-equal allocations fall back to the full span.
- Five **wrapper enums** — `SassDeclaration`, `CallableInvocation`,
  `SassReference`, `SassDependency`, `Import` — exist for structural fidelity:
  they are **dead code** (0 dispatch sites; `CallableInvocation` has one
  Dart-only site, and only `Import` is actually used, ~7 sites), kept so the
  port mirrors Dart's sealed interfaces. `SassDeclaration` has **5 variants**
  (not 7 — `ForwardRule` and `UseRule` do not implement it).
- `CallableDeclaration` is an enum wrapping `MixinRule`/`FunctionRule`/
  `ContentBlock` (Dart's abstract base class); `ContentBlock`'s name is
  `"@content"`. `VariableDeclaration` rejects `namespace.is_some() && global`
  (you cannot declare another module's member with `!global`).

### The nil-slice invariant

Where Go distinguishes `nil []Statement` (no block, ends with `;`) from
`[]Statement{}` (an empty `{}` block), Rust uses `Option<Vec<Statement>>`:
`None` = no block, `Some(vec![])` = empty block. This affects `Declaration`,
`AtRule`, `StyleRule`, `MediaRule`, `SupportsRule`, and `IncludeRule`'s content
argument.

### `InterpolationMap` (span back-mapping)

`InterpolationMap { interpolation, target_offsets }` maps generated-output
offsets back to the interpolation's source spans: `map_span` (guarded by
`is_mapped`), `map_span_inner` (unguarded core), `map_file_span`, and
`map_exception`. Provenance is **address identity**
(`FileSource::identical`, matching Dart's `identical()` on `SourceFile`) —
never structural `==` and never URL comparison (`None == None` would match
everything). `map_file_span`/`map_exception` synthesize their query spans in
the interpolation's own file (a coordinate-space placeholder — the owned
snapshot carries generated-output offsets but no generated-file ref), so they
route through `map_span_inner`, bypassing the `is_mapped` guard; only a
genuinely same-allocation span early-returns. See `parse.md` (three-phase
error mapping) and `common.md` (`FileSource::identical`).

### Display conventions

`StringExpression.source_interpolation` returns the bare `text`
interpolation (Dart's visitor writes `span.text`, and `span` covers the
quotes on both sides — same observable text). `as_interpolation` re-adds
quotes for quoted strings (Dart's `asInterpolation`, including the
`quoteText` shape: best-quote choice + static `#{` escaping); `UseRule`
display quotes the URL and elides `as <ns>` when the namespace matches the
URL basename (last path segment up to the first `.`, Dart's
`pathSegments.last`).

## CSS AST

Two parallel enums, mirroring Dart's mutable/immutable split:

```rust
// Frozen (serialization)
enum CssNode<'parse> { Stylesheet, StyleRule, AtRule, Comment, Declaration, Import, KeyframeBlock, MediaRule, SupportsRule }
enum CssParentNode<'parse> { Stylesheet, StyleRule, AtRule, KeyframeBlock, MediaRule, SupportsRule }  // the 6 parent variants

// Mutable (evaluation)
enum ModifiableCssNodeKind<'parse> { /* same 9 variants, modifiable forms */ }
struct ModifiableCssNode<'parse> { inner: Rc<RefCell<ModifiableCssNodeInner<'parse>>> }
struct ModifiableCssNodeInner<'parse> { kind, parent: Weak<...>, index_in_parent: usize, is_group_end: bool }
```

### Parent chain

Parents own children via `Vec<ModifiableCssNode>` (each an `Rc` clone); children
hold a `Weak` back-pointer, so there is no reference cycle. All parent logic
lives on the outer `ModifiableCssNode`; `ModifiableCssNodeKind` is unaware of it.

- `children()` returns an **owned** `Vec` (drops the `RefCell` borrow before the
  caller recurses, avoiding double-borrow panics); leaf types return `None`
  (`children_ref()`/`children_mut()`/`is_parent()` are `Option`-returning, no
  panics).
- `PartialEq` on `ModifiableCssNode` is **identity** (`Rc::ptr_eq`), used for
  `parent == root` checks.
- Tree guards mirror Dart's modifiable node: `add_child` errors on leaf types
  and on childless at-rules (Dart has no `addChild` on leaves / asserts
  `!isChildless`); `remove` errors without a parent (Dart `StateError`);
  `clear_children` nulls both parent and child index (`usize::MAX` is the
  unset sentinel — indices are always valid positions while attached, and
  `add_child` re-stamps on insert).
- `add_child`, `remove`, `clear_children`, `has_following_sibling` work through
  the `Weak::upgrade() → drop Ref → borrow_mut()` pattern: the mutable borrow
  targets the _parent's_ `RefCell` while reindexing borrows the _child's_ cell —
  different cells, no conflict.
- `is_invisible` mirrors Dart's shared `CssNode.isInvisible`
  (`_IsInvisibleVisitor`): style rules check selector + children, at-rules are
  never invisible, and every other parent (stylesheet, keyframe block,
  media/supports rules) is invisible iff all children are; leaf types
  (comment, declaration, import) are visible.
- `equals_ignoring_children` is Dart-exact per variant (`Stylesheet → true`;
  `StyleRule → SelectorList value equality` (`selector_value()`, matching Dart's
  `other.selector == selector` — never allocation identity);
  `AtRule → name+value+childless`;
  `KeyframeBlock → selector.value`; `MediaRule → queries`; `SupportsRule →
condition`; everything else `false`).
- `copy_without_children` clones all distinguishing fields **except**
  `StyleRule.from_plain_css` (reset to `false`, matching Dart's constructor
  default) and empties the children — used when a target parent has a
  following sibling, to avoid corrupting it, and when copying style rules
  into `@media`/at-rule bodies so re-evaluated plain-CSS content merges
  instead of nesting.
- `to_css_node()` is a one-time O(n) deep conversion to the frozen `CssNode`
  enum, performed once at the end of evaluation.

**Why the covariant-parent design:** Dart's covariant `parent` return keeps
parent-chain walks in modifiable-land (its evaluator needs exactly one
downcast); Go's interface-based port needs 15+. Rust eliminates them by making
the mutable node a concrete enum type.

### `clone_css`

`clone_css_stylesheet(&ModifiableCssNode, &ExtensionStore) -> SassResult<(ModifiableCssNode, ExtensionStore)>`
deep-copies a module's CSS tree and its extension store for `@use` boundaries.
The clone map is `HashMap<SelectorListIdentity, StoreBox<SelectorList>>`
keyed by selector allocation identity; a selector id absent from the map
returns `Err("The ExtensionStore and CssStylesheet passed to
clone_css_stylesheet() must come from the same compilation.")`
— the two must come from the same compilation. Cloned style rules drop
`from_plain_css` (Dart constructor default `false`), like
`copy_without_children`.

## Interpolated selectors

`InterpolatedSelector` (4 variants) and `InterpolatedSimpleSelector` (8 variants)
represent selectors still containing `#{}` interpolation at parse time, with an
`InterpolatedSelectorVisitor` (11 methods).

## File mapping

| Dart                                        | Go                               | Rust                                    |
| ------------------------------------------- | -------------------------------- | --------------------------------------- |
| `lib/src/ast/sass/*.dart`, `ast/css/*.dart` | `go/value/sass_*.go`, `css_*.go` | `src/ast/sass/*.rs`, `src/ast/css/*.rs` |
