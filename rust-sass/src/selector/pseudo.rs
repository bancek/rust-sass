// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector/pseudo.dart + lib/src/extend/functions.dart
// go-source: go/value/selector_pseudo.go + go/value/selector_extend_functions.go

use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::selector::list::SelectorList;
use crate::selector::selector_assert_not_bogus_impl;
use crate::selector::visitor::SelectorVisitor;
use crate::selector::weave::compound_is_superselector;
use crate::selector::CompoundSelector;
use crate::selector::Selector;
use crate::selector::WarnLogger;
use crate::serialize::SerializeVisitor;
use crate::unvendor::unvendor;
use crate::value::hash::{bool_hash_code, hash_combine, string_hash_code};
use std::hash::Hash;
use std::hash::Hasher;

use crate::selector::SimpleSelector;

/// A pseudo-class or pseudo-element selector.
///
/// What a specific pseudo selector means depends on its name. Some take
/// arguments, including other selectors. Sass hand-codes logic for each
/// pseudo selector that takes a selector argument so that extension and
/// other selector operations treat them correctly.
#[derive(Clone, Debug)]
pub struct PseudoSelector<'parse> {
    /// The source span covering this selector.
    pub span: FileSpan<'parse>,
    /// The name of this selector.
    pub name: String,
    // Like `name`, but with any vendor prefix stripped. Internal-only in
    // Dart (`@internal`); kept public here.
    pub normalized_name: String,
    /// Whether this is a pseudo-class selector.
    ///
    /// This is `true` exactly when [`is_element`](Self::is_element) is
    /// `false`.
    pub is_class: bool,
    /// Whether this is syntactically a pseudo-class selector.
    ///
    /// This matches [`is_class`](Self::is_class) except for pseudo-elements
    /// written with single-colon syntax (`:before`, `:after`, `:first-line`,
    /// written with single-colon syntax (`:before`, `:after`, `:first-line`,
    /// `:first-letter`). There is no separate `is_syntactic_element` accessor
    /// (Dart has an `isSyntacticElement` getter); negate this field directly.
    pub is_syntactic_class: bool,
    /// The non-selector argument passed to this selector, if any.
    ///
    /// This is `None` when there is no argument. When both `argument` and
    /// [`selector`](Self::selector) are present, the selector follows the
    /// argument.
    pub argument: Option<String>,
    /// The selector argument passed to this selector, if any.
    ///
    /// This is `None` when there is no selector. When both
    /// [`argument`](Self::argument) and `selector` are present, the selector
    /// follows the argument.
    pub selector: Option<Box<Selector<'parse>>>,
}

impl<'parse> PseudoSelector<'parse> {
    /// Creates a pseudo selector with the given `name`.
    ///
    /// When `element` is `false`, the result is a pseudo-class unless `name`
    /// is a pseudo-element writable with single-colon syntax (`before`,
    /// `after`, `first-line`, `first-letter`), in which case it is still an
    /// element — only syntactically a class.
    pub fn new(
        name: String,
        span: FileSpan<'parse>,
        element: bool,
        argument: Option<String>,
        selector: Option<Selector<'parse>>,
    ) -> Self {
        let is_class = !element && !is_fake_pseudo_element(&name);
        let normalized_name = unvendor(&name);
        let selector = selector.map(Box::new);
        PseudoSelector {
            span,
            name,
            normalized_name,
            is_class,
            is_syntactic_class: !element,
            argument,
            selector,
        }
    }

    /// The source span of this selector.
    pub fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }

    /// Whether this is a pseudo-element selector.
    ///
    /// This is `true` exactly when [`is_class`](Self::is_class) is `false`.
    pub fn is_element(&self) -> bool {
        !self.is_class
    }

    // Whether this is a valid `:host` selector. Internal-only in Dart
    // (`@internal`); kept public here.
    // Note: Dart also has an `isSyntacticElement` getter (`!isSyntacticClass`);
    // there is no separate Rust accessor — read `is_syntactic_class` directly.
    pub fn is_host(&self) -> bool {
        self.is_class && self.name == "host"
    }

    // Whether this is a valid `:host-context` selector, which requires a
    // selector argument. Internal-only in Dart (`@internal`); kept public
    // here.
    pub fn is_host_context(&self) -> bool {
        self.is_class && self.name == "host-context" && self.selector.is_some()
    }

    /// Whether this selector need not be emitted.
    ///
    /// A bare pseudo is always emitted. `:not(...)` is invisible only when
    /// bogus — never merely because its argument is invisible, since
    /// "doesn't match nothing" is equivalent to `*`. Any other
    /// selector-bearing pseudo follows its argument's visibility.
    pub fn is_invisible(&self) -> bool {
        match &self.selector {
            None => false,
            Some(sel) => {
                if self.name == "not" {
                    sel.is_bogus()
                } else {
                    sel.is_invisible()
                }
            }
        }
    }

    // Matches Dart: delegates to `is_invisible`, ignoring bogus
    // combinators — except that `:not(...)` is never treated as invisible
    // on those grounds.
    pub fn is_invisible_other_than_bogus_combinators(&self) -> bool {
        match &self.selector {
            None => false,
            Some(sel) => {
                if self.name == "not" {
                    false
                } else {
                    sel.is_invisible_other_than_bogus_combinators()
                }
            }
        }
    }

    // Matches Dart: a pseudo is useless exactly when it is bogus.
    pub fn is_useless(&self) -> bool {
        self.is_bogus()
    }

    /// Whether this selector is not valid CSS.
    ///
    /// A bare pseudo is always valid. `:has(...)` tolerates leading
    /// combinators in its argument, as the CSS spec allows; any other
    /// selector-bearing pseudo follows its argument's bogusness.
    pub fn is_bogus(&self) -> bool {
        match &self.selector {
            None => false,
            Some(sel) => {
                if self.name == "has" {
                    sel.is_bogus_other_than_leading_combinator()
                } else {
                    sel.is_bogus()
                }
            }
        }
    }

    // Matches Dart: delegates to `is_bogus`, ignoring a leading combinator.
    pub fn is_bogus_other_than_leading_combinator(&self) -> bool {
        self.is_bogus()
    }

    /// Matches Dart: _ContainsParentSelectorVisitor via AnySelectorVisitor —
    /// recurses into the selector argument. (Go's containsParentVisitor
    /// performs the same recursion in its VisitPseudoSelector.)
    pub fn contains_parent_selector(&self) -> SassResult<bool> {
        match &self.selector {
            Some(sel) => sel.contains_parent_selector(),
            None => Ok(false),
        }
    }

    /// This selector's specificity, per the selectors spec.
    ///
    /// Pseudo-elements count 1. Bare pseudo-classes count 1000 (specificity
    /// is base-1000). `:where()` counts 0; `:is()`, `:not()`, `:has()`, and
    /// `:matches()` count the max of their arguments; `:nth-child()` and
    /// `:nth-last-child()` count 1000 plus that max. Anything else with a
    /// selector argument counts 1000.
    pub fn specificity(&self) -> usize {
        if self.is_element() {
            return 1;
        }
        match &self.selector {
            None => 1000,
            Some(sel) => match sel.as_ref() {
                Selector::List(ref list) => match self.normalized_name.as_str() {
                    "where" => 0,
                    "is" | "not" | "has" | "matches" => {
                        let mut max_sp = 0;
                        for comp in &list.0.components {
                            let sp = comp.specificity();
                            if sp > max_sp {
                                max_sp = sp;
                            }
                        }
                        max_sp
                    }
                    "nth-child" | "nth-last-child" => {
                        let mut max_sp = 0;
                        for comp in &list.0.components {
                            let sp = comp.specificity();
                            if sp > max_sp {
                                max_sp = sp;
                            }
                        }
                        1000 + max_sp
                    }
                    _ => 1000,
                },
                _ => 1000,
            },
        }
    }

    // Matches Dart: pseudo-elements and selector-bearing pseudos need
    // complex non-local superselector reasoning. Internal-only in Dart
    // (`@internal`); kept public here.
    pub fn has_complicated_superselector_semantics(&self) -> bool {
        self.is_element() || self.selector.is_some()
    }

    // Matches Dart: combines the name, element-ness, argument, and selector
    // hashes; the span is excluded. The selector list hashes by identity
    // when one is present.
    pub fn hash_code(&self) -> i32 {
        let mut h = hash_combine(
            string_hash_code(&self.name),
            bool_hash_code(self.is_element()),
        );
        if let Some(ref arg) = self.argument {
            h = hash_combine(h, string_hash_code(arg));
        }
        if let Some(ref sel) = self.selector {
            h = hash_combine(h, sel.hash_code());
        }
        h
    }

    /// Serializes this selector to CSS, quoting as `inspect` requests.
    pub fn to_css_string(&self, inspect: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(false, inspect);
        visitor.visit_pseudo_selector(self)?;
        Ok(visitor.into_string())
    }

    /// Warns when this selector is bogus (custom functions only); a no-op
    /// unless the selector argument is bogus.
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

    // Matches Dart `PseudoSelector.addSuffix` (pseudo.dart): selector- or
    // argument-bearing pseudos reject suffixes, as does the base
    // implementation; otherwise appends `suffix` to the name. Internal-only
    // in Dart (`@internal`); kept public here.
    pub fn add_suffix(&self, suffix: &str) -> SassResult<SimpleSelector<'parse>> {
        if self.argument.is_some() || self.selector.is_some() {
            return Err(Box::new(SassError::Script {
                message: format!(
                    "Selector \"{}\" can't have a suffix",
                    self.to_css_string(true)?
                ),
                argument_name: None,
            }));
        }
        Ok(SimpleSelector::Pseudo(PseudoSelector::new(
            format!("{}{}", self.name, suffix),
            self.span,
            self.is_element(),
            None,
            None,
        )))
    }

    /// Returns a copy of this selector with its selector argument replaced
    /// by `sel`.
    pub fn with_selector(&self, sel: &SelectorList<'parse>) -> SassResult<PseudoSelector<'parse>> {
        Ok(PseudoSelector::new(
            self.name.clone(),
            self.span,
            self.is_element(),
            self.argument.clone(),
            Some(Selector::List(*sel)),
        ))
    }

    // Matches Dart `PseudoSelector.isSuperselector` (pseudo.dart): a bare
    // pseudo matches only itself; `::slotted()` compares selector
    // arguments; anything else falls back to the compound comparison in
    // extend/functions.dart.
    pub(crate) fn is_superselector_variant(
        &self,
        other: &SimpleSelector<'parse>,
    ) -> SassResult<bool> {
        if self.selector.is_none() {
            return Ok(SimpleSelector::Pseudo(self.clone()) == *other);
        }
        if let SimpleSelector::Pseudo(ref p) = other {
            if self.is_element()
                && p.is_element()
                && self.normalized_name == "slotted"
                && p.name == self.name
            {
                if p.selector.is_some() {
                    if let Some(Selector::List(ref my_list)) = self.selector.as_deref() {
                        if let Some(super::Selector::List(ref other_list)) = p.selector.as_deref() {
                            return my_list.is_superselector(other_list);
                        }
                    }
                }
                return Ok(false);
            }
        }
        let compound_a =
            CompoundSelector::new(vec![SimpleSelector::Pseudo(self.clone())], self.span)?;
        let compound_b = CompoundSelector::new(vec![other.clone()], self.span)?;
        compound_is_superselector(&compound_a, &compound_b, None)
    }

    // Matches Dart `PseudoSelector.unify` (pseudo.dart): `:host` compounds
    // may only hold host-like pseudos; a lone universal or host pseudo
    // delegates to its own `unify`; a compound holds at most one pseudo
    // element, ordered after pseudo-classes. Internal-only in Dart
    // (`@internal`); kept public here.
    pub fn unify(
        &self,
        comps: &[SimpleSelector<'parse>],
    ) -> SassResult<Option<Vec<SimpleSelector<'parse>>>> {
        if self.name == "host" || self.name == "host-context" {
            for simple in comps {
                if let SimpleSelector::Pseudo(ref ps) = simple {
                    if !ps.is_host() && ps.selector.is_none() {
                        return Ok(None);
                    }
                } else {
                    return Ok(None);
                }
            }
        } else if comps.len() == 1 {
            let other = &comps[0];
            if matches!(other, SimpleSelector::Universal(_)) {
                return other.unify(&[SimpleSelector::Pseudo(self.clone())]);
            }
            if let SimpleSelector::Pseudo(ref ps) = other {
                if ps.is_host() || ps.is_host_context() {
                    return other.unify(&[SimpleSelector::Pseudo(self.clone())]);
                }
            }
        }
        for simple in comps {
            if *simple == SimpleSelector::Pseudo(self.clone()) {
                return Ok(Some(comps.to_vec()));
            }
        }
        let mut result = Vec::with_capacity(comps.len() + 1);
        let mut added_this = false;
        for simple in comps {
            if let SimpleSelector::Pseudo(ref ps) = simple {
                if !added_this && ps.is_element() {
                    if self.is_element() {
                        return Ok(None);
                    }
                    result.push(SimpleSelector::Pseudo(self.clone()));
                    added_this = true;
                }
            }
            result.push(simple.clone());
        }
        if !added_this {
            result.push(SimpleSelector::Pseudo(self.clone()));
        }
        Ok(Some(result))
    }
}

// Whether `name` is a pseudo-element writable with single-colon syntax
// (`before`, `after`, `first-line`, `first-letter`), matched
// case-insensitively on the first character. Matches Dart's private
// `_isFakePseudoElement`.
fn is_fake_pseudo_element(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let first = name.as_bytes()[0];
    match first {
        b'a' | b'A' => name.eq_ignore_ascii_case("after"),
        b'b' | b'B' => name.eq_ignore_ascii_case("before"),
        b'f' | b'F' => {
            name.eq_ignore_ascii_case("first-line") || name.eq_ignore_ascii_case("first-letter")
        }
        _ => false,
    }
}

// Matches Dart: structural equality over name, class-ness, argument, and
// selector (compared by identity when present); the span is excluded.
impl<'parse> PartialEq for PseudoSelector<'parse> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.is_class == other.is_class
            && self.argument == other.argument
            && self.selector == other.selector
    }
}

impl<'parse> Eq for PseudoSelector<'parse> {}

// Hashes via `hash_code`, matching Dart's `hashCode`.
impl<'parse> Hash for PseudoSelector<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.hash_code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;

    #[test]
    fn test_new_pseudo_selector_class() {
        let s = PseudoSelector::new("hover".into(), BOGUS_SPAN, false, None, None);
        assert_eq!(s.name, "hover");
        assert!(s.is_class);
        assert!(!s.is_element());
        assert_eq!(s.normalized_name, "hover");
    }

    #[test]
    fn test_new_pseudo_selector_element() {
        let s = PseudoSelector::new("before".into(), BOGUS_SPAN, true, None, None);
        assert!(!s.is_class);
        assert!(s.is_element());
    }

    #[test]
    fn test_is_fake_pseudo_element() {
        assert!(is_fake_pseudo_element("after"));
        assert!(is_fake_pseudo_element("After"));
        assert!(is_fake_pseudo_element("before"));
        assert!(is_fake_pseudo_element("first-line"));
        assert!(is_fake_pseudo_element("first-letter"));
        assert!(!is_fake_pseudo_element("hover"));
        assert!(!is_fake_pseudo_element("before-stuff"));
    }

    #[test]
    fn test_pseudo_selector_specificity_element() {
        let s = PseudoSelector::new("before".into(), BOGUS_SPAN, true, None, None);
        assert_eq!(s.specificity(), 1);
    }

    #[test]
    fn test_pseudo_selector_specificity_class() {
        let s = PseudoSelector::new("hover".into(), BOGUS_SPAN, false, None, None);
        assert_eq!(s.specificity(), 1000);
    }

    #[test]
    fn test_pseudo_selector_is_invisible() {
        let s = PseudoSelector::new("hover".into(), BOGUS_SPAN, false, None, None);
        assert!(!s.is_invisible());
    }

    #[test]
    fn test_pseudo_selector_is_bogus() {
        let s = PseudoSelector::new("hover".into(), BOGUS_SPAN, false, None, None);
        assert!(!s.is_bogus());
    }

    #[test]
    fn test_pseudo_selector_is_useless() {
        let s = PseudoSelector::new("hover".into(), BOGUS_SPAN, false, None, None);
        assert!(!s.is_useless());
    }

    #[test]
    fn test_pseudo_selector_is_host() {
        let s = PseudoSelector::new("host".into(), BOGUS_SPAN, false, None, None);
        assert!(s.is_host());
    }

    #[test]
    fn test_pseudo_selector_add_suffix_simple() {
        let s = PseudoSelector::new("hover".into(), BOGUS_SPAN, false, None, None);
        let result = s.add_suffix("-x").unwrap();
        if let SimpleSelector::Pseudo(ref ps) = result {
            assert_eq!(ps.name, "hover-x");
        } else {
            panic!("expected PseudoSelector");
        }
    }

    #[test]
    fn test_pseudo_selector_hash_code() {
        let s1 = PseudoSelector::new("hover".into(), BOGUS_SPAN, false, None, None);
        let s2 = PseudoSelector::new("hover".into(), BOGUS_SPAN, false, None, None);
        assert_eq!(s1.hash_code(), s2.hash_code());
    }

    #[test]
    fn test_pseudo_selector_partial_eq() {
        let s1 = PseudoSelector::new("hover".into(), BOGUS_SPAN, false, None, None);
        let s2 = PseudoSelector::new("hover".into(), BOGUS_SPAN, false, None, None);
        assert_eq!(s1, s2);

        let s3 = PseudoSelector::new("before".into(), BOGUS_SPAN, true, None, None);
        assert_ne!(s1, s3);
    }

    #[test]
    fn test_pseudo_selector_has_complicated_semantics() {
        let s = PseudoSelector::new("hover".into(), BOGUS_SPAN, false, None, None);
        assert!(!s.has_complicated_superselector_semantics());

        let s2 = PseudoSelector::new("before".into(), BOGUS_SPAN, true, None, None);
        assert!(s2.has_complicated_superselector_semantics());
    }

    #[test]
    fn test_pseudo_selector_to_css_string_class() {
        let s = PseudoSelector::new("hover".into(), BOGUS_SPAN, false, None, None);
        assert_eq!(s.to_css_string(true).unwrap(), ":hover");
    }

    #[test]
    fn test_pseudo_selector_to_css_string_element() {
        let s = PseudoSelector::new("before".into(), BOGUS_SPAN, true, None, None);
        assert_eq!(s.to_css_string(true).unwrap(), "::before");
    }

    #[test]
    fn test_pseudo_selector_to_css_string_with_arg() {
        let s = PseudoSelector::new(
            "nth-child".into(),
            BOGUS_SPAN,
            false,
            Some("2n+1".into()),
            None,
        );
        assert_eq!(s.to_css_string(true).unwrap(), ":nth-child(2n+1)");
    }

    #[test]
    fn test_pseudo_selector_assert_not_bogus() {
        let s = PseudoSelector::new("hover".into(), BOGUS_SPAN, false, None, None);
        assert!(s.assert_not_bogus(None, None).is_ok());
    }
}
