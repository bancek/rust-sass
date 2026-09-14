// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/util/fuzzy_equality.dart
// go-source: go/util/fuzzy_equality.go

//! Fuzzy float equality/hash for use as map keys.
//!
//! Ports Dart's `FuzzyEquality` (a `package:collection` `Equality<double>`):
//! equality and hashing both delegate to [`super::number`], so floats that
//! compare fuzzy-equal hash identically.

use crate::util::number;
use std::any::Any;

/// Equality + hashing for floats under Sass fuzzy equality.
///
/// Matches Dart: `FuzzyEquality`.
#[derive(Debug, Clone, Copy, Default)]
pub struct FuzzyEquality;

impl FuzzyEquality {
    /// Returns whether `a` and `b` are equal up to the 11th decimal digit.
    pub fn equals(a: f64, b: f64) -> bool {
        number::fuzzy_equals(a, b)
    }

    /// Returns a hash code for `n` consistent with [`FuzzyEquality::equals`].
    pub fn hash(n: f64) -> i32 {
        number::fuzzy_hash_code(n)
    }

    /// Returns whether `o` is a valid key (a float).
    pub fn is_valid_key(o: &dyn Any) -> bool {
        o.is::<f64>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_equals() {
        assert!(FuzzyEquality::equals(1.0, 1.0));
        assert!(FuzzyEquality::equals(1.0, 1.0 + 1e-12));
        assert!(!FuzzyEquality::equals(1.0, 1.1));
    }

    #[test]
    fn test_hash() {
        let h1 = FuzzyEquality::hash(1.0);
        let h2 = FuzzyEquality::hash(1.0 + 1e-12);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_is_valid_key() {
        assert!(FuzzyEquality::is_valid_key(&1.0f64));
        assert!(!FuzzyEquality::is_valid_key(&"not a float"));
        assert!(!FuzzyEquality::is_valid_key(&42i32));
    }
}
