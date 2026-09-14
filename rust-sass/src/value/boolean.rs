// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/boolean.dart
// go-source: go/value/boolean.go

use crate::common::SassResult;
use crate::serialize::SerializeVisitor;

use crate::value::ValueVisitor;

/// A SassScript boolean value.
#[derive(Clone, Copy, Debug)]
pub struct SassBoolean {
    /// Whether this value is `true` or `false`.
    pub value: bool,
}

impl SassBoolean {
    /// Returns a [`SassBoolean`] corresponding to `value`.
    ///
    /// This just returns [`SASS_TRUE`] or [`SASS_FALSE`]; it doesn't allocate
    /// a new value.
    pub fn new(value: bool) -> SassBoolean {
        SassBoolean { value }
    }

    /// Whether the value counts as `true` in an `@if` statement and other
    /// contexts.
    pub fn is_truthy(&self) -> bool {
        self.value
    }

    /// Returns a valid CSS representation of `self`.
    ///
    /// Use [`to_display_string`](Self::to_display_string) instead to get a
    /// string representation even if this isn't valid CSS (booleans always
    /// are).
    ///
    /// If `quote` is `false`, quoted strings are emitted without quotes
    /// (no effect on booleans).
    pub fn to_css_string(&self, quote: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(quote, false);
        visitor.visit_boolean(self)?;
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
        visitor.visit_boolean(self)?;
        Ok(visitor.into_string())
    }

    /// Returns the hash code for this boolean (`true` → 1, `false` → 0).
    pub fn hash_code(&self) -> i32 {
        if self.value {
            1
        } else {
            0
        }
    }

    /// Compares this boolean to `other` by value.
    pub fn equals(&self, other: &SassBoolean) -> bool {
        self.value == other.value
    }

    // The SassScript unary `not` operation.
    //
    // Matches Dart: `SassBoolean.unaryNot` (value/boolean.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`); base contract in
    // `Value.unaryNot` (value.dart).
    pub fn unary_not(&self) -> SassBoolean {
        SassBoolean { value: !self.value }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SassTrue;
#[derive(Clone, Copy, Debug)]
pub struct SassFalse;

/// The SassScript `true` value.
pub const SASS_TRUE: SassBoolean = SassBoolean { value: true };
/// The SassScript `false` value.
pub const SASS_FALSE: SassBoolean = SassBoolean { value: false };

#[cfg(test)]
mod tests {
    use crate::value::{Value, ValueKind};
    use bumpalo::Bump;

    use super::*;

    #[test]
    fn test_boolean_new() {
        let t = SassBoolean::new(true);
        assert!(t.value);
        assert!(t.is_truthy());
        let f = SassBoolean::new(false);
        assert!(!f.value);
        assert!(!f.is_truthy());
    }

    #[test]
    fn test_boolean_is_truthy() {
        assert!(SASS_TRUE.is_truthy());
        assert!(!SASS_FALSE.is_truthy());
    }

    #[test]
    fn test_boolean_unary_not() {
        assert!(!SASS_TRUE.unary_not().value);
        assert!(SASS_FALSE.unary_not().value);
    }

    #[test]
    fn test_boolean_equals() {
        assert!(SASS_TRUE.equals(&SASS_TRUE));
        assert!(SASS_FALSE.equals(&SASS_FALSE));
        assert!(!SASS_TRUE.equals(&SASS_FALSE));
    }

    #[test]
    fn test_boolean_hash_code() {
        assert_eq!(SASS_TRUE.hash_code(), 1);
        assert_eq!(SASS_FALSE.hash_code(), 0);
    }

    #[test]
    fn test_boolean_list_props() {
        assert!(SASS_TRUE.is_truthy());
        assert!(!SASS_FALSE.is_truthy());
    }

    #[test]
    fn test_boolean_to_css_string() {
        let arena = Bump::new();
        let v = Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE));
        assert_eq!(v.to_css_string(true).unwrap(), "true");
        let v = Value::new_with_arena(&arena, ValueKind::Boolean(SASS_FALSE));
        assert_eq!(v.to_css_string(true).unwrap(), "false");
    }

    #[test]
    fn test_boolean_to_string() {
        let arena = Bump::new();
        let v = Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE));
        assert_eq!(v.to_display_string().unwrap(), "true");
        let v = Value::new_with_arena(&arena, ValueKind::Boolean(SASS_FALSE));
        assert_eq!(v.to_display_string().unwrap(), "false");
    }

    #[test]
    fn test_boolean_real_null() {
        // real_null returns Some(self) for non-null values
        // tested via Value enum
    }
}
