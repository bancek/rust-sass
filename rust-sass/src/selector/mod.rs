// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector.dart + lib/src/ast/selector/simple.dart (SimpleSelector base: isSuperselector, unify)
// go-source: go/value/selector.go

//! Selector AST: [`Selector`], [`SimpleSelector`], and the shared
//! bogus/invisible/superselector/unify dispatch.

pub mod attribute;
pub mod class;
pub mod combinator;
pub mod complex;
pub mod complex_component;
pub mod compound;
pub mod id;
pub mod list;
pub mod parent;
pub mod placeholder;
pub mod pseudo;
pub mod qualified_name;
pub mod ty;
pub mod unify;
pub mod universal;
pub mod util;
pub mod visitor;
pub mod weave;

use crate::serialize::SerializeVisitor;
use std::fmt;
use std::hash::{Hash, Hasher};

use crate::common::exception::{SassError, SassResult};
use crate::deprecation::{Deprecation, BOGUS_COMBINATORS};
use crate::selector::visitor::SelectorVisitor;

use crate::selector::attribute::AttributeSelector;
use crate::selector::class::ClassSelector;
use crate::selector::complex::ComplexSelector;
use crate::selector::compound::CompoundSelector;
use crate::selector::id::IdSelector;
use crate::selector::list::SelectorList;
pub use crate::selector::list::SelectorListIdentity;
use crate::selector::parent::ParentSelector;
use crate::selector::placeholder::PlaceholderSelector;
use crate::selector::pseudo::PseudoSelector;
use crate::selector::ty::TypeSelector;
use crate::selector::universal::UniversalSelector;

/// WarnLogger is used by `assert_not_bogus` to emit deprecation warnings
/// through the evaluator's warn pipeline (dedup, quiet-deps, stack trace).
///
/// Matches Go: value.WarnLogger (go/value/selector.go).
pub trait WarnLogger {
    fn warn_deprecation(
        &mut self,
        message: &str,
        deprecation: &'static Deprecation,
    ) -> SassResult<()>;
}

/// A node in the abstract syntax tree for a selector.
///
/// This selector tree is mostly plain CSS, but may also contain a parent
/// (`&`) or placeholder (`%`) selector. Matches Dart: `Selector`
/// (ast/selector.dart).
///
/// Selectors have structural equality semantics: spans never participate in
/// equality or hashing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selector<'parse> {
    Simple(SimpleSelector<'parse>),
    Compound(CompoundSelector<'parse>),
    Complex(ComplexSelector<'parse>),
    List(SelectorList<'parse>),
}

/// A single simple selector, such as `.foo`, `#bar`, or `div`.
///
/// Matches Dart: `SimpleSelector` (ast/selector/simple.dart). Each method
/// below dispatches to the per-type implementation documented on the
/// corresponding struct.
#[derive(Clone, Debug)]
pub enum SimpleSelector<'parse> {
    Attribute(AttributeSelector<'parse>),
    Class(ClassSelector<'parse>),
    Id(IdSelector<'parse>),
    Pseudo(PseudoSelector<'parse>),
    Parent(ParentSelector<'parse>),
    Placeholder(PlaceholderSelector<'parse>),
    Type(TypeSelector<'parse>),
    Universal(UniversalSelector<'parse>),
}

impl<'parse> Selector<'parse> {
    /// Whether this selector is not valid CSS.
    ///
    /// This covers both selectors that are only useful for build-time nesting
    /// and selectors with invalid combinators kept for backwards
    /// compatibility. Matches Dart: `Selector.isBogus`.
    pub fn is_bogus(&self) -> bool {
        match self {
            Selector::Simple(s) => s.is_bogus(),
            Selector::Compound(c) => c.is_bogus(),
            Selector::Complex(c) => c.is_bogus(),
            Selector::List(l) => l.is_bogus(),
        }
    }

    // Whether this selector, and complex selectors containing it, should not
    // be emitted. Matches Dart: `Selector.isInvisible` (@internal).
    pub fn is_invisible(&self) -> bool {
        match self {
            Selector::Simple(s) => s.is_invisible(),
            Selector::Compound(c) => c.is_invisible(),
            Selector::Complex(c) => c.is_invisible(),
            Selector::List(l) => l.is_invisible(),
        }
    }

    // Whether this would be invisible even without bogus combinators.
    // Matches Dart: `Selector.isInvisibleOtherThanBogusCombinators`
    // (@internal).
    pub fn is_invisible_other_than_bogus_combinators(&self) -> bool {
        match self {
            Selector::Simple(s) => s.is_invisible_other_than_bogus_combinators(),
            Selector::Compound(c) => c.is_invisible_other_than_bogus_combinators(),
            Selector::Complex(c) => c.is_invisible_other_than_bogus_combinators(),
            Selector::List(l) => l.is_invisible_other_than_bogus_combinators(),
        }
    }

    // Whether this is bogus apart from having a leading combinator.
    // Matches Dart: `Selector.isBogusOtherThanLeadingCombinator` (@internal).
    pub fn is_bogus_other_than_leading_combinator(&self) -> bool {
        match self {
            Selector::Complex(c) => c.is_bogus_other_than_leading_combinator(),
            Selector::List(l) => l.is_bogus_other_than_leading_combinator(),
            _ => self.is_bogus(),
        }
    }

    // Whether this is a useless selector: bogus _and_ unable to become valid
    // CSS via `@extend` or nesting. Matches Dart: `Selector.isUseless`
    // (@internal).
    pub fn is_useless(&self) -> bool {
        match self {
            Selector::Simple(s) => s.is_useless(),
            Selector::Compound(c) => c.is_useless(),
            Selector::Complex(c) => c.is_useless(),
            Selector::List(l) => l.is_useless(),
        }
    }

    // Whether this contains a parent (`&`) selector.
    // Matches Dart: `Selector.containsParentSelector` (@internal).
    pub fn contains_parent_selector(&self) -> SassResult<bool> {
        match self {
            Selector::Simple(s) => s.contains_parent_selector(),
            Selector::Compound(c) => c.contains_parent_selector(),
            Selector::Complex(c) => c.contains_parent_selector(),
            Selector::List(l) => l.contains_parent_selector(),
        }
    }

    pub fn hash_code(&self) -> i32 {
        match self {
            Selector::Simple(s) => s.hash_code(),
            Selector::Compound(c) => c.hash_code(),
            Selector::Complex(c) => c.hash_code(),
            Selector::List(l) => l.0.hash_code(),
        }
    }

    /// Emits a warning if `self` is a bogus selector.
    ///
    /// May only be called from within a custom Sass function; this becomes an
    /// error in Dart Sass 2.0.0. Matches Dart: `Selector.assertNotBogus`.
    ///
    /// Without a [`WarnLogger`], returns a script error instead of warning.
    pub fn assert_not_bogus(
        &self,
        name: Option<&str>,
        warn: Option<&mut dyn WarnLogger>,
    ) -> SassResult<()> {
        match self {
            Selector::Simple(s) => s.assert_not_bogus(name, warn),
            Selector::Compound(c) => c.assert_not_bogus(name, warn),
            Selector::Complex(c) => c.assert_not_bogus(name, warn),
            Selector::List(l) => l.assert_not_bogus(name, warn),
        }
    }

    /// Calls the appropriate visit method on `v`.
    /// Matches Dart: `Selector.accept`.
    pub fn accept<V: SelectorVisitor<'parse> + ?Sized>(&self, v: &mut V) -> SassResult<V::Output> {
        match self {
            Selector::Simple(s) => s.accept(v),
            Selector::Compound(c) => v.visit_compound_selector(c),
            Selector::Complex(c) => v.visit_complex_selector(c),
            Selector::List(l) => v.visit_selector_list(l),
        }
    }
}

impl<'parse> SimpleSelector<'parse> {
    pub fn is_invisible(&self) -> bool {
        match self {
            SimpleSelector::Attribute(a) => a.is_invisible(),
            SimpleSelector::Class(c) => c.is_invisible(),
            SimpleSelector::Id(i) => i.is_invisible(),
            SimpleSelector::Pseudo(p) => p.is_invisible(),
            SimpleSelector::Parent(p) => p.is_invisible(),
            SimpleSelector::Placeholder(p) => p.is_invisible(),
            SimpleSelector::Type(t) => t.is_invisible(),
            SimpleSelector::Universal(u) => u.is_invisible(),
        }
    }

    pub fn is_bogus(&self) -> bool {
        match self {
            SimpleSelector::Attribute(a) => a.is_bogus(),
            SimpleSelector::Class(c) => c.is_bogus(),
            SimpleSelector::Id(i) => i.is_bogus(),
            SimpleSelector::Pseudo(p) => p.is_bogus(),
            SimpleSelector::Parent(p) => p.is_bogus(),
            SimpleSelector::Placeholder(p) => p.is_bogus(),
            SimpleSelector::Type(t) => t.is_bogus(),
            SimpleSelector::Universal(u) => u.is_bogus(),
        }
    }

    pub fn is_useless(&self) -> bool {
        match self {
            SimpleSelector::Attribute(a) => a.is_useless(),
            SimpleSelector::Class(c) => c.is_useless(),
            SimpleSelector::Id(i) => i.is_useless(),
            SimpleSelector::Pseudo(p) => p.is_useless(),
            SimpleSelector::Parent(p) => p.is_useless(),
            SimpleSelector::Placeholder(p) => p.is_useless(),
            SimpleSelector::Type(t) => t.is_useless(),
            SimpleSelector::Universal(u) => u.is_useless(),
        }
    }

    pub fn contains_parent_selector(&self) -> SassResult<bool> {
        match self {
            SimpleSelector::Attribute(a) => a.contains_parent_selector(),
            SimpleSelector::Class(c) => c.contains_parent_selector(),
            SimpleSelector::Id(i) => i.contains_parent_selector(),
            SimpleSelector::Pseudo(p) => p.contains_parent_selector(),
            SimpleSelector::Parent(p) => p.contains_parent_selector(),
            SimpleSelector::Placeholder(p) => p.contains_parent_selector(),
            SimpleSelector::Type(t) => t.contains_parent_selector(),
            SimpleSelector::Universal(u) => u.contains_parent_selector(),
        }
    }

    pub fn is_invisible_other_than_bogus_combinators(&self) -> bool {
        match self {
            SimpleSelector::Attribute(a) => a.is_invisible_other_than_bogus_combinators(),
            SimpleSelector::Class(c) => c.is_invisible_other_than_bogus_combinators(),
            SimpleSelector::Id(i) => i.is_invisible_other_than_bogus_combinators(),
            SimpleSelector::Pseudo(p) => p.is_invisible_other_than_bogus_combinators(),
            SimpleSelector::Parent(p) => p.is_invisible_other_than_bogus_combinators(),
            SimpleSelector::Placeholder(p) => p.is_invisible_other_than_bogus_combinators(),
            SimpleSelector::Type(t) => t.is_invisible_other_than_bogus_combinators(),
            SimpleSelector::Universal(u) => u.is_invisible_other_than_bogus_combinators(),
        }
    }

    pub fn is_bogus_other_than_leading_combinator(&self) -> bool {
        match self {
            SimpleSelector::Attribute(a) => a.is_bogus_other_than_leading_combinator(),
            SimpleSelector::Class(c) => c.is_bogus_other_than_leading_combinator(),
            SimpleSelector::Id(i) => i.is_bogus_other_than_leading_combinator(),
            SimpleSelector::Pseudo(p) => p.is_bogus_other_than_leading_combinator(),
            SimpleSelector::Parent(p) => p.is_bogus_other_than_leading_combinator(),
            SimpleSelector::Placeholder(p) => p.is_bogus_other_than_leading_combinator(),
            SimpleSelector::Type(t) => t.is_bogus_other_than_leading_combinator(),
            SimpleSelector::Universal(u) => u.is_bogus_other_than_leading_combinator(),
        }
    }

    /// This selector's specificity, in base 1000.
    ///
    /// Matches Dart: `SimpleSelector.specificity`.
    pub fn specificity(&self) -> usize {
        match self {
            SimpleSelector::Attribute(a) => a.specificity(),
            SimpleSelector::Class(c) => c.specificity(),
            SimpleSelector::Id(i) => i.specificity(),
            SimpleSelector::Pseudo(p) => p.specificity(),
            SimpleSelector::Parent(p) => p.specificity(),
            SimpleSelector::Placeholder(p) => p.specificity(),
            SimpleSelector::Type(t) => t.specificity(),
            SimpleSelector::Universal(u) => u.specificity(),
        }
    }

    // Whether this needs complex non-local reasoning to decide super- or
    // sub-selector relations (pseudo-elements and pseudos with selector
    // arguments). Matches Dart: `SimpleSelector.hasComplicatedSuperselectorSemantics`
    // (@internal).
    pub fn has_complicated_superselector_semantics(&self) -> bool {
        match self {
            SimpleSelector::Attribute(a) => a.has_complicated_superselector_semantics(),
            SimpleSelector::Class(c) => c.has_complicated_superselector_semantics(),
            SimpleSelector::Id(i) => i.has_complicated_superselector_semantics(),
            SimpleSelector::Pseudo(p) => p.has_complicated_superselector_semantics(),
            SimpleSelector::Parent(p) => p.has_complicated_superselector_semantics(),
            SimpleSelector::Placeholder(p) => p.has_complicated_superselector_semantics(),
            SimpleSelector::Type(t) => t.has_complicated_superselector_semantics(),
            SimpleSelector::Universal(u) => u.has_complicated_superselector_semantics(),
        }
    }

    pub fn hash_code(&self) -> i32 {
        match self {
            SimpleSelector::Attribute(a) => a.hash_code(),
            SimpleSelector::Class(c) => c.hash_code(),
            SimpleSelector::Id(i) => i.hash_code(),
            SimpleSelector::Pseudo(p) => p.hash_code(),
            SimpleSelector::Parent(p) => p.hash_code(),
            SimpleSelector::Placeholder(p) => p.hash_code(),
            SimpleSelector::Type(t) => t.hash_code(),
            SimpleSelector::Universal(u) => u.hash_code(),
        }
    }

    pub fn assert_not_bogus(
        &self,
        name: Option<&str>,
        warn: Option<&mut dyn WarnLogger>,
    ) -> SassResult<()> {
        match self {
            SimpleSelector::Attribute(a) => a.assert_not_bogus(name, warn),
            SimpleSelector::Class(c) => c.assert_not_bogus(name, warn),
            SimpleSelector::Id(i) => i.assert_not_bogus(name, warn),
            SimpleSelector::Pseudo(p) => p.assert_not_bogus(name, warn),
            SimpleSelector::Parent(p) => p.assert_not_bogus(name, warn),
            SimpleSelector::Placeholder(p) => p.assert_not_bogus(name, warn),
            SimpleSelector::Type(t) => t.assert_not_bogus(name, warn),
            SimpleSelector::Universal(u) => u.assert_not_bogus(name, warn),
        }
    }

    // Returns a copy of `self` as though written with `suffix` appended.
    // Assumes a valid identifier suffix; otherwise returns a script error.
    // Matches Dart: `SimpleSelector.addSuffix` (@internal).
    pub fn add_suffix(&self, suffix: &str) -> SassResult<SimpleSelector<'parse>> {
        match self {
            SimpleSelector::Attribute(a) => a.add_suffix(suffix),
            SimpleSelector::Class(c) => c.add_suffix(suffix),
            SimpleSelector::Id(i) => i.add_suffix(suffix),
            SimpleSelector::Pseudo(p) => p.add_suffix(suffix),
            SimpleSelector::Parent(p) => p.add_suffix(suffix),
            SimpleSelector::Placeholder(p) => p.add_suffix(suffix),
            SimpleSelector::Type(t) => t.add_suffix(suffix),
            SimpleSelector::Universal(u) => u.add_suffix(suffix),
        }
    }

    // The shared `SimpleSelector.isSuperselector` base logic: equality plus
    // the subselector-pseudo rule (`.foo` is a superselector of
    // `:matches(.foo)` and friends). Per-variant logic then runs in
    // `is_superselector_variant`. Matches Dart: `SimpleSelector.isSuperselector`
    // plus each subclass override's `super` call.
    fn base_is_superselector(&self, other: &SimpleSelector<'parse>) -> SassResult<bool> {
        if self == other {
            return Ok(true);
        }
        if let SimpleSelector::Pseudo(ref p) = other {
            if p.is_class {
                if let Some(ref sel) = p.selector {
                    if let Selector::List(ref list) = sel.as_ref() {
                        let n = &p.normalized_name;
                        if n == "is"
                            || n == "matches"
                            || n == "where"
                            || n == "any"
                            || n == "nth-child"
                            || n == "nth-last-child"
                        {
                            for complex in &list.0.components {
                                if complex.components.is_empty() {
                                    return Ok(false);
                                }
                                let last_comp = &complex.components[complex.components.len() - 1];
                                let mut found = false;
                                for comp in &last_comp.selector.components {
                                    if self.is_superselector(comp)? {
                                        found = true;
                                        break;
                                    }
                                }
                                if !found {
                                    return Ok(false);
                                }
                            }
                            return Ok(true);
                        }
                    }
                }
            }
        }
        Ok(false)
    }

    /// Whether `self` matches every element `other` matches, plus possibly
    /// more. Matches Dart: `SimpleSelector.isSuperselector`.
    pub fn is_superselector(&self, other: &SimpleSelector<'parse>) -> SassResult<bool> {
        if self.base_is_superselector(other)? {
            return Ok(true);
        }
        match self {
            SimpleSelector::Class(c) => c.is_superselector_variant(other),
            SimpleSelector::Id(i) => i.is_superselector_variant(other),
            SimpleSelector::Placeholder(p) => p.is_superselector_variant(other),
            SimpleSelector::Type(t) => t.is_superselector_variant(other),
            SimpleSelector::Universal(u) => u.is_superselector_variant(other),
            SimpleSelector::Pseudo(p) => p.is_superselector_variant(other),
            SimpleSelector::Parent(p) => p.is_superselector_variant(other),
            SimpleSelector::Attribute(a) => a.is_superselector_variant(other),
        }
    }

    // Returns the components of a compound matching only elements matched by
    // both `self` and `comps`, or `None` when unification is impossible.
    // Matches Dart: `SimpleSelector.unify` (@internal); the default logic
    // (dedup, pseudos sort last) lives in `simple_base_unify` below while
    // per-type overrides live on each selector struct.
    pub fn unify(
        &self,
        comps: &[SimpleSelector<'parse>],
    ) -> SassResult<Option<Vec<SimpleSelector<'parse>>>> {
        match self {
            SimpleSelector::Attribute(a) => a.unify(comps),
            SimpleSelector::Class(c) => c.unify(comps),
            SimpleSelector::Id(i) => i.unify(comps),
            SimpleSelector::Pseudo(p) => p.unify(comps),
            SimpleSelector::Parent(p) => p.unify(comps),
            SimpleSelector::Placeholder(p) => p.unify(comps),
            SimpleSelector::Type(t) => t.unify(comps),
            SimpleSelector::Universal(u) => u.unify(comps),
        }
    }

    pub fn accept<V: SelectorVisitor<'parse> + ?Sized>(&self, v: &mut V) -> SassResult<V::Output> {
        match self {
            SimpleSelector::Attribute(a) => v.visit_attribute_selector(a),
            SimpleSelector::Class(c) => v.visit_class_selector(c),
            SimpleSelector::Id(i) => v.visit_id_selector(i),
            SimpleSelector::Pseudo(p) => v.visit_pseudo_selector(p),
            SimpleSelector::Parent(p) => v.visit_parent_selector(p),
            SimpleSelector::Placeholder(p) => v.visit_placeholder_selector(p),
            SimpleSelector::Type(t) => v.visit_type_selector(t),
            SimpleSelector::Universal(u) => v.visit_universal_selector(u),
        }
    }

    /// Serializes this selector. Matches Go: `Selector.String()` —
    /// `SerializeSelector(s, inspect)`.
    ///
    /// Matches Dart: `Selector.toString` (`serializeSelector(this, inspect: true)`).
    pub fn to_css_string(&self, inspect: bool) -> SassResult<String> {
        match self {
            SimpleSelector::Attribute(a) => a.to_css_string(inspect),
            SimpleSelector::Class(c) => c.to_css_string(inspect),
            SimpleSelector::Id(i) => i.to_css_string(inspect),
            SimpleSelector::Pseudo(p) => p.to_css_string(inspect),
            SimpleSelector::Parent(p) => p.to_css_string(inspect),
            SimpleSelector::Placeholder(p) => p.to_css_string(inspect),
            SimpleSelector::Type(t) => t.to_css_string(inspect),
            SimpleSelector::Universal(u) => u.to_css_string(inspect),
        }
    }
}

/// Shared helper for `assert_not_bogus`. Matches Go's `selectorAssertNotBogus` in
/// `go/value/selector.go` and Dart's `Selector.assertNotBogus` in
/// `lib/src/ast/selector.dart`.
pub(crate) fn selector_assert_not_bogus_impl(
    is_bogus: bool,
    serialized: &str,
    name: Option<&str>,
    warn: Option<&mut dyn WarnLogger>,
) -> SassResult<()> {
    if !is_bogus {
        return Ok(());
    }
    let prefix = match name {
        Some(n) => format!("${}: ", n),
        None => String::new(),
    };
    let message = format!(
        "{}{} is not valid CSS.\nThis will be an error in Dart Sass 2.0.0.\n\nMore info: https://sass-lang.com/d/bogus-combinators",
        prefix, serialized
    );
    if let Some(w) = warn {
        w.warn_deprecation(&message, &BOGUS_COMBINATORS)?;
        Ok(())
    } else {
        Err(Box::new(SassError::Script {
            message,
            argument_name: name.map(|n| format!("${}", n)),
        }))
    }
}

impl<'parse> PartialEq for SimpleSelector<'parse> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (SimpleSelector::Attribute(a), SimpleSelector::Attribute(b)) => a == b,
            (SimpleSelector::Class(a), SimpleSelector::Class(b)) => a == b,
            (SimpleSelector::Id(a), SimpleSelector::Id(b)) => a == b,
            (SimpleSelector::Pseudo(a), SimpleSelector::Pseudo(b)) => a == b,
            (SimpleSelector::Parent(a), SimpleSelector::Parent(b)) => a == b,
            (SimpleSelector::Placeholder(a), SimpleSelector::Placeholder(b)) => a == b,
            (SimpleSelector::Type(a), SimpleSelector::Type(b)) => a == b,
            (SimpleSelector::Universal(a), SimpleSelector::Universal(b)) => a == b,
            _ => false,
        }
    }
}

impl<'parse> Eq for SimpleSelector<'parse> {}

impl<'parse> Hash for SimpleSelector<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            SimpleSelector::Attribute(a) => a.hash(state),
            SimpleSelector::Class(c) => c.hash(state),
            SimpleSelector::Id(i) => i.hash(state),
            SimpleSelector::Pseudo(p) => p.hash(state),
            SimpleSelector::Parent(p) => p.hash(state),
            SimpleSelector::Placeholder(p) => p.hash(state),
            SimpleSelector::Type(t) => t.hash(state),
            SimpleSelector::Universal(u) => u.hash(state),
        }
    }
}

impl<'parse> fmt::Display for SimpleSelector<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut visitor = SerializeVisitor::new_plain(false, false);
        self.accept(&mut visitor).map_err(|_| fmt::Error)?;
        write!(f, "{}", visitor.into_string())
    }
}

impl<'parse> Hash for Selector<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.hash_code());
    }
}

impl<'parse> fmt::Display for Selector<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut visitor = SerializeVisitor::new_plain(false, false);
        self.accept(&mut visitor).map_err(|_| fmt::Error)?;
        write!(f, "{}", visitor.into_string())
    }
}

// Default `SimpleSelector.unify`: returns `comps` unchanged when `s` is
// already present, otherwise appends `s` before the first pseudo so pseudos
// stay last. A lone universal or `:host`/`:host-context` pseudo delegates to
// that selector's own `unify`. Matches Dart: `SimpleSelector.unify`
// (ast/selector/simple.dart).
pub(crate) fn simple_base_unify<'parse>(
    s: SimpleSelector<'parse>,
    comps: &[SimpleSelector<'parse>],
) -> SassResult<Option<Vec<SimpleSelector<'parse>>>> {
    if comps.len() == 1 {
        let other = &comps[0];
        if matches!(other, SimpleSelector::Universal(_)) {
            return other.unify(&[s]);
        }
        if let SimpleSelector::Pseudo(ref ps) = other {
            if ps.is_host() || ps.is_host_context() {
                return other.unify(&[s]);
            }
        }
    }
    for comp in comps {
        if comp == &s {
            return Ok(Some(comps.to_vec()));
        }
    }
    let mut result: Vec<SimpleSelector<'parse>> = Vec::with_capacity(comps.len() + 1);
    let mut added_this = false;
    for comp in comps {
        if !added_this {
            if let SimpleSelector::Pseudo(_) = comp {
                result.push(s.clone());
                added_this = true;
            }
        }
        result.push(comp.clone());
    }
    if !added_this {
        result.push(s);
    }
    Ok(Some(result))
}
