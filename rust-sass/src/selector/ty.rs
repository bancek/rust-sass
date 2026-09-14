// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector/type.dart + lib/src/extend/functions.dart
// go-source: go/value/selector_type.go + go/value/selector_extend_functions.go

use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::selector::qualified_name::QualifiedName;
use crate::selector::selector_assert_not_bogus_impl;
use crate::selector::unify::unify_universal_and_element;
use crate::selector::visitor::SelectorVisitor;
use crate::selector::WarnLogger;
use crate::serialize::SerializeVisitor;
use crate::value::hash::{hash_combine, string_hash_code, string_opt_hash_code};
use std::hash::Hash;
use std::hash::Hasher;

use crate::selector::SimpleSelector;

/// A type selector.
///
/// Selects elements whose name equals the given name.
#[derive(Clone, Debug)]
pub struct TypeSelector<'parse> {
    /// The source span covering this selector.
    pub span: FileSpan<'parse>,
    /// The element name being selected.
    pub name: QualifiedName,
}

// Matches Dart: structural equality over the name; the span is excluded.
impl<'parse> PartialEq for TypeSelector<'parse> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl<'parse> Eq for TypeSelector<'parse> {}

impl<'parse> TypeSelector<'parse> {
    /// Creates a type selector for `name`.
    pub fn new(name: QualifiedName, span: FileSpan<'parse>) -> Self {
        TypeSelector { span, name }
    }

    /// The source span of this selector.
    pub fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }

    // Matches Dart: type selectors are always emitted, never invisible.
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

    // Matches Dart: type selectors never contain a parent selector.
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

    /// This selector's specificity: 1.
    pub fn specificity(&self) -> usize {
        1
    }

    // Matches Dart: plain type matching needs no non-local reasoning.
    pub fn has_complicated_superselector_semantics(&self) -> bool {
        false
    }

    // Matches Dart: the hash of the qualified name; the span is excluded.
    pub fn hash_code(&self) -> i32 {
        hash_combine(
            string_hash_code(&self.name.name),
            string_opt_hash_code(&self.name.namespace),
        )
    }

    /// Serializes this selector to CSS, quoting as `inspect` requests.
    pub fn to_css_string(&self, inspect: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(false, inspect);
        visitor.visit_type_selector(self)?;
        Ok(visitor.into_string())
    }

    /// Warns when this selector is bogus (custom functions only); a no-op
    /// here since type selectors are never bogus.
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

    // Matches Dart `TypeSelector.addSuffix` (type.dart): appends `suffix`
    // to the name, keeping the namespace. Internal-only in Dart
    // (`@internal`); kept public here.
    pub fn add_suffix(&self, suffix: &str) -> SassResult<SimpleSelector<'parse>> {
        Ok(SimpleSelector::Type(TypeSelector {
            span: self.span,
            name: QualifiedName::new_with_namespace(
                format!("{}{}", self.name.name, suffix),
                self.name.namespace.clone(),
            ),
        }))
    }

    // Matches Dart `TypeSelector.isSuperselector` (type.dart): same element
    // name, and either a wildcard namespace on `self` or matching
    // namespaces. Equality with `other`'s own variant is handled by the
    // caller.
    pub(crate) fn is_superselector_variant(
        &self,
        other: &SimpleSelector<'parse>,
    ) -> SassResult<bool> {
        if let SimpleSelector::Type(ref o) = other {
            if self.name.name == o.name.name
                && (self.name.namespace.as_deref() == Some("*")
                    || self.name.namespace == o.name.namespace)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // Matches Dart `TypeSelector.unify` (type.dart): merges with a leading
    // universal or type selector via `unify_universal_and_element`
    // (extend/functions.dart); otherwise prepends `self` to `comps`.
    // Internal-only in Dart (`@internal`); kept public here.
    pub fn unify(
        &self,
        comps: &[SimpleSelector<'parse>],
    ) -> SassResult<Option<Vec<SimpleSelector<'parse>>>> {
        if !comps.is_empty() {
            match &comps[0] {
                SimpleSelector::Universal(_) | SimpleSelector::Type(_) => {
                    let other = &comps[0];
                    let s1 = SimpleSelector::Type(self.clone());
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
        let mut result = Vec::with_capacity(comps.len() + 1);
        result.push(SimpleSelector::Type(self.clone()));
        result.extend_from_slice(comps);
        Ok(Some(result))
    }
}

// Hashes via `hash_code`, matching Dart's `hashCode`.
impl<'parse> Hash for TypeSelector<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.hash_code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;

    #[test]
    fn test_new_type_selector() {
        let name = QualifiedName::new("div".into());
        let s = TypeSelector::new(name, BOGUS_SPAN);
        assert_eq!(s.name.name, "div");
    }

    #[test]
    fn test_type_selector_is_invisible() {
        let s = TypeSelector::new(QualifiedName::new("div".into()), BOGUS_SPAN);
        assert!(!s.is_invisible());
    }

    #[test]
    fn test_type_selector_is_bogus() {
        let s = TypeSelector::new(QualifiedName::new("div".into()), BOGUS_SPAN);
        assert!(!s.is_bogus());
    }

    #[test]
    fn test_type_selector_specificity() {
        let s = TypeSelector::new(QualifiedName::new("div".into()), BOGUS_SPAN);
        assert_eq!(s.specificity(), 1);
    }

    #[test]
    fn test_type_selector_add_suffix() {
        let s = TypeSelector::new(QualifiedName::new("div".into()), BOGUS_SPAN);
        let result = s.add_suffix("bar").unwrap();
        if let SimpleSelector::Type(ref ts) = result {
            assert_eq!(ts.name.name, "divbar");
        } else {
            panic!("expected TypeSelector");
        }
    }

    #[test]
    fn test_type_selector_partial_eq() {
        let s1 = TypeSelector::new(QualifiedName::new("div".into()), BOGUS_SPAN);
        let s2 = TypeSelector::new(QualifiedName::new("div".into()), BOGUS_SPAN);
        assert_eq!(s1, s2);
        let s3 = TypeSelector::new(QualifiedName::new("span".into()), BOGUS_SPAN);
        assert_ne!(s1, s3);
    }

    #[test]
    fn test_type_selector_hash_code() {
        let s1 = TypeSelector::new(QualifiedName::new("div".into()), BOGUS_SPAN);
        let s2 = TypeSelector::new(QualifiedName::new("div".into()), BOGUS_SPAN);
        assert_eq!(s1.hash_code(), s2.hash_code());
    }

    #[test]
    fn test_type_selector_is_superselector_wildcard_ns() {
        let ns = Some("*".to_string());
        let name = QualifiedName::new_with_namespace("div".into(), ns);
        let s = TypeSelector::new(name, BOGUS_SPAN);
        let target = TypeSelector::new(QualifiedName::new("div".into()), BOGUS_SPAN);
        assert!(s
            .is_superselector_variant(&SimpleSelector::Type(target))
            .unwrap());
    }

    #[test]
    fn test_type_selector_to_css_string() {
        let s = TypeSelector::new(QualifiedName::new("div".into()), BOGUS_SPAN);
        assert_eq!(s.to_css_string(true).unwrap(), "div");
    }

    #[test]
    fn test_type_selector_assert_not_bogus() {
        let s = TypeSelector::new(QualifiedName::new("div".into()), BOGUS_SPAN);
        assert!(s.assert_not_bogus(None, None).is_ok());
    }
}
