// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/interface/css.dart
// go-source: go/value/css_visitor.go

use crate::common::exception::SassResult;

use crate::ast::css::at_rule::CssAtRule;
use crate::ast::css::comment::CssComment;
use crate::ast::css::declaration::CssDeclaration;
use crate::ast::css::import::CssImport;
use crate::ast::css::keyframe_block::CssKeyframeBlock;
use crate::ast::css::media_rule::CssMediaRule;
use crate::ast::css::style_rule::CssStyleRule;
use crate::ast::css::stylesheet::CssStylesheet;
use crate::ast::css::supports_rule::CssSupportsRule;

/// An interface for visitors that traverse frozen CSS statements.
///
/// This is the serialization-time counterpart to the modifiable visitor
/// (`super::modifiable_visitor::ModifiableCssVisitor`): the same 9 node
/// kinds, but in their immutable forms.
pub trait CssVisitor<'parse> {
    type Output;

    fn visit_css_at_rule(&mut self, node: &CssAtRule<'parse>) -> SassResult<Self::Output>;
    fn visit_css_comment(&mut self, node: &CssComment<'parse>) -> SassResult<Self::Output>;
    fn visit_css_declaration(&mut self, node: &CssDeclaration<'parse>) -> SassResult<Self::Output>;
    fn visit_css_import(&mut self, node: &CssImport<'parse>) -> SassResult<Self::Output>;
    fn visit_css_keyframe_block(
        &mut self,
        node: &CssKeyframeBlock<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_css_media_rule(&mut self, node: &CssMediaRule<'parse>) -> SassResult<Self::Output>;
    fn visit_css_style_rule(&mut self, node: &CssStyleRule<'parse>) -> SassResult<Self::Output>;
    fn visit_css_stylesheet(&mut self, node: &CssStylesheet<'parse>) -> SassResult<Self::Output>;
    fn visit_css_supports_rule(
        &mut self,
        node: &CssSupportsRule<'parse>,
    ) -> SassResult<Self::Output>;
}
