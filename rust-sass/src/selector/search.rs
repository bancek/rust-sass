// Copyright 2023 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/selector_search.dart
// go-source: go/value/selector_search.go

use crate::common::exception::SassResult;
use crate::selector::visitor::SelectorVisitor;
use crate::selector::ComplexSelector;
use crate::selector::CompoundSelector;
use crate::selector::PseudoSelector;
use crate::selector::SelectorList;
use crate::selector::{
    AttributeSelector, ClassSelector, IdSelector, ParentSelector, PlaceholderSelector,
    TypeSelector, UniversalSelector,
};

/// A [`SelectorVisitor`](super::visitor::SelectorVisitor) that returns the
/// first match found while traversing the selector AST.
///
/// Each leaf method returns `false` by default; the compound visitors
/// short-circuit on the first `true`. Matches Dart: `SelectorSearchVisitor`
/// (selector_search.dart), except Dart is generic over `T?` (first non-`null`
/// result) while this port fixes `Output = bool`. Extend this pattern —
// return the sought node instead of `bool` — to find the first instance of
// particular nodes, as Dart's `_ParentSelectorVisitor` does.
pub struct SelectorSearchVisitor;

impl<'parse> SelectorVisitor<'parse> for SelectorSearchVisitor {
    type Output = bool;

    fn visit_attribute_selector(&mut self, _: &AttributeSelector<'parse>) -> SassResult<bool> {
        Ok(false)
    }
    fn visit_class_selector(&mut self, _: &ClassSelector<'parse>) -> SassResult<bool> {
        Ok(false)
    }
    fn visit_id_selector(&mut self, _: &IdSelector<'parse>) -> SassResult<bool> {
        Ok(false)
    }
    fn visit_parent_selector(&mut self, _: &ParentSelector<'parse>) -> SassResult<bool> {
        Ok(false)
    }
    fn visit_placeholder_selector(&mut self, _: &PlaceholderSelector<'parse>) -> SassResult<bool> {
        Ok(false)
    }
    fn visit_type_selector(&mut self, _: &TypeSelector<'parse>) -> SassResult<bool> {
        Ok(false)
    }
    fn visit_universal_selector(&mut self, _: &UniversalSelector<'parse>) -> SassResult<bool> {
        Ok(false)
    }
    fn visit_pseudo_selector(&mut self, pseudo: &PseudoSelector<'parse>) -> SassResult<bool> {
        if let Some(ref sel) = pseudo.selector {
            sel.accept(self)
        } else {
            Ok(false)
        }
    }
    fn visit_complex_selector(&mut self, complex: &ComplexSelector<'parse>) -> SassResult<bool> {
        for component in &complex.components {
            if self.visit_compound_selector(&component.selector)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn visit_compound_selector(&mut self, compound: &CompoundSelector<'parse>) -> SassResult<bool> {
        for simple in &compound.components {
            if simple.accept(self)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn visit_selector_list(&mut self, list: &SelectorList<'parse>) -> SassResult<bool> {
        for complex in &list.components {
            if self.visit_complex_selector(complex)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
