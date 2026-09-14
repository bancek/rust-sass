# Module: `member_map.rs`

Read-only ordered map views for module member filtering (`@forward ... show/hide`,
prefixes, shadowing). Ports Dart's `util/{prefixed,unprefixed,limited,merged,
public_member_map_view}.dart` and Go's `orderedmap` package.

## `MemberMap` trait

```rust
pub trait MemberMap<V: Clone>: Debug {
    fn get(&self, key: &str) -> Option<V>;
    fn has(&self, key: &str) -> bool;
    fn len(&self) -> usize;
    fn keys(&self) -> Vec<String>;              // insertion order
    fn entries(&self) -> Vec<(String, V)>;      // insertion order
}
```

Views are arena-allocated `&'parse dyn MemberMap` — composition allocates the
next view layer into the `Bump`, so building a filtered view never allocates
outside the compile arena.

## Views

| Type                  | Dart counterpart              | Semantics                                                                                                                                                                                                                                                                                                                                              |
| --------------------- | ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `IndexMapView<V>`     | owned map                     | Wraps an owned `IndexMap` (built-in modules).                                                                                                                                                                                                                                                                                                          |
| `RefCellMap<'p, V>`   | —                             | Borrows an arena `&RefCell<IndexMap>` (environment frames).                                                                                                                                                                                                                                                                                            |
| `PrefixedMapView`     | `prefixed_map_view.dart`      | Strips/adds the `@forward` prefix on lookup.                                                                                                                                                                                                                                                                                                           |
| `LimitedMapView`      | `limited_map_view.dart`       | `new_safelist` (show-list) / `new_blocklist` (hide-list). `new_safelist_ordered` (safelist `&[String]` in source order) makes `keys`/`entries` follow **safelist** order like Dart's `Set.intersection` — observable via `meta.module-variables`. The plain `new_safelist` (unordered `HashSet`) keeps membership right but falls back to inner order. |
| `MergedMapView`       | `merged_map_view.dart`        | First-appearance order, later maps win on conflicts.                                                                                                                                                                                                                                                                                                   |
| `PublicMemberMapView` | `public_member_map_view.dart` | Hides private (`-`/`_` prefixed) members.                                                                                                                                                                                                                                                                                                              |

## Composition helpers

```rust
pub fn forwarded_map<'a, V>(arena, inner, prefix: Option<&str>, safelist: Option<&HashSet<String>>, blocklist: Option<&HashSet<String>>) -> &'a dyn MemberMap<V>;
pub fn forwarded_map_ordered<'a, V>(arena, inner, prefix: Option<&str>, safelist_order: Option<&[String]>, safelist: Option<&HashSet<String>>, blocklist: Option<&HashSet<String>>) -> &'a dyn MemberMap<V>;
// Dart ForwardedModuleView._forwardedMap: prefix → safelist/blocklist; returns inner unchanged when no filtering applies.
// The ordered variant threads `ForwardRule.shown_order_*` (source-order show lists from `member_list_impl`, which returns both Vecs and HashSets) so safelist order survives end to end.
pub fn member_map<'a, V>(arena, local, forwarded: Vec<&'a dyn MemberMap<V>>) -> &'a dyn MemberMap<V>;
// Dart _EnvironmentModule._memberMap: local wrapped in PublicMemberMapView, then merged; local last so it shadows forwarded.
pub fn needs_blocklist(map, blocklist) -> bool;   // Go needsBlocklist
pub fn map_key_diff(full, filtered) -> HashSet<String>;  // Go mapKeyDiff (used by shadowed.rs clone)
```

## Working here

- Key **order** is observable (`meta.module-variables` iteration, `@each`
  serialization) — any new view must document whether it preserves inner
  order, safelist order, or first-appearance order, with a test locking it.
- Views are read-only by design; shadowing/mutation happens in
  `module/shadowed.rs` + `environment/mod.rs`, never here.
