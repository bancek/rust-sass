// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/interface/modifiable_css.dart (ModifiableCssVisitor) + lib/src/visitor/clone_css.dart (CloneCssVisitor methods mirror _CloneCssVisitor)
// go-source: go/value/css_modifiable_visitor.go

use bumpalo::Bump;

use crate::common::exception::SassResult;

use crate::ast::css::at_rule::ModifiableCssAtRule;
use crate::ast::css::comment::ModifiableCssComment;
use crate::ast::css::declaration::ModifiableCssDeclaration;
use crate::ast::css::import::ModifiableCssImport;
use crate::ast::css::keyframe_block::ModifiableCssKeyframeBlock;
use crate::ast::css::media_rule::ModifiableCssMediaRule;
use crate::ast::css::modifiable_node::ModifiableCssNode;
use crate::ast::css::style_rule::ModifiableCssStyleRule;
use crate::ast::css::stylesheet::ModifiableCssStylesheet;
use crate::ast::css::supports_rule::ModifiableCssSupportsRule;

/// An interface for visitors that traverse modifiable CSS statements.
///
/// Unlike the frozen counterpart (`super::visitor::CssVisitor`), the visited
/// nodes are the mutable evaluation-time forms. Each method takes the
/// concrete modifiable node and returns an associated
/// [`Output`](Self::Output).
pub trait ModifiableCssVisitor<'parse> {
    type Output;

    fn visit_css_at_rule(&mut self, node: &ModifiableCssAtRule<'parse>)
        -> SassResult<Self::Output>;
    fn visit_css_comment(
        &mut self,
        node: &ModifiableCssComment<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_css_declaration(
        &mut self,
        node: &ModifiableCssDeclaration<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_css_import(&mut self, node: &ModifiableCssImport<'parse>) -> SassResult<Self::Output>;
    fn visit_css_keyframe_block(
        &mut self,
        node: &ModifiableCssKeyframeBlock<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_css_media_rule(
        &mut self,
        node: &ModifiableCssMediaRule<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_css_style_rule(
        &mut self,
        node: &ModifiableCssStyleRule<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_css_stylesheet(
        &mut self,
        node: &ModifiableCssStylesheet<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_css_supports_rule(
        &mut self,
        node: &ModifiableCssSupportsRule<'parse>,
    ) -> SassResult<Self::Output>;
}

/// A visitor that deep-copies a modifiable CSS subtree.
///
/// Each method rebuilds its node (children included) as a fresh modifiable
/// node. Unlike [`ModifiableCssVisitor`], the visitor is taken by shared
/// reference with an explicit arena, so cloning works without mutable
/// visitor state.
pub trait CloneCssVisitor<'parse> {
    fn visit_css_at_rule<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssAtRule<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>>;
    fn visit_css_comment<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssComment<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>>;
    fn visit_css_declaration<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssDeclaration<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>>;
    fn visit_css_import<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssImport<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>>;
    fn visit_css_keyframe_block<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssKeyframeBlock<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>>;
    fn visit_css_media_rule<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssMediaRule<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>>;
    fn visit_css_style_rule<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssStyleRule<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>>;
    fn visit_css_stylesheet<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssStylesheet<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>>;
    fn visit_css_supports_rule<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssSupportsRule<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>>;
}
