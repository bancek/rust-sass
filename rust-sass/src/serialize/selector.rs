// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

//! Selector serialization: one `visit_*` per selector node (Dart's
//! `visitAttributeSelector`…`visitUniversalSelector` plus `_writeCombinators`).
//!
//! Thin `SelectorVisitor` wrappers delegate to free `visit_*_impl` functions
//! so pseudo-selector arguments can dispatch by `match` where `accept()` is
//! unavailable. `serializeSelector` has no Rust entry point yet — style rules
//! route through `visit_selector_list_impl`.

// dart-source: lib/src/visitor/serialize.dart (visitAttributeSelector, visitClassSelector, visitComplexSelector, _writeCombinators, visitCompoundSelector, visitIDSelector, visitSelectorList, visitParentSelector, visitPlaceholderSelector, visitPseudoSelector, visitTypeSelector, visitUniversalSelector)
// go-source: go/value/visitor_selector.go

use crate::common::ast_css_value::CssValue;
use crate::parse::parser::is_identifier;
use crate::selector::Selector;
use crate::selector::SimpleSelector;
use crate::serialize::string::visit_quoted_string;
use std::fmt::Write;

use crate::common::SassResult;
use crate::selector::attribute::AttributeSelector;
use crate::selector::class::ClassSelector;
use crate::selector::combinator::Combinator;
use crate::selector::complex::ComplexSelector;
use crate::selector::compound::CompoundSelector;
use crate::selector::id::IdSelector;
use crate::selector::list::SelectorList;
use crate::selector::parent::ParentSelector;
use crate::selector::placeholder::PlaceholderSelector;
use crate::selector::pseudo::PseudoSelector;
use crate::selector::ty::TypeSelector;
use crate::selector::universal::UniversalSelector;
use crate::selector::visitor::SelectorVisitor;
use crate::source_map_buffer::SourceMapBuffer;

use crate::serialize::{write_between, OutputStyle, SerializeState, SerializeVisitor};

impl<'parse> SelectorVisitor<'parse> for SerializeVisitor<'parse> {
    type Output = ();

    fn visit_selector_list(&mut self, list: &SelectorList<'parse>) -> SassResult<()> {
        visit_selector_list_impl(&mut self.buffer, &self.inner, list)
    }
    fn visit_complex_selector(&mut self, c: &ComplexSelector<'parse>) -> SassResult<()> {
        visit_complex_selector_impl(&mut self.buffer, &self.inner, c)
    }
    fn visit_compound_selector(&mut self, c: &CompoundSelector<'parse>) -> SassResult<()> {
        visit_compound_selector_impl(&mut self.buffer, &self.inner, c)
    }
    fn visit_attribute_selector(&mut self, a: &AttributeSelector<'parse>) -> SassResult<()> {
        visit_attribute_selector_impl(&mut self.buffer, &self.inner, a)
    }
    fn visit_class_selector(&mut self, c: &ClassSelector<'parse>) -> SassResult<()> {
        visit_class_selector_impl(&mut self.buffer, c)
    }
    fn visit_id_selector(&mut self, i: &IdSelector<'parse>) -> SassResult<()> {
        visit_id_selector_impl(&mut self.buffer, i)
    }
    fn visit_placeholder_selector(&mut self, p: &PlaceholderSelector<'parse>) -> SassResult<()> {
        visit_placeholder_selector_impl(&mut self.buffer, p)
    }
    fn visit_pseudo_selector(&mut self, p: &PseudoSelector<'parse>) -> SassResult<()> {
        visit_pseudo_selector_impl(&mut self.buffer, &self.inner, p)
    }
    fn visit_type_selector(&mut self, t: &TypeSelector<'parse>) -> SassResult<()> {
        visit_type_selector_impl(&mut self.buffer, t)
    }
    fn visit_universal_selector(&mut self, u: &UniversalSelector<'parse>) -> SassResult<()> {
        visit_universal_selector_impl(&mut self.buffer, u)
    }
    fn visit_parent_selector(&mut self, p: &ParentSelector<'parse>) -> SassResult<()> {
        visit_parent_selector_impl(&mut self.buffer, p)
    }
}

/// Comma-separated complexes, skipping invisible ones outside inspect mode;
/// line-broken complexes re-indent (Dart's `visitSelectorList`).
pub(crate) fn visit_selector_list_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    list: &SelectorList<'_>,
) -> SassResult<()> {
    let complexes: Vec<&ComplexSelector<'_>> = if state.inspect {
        list.0.components.iter().collect()
    } else {
        list.0
            .components
            .iter()
            .filter(|c| !c.is_invisible())
            .collect()
    };
    let mut first = true;
    for c in &complexes {
        if first {
            first = false;
        } else {
            buf.write_char(',').unwrap();
            if c.line_break {
                // Multi-line selectors collapse to `, ` in COMPACT (libsass
                // suppresses `hasPostLineBreak`, `inspect.cpp:1101`); the
                // block break is a space there and a line feed elsewhere.
                state.write_block_break(buf);
                state.write_indentation(buf);
            } else {
                state.write_optional_space(buf);
            }
        }
        visit_complex_selector_impl(buf, state, c)?;
    }
    Ok(())
}

/// Leading combinators, compounds, and inner combinators with
/// compressed-aware spacing (Dart's `visitComplexSelector`).
pub(crate) fn visit_complex_selector_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    complex: &ComplexSelector<'_>,
) -> SassResult<()> {
    write_combinators_impl(buf, state, &complex.leading_combinators)?;
    if !complex.leading_combinators.is_empty() && !complex.components.is_empty() {
        state.write_optional_space(buf);
    }
    for (i, comp) in complex.components.iter().enumerate() {
        visit_compound_selector_impl(buf, state, &comp.selector)?;
        if !comp.combinators.is_empty() {
            state.write_optional_space(buf);
        }
        write_combinators_impl(buf, state, &comp.combinators)?;
        if i != complex.components.len() - 1
            && (!matches!(state.style, OutputStyle::Compressed) || comp.combinators.is_empty())
        {
            buf.write_char(' ').unwrap();
        }
    }
    Ok(())
}

/// Concatenated simple selectors; an emptied compound (all parts optimized
/// away) emits `*` (Dart's `visitCompoundSelector`).
pub(crate) fn visit_compound_selector_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    compound: &CompoundSelector<'_>,
) -> SassResult<()> {
    let start = buf.len();
    for simple in &compound.components {
        visit_simple_selector_impl(buf, state, simple)?;
    }
    if buf.len() == start {
        buf.write_char('*').unwrap();
    }
    Ok(())
}

// Manual `match` dispatch for nested positions (pseudo args); Dart uses `accept`.
fn visit_simple_selector_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    simple: &SimpleSelector<'_>,
) -> SassResult<()> {
    match simple {
        SimpleSelector::Attribute(a) => visit_attribute_selector_impl(buf, state, a),
        SimpleSelector::Class(c) => visit_class_selector_impl(buf, c),
        SimpleSelector::Id(i) => visit_id_selector_impl(buf, i),
        SimpleSelector::Pseudo(p) => visit_pseudo_selector_impl(buf, state, p),
        SimpleSelector::Parent(p) => visit_parent_selector_impl(buf, p),
        SimpleSelector::Placeholder(p) => visit_placeholder_selector_impl(buf, p),
        SimpleSelector::Type(t) => visit_type_selector_impl(buf, t),
        SimpleSelector::Universal(u) => visit_universal_selector_impl(buf, u),
    }
}

/// `[name op value modifier]`; identifier-safe values stay bare except
/// `--`-prefixed ones (IE11 needs them quoted), others quote
/// (Dart's `visitAttributeSelector`).
pub(crate) fn visit_attribute_selector_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    attr: &AttributeSelector<'_>,
) -> SassResult<()> {
    buf.write_char('[').unwrap();
    write!(buf, "{}", attr.name).unwrap();
    if let Some(ref op) = attr.op {
        write!(buf, "{}", op).unwrap();
        if let Some(ref value) = attr.value {
            let is_ident = is_identifier(value) && !value.starts_with("--");
            if is_ident {
                write!(buf, "{}", value).unwrap();
                if attr.modifier.is_some() {
                    buf.write_char(' ').unwrap();
                }
            } else {
                visit_quoted_string(buf, state, value);
                if attr.modifier.is_some() {
                    state.write_optional_space(buf);
                }
            }
        }
    }
    if let Some(ref m) = attr.modifier {
        write!(buf, "{}", m).unwrap();
    }
    buf.write_char(']').unwrap();
    Ok(())
}

/// `.name` (Dart's `visitClassSelector`).
pub(crate) fn visit_class_selector_impl(
    buf: &mut SourceMapBuffer<'_>,
    c: &ClassSelector<'_>,
) -> SassResult<()> {
    buf.write_char('.').unwrap();
    write!(buf, "{}", c.name).unwrap();
    Ok(())
}

/// `#name` (Dart's `visitIDSelector`).
pub(crate) fn visit_id_selector_impl(
    buf: &mut SourceMapBuffer<'_>,
    i: &IdSelector<'_>,
) -> SassResult<()> {
    buf.write_char('#').unwrap();
    write!(buf, "{}", i.name).unwrap();
    Ok(())
}

/// `%name` (Dart's `visitPlaceholderSelector`).
pub(crate) fn visit_placeholder_selector_impl(
    buf: &mut SourceMapBuffer<'_>,
    p: &PlaceholderSelector<'_>,
) -> SassResult<()> {
    buf.write_char('%').unwrap();
    write!(buf, "{}", p.name).unwrap();
    Ok(())
}

/// `:name(args selector)` with `::` for syntactic elements; `:not()` over
/// only-invisible selectors (≡ `*`) emits nothing
/// (Dart's `visitPseudoSelector`).
pub(crate) fn visit_pseudo_selector_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    pseudo: &PseudoSelector<'_>,
) -> SassResult<()> {
    if pseudo.name == "not" {
        if let Some(ref sel) = pseudo.selector {
            if let Selector::List(ref sl) = **sel {
                if sl.is_invisible() {
                    return Ok(());
                }
            }
        }
    }
    buf.write_char(':').unwrap();
    if !pseudo.is_syntactic_class {
        buf.write_char(':').unwrap();
    }
    write!(buf, "{}", pseudo.name).unwrap();
    if pseudo.argument.is_none() && pseudo.selector.is_none() {
        return Ok(());
    }
    buf.write_char('(').unwrap();
    if let Some(ref a) = pseudo.argument {
        write!(buf, "{}", a).unwrap();
        if pseudo.selector.is_some() {
            buf.write_char(' ').unwrap();
        }
    }
    if let Some(ref sel) = pseudo.selector {
        visit_selector_impl(buf, state, sel)?;
    }
    buf.write_char(')').unwrap();
    Ok(())
}

/// Bare type name (Dart's `visitTypeSelector`).
pub(crate) fn visit_type_selector_impl(
    buf: &mut SourceMapBuffer<'_>,
    t: &TypeSelector<'_>,
) -> SassResult<()> {
    write!(buf, "{}", t.name).unwrap();
    Ok(())
}

/// Optional `namespace|` plus `*` (Dart's `visitUniversalSelector`).
pub(crate) fn visit_universal_selector_impl(
    buf: &mut SourceMapBuffer<'_>,
    u: &UniversalSelector<'_>,
) -> SassResult<()> {
    if let Some(ref ns) = u.namespace {
        write!(buf, "{}", ns).unwrap();
        buf.write_char('|').unwrap();
    }
    buf.write_char('*').unwrap();
    Ok(())
}

/// `&` plus suffix (Dart's `visitParentSelector`).
pub(crate) fn visit_parent_selector_impl(
    buf: &mut SourceMapBuffer<'_>,
    p: &ParentSelector<'_>,
) -> SassResult<()> {
    buf.write_char('&').unwrap();
    if let Some(s) = p.suffix() {
        write!(buf, "{}", s).unwrap();
    }
    Ok(())
}

// Manual `match` dispatch for pseudo-selector arguments; Dart uses `accept`.
fn visit_selector_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    sel: &Selector<'_>,
) -> SassResult<()> {
    match sel {
        Selector::Simple(s) => visit_simple_selector_impl(buf, state, s),
        Selector::Compound(c) => visit_compound_selector_impl(buf, state, c),
        Selector::Complex(c) => visit_complex_selector_impl(buf, state, c),
        Selector::List(l) => visit_selector_list_impl(buf, state, l),
    }
}

/// Joins combinators with spaces in expanded mode, nothing in compressed
/// (Dart's `_writeCombinators`).
pub(crate) fn write_combinators_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    combinators: &[CssValue<'_, Combinator>],
) -> SassResult<()> {
    let sep = match state.style {
        OutputStyle::Compressed => "",
        OutputStyle::Expanded | OutputStyle::Nested | OutputStyle::Compact => " ",
    };
    write_between(buf, combinators, sep, |buf, cv| {
        write!(buf, "{}", cv.value).unwrap();
        Ok(())
    })
}
