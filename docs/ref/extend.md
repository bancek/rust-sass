# Module: `extend/`

The `@extend` store: how extensions are registered and retroactively applied to
selectors.

## Types

```rust
enum ExtendMode { Normal, Replace, AllTargets }

struct Extender { selector: ComplexSelector, specificity: usize, is_original: bool }
struct Extension { extender: Extender, target: Box<SimpleSelector>, media_context: Option<Vec<CssMediaQuery>>, is_optional: bool, span: FileSpan }

enum ExtensionStore { Empty, Default(DefaultExtensionStore) }

struct DefaultExtensionStore {
    selectors: IndexMap<SimpleSelector, HashSet<MutableBox<SelectorList>>>,
    extensions: IndexMap<SimpleSelector, IndexMap<ComplexSelector, Extension>>,
    extensions_by_extender: IndexMap<SimpleSelector, Vec<Extension>>,
    media_contexts: HashMap<MutableBox<SelectorList>, Vec<CssMediaQuery>>,
    source_specificity: IndexMap<SimpleSelector, usize>,
    originals: HashSet<ComplexSelector>,
    mode: ExtendMode,
    // ...
}
```

`ExtendMode::AllTargets` bypasses superselector checking when choosing
extenders. `mergedExtension` (a binary tree in `merged.rs`) records the merge
history of an extension for debug and error messages.

`CssMediaQuery` compares **without** `conjunction` (modifier, type,
conditions only) and its `Hash` excludes `conjunction` too, matching Dart's
`==`/`hashCode` — so the `Hash`/`Eq` contract holds.

## Identity wrappers

`MutableBox` (a shared mutable reference) and `StoreBox` (a sealed, read-only
handle) both use `Rc::ptr_eq` identity — there is no `id` counter (Dart's
`ModifiableBox` uses default reference equality). `SelectorList` itself has
**structural** `Eq`/`Hash` (compares components), while `SelectorListIdentity`
provides **identity** `Eq`/`Hash` via `Rc::ptr_eq` — matching Dart's
`Map.identity()`. `clone_store()` returns a `HashMap<SelectorListIdentity,
StoreBox<SelectorList>>` keyed by `SelectorListIdentity`: the CSS clone visitor
uses this map to translate old style-rule selectors to new store boxes at a
`@use` boundary.

## Add-selector flow

1. Mark originals.
2. `extendList` → `extendComplex` → per compound, `extendSimple` → look up the
   extensions map.
3. `unifyExtenders` — weave the matching extenders.
4. `Trim` — superselector dedup, **skipped if the result exceeds 100** (a
   performance guard).
5. Seal the box (prevents further modification).

## Add-extension flow

1. Register in `extensions` (target → extender → `Extension`).
2. Register in `extensions_by_extender` (the reverse index).
3. **Retroactive propagation** — extend all previously-registered selectors that
   contain the target.
4. **Chain propagation** — `extendExistingExtensions` walks
   `extensions_by_extender` so an extender of an extender's target is also
   propagated; previously extended selectors must **not** be double-extended.

## Scoping

`assertCompatibleMediaContext` prevents an extension from crossing media-query
boundaries. Private placeholders (prefixed `-`/`_`) are filtered out of the
selectors index.

## File mapping

| Dart                            | Go               | Rust                                            |
| ------------------------------- | ---------------- | ----------------------------------------------- |
| `lib/src/extend/*.dart`         | `go/extend/*.go` | `src/extend/{mode,extension,store,merged}.rs`   |
| `lib/src/ast/selector/box.dart` | `go/box/box.go`  | `src/extend/store.rs` (`MutableBox`/`StoreBox`) |
