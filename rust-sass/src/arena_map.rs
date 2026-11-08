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
use std::fmt;
use std::hash::{BuildHasher, Hash};
use std::ops::{Index, IndexMut};

use bumpalo::Bump;
use hashbrown::HashTable;

/// An arena-backed `Vec` (Dart `List<T>`).
pub type ArenaVec<'p, T> = allocator_api2::vec::Vec<T, &'p Bump>;

/// The cached hash makes removals and rehashes allocation-free of hashing.
struct Bucket<K, V> {
    hash: u64,
    key: K,
    value: V,
}

/// An insertion-ordered map backed by the compile arena.
pub struct ArenaMap<'parse, K, V, S = RandomState> {
    entries: ArenaVec<'parse, Bucket<K, V>>,
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

    /// Rebuilds the hash index from `entries`; used after in-place reordering.
    fn rebuild_indices(&mut self) {
        self.indices.clear();
        let entries = &self.entries;
        for (index, bucket) in entries.iter().enumerate() {
            self.indices
                .insert_unique(bucket.hash, index, |&stored| entries[stored].hash);
        }
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

    /// Removes `key`, moving the last entry into its place (O(1) bookkeeping,
    /// order changes), returning its value.
    pub fn swap_remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.swap_remove_full(key).map(|(_, _, value)| value)
    }

    /// Like [`swap_remove`](Self::swap_remove), also returning the removed
    /// entry's index and the key it had.
    pub fn swap_remove_full<Q>(&mut self, key: &Q) -> Option<(usize, K, V)>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = self.hash(key);
        let index = self.take_index(key, hash)?;
        let last = self.entries.len() - 1;
        let bucket = self.entries.swap_remove(index);
        if index != last {
            let moved_hash = self.entries[index].hash;
            if let Some(moved) = self.indices.find_mut(moved_hash, |&stored| stored == last) {
                *moved = index;
            }
        }
        Some((index, bucket.key, bucket.value))
    }

    /// Removes `key`, preserving the order of the remaining entries (O(n)),
    /// returning its value.
    pub fn shift_remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.shift_remove_full(key).map(|(_, _, value)| value)
    }

    /// Like [`shift_remove`](Self::shift_remove), also returning the removed
    /// entry's index and the key it had.
    pub fn shift_remove_full<Q>(&mut self, key: &Q) -> Option<(usize, K, V)>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = self.hash(key);
        let index = self.take_index(key, hash)?;
        let bucket = self.entries.remove(index);
        for position in index..self.entries.len() {
            let hash = self.entries[position].hash;
            if let Some(moved) = self
                .indices
                .find_mut(hash, |&stored| stored == position + 1)
            {
                *moved = position;
            }
        }
        Some((index, bucket.key, bucket.value))
    }

    /// Keeps only the entries for which `keep` returns `true`, preserving
    /// their order.
    pub fn retain<F>(&mut self, mut keep: F)
    where
        F: FnMut(&K, &mut V) -> bool,
    {
        self.entries
            .retain_mut(|bucket| keep(&bucket.key, &mut bucket.value));
        self.rebuild_indices();
    }

    /// Removes `key` from the hash index, returning its entry index.
    fn take_index<Q>(&mut self, key: &Q, hash: u64) -> Option<usize>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let entry = self
            .indices
            .find_entry(hash, |&index| self.entries[index].key.borrow() == key)
            .ok()?;
        Some(entry.remove().0)
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

/// A view into one map entry, which may be occupied or vacant.
pub enum Entry<'a, 'parse, K, V, S> {
    Occupied(OccupiedEntry<'a, 'parse, K, V, S>),
    Vacant(VacantEntry<'a, 'parse, K, V, S>),
}

impl<'a, 'parse, K, V, S> Entry<'a, 'parse, K, V, S> {
    /// The key this entry is for.
    pub fn key(&self) -> &K {
        match self {
            Entry::Occupied(entry) => entry.key(),
            Entry::Vacant(entry) => entry.key(),
        }
    }

    /// Returns the entry's value, inserting `default` first if vacant.
    pub fn or_insert(self, default: V) -> &'a mut V {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(default),
        }
    }

    /// Returns the entry's value, inserting `default()` first if vacant.
    pub fn or_insert_with<F: FnOnce() -> V>(self, default: F) -> &'a mut V {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(default()),
        }
    }

    /// Returns the entry's value, inserting `V::default()` first if vacant.
    pub fn or_default(self) -> &'a mut V
    where
        V: Default,
    {
        self.or_insert_with(V::default)
    }

    /// Runs `f` on the value if the entry is occupied.
    pub fn and_modify<F: FnOnce(&mut V)>(mut self, f: F) -> Self {
        if let Entry::Occupied(entry) = &mut self {
            f(entry.get_mut());
        }
        self
    }
}

/// An occupied [`Entry`].
pub struct OccupiedEntry<'a, 'parse, K, V, S> {
    map: &'a mut ArenaMap<'parse, K, V, S>,
    index: usize,
}

impl<'a, 'parse, K, V, S> OccupiedEntry<'a, 'parse, K, V, S> {
    pub fn key(&self) -> &K {
        &self.map.entries[self.index].key
    }

    pub fn get(&self) -> &V {
        &self.map.entries[self.index].value
    }

    pub fn get_mut(&mut self) -> &mut V {
        &mut self.map.entries[self.index].value
    }

    pub fn into_mut(self) -> &'a mut V {
        let Self { map, index } = self;
        &mut map.entries[index].value
    }

    pub fn insert(&mut self, value: V) -> V {
        std::mem::replace(&mut self.map.entries[self.index].value, value)
    }

    pub fn index(&self) -> usize {
        self.index
    }
}

/// A vacant [`Entry`].
pub struct VacantEntry<'a, 'parse, K, V, S> {
    map: &'a mut ArenaMap<'parse, K, V, S>,
    key: K,
    hash: u64,
}

impl<'a, 'parse, K, V, S> VacantEntry<'a, 'parse, K, V, S> {
    pub fn key(&self) -> &K {
        &self.key
    }

    pub fn into_key(self) -> K {
        self.key
    }

    /// The index the inserted entry will have.
    pub fn index(&self) -> usize {
        self.map.entries.len()
    }

    pub fn insert(self, value: V) -> &'a mut V {
        let Self { map, key, hash } = self;
        let index = map.entries.len();
        map.entries.push(Bucket { hash, key, value });
        let entries = &map.entries;
        map.indices
            .insert_unique(hash, index, |&stored| entries[stored].hash);
        &mut map.entries[index].value
    }
}

impl<'parse, K, V, S> ArenaMap<'parse, K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    /// Returns a view into the entry for `key`.
    pub fn entry(&mut self, key: K) -> Entry<'_, 'parse, K, V, S> {
        let hash = self.hash(&key);
        let found = self
            .indices
            .find(hash, |&index| self.entries[index].key == key)
            .copied();
        match found {
            Some(index) => Entry::Occupied(OccupiedEntry { map: self, index }),
            None => Entry::Vacant(VacantEntry {
                map: self,
                key,
                hash,
            }),
        }
    }
}

impl<K, V, S> Clone for ArenaMap<'_, K, V, S>
where
    K: Clone,
    V: Clone,
    S: Clone,
{
    /// Clones the map into the **same** arena. The copy is retained until the
    /// compilation ends, so do not clone in a per-operation hot path.
    fn clone(&self) -> Self {
        let mut map = Self::with_capacity_and_hasher_in(
            self.len(),
            self.hasher.clone(),
            self.entries.allocator(),
        );
        for bucket in self.entries.iter() {
            map.entries.push(Bucket {
                hash: bucket.hash,
                key: bucket.key.clone(),
                value: bucket.value.clone(),
            });
        }
        map.rebuild_indices();
        map
    }
}

impl<K: fmt::Debug, V: fmt::Debug, S> fmt::Debug for ArenaMap<'_, K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K, V, S> PartialEq for ArenaMap<'_, K, V, S>
where
    K: Hash + Eq,
    V: PartialEq,
    S: BuildHasher,
{
    /// Equality is order-insensitive, matching `IndexMap`.
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|(key, value)| other.get(key) == Some(value))
    }
}

impl<K, V, S> Eq for ArenaMap<'_, K, V, S>
where
    K: Hash + Eq,
    V: Eq,
    S: BuildHasher,
{
}

impl<K, V, S> Extend<(K, V)> for ArenaMap<'_, K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (key, value) in iter {
            self.insert(key, value);
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

    #[test]
    fn entry_api() {
        let arena = Bump::new();
        let mut map: ArenaMap<'_, &str, i32> = ArenaMap::new_in(&arena);

        *map.entry("a").or_default() += 1;
        *map.entry("a").or_insert(5) += 1;
        map.entry("b").or_insert_with(|| 7);
        map.entry("a").and_modify(|value| *value *= 2);
        assert_eq!(map.get("a"), Some(&4));
        assert_eq!(map.get("b"), Some(&7));

        match map.entry("c") {
            Entry::Vacant(entry) => {
                assert_eq!(entry.key(), &"c");
                let index = entry.index();
                assert_eq!(index, 2);
                entry.insert(9);
            }
            Entry::Occupied(_) => panic!("expected a vacant entry"),
        }
        assert_eq!(map.get("c"), Some(&9));

        match map.entry("c") {
            Entry::Occupied(mut entry) => {
                assert_eq!(entry.key(), &"c");
                assert_eq!(entry.get(), &9);
                *entry.get_mut() += 1;
                assert_eq!(entry.insert(11), 10);
                assert_eq!(entry.index(), 2);
            }
            Entry::Vacant(_) => panic!("expected an occupied entry"),
        }
        assert_eq!(map.get("c"), Some(&11));
    }

    #[test]
    fn removals_preserve_or_reorder_as_documented() {
        let arena = Bump::new();
        let mut map: ArenaMap<'_, &str, i32> = ArenaMap::new_in(&arena);
        for (index, key) in ["a", "b", "c", "d"].into_iter().enumerate() {
            map.insert(key, index as i32);
        }

        // swap_remove moves the last entry into the hole.
        assert_eq!(map.swap_remove("b"), Some(1));
        let order: Vec<_> = map.keys().copied().collect();
        assert_eq!(order, ["a", "d", "c"]);
        assert_eq!(map.get("d"), Some(&3));
        assert_eq!(map.len(), 3);

        // shift_remove keeps the order.
        assert_eq!(map.shift_remove_full("a"), Some((0, "a", 0)));
        let order: Vec<_> = map.keys().copied().collect();
        assert_eq!(order, ["d", "c"]);
        assert_eq!(map.get("c"), Some(&2));

        assert_eq!(map.shift_remove("missing"), None);
        assert_eq!(map.swap_remove("missing"), None);
    }

    #[test]
    fn retain_keeps_order_and_lookups() {
        let arena = Bump::new();
        let mut map: ArenaMap<'_, i32, i32> = ArenaMap::new_in(&arena);
        for key in 0..6 {
            map.insert(key, key * 10);
        }

        map.retain(|key, value| {
            *value += 1;
            key % 2 == 0
        });

        let entries: Vec<_> = map.iter().map(|(key, value)| (*key, *value)).collect();
        assert_eq!(entries, [(0, 1), (2, 21), (4, 41)]);
        assert_eq!(map.get(&2), Some(&21));
        assert_eq!(map.get(&1), None);
    }

    #[test]
    fn clone_into_same_arena() {
        let arena = Bump::new();
        let mut map: ArenaMap<'_, &str, i32> = ArenaMap::new_in(&arena);
        map.insert("a", 1);
        map.insert("b", 2);

        let clone = map.clone();
        assert_eq!(clone, map);
        assert_eq!(clone.len(), 2);
        let order: Vec<_> = clone.iter().map(|(key, value)| (*key, *value)).collect();
        assert_eq!(order, [("a", 1), ("b", 2)]);

        // The clone has its own storage: mutating the original does not show.
        map.insert("a", 10);
        assert_eq!(clone.get("a"), Some(&1));
    }

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
    }

    #[test]
    fn differential_against_indexmap() {
        use indexmap::IndexMap;

        let arena = Bump::new();
        for seed in [1, 7, 42] {
            let mut ours: ArenaMap<'_, u32, u32> = ArenaMap::new_in(&arena);
            let mut theirs: IndexMap<u32, u32> = IndexMap::new();
            let mut rng = Lcg(seed);

            for op in 0..2_000 {
                let key = (rng.next() % 32) as u32;
                match rng.next() % 8 {
                    0..=2 => {
                        let value = rng.next() as u32;
                        assert_eq!(
                            ours.insert(key, value),
                            theirs.insert(key, value),
                            "seed {seed}, op {op}: insert"
                        );
                    }
                    3 => {
                        assert_eq!(
                            ours.get(&key),
                            theirs.get(&key),
                            "seed {seed}, op {op}: get"
                        );
                    }
                    4 => {
                        assert_eq!(
                            ours.swap_remove(&key),
                            theirs.swap_remove(&key),
                            "seed {seed}, op {op}: swap_remove"
                        );
                    }
                    5 => {
                        assert_eq!(
                            ours.shift_remove(&key),
                            theirs.shift_remove(&key),
                            "seed {seed}, op {op}: shift_remove"
                        );
                    }
                    6 => {
                        let modulus = (rng.next() % 3) as u32;
                        ours.retain(|candidate, _| candidate % 3 != modulus);
                        theirs.retain(|candidate, _| candidate % 3 != modulus);
                        assert_eq!(ours.len(), theirs.len(), "seed {seed}, op {op}: retain");
                    }
                    _ => {
                        let value = rng.next() as u32;
                        *ours.entry(key).or_insert(0) = value;
                        *theirs.entry(key).or_insert(0) = value;
                    }
                }

                assert_eq!(ours.len(), theirs.len(), "seed {seed}, op {op}: len");
                let ours_entries: Vec<_> = ours.iter().map(|(key, value)| (*key, *value)).collect();
                let theirs_entries: Vec<_> =
                    theirs.iter().map(|(key, value)| (*key, *value)).collect();
                assert_eq!(
                    ours_entries, theirs_entries,
                    "seed {seed}, op {op}: order/values"
                );

                if op % 97 == 0 {
                    let clone = ours.clone();
                    let clone_entries: Vec<_> =
                        clone.iter().map(|(key, value)| (*key, *value)).collect();
                    assert_eq!(clone_entries, theirs_entries, "seed {seed}, op {op}: clone");
                }
            }
        }
    }
}
