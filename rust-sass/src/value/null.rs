// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/null.dart
// go-source: go/value/null.go

use crate::common::SassResult;
use crate::serialize::SerializeVisitor;
use crate::value::SassBoolean;
use crate::value::SASS_TRUE;

use crate::value::ValueVisitor;

/// A SassScript null value.
///
/// The canonical instance is [`SASS_NULL`]; like in Dart this is a unit
/// value with no additional state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SassNull;

/// The SassScript `null` value.
pub const SASS_NULL: SassNull = SassNull;

impl SassNull {
    /// Whether the value counts as `true` in an `@if` statement and other
    /// contexts.
    pub fn is_truthy(&self) -> bool {
        false
    }

    /// Returns a valid CSS representation of `self`.
    ///
    /// Use [`to_display_string`](Self::to_display_string) instead to get a
    /// string representation even if this isn't valid CSS (null serializes
    /// as the empty string).
    ///
    /// If `quote` is `false`, quoted strings are emitted without quotes
    /// (no effect on null).
    pub fn to_css_string(&self, quote: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(quote, false);
        visitor.visit_null()?;
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
        visitor.visit_null()?;
        Ok(visitor.into_string())
    }

    // Whether the value serializes to the empty string in CSS. Null always
    // does.
    //
    // Matches Dart: `isBlank` (value/null.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`); base contract in
    // `Value.isBlank` (value.dart).
    pub fn is_blank(&self) -> bool {
        true
    }

    /// Returns the hash code for null (always 0).
    pub fn hash_code(&self) -> i32 {
        0
    }

    /// Compares this null to `other`; all nulls are equal.
    pub fn equals(&self, _other: &SassNull) -> bool {
        true
    }

    // The SassScript unary `not` operation. Null is falsy, so this returns
    // true.
    //
    // Matches Dart: `_SassNull.unaryNot` (value/null.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`); base contract in
    // `Value.unaryNot` (value.dart).
    pub fn unary_not(&self) -> SassBoolean {
        SASS_TRUE
    }
}

#[cfg(test)]
mod tests {
    use crate::value::ValueKind;

    use super::*;

    #[test]
    fn test_null_is_truthy() {
        assert!(!SASS_NULL.is_truthy());
    }

    #[test]
    fn test_null_is_blank() {
        assert!(SASS_NULL.is_blank());
    }

    #[test]
    fn test_null_hash_code() {
        assert_eq!(SASS_NULL.hash_code(), 0);
    }

    #[test]
    fn test_null_equals() {
        assert!(SASS_NULL.equals(&SASS_NULL));
    }

    #[test]
    fn test_null_to_css_string() {
        assert_eq!(ValueKind::Null.to_css_string(true).unwrap(), "");
    }

    #[test]
    fn test_null_to_string() {
        assert_eq!(ValueKind::Null.to_display_string().unwrap(), "null");
    }
}
