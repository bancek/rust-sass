// dart-source: N/A (infrastructure: arena-backed ordered map, mirrors the IndexMap API the port uses)
// go-source: N/A (infrastructure)

//! An insertion-ordered hash map whose storage lives in the compile arena.
//!
//! The layout follows `indexmap`'s `IndexMapCore` (dense entries plus a hash
//! index, MIT/Apache-2.0); this is an independent implementation written here
//! because `indexmap::IndexMap` has no allocator hook. The port stores maps
//! inside arena-allocated values (`EnvironmentInner`, `SassMap`,
//! `ExtensionStore`, …).
//! `bumpalo::Bump` never runs destructors, so an `IndexMap` there leaks both its
//! buffers and (for `String` keys) the keys. This type uses the same internal
//! shape as indexmap — a dense `Vec` of entries plus a hash index — but both
//! allocations come from `A = &'parse Bump`, so nothing owns heap memory and no
//! `Drop` is required: the arena frees everything when the compilation ends.
//!
//! Lookups and insertion are O(1) average. `swap_remove` is O(1) and
//! `shift_remove` is O(n), matching `IndexMap`'s semantics.
//!
//! **Clone is not free:** `Clone` allocates into the same arena and is retained
//! until the compilation ends. Fields that are cloned per operation must hold a
//! handle (`&'parse ArenaMap`) rather than an owned map.
//!
//! See `docs/architecture.md`.

use std::borrow::Borrow;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};
use std::ops::{Index, IndexMut};

use allocator_api2::vec::Vec as ArenaVec;
use bumpalo::Bump;
use hashbrown::HashTable;

/// The cached hash makes removals and rehashes allocation-free of hashing.
struct Bucket<K, V> {
    hash: u64,
    key: K,
    value: V,
}

/// An insertion-ordered map backed by the compile arena.
pub struct ArenaMap<'parse, K, V, S = RandomState> {
    entries: ArenaVec<Bucket<K, V>, &'parse Bump>,
    /// Maps a hash to its entry's index in `entries`.
    indices: HashTable<usize, &'parse Bump>,
    hasher: S,
}

impl<'parse, K, V> ArenaMap<'parse, K, V, RandomState> {
    /// Creates an empty map allocated in `arena`.
    pub fn new_in(arena: &'parse Bump) -> Self {
        Self::with_hasher_in(RandomState::new(), arena)
    }

    /// Creates an empty map with room for `capacity` entries.
    pub fn with_capacity_in(capacity: usize, arena: &'parse Bump) -> Self {
        Self::with_capacity_and_hasher_in(capacity, RandomState::new(), arena)
    }
}

impl<'parse, K, V, S> ArenaMap<'parse, K, V, S> {
    /// Creates an empty map with the given hasher builder.
    pub fn with_hasher_in(hasher: S, arena: &'parse Bump) -> Self {
        Self {
            entries: ArenaVec::new_in(arena),
            indices: HashTable::new_in(arena),
            hasher,
        }
    }

    /// Creates an empty map with room for `capacity` entries and the given
    /// hasher builder.
    pub fn with_capacity_and_hasher_in(capacity: usize, hasher: S, arena: &'parse Bump) -> Self {
        Self {
            entries: ArenaVec::with_capacity_in(capacity, arena),
            indices: HashTable::with_capacity_in(capacity, arena),
            hasher,
        }
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes all entries. The arena storage is not reclaimed.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.indices.clear();
    }

    /// Iterates entries in insertion order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&K, &V)> + '_ {
        self.entries
            .iter()
            .map(|bucket| (&bucket.key, &bucket.value))
    }

    /// Iterates entries in insertion order, with mutable values.
    pub fn iter_mut(&mut self) -> impl ExactSizeIterator<Item = (&K, &mut V)> + '_ {
        self.entries
            .iter_mut()
            .map(|bucket| (&bucket.key, &mut bucket.value))
    }

    /// Iterates keys in insertion order.
    pub fn keys(&self) -> impl ExactSizeIterator<Item = &K> + '_ {
        self.entries.iter().map(|bucket| &bucket.key)
    }

    /// Iterates values in insertion order.
    pub fn values(&self) -> impl ExactSizeIterator<Item = &V> + '_ {
        self.entries.iter().map(|bucket| &bucket.value)
    }

    /// Iterates values mutably in insertion order.
    pub fn values_mut(&mut self) -> impl ExactSizeIterator<Item = &mut V> + '_ {
        self.entries.iter_mut().map(|bucket| &mut bucket.value)
    }

    fn hash<Q>(&self, key: &Q) -> u64
    where
        S: BuildHasher,
        Q: Hash + ?Sized,
    {
        self.hasher.hash_one(key)
    }
}

impl<'parse, K, V, S> ArenaMap<'parse, K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    fn get_index_of<Q>(&self, key: &Q) -> Option<usize>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = self.hash(key);
        self.indices
            .find(hash, |&index| self.entries[index].key.borrow() == key)
            .copied()
    }

    /// Returns the value for `key`.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.get_index_of(key)
            .map(|index| &self.entries[index].value)
    }

    /// Returns the value for `key` mutably.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let index = self.get_index_of(key)?;
        Some(&mut self.entries[index].value)
    }

    /// Returns the key and value for `key`.
    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.get_index_of(key)
            .map(|index| (&self.entries[index].key, &self.entries[index].value))
    }

    /// Returns whether `key` is present.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.get_index_of(key).is_some()
    }

    /// Inserts `key → value`, returning the previous value for `key` if any.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.insert_full(key, value).1
    }

    /// Inserts `key → value`, returning the entry's index and the previous
    /// value for `key` if any.
    pub fn insert_full(&mut self, key: K, value: V) -> (usize, Option<V>) {
        let hash = self.hash(&key);
        if let Some(&index) = self
            .indices
            .find(hash, |&index| self.entries[index].key == key)
        {
            let previous = std::mem::replace(&mut self.entries[index].value, value);
            return (index, Some(previous));
        }

        let index = self.entries.len();
        self.entries.push(Bucket { hash, key, value });
        // Rehashing reads the cached hash back out of `entries`; the closure
        // does not run while the map is otherwise mutably borrowed.
        let entries = &self.entries;
        self.indices
            .insert_unique(hash, index, |&stored| entries[stored].hash);
        (index, None)
    }
}

impl<'parse, K, V, S, Q> Index<&Q> for ArenaMap<'parse, K, V, S>
where
    K: Borrow<Q> + Hash + Eq,
    Q: Hash + Eq + ?Sized,
    S: BuildHasher,
{
    type Output = V;

    fn index(&self, key: &Q) -> &V {
        match self.get(key) {
            Some(value) => value,
            None => panic!("ArenaMap: no entry found for key"),
        }
    }
}

impl<'parse, K, V, S, Q> IndexMut<&Q> for ArenaMap<'parse, K, V, S>
where
    K: Borrow<Q> + Hash + Eq,
    Q: Hash + Eq + ?Sized,
    S: BuildHasher,
{
    fn index_mut(&mut self, key: &Q) -> &mut V {
        match self.get_mut(key) {
            Some(value) => value,
            None => panic!("ArenaMap: no entry found for key"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::Hasher;

    #[test]
    fn insert_get_and_order() {
        let arena = Bump::new();
        let mut map: ArenaMap<'_, &str, i32> = ArenaMap::new_in(&arena);

        assert!(map.is_empty());
        assert_eq!(map.insert("a", 1), None);
        assert_eq!(map.insert("b", 2), None);
        assert_eq!(map.insert("a", 10), Some(1));
        assert_eq!(map.len(), 2);
        assert!(!map.is_empty());

        let order: Vec<_> = map.iter().map(|(key, value)| (*key, *value)).collect();
        assert_eq!(order, [("a", 10), ("b", 2)]);

        assert_eq!(map.get("a"), Some(&10));
        assert_eq!(map.get("missing"), None);
        assert!(map.contains_key("b"));
        assert!(!map.contains_key("missing"));
        assert_eq!(map.get_key_value("b"), Some((&"b", &2)));
        assert_eq!(map["a"], 10);
    }

    #[test]
    fn insert_full_reports_index() {
        let arena = Bump::new();
        let mut map: ArenaMap<'_, &str, i32> = ArenaMap::new_in(&arena);

        assert_eq!(map.insert_full("a", 1), (0, None));
        assert_eq!(map.insert_full("b", 2), (1, None));
        assert_eq!(map.insert_full("a", 3), (0, Some(1)));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn get_mut_and_mutable_iterators() {
        let arena = Bump::new();
        let mut map: ArenaMap<'_, &str, i32> = ArenaMap::new_in(&arena);
        map.insert("a", 1);
        map.insert("b", 2);

        *map.get_mut("a").unwrap() = 10;
        for value in map.values_mut() {
            *value += 1;
        }
        for (_, value) in map.iter_mut() {
            *value += 1;
        }

        let values: Vec<_> = map.values().copied().collect();
        assert_eq!(values, [12, 4]);
    }

    #[test]
    fn string_keys_borrow_as_str() {
        let arena = Bump::new();
        let mut map: ArenaMap<'_, String, i32> = ArenaMap::new_in(&arena);
        map.insert("a".to_string(), 1);

        assert_eq!(map.get("a"), Some(&1));
        assert_eq!(map.get(&"a".to_string()), Some(&1));
        assert!(map.contains_key("a"));
    }

    #[test]
    fn clear_and_len() {
        let arena = Bump::new();
        let mut map: ArenaMap<'_, &str, i32> = ArenaMap::with_capacity_in(8, &arena);
        map.insert("a", 1);
        map.insert("b", 2);
        assert_eq!(map.len(), 2);

        map.clear();
        assert!(map.is_empty());
        assert_eq!(map.get("a"), None);

        // The map is still usable after clearing.
        map.insert("c", 3);
        assert_eq!(map.get("c"), Some(&3));
    }

    #[test]
    fn hash_collisions_distinguish_keys() {
        #[derive(Clone, Default)]
        struct ConstantHasher;

        struct ConstantHash;

        impl Hasher for ConstantHash {
            fn finish(&self) -> u64 {
                7
            }

            fn write(&mut self, _bytes: &[u8]) {}
        }

        impl BuildHasher for ConstantHasher {
            type Hasher = ConstantHash;

            fn build_hasher(&self) -> Self::Hasher {
                ConstantHash
            }
        }

        let arena = Bump::new();
        let mut map: ArenaMap<'_, &str, i32, ConstantHasher> =
            ArenaMap::with_hasher_in(ConstantHasher, &arena);

        map.insert("a", 1);
        map.insert("b", 2);
        map.insert("c", 3);

        // Same hash for every key: the equality filter, not the hash, resolves.
        assert_eq!(map.get("a"), Some(&1));
        assert_eq!(map.get("b"), Some(&2));
        assert_eq!(map.get("c"), Some(&3));
        assert_eq!(map.len(), 3);
        let order: Vec<_> = map.keys().copied().collect();
        assert_eq!(order, ["a", "b", "c"]);
    }
}
