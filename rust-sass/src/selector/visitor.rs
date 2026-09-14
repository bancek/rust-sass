// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/interface/selector.dart
// go-source: go/value/selector_visitor.go

use crate::common::exception::SassResult;
use crate::selector::attribute::AttributeSelector;
use crate::selector::class::ClassSelector;
use crate::selector::complex::ComplexSelector;
use crate::selector::compound::CompoundSelector;
use crate::selector::id::IdSelector;
use crate::selector::list::SelectorList;
use crate::selector::parent::ParentSelector;
use crate::selector::placeholder::PlaceholderSelector;
use crate::selector::pseudo::PseudoSelector;
use crate::selector::ty::TypeSelector;
use crate::selector::universal::UniversalSelector;

/// An interface for [visitors](https://en.wikipedia.org/wiki/Visitor_pattern)
/// that traverse selectors.
///
/// Each AST node dispatches to its method via `accept()`; the associated
/// [`Output`](Self::Output) type lets one traversal serve many result kinds
/// (`bool` for the any/search visitors, `()` for the recursive visitor).
/// Matches Dart: `SelectorVisitor<T>` (visitor/interface/selector.dart).
//
// Note: `any.rs`, `recursive.rs`, and `search.rs` are not wired into the
// module tree (`mod.rs` declares no `mod any/recursive/search`), so the
// above names are intentionally unlinked.
pub trait SelectorVisitor<'parse> {
    type Output;

    fn visit_attribute_selector(
        &mut self,
        node: &AttributeSelector<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_class_selector(&mut self, node: &ClassSelector<'parse>) -> SassResult<Self::Output>;
    fn visit_complex_selector(
        &mut self,
        node: &ComplexSelector<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_compound_selector(
        &mut self,
        node: &CompoundSelector<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_id_selector(&mut self, node: &IdSelector<'parse>) -> SassResult<Self::Output>;
    fn visit_selector_list(&mut self, node: &SelectorList<'parse>) -> SassResult<Self::Output>;
    fn visit_parent_selector(&mut self, node: &ParentSelector<'parse>) -> SassResult<Self::Output>;
    fn visit_placeholder_selector(
        &mut self,
        node: &PlaceholderSelector<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_pseudo_selector(&mut self, node: &PseudoSelector<'parse>) -> SassResult<Self::Output>;
    fn visit_type_selector(&mut self, node: &TypeSelector<'parse>) -> SassResult<Self::Output>;
    fn visit_universal_selector(
        &mut self,
        node: &UniversalSelector<'parse>,
    ) -> SassResult<Self::Output>;
}
