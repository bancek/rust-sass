// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/interface/interpolated_selector.dart
// go-source: go/value/sass_interpolated_selector_visitor.go

use crate::common::exception::SassResult;

use crate::ast::sass::interpolated_selector::attribute::InterpolatedAttributeSelector;
use crate::ast::sass::interpolated_selector::class::InterpolatedClassSelector;
use crate::ast::sass::interpolated_selector::complex::InterpolatedComplexSelector;
use crate::ast::sass::interpolated_selector::compound::InterpolatedCompoundSelector;
use crate::ast::sass::interpolated_selector::id::InterpolatedIDSelector;
use crate::ast::sass::interpolated_selector::list::InterpolatedSelectorList;
use crate::ast::sass::interpolated_selector::parent::InterpolatedParentSelector;
use crate::ast::sass::interpolated_selector::placeholder::InterpolatedPlaceholderSelector;
use crate::ast::sass::interpolated_selector::pseudo::InterpolatedPseudoSelector;
use crate::ast::sass::interpolated_selector::ty::InterpolatedTypeSelector;
use crate::ast::sass::interpolated_selector::universal::InterpolatedUniversalSelector;

/// A visitor that traverses interpolated selectors.
///
/// One method per concrete selector type; the enums dispatch via
/// [`accept`](super::selector::InterpolatedSelector::accept) and
/// [`accept`](super::simple::InterpolatedSimpleSelector::accept).
pub trait InterpolatedSelectorVisitor<'parse> {
    /// The result type produced by each visit method.
    type Output;

    /// Visits an attribute selector.
    fn visit_attribute_selector(
        &mut self,
        node: &InterpolatedAttributeSelector<'parse>,
    ) -> SassResult<Self::Output>;
    /// Visits a class selector.
    fn visit_class_selector(
        &mut self,
        node: &InterpolatedClassSelector<'parse>,
    ) -> SassResult<Self::Output>;
    /// Visits a complex selector.
    fn visit_complex_selector(
        &mut self,
        node: &InterpolatedComplexSelector<'parse>,
    ) -> SassResult<Self::Output>;
    /// Visits a compound selector.
    fn visit_compound_selector(
        &mut self,
        node: &InterpolatedCompoundSelector<'parse>,
    ) -> SassResult<Self::Output>;
    /// Visits an ID selector.
    fn visit_id_selector(
        &mut self,
        node: &InterpolatedIDSelector<'parse>,
    ) -> SassResult<Self::Output>;
    /// Visits a parent selector.
    fn visit_parent_selector(
        &mut self,
        node: &InterpolatedParentSelector<'parse>,
    ) -> SassResult<Self::Output>;
    /// Visits a placeholder selector.
    fn visit_placeholder_selector(
        &mut self,
        node: &InterpolatedPlaceholderSelector<'parse>,
    ) -> SassResult<Self::Output>;
    /// Visits a pseudo selector.
    fn visit_pseudo_selector(
        &mut self,
        node: &InterpolatedPseudoSelector<'parse>,
    ) -> SassResult<Self::Output>;
    /// Visits a selector list.
    fn visit_selector_list(
        &mut self,
        node: &InterpolatedSelectorList<'parse>,
    ) -> SassResult<Self::Output>;
    /// Visits a type selector.
    fn visit_type_selector(
        &mut self,
        node: &InterpolatedTypeSelector<'parse>,
    ) -> SassResult<Self::Output>;
    /// Visits a universal selector.
    fn visit_universal_selector(
        &mut self,
        node: &InterpolatedUniversalSelector<'parse>,
    ) -> SassResult<Self::Output>;
}
