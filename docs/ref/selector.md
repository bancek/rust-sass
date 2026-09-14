# Module: `selector/`

Selectors and the extend algorithm primitives. Selectors are enums, not trait
objects.

## Types

```rust
enum Selector<'parse> { Simple(SimpleSelector<'parse>), Compound(CompoundSelector<'parse>), Complex(ComplexSelector<'parse>), List(SelectorList<'parse>) }

enum SimpleSelector<'parse> {
    Attribute, Class, Id, Pseudo, Parent, Placeholder, Type, Universal,   // 8 variants
}
enum Combinator { NextSibling, Child, FollowingSibling }
```

All twelve concrete types derive `Debug`, `Clone`, and `PartialEq`/`Eq`/`Hash`,
with dispatch via `accept()` and the `SelectorVisitor` trait.

## Equality and hashing

- **Span is excluded from equality and hash** everywhere. `ClassSelector`,
  `IdSelector`, `TypeSelector`, `PlaceholderSelector`, `UniversalSelector`,
  `CompoundSelector`, `ComplexSelector`, and `ComplexSelectorComponent` use
  manual `PartialEq`/`Hash` over logical fields only — Dart never compares
  `span`.
- `ParentSelector` is the one identity-based type: a `Copy` arena handle
  (`&'parse ParentSelectorInner`) with `std::ptr::eq`/address-hash
  equality, matching Dart's default `Object.==`.
- `is_superselector` is a **method** on `SimpleSelector` (not a free function):
  `base_is_superselector()` does the Dart base-class logic (`:is`/`:where`/
  `:not`/`:nth-child`), then per-variant `is_superselector_variant()`.

## `SelectorList` dual equality

`SelectorList` is a `Copy` arena handle (`&'parse SelectorListInner`) with two equality modes:

- `SelectorList` — **structural** `Eq`/`Hash` (compares components), matching
  Dart's `==`.
- `SelectorListIdentity` — **identity** `Eq`/`Hash` via `std::ptr::eq`, matching
  Dart's `Map.identity()` in the extension store.

`SelectorList::identity()` returns the identity wrapper; the extension store's
`clone_store()` map uses it to translate old style-rule selectors to new store
boxes.

## Extend algorithms

- **Weave** — LCS-based interleaving of N complex selectors, preserving relative
  order, with `mergeTrailingCombinators` handling the 7 combinator-merge cases.
- **Unify** — `unify_complex` extracts and unifies base compounds, weaves the
  results, and returns `None` on ID conflicts; `unifyUniversalAndElement` and
  `namespaceAndName` cover the simple-selector unifications.
- **Superselector** — `list`/`complex`/`compound`/`simple` levels, with
  pseudo-class strategies for `:is`/`:not`/`:where`/`:has`/`:nth-child`.

### Specificity (base-1000)

| Selector                                            | Value              |
| --------------------------------------------------- | ------------------ |
| Universal                                           | 0                  |
| Type, pseudo-element                                | 1                  |
| Attribute, class, placeholder, parent, pseudo-class | 1000               |
| ID                                                  | 1,000,000          |
| `:where()`                                          | 0                  |
| `:is()`, `:not()`, `:has()`, `:matches()`           | max of args        |
| `:nth-child()`, `:nth-last-child()`                 | 1000 + max of args |

## File mapping

| Dart                                        | Go                                        | Rust                                |
| ------------------------------------------- | ----------------------------------------- | ----------------------------------- |
| `lib/src/selector/*.dart`, `visitor/*.dart` | `go/value/selector*.go`                   | `src/selector/*.rs`                 |
| `lib/src/visitor/recursive_selector.dart`   | `go/value/selector_recursive_selector.go` | `src/selector/recursive.rs`         |
| `lib/src/selector/extend.dart`              | `go/value/selector_extend.go`             | `src/selector/weave.rs`, `unify.rs` |
