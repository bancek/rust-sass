// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector/complex.dart + lib/src/extend/functions.dart
// go-source: go/value/selector_complex.go + go/value/selector_extend_functions.go

use crate::common::source_span_file_source::FileSource;
use crate::parse::selector_parse::SelectorParser;
use crate::selector::selector_assert_not_bogus_impl;
use crate::selector::weave::complex_is_superselector;
use crate::selector::WarnLogger;
use crate::serialize::SerializeVisitor;
use bumpalo::Bump;
use std::hash::Hash;
use std::hash::Hasher;

use crate::common::ast_css_value::CssValue;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::selector::combinator::Combinator;
use crate::selector::complex_component::ComplexSelectorComponent;
use crate::selector::compound::CompoundSelector;
use crate::selector::visitor::SelectorVisitor;
use crate::url::SassUrl;
use crate::value::hash::hash_combine;

/// A complex selector.
///
/// A complex selector is composed of [`CompoundSelector`]s separated by
/// [`Combinator`]s. It selects elements based on their parent selectors.
#[derive(Clone, Debug)]
pub struct ComplexSelector<'parse> {
    pub span: FileSpan<'parse>,
    /// This selector's leading combinators.
    ///
    /// Empty means no leading combinator. More than one element is invalid
    /// CSS, still supported for backwards-compatibility purposes.
    pub leading_combinators: Vec<CssValue<'parse, Combinator>>,
    /// The components of this selector.
    ///
    /// Only empty when [`ComplexSelector::leading_combinators`] is not empty.
    /// Descendant combinators are implicit between adjacent compounds.
    /// Multiple adjacent [`Combinator`]s are invalid CSS, supported for CSS
    /// hack purposes.
    pub components: Vec<ComplexSelectorComponent<'parse>>,
    // Whether a line break is emitted *before* this selector when serializing.
    pub line_break: bool,
}

impl<'parse> ComplexSelector<'parse> {
    /// Creates a complex selector from leading combinators and components.
    ///
    /// Returns an error if both lists are empty.
    pub fn new(
        leading_combinators: Vec<CssValue<'parse, Combinator>>,
        components: Vec<ComplexSelectorComponent<'parse>>,
        span: FileSpan<'parse>,
        line_break: bool,
    ) -> SassResult<Self> {
        if leading_combinators.is_empty() && components.is_empty() {
            return Err(Box::new(SassError::Script {
                message: "leadingCombinators and components may not both be empty.".into(),
                argument_name: None,
            }));
        }
        Ok(ComplexSelector {
            span,
            leading_combinators: leading_combinators.to_vec(),
            components: components.to_vec(),
            line_break,
        })
    }

    pub fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }

    // Whether this selector, and complex selectors containing it, should not
    // be emitted. Matches Dart: `Selector.isInvisible` (@internal).
    pub fn is_invisible(&self) -> bool {
        if self.components.is_empty() {
            return true;
        }
        for comp in &self.components {
            if comp.selector.is_invisible() {
                return true;
            }
        }
        self.is_bogus_other_than_leading_combinator()
    }

    /// Whether this selector is not valid CSS.
    ///
    /// This includes both selectors useful only for build-time nesting and
    /// selectors with invalid combinators still supported for
    /// backwards-compatibility reasons.
    pub fn is_bogus(&self) -> bool {
        if self.components.is_empty() {
            return !self.leading_combinators.is_empty();
        }
        if !self.leading_combinators.is_empty() {
            return true;
        }
        if let Some(last) = self.components.last() {
            if !last.combinators.is_empty() {
                return true;
            }
        }
        for comp in &self.components {
            if comp.combinators.len() > 1 {
                return true;
            }
            if comp.selector.is_bogus() {
                return true;
            }
        }
        false
    }

    // Whether this is a useless selector: bogus _and_ unable to become valid
    // CSS via `@extend` or nesting. Matches Dart: `Selector.isUseless`
    // (@internal).
    pub fn is_useless(&self) -> bool {
        if self.leading_combinators.len() > 1 {
            return true;
        }
        for comp in &self.components {
            if comp.combinators.len() > 1 {
                return true;
            }
            if comp.selector.is_useless() {
                return true;
            }
        }
        false
    }

    // Whether this contains a parent (`&`) selector.
    // Matches Dart: `Selector.containsParentSelector` (@internal).
    pub fn contains_parent_selector(&self) -> SassResult<bool> {
        for comp in &self.components {
            if comp.selector.contains_parent_selector()? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // Whether this is bogus apart from having a leading combinator.
    // Matches Dart: `Selector.isBogusOtherThanLeadingCombinator` (@internal).
    pub fn is_bogus_other_than_leading_combinator(&self) -> bool {
        if self.components.is_empty() {
            return !self.leading_combinators.is_empty();
        }
        if self.leading_combinators.len() > 1 {
            return true;
        }
        if let Some(last) = self.components.last() {
            if !last.combinators.is_empty() {
                return true;
            }
        }
        for comp in &self.components {
            if comp.combinators.len() > 1 {
                return true;
            }
            if comp.selector.is_bogus() {
                return true;
            }
        }
        false
    }

    // Whether this would be invisible even without bogus combinators.
    // Matches Dart: `Selector.isInvisibleOtherThanBogusCombinators`
    // (@internal).
    pub fn is_invisible_other_than_bogus_combinators(&self) -> bool {
        for comp in &self.components {
            if comp.selector.is_invisible_other_than_bogus_combinators() {
                return true;
            }
        }
        false
    }

    /// This selector's specificity, in base 1000.
    ///
    /// The spec requires "sufficiently high"; no single selector sequence
    /// realistically holds 1000 simple selectors.
    pub fn specificity(&self) -> usize {
        let mut sum = 0;
        for component in &self.components {
            sum += component.selector.specificity();
        }
        sum
    }

    // If this is composed of a single compound selector with no combinators,
    // returns it; otherwise returns `None`.
    // Matches Dart: `ComplexSelector.singleCompound` (@internal).
    pub fn single_compound(&self) -> Option<&CompoundSelector<'parse>> {
        if !self.leading_combinators.is_empty() {
            return None;
        }
        if self.components.len() == 1 {
            let comp = &self.components[0];
            if comp.combinators.is_empty() {
                return Some(&comp.selector);
            }
        }
        None
    }

    pub fn hash_code(&self) -> i32 {
        let mut h = 0;
        for lc in &self.leading_combinators {
            h = hash_combine(h, (lc.value as i32).wrapping_mul(31));
        }
        for comp in &self.components {
            h = hash_combine(h, comp.hash_code());
        }
        h
    }

    pub fn to_css_string(&self, inspect: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(false, inspect);
        visitor.visit_complex_selector(self)?;
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
    /// well as possibly matching more.
    ///
    /// Extend-algorithm docs live on the owner in
    /// [`crate::selector::weave`] (see Dart `extend/functions.dart`).
    pub fn is_superselector(&self, other: &ComplexSelector<'parse>) -> SassResult<bool> {
        if !self.leading_combinators.is_empty() || !other.leading_combinators.is_empty() {
            return Ok(false);
        }
        complex_is_superselector(&self.components, &other.components)
    }

    /// Parses a complex selector from `contents`.
    ///
    /// If passed, `url` names the file `contents` comes from; `allow_parent`
    /// controls whether a parent (`&`) selector is allowed. Returns an error
    /// if parsing fails.
    pub fn parse<'compile: 'parse>(
        arena: &'compile Bump,
        contents: &str,
        url: Option<&SassUrl>,
        allow_parent: bool,
    ) -> SassResult<ComplexSelector<'parse>> {
        let file = FileSource::new_in(arena, contents, url.cloned());
        let mut parser =
            SelectorParser::new_with_options(arena, file, allow_parent, false, None, None, None);
        parser.parse_complex_selector()
    }

    // Returns a copy of `self` with `combinators` added to the end of the
    // final component. `force_line_break` marks the new selector as having a
    // line break. Matches Dart: `ComplexSelector.withAdditionalCombinators`
    // (@internal).
    pub fn with_additional_combinators(
        &self,
        combinators: &[CssValue<'parse, Combinator>],
        force_line_break: bool,
    ) -> Self {
        if combinators.is_empty() {
            return self.clone();
        }
        if !self.components.is_empty() {
            let mut new_components = self.components.clone();
            let last_idx = new_components.len() - 1;
            new_components[last_idx] =
                new_components[last_idx].with_additional_combinators(combinators);
            return ComplexSelector {
                span: self.span,
                leading_combinators: self.leading_combinators.clone(),
                components: new_components,
                line_break: self.line_break || force_line_break,
            };
        }
        let mut new_lc = self.leading_combinators.clone();
        new_lc.extend_from_slice(combinators);
        ComplexSelector {
            span: self.span,
            leading_combinators: new_lc,
            components: vec![],
            line_break: self.line_break || force_line_break,
        }
    }

    // Returns a copy of `self` with an additional `component` appended; `span`
    // is used for the new selector and `force_line_break` marks it as having
    // a line break. Matches Dart: `ComplexSelector.withAdditionalComponent`
    // (@internal).
    pub fn with_additional_component(
        &self,
        component: &ComplexSelectorComponent<'parse>,
        span: FileSpan<'parse>,
        force_line_break: bool,
    ) -> Self {
        let mut new_components = self.components.clone();
        new_components.push(component.clone());
        ComplexSelector {
            span,
            leading_combinators: self.leading_combinators.clone(),
            components: new_components,
            line_break: self.line_break || force_line_break,
        }
    }

    // Returns a copy of `self` with `child`'s combinators added to the end.
    // If `child` has leading combinators they're appended to `self`'s last
    // combinator; parent selectors are _not_ resolved. `span` is used for the
    // new selector. Matches Dart: `ComplexSelector.concatenate` (@internal).
    pub fn concatenate(
        &self,
        child: &ComplexSelector<'parse>,
        span: FileSpan<'parse>,
        force_line_break: bool,
    ) -> Self {
        if child.leading_combinators.is_empty() {
            let mut new_components = self.components.clone();
            new_components.extend_from_slice(&child.components);
            return ComplexSelector {
                span,
                leading_combinators: self.leading_combinators.clone(),
                components: new_components,
                line_break: self.line_break || child.line_break || force_line_break,
            };
        }
        if !self.components.is_empty() {
            let mut new_components = self.components.clone();
            let last_idx = new_components.len() - 1;
            new_components[last_idx] =
                new_components[last_idx].with_additional_combinators(&child.leading_combinators);
            new_components.extend_from_slice(&child.components);
            return ComplexSelector {
                span,
                leading_combinators: self.leading_combinators.clone(),
                components: new_components,
                line_break: self.line_break || child.line_break || force_line_break,
            };
        }
        let mut new_lc = self.leading_combinators.clone();
        new_lc.extend_from_slice(&child.leading_combinators);
        ComplexSelector {
            span,
            leading_combinators: new_lc,
            components: child.components.clone(),
            line_break: self.line_break || child.line_break || force_line_break,
        }
    }
}

impl<'parse> PartialEq for ComplexSelector<'parse> {
    fn eq(&self, other: &Self) -> bool {
        if self.leading_combinators.len() != other.leading_combinators.len() {
            return false;
        }
        for i in 0..self.leading_combinators.len() {
            if self.leading_combinators[i] != other.leading_combinators[i] {
                return false;
            }
        }
        if self.components.len() != other.components.len() {
            return false;
        }
        for i in 0..self.components.len() {
            if self.components[i] != other.components[i] {
                return false;
            }
        }
        true
    }
}

impl<'parse> Eq for ComplexSelector<'parse> {}

impl<'parse> Hash for ComplexSelector<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.hash_code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::selector::class::ClassSelector;
    use crate::selector::SimpleSelector;

    fn make_compound(name: &str) -> Box<CompoundSelector<'static>> {
        let s = SimpleSelector::Class(ClassSelector::new(name.into(), BOGUS_SPAN));
        Box::new(CompoundSelector::new(vec![s], BOGUS_SPAN).unwrap())
    }

    fn make_component(name: &str) -> ComplexSelectorComponent<'static> {
        ComplexSelectorComponent::new(make_compound(name), vec![], BOGUS_SPAN)
    }

    #[test]
    fn test_new() {
        let comp = make_component("foo");
        let cs = ComplexSelector::new(vec![], vec![comp.clone()], BOGUS_SPAN, false).unwrap();
        assert_eq!(cs.components.len(), 1);
        assert_eq!(cs.components[0].selector, comp.selector);
    }

    #[test]
    fn test_new_empty() {
        let result = ComplexSelector::new(vec![], vec![], BOGUS_SPAN, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_invisible() {
        let cs =
            ComplexSelector::new(vec![], vec![make_component("foo")], BOGUS_SPAN, false).unwrap();
        assert!(!cs.is_invisible());
    }

    #[test]
    fn test_is_bogus() {
        let cs =
            ComplexSelector::new(vec![], vec![make_component("foo")], BOGUS_SPAN, false).unwrap();
        assert!(!cs.is_bogus());
    }

    #[test]
    fn test_is_useless() {
        let cs =
            ComplexSelector::new(vec![], vec![make_component("foo")], BOGUS_SPAN, false).unwrap();
        assert!(!cs.is_useless());
    }

    #[test]
    fn test_single_compound() {
        let comp = make_component("foo");
        let cs = ComplexSelector::new(vec![], vec![comp.clone()], BOGUS_SPAN, false).unwrap();
        assert!(cs.single_compound().is_some());
    }

    #[test]
    fn test_specificity() {
        let c1 = make_component("foo");
        let c2 = make_component("bar");
        let cs = ComplexSelector::new(vec![], vec![c1, c2], BOGUS_SPAN, false).unwrap();
        assert_eq!(cs.specificity(), 2000);
    }

    #[test]
    fn test_hash_code() {
        let cs1 =
            ComplexSelector::new(vec![], vec![make_component("foo")], BOGUS_SPAN, false).unwrap();
        let cs2 =
            ComplexSelector::new(vec![], vec![make_component("foo")], BOGUS_SPAN, false).unwrap();
        assert_eq!(cs1.hash_code(), cs2.hash_code());
    }

    #[test]
    fn test_with_additional_combinators() {
        let cs =
            ComplexSelector::new(vec![], vec![make_component("foo")], BOGUS_SPAN, false).unwrap();
        let comb = CssValue::new(Combinator::Child, BOGUS_SPAN);
        let result = cs.with_additional_combinators(&[comb], false);
        assert_eq!(result.components.len(), 1);
        assert_eq!(result.components[0].combinators.len(), 1);
    }

    #[test]
    fn test_concatenate() {
        let cs1 =
            ComplexSelector::new(vec![], vec![make_component("foo")], BOGUS_SPAN, false).unwrap();
        let cs2 =
            ComplexSelector::new(vec![], vec![make_component("bar")], BOGUS_SPAN, false).unwrap();
        let result = cs1.concatenate(&cs2, BOGUS_SPAN, false);
        assert_eq!(result.components.len(), 2);
    }

    #[test]
    fn test_eq() {
        let cs1 =
            ComplexSelector::new(vec![], vec![make_component("foo")], BOGUS_SPAN, false).unwrap();
        let cs2 =
            ComplexSelector::new(vec![], vec![make_component("foo")], BOGUS_SPAN, false).unwrap();
        assert_eq!(cs1, cs2);

        let cs3 = ComplexSelector::new(
            vec![CssValue::new(Combinator::Child, BOGUS_SPAN)],
            vec![make_component("foo")],
            BOGUS_SPAN,
            false,
        )
        .unwrap();
        assert_ne!(cs1, cs3);
    }

    #[test]
    fn test_complex_selector_assert_not_bogus() {
        let cs =
            ComplexSelector::new(vec![], vec![make_component("foo")], BOGUS_SPAN, false).unwrap();
        assert!(cs.assert_not_bogus(None, None).is_ok());
    }

    #[test]
    fn test_parse_simple() {
        let arena = bumpalo::Bump::new();
        let cs = ComplexSelector::parse(&arena, ".foo", None, true).unwrap();
        assert_eq!(cs.components.len(), 1);
    }

    #[test]
    fn test_parse_empty_error() {
        let arena = bumpalo::Bump::new();
        let result = ComplexSelector::parse(&arena, "", None, true);
        assert!(result.is_err());
    }
}
