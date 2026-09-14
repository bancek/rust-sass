// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/argument_list.dart
// go-source: go/value/argument_list.go

use crate::common::SassResult;
use std::cell::Cell;

use bumpalo::Bump;
use indexmap::IndexMap;

use crate::value::{ListSeparator, SassList, Value};

/// A SassScript argument list.
///
/// An argument list comes from a rest argument. It's distinct from a normal
/// [`SassList`] in that it may contain a keyword map as well as the
/// positional arguments.
#[derive(Clone, Debug)]
pub struct SassArgumentList<'parse> {
    /// The positional arguments.
    pub list: SassList<'parse>,
    /// The keyword arguments attached to this argument list.
    ///
    /// The argument names don't include `$`.
    pub keywords: IndexMap<String, Value<'parse>>,
    /// Whether `keywords()` has been accessed. Arena-allocated `Cell`, shared
    /// across clones by reference (Copy), matching Go's pointer semantics (the
    /// evaluator checks the original value after built-in calls).
    pub were_keywords_accessed: &'parse Cell<bool>,
}

impl<'parse> SassArgumentList<'parse> {
    /// Creates an argument list with the given positional `contents`,
    /// `keywords`, and list `separator`.
    pub fn new<'compile: 'parse>(
        arena: &'compile Bump,
        contents: Vec<Value<'parse>>,
        keywords: IndexMap<String, Value<'parse>>,
        separator: ListSeparator,
    ) -> Self {
        SassArgumentList {
            list: SassList::new(contents, separator, false),
            keywords,
            were_keywords_accessed: arena.alloc(Cell::new(false)),
        }
    }

    /// Returns the keyword arguments, marking them as accessed.
    /// Matches Go: SassArgumentList.Keywords / Dart: keywords getter.
    pub fn keywords(&self) -> &IndexMap<String, Value<'parse>> {
        self.were_keywords_accessed.set(true);
        &self.keywords
    }

    /// Returns the same value as [`keywords`](Self::keywords), but doesn't
    /// mark them accessed.
    ///
    /// Normally, any time [`keywords`](Self::keywords) is accessed it's
    /// marked as such, which indicates that the caller was allowed to pass
    /// keywords to a rest argument. This avoids this marking.
    pub fn keywords_without_marking(&self) -> &IndexMap<String, Value<'parse>> {
        &self.keywords
    }

    /// This value as a list: the positional arguments.
    ///
    /// All SassScript values can be used as lists; argument lists count as
    /// their positional contents.
    pub fn as_list(&self) -> &[Value<'parse>] {
        self.list.as_list()
    }

    // The length of [`as_list`](Self::asList).
    //
    // Matches Dart: `lengthAsList` inherited from `SassList`
    // (value/list.dart via value/argument_list.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`); base contract in
    // `Value.lengthAsList` (value.dart).
    pub fn length_as_list(&self) -> usize {
        self.list.length_as_list()
    }

    // Whether the value serializes to the empty string in CSS: an
    // unbracketed argument list whose positional elements are all blank.
    //
    // Matches Dart: `isBlank` inherited from `SassList`
    // (value/list.dart via value/argument_list.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`); base contract in
    // `Value.isBlank` (value.dart).
    pub fn is_blank(&self) -> bool {
        self.list.is_blank()
    }

    /// Whether this value as a list has brackets (always `false` for
    /// argument lists).
    ///
    /// All SassScript values can be used as lists; argument lists count as
    /// their positional contents.
    pub fn has_brackets(&self) -> bool {
        self.list.has_brackets
    }

    /// Returns a valid CSS representation of `self`.
    ///
    /// Use [`to_display_string`](Self::to_display_string) instead to get a
    /// string representation even if this isn't valid CSS.
    ///
    /// If `quote` is `false`, quoted strings are emitted without quotes.
    pub fn to_css_string(&self, quote: bool) -> SassResult<String> {
        self.list.to_css_string(quote)
    }

    /// Returns a string representation of `self`.
    ///
    /// Note that this is equivalent to calling `inspect()` on the value, and
    /// thus won't reflect the user's output settings.
    /// [`to_css_string`](Self::to_css_string) should be used instead to
    /// convert `self` to CSS.
    pub fn to_display_string(&self) -> SassResult<String> {
        self.list.to_display_string()
    }

    /// Compares this argument list's positional contents to `other`,
    /// like [`SassList::equals`].
    pub fn equals(&self, other: &SassList<'parse>) -> bool {
        self.list.equals(other)
    }

    /// Returns the hash code for this argument list's positional contents,
    /// like [`SassList::hash_code`].
    pub fn hash_code(&self) -> i32 {
        self.list.hash_code()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{Value, ValueKind};
    use bumpalo::Bump;

    #[test]
    fn test_arglist_construction() {
        let arena = Bump::new();
        let al = SassArgumentList::new(
            &arena,
            vec![
                ValueKind::unitless_number(&arena, 1.0),
                ValueKind::unitless_number(&arena, 2.0),
            ],
            IndexMap::new(),
            ListSeparator::Space,
        );
        assert_eq!(al.length_as_list(), 2);
        assert!(!al.has_brackets());
    }

    #[test]
    fn test_arglist_keywords() {
        let arena = Bump::new();
        let mut keywords = IndexMap::new();
        keywords.insert("a".to_string(), ValueKind::unitless_number(&arena, 1.0));
        let al = SassArgumentList::new(&arena, vec![], keywords, ListSeparator::Space);
        assert!(!al.were_keywords_accessed.get());
        let _ = al.keywords();
        assert!(al.were_keywords_accessed.get());
    }

    #[test]
    fn test_arglist_keywords_without_marking() {
        let arena = Bump::new();
        let mut keywords = IndexMap::new();
        keywords.insert("a".to_string(), ValueKind::unitless_number(&arena, 1.0));
        let al = SassArgumentList::new(&arena, vec![], keywords, ListSeparator::Space);
        let _ = al.keywords_without_marking();
        assert!(!al.were_keywords_accessed.get());
    }

    #[test]
    fn test_arglist_equals() {
        let arena = Bump::new();
        let v1 = ValueKind::unitless_number(&arena, 1.0);
        let v2 = ValueKind::unitless_number(&arena, 2.0);
        let al = SassArgumentList::new(&arena, vec![v1, v2], IndexMap::new(), ListSeparator::Space);
        let l = SassList::new(vec![v1, v2], ListSeparator::Space, false);
        assert!(al.equals(&l));
        assert!(l.equals(&al.list));
    }

    #[test]
    fn test_arglist_hash_code() {
        let arena = Bump::new();
        let al = SassArgumentList::new(
            &arena,
            vec![
                ValueKind::unitless_number(&arena, 1.0),
                ValueKind::unitless_number(&arena, 2.0),
            ],
            IndexMap::new(),
            ListSeparator::Space,
        );
        let l = SassList::new(
            vec![
                ValueKind::unitless_number(&arena, 1.0),
                ValueKind::unitless_number(&arena, 2.0),
            ],
            ListSeparator::Space,
            false,
        );
        assert_eq!(al.hash_code(), l.hash_code());
    }

    #[test]
    fn test_arglist_to_css_string() {
        let arena = Bump::new();
        let list = SassList::new(
            vec![
                ValueKind::unitless_number(&arena, 1.0),
                ValueKind::unitless_number(&arena, 2.0),
            ],
            ListSeparator::Space,
            false,
        );
        let al = SassArgumentList::new(
            &arena,
            list.contents,
            Default::default(),
            ListSeparator::Space,
        );
        let v = Value::new_with_arena(&arena, ValueKind::ArgumentList(al));
        assert_eq!(v.to_css_string(true).unwrap(), "1 2");
    }

    #[test]
    fn test_arglist_to_string() {
        let arena = Bump::new();
        // Mirrors Go: TestArgListString — String() adds Dart toString parens.
        let list = SassList::new(
            vec![
                ValueKind::unitless_number(&arena, 1.0),
                ValueKind::unitless_number(&arena, 2.0),
            ],
            ListSeparator::Space,
            false,
        );
        let al = SassArgumentList::new(
            &arena,
            list.contents,
            Default::default(),
            ListSeparator::Space,
        );
        let v = Value::new_with_arena(&arena, ValueKind::ArgumentList(al));
        assert_eq!(v.to_display_string().unwrap(), "(1 2)");
    }
}
