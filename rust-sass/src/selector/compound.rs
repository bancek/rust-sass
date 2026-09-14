// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector/compound.dart + lib/src/extend/functions.dart
// go-source: go/value/selector_compound.go + go/value/selector_extend_functions.go

use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::selector::selector_assert_not_bogus_impl;
use crate::selector::visitor::SelectorVisitor;
use crate::selector::weave::compound_is_superselector;
use crate::selector::SimpleSelector;
use crate::selector::WarnLogger;
use crate::serialize::SerializeVisitor;
use crate::value::hash::hash_combine;
use std::hash::Hash;
use std::hash::Hasher;

/// A compound selector.
///
/// A compound selector is composed of [`SimpleSelector`](super::SimpleSelector)s.
/// It matches an element that matches all of the component simple selectors.
#[derive(Clone, Debug)]
pub struct CompoundSelector<'parse> {
    pub span: FileSpan<'parse>,
    /// The components of this selector.
    ///
    /// This is never empty.
    pub components: Vec<SimpleSelector<'parse>>,
}

impl<'parse> PartialEq for CompoundSelector<'parse> {
    fn eq(&self, other: &Self) -> bool {
        self.components == other.components
    }
}

impl<'parse> Eq for CompoundSelector<'parse> {}

impl<'parse> CompoundSelector<'parse> {
    /// Creates a compound selector from simple-selector components.
    ///
    /// Returns an error if `components` is empty.
    pub fn new(
        components: Vec<SimpleSelector<'parse>>,
        span: FileSpan<'parse>,
    ) -> SassResult<Self> {
        if components.is_empty() {
            return Err(Box::new(SassError::Script {
                message: "components may not be empty".into(),
                argument_name: None,
            }));
        }
        Ok(CompoundSelector {
            span,
            components: components.to_vec(),
        })
    }

    pub fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }

    // Whether this selector, and complex selectors containing it, should not
    // be emitted. Matches Dart: `Selector.isInvisible` (@internal).
    pub fn is_invisible(&self) -> bool {
        for comp in &self.components {
            if comp.is_invisible() {
                return true;
            }
        }
        false
    }

    /// Whether this selector is not valid CSS.
    ///
    /// Matches Dart: `Selector.isBogus`.
    pub fn is_bogus(&self) -> bool {
        for comp in &self.components {
            if comp.is_bogus() {
                return true;
            }
        }
        false
    }

    // Whether this is a useless selector: bogus _and_ unable to become valid
    // CSS via `@extend` or nesting. Matches Dart: `Selector.isUseless`
    // (@internal).
    pub fn is_useless(&self) -> bool {
        for comp in &self.components {
            if comp.is_useless() {
                return true;
            }
        }
        false
    }

    // Whether this contains a parent (`&`) selector.
    // Matches Dart: `Selector.containsParentSelector` (@internal).
    pub fn contains_parent_selector(&self) -> SassResult<bool> {
        for s in &self.components {
            if s.contains_parent_selector()? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // Whether this would be invisible even without bogus combinators.
    // Matches Dart: `Selector.isInvisibleOtherThanBogusCombinators`
    // (@internal).
    pub fn is_invisible_other_than_bogus_combinators(&self) -> bool {
        for comp in &self.components {
            if comp.is_invisible_other_than_bogus_combinators() {
                return true;
            }
        }
        false
    }

    // Whether this is bogus apart from having a leading combinator.
    // Matches Dart: `Selector.isBogusOtherThanLeadingCombinator` (@internal).
    pub fn is_bogus_other_than_leading_combinator(&self) -> bool {
        self.is_bogus()
    }

    /// This selector's specificity, in base 1000.
    ///
    /// The spec requires "sufficiently high"; no single selector sequence
    /// realistically holds 1000 simple selectors.
    pub fn specificity(&self) -> usize {
        let mut sum = 0;
        for component in &self.components {
            sum += component.specificity();
        }
        sum
    }

    // If this is composed of a single simple selector, returns it; otherwise
    // returns `None`. Matches Dart: `CompoundSelector.singleSimple`
    // (@internal).
    pub fn single_simple(&self) -> Option<&SimpleSelector<'parse>> {
        if self.components.len() == 1 {
            Some(&self.components[0])
        } else {
            None
        }
    }

    // Whether any simple selector in this requires complex non-local
    // reasoning to determine super-/sub-selector relationships: pseudo-elements
    // and pseudo-selectors taking selector arguments.
    // Matches Dart: `CompoundSelector.hasComplicatedSuperselectorSemantics`
    // (@internal).
    pub fn has_complicated_superselector_semantics(&self) -> bool {
        for component in &self.components {
            if component.has_complicated_superselector_semantics() {
                return true;
            }
        }
        false
    }

    pub fn hash_code(&self) -> i32 {
        let mut h = 0;
        for comp in &self.components {
            h = hash_combine(h, comp.hash_code());
        }
        h
    }

    pub fn to_css_string(&self, inspect: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(false, inspect);
        visitor.visit_compound_selector(self)?;
        Ok(visitor.into_string())
    }

    /// Emits a warning if `self` is a bogus selector.
    ///
    /// May only be called from within a custom Sass function; this becomes an
    /// error in Dart Sass 2.0.0.
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

    /// Whether this is a superselector of `other`.
    ///
    /// That is, whether this matches every element that `other` matches, as
    /// well as possibly additional elements.
    ///
    /// Extend-algorithm docs live on the owner in
    /// [`crate::selector::weave`] (see Dart `extend/functions.dart`).
    pub fn is_superselector(&self, other: &CompoundSelector<'parse>) -> SassResult<bool> {
        compound_is_superselector(self, other, None)
    }
}

impl<'parse> Hash for CompoundSelector<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.hash_code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::selector::class::ClassSelector;
    use crate::selector::id::IdSelector;
    use crate::selector::parent::ParentSelector;
    use crate::selector::placeholder::PlaceholderSelector;
    use crate::selector::pseudo::PseudoSelector;
    use crate::selector::SimpleSelector;
    use bumpalo::Bump;

    fn cls(name: &str) -> SimpleSelector<'static> {
        SimpleSelector::Class(ClassSelector::new(name.into(), BOGUS_SPAN))
    }

    fn id(name: &str) -> SimpleSelector<'static> {
        SimpleSelector::Id(IdSelector::new(name.into(), BOGUS_SPAN))
    }

    #[test]
    fn test_new_compound_selector() {
        let c = CompoundSelector::new(vec![cls("foo")], BOGUS_SPAN).unwrap();
        assert_eq!(c.components.len(), 1);
    }

    #[test]
    fn test_new_compound_selector_empty() {
        let result = CompoundSelector::new(vec![], BOGUS_SPAN);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_invisible() {
        let c = CompoundSelector::new(vec![cls("foo")], BOGUS_SPAN).unwrap();
        assert!(!c.is_invisible());

        let ph = SimpleSelector::Placeholder(PlaceholderSelector::new("foo".into(), BOGUS_SPAN));
        let c2 = CompoundSelector::new(vec![ph], BOGUS_SPAN).unwrap();
        assert!(c2.is_invisible());
    }

    #[test]
    fn test_is_bogus() {
        let c = CompoundSelector::new(vec![cls("foo")], BOGUS_SPAN).unwrap();
        assert!(!c.is_bogus());
    }

    #[test]
    fn test_is_useless() {
        let c = CompoundSelector::new(vec![cls("foo")], BOGUS_SPAN).unwrap();
        assert!(!c.is_useless());
    }

    #[test]
    fn test_contains_parent_selector() {
        let arena = Bump::new();
        let c = CompoundSelector::new(vec![cls("foo")], BOGUS_SPAN).unwrap();
        assert!(!c.contains_parent_selector().unwrap());

        let parent = SimpleSelector::Parent(ParentSelector::new(&arena, BOGUS_SPAN, None));
        let c2 = CompoundSelector::new(vec![parent], BOGUS_SPAN).unwrap();
        assert!(c2.contains_parent_selector().unwrap());
    }

    #[test]
    fn test_single_simple() {
        let c = CompoundSelector::new(vec![cls("foo")], BOGUS_SPAN).unwrap();
        assert!(c.single_simple().is_some());

        let c2 = CompoundSelector::new(vec![cls("foo"), id("bar")], BOGUS_SPAN).unwrap();
        assert!(c2.single_simple().is_none());
    }

    #[test]
    fn test_specificity() {
        let c = CompoundSelector::new(vec![cls("foo"), id("bar")], BOGUS_SPAN).unwrap();
        assert_eq!(c.specificity(), 1001000);
    }

    #[test]
    fn test_has_complicated_superselector_semantics() {
        let c = CompoundSelector::new(vec![cls("foo")], BOGUS_SPAN).unwrap();
        assert!(!c.has_complicated_superselector_semantics());

        let pe = SimpleSelector::Pseudo(PseudoSelector::new(
            "before".into(),
            BOGUS_SPAN,
            true,
            None,
            None,
        ));
        let c2 = CompoundSelector::new(vec![pe], BOGUS_SPAN).unwrap();
        assert!(c2.has_complicated_superselector_semantics());
    }

    #[test]
    fn test_hash_code() {
        let c1 = CompoundSelector::new(vec![cls("foo")], BOGUS_SPAN).unwrap();
        let c2 = CompoundSelector::new(vec![cls("foo")], BOGUS_SPAN).unwrap();
        assert_eq!(c1.hash_code(), c2.hash_code());
    }

    #[test]
    fn test_compound_selector_to_css_string() {
        let c = CompoundSelector::new(vec![cls("foo")], BOGUS_SPAN).unwrap();
        assert_eq!(c.to_css_string(true).unwrap(), ".foo");
    }

    #[test]
    fn test_compound_selector_to_css_string_multi() {
        let c = CompoundSelector::new(
            vec![
                cls("foo"),
                SimpleSelector::Id(IdSelector::new("bar".into(), BOGUS_SPAN)),
            ],
            BOGUS_SPAN,
        )
        .unwrap();
        assert_eq!(c.to_css_string(true).unwrap(), ".foo#bar");
    }

    #[test]
    fn test_compound_selector_assert_not_bogus() {
        let c = CompoundSelector::new(vec![cls("foo")], BOGUS_SPAN).unwrap();
        assert!(c.assert_not_bogus(None, None).is_ok());
    }
}
