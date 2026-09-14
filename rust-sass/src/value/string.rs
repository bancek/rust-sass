// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/string.dart
// go-source: go/value/string.go

use crate::common::SassError;
use crate::value::hash::string_hash_code;
use bumpalo::Bump;

use crate::common::exception::SassResult;
use crate::value::{Value, ValueKind};

use crate::serialize::SerializeVisitor;
use crate::value::ValueVisitor;

/// A SassScript string.
///
/// Strings can either be quoted or unquoted. Unquoted strings are usually CSS
/// identifiers, but they may contain any text.
#[derive(Clone, Debug)]
pub struct SassString<'parse> {
    /// The contents of the string.
    ///
    /// For quoted strings this is the semantic content: escape sequences
    /// written in the source text are resolved to their Unicode values. For
    /// unquoted strings, escape sequences are preserved as literal
    /// backslashes, which is what lets an identifier with escapes such as
    /// `url\u28 http://example.com\u29` stay distinct from an unquoted
    /// string containing characters that aren't valid in identifiers, such
    /// as `url(http://example.com)`. It also means `foo` and `f\6F\6F`
    /// don't compare equal.
    pub text: &'parse str,
    /// Whether this string has quotes.
    pub has_quotes: bool,
}

impl<'parse> SassString<'parse> {
    /// Creates a string with the given `text`.
    pub fn new(text: &'parse str, has_quotes: bool) -> Self {
        SassString { text, has_quotes }
    }

    /// Whether the value counts as `true` in an `@if` statement and other
    /// contexts.
    pub fn is_truthy(&self) -> bool {
        true
    }

    // Whether the value serializes to the empty string in CSS: an unquoted
    // empty string.
    //
    // Matches Dart: `isBlank` (value/string.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`); base contract in
    // `Value.isBlank` (value.dart).
    pub fn is_blank(&self) -> bool {
        !self.has_quotes && self.text.is_empty()
    }

    /// Compares this string to `other` by text, ignoring quotes.
    pub fn equals(&self, other: &SassString) -> bool {
        self.text == other.text
    }

    /// Returns the hash code for this string's text (quotes ignored, to
    /// match [`equals`](Self::equals)).
    pub fn hash_code(&self) -> i32 {
        string_hash_code(self.text)
    }

    /// Returns a valid CSS representation of `self`.
    ///
    /// Use [`to_display_string`](Self::to_display_string) instead to get a
    /// string representation even if this isn't valid CSS.
    ///
    /// If `quote` is `false`, quoted strings are emitted without quotes.
    pub fn to_css_string(&self, quote: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(quote, false);
        visitor.visit_string(self)?;
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
        visitor.visit_string(self)?;
        Ok(visitor.into_string())
    }

    /// Sass's notion of the length of this string: the number of Unicode
    /// code points, not UTF-16 code units or bytes.
    ///
    /// For example U+1F60A counts as one character here, even though Dart's
    /// `String.length` reports two UTF-16 code units for it. This matches
    /// what `str-length()` returns.
    pub fn sass_length(&self) -> usize {
        self.text.chars().count()
    }

    // Whether CSS may treat this value as a number, such as `calc()` or
    // `var()`. Functions that shadow plain CSS functions use this to decide
    // when to gracefully pass the argument through.
    //
    // Matches Dart: `isSpecialNumber` (value/string.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`); base contract in
    // `Value.isSpecialNumber` (value.dart).
    pub fn is_special_number(&self) -> bool {
        if self.has_quotes {
            return false;
        }
        if self.text.len() < 6 {
            return false;
        }
        let lower = self.text.to_lowercase();
        let prefixes = [
            "calc(", "attr(", "clamp(", "env(", "if(", "min(", "max(", "var(",
        ];
        prefixes.iter().any(|p| lower.starts_with(p))
    }

    // Whether this is a call to `var()`, which may be substituted in CSS
    // for a custom property value. Functions that shadow plain CSS
    // functions use this to decide when to gracefully pass the argument
    // through.
    //
    // Matches Dart: `isSpecialVariable` (value/string.dart) —
    // intentionally undocumented in Dart (`@nodoc`/`@internal`); base
    // contract in `Value.isSpecialVariable` (value.dart).
    pub fn is_special_variable(&self) -> bool {
        if self.has_quotes {
            return false;
        }
        if self.text.len() < 6 {
            return false;
        }
        let lower = self.text.to_lowercase();
        lower.starts_with("attr(") || lower.starts_with("if(") || lower.starts_with("var(")
    }

    // The SassScript `+` operation: concatenates `other` onto `self`,
    // preserving `self`'s quotes (a string operand contributes its text,
    // any other operand its CSS form).
    //
    // Matches Dart: `SassString.plus` (value/string.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`); refines the base
    // contract in `Value.plus` (value.dart).
    pub fn plus<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        other: &ValueKind<'parse>,
    ) -> SassResult<Value<'parse>> {
        match other {
            ValueKind::String(os) => Ok(Value::new_with_arena(
                arena,
                ValueKind::String(SassString::new(
                    arena.alloc_str(&format!("{}{}", self.text, os.text)),
                    self.has_quotes,
                )),
            )),
            _ => {
                let right = other.to_css_string(true)?;
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::String(SassString::new(
                        arena.alloc_str(&format!("{}{}", self.text, right)),
                        self.has_quotes,
                    )),
                ))
            }
        }
    }

    // Fails unless this is a quoted string.
    //
    // Matches Dart: `assertQuoted` (value/string.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`). Unlike Dart this takes
    // no argument name; use the caller's attribution instead.
    pub fn assert_quoted(&self) -> SassResult<()> {
        if !self.has_quotes {
            let s = self.to_display_string()?;
            Err(Box::new(SassError::Script {
                message: format!("Expected {s} to be a quoted string."),
                argument_name: None,
            }))
        } else {
            Ok(())
        }
    }

    // Fails unless this is an unquoted string.
    //
    // Matches Dart: `assertUnquoted` (value/string.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`). Unlike Dart this takes
    // no argument name; use [`assert_unquoted_with_name`](Self::assert_unquoted_with_name)
    // for attributed errors.
    pub fn assert_unquoted(&self) -> SassResult<()> {
        if self.has_quotes {
            let s = self.to_display_string()?;
            Err(Box::new(SassError::Script {
                message: format!("Expected {s} to be an unquoted string."),
                argument_name: None,
            }))
        } else {
            Ok(())
        }
    }

    /// Like [`assert_unquoted`](Self::assert_unquoted), but attributes the
    /// error to the argument [name]. Matches Dart's
    /// `assertUnquoted([String? name])` cascade used by
    /// `InterpolationMethod.fromValue` (`(list.first.assertString(name)
    /// ..assertUnquoted(name))`).
    pub fn assert_unquoted_with_name(&self, name: Option<&str>) -> SassResult<()> {
        if self.has_quotes {
            let s = self.to_display_string()?;
            Err(Box::new(SassError::Script {
                message: format!("Expected {s} to be an unquoted string."),
                argument_name: name.map(str::to_string),
            }))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::SASS_TRUE;

    #[test]
    fn test_string_props() {
        let s = SassString::new("hello", true);
        assert_eq!(s.text, "hello");
        assert!(s.has_quotes);
        assert!(s.is_truthy());
    }

    #[test]
    fn test_string_is_blank() {
        let unq = SassString::new("", false);
        assert!(unq.is_blank());
        let q = SassString::new("", true);
        assert!(!q.is_blank());
        let s = SassString::new(" ", false);
        assert!(!s.is_blank());
    }

    #[test]
    fn test_string_equals() {
        let arena = Bump::new();
        let a = SassString::new(arena.alloc_str("hello"), true);
        let b = SassString::new(arena.alloc_str("hello"), false);
        assert!(a.equals(&b), "should ignore quotes");
        let c = SassString::new(arena.alloc_str("world"), true);
        assert!(!a.equals(&c));
    }

    #[test]
    fn test_string_hash_code() {
        let arena = Bump::new();
        let a = SassString::new(arena.alloc_str("hello"), true);
        let b = SassString::new(arena.alloc_str("hello"), false);
        assert_eq!(a.hash_code(), b.hash_code(), "hash should ignore quotes");
        let c = SassString::new(arena.alloc_str("world"), true);
        assert_ne!(a.hash_code(), c.hash_code());
        assert_eq!(a.hash_code(), a.hash_code(), "hash should be deterministic");
    }

    #[test]
    fn test_string_sass_length() {
        let arena = Bump::new();
        let s = SassString::new(arena.alloc_str("abc"), false);
        assert_eq!(s.sass_length(), 3);
        let s2 = SassString::new(arena.alloc_str("é"), false);
        assert_eq!(s2.sass_length(), 1);
        let s3 = SassString::new(arena.alloc_str("á"), false); // a + combining accent = 2 chars
        assert_eq!(s3.sass_length(), 2);
    }

    #[test]
    fn test_string_is_special_number() {
        let arena = Bump::new();
        let tests: &[(&str, bool)] = &[
            ("calc(1px + 2px)", true),
            ("attr(data-val)", true),
            ("clamp(0, 1, 2)", true),
            ("env(SAFE)", true),
            ("if(true, 1, 2)", true),
            ("min(1, 2)", true),
            ("max(1, 2)", true),
            ("var(--x)", true),
            ("normal", false),
            ("ab", false),
        ];
        for (text, expected) in tests {
            let s = SassString::new(arena.alloc_str(text), false);
            assert_eq!(
                s.is_special_number(),
                *expected,
                "is_special_number({text:?})"
            );
        }
    }

    #[test]
    fn test_string_is_special_variable() {
        let arena = Bump::new();
        let tests: &[(&str, bool)] = &[
            ("var(--x)", true),
            ("attr(data-val)", true),
            ("if(true, 1, 2)", true),
            ("calc(1px)", false),
            ("normal", false),
        ];
        for (text, expected) in tests {
            let s = SassString::new(arena.alloc_str(text), false);
            assert_eq!(
                s.is_special_variable(),
                *expected,
                "is_special_variable({text:?})"
            );
        }
    }

    #[test]
    fn test_string_to_css_string_quoted() {
        let arena = Bump::new();
        let v = SassString::new(arena.alloc_str("hello"), true);
        let val = Value::new_with_arena(&arena, ValueKind::String(v));
        assert_eq!(val.to_css_string(true).unwrap(), "\"hello\"");
    }

    #[test]
    fn test_string_to_css_string_unquoted() {
        let arena = Bump::new();
        let v = SassString::new(arena.alloc_str("hello"), false);
        let val = Value::new_with_arena(&arena, ValueKind::String(v));
        assert_eq!(val.to_css_string(true).unwrap(), "hello");
    }

    #[test]
    fn test_string_to_string() {
        let arena = Bump::new();
        let v = SassString::new(arena.alloc_str("hello"), true);
        let val = Value::new_with_arena(&arena, ValueKind::String(v));
        assert_eq!(val.to_display_string().unwrap(), "\"hello\"");
    }

    #[test]
    fn test_string_plus_non_string() {
        let arena = Bump::new();
        let s = SassString::new(arena.alloc_str("a"), false);
        let result = s
            .plus(
                &arena,
                &Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE)),
            )
            .unwrap();
        match &*result {
            ValueKind::String(ss) => {
                assert_eq!(ss.text, "atrue");
                assert!(!ss.has_quotes);
            }
            _ => panic!("expected Value::String"),
        }
    }
}
