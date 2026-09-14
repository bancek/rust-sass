// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector/universal.dart + lib/src/extend/functions.dart
// go-source: go/value/selector_universal.go + go/value/selector_extend_functions.go

use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::selector::selector_assert_not_bogus_impl;
use crate::selector::unify::unify_universal_and_element;
use crate::selector::visitor::SelectorVisitor;
use crate::selector::WarnLogger;
use crate::serialize::SerializeVisitor;
use crate::value::hash::string_opt_hash_code;
use std::hash::Hash;
use std::hash::Hasher;

use crate::selector::SimpleSelector;

/// Matches any element in the given namespace.
#[derive(Clone, Debug)]
pub struct UniversalSelector<'parse> {
    /// The source span covering this selector.
    pub span: FileSpan<'parse>,
    /// The selector namespace.
    ///
    /// `None` matches all elements in the default namespace; `Some("")`
    /// matches all elements in no namespace; `Some("*")` matches all elements
    /// in any namespace; otherwise it matches all elements in the given
    /// namespace.
    pub namespace: Option<String>,
}

// Matches Dart: structural equality over the namespace; the span is
// excluded.
impl<'parse> PartialEq for UniversalSelector<'parse> {
    fn eq(&self, other: &Self) -> bool {
        self.namespace == other.namespace
    }
}

impl<'parse> Eq for UniversalSelector<'parse> {}

impl<'parse> UniversalSelector<'parse> {
    /// Creates a universal selector with the given namespace.
    pub fn new(span: FileSpan<'parse>, namespace: Option<String>) -> Self {
        UniversalSelector { span, namespace }
    }

    /// The source span of this selector.
    pub fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }

    // Matches Dart: universal selectors are always emitted, never invisible.
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

    // Matches Dart: universal selectors never contain a parent selector.
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

    /// This selector's specificity: 0.
    pub fn specificity(&self) -> usize {
        0
    }

    // Matches Dart: plain namespace matching needs no non-local reasoning.
    pub fn has_complicated_superselector_semantics(&self) -> bool {
        false
    }

    // Matches Dart: the hash of the namespace; the span is excluded.
    pub fn hash_code(&self) -> i32 {
        string_opt_hash_code(&self.namespace)
    }

    /// Serializes this selector to CSS, quoting as `inspect` requests.
    pub fn to_css_string(&self, inspect: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(false, inspect);
        visitor.visit_universal_selector(self)?;
        Ok(visitor.into_string())
    }

    /// Warns when this selector is bogus (custom functions only); a no-op
    /// here since universal selectors are never bogus.
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

    // Matches Dart: the universal selector cannot take a suffix.
    pub fn add_suffix(&self, _suffix: &str) -> SassResult<SimpleSelector<'parse>> {
        Err(Box::new(SassError::Script {
            message: format!(
                "Selector \"{}\" can't have a suffix",
                self.to_css_string(true)?
            ),
            argument_name: None,
        }))
    }

    // Matches Dart `UniversalSelector.isSuperselector` (universal.dart): a
    // wildcard namespace matches everything; otherwise compares namespaces
    // against type/universal targets, and a missing namespace matches
    // anything still unmatched. Equality with `other`'s own variant is
    // handled by the caller.
    pub(crate) fn is_superselector_variant(
        &self,
        other: &SimpleSelector<'parse>,
    ) -> SassResult<bool> {
        if self.namespace.as_deref() == Some("*") {
            return Ok(true);
        }
        match other {
            SimpleSelector::Type(ref t) => {
                return Ok(self.namespace == t.name.namespace);
            }
            SimpleSelector::Universal(ref u) => {
                return Ok(self.namespace == u.namespace);
            }
            _ => {}
        }
        if self.namespace.is_none() {
            return Ok(true);
        }
        Ok(false)
    }

    // Matches Dart `UniversalSelector.unify` (universal.dart): merges with a
    // leading universal or type selector via `unify_universal_and_element`
    // (extend/functions.dart); refuses `:host` compounds; a bare or wildcard
    // namespace absorbs `comps`; otherwise prepends `self`. Internal-only in
    // Dart (`@internal`); kept public here.
    pub fn unify(
        &self,
        comps: &[SimpleSelector<'parse>],
    ) -> SassResult<Option<Vec<SimpleSelector<'parse>>>> {
        if !comps.is_empty() {
            match &comps[0] {
                SimpleSelector::Universal(_) | SimpleSelector::Type(_) => {
                    let other = &comps[0];
                    let s1 = SimpleSelector::Universal(self.clone());
                    if let Some(unified) = unify_universal_and_element(&s1, other, self.span)? {
                        let mut result = Vec::with_capacity(comps.len());
                        result.push(unified);
                        result.extend_from_slice(&comps[1..]);
                        return Ok(Some(result));
                    }
                    return Ok(None);
                }
                _ => {}
            }
        }
        if comps.len() == 1 {
            if let SimpleSelector::Pseudo(ref ps) = comps[0] {
                if ps.is_host() || ps.is_host_context() {
                    return Ok(None);
                }
            }
        }
        if comps.is_empty() {
            return Ok(Some(vec![SimpleSelector::Universal(self.clone())]));
        }
        if self.namespace.is_none() || self.namespace.as_deref() == Some("*") {
            return Ok(Some(comps.to_vec()));
        }
        let mut result = Vec::with_capacity(comps.len() + 1);
        result.push(SimpleSelector::Universal(self.clone()));
        result.extend_from_slice(comps);
        Ok(Some(result))
    }
}

// Hashes via `hash_code`, matching Dart's `hashCode`.
impl<'parse> Hash for UniversalSelector<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.hash_code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::selector::class::ClassSelector;
    use crate::selector::qualified_name::QualifiedName;
    use crate::selector::ty::TypeSelector;

    #[test]
    fn test_new_universal_selector() {
        let s = UniversalSelector::new(BOGUS_SPAN, None);
        assert_eq!(s.namespace, None);
    }

    #[test]
    fn test_new_universal_selector_with_namespace() {
        let s = UniversalSelector::new(BOGUS_SPAN, Some("svg".into()));
        assert_eq!(s.namespace, Some("svg".into()));
    }

    #[test]
    fn test_universal_selector_is_invisible() {
        let s = UniversalSelector::new(BOGUS_SPAN, None);
        assert!(!s.is_invisible());
    }

    #[test]
    fn test_universal_selector_is_bogus() {
        let s = UniversalSelector::new(BOGUS_SPAN, None);
        assert!(!s.is_bogus());
    }

    #[test]
    fn test_universal_selector_specificity() {
        let s = UniversalSelector::new(BOGUS_SPAN, None);
        assert_eq!(s.specificity(), 0);
    }

    #[test]
    fn test_universal_selector_hash_code() {
        let s1 = UniversalSelector::new(BOGUS_SPAN, None);
        let s2 = UniversalSelector::new(BOGUS_SPAN, None);
        assert_eq!(s1.hash_code(), s2.hash_code());
    }

    #[test]
    fn test_universal_selector_partial_eq() {
        let s1 = UniversalSelector::new(BOGUS_SPAN, None);
        let s2 = UniversalSelector::new(BOGUS_SPAN, None);
        assert_eq!(s1, s2);

        let s3 = UniversalSelector::new(BOGUS_SPAN, Some("svg".into()));
        assert_ne!(s1, s3);

        let s4 = UniversalSelector::new(BOGUS_SPAN, Some("svg".into()));
        assert_eq!(s3, s4);
    }

    #[test]
    fn test_universal_selector_is_superselector_wildcard_ns() {
        let s = UniversalSelector::new(BOGUS_SPAN, Some("*".into()));
        let target = SimpleSelector::Type(TypeSelector::new(
            QualifiedName::new("div".into()),
            BOGUS_SPAN,
        ));
        assert!(s.is_superselector_variant(&target).unwrap());
    }

    #[test]
    fn test_universal_selector_is_superselector_nil_ns() {
        let s = UniversalSelector::new(BOGUS_SPAN, None);
        let target = SimpleSelector::Class(ClassSelector::new("foo".into(), BOGUS_SPAN));
        assert!(s.is_superselector_variant(&target).unwrap());
    }

    #[test]
    fn test_universal_selector_to_css_string() {
        let s = UniversalSelector::new(BOGUS_SPAN, None);
        assert_eq!(s.to_css_string(true).unwrap(), "*");
    }

    #[test]
    fn test_universal_selector_to_css_string_with_ns() {
        let s = UniversalSelector::new(BOGUS_SPAN, Some("svg".into()));
        assert_eq!(s.to_css_string(true).unwrap(), "svg|*");
    }

    #[test]
    fn test_universal_selector_assert_not_bogus() {
        let s = UniversalSelector::new(BOGUS_SPAN, None);
        assert!(s.assert_not_bogus(None, None).is_ok());
    }
}
