// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector/list.dart + lib/src/extend/functions.dart
// go-source: go/value/selector_list.go + go/value/selector_extend_functions.go

use crate::selector::selector_assert_not_bogus_impl;
use crate::selector::weave::list_is_superselector;
use crate::selector::weave::unify_complex;
use crate::selector::Selector;
use crate::selector::WarnLogger;
use crate::serialize::SerializeVisitor;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

use bumpalo::Bump;

use crate::common::ast_css_value::CssValue;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::selector::combinator::Combinator;
use crate::selector::complex::ComplexSelector;
use crate::selector::complex_component::ComplexSelectorComponent;
use crate::selector::compound::CompoundSelector;
use crate::selector::util::is_parent_selector;
use crate::selector::visitor::SelectorVisitor;
use crate::selector::SimpleSelector;
use crate::value::hash::hash_combine;
use crate::value::{ListSeparator, SassList, SassString, Value, ValueKind};

// =============================================================================
// SelectorListInner — the data (structural equality)
// =============================================================================

/// A selector list.
///
/// A selector list is composed of [`ComplexSelector`]s. It matches any
/// element that matches any of the component selectors. Matches Dart:
/// `SelectorList` (ast/selector/list.dart).
#[derive(Debug)]
pub struct SelectorListInner<'parse> {
    pub span: FileSpan<'parse>,
    /// The components of this selector.
    ///
    /// This is never empty.
    pub components: Vec<ComplexSelector<'parse>>,
}

impl<'parse> SelectorListInner<'parse> {
    pub fn hash_code(&self) -> i32 {
        let mut h = 0;
        for comp in &self.components {
            h = hash_combine(h, comp.hash_code());
        }
        h
    }
}

impl<'parse> PartialEq for SelectorListInner<'parse> {
    fn eq(&self, other: &Self) -> bool {
        self.components == other.components
    }
}

impl<'parse> Eq for SelectorListInner<'parse> {}

impl<'parse> Hash for SelectorListInner<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.hash_code());
    }
}

// =============================================================================
// SelectorList — structural equality wrapper (Clone = Rc::clone)
// =============================================================================

/// The `Copy` arena handle for a selector list, with structural equality
/// (compares components, matching Dart's `==`) and `Hash` over the combined
/// component hashes.
///
/// Use [`identity`](Self::identity) for the identity-keyed store maps
/// (matching Dart's `Map.identity()` in the extension store).
#[derive(Clone, Copy, Debug)]
pub struct SelectorList<'parse>(pub(crate) &'parse SelectorListInner<'parse>);

impl<'parse> SelectorList<'parse> {
    /// Creates a list; returns a script error when `components` is empty.
    /// Matches Dart: `SelectorList` constructor (`components may not be empty`).
    pub fn new<'compile: 'parse>(
        arena: &'compile Bump,
        components: Vec<ComplexSelector<'parse>>,
        span: FileSpan<'parse>,
    ) -> SassResult<Self> {
        if components.is_empty() {
            return Err(Box::new(SassError::Script {
                message: "components may not be empty".into(),
                argument_name: None,
            }));
        }
        Ok(SelectorList(arena.alloc(SelectorListInner {
            span,
            components: components.to_vec(),
        })))
    }

    pub fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.0.span)
    }

    // Whether every complex in the list is invisible (placeholders,
    // `:not` of invisible, etc.). Matches Dart: `Selector.isInvisible`
    // as realized by `_IsInvisibleVisitor` on `SelectorList`.
    pub fn is_invisible(&self) -> bool {
        for comp in &self.0.components {
            if !comp.is_invisible() {
                return false;
            }
        }
        true
    }

    // Whether any complex in the list is bogus. Matches Dart:
    // `Selector.isBogus` as realized by `_IsBogusVisitor` on `SelectorList`.
    pub fn is_bogus(&self) -> bool {
        for comp in &self.0.components {
            if comp.is_bogus() {
                return true;
            }
        }
        false
    }

    // Whether any complex in the list is useless (bogus and not fixable by
    // `@extend`/nesting). Matches Dart: `Selector.isUseless` as realized by
    // `_IsUselessVisitor` on `SelectorList`.
    pub fn is_useless(&self) -> bool {
        for comp in &self.0.components {
            if comp.is_useless() {
                return true;
            }
        }
        false
    }

    // Whether any complex is bogus apart from a leading combinator.
    // Matches Dart: `Selector.isBogusOtherThanLeadingCombinator`.
    pub fn is_bogus_other_than_leading_combinator(&self) -> bool {
        for comp in &self.0.components {
            if comp.is_bogus_other_than_leading_combinator() {
                return true;
            }
        }
        false
    }

    // Whether every complex is invisible even ignoring bogus combinators.
    // Matches Dart: `Selector.isInvisibleOtherThanBogusCombinators`.
    pub fn is_invisible_other_than_bogus_combinators(&self) -> bool {
        for comp in &self.0.components {
            if !comp.is_invisible_other_than_bogus_combinators() {
                return false;
            }
        }
        true
    }

    // Whether any simple selector in the list (descending into pseudo
    // selector arguments) contains a parent (`&`) selector.
    // Matches Dart: `Selector.containsParentSelector` via
    // `_ContainsParentSelectorVisitor`.
    pub fn contains_parent_selector(&self) -> SassResult<bool> {
        for complex in &self.0.components {
            for comp in &complex.components {
                for simple in &comp.selector.components {
                    if simple.contains_parent_selector()? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    // Structural content equality used alongside `PartialEq`.
    pub fn content_eq(&self, other: &Self) -> bool {
        self.0.components == other.0.components
    }

    /// Returns the identity handle for store maps keyed by allocation
    /// rather than contents. Matches Dart's `Map.identity()` usage in the
    /// extension store.
    pub fn identity(&self) -> SelectorListIdentity<'parse> {
        SelectorListIdentity(self.0)
    }

    /// Serializes via the plain serialize visitor. Matches Dart:
    /// `serializeSelector` as used by `Selector.toString` (`inspect: true`).
    pub fn to_css_string(&self, inspect: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(false, inspect);
        visitor.visit_selector_list(self)?;
        Ok(visitor.into_string())
    }

    /// Emits a warning if the list is a bogus selector (custom-function
    /// context only; an error in Dart Sass 2.0.0). Matches Dart:
    /// `Selector.assertNotBogus`.
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

    /// Whether `self` matches every element `other` matches, plus possibly
    /// more. Matches Dart: `SelectorList.isSuperselector`
    /// (via `listIsSuperselector` in extend/functions.dart).
    pub fn is_superselector(&self, other: &SelectorList<'parse>) -> SassResult<bool> {
        list_is_superselector(&self.0.components, &other.0.components)
    }

    /// Returns a selector list matching only elements matched by both `self`
    /// and `other`, or `None` when no such list exists.
    /// Matches Dart: `SelectorList.unify` (ast/selector/list.dart).
    pub fn unify<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        other: &SelectorList<'parse>,
    ) -> SassResult<Option<SelectorList<'parse>>> {
        let mut contents: Vec<ComplexSelector<'parse>> = Vec::new();
        for complex1 in &self.0.components {
            for complex2 in &other.0.components {
                let span = complex1.span()?;
                if let Some(unified) = unify_complex(&[complex1.clone(), complex2.clone()], span)? {
                    contents.extend(unified);
                }
            }
        }
        if contents.is_empty() {
            return Ok(None);
        }
        Ok(Some(SelectorList::new(arena, contents, self.0.span)?))
    }

    /// Returns a new selector list representing `self` nested within
    /// `parent`.
    ///
    /// By default replaces parent (`&`) selectors with `parent`; with
    /// `preserve_parent_selectors` keeps them instead. With `implicit_parent`,
    /// complexes without an explicit `&` are prepended with `parent`. A `None`
    /// parent returns `self` as-is unless it holds a suffixed `&`, which is
    /// an error at the top level. Matches Dart: `SelectorList.nestWithin`
    /// (ast/selector/list.dart).
    pub fn nest_within<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        parent: Option<&SelectorList<'parse>>,
        implicit_parent: bool,
        preserve_parent_selectors: bool,
    ) -> SassResult<SelectorList<'parse>> {
        let parent = match parent {
            None => {
                if preserve_parent_selectors {
                    return Ok(*self);
                }
                if let Some(ps) = find_parent_selector_with_suffix(self) {
                    let span = ps.span()?;
                    return Err(Box::new(SassError::Sass {
                        message:
                            "A top-level selector may not contain a parent selector with a suffix."
                                .into(),
                        span: SourceSpanWithContext::from_file_span(&span)?,
                        cause: None,
                        loaded_urls: vec![],
                    }));
                }
                return Ok(*self);
            }
            Some(p) => p,
        };

        let nested: Vec<Vec<ComplexSelector<'parse>>> = self
            .0
            .components
            .iter()
            .map(|complex| {
                self.nest_within_complex(
                    arena,
                    complex,
                    parent,
                    implicit_parent,
                    preserve_parent_selectors,
                )
            })
            .collect::<SassResult<_>>()?;

        let flattened = flatten_vertically(nested);
        SelectorList::new(arena, flattened, self.0.span)
    }

    fn nest_within_complex<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        complex: &ComplexSelector<'parse>,
        parent: &SelectorList<'parse>,
        implicit_parent: bool,
        preserve_parent_selectors: bool,
    ) -> SassResult<Vec<ComplexSelector<'parse>>> {
        let has_parent = if preserve_parent_selectors {
            false
        } else {
            complex_contains_parent_selector(complex)?
        };

        if preserve_parent_selectors || !has_parent {
            if !implicit_parent {
                return Ok(vec![complex.clone()]);
            }
            let span = complex.span()?;
            let result: Vec<ComplexSelector<'parse>> = parent
                .0
                .components
                .iter()
                .map(|parent_complex| parent_complex.concatenate(complex, span, false))
                .collect();
            return Ok(result);
        }

        let mut new_complexes: Vec<ComplexSelector<'parse>> = Vec::new();
        for component in &complex.components {
            let resolved = self.nest_within_compound(arena, component, parent)?;
            match resolved {
                None => {
                    if new_complexes.is_empty() {
                        let span = complex.span()?;
                        new_complexes.push(ComplexSelector::new(
                            complex.leading_combinators.clone(),
                            vec![component.clone()],
                            span,
                            false,
                        )?);
                    } else {
                        let span = complex.span()?;
                        for nc in &mut new_complexes {
                            *nc = nc.with_additional_component(component, span, false);
                        }
                    }
                }
                Some(ref resolved_vec) if new_complexes.is_empty() => {
                    if complex.leading_combinators.is_empty() {
                        new_complexes.extend_from_slice(resolved_vec);
                    } else {
                        let span = complex.span()?;
                        for rc in resolved_vec {
                            let mut new_lc = complex.leading_combinators.clone();
                            if !rc.leading_combinators.is_empty() {
                                new_lc.extend_from_slice(&rc.leading_combinators);
                            }
                            new_complexes.push(ComplexSelector::new(
                                new_lc,
                                rc.components.clone(),
                                span,
                                rc.line_break,
                            )?);
                        }
                    }
                }
                Some(ref resolved_vec) => {
                    let mut updated = Vec::new();
                    for nc in &new_complexes {
                        let span = nc.span()?;
                        for rc in resolved_vec {
                            updated.push(nc.concatenate(rc, span, false));
                        }
                    }
                    new_complexes = updated;
                }
            }
        }
        Ok(new_complexes)
    }

    fn nest_within_compound<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        component: &ComplexSelectorComponent<'parse>,
        parent: &SelectorList<'parse>,
    ) -> SassResult<Option<Vec<ComplexSelector<'parse>>>> {
        let simples = &component.selector.components;
        let mut contains_selector_pseudo = false;
        for simple in simples {
            if let SimpleSelector::Pseudo(ref ps) = simple {
                if let Some(ref sel) = ps.selector {
                    if sel.contains_parent_selector()? {
                        contains_selector_pseudo = true;
                        break;
                    }
                }
            }
        }
        let first_simple = &simples[0];
        if !contains_selector_pseudo && !is_parent_selector(first_simple) {
            return Ok(None);
        }

        let first_simple_span = simple_span(first_simple)?;

        let resolved_simples: Vec<SimpleSelector<'parse>> = if contains_selector_pseudo {
            let mut result = Vec::with_capacity(simples.len());
            for simple in simples {
                if let SimpleSelector::Pseudo(ref ps) = simple {
                    if let Some(ref sel) = ps.selector {
                        if sel.contains_parent_selector().map_err(|e| {
                            e.with_additional_span(
                                SourceSpanWithContext::from_file_span(&first_simple_span).unwrap(),
                                "parent selector".into(),
                            )
                        })? {
                            let nested = match sel.as_ref() {
                                Selector::List(ref list) => list
                                    .nest_within(arena, Some(parent), false, false)
                                    .map_err(|e| {
                                        e.with_additional_span(
                                            SourceSpanWithContext::from_file_span(
                                                &first_simple_span,
                                            )
                                            .unwrap(),
                                            "parent selector".into(),
                                        )
                                    })?,
                                _ => {
                                    return Err(Box::new(SassError::Script {
                                        message: "Expected SelectorList".into(),
                                        argument_name: None,
                                    }))
                                }
                            };
                            let psel = ps.with_selector(&nested).map_err(|e| {
                                e.with_additional_span(
                                    SourceSpanWithContext::from_file_span(&first_simple_span)
                                        .unwrap(),
                                    "parent selector".into(),
                                )
                            })?;
                            result.push(SimpleSelector::Pseudo(psel));
                            continue;
                        }
                    }
                }
                result.push(simple.clone());
            }
            result
        } else {
            simples.to_vec()
        };

        let parent_selector = if let SimpleSelector::Parent(ref ps) = first_simple {
            ps
        } else {
            let component_span = component.selector.span()?;
            let compound =
                CompoundSelector::new(resolved_simples, component_span).map_err(|e| {
                    e.with_additional_span(
                        SourceSpanWithContext::from_file_span(&first_simple_span).unwrap(),
                        "parent selector".into(),
                    )
                })?;
            let cs = ComplexSelector::new(
                vec![],
                vec![ComplexSelectorComponent::new(
                    Box::new(compound),
                    component.combinators.clone(),
                    component.span,
                )],
                component.span,
                false,
            )
            .map_err(|e| {
                e.with_additional_span(
                    SourceSpanWithContext::from_file_span(&first_simple_span).unwrap(),
                    "parent selector".into(),
                )
            })?;
            return Ok(Some(vec![cs]));
        };

        let wrap_err = |e: Box<SassError>| {
            let ctx = SourceSpanWithContext::from_file_span(&first_simple_span).unwrap();
            e.with_additional_span(ctx, "parent selector".into())
        };

        if simples.len() == 1 && parent_selector.suffix().is_none() {
            let with_combinators =
                parent.with_additional_combinators(arena, &component.combinators)?;
            return Ok(Some(with_combinators.0.components.clone()));
        }

        let mut result = Vec::new();
        for complex in &parent.0.components {
            let last_component = &complex.components[complex.components.len() - 1];
            if !last_component.combinators.is_empty() {
                let trimmed = last_component.span.trim_right()?;
                let trimmed_span = SourceSpanWithContext::from_file_span(&trimmed)?;
                let complex_str = complex.to_css_string(true)?;
                return Err(Box::new(SassError::MultiSpan {
                    message: format!(
                        "Selector \"{}\" can't be used as a parent in a compound selector.",
                        complex_str
                    ),
                    span: trimmed_span,
                    primary_label: Some("outer selector".into()),
                    secondary: vec![(
                        SourceSpanWithContext::from_file_span(&first_simple_span).unwrap(),
                        "parent selector".into(),
                    )],
                    original_source: None,
                    cause: None,
                    loaded_urls: vec![],
                    trace: Default::default(),
                }));
            }

            let suffix = parent_selector.suffix();
            let last_simples = &last_component.selector.components;
            let component_span = component.selector.span()?;
            let last = if suffix.is_none() {
                let mut combined =
                    Vec::with_capacity(last_simples.len() + resolved_simples.len() - 1);
                combined.extend_from_slice(last_simples);
                combined.extend_from_slice(&resolved_simples[1..]);
                CompoundSelector::new(combined, component_span)
            } else {
                let added = last_simples[last_simples.len() - 1]
                    .add_suffix(suffix.unwrap_or(""))
                    .map_err(&wrap_err)?;
                let mut combined =
                    Vec::with_capacity(last_simples.len() + resolved_simples.len() - 1);
                combined.extend_from_slice(&last_simples[..last_simples.len() - 1]);
                combined.push(added);
                combined.extend_from_slice(&resolved_simples[1..]);
                CompoundSelector::new(combined, component_span)
            }
            .map_err(&wrap_err)?;

            let mut comps: Vec<ComplexSelectorComponent<'parse>> =
                Vec::with_capacity(complex.components.len());
            comps.extend_from_slice(&complex.components[..complex.components.len() - 1]);
            comps.push(ComplexSelectorComponent::new(
                Box::new(last),
                component.combinators.clone(),
                component.span,
            ));

            let cs = ComplexSelector::new(
                complex.leading_combinators.clone(),
                comps,
                component.span,
                complex.line_break,
            )
            .map_err(&wrap_err)?;
            result.push(cs);
        }
        Ok(Some(result))
    }

    /// Returns a SassScript list representing this selector, in the same
    /// format as `selector-parse()`: one space-separated list per complex
    /// (leading combinators, compound text, trailing combinators as unquoted
    /// strings), joined as a comma list. Matches Dart:
    /// `SelectorList.asSassList`.
    pub fn as_sass_list<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
    ) -> SassResult<Value<'parse>> {
        let mut complexes: Vec<Value<'parse>> = Vec::with_capacity(self.0.components.len());
        for complex in &self.0.components {
            let mut parts: Vec<Value<'parse>> = Vec::new();
            for lc in &complex.leading_combinators {
                parts.push(Value::new_with_arena(
                    arena,
                    ValueKind::String(SassString::new(
                        arena.alloc_str(&lc.value.to_string()),
                        false,
                    )),
                ));
            }
            for comp in &complex.components {
                parts.push(Value::new_with_arena(
                    arena,
                    ValueKind::String(SassString::new(
                        arena.alloc_str(&comp.selector.to_css_string(true)?),
                        false,
                    )),
                ));
                for c in &comp.combinators {
                    parts.push(Value::new_with_arena(
                        arena,
                        ValueKind::String(SassString::new(
                            arena.alloc_str(&c.value.to_string()),
                            false,
                        )),
                    ));
                }
            }
            complexes.push(Value::new_with_arena(
                arena,
                ValueKind::List(SassList::new(parts, ListSeparator::Space, false)),
            ));
        }
        Ok(Value::new_with_arena(
            arena,
            ValueKind::List(SassList::new(complexes, ListSeparator::Comma, false)),
        ))
    }

    // Returns a copy of `self` with `combinators` appended to each complex.
    // Matches Dart: `SelectorList.withAdditionalCombinators` (@internal).
    pub fn with_additional_combinators<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        combinators: &[CssValue<'parse, Combinator>],
    ) -> SassResult<Self> {
        if combinators.is_empty() {
            return Ok(*self);
        }
        let mut new_components = Vec::with_capacity(self.0.components.len());
        for comp in &self.0.components {
            new_components.push(comp.with_additional_combinators(combinators, false));
        }
        SelectorList::new(arena, new_components, self.0.span)
    }
}

impl<'parse> PartialEq for SelectorList<'parse> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<'parse> Eq for SelectorList<'parse> {}

impl<'parse> Hash for SelectorList<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.0.hash_code());
    }
}

// =============================================================================
// SelectorListIdentity — identity-based wrapper for store maps
// =============================================================================

/// Identity-based handle for the extension store's maps: equality and hash
/// are by allocation address, matching Dart's `Map.identity()`. Obtain via
/// [`SelectorList::identity`](SelectorList::identity); the store's
/// `clone_store` map uses this to translate old style-rule selectors to new
/// store boxes. Matches Dart usage in `extension_store.dart`.
#[derive(Clone, Copy, Debug)]
pub struct SelectorListIdentity<'parse>(pub(crate) &'parse SelectorListInner<'parse>);

impl<'parse> PartialEq for SelectorListIdentity<'parse> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl<'parse> Eq for SelectorListIdentity<'parse> {}

impl<'parse> Hash for SelectorListIdentity<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.0 as *const SelectorListInner<'parse>).hash(state);
    }
}

impl<'parse> From<&SelectorList<'parse>> for SelectorListIdentity<'parse> {
    fn from(sl: &SelectorList<'parse>) -> Self {
        SelectorListIdentity(sl.0)
    }
}

impl<'parse> From<SelectorList<'parse>> for SelectorListIdentity<'parse> {
    fn from(sl: SelectorList<'parse>) -> Self {
        SelectorListIdentity(sl.0)
    }
}

// =============================================================================
// Private helpers
// =============================================================================

/// Matches Go: containsParentVisitor — recurses into selector pseudos
/// (Dart: _ParentSelectorVisitor via SelectorSearchVisitor).
/// Returns whether `complex` recursively contains a parent (`&`) selector,
/// descending into pseudo selector arguments. Matches Dart:
/// `_containsParentSelector` (ast/selector/list.dart), realized through
/// `_ParentSelectorVisitor` (a `SelectorSearchVisitor`).
fn complex_contains_parent_selector<'parse>(complex: &ComplexSelector<'parse>) -> SassResult<bool> {
    for component in &complex.components {
        for simple in &component.selector.components {
            if simple.contains_parent_selector()? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Matches Go: parentSelectorWithSuffixVisitor — recurses into selector
/// pseudos (Dart: _ParentSelectorVisitor via SelectorSearchVisitor).
/// Finds the first suffixed parent (`&`) selector in `list`, descending
/// into pseudo selector arguments, or `None` when there is none. Backs the
/// top-level-suffix error in [`SelectorList::nest_within`]. Matches Dart:
/// `nestWithin`'s `_ParentSelectorVisitor` match on `suffix` + `span`
/// (ast/selector/list.dart).
fn find_parent_selector_with_suffix<'parse>(
    list: &'parse SelectorList<'parse>,
) -> Option<&'parse ParentSelector<'parse>> {
    for complex in &list.0.components {
        for component in &complex.components {
            for simple in &component.selector.components {
                if let Some(ps) = simple_parent_selector_with_suffix(simple) {
                    return Some(ps);
                }
            }
        }
    }
    None
}

// Matches Dart's `_ParentSelectorVisitor` arm: a bare suffixed `&`, or the
// first such selector inside a pseudo's selector argument.
fn simple_parent_selector_with_suffix<'parse>(
    simple: &'parse SimpleSelector<'parse>,
) -> Option<&'parse ParentSelector<'parse>> {
    match simple {
        SimpleSelector::Parent(ps) if ps.suffix().is_some() => Some(ps),
        SimpleSelector::Pseudo(pseudo) => match pseudo.selector {
            Some(ref sel) => match sel.as_ref() {
                Selector::List(inner) => find_parent_selector_with_suffix(inner),
                _ => None,
            },
            None => None,
        },
        _ => None,
    }
}

use crate::selector::parent::ParentSelector;

// Rust-only dispatch helper: Dart reads `.span` off the `Selector` base
// class; the Rust enum has no shared base, so each variant's span is matched
// explicitly.
fn simple_span<'parse>(s: &SimpleSelector<'parse>) -> SassResult<FileSpan<'parse>> {
    match s {
        SimpleSelector::Attribute(a) => a.span(),
        SimpleSelector::Class(c) => c.span(),
        SimpleSelector::Id(i) => i.span(),
        SimpleSelector::Pseudo(p) => p.span(),
        SimpleSelector::Parent(p) => p.span(),
        SimpleSelector::Placeholder(p) => p.span(),
        SimpleSelector::Type(t) => t.span(),
        SimpleSelector::Universal(u) => u.span(),
    }
}

// Interleaves the per-complex result lists round-robin: first result of
// each complex, then second of each, and so on. Matches Dart:
// `flattenVertically` (utils.dart) as used by `SelectorList.nestWithin`.
fn flatten_vertically<'parse>(
    components: Vec<Vec<ComplexSelector<'parse>>>,
) -> Vec<ComplexSelector<'parse>> {
    if components.len() == 1 {
        return components.into_iter().next().unwrap();
    }
    let mut queues: Vec<_> = components
        .into_iter()
        .map(|inner| {
            let v: Vec<ComplexSelector<'parse>> = inner.into_iter().collect();
            VecDeque::from(v)
        })
        .collect();
    let mut result = Vec::new();
    while !queues.is_empty() {
        let mut i = 0;
        while i < queues.len() {
            result.push(queues[i].pop_front().unwrap());
            if queues[i].is_empty() {
                queues.remove(i);
            } else {
                i += 1;
            }
        }
    }
    result
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use bumpalo::Bump;
    use std::collections::HashMap;

    use super::*;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::selector::class::ClassSelector;
    use crate::selector::complex_component::ComplexSelectorComponent;
    use crate::selector::compound::CompoundSelector;
    use crate::selector::parent::ParentSelector;
    use crate::selector::SimpleSelector;

    fn make_complex(name: &str) -> ComplexSelector<'static> {
        let s = SimpleSelector::Class(ClassSelector::new(name.into(), BOGUS_SPAN));
        let compound = Box::new(CompoundSelector::new(vec![s], BOGUS_SPAN).unwrap());
        let component = ComplexSelectorComponent::new(compound, vec![], BOGUS_SPAN);
        ComplexSelector::new(vec![], vec![component], BOGUS_SPAN, false).unwrap()
    }

    #[test]
    fn test_new() {
        let arena = Bump::new();
        let c = make_complex("foo");
        let list = SelectorList::new(&arena, vec![c], BOGUS_SPAN).unwrap();
        assert_eq!(list.0.components.len(), 1);
    }

    #[test]
    fn test_new_empty() {
        let arena = Bump::new();
        assert!(SelectorList::new(&arena, vec![], BOGUS_SPAN).is_err());
    }

    #[test]
    fn test_is_invisible() {
        let arena = Bump::new();
        let list = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        assert!(!list.is_invisible());
    }

    #[test]
    fn test_is_bogus() {
        let arena = Bump::new();
        let list = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        assert!(!list.is_bogus());
    }

    #[test]
    fn test_is_useless() {
        let arena = Bump::new();
        let list = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        assert!(!list.is_useless());
    }

    #[test]
    fn test_contains_parent_selector() {
        let arena = Bump::new();
        let list = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        assert!(!list.contains_parent_selector().unwrap());

        let parent = SimpleSelector::Parent(ParentSelector::new(&arena, BOGUS_SPAN, None));
        let compound = Box::new(CompoundSelector::new(vec![parent], BOGUS_SPAN).unwrap());
        let comp = ComplexSelectorComponent::new(compound, vec![], BOGUS_SPAN);
        let complex = ComplexSelector::new(vec![], vec![comp], BOGUS_SPAN, false).unwrap();
        let list2 = SelectorList::new(&arena, vec![complex], BOGUS_SPAN).unwrap();
        assert!(list2.contains_parent_selector().unwrap());
    }

    #[test]
    fn test_structural_eq() {
        let arena = Bump::new();
        let list1 = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        let list2 = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        assert_eq!(list1, list2, "structural equality: same components");
    }

    #[test]
    fn test_structural_ne() {
        let arena = Bump::new();
        let list1 = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        let list2 = SelectorList::new(
            &arena,
            vec![make_complex("foo"), make_complex("bar")],
            BOGUS_SPAN,
        )
        .unwrap();
        assert_ne!(list1, list2);
    }

    #[test]
    fn test_content_eq() {
        let arena = Bump::new();
        let list1 = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        let list2 = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        assert!(list1.content_eq(&list2));
    }

    #[test]
    fn test_clone_shares_identity() {
        let arena = Bump::new();
        let list1 = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        let list2 = list1;
        assert_eq!(list1, list2, "clone is structurally equal");
        assert_eq!(
            list1.identity(),
            list2.identity(),
            "clone shares identity via Rc"
        );
    }

    #[test]
    fn test_identity_different_not_equal() {
        let arena = Bump::new();
        let list1 = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        let list2 = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        assert_ne!(
            list1.identity(),
            list2.identity(),
            "different Rc → different identity"
        );
    }

    #[test]
    // Identity keys carry span memo `Cell`s excluded from `Eq`/`Hash`;
    // the lint is a false positive by design.
    #[allow(clippy::mutable_key_type)]
    fn test_selector_list_identity_hashmap_key() {
        let arena = Bump::new();
        let mut map: HashMap<SelectorListIdentity, i32> = HashMap::new();
        let list = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        let id = list.identity();
        map.insert(id, 1);
        assert_eq!(map.get(&id), Some(&1));

        let list2 = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        let id2 = list2.identity();
        assert_eq!(map.get(&id2), None, "different Rc should not be found");
    }

    #[test]
    fn test_with_additional_combinators() {
        let arena = Bump::new();
        let list = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        let comb = CssValue::new(Combinator::Child, BOGUS_SPAN);
        let result = list.with_additional_combinators(&arena, &[comb]).unwrap();
        assert_eq!(result.0.components.len(), 1);
    }

    #[test]
    fn test_flatten_vertically() {
        let a = vec![make_complex("1a")];
        let b = vec![make_complex("2a")];
        let result = flatten_vertically(vec![a, b]);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_selector_list_to_css_string() {
        let arena = Bump::new();
        let list = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        assert_eq!(list.to_css_string(true).unwrap(), ".foo");
    }

    #[test]
    fn test_selector_list_assert_not_bogus() {
        let arena = Bump::new();
        let list = SelectorList::new(&arena, vec![make_complex("foo")], BOGUS_SPAN).unwrap();
        assert!(list.assert_not_bogus(None, None).is_ok());
    }
}
