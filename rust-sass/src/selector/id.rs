// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector/id.dart + lib/src/extend/functions.dart
// go-source: go/value/selector_id.go + go/value/selector_extend_functions.go

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

/// An ID selector.
///
/// Selects elements whose `id` attribute exactly matches the given name.
#[derive(Clone, Debug)]
pub struct IdSelector<'parse> {
    /// The source span covering this selector.
    pub span: FileSpan<'parse>,
    /// The ID name this selects for.
    pub name: String,
}

// Matches Dart: structural equality over the name; the span is excluded.
impl<'parse> PartialEq for IdSelector<'parse> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl<'parse> Eq for IdSelector<'parse> {}

impl<'parse> IdSelector<'parse> {
    /// Creates an ID selector for `name`.
    pub fn new(name: String, span: FileSpan<'parse>) -> Self {
        IdSelector { span, name }
    }

    /// The source span of this selector.
    pub fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }

    // Matches Dart: ID selectors are always emitted, never invisible.
    pub fn is_invisible(&self) -> bool {
        false
    }

    /// Whether this selector is not valid CSS; always `false` here.
    pub fn is_bogus(&self) -> bool {
        false
    }

    // Matches Dart: never useless — valid CSS on its own.
    pub fn is_useless(&self) -> bool {
        false
    }

    // Matches Dart: ID selectors never contain a parent selector.
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

    /// This selector's specificity: 1,000,000 — the base specificity squared.
    pub fn specificity(&self) -> usize {
        1_000_000
    }

    // Matches Dart: plain ID matching needs no non-local reasoning.
    pub fn has_complicated_superselector_semantics(&self) -> bool {
        false
    }

    // Matches Dart: the hash of the name; the span is excluded.
    pub fn hash_code(&self) -> i32 {
        string_hash_code(&self.name)
    }

    /// Serializes this selector to CSS, quoting as `inspect` requests.
    pub fn to_css_string(&self, inspect: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(false, inspect);
        visitor.visit_id_selector(self)?;
        Ok(visitor.into_string())
    }

    /// Warns when this selector is bogus (custom functions only); a no-op
    /// here since ID selectors are never bogus.
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

    // Matches Dart `IDSelector.addSuffix` (id.dart): appends `suffix` to the
    // name. Internal-only in Dart (`@internal`); kept public here.
    pub fn add_suffix(&self, suffix: &str) -> SassResult<SimpleSelector<'parse>> {
        Ok(SimpleSelector::Id(IdSelector::new(
            format!("{}{}", self.name, suffix),
            self.span,
        )))
    }

    // Matches Dart: an ID is a superselector only of the same ID name;
    // equality with `other`'s own variant is handled by the caller.
    pub(crate) fn is_superselector_variant(
        &self,
        other: &SimpleSelector<'parse>,
    ) -> SassResult<bool> {
        if let SimpleSelector::Id(ref o) = other {
            return Ok(self.name == o.name);
        }
        Ok(false)
    }

    // Matches Dart `IDSelector.unify` (id.dart): a compound may hold only
    // one ID, so unification fails when `comps` contains a different ID;
    // otherwise falls back to the base insertion behavior.
    pub fn unify(
        &self,
        comps: &[SimpleSelector<'parse>],
    ) -> SassResult<Option<Vec<SimpleSelector<'parse>>>> {
        for other in comps {
            if let SimpleSelector::Id(ref o) = other {
                if !o.eq(self) {
                    return Ok(None);
                }
            }
        }
        simple_base_unify(SimpleSelector::Id(self.clone()), comps)
    }
}

// Hashes via `hash_code`, matching Dart's `hashCode`.
impl<'parse> Hash for IdSelector<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.hash_code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;

    #[test]
    fn test_new_id_selector() {
        let s = IdSelector::new("main".into(), BOGUS_SPAN);
        assert_eq!(s.name, "main");
    }

    #[test]
    fn test_id_selector_is_invisible() {
        let s = IdSelector::new("main".into(), BOGUS_SPAN);
        assert!(!s.is_invisible());
    }

    #[test]
    fn test_id_selector_is_bogus() {
        let s = IdSelector::new("main".into(), BOGUS_SPAN);
        assert!(!s.is_bogus());
    }

    #[test]
    fn test_id_selector_specificity() {
        let s = IdSelector::new("main".into(), BOGUS_SPAN);
        assert_eq!(s.specificity(), 1_000_000);
    }

    #[test]
    fn test_id_selector_add_suffix() {
        let s = IdSelector::new("main".into(), BOGUS_SPAN);
        let result = s.add_suffix("-suffix").unwrap();
        if let SimpleSelector::Id(ref is_) = result {
            assert_eq!(is_.name, "main-suffix");
        } else {
            panic!("expected IdSelector");
        }
    }

    #[test]
    fn test_id_selector_partial_eq() {
        let s1 = IdSelector::new("main".into(), BOGUS_SPAN);
        let s2 = IdSelector::new("main".into(), BOGUS_SPAN);
        assert_eq!(s1, s2);
        let s3 = IdSelector::new("other".into(), BOGUS_SPAN);
        assert_ne!(s1, s3);
    }

    #[test]
    fn test_id_selector_hash_code() {
        let s1 = IdSelector::new("main".into(), BOGUS_SPAN);
        let s2 = IdSelector::new("main".into(), BOGUS_SPAN);
        assert_eq!(s1.hash_code(), s2.hash_code());
    }

    #[test]
    fn test_id_selector_to_css_string() {
        let s = IdSelector::new("foo".into(), BOGUS_SPAN);
        assert_eq!(s.to_css_string(true).unwrap(), "#foo");
    }

    #[test]
    fn test_id_selector_assert_not_bogus() {
        let s = IdSelector::new("foo".into(), BOGUS_SPAN);
        assert!(s.assert_not_bogus(None, None).is_ok());
    }
}
