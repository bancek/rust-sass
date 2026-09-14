// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/map.dart
// go-source: go/value/map.go

use crate::value::hash::list_hash;
use crate::value::hash::map_hash;
use crate::value::ListSeparator;
use crate::value::SassList;
use bumpalo::Bump;
use indexmap::IndexMap;

use crate::common::SassResult;
use crate::serialize::SerializeVisitor;
use crate::value::{Value, ValueKind};

use crate::value::ValueVisitor;

/// A SassScript map.
#[derive(Clone, Debug)]
pub struct SassMap<'parse> {
    /// The contents of the map.
    pub entries: IndexMap<Value<'parse>, Value<'parse>>,
}

impl<'parse> SassMap<'parse> {
    /// Returns an empty map.
    pub fn empty() -> Self {
        SassMap {
            entries: IndexMap::new(),
        }
    }

    /// Returns a valid CSS representation of `self`.
    ///
    /// Use [`to_display_string`](Self::to_display_string) instead to get a
    /// string representation even if this isn't valid CSS (maps never are).
    ///
    /// If `quote` is `false`, quoted strings are emitted without quotes.
    pub fn to_css_string(&self, quote: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(quote, false);
        visitor.visit_map(self)?;
        Ok(visitor.into_string())
    }

    /// Returns a string representation of `self`.
    ///
    /// Note that this is equivalent to calling `inspect()` on the value, and
    /// thus won't reflect the user's output settings.
    /// [`to_css_string`](Self::to_css_string) should be used instead to
    /// convert `self` to CSS.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(true, true);
        visitor.visit_map(self)?;
        Ok(visitor.into_string())
    }

    /// Creates a map from `iter`, keeping the first value when a key
    /// appears more than once.
    pub fn from_entries(iter: impl IntoIterator<Item = (Value<'parse>, Value<'parse>)>) -> Self {
        let mut m = SassMap::empty();
        for (k, v) in iter {
            if !m.entries.contains_key(&k) {
                m.entries.insert(k, v);
            }
        }
        m
    }

    /// Returns the number of entries in the map.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when the map has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the value associated with `key`, if any.
    pub fn get(&self, key: &Value<'parse>) -> Option<&Value<'parse>> {
        self.entries.get(key)
    }

    /// Returns `true` when `key` is present in the map.
    pub fn contains(&self, key: &Value<'parse>) -> bool {
        self.entries.contains_key(key)
    }

    /// Inserts `val` under `key`, replacing any previous value.
    pub fn set(&mut self, key: Value<'parse>, val: Value<'parse>) {
        self.entries.insert(key, val);
    }

    /// Removes `key` and its value from the map, if present.
    pub fn delete(&mut self, key: &Value<'parse>) {
        self.entries.shift_remove(key);
    }

    /// Returns a copy of `self` with its own entry table.
    pub fn copy(&self) -> Self {
        SassMap {
            entries: self.entries.clone(),
        }
    }

    /// This value as a list: one space-separated key/value pair per entry.
    ///
    /// All SassScript values can be used as lists; maps count as lists of
    /// pairs.
    pub fn as_list<'compile: 'parse>(&self, arena: &'compile Bump) -> Vec<Value<'parse>> {
        self.entries
            .iter()
            .map(|(k, v)| {
                let pair = SassList::new(vec![*k, *v], ListSeparator::Space, false);
                Value::new_with_arena(arena, ValueKind::List(pair))
            })
            .collect()
    }

    /// Returns the hash code for this map. An empty map hashes like an
    /// empty list; otherwise the combine is order-independent so equal
    /// maps with different insertion orders hash equally.
    pub fn hash_code(&self) -> i32 {
        if self.entries.is_empty() {
            return list_hash(&[]);
        }
        // Dart `mapHash` is unordered (package:collection MapEquality): equal
        // maps with different insertion orders must hash equally, so the
        // combine must be order-independent (summed, not iterated).
        let flat: Vec<(i32, i32)> = self
            .entries
            .iter()
            .map(|(k, v)| (k.hash_code(), v.hash_code()))
            .collect();
        map_hash(&flat)
    }

    /// Compares this map to `other` entry-wise, ignoring insertion order.
    /// An empty map additionally equals an empty list.
    pub fn equals(&self, other: &SassMap<'parse>) -> bool {
        if self.entries.len() != other.entries.len() {
            return false;
        }
        for (k, v) in &self.entries {
            match other.entries.get(k) {
                Some(ov) if v == ov => continue,
                _ => return false,
            }
        }
        true
    }

    /// The separator for this value as a list: comma for a non-empty map,
    /// undecided for an empty one.
    ///
    /// All SassScript values can be used as lists; maps count as lists of
    /// pairs.
    pub fn separator(&self) -> ListSeparator {
        if self.entries.is_empty() {
            ListSeparator::Undecided
        } else {
            ListSeparator::Comma
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::ListSeparator;
    use crate::value::SassList;
    use crate::value::SassString;
    use crate::value::{Value, ValueKind};
    use bumpalo::Bump;

    fn string_val<'compile: 'parse, 'parse>(arena: &'compile Bump, s: &str) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(s), true)),
        )
    }

    #[test]
    fn test_map_empty() {
        let m = SassMap::empty();
        assert_eq!(m.len(), 0);
        assert!(m.is_empty());
    }

    #[test]
    fn test_map_get() {
        let arena = Bump::new();
        let m = SassMap::from_entries(vec![(
            ValueKind::unitless_number(&arena, 1.0),
            ValueKind::unitless_number(&arena, 42.0),
        )]);
        let v = m.get(&ValueKind::unitless_number(&arena, 1.0));
        assert!(v.is_some());
        assert!(m.get(&ValueKind::unitless_number(&arena, 99.0)).is_none());
    }

    #[test]
    fn test_map_set() {
        let arena = Bump::new();
        let mut m = SassMap::empty();
        m.set(
            ValueKind::unitless_number(&arena, 1.0),
            ValueKind::unitless_number(&arena, 10.0),
        );
        assert_eq!(m.len(), 1);
        assert!(m.contains(&ValueKind::unitless_number(&arena, 1.0)));
        m.set(
            ValueKind::unitless_number(&arena, 1.0),
            ValueKind::unitless_number(&arena, 20.0),
        );
        assert_eq!(m.len(), 1);
        let v = m.get(&ValueKind::unitless_number(&arena, 1.0)).unwrap();
        assert_eq!(*v, ValueKind::unitless_number(&arena, 20.0));
    }

    #[test]
    fn test_map_delete() {
        let arena = Bump::new();
        let mut m = SassMap::from_entries(vec![(
            ValueKind::unitless_number(&arena, 1.0),
            ValueKind::unitless_number(&arena, 10.0),
        )]);
        m.delete(&ValueKind::unitless_number(&arena, 1.0));
        assert_eq!(m.len(), 0);
        assert!(!m.contains(&ValueKind::unitless_number(&arena, 1.0)));
    }

    #[test]
    fn test_map_copy() {
        let arena = Bump::new();
        let m = SassMap::from_entries(vec![(
            ValueKind::unitless_number(&arena, 1.0),
            ValueKind::unitless_number(&arena, 10.0),
        )]);
        let mut c = m.copy();
        c.set(
            ValueKind::unitless_number(&arena, 1.0),
            ValueKind::unitless_number(&arena, 99.0),
        );
        let v = m.get(&ValueKind::unitless_number(&arena, 1.0)).unwrap();
        assert_eq!(
            *v,
            ValueKind::unitless_number(&arena, 10.0),
            "original should not be affected"
        );
    }

    #[test]
    fn test_map_as_list() {
        let arena = Bump::new();
        let bump = Bump::new();
        let m = SassMap::from_entries(vec![(
            string_val(&bump, "a"),
            ValueKind::unitless_number(&arena, 1.0),
        )]);
        let pairs = m.as_list(&arena);
        assert_eq!(pairs.len(), 1);
    }

    #[test]
    fn test_map_equals() {
        let arena = Bump::new();
        let m1 = SassMap::from_entries(vec![(
            ValueKind::unitless_number(&arena, 1.0),
            ValueKind::unitless_number(&arena, 10.0),
        )]);
        let m2 = SassMap::from_entries(vec![(
            ValueKind::unitless_number(&arena, 1.0),
            ValueKind::unitless_number(&arena, 10.0),
        )]);
        assert!(m1.equals(&m2));
        let m3 = SassMap::from_entries(vec![(
            ValueKind::unitless_number(&arena, 2.0),
            ValueKind::unitless_number(&arena, 10.0),
        )]);
        assert!(!m1.equals(&m3));
    }

    #[test]
    fn test_map_equal_empty_vs_list() {
        let arena = Bump::new();
        let m = SassMap::empty();
        let l = SassList::empty(ListSeparator::Space, false);
        let vm = Value::new_with_arena(&arena, ValueKind::Map(m));
        let vl = Value::new_with_arena(&arena, ValueKind::List(l));
        assert_eq!(vm, vl);
        assert_eq!(vl, vm);
    }

    #[test]
    fn test_map_hash_code() {
        let arena = Bump::new();
        let m1 = SassMap::from_entries(vec![(
            ValueKind::unitless_number(&arena, 1.0),
            ValueKind::unitless_number(&arena, 10.0),
        )]);
        let m2 = SassMap::from_entries(vec![(
            ValueKind::unitless_number(&arena, 1.0),
            ValueKind::unitless_number(&arena, 10.0),
        )]);
        assert_eq!(m1.hash_code(), m2.hash_code());
    }

    #[test]
    fn test_map_hash_empty_matches_empty_list() {
        let m = SassMap::empty();
        let l = SassList::empty(ListSeparator::Space, false);
        assert_eq!(m.hash_code(), l.hash_code());
    }

    #[test]
    fn test_hash_order_independent() {
        // Swapped-insertion `from_entries` → equal
        // `equals`, so `hash_code` must also agree (Dart `mapHash` is
        // unordered). Previously the hash flattened in iteration order.
        // NOTE: key magnitudes must stay below ~21.5 (`fuzzy_hash_code`
        // saturates at `i32::MAX` above that, which would mask order effects
        // — all entries hashing identically makes even the ordered combine
        // agree). `1e-11` rounds to fuzzy code 1, `2e-11` to 2, etc.
        let arena = Bump::new();
        let ka = ValueKind::unitless_number(&arena, 1e-11);
        let va = ValueKind::unitless_number(&arena, 3e-11);
        let kb = ValueKind::unitless_number(&arena, 2e-11);
        let vb = ValueKind::unitless_number(&arena, 4e-11);
        assert_eq!(ka.hash_code(), 1, "precondition: distinct fuzzy codes");
        assert_eq!(kb.hash_code(), 2, "precondition: distinct fuzzy codes");
        let m1 = SassMap::from_entries(vec![(ka, va), (kb, vb)]);
        let m2 = SassMap::from_entries(vec![(kb, vb), (ka, va)]);
        assert!(m1.equals(&m2), "swapped maps must be equal (precondition)");
        assert_eq!(
            m1.hash_code(),
            m2.hash_code(),
            "equal maps must hash equally regardless of insertion order"
        );
    }

    #[test]
    fn test_map_separator() {
        let arena = Bump::new();
        assert_eq!(SassMap::empty().separator(), ListSeparator::Undecided);
        let m = SassMap::from_entries(vec![(
            ValueKind::unitless_number(&arena, 1.0),
            ValueKind::unitless_number(&arena, 1.0),
        )]);
        assert_eq!(m.separator(), ListSeparator::Comma);
    }

    #[test]
    fn test_map_to_css_string() {
        let arena = Bump::new();
        let mut map = SassMap::empty();
        let key = Value::new_with_arena(
            &arena,
            ValueKind::String(SassString::new(arena.alloc_str("a"), true)),
        );
        map.set(key, ValueKind::unitless_number(&arena, 1.0));
        let v = Value::new_with_arena(&arena, ValueKind::Map(map));
        assert!(v.to_css_string(true).is_err());
    }

    #[test]
    fn test_map_to_string() {
        let arena = Bump::new();
        let mut map = SassMap::empty();
        let key = Value::new_with_arena(
            &arena,
            ValueKind::String(SassString::new(arena.alloc_str("a"), true)),
        );
        map.set(key, ValueKind::unitless_number(&arena, 1.0));
        let v = Value::new_with_arena(&arena, ValueKind::Map(map));
        assert_eq!(v.to_display_string().unwrap(), "(\"a\": 1)");
    }
}
