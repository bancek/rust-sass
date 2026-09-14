// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/embedded/opaque_registry.dart
// go-source: go/embedded/opaque_registry.go

use std::collections::HashMap;
use std::hash::Hash;

/// Maps compiler-defined values (functions/mixins) to opaque integer IDs for
/// the embedded protocol.
///
/// The registry stores the **whole value** (not just its callable), keyed by
/// the value itself — matching Dart's `OpaqueRegistry<T>` (`Map<T, int>`),
/// where `T`'s `hashCode`/`==` delegate to the callable's identity. Storing
/// the value means a `CompilerFunction`/`CompilerMixin` round-trips through
/// the host with its compile context intact, so `meta.call`/`meta.apply`'s
/// `assertCompileContext` sees the eval's context.
///
/// The registry lives for one compilation (the arena is live for the whole
/// compile). At the concrete compile call site the `'compile`/`'parse`
/// lifetimes unify (`'compile: 'parse, 'parse: 'compile`), which is how
/// `SassFunction`/`SassMixin` (holding `Callable<'parse, 'parse>`) coerce into
/// the registry's `'parse` element type.
///
/// Matches Dart: OpaqueRegistry<T>.
pub struct OpaqueRegistry<T> {
    elements: Vec<T>,
    ids: HashMap<T, u32>,
}

impl<T: Eq + Hash + Clone> OpaqueRegistry<T> {
    pub fn new() -> Self {
        OpaqueRegistry {
            elements: Vec::new(),
            ids: HashMap::new(),
        }
    }

    /// Returns the ID for `value`, registering it if this is the first time it's
    /// been seen. IDs are assigned in first-seen order starting at 0.
    pub fn get_id(&mut self, value: &T) -> u32 {
        if let Some(&id) = self.ids.get(value) {
            return id;
        }
        let id = self.elements.len() as u32;
        self.elements.push(value.clone());
        self.ids.insert(value.clone(), id);
        id
    }

    pub fn get(&self, id: u32) -> Option<&T> {
        self.elements.get(id as usize)
    }

    /// Clears the registry (between compilations on a reused context).
    pub fn clear(&mut self) {
        self.elements.clear();
        self.ids.clear();
    }
}

impl<T: Eq + Hash + Clone> Default for OpaqueRegistry<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_sass::callable::{Callable, CallableKind, PlainCssCallable};
    use rust_sass::value::{SassFunction, SassMixin};
    use rust_sass::Bump;

    fn dummy<'p>(arena: &'p Bump, name: &str) -> Callable<'p, 'p> {
        Callable::new(
            arena,
            CallableKind::PlainCss(PlainCssCallable { name: name.into() }),
        )
    }

    #[test]
    fn get_id_is_idempotent() {
        let bump = Bump::new();
        let mut registry: OpaqueRegistry<SassFunction<'_>> = OpaqueRegistry::new();
        let f = SassFunction::new(dummy(&bump, "foo"));
        assert_eq!(registry.get_id(&f), 0);
        assert_eq!(registry.get_id(&f), 0);
        assert_eq!(registry.get_id(&f), 0);
    }

    #[test]
    fn distinct_values_get_distinct_ids() {
        let bump = Bump::new();
        let mut registry: OpaqueRegistry<SassFunction<'_>> = OpaqueRegistry::new();
        let a = SassFunction::new(dummy(&bump, "a"));
        let b = SassFunction::new(dummy(&bump, "b"));
        assert_eq!(registry.get_id(&a), 0);
        assert_eq!(registry.get_id(&b), 1);
        assert_eq!(registry.get_id(&a), 0);
        assert_eq!(registry.get_id(&b), 1);
    }

    #[test]
    fn get_roundtrips() {
        let bump = Bump::new();
        let mut registry: OpaqueRegistry<SassMixin<'_>> = OpaqueRegistry::new();
        let a = SassMixin::new(dummy(&bump, "a"));
        let id = registry.get_id(&a);
        let got = registry.get(id).unwrap();
        assert!(got.equals(&a));
        assert_eq!(registry.get(99), None);
    }
}
