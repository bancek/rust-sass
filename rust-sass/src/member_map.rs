// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/util/prefixed_map_view.dart
//              lib/src/util/unprefixed_map_view.dart
//              lib/src/util/limited_map_view.dart
//              lib/src/util/merged_map_view.dart
//              lib/src/util/public_member_map_view.dart
// go-source: go/orderedmap/ (Map interface, PrefixedMapView, LimitedMapView,
//            MergedMapView, PublicMemberMapView, LinkedMap)

//! Read-only ordered map views for module member filtering (`@forward ...
//! show/hide`, prefixes, shadowing).
//!
//! Ports Dart's `util/{prefixed,limited,merged,public_member_map_view}.dart`
//! plus `UnprefixedMapView` (`util/unprefixed_map_view.dart`, whose prefix-
//! stripping read path is folded into [`Configuration`]'s `Filter::Prefix`
//! handling rather than kept as a view) and Go's `orderedmap` package.

use std::cell::{Ref, RefCell};
use std::collections::HashSet;
use std::fmt::Debug;
use std::fmt::Formatter;

use bumpalo::Bump;
use indexmap::IndexMap;

// ===========================================================================
// MemberMap trait — read-only ordered map for module member views.
// Mirrors Go's orderedmap.Map[K,V] interface and Dart's Map<K,V> usage
// in module views. Views are arena-allocated `&'parse dyn MemberMap`.
// Key order is observable (`meta.module-variables`, `@each`
// serialization): every view below documents which order it preserves.
// ===========================================================================

/// Read-only ordered map view over module members.
///
/// Dart: the `Map` role in the module views; Go: `orderedmap.Map`.
pub trait MemberMap<V: Clone>: Debug {
    fn get(&self, key: &str) -> Option<V>;
    fn has(&self, key: &str) -> bool;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Keys in insertion order.
    fn keys(&self) -> Vec<String>;
    /// (key, value) pairs in insertion order.
    fn entries(&self) -> Vec<(String, V)>;
}

// ===========================================================================
// IndexMapView — wraps an owned IndexMap (for BuiltInModule, immutable).
//
// Rust-only: Dart's built-ins hold plain maps; the view bridges them to the
// `MemberMap` composition chain without copying.
// ===========================================================================

#[derive(Debug, Clone)]
pub struct IndexMapView<V: Clone + Debug> {
    pub map: IndexMap<String, V>,
}

impl<V: Clone + Debug> MemberMap<V> for IndexMapView<V> {
    fn get(&self, key: &str) -> Option<V> {
        self.map.get(key).cloned()
    }

    fn has(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn keys(&self) -> Vec<String> {
        self.map.keys().cloned().collect()
    }

    fn entries(&self) -> Vec<(String, V)> {
        self.map
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

// ===========================================================================
// RefCellMap — wraps &RefCell<IndexMap> for live environment views.
// Reads through RefCell on each access to see mutations.
//
// Rust-only: Dart's module views read live maps directly (GC aliasing); the
// wrapper reproduces that through the arena `&RefCell` frames.
// ===========================================================================

#[derive(Debug, Clone)]
pub struct RefCellMap<'p, V: Clone + Debug> {
    pub inner: &'p RefCell<IndexMap<String, V>>,
}

impl<'p, V: Clone + Debug> RefCellMap<'p, V> {
    pub fn new(inner: &'p RefCell<IndexMap<String, V>>) -> Self {
        Self { inner }
    }

    /// Returns a Ref guard for direct borrow access (for tests).
    pub fn borrow(&self) -> Ref<'_, IndexMap<String, V>> {
        self.inner.borrow()
    }
}

impl<'p, V: Clone + Debug> MemberMap<V> for RefCellMap<'p, V> {
    fn get(&self, key: &str) -> Option<V> {
        self.inner.borrow().get(key).cloned()
    }

    fn has(&self, key: &str) -> bool {
        self.inner.borrow().contains_key(key)
    }

    fn len(&self) -> usize {
        self.inner.borrow().len()
    }

    fn keys(&self) -> Vec<String> {
        self.inner.borrow().keys().cloned().collect()
    }

    fn entries(&self) -> Vec<(String, V)> {
        self.inner
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

// ===========================================================================
// PrefixedMapView — adds a prefix to all keys.
// Dart: PrefixedMapView (lib/src/util/prefixed_map_view.dart)
// Keys iterate in inner order with the prefix prepended; lookups strip the
// prefix before delegating.
// ===========================================================================

/// An unmodifiable view of a map with string keys that allows keys to be
/// accessed with an additional prefix.
///
/// Dart: `PrefixedMapView` (`util/prefixed_map_view.dart`).
pub struct PrefixedMapView<'s, V: Clone> {
    pub inner: &'s dyn MemberMap<V>,
    pub prefix: String,
}

impl<'s, V: Clone> Debug for PrefixedMapView<'s, V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrefixedMapView")
            .field("prefix", &self.prefix)
            .field("inner", &self.inner)
            .finish()
    }
}

impl<'s, V: Clone> MemberMap<V> for PrefixedMapView<'s, V> {
    fn get(&self, key: &str) -> Option<V> {
        key.strip_prefix(&self.prefix)
            .and_then(|stripped| self.inner.get(stripped))
    }

    fn has(&self, key: &str) -> bool {
        key.starts_with(&self.prefix) && self.inner.has(&key[self.prefix.len()..])
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn keys(&self) -> Vec<String> {
        self.inner
            .keys()
            .into_iter()
            .map(|k| format!("{}{}", self.prefix, k))
            .collect()
    }

    fn entries(&self) -> Vec<(String, V)> {
        self.inner
            .entries()
            .into_iter()
            .map(|(k, v)| (format!("{}{}", self.prefix, k), v))
            .collect()
    }
}

// ===========================================================================
// LimitedMapView — safelist or blocklist filter.
// Dart: LimitedMapView (lib/src/util/limited_map_view.dart)
// Precomputes allowed keys at construction. Reads live values from inner.
//
// Key order: safelist views iterate the *safelist* (like Dart's
// `safelist.intersection(MapKeySet)`); blocklist views keep inner order.
// Unlike Dart, `remove` is absent: Rust views are fully read-only (Dart's
// `remove` served `@use ... with`, which `Configuration` handles here).
// ===========================================================================

/// A mostly-unmodifiable view of a map that only allows certain keys to be
/// accessed. Behaves as though disallowed keys don't exist, even when the
/// underlying map holds them; values are read live from the inner map, but
/// the key set is frozen at construction.
///
/// Dart: `LimitedMapView` (`util/limited_map_view.dart`).
pub struct LimitedMapView<'s, V: Clone> {
    inner: &'s dyn MemberMap<V>,
    allowed: HashSet<String>,
    /// Safelist order (Dart `LimitedMapView.safelist` iterates the safelist).
    /// `None` for blocklist views (inner order). Stored because `HashSet`
    /// iteration is nondeterministic — `keys`/`entries` must follow it.
    safelist_order: Option<Vec<String>>,
}

impl<'s, V: Clone> Debug for LimitedMapView<'s, V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LimitedMapView")
            .field("allowed", &self.allowed)
            .field("inner", &self.inner)
            .finish()
    }
}

impl<'s, V: Clone + Debug> LimitedMapView<'s, V> {
    /// Returns a view allowing only keys in `safelist` (Dart:
    /// `LimitedMapView.safelist`).
    pub fn new_safelist(inner: &'s dyn MemberMap<V>, safelist: &HashSet<String>) -> Self {
        // Dart `LimitedMapView.safelist`: `safelist.intersection(MapKeySet(_map))`
        // — iteration follows the SAFEList order, not the inner map's. But the
        // safelist here is a `HashSet` (order lost at parse); the ordered
        // variant below must be used where order matters. This keeps the
        // membership right; order falls back to inner order.
        let allowed: HashSet<String> = safelist
            .iter()
            .filter(|k| inner.has(k.as_str()))
            .cloned()
            .collect();
        Self {
            inner,
            allowed,
            safelist_order: None,
        }
    }

    /// Ordered variant: `safelist` in source order (Dart `LinkedHashSet`
    /// iteration). `keys`/`entries` follow it; membership as above.
    pub fn new_safelist_ordered(inner: &'s dyn MemberMap<V>, safelist: &[String]) -> Self {
        let allowed: HashSet<String> = safelist
            .iter()
            .filter(|k| inner.has(k.as_str()))
            .cloned()
            .collect();
        let order: Vec<String> = safelist
            .iter()
            .filter(|k| inner.has(k.as_str()))
            .cloned()
            .collect();
        Self {
            inner,
            allowed,
            safelist_order: Some(order),
        }
    }

    /// Returns a view excluding keys in `blocklist` (Dart:
    /// `LimitedMapView.blocklist`).
    pub fn new_blocklist(inner: &'s dyn MemberMap<V>, blocklist: &HashSet<String>) -> Self {
        let allowed: HashSet<String> = inner
            .keys()
            .into_iter()
            .filter(|k| !blocklist.contains(k.as_str()))
            .collect();
        Self {
            inner,
            allowed,
            safelist_order: None,
        }
    }
}

impl<'s, V: Clone> MemberMap<V> for LimitedMapView<'s, V> {
    fn get(&self, key: &str) -> Option<V> {
        if !self.allowed.contains(key) {
            return None;
        }
        self.inner.get(key)
    }

    fn has(&self, key: &str) -> bool {
        self.allowed.contains(key)
    }

    fn len(&self) -> usize {
        self.allowed.len()
    }

    fn keys(&self) -> Vec<String> {
        if let Some(ref order) = self.safelist_order {
            return order.clone();
        }
        self.inner
            .keys()
            .into_iter()
            .filter(|k| self.allowed.contains(k))
            .collect()
    }

    fn entries(&self) -> Vec<(String, V)> {
        if let Some(ref order) = self.safelist_order {
            return order
                .iter()
                .filter_map(|k| self.inner.get(k).map(|v| (k.clone(), v)))
                .collect();
        }
        self.inner
            .entries()
            .into_iter()
            .filter(|(k, _)| self.allowed.contains(k))
            .collect()
    }
}

// ===========================================================================
// MergedMapView — merges multiple maps into one. Later maps win.
// Dart: MergedMapView (lib/src/util/merged_map_view.dart)
// Precomputes mapsByKey at construction. Reads live values from owning maps.
// Keys iterate in first-appearance order; each key reads from the LAST map
// that yielded it. Nested merged views are NOT flattened (unlike Dart) —
// composition depth here is bounded (local + forwarded), so the O(depth)
// overhead Dart avoids never materializes.
// ===========================================================================

/// An unmodifiable view of multiple maps merged as though a single map.
/// Values in later maps take precedence over earlier ones.
///
/// Dart: `MergedMapView` (`util/merged_map_view.dart`).
pub struct MergedMapView<'s, V: Clone> {
    pub maps_by_key: IndexMap<String, &'s dyn MemberMap<V>>,
    count: usize,
}

impl<'s, V: Clone> Debug for MergedMapView<'s, V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergedMapView")
            .field("count", &self.count)
            .field("maps_by_key", &self.maps_by_key)
            .finish()
    }
}

impl<'s, V: Clone + Debug> MergedMapView<'s, V> {
    /// Creates a combined view of `maps`. Values may change independently of
    /// the view, but the key sets may not.
    pub fn new(maps: Vec<&'s dyn MemberMap<V>>) -> Self {
        let mut maps_by_key: IndexMap<String, &'s dyn MemberMap<V>> = IndexMap::new();
        for map in maps {
            for k in map.keys() {
                maps_by_key.insert(k, map);
            }
        }
        let count = maps_by_key.len();
        Self { maps_by_key, count }
    }
}

impl<'s, V: Clone> MemberMap<V> for MergedMapView<'s, V> {
    fn get(&self, key: &str) -> Option<V> {
        self.maps_by_key.get(key)?.get(key)
    }

    fn has(&self, key: &str) -> bool {
        self.maps_by_key.contains_key(key)
    }

    fn len(&self) -> usize {
        self.count
    }

    fn keys(&self) -> Vec<String> {
        self.maps_by_key.keys().cloned().collect()
    }

    fn entries(&self) -> Vec<(String, V)> {
        self.maps_by_key
            .iter()
            // `maps_by_key` maps each key to an owning map that yielded it in
            // `new`, so `get` is `Some` by construction; skip (rather than
            // panic) if an underlying map changed out from under the view.
            .filter_map(|(k, m)| m.get(k).map(|v| (k.clone(), v)))
            .collect()
    }
}

// ===========================================================================
// PublicMemberMapView — hides Sass-private members (_ and - prefix).
// Dart: PublicMemberMapView (lib/src/util/public_member_map_view.dart)
// Length is NOT O(1), matching Dart. Keys keep inner order, filtered.
// ===========================================================================

/// An unmodifiable view hiding members whose names begin with `_` or `-`.
///
/// Dart: `PublicMemberMapView` (`util/public_member_map_view.dart`).
pub struct PublicMemberMapView<'s, V: Clone> {
    pub inner: &'s dyn MemberMap<V>,
}

impl<'s, V: Clone> Debug for PublicMemberMapView<'s, V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublicMemberMapView")
            .field("len", &self.len())
            .finish()
    }
}

impl<'s, V: Clone> PublicMemberMapView<'s, V> {
    fn is_public(name: &str) -> bool {
        if name.is_empty() {
            return true;
        }
        let first = name.as_bytes()[0];
        first != b'-' && first != b'_'
    }
}

impl<'s, V: Clone> MemberMap<V> for PublicMemberMapView<'s, V> {
    fn get(&self, key: &str) -> Option<V> {
        if !Self::is_public(key) {
            return None;
        }
        self.inner.get(key)
    }

    fn has(&self, key: &str) -> bool {
        Self::is_public(key) && self.inner.has(key)
    }

    fn len(&self) -> usize {
        self.keys().len()
    }

    fn keys(&self) -> Vec<String> {
        self.inner
            .keys()
            .into_iter()
            .filter(|k| Self::is_public(k))
            .collect()
    }

    fn entries(&self) -> Vec<(String, V)> {
        self.inner
            .entries()
            .into_iter()
            .filter(|(k, _)| Self::is_public(k))
            .collect()
    }
}

// ===========================================================================
// Composition helpers
// ===========================================================================

/// Wraps `inner` so it only shows members allowed by `prefix` then
/// `safelist`/`blocklist` (at most one of the two). Returns `inner` unchanged
/// when no filtering applies.
///
/// Dart: `ForwardedModuleView._forwardedMap` (`forwarded_view.dart:86-110`).
/// Mirrors: ForwardedModuleView._forwardedMap (forwarded_view.dart:86-110)
pub fn forwarded_map<'a, V: Clone + Debug>(
    arena: &'a Bump,
    inner: &'a dyn MemberMap<V>,
    prefix: Option<&str>,
    safelist: Option<&HashSet<String>>,
    blocklist: Option<&HashSet<String>>,
) -> &'a dyn MemberMap<V> {
    forwarded_map_ordered(arena, inner, prefix, None, safelist, blocklist)
}

/// Ordered variant: `safelist_order` carries the source-order list (Dart
/// `LinkedHashSet` iteration) so `keys`/`entries` follow it.
pub fn forwarded_map_ordered<'a, V: Clone + Debug>(
    arena: &'a Bump,
    inner: &'a dyn MemberMap<V>,
    prefix: Option<&str>,
    safelist_order: Option<&[String]>,
    safelist: Option<&HashSet<String>>,
    blocklist: Option<&HashSet<String>>,
) -> &'a dyn MemberMap<V> {
    if prefix.is_none() && safelist.is_none() && blocklist.is_none_or(|b| b.is_empty()) {
        return inner;
    }

    let mut result: &'a dyn MemberMap<V> = inner;
    if let Some(p) = prefix {
        result = arena.alloc(PrefixedMapView {
            inner: result,
            prefix: p.to_string(),
        });
    }
    if let Some(order) = safelist_order {
        result = arena.alloc(LimitedMapView::new_safelist_ordered(result, order));
    } else if let Some(s) = safelist {
        result = arena.alloc(LimitedMapView::new_safelist(result, s));
    } else if let Some(b) = blocklist {
        if !b.is_empty() {
            result = arena.alloc(LimitedMapView::new_blocklist(result, b));
        }
    }
    result
}

/// Wraps `local` in [`PublicMemberMapView`], then merges it with `forwarded`
/// so local members shadow forwarded ones (local last, later maps win).
/// Empty forwarded maps are skipped.
///
/// Dart: `_EnvironmentModule._memberMap` (`environment.dart:1055-1072`).
/// Mirrors: _EnvironmentModule._memberMap (environment.dart:1057-1072)
pub fn member_map<'a, V: Clone + Debug>(
    arena: &'a Bump,
    local: &'a dyn MemberMap<V>,
    forwarded: Vec<&'a dyn MemberMap<V>>,
) -> &'a dyn MemberMap<V> {
    let public = arena.alloc(PublicMemberMapView { inner: local });
    let non_empty: Vec<_> = forwarded.into_iter().filter(|m| !m.is_empty()).collect();
    if non_empty.is_empty() {
        return public;
    }
    let mut maps = non_empty;
    maps.push(public);
    arena.alloc(MergedMapView::new(maps))
}

// ===========================================================================
// Helpers for module/shadowed.rs — work on &dyn MemberMap instead of &IndexMap
// ===========================================================================

/// Returns whether any of `map`'s keys are in `blocklist` — i.e. whether a
/// blocklist view would change anything.
///
/// Dart: `ShadowedModuleView._needsBlocklist` (`shadowed_view.dart:91-95`);
/// Go `needsBlocklist`.
pub fn needs_blocklist<V: Clone>(map: &dyn MemberMap<V>, blocklist: &HashSet<String>) -> bool {
    if blocklist.is_empty() || map.is_empty() {
        return false;
    }
    blocklist.iter().any(|k| map.has(k))
}

/// Keys in `full` but not in `filtered`. Used to re-derive a shadowed view's
/// blocklists after cloning the inner module's CSS.
///
/// Go `mapKeyDiff` (used by shadowed.rs clone).
pub fn map_key_diff<V: Clone>(
    full: &dyn MemberMap<V>,
    filtered: &dyn MemberMap<V>,
) -> HashSet<String> {
    let full_keys: HashSet<String> = full.keys().into_iter().collect();
    let filtered_keys: HashSet<String> = filtered.keys().into_iter().collect();
    full_keys.difference(&filtered_keys).cloned().collect()
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_map() -> IndexMap<String, i32> {
        let mut m = IndexMap::new();
        m.insert("a".into(), 1);
        m.insert("b".into(), 2);
        m.insert("c".into(), 3);
        m.insert("_private".into(), 99);
        m.insert("-dash".into(), 100);
        m
    }

    fn make_imv() -> IndexMapView<i32> {
        IndexMapView { map: make_map() }
    }

    fn make_v(arena: &Bump) -> &dyn MemberMap<i32> {
        arena.alloc(make_imv())
    }

    fn make_refcell_view<'a>(
        arena: &'a Bump,
        inner: &'a RefCell<IndexMap<String, i32>>,
    ) -> &'a dyn MemberMap<i32> {
        arena.alloc(RefCellMap { inner })
    }

    // ── IndexMapView ──

    #[rust_sass_macros::maybe_test]
    async fn test_safelist_follows_safelist_order() {
        // Dart `LimitedMapView.safelist` iterates the
        // safelist (`safelist.intersection(MapKeySet(_map))`), NOT the inner
        // map. Inner defines b-then-a; safelist lists a-then-b → keys must
        // come out a-then-b (the `meta.module-variables` order for
        // `@forward "m" show $a, $b`).
        let arena = Bump::new();
        let mut inner_map = IndexMap::new();
        inner_map.insert("b".to_string(), 2);
        inner_map.insert("a".to_string(), 1);
        let inner: &dyn MemberMap<i32> = arena.alloc(IndexMapView { map: inner_map });
        let order = vec!["a".to_string(), "b".to_string()];
        let limited: &dyn MemberMap<i32> =
            arena.alloc(LimitedMapView::new_safelist_ordered(inner, &order));
        assert_eq!(limited.keys(), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            limited.entries(),
            vec![("a".to_string(), 1), ("b".to_string(), 2)]
        );
    }
    #[test]
    fn test_index_map_view_get_existing() {
        let v = make_imv();
        assert_eq!(v.get("a"), Some(1));
        assert_eq!(v.get("b"), Some(2));
        assert_eq!(v.get("c"), Some(3));
    }

    #[test]
    fn test_index_map_view_get_missing() {
        let v = make_imv();
        assert_eq!(v.get("z"), None);
    }

    #[test]
    fn test_index_map_view_get_private() {
        let v = make_imv();
        assert_eq!(v.get("_private"), Some(99));
        assert_eq!(v.get("-dash"), Some(100));
    }

    #[test]
    fn test_index_map_view_has() {
        let v = make_imv();
        assert!(v.has("a"));
        assert!(!v.has("z"));
    }

    #[test]
    fn test_index_map_view_len() {
        assert_eq!(make_imv().len(), 5);
    }

    #[test]
    fn test_index_map_view_keys_preserves_order() {
        let v = make_imv();
        let keys = v.keys();
        assert_eq!(keys, vec!["a", "b", "c", "_private", "-dash"]);
    }

    #[test]
    fn test_index_map_view_entries() {
        let v = make_imv();
        let entries = v.entries();
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0], ("a".into(), 1));
        assert_eq!(entries[1], ("b".into(), 2));
    }

    // ── RefCellMap ──

    #[test]
    fn test_refcell_map_get() {
        let arena = Bump::new();
        let inner = RefCell::new(make_map());
        let v = make_refcell_view(&arena, &inner);
        assert_eq!(v.get("a"), Some(1));
    }

    #[test]
    fn test_refcell_map_mutation_visible() {
        let inner = RefCell::new(make_map());
        let v = RefCellMap { inner: &inner };
        assert_eq!(v.get("a"), Some(1));
        // Mutate through the RefCell
        inner.borrow_mut().insert("a".into(), 42);
        // View sees the new value (read-through)
        assert_eq!(v.get("a"), Some(42));
    }

    #[test]
    fn test_refcell_map_mutation_new_key_not_in_keys_snapshot() {
        // Keys snapshot at construction time would miss new keys.
        // But RefCellMap re-reads from RefCell each time.
        let inner = RefCell::new(make_map());
        let v = RefCellMap { inner: &inner };
        assert_eq!(v.len(), 5);
        inner.borrow_mut().insert("new".into(), 777);
        assert_eq!(v.get("new"), Some(777));
        assert_eq!(v.len(), 6);
    }

    // ── PrefixedMapView ──

    #[test]
    fn test_prefixed_map_view_get_with_prefix() {
        let arena = Bump::new();
        let inner = make_v(&arena);
        let prefixed: &dyn MemberMap<i32> = arena.alloc(PrefixedMapView {
            inner,
            prefix: "ns-".into(),
        });
        assert_eq!(prefixed.get("ns-a"), Some(1));
        assert_eq!(prefixed.get("ns-b"), Some(2));
    }

    #[test]
    fn test_prefixed_map_view_get_without_prefix_returns_none() {
        let arena = Bump::new();
        let inner = make_v(&arena);
        let prefixed: &dyn MemberMap<i32> = arena.alloc(PrefixedMapView {
            inner,
            prefix: "ns-".into(),
        });
        assert_eq!(prefixed.get("a"), None);
        assert_eq!(prefixed.get("b"), None);
    }

    #[test]
    fn test_prefixed_map_view_get_partial_prefix_returns_none() {
        let arena = Bump::new();
        let inner = make_v(&arena);
        let prefixed: &dyn MemberMap<i32> = arena.alloc(PrefixedMapView {
            inner,
            prefix: "ns-".into(),
        });
        assert_eq!(prefixed.get("n-a"), None);
    }

    #[test]
    fn test_prefixed_map_view_has() {
        let arena = Bump::new();
        let inner = make_v(&arena);
        let prefixed: &dyn MemberMap<i32> = arena.alloc(PrefixedMapView {
            inner,
            prefix: "ns-".into(),
        });
        assert!(prefixed.has("ns-a"));
        assert!(!prefixed.has("a"));
    }

    #[test]
    fn test_prefixed_map_view_len() {
        let arena = Bump::new();
        let inner = make_v(&arena);
        let prefixed: &dyn MemberMap<i32> = arena.alloc(PrefixedMapView {
            inner,
            prefix: "ns-".into(),
        });
        assert_eq!(prefixed.len(), 5);
    }

    #[test]
    fn test_prefixed_map_view_keys_have_prefix() {
        let arena = Bump::new();
        let inner = make_v(&arena);
        let prefixed: &dyn MemberMap<i32> = arena.alloc(PrefixedMapView {
            inner,
            prefix: "ns-".into(),
        });
        let keys = prefixed.keys();
        assert_eq!(
            keys,
            vec![
                "ns-a".to_string(),
                "ns-b".to_string(),
                "ns-c".to_string(),
                "ns-_private".to_string(),
                "ns--dash".to_string(),
            ]
        );
    }

    // ── LimitedMapView (safelist) ──

    #[test]
    fn test_limited_safelist_allows_safelisted_key() {
        let arena = Bump::new();
        let safelist: HashSet<String> = ["a".into(), "c".into()].into();
        let limited: &dyn MemberMap<i32> =
            arena.alloc(LimitedMapView::new_safelist(make_v(&arena), &safelist));
        assert_eq!(limited.get("a"), Some(1));
        assert_eq!(limited.get("c"), Some(3));
        assert_eq!(limited.get("b"), None);
    }

    #[test]
    fn test_limited_safelist_has() {
        let arena = Bump::new();
        let safelist: HashSet<String> = ["a".into()].into();
        let limited: &dyn MemberMap<i32> =
            arena.alloc(LimitedMapView::new_safelist(make_v(&arena), &safelist));
        assert!(limited.has("a"));
        assert!(!limited.has("b"));
    }

    #[test]
    fn test_limited_safelist_len() {
        let arena = Bump::new();
        let safelist: HashSet<String> = ["a".into(), "c".into()].into();
        let limited: &dyn MemberMap<i32> =
            arena.alloc(LimitedMapView::new_safelist(make_v(&arena), &safelist));
        assert_eq!(limited.len(), 2);
    }

    #[test]
    fn test_limited_safelist_live_value() {
        let arena = Bump::new();
        let inner_map = RefCell::new(make_map());
        let inner = make_refcell_view(&arena, &inner_map);
        let safelist: HashSet<String> = ["a".into()].into();
        let limited: &dyn MemberMap<i32> =
            arena.alloc(LimitedMapView::new_safelist(inner, &safelist));
        assert_eq!(limited.get("a"), Some(1));
        // Mutate the underlying value
        inner_map.borrow_mut().insert("a".into(), 99);
        // View sees the new value (live read)
        assert_eq!(limited.get("a"), Some(99));
    }

    #[test]
    fn test_limited_safelist_precomputed_keys_dont_see_new_keys() {
        let arena = Bump::new();
        let inner_map = RefCell::new(make_map());
        let inner = make_refcell_view(&arena, &inner_map);
        let safelist: HashSet<String> = ["a".into(), "new".into()].into();
        let limited: &dyn MemberMap<i32> =
            arena.alloc(LimitedMapView::new_safelist(inner, &safelist));
        assert_eq!(limited.len(), 1); // "a" exists, but "new" was not in inner at construction
                                      // Add "new" key to inner AFTER construction
        inner_map.borrow_mut().insert("new".into(), 777);
        // Can't see it — allowed was precomputed
        assert_eq!(limited.get("new"), None);
        assert_eq!(limited.len(), 1);
    }

    // ── LimitedMapView (blocklist) ──

    #[test]
    fn test_limited_blocklist_blocks_key() {
        let arena = Bump::new();
        let blocklist: HashSet<String> = ["b".into()].into();
        let limited: &dyn MemberMap<i32> =
            arena.alloc(LimitedMapView::new_blocklist(make_v(&arena), &blocklist));
        assert_eq!(limited.get("a"), Some(1));
        assert_eq!(limited.get("b"), None);
        assert_eq!(limited.get("c"), Some(3));
    }

    #[test]
    fn test_limited_blocklist_len() {
        let arena = Bump::new();
        let blocklist: HashSet<String> = ["b".into()].into();
        let limited: &dyn MemberMap<i32> =
            arena.alloc(LimitedMapView::new_blocklist(make_v(&arena), &blocklist));
        assert_eq!(limited.len(), 4); // 5 total - 1 blocked = 4
    }

    #[test]
    fn test_limited_empty_blocklist_passes_all() {
        let arena = Bump::new();
        let blocklist: HashSet<String> = HashSet::new();
        let limited: &dyn MemberMap<i32> =
            arena.alloc(LimitedMapView::new_blocklist(make_v(&arena), &blocklist));
        assert_eq!(limited.len(), 5);
        assert_eq!(limited.get("a"), Some(1));
    }

    // ── MergedMapView ──

    #[test]
    fn test_merged_later_map_wins() {
        let arena = Bump::new();
        let m1: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::from([("a".into(), 1), ("b".into(), 2)]),
        });
        let m2: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::from([("a".into(), 100), ("c".into(), 3)]),
        });
        let merged = MergedMapView::new(vec![m1, m2]);
        // m2's value for "a" wins (100, not 1)
        assert_eq!(merged.get("a"), Some(100));
        assert_eq!(merged.get("b"), Some(2));
        assert_eq!(merged.get("c"), Some(3));
    }

    #[test]
    fn test_merged_has() {
        let arena = Bump::new();
        let m1: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::from([("a".into(), 1)]),
        });
        let m2: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::from([("b".into(), 2)]),
        });
        let merged = MergedMapView::new(vec![m1, m2]);
        assert!(merged.has("a"));
        assert!(merged.has("b"));
        assert!(!merged.has("c"));
    }

    #[test]
    fn test_merged_len() {
        let arena = Bump::new();
        let m1: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::from([("a".into(), 1), ("b".into(), 2)]),
        });
        let m2: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::from([("a".into(), 3), ("c".into(), 4)]),
        });
        let merged = MergedMapView::new(vec![m1, m2]);
        assert_eq!(merged.len(), 3); // a, b, c
    }

    #[test]
    fn test_merged_live_value() {
        let arena = Bump::new();
        let inner_map = RefCell::new(IndexMap::from([("a".into(), 1)]));
        let m1 = make_refcell_view(&arena, &inner_map);
        let m2: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::new(),
        });
        let merged = MergedMapView::new(vec![m1, m2]);
        assert_eq!(merged.get("a"), Some(1));
        // Mutate underlying value
        inner_map.borrow_mut().insert("a".into(), 99);
        // View sees the new value
        assert_eq!(merged.get("a"), Some(99));
    }

    #[test]
    fn test_merged_keys_preserves_insertion_order() {
        let arena = Bump::new();
        let m1: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::from([("z".into(), 1), ("a".into(), 2)]),
        });
        let m2: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::from([("b".into(), 3)]),
        });
        let merged = MergedMapView::new(vec![m1, m2]);
        // m1 keys (z, a) then m2 keys (b)
        assert_eq!(merged.keys(), vec!["z", "a", "b"]);
    }

    // ── PublicMemberMapView ──

    #[test]
    fn test_public_member_hides_underscore() {
        let arena = Bump::new();
        let public: &dyn MemberMap<i32> = arena.alloc(PublicMemberMapView {
            inner: make_v(&arena),
        });
        assert_eq!(public.get("_private"), None);
        assert!(!public.has("_private"));
    }

    #[test]
    fn test_public_member_hides_dash() {
        let arena = Bump::new();
        let public: &dyn MemberMap<i32> = arena.alloc(PublicMemberMapView {
            inner: make_v(&arena),
        });
        assert_eq!(public.get("-dash"), None);
        assert!(!public.has("-dash"));
    }

    #[test]
    fn test_public_member_shows_normal_keys() {
        let arena = Bump::new();
        let public: &dyn MemberMap<i32> = arena.alloc(PublicMemberMapView {
            inner: make_v(&arena),
        });
        assert_eq!(public.get("a"), Some(1));
        assert_eq!(public.get("b"), Some(2));
    }

    #[test]
    fn test_public_member_len_not_o1() {
        let arena = Bump::new();
        let public: &dyn MemberMap<i32> = arena.alloc(PublicMemberMapView {
            inner: make_v(&arena),
        });
        assert_eq!(public.len(), 3); // a, b, c (not _private or -dash)
    }

    #[test]
    fn test_public_member_keys() {
        let arena = Bump::new();
        let public: &dyn MemberMap<i32> = arena.alloc(PublicMemberMapView {
            inner: make_v(&arena),
        });
        assert_eq!(public.keys(), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_public_member_empty_string_is_public() {
        let arena = Bump::new();
        let mut m = IndexMap::new();
        m.insert("".into(), 0);
        let inner: &dyn MemberMap<i32> = arena.alloc(IndexMapView { map: m });
        let public: &dyn MemberMap<i32> = arena.alloc(PublicMemberMapView { inner });
        assert_eq!(public.get(""), Some(0));
    }

    // ── forwarded_map ──

    #[test]
    fn test_forwarded_map_no_op() {
        let arena = Bump::new();
        let result = forwarded_map(&arena, make_v(&arena), None, None, None);
        assert_eq!(result.get("a"), Some(1));
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn test_forwarded_map_prefix_only() {
        let arena = Bump::new();
        let result = forwarded_map(&arena, make_v(&arena), Some("ns-"), None, None);
        assert_eq!(result.get("ns-a"), Some(1));
        assert_eq!(result.get("a"), None);
    }

    #[test]
    fn test_forwarded_map_prefix_and_safelist() {
        let arena = Bump::new();
        let safelist: HashSet<String> = ["ns-a".into(), "ns-c".into()].into();
        let result = forwarded_map(&arena, make_v(&arena), Some("ns-"), Some(&safelist), None);
        // Prefix applied first, then safelist filters on the PREFIXED keys
        assert_eq!(result.get("ns-a"), Some(1));
        assert_eq!(result.get("ns-b"), None);
        assert_eq!(result.get("ns-c"), Some(3));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_forwarded_map_prefix_and_blocklist() {
        let arena = Bump::new();
        let blocklist: HashSet<String> = ["ns-b".into()].into();
        let result = forwarded_map(&arena, make_v(&arena), Some("ns-"), None, Some(&blocklist));
        assert_eq!(result.get("ns-a"), Some(1));
        assert_eq!(result.get("ns-b"), None);
        assert_eq!(result.get("ns-c"), Some(3));
        assert_eq!(result.len(), 4); // 5 - 1 blocked
    }

    #[test]
    fn test_forwarded_map_empty_blocklist() {
        let arena = Bump::new();
        let blocklist: HashSet<String> = HashSet::new();
        let result = forwarded_map(&arena, make_v(&arena), Some("ns-"), None, Some(&blocklist));
        assert_eq!(result.len(), 5);
        assert_eq!(result.get("ns-a"), Some(1));
    }

    #[test]
    fn test_forwarded_map_chained_views_read_live() {
        let arena = Bump::new();
        let inner_map = RefCell::new(make_map());
        let inner = make_refcell_view(&arena, &inner_map);
        let result = forwarded_map(&arena, inner, Some("ns-"), None, None);
        assert_eq!(result.get("ns-a"), Some(1));
        // Mutate underlying value
        inner_map.borrow_mut().insert("a".into(), 99);
        // Prefixed view sees the new value (read-through chain: Prefixed → RefCellMap → RefCell)
        assert_eq!(result.get("ns-a"), Some(99));
    }

    // ── member_map ──

    #[test]
    fn test_member_map_local_only() {
        let arena = Bump::new();
        let local = make_v(&arena);
        let result = member_map(&arena, local, vec![]);
        // Wrapped in PublicMemberMapView — private hidden
        assert_eq!(result.get("a"), Some(1));
        assert_eq!(result.get("_private"), None);
        assert_eq!(result.get("-dash"), None);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_member_map_forwarded_and_local() {
        let arena = Bump::new();
        let fwd: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::from([("a".into(), 100), ("x".into(), 10)]),
        });
        let local: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::from([("a".into(), 1), ("b".into(), 2)]),
        });
        let result = member_map(&arena, local, vec![fwd]);
        // Local "a" shadows forwarded "a" (later = local wins)
        assert_eq!(result.get("a"), Some(1));
        // Forwarded "x" visible
        assert_eq!(result.get("x"), Some(10));
        // Local "b" visible
        assert_eq!(result.get("b"), Some(2));
        // Local private hidden by public view
        assert_eq!(result.len(), 3); // a, x, b
    }

    #[test]
    fn test_member_map_local_shadows_forwarded() {
        let arena = Bump::new();
        let fwd: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::from([("shared".into(), 999)]),
        });
        let local: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::from([("shared".into(), 1)]),
        });
        let result = member_map(&arena, local, vec![fwd]);
        // Local shadowed "shared" = 1 (not 999)
        assert_eq!(result.get("shared"), Some(1));
    }

    // ── needs_blocklist ──

    #[test]
    fn test_needs_blocklist_false_when_none_blocked() {
        let v = make_imv();
        let blocklist: HashSet<String> = ["z".into(), "w".into()].into();
        assert!(!needs_blocklist(&v, &blocklist));
    }

    #[test]
    fn test_needs_blocklist_true_when_blocked_exists() {
        let v = make_imv();
        let blocklist: HashSet<String> = ["b".into()].into();
        assert!(needs_blocklist(&v, &blocklist));
    }

    #[test]
    fn test_needs_blocklist_empty_blocklist() {
        let v = make_imv();
        assert!(!needs_blocklist(&v, &HashSet::new()));
    }

    #[test]
    fn test_needs_blocklist_empty_map() {
        let v: IndexMapView<i32> = IndexMapView {
            map: IndexMap::new(),
        };
        let blocklist: HashSet<String> = ["a".into()].into();
        assert!(!needs_blocklist(&v, &blocklist));
    }

    // ── map_key_diff ──

    #[test]
    fn test_map_key_diff_same_maps() {
        let m1 = IndexMapView {
            map: IndexMap::from([("a".into(), 1)]),
        };
        let m2 = IndexMapView {
            map: IndexMap::from([("a".into(), 2)]),
        };
        let diff = map_key_diff(&m1, &m2);
        assert!(diff.is_empty());
    }

    #[test]
    fn test_map_key_diff_extra_keys() {
        let full = IndexMapView {
            map: IndexMap::from([("a".into(), 1), ("b".into(), 2)]),
        };
        let filtered = IndexMapView {
            map: IndexMap::from([("a".into(), 3)]),
        };
        let diff = map_key_diff(&full, &filtered);
        let expected: HashSet<String> = ["b".into()].into();
        assert_eq!(diff, expected);
    }

    #[test]
    fn test_map_key_diff_filtered_has_more() {
        let full = IndexMapView {
            map: IndexMap::from([("a".into(), 1)]),
        };
        let filtered = IndexMapView {
            map: IndexMap::from([("a".into(), 3), ("b".into(), 4)]),
        };
        let diff = map_key_diff(&full, &filtered);
        assert!(diff.is_empty());
    }

    // ── Integration: full composition chain ──

    #[test]
    fn test_full_forwarded_then_merged_with_local() {
        // Simulates: @use "forwarded" as ns-* with show $a, then merge with local
        let arena = Bump::new();
        let inner = make_v(&arena); // a=1, b=2, c=3, _private=99, -dash=100

        // @forward as ns-* show $a
        let safelist: HashSet<String> = ["ns-a".into()].into();
        let forwarded = forwarded_map(&arena, inner, Some("ns-"), Some(&safelist), None);
        assert_eq!(forwarded.get("ns-a"), Some(1));
        assert_eq!(forwarded.get("ns-b"), None);

        // Merge with local (which has its own "a")
        let local: &dyn MemberMap<i32> = arena.alloc(IndexMapView {
            map: IndexMap::from([("local".into(), 42), ("ns-a".into(), 99)]),
        });
        let merged = member_map(&arena, local, vec![forwarded]);
        // "ns-a" from forwarded wins over local "ns-a"?
        // No — local is LAST in member_map, so local shadows forwarded
        assert_eq!(merged.get("ns-a"), Some(99));
        assert_eq!(merged.get("local"), Some(42));
        assert_eq!(merged.get("ns-b"), None);
    }
}
