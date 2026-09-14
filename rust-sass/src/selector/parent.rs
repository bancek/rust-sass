// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector/parent.dart + lib/src/extend/functions.dart
// go-source: go/value/selector_parent.go + go/value/selector_extend_functions.go

use crate::selector::selector_assert_not_bogus_impl;
use crate::selector::WarnLogger;
use crate::serialize::SerializeVisitor;
use std::hash::{Hash, Hasher};

use bumpalo::Bump;

use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::selector::visitor::SelectorVisitor;

use crate::selector::SimpleSelector;

// The arena-owned payload. Held behind a `Copy` handle (below) so parent
// selectors compare by allocation identity, matching Dart's default
// `Object.==`.
#[derive(Debug)]
pub struct ParentSelectorInner<'parse> {
    /// The source span of the `&`.
    pub span: FileSpan<'parse>,
    /// The suffix appended to the resolved parent selector, or `None` when
    /// the parent is used unmodified. Assumed to be a valid identifier
    /// suffix.
    pub suffix: Option<String>,
}

/// A selector matching the parent in the Sass stylesheet.
///
/// This is not plain CSS — it must be resolved away before a CSS document
/// is emitted. A `Copy` arena handle; distinct allocations never compare
/// equal.
#[derive(Clone, Copy, Debug)]
pub struct ParentSelector<'parse>(&'parse ParentSelectorInner<'parse>);

impl<'parse> ParentSelector<'parse> {
    /// Allocates a parent selector into the compile arena.
    pub fn new<'compile: 'parse>(
        arena: &'compile Bump,
        span: FileSpan<'parse>,
        suffix: Option<String>,
    ) -> Self {
        ParentSelector(arena.alloc(ParentSelectorInner { span, suffix }))
    }

    /// The source span of this selector.
    pub fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.0.span)
    }

    /// The suffix added to the parent selector once resolved, if any.
    pub fn suffix(&self) -> Option<&str> {
        self.0.suffix.as_deref()
    }

    // Matches Dart: parent selectors are resolved before emission, but are
    // not themselves invisible.
    pub fn is_invisible(&self) -> bool {
        false
    }

    /// Whether this selector is not valid CSS; always `false` here.
    pub fn is_bogus(&self) -> bool {
        false
    }

    // Matches Dart: never useless.
    pub fn is_useless(&self) -> bool {
        false
    }

    // Matches Dart: a parent selector always contains one by definition.
    pub fn contains_parent_selector(&self) -> SassResult<bool> {
        Ok(true)
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

    // Matches Dart: needs no non-local superselector reasoning.
    pub fn has_complicated_superselector_semantics(&self) -> bool {
        false
    }

    // Matches Dart: identity hash — distinct instances hash distinctly,
    // mirroring Dart's default `Object.hashCode`.
    pub fn hash_code(&self) -> i32 {
        // let ptr = self.0 as *const ParentSelectorInner<'parse> as usize;
        // (ptr ^ (ptr >> 32)) as i32
        self.0 as *const ParentSelectorInner<'parse> as i32
    }

    /// Serializes this selector to CSS, quoting as `inspect` requests.
    pub fn to_css_string(&self, inspect: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(false, inspect);
        visitor.visit_parent_selector(self)?;
        Ok(visitor.into_string())
    }

    /// Warns when this selector is bogus (custom functions only); a no-op
    /// here since parent selectors are never bogus.
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

    // Matches Dart: the suffix is resolved during parent resolution, so it
    // cannot be extended with another suffix here.
    pub fn add_suffix(&self, _suffix: &str) -> SassResult<SimpleSelector<'parse>> {
        Err(Box::new(SassError::Script {
            message: "parent selector cannot have a suffix".into(),
            argument_name: None,
        }))
    }

    // Matches Dart: no variant-specific rule — equality with `other` is
    // handled by the caller, so this is always false.
    pub(crate) fn is_superselector_variant(
        &self,
        _other: &SimpleSelector<'parse>,
    ) -> SassResult<bool> {
        Ok(false)
    }

    // Matches Dart `ParentSelector.unify` (parent.dart): `&` never supports
    // unification.
    pub fn unify(
        &self,
        _comps: &[SimpleSelector<'parse>],
    ) -> SassResult<Option<Vec<SimpleSelector<'parse>>>> {
        Err(Box::new(SassError::Script {
            message: "& doesn't support unification.".into(),
            argument_name: None,
        }))
    }
}

// Matches Dart: identity equality by allocation address; two parent
// selectors with equal suffixes still compare unequal.
impl<'parse> PartialEq for ParentSelector<'parse> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl<'parse> Eq for ParentSelector<'parse> {}

// Matches Dart: identity (address) hash, mirroring `hash_code`.
impl<'parse> Hash for ParentSelector<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.0 as *const ParentSelectorInner<'parse>).hash(state);
    }
}

#[cfg(test)]
mod tests {
    use bumpalo::Bump;
    use std::collections::HashMap;

    use super::*;
    use crate::common::file_span::BOGUS_SPAN;

    #[test]
    fn test_new_parent_selector() {
        let arena = Bump::new();
        let s = ParentSelector::new(&arena, BOGUS_SPAN, None);
        assert_eq!(s.suffix(), None);
    }

    #[test]
    fn test_parent_selector_with_suffix() {
        let arena = Bump::new();
        let s = ParentSelector::new(&arena, BOGUS_SPAN, Some("suffix".into()));
        assert_eq!(s.suffix(), Some("suffix"));
    }

    #[test]
    fn test_parent_selector_contains_parent_selector() {
        let arena = Bump::new();
        let s = ParentSelector::new(&arena, BOGUS_SPAN, None);
        assert!(s.contains_parent_selector().unwrap());
    }

    #[test]
    fn test_parent_selector_is_invisible() {
        let arena = Bump::new();
        let s = ParentSelector::new(&arena, BOGUS_SPAN, None);
        assert!(!s.is_invisible());
    }

    #[test]
    fn test_parent_selector_is_bogus() {
        let arena = Bump::new();
        let s = ParentSelector::new(&arena, BOGUS_SPAN, None);
        assert!(!s.is_bogus());
    }

    #[test]
    fn test_parent_selector_specificity() {
        let arena = Bump::new();
        let s = ParentSelector::new(&arena, BOGUS_SPAN, None);
        assert_eq!(s.specificity(), 1000);
    }

    #[test]
    fn test_parent_selector_add_suffix() {
        let arena = Bump::new();
        let s = ParentSelector::new(&arena, BOGUS_SPAN, None);
        assert!(s.add_suffix("x").is_err());
    }

    #[test]
    fn test_parent_identity_different_rc_not_equal() {
        let arena = Bump::new();
        let s1 = ParentSelector::new(&arena, BOGUS_SPAN, None);
        let s2 = ParentSelector::new(&arena, BOGUS_SPAN, None);
        assert_ne!(s1, s2, "different Rc should be != per Dart identity");
    }

    #[test]
    fn test_parent_identity_same_rc_equal() {
        let arena = Bump::new();
        let s1 = ParentSelector::new(&arena, BOGUS_SPAN, None);
        let s2 = s1;
        assert_eq!(s1, s2, "clone shares identity, should be ==");
        assert_eq!(s1.hash_code(), s2.hash_code());
    }

    #[test]
    fn test_parent_identity_different_rc_same_suffix_not_equal() {
        let arena = Bump::new();
        let s1 = ParentSelector::new(&arena, BOGUS_SPAN, Some("x".into()));
        let s2 = ParentSelector::new(&arena, BOGUS_SPAN, Some("x".into()));
        assert_ne!(
            s1, s2,
            "different Rc with same suffix should be != (Dart identity)"
        );
    }

    #[test]
    // Same identity-key rationale as `test_selector_list_identity_hashmap_key`.
    #[allow(clippy::mutable_key_type)]
    fn test_parent_hashmap_key() {
        let arena = Bump::new();
        let mut map: HashMap<ParentSelector, i32> = HashMap::new();
        let key = ParentSelector::new(&arena, BOGUS_SPAN, Some("&".into()));
        map.insert(key, 1);
        assert_eq!(map.get(&key), Some(&1));
        let different = ParentSelector::new(&arena, BOGUS_SPAN, Some("&".into()));
        assert_eq!(
            map.get(&different),
            None,
            "different Rc should not be found"
        );
    }

    #[test]
    fn test_parent_selector_to_css_string() {
        let arena = Bump::new();
        let s = ParentSelector::new(&arena, BOGUS_SPAN, None);
        assert_eq!(s.to_css_string(true).unwrap(), "&");
    }

    #[test]
    fn test_parent_selector_to_css_string_with_suffix() {
        let arena = Bump::new();
        let s = ParentSelector::new(&arena, BOGUS_SPAN, Some("my-suffix".into()));
        assert_eq!(s.to_css_string(true).unwrap(), "&my-suffix");
    }

    #[test]
    fn test_parent_selector_assert_not_bogus() {
        let arena = Bump::new();
        let s = ParentSelector::new(&arena, BOGUS_SPAN, None);
        assert!(s.assert_not_bogus(None, None).is_ok());
    }
}
