// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/recursive_selector.dart
// go-source: go/value/selector_recursive_selector.go

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

/// A visitor that recursively traverses each component of a selector AST.
///
/// Leaf selectors are no-ops; compound visitors descend into components and
/// into the selector argument of pseudo-selectors. Matches Dart:
/// `RecursiveSelectorVisitor` (recursive_selector.dart).
pub struct RecursiveSelectorVisitor;

impl<'parse> SelectorVisitor<'parse> for RecursiveSelectorVisitor {
    type Output = ();

    fn visit_attribute_selector(&mut self, _: &AttributeSelector<'parse>) -> SassResult<()> {
        Ok(())
    }
    fn visit_class_selector(&mut self, _: &ClassSelector<'parse>) -> SassResult<()> {
        Ok(())
    }
    fn visit_id_selector(&mut self, _: &IdSelector<'parse>) -> SassResult<()> {
        Ok(())
    }
    fn visit_parent_selector(&mut self, _: &ParentSelector<'parse>) -> SassResult<()> {
        Ok(())
    }
    fn visit_placeholder_selector(&mut self, _: &PlaceholderSelector<'parse>) -> SassResult<()> {
        Ok(())
    }
    fn visit_type_selector(&mut self, _: &TypeSelector<'parse>) -> SassResult<()> {
        Ok(())
    }
    fn visit_universal_selector(&mut self, _: &UniversalSelector<'parse>) -> SassResult<()> {
        Ok(())
    }
    fn visit_pseudo_selector(&mut self, pseudo: &PseudoSelector<'parse>) -> SassResult<()> {
        if let Some(ref sel) = pseudo.selector {
            sel.accept(self)
        } else {
            Ok(())
        }
    }
    fn visit_complex_selector(&mut self, complex: &ComplexSelector<'parse>) -> SassResult<()> {
        for component in &complex.components {
            self.visit_compound_selector(&component.selector)?;
        }
        Ok(())
    }
    fn visit_compound_selector(&mut self, compound: &CompoundSelector<'parse>) -> SassResult<()> {
        for simple in &compound.components {
            simple.accept(self)?;
        }
        Ok(())
    }
    fn visit_selector_list(&mut self, list: &SelectorList<'parse>) -> SassResult<()> {
        for complex in &list.components {
            self.visit_complex_selector(complex)?;
        }
        Ok(())
    }
}
