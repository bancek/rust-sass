// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/list.dart
// go-source: go/value/list.go

use crate::value::hash::list_hash;
use crate::value::Value;
use std::fmt;

use crate::common::SassResult;
use crate::serialize::SerializeVisitor;

use crate::value::{ListValue, ValueVisitor};

/// An enum of list separator types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListSeparator {
    /// A space-separated list.
    Space,
    /// A comma-separated list.
    Comma,
    /// A slash-separated list.
    Slash,
    /// A separator that hasn't yet been determined.
    ///
    /// Singleton lists and empty lists don't have separators defined. This
    /// means that list functions will prefer other lists' separators if
    /// possible.
    Undecided,
}

impl fmt::Display for ListSeparator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ListSeparator::Space => write!(f, "space"),
            ListSeparator::Comma => write!(f, "comma"),
            ListSeparator::Slash => write!(f, "slash"),
            ListSeparator::Undecided => write!(f, "undecided"),
        }
    }
}

impl ListSeparator {
    /// The separator character, or `None` when the separator hasn't been
    /// decided yet.
    pub fn sep_str(&self) -> Option<&'static str> {
        match self {
            ListSeparator::Space => Some(" "),
            ListSeparator::Comma => Some(","),
            ListSeparator::Slash => Some("/"),
            ListSeparator::Undecided => None,
        }
    }
}

/// A SassScript list.
#[derive(Clone, Debug)]
pub struct SassList<'parse> {
    /// The contents of the list.
    pub contents: Vec<Value<'parse>>,
    /// The separator between elements.
    pub separator: ListSeparator,
    /// Whether the list was written with brackets (`[1 2]`).
    pub has_brackets: bool,
}

impl<'parse> SassList<'parse> {
    /// Creates a list with the given `contents` and `separator`.
    ///
    /// Dart requires an explicit separator (not
    /// [`Undecided`](ListSeparator::Undecided)) for more than one element;
    /// this constructor doesn't validate, so callers must uphold it.
    pub fn new(
        contents: Vec<Value<'parse>>,
        separator: ListSeparator,
        has_brackets: bool,
    ) -> SassList<'parse> {
        SassList {
            contents,
            separator,
            has_brackets,
        }
    }

    /// Returns an empty list with the given `separator` and `has_brackets`.
    pub fn empty(separator: ListSeparator, has_brackets: bool) -> Self {
        SassList {
            contents: vec![],
            separator,
            has_brackets,
        }
    }

    // The length of [`as_list`](Self::asList).
    //
    // Matches Dart: `lengthAsList` (value/list.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`); base contract in
    // `Value.lengthAsList` (value.dart).
    pub fn length_as_list(&self) -> usize {
        self.contents.len()
    }

    /// This value as a list: the list's own contents.
    ///
    /// All SassScript values can be used as lists; lists just return their
    /// contents directly.
    pub fn as_list(&self) -> &[Value<'parse>] {
        &self.contents
    }

    /// Returns a valid CSS representation of `self`.
    ///
    /// Use [`to_display_string`](Self::to_display_string) instead to get a
    /// string representation even if this isn't valid CSS.
    ///
    /// If `quote` is `false`, quoted strings are emitted without quotes.
    pub fn to_css_string(&self, quote: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(quote, false);
        visitor.visit_list(&ListValue::List(self))?;
        Ok(visitor.into_string())
    }

    /// Returns a string representation of `self`.
    ///
    /// Note that this is equivalent to calling `inspect()` on the value, and
    /// thus won't reflect the user's output settings.
    /// [`to_css_string`](Self::to_css_string) should be used instead to
    /// convert `self` to CSS.
    ///
    /// Parentheses are added around the debug form to make the list bounds
    /// clear, except for bracketed, empty, or single-element comma lists.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(true, true);
        visitor.visit_list(&ListValue::List(self))?;
        let s = visitor.into_string();
        if self.has_brackets
            || self.contents.is_empty()
            || (self.contents.len() == 1 && self.separator == ListSeparator::Comma)
        {
            Ok(s)
        } else {
            Ok(format!("({s})"))
        }
    }

    // Whether the value serializes to the empty string in CSS: an
    // unbracketed list whose elements are all blank.
    //
    // Matches Dart: `isBlank` (value/list.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`); base contract in
    // `Value.isBlank` (value.dart).
    pub fn is_blank(&self) -> bool {
        if self.has_brackets {
            return false;
        }
        self.contents.iter().all(|e| e.is_blank())
    }

    /// Returns the hash code for this list's contents (separators and
    /// brackets are ignored, to match [`equals`](Self::equals)).
    pub fn hash_code(&self) -> i32 {
        let values: Vec<i32> = self.contents.iter().map(|v| v.hash_code()).collect();
        list_hash(&values)
    }

    /// Compares this list to `other` element-wise.
    ///
    /// Separators and brackets must match. An empty list additionally
    /// equals an empty map.
    pub fn equals(&self, other: &SassList<'parse>) -> bool {
        if self.separator != other.separator
            || self.has_brackets != other.has_brackets
            || self.contents.len() != other.contents.len()
        {
            return false;
        }
        self.contents
            .iter()
            .zip(&other.contents)
            .all(|(a, b)| a == b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{Value, ValueKind};
    use bumpalo::Bump;

    #[test]
    fn test_new_list_empty() {
        let l = SassList::empty(ListSeparator::Space, false);
        assert_eq!(l.length_as_list(), 0);
        assert_eq!(l.separator, ListSeparator::Space);
        assert!(!l.has_brackets);
    }

    #[test]
    fn test_list_equals() {
        let arena = Bump::new();
        let v1 = ValueKind::unitless_number(&arena, 1.0);
        let v2 = ValueKind::unitless_number(&arena, 2.0);
        let l1 = SassList::new(vec![v1, v2], ListSeparator::Space, false);
        let l2 = SassList::new(vec![v1, v2], ListSeparator::Space, false);
        assert!(l1.equals(&l2));
    }

    #[test]
    fn test_list_equals_different_separator() {
        let arena = Bump::new();
        let v1 = ValueKind::unitless_number(&arena, 1.0);
        let v2 = ValueKind::unitless_number(&arena, 2.0);
        let l1 = SassList::new(vec![v1, v2], ListSeparator::Space, false);
        let l2 = SassList::new(vec![v1, v2], ListSeparator::Comma, false);
        assert!(!l1.equals(&l2));
    }

    #[test]
    fn test_list_equals_different_brackets() {
        let arena = Bump::new();
        let v1 = ValueKind::unitless_number(&arena, 1.0);
        let l1 = SassList::new(vec![v1], ListSeparator::Space, false);
        let l2 = SassList::new(vec![v1], ListSeparator::Space, true);
        assert!(!l1.equals(&l2));
    }

    #[test]
    fn test_list_hash_code() {
        let arena = Bump::new();
        let v1 = ValueKind::unitless_number(&arena, 1.0);
        let v2 = ValueKind::unitless_number(&arena, 2.0);
        let l1 = SassList::new(vec![v1, v2], ListSeparator::Space, false);
        let l2 = SassList::new(
            vec![
                ValueKind::unitless_number(&arena, 1.0),
                ValueKind::unitless_number(&arena, 2.0),
            ],
            ListSeparator::Comma,
            false,
        );
        assert_eq!(
            l1.hash_code(),
            l2.hash_code(),
            "hash should ignore separator"
        );
    }

    #[test]
    fn test_list_is_blank() {
        let l = SassList::empty(ListSeparator::Space, false);
        assert!(l.is_blank(), "empty list without brackets should be blank");
        let l2 = SassList::empty(ListSeparator::Space, true);
        assert!(!l2.is_blank(), "bracketed list should not be blank");
    }

    #[test]
    fn test_list_as_list() {
        let arena = Bump::new();
        let v1 = ValueKind::unitless_number(&arena, 1.0);
        let l = SassList::new(vec![v1], ListSeparator::Space, false);
        assert_eq!(l.as_list().len(), 1);
    }

    #[test]
    fn test_list_separator_display() {
        assert_eq!(format!("{}", ListSeparator::Space), "space");
        assert_eq!(format!("{}", ListSeparator::Comma), "comma");
        assert_eq!(format!("{}", ListSeparator::Slash), "slash");
        assert_eq!(format!("{}", ListSeparator::Undecided), "undecided");
    }

    #[test]
    fn test_list_to_css_string() {
        let arena = Bump::new();
        let list = SassList::new(
            vec![
                ValueKind::unitless_number(&arena, 1.0),
                ValueKind::unitless_number(&arena, 2.0),
            ],
            ListSeparator::Space,
            false,
        );
        let v = Value::new_with_arena(&arena, ValueKind::List(list));
        assert_eq!(v.to_css_string(true).unwrap(), "1 2");
    }

    #[test]
    fn test_list_to_css_string_empty_error() {
        let arena = Bump::new();
        let list = SassList::new(vec![], ListSeparator::Space, false);
        let v = Value::new_with_arena(&arena, ValueKind::List(list));
        assert!(v.to_css_string(true).is_err());
    }

    #[test]
    fn test_list_to_string() {
        let arena = Bump::new();
        // Mirrors Go: TestListString — String() adds Dart toString parens.
        let list = SassList::new(
            vec![
                ValueKind::unitless_number(&arena, 1.0),
                ValueKind::unitless_number(&arena, 2.0),
                ValueKind::unitless_number(&arena, 3.0),
            ],
            ListSeparator::Comma,
            false,
        );
        let v = Value::new_with_arena(&arena, ValueKind::List(list));
        assert_eq!(v.to_display_string().unwrap(), "(1, 2, 3)");

        let space_list = SassList::new(
            vec![
                ValueKind::unitless_number(&arena, 1.0),
                ValueKind::unitless_number(&arena, 2.0),
                ValueKind::unitless_number(&arena, 3.0),
            ],
            ListSeparator::Space,
            false,
        );
        let v = Value::new_with_arena(&arena, ValueKind::List(space_list));
        assert_eq!(v.to_display_string().unwrap(), "(1 2 3)");

        let empty = SassList::new(vec![], ListSeparator::Space, false);
        let v = Value::new_with_arena(&arena, ValueKind::List(empty));
        assert_eq!(v.to_display_string().unwrap(), "()");
    }

    #[test]
    fn test_list_to_string_single_comma_inspect() {
        let arena = Bump::new();
        // Mirrors Go: TestListString_SingleCommaInspect — no extra parens added.
        let single = SassList::new(
            vec![ValueKind::unitless_number(&arena, 42.0)],
            ListSeparator::Comma,
            false,
        );
        let v = Value::new_with_arena(&arena, ValueKind::List(single));
        assert_eq!(v.to_display_string().unwrap(), "(42,)");
    }

    #[test]
    fn test_list_to_string_bracketed() {
        let arena = Bump::new();
        // Bracketed lists don't get extra parens (Go listString HasBrackets branch).
        let list = SassList::new(
            vec![
                ValueKind::unitless_number(&arena, 1.0),
                ValueKind::unitless_number(&arena, 2.0),
            ],
            ListSeparator::Space,
            true,
        );
        let v = Value::new_with_arena(&arena, ValueKind::List(list));
        assert_eq!(v.to_display_string().unwrap(), "[1 2]");
    }
}
