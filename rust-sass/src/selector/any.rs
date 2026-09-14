// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/any_selector.dart
// go-source: go/value/selector_any.go

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

// A visitor that visits each selector in a Sass selector AST and reports
// whether any of them matched.
//
// Every leaf method returns `false` by default; the compound visitors
// short-circuit on the first `true`. Matches Dart: `AnySelectorVisitor`
// (any_selector.dart) — an `@internal` mixin, so this stays a plain comment.
// The private `_IsInvisibleVisitor`, `_IsBogusVisitor`, `_IsUselessVisitor`
// and `_ContainsParentSelectorVisitor` in Dart's `ast/selector.dart` are all
// built on this mixin; their logic lives on the per-type `is_*` methods.
pub struct AnySelectorVisitor;

impl<'parse> SelectorVisitor<'parse> for AnySelectorVisitor {
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
