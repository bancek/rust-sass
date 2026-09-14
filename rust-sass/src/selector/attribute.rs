// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector/attribute.dart + lib/src/extend/functions.dart
// go-source: go/value/selector_attribute.go + go/value/selector_extend_functions.go

use crate::selector::selector_assert_not_bogus_impl;
use crate::selector::simple_base_unify;
use crate::selector::WarnLogger;
use crate::serialize::SerializeVisitor;
use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;

use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::selector::qualified_name::QualifiedName;
use crate::selector::visitor::SelectorVisitor;
use crate::value::hash::{hash_combine, string_hash_code, string_opt_hash_code};

use crate::selector::SimpleSelector;

/// An operator giving an [`AttributeSelector::value`] its meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AttributeOperator {
    /// The attribute value exactly equals the given value.
    Equal,
    /// The attribute value is a whitespace-separated list of words, one of
    /// which is the given value.
    Include,
    /// The attribute value is exactly the given value, or starts with the
    /// given value followed by a dash.
    Dash,
    /// The attribute value begins with the given value.
    Prefix,
    /// The attribute value ends with the given value.
    Suffix,
    /// The attribute value contains the given value.
    Substring,
}

// Renders the operator's token text, matching Dart's `toString()`.
impl fmt::Display for AttributeOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AttributeOperator::Equal => f.write_str("="),
            AttributeOperator::Include => f.write_str("~="),
            AttributeOperator::Dash => f.write_str("|="),
            AttributeOperator::Prefix => f.write_str("^="),
            AttributeOperator::Suffix => f.write_str("$="),
            AttributeOperator::Substring => f.write_str("*="),
        }
    }
}

/// An attribute selector.
///
/// Selects elements carrying the given attribute, optionally asserting
/// something about its value as well.
#[derive(Clone, Debug)]
pub struct AttributeSelector<'parse> {
    /// The source span covering this selector.
    pub span: FileSpan<'parse>,
    /// The name of the attribute being selected for.
    pub name: QualifiedName,
    /// The operator defining the semantics of [`value`](Self::value).
    ///
    /// `None` matches any element with the attribute, whatever its value;
    /// it is `None` exactly when `value` is `None`.
    pub op: Option<AttributeOperator>,
    /// An assertion about the value of [`name`](Self::name).
    ///
    /// Its precise meaning is defined by [`op`](Self::op). `None` matches
    /// regardless of value; it is `None` exactly when `op` is `None`.
    pub value: Option<String>,
    /// The modifier saying how the match is processed, such as a
    /// case-sensitivity flag (see the selectors-4
    /// [attribute-case](https://www.w3.org/TR/selectors-4/#attribute-case)
    /// section).
    ///
    /// Always `None` when [`op`](Self::op) is `None`.
    pub modifier: Option<String>,
}

impl<'parse> AttributeSelector<'parse> {
    /// Creates an attribute selector matching any element with a property of
    /// the given `name`.
    pub fn new(name: QualifiedName, span: FileSpan<'parse>) -> Self {
        AttributeSelector {
            span,
            name,
            op: None,
            value: None,
            modifier: None,
        }
    }

    /// Creates an attribute selector matching elements with a property named
    /// `name` whose value satisfies `value` under the semantics of `op`.
    pub fn new_with_operator(
        name: QualifiedName,
        op: AttributeOperator,
        value: String,
        span: FileSpan<'parse>,
        modifier: Option<String>,
    ) -> Self {
        AttributeSelector {
            span,
            name,
            op: Some(op),
            value: Some(value),
            modifier,
        }
    }

    /// The source span of this selector.
    pub fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }

    // Matches Dart: attribute selectors are always emitted, never invisible.
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

    // Matches Dart: attribute selectors never contain a parent selector.
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

    // Matches Dart: plain attribute matching needs no non-local reasoning.
    pub fn has_complicated_superselector_semantics(&self) -> bool {
        false
    }

    // Matches Dart: combines the name, namespace, operator, value, and
    // modifier hashes; the span is excluded.
    pub fn hash_code(&self) -> i32 {
        let mut h = hash_combine(
            string_hash_code(&self.name.name),
            string_opt_hash_code(&self.name.namespace),
        );
        if let Some(op) = &self.op {
            h = hash_combine(h, *op as i32);
            if let Some(val) = &self.value {
                h = hash_combine(h, string_hash_code(val));
                if let Some(modifier) = &self.modifier {
                    h = hash_combine(h, string_hash_code(modifier));
                }
            }
        }
        h
    }

    /// Serializes this selector to CSS, quoting as `inspect` requests.
    pub fn to_css_string(&self, inspect: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(false, inspect);
        visitor.visit_attribute_selector(self)?;
        Ok(visitor.into_string())
    }

    /// Warns when this selector is bogus (custom functions only); a no-op
    /// here since attribute selectors are never bogus.
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

    // Matches Dart: attribute selectors reject suffixes.
    pub fn add_suffix(&self, _suffix: &str) -> SassResult<SimpleSelector<'parse>> {
        Err(Box::new(SassError::Script {
            message: "attribute selector cannot have a suffix".into(),
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

    // Matches Dart `SimpleSelector.unify` (extend/functions.dart): returns a
    // copy of `comps` with this selector inserted before any trailing
    // pseudo selectors.
    pub fn unify(
        &self,
        comps: &[SimpleSelector<'parse>],
    ) -> SassResult<Option<Vec<SimpleSelector<'parse>>>> {
        simple_base_unify(SimpleSelector::Attribute(self.clone()), comps)
    }
}

// Matches Dart: structural equality over name, operator, value, and
// modifier; the span is excluded.
impl<'parse> PartialEq for AttributeSelector<'parse> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.op == other.op
            && self.value == other.value
            && self.modifier == other.modifier
    }
}

impl<'parse> Eq for AttributeSelector<'parse> {}

// Hashes via `hash_code`, matching Dart's `hashCode`.
impl<'parse> Hash for AttributeSelector<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.hash_code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;

    #[test]
    fn test_new_attribute_selector() {
        let name = QualifiedName::new("href".into());
        let s = AttributeSelector::new(name, BOGUS_SPAN);
        assert_eq!(s.name.name, "href");
        assert_eq!(s.op, None);
        assert_eq!(s.value, None);
    }

    #[test]
    fn test_new_attribute_selector_with_operator() {
        let name = QualifiedName::new("class".into());
        let s = AttributeSelector::new_with_operator(
            name,
            AttributeOperator::Equal,
            "foo".into(),
            BOGUS_SPAN,
            None,
        );
        assert_eq!(s.op, Some(AttributeOperator::Equal));
        assert_eq!(s.value, Some("foo".into()));
    }

    #[test]
    fn test_attribute_operator_display() {
        assert_eq!(AttributeOperator::Equal.to_string(), "=");
        assert_eq!(AttributeOperator::Include.to_string(), "~=");
        assert_eq!(AttributeOperator::Dash.to_string(), "|=");
        assert_eq!(AttributeOperator::Prefix.to_string(), "^=");
        assert_eq!(AttributeOperator::Suffix.to_string(), "$=");
        assert_eq!(AttributeOperator::Substring.to_string(), "*=");
    }

    #[test]
    fn test_attribute_selector_is_invisible() {
        let s = AttributeSelector::new(QualifiedName::new("href".into()), BOGUS_SPAN);
        assert!(!s.is_invisible());
    }

    #[test]
    fn test_attribute_selector_is_bogus() {
        let s = AttributeSelector::new(QualifiedName::new("href".into()), BOGUS_SPAN);
        assert!(!s.is_bogus());
    }

    #[test]
    fn test_attribute_selector_specificity() {
        let s = AttributeSelector::new(QualifiedName::new("href".into()), BOGUS_SPAN);
        assert_eq!(s.specificity(), 1000);
    }

    #[test]
    fn test_attribute_selector_add_suffix() {
        let s = AttributeSelector::new(QualifiedName::new("href".into()), BOGUS_SPAN);
        assert!(s.add_suffix("x").is_err());
    }

    #[test]
    fn test_attribute_selector_partial_eq() {
        let name = QualifiedName::new("href".into());
        let s1 = AttributeSelector::new(name.clone(), BOGUS_SPAN);
        let s2 = AttributeSelector::new(name.clone(), BOGUS_SPAN);
        assert_eq!(s1, s2);

        let s3 = AttributeSelector::new_with_operator(
            name.clone(),
            AttributeOperator::Equal,
            "x".into(),
            BOGUS_SPAN,
            None,
        );
        assert_ne!(s1, s3);

        let s4 = AttributeSelector::new_with_operator(
            name,
            AttributeOperator::Equal,
            "x".into(),
            BOGUS_SPAN,
            None,
        );
        assert_eq!(s3, s4);
    }

    #[test]
    fn test_attribute_selector_hash_code() {
        let name = QualifiedName::new("href".into());
        let s1 = AttributeSelector::new(name.clone(), BOGUS_SPAN);
        let s2 = AttributeSelector::new(name, BOGUS_SPAN);
        assert_eq!(s1.hash_code(), s2.hash_code());
    }

    #[test]
    fn test_attribute_selector_to_css_string() {
        let s = AttributeSelector::new(QualifiedName::new("href".into()), BOGUS_SPAN);
        assert_eq!(s.to_css_string(true).unwrap(), "[href]");
    }

    #[test]
    fn test_attribute_selector_to_css_string_with_value() {
        let s = AttributeSelector::new_with_operator(
            QualifiedName::new("class".into()),
            AttributeOperator::Equal,
            "foo".into(),
            BOGUS_SPAN,
            None,
        );
        assert_eq!(s.to_css_string(true).unwrap(), "[class=foo]");
    }

    #[test]
    fn test_attribute_selector_assert_not_bogus() {
        let s = AttributeSelector::new(QualifiedName::new("href".into()), BOGUS_SPAN);
        assert!(s.assert_not_bogus(None, None).is_ok());
    }
}
