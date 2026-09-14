// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector/placeholder.dart + lib/src/extend/functions.dart
// go-source: go/value/selector_placeholder.go + go/value/selector_extend_functions.go

use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::selector::selector_assert_not_bogus_impl;
use crate::selector::simple_base_unify;
use crate::selector::visitor::SelectorVisitor;
use crate::selector::WarnLogger;
use crate::serialize::SerializeVisitor;
use crate::value::hash::string_hash_code;
use std::hash::Hash;
use std::hash::Hasher;

use crate::selector::SimpleSelector;

/// A placeholder selector.
///
/// This matches no elements. It exists to be extended with `@extend`, and
/// is not plain CSS — it must be removed before a CSS document is emitted.
#[derive(Clone, Debug)]
pub struct PlaceholderSelector<'parse> {
    /// The source span covering this selector.
    pub span: FileSpan<'parse>,
    /// The name of the placeholder.
    pub name: String,
}

// Matches Dart: structural equality over the name; the span is excluded.
impl<'parse> PartialEq for PlaceholderSelector<'parse> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl<'parse> Eq for PlaceholderSelector<'parse> {}

impl<'parse> PlaceholderSelector<'parse> {
    /// Creates a placeholder selector for `name`.
    pub fn new(name: String, span: FileSpan<'parse>) -> Self {
        PlaceholderSelector { span, name }
    }

    /// The source span of this selector.
    pub fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }

    // Matches Dart: placeholders match nothing, so they are always
    // invisible and dropped before emission.
    pub fn is_invisible(&self) -> bool {
        true
    }

    /// Whether this selector is not valid CSS; always `false` here.
    pub fn is_bogus(&self) -> bool {
        false
    }

    // Matches Dart: never useless.
    pub fn is_useless(&self) -> bool {
        false
    }

    // Matches Dart: placeholders never contain a parent selector.
    pub fn contains_parent_selector(&self) -> SassResult<bool> {
        Ok(false)
    }

    // Matches Dart: delegates to `is_invisible`, ignoring bogus combinators.
    pub fn is_invisible_other_than_bogus_combinators(&self) -> bool {
        self.is_invisible()
    }

    // Matches Dart: delegates to `is_bogus`, ignoring a leading combinator.
    pub fn is_bogus_other_than_leading_combinator(&self) -> bool {
        self.is_bogus()
    }

    /// This selector's specificity: 1000 (specificity is base-1000).
    pub fn specificity(&self) -> usize {
        1000
    }

    // Matches Dart: plain placeholder matching needs no non-local reasoning.
    pub fn has_complicated_superselector_semantics(&self) -> bool {
        false
    }

    /// Whether this is private — that is, whether the name begins with `-`
    /// or `_`.
    pub fn is_private(&self) -> bool {
        !self.name.is_empty() && (self.name.starts_with('-') || self.name.starts_with('_'))
    }

    // Matches Dart: the hash of the name; the span is excluded.
    pub fn hash_code(&self) -> i32 {
        string_hash_code(&self.name)
    }

    /// Serializes this selector to CSS, quoting as `inspect` requests.
    pub fn to_css_string(&self, inspect: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(false, inspect);
        visitor.visit_placeholder_selector(self)?;
        Ok(visitor.into_string())
    }

    /// Warns when this selector is bogus (custom functions only); a no-op
    /// here since placeholders are never bogus.
    pub fn assert_not_bogus(
        &self,
        name: Option<&str>,
        warn: Option<&mut dyn WarnLogger>,
    ) -> SassResult<()> {
        if !self.is_bogus() {
            return Ok(());
        }
        let serialized = self.to_css_string(true)?;
        selector_assert_not_bogus_impl(true, &serialized, name, warn)
    }

    // Matches Dart `PlaceholderSelector.addSuffix` (placeholder.dart):
    // appends `suffix` to the name. Internal-only in Dart (`@internal`);
    // kept public here.
    pub fn add_suffix(&self, suffix: &str) -> SassResult<SimpleSelector<'parse>> {
        Ok(SimpleSelector::Placeholder(PlaceholderSelector::new(
            format!("{}{}", self.name, suffix),
            self.span,
        )))
    }

    // Matches Dart: a placeholder is a superselector only of the same
    // placeholder name; equality with `other`'s own variant is handled by
    // the caller.
    pub(crate) fn is_superselector_variant(
        &self,
        other: &SimpleSelector<'parse>,
    ) -> SassResult<bool> {
        if let SimpleSelector::Placeholder(ref o) = other {
            return Ok(self.name == o.name);
        }
        Ok(false)
    }

    // Matches Dart `SimpleSelector.unify` (extend/functions.dart): returns a
    // copy of `comps` with this selector inserted before any trailing
    // pseudo selectors.
    pub fn unify(
        &self,
        comps: &[SimpleSelector<'parse>],
    ) -> SassResult<Option<Vec<SimpleSelector<'parse>>>> {
        simple_base_unify(SimpleSelector::Placeholder(self.clone()), comps)
    }
}

// Hashes via `hash_code`, matching Dart's `hashCode`.
impl<'parse> Hash for PlaceholderSelector<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.hash_code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;

    #[test]
    fn test_new_placeholder_selector() {
        let s = PlaceholderSelector::new("foo".into(), BOGUS_SPAN);
        assert_eq!(s.name, "foo");
    }

    #[test]
    fn test_placeholder_selector_is_invisible() {
        let s = PlaceholderSelector::new("foo".into(), BOGUS_SPAN);
        assert!(s.is_invisible());
    }

    #[test]
    fn test_placeholder_selector_is_bogus() {
        let s = PlaceholderSelector::new("foo".into(), BOGUS_SPAN);
        assert!(!s.is_bogus());
    }

    #[test]
    fn test_placeholder_selector_is_private() {
        let s1 = PlaceholderSelector::new("-foo".into(), BOGUS_SPAN);
        assert!(s1.is_private());
        let s2 = PlaceholderSelector::new("_bar".into(), BOGUS_SPAN);
        assert!(s2.is_private());
        let s3 = PlaceholderSelector::new("baz".into(), BOGUS_SPAN);
        assert!(!s3.is_private());
    }

    #[test]
    fn test_placeholder_selector_specificity() {
        let s = PlaceholderSelector::new("foo".into(), BOGUS_SPAN);
        assert_eq!(s.specificity(), 1000);
    }

    #[test]
    fn test_placeholder_selector_add_suffix() {
        let s = PlaceholderSelector::new("foo".into(), BOGUS_SPAN);
        let result = s.add_suffix("bar").unwrap();
        if let SimpleSelector::Placeholder(ref ps) = result {
            assert_eq!(ps.name, "foobar");
        } else {
            panic!("expected PlaceholderSelector");
        }
    }

    #[test]
    fn test_placeholder_selector_partial_eq() {
        let s1 = PlaceholderSelector::new("foo".into(), BOGUS_SPAN);
        let s2 = PlaceholderSelector::new("foo".into(), BOGUS_SPAN);
        assert_eq!(s1, s2);
        let s3 = PlaceholderSelector::new("bar".into(), BOGUS_SPAN);
        assert_ne!(s1, s3);
    }

    #[test]
    fn test_placeholder_selector_hash_code() {
        let s1 = PlaceholderSelector::new("foo".into(), BOGUS_SPAN);
        let s2 = PlaceholderSelector::new("foo".into(), BOGUS_SPAN);
        assert_eq!(s1.hash_code(), s2.hash_code());
    }

    #[test]
    fn test_placeholder_selector_to_css_string() {
        let s = PlaceholderSelector::new("foo".into(), BOGUS_SPAN);
        assert_eq!(s.to_css_string(true).unwrap(), "%foo");
    }

    #[test]
    fn test_placeholder_selector_assert_not_bogus() {
        let s = PlaceholderSelector::new("foo".into(), BOGUS_SPAN);
        assert!(s.assert_not_bogus(None, None).is_ok());
    }
}
