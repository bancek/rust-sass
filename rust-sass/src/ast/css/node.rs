// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/css/node.dart
// go-source: go/value/css_node.go

use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::css::at_rule::CssAtRule;
use crate::ast::css::comment::CssComment;
use crate::ast::css::declaration::CssDeclaration;
use crate::ast::css::import::CssImport;
use crate::ast::css::keyframe_block::CssKeyframeBlock;
use crate::ast::css::media_rule::CssMediaRule;
use crate::ast::css::style_rule::CssStyleRule;
use crate::ast::css::stylesheet::CssStylesheet;
use crate::ast::css::supports_rule::CssSupportsRule;
use crate::ast::css::visitor::CssVisitor;

/// A statement in a plain CSS syntax tree.
///
/// This is the frozen output of evaluation: the serializer walks this enum.
/// During evaluation the parallel [`ModifiableCssNode`](super::modifiable_node::ModifiableCssNode)
/// tree is used instead; [`to_css_node`](super::modifiable_node::ModifiableCssNodeKind::to_css_node)
/// performs the one-time deep conversion at the end. See `docs/ref/ast.md`
/// (CSS AST section).
#[derive(Clone, Debug)]
pub enum CssNode<'parse> {
    Stylesheet(CssStylesheet<'parse>),
    StyleRule(CssStyleRule<'parse>),
    AtRule(CssAtRule<'parse>),
    Comment(CssComment<'parse>),
    Declaration(CssDeclaration<'parse>),
    Import(CssImport<'parse>),
    KeyframeBlock(CssKeyframeBlock<'parse>),
    MediaRule(CssMediaRule<'parse>),
    SupportsRule(CssSupportsRule<'parse>),
}

impl<'parse> CssNode<'parse> {
    /// Calls the appropriate visit method on `visitor`.
    pub fn accept<V: CssVisitor<'parse> + ?Sized>(&self, visitor: &mut V) -> SassResult<V::Output> {
        match self {
            CssNode::Stylesheet(node) => visitor.visit_css_stylesheet(node),
            CssNode::StyleRule(node) => visitor.visit_css_style_rule(node),
            CssNode::AtRule(node) => visitor.visit_css_at_rule(node),
            CssNode::Comment(node) => visitor.visit_css_comment(node),
            CssNode::Declaration(node) => visitor.visit_css_declaration(node),
            CssNode::Import(node) => visitor.visit_css_import(node),
            CssNode::KeyframeBlock(node) => visitor.visit_css_keyframe_block(node),
            CssNode::MediaRule(node) => visitor.visit_css_media_rule(node),
            CssNode::SupportsRule(node) => visitor.visit_css_supports_rule(node),
        }
    }

    /// Whether this is invisible and won't be emitted to the compiled stylesheet.
    ///
    /// Note that this doesn't consider nodes that contain loud comments to be
    /// invisible even though they're omitted in compressed mode.
    //
    // Matches Dart: `CssNode.isInvisible` (node.dart) — `@internal`, so this
    // would be a plain `//` on a non-shared helper, but it is ported here as
    // rustdoc because the logic is inlined on `CssNode` itself (the shared
    // `_IsInvisibleVisitor` lives on the modifiable tree; split rule).
    // Style rules check selector + children, at-rules are never invisible,
    // and every other parent is invisible iff all children are; leaf types
    // (comment, declaration, import) are visible.
    pub fn is_invisible(&self) -> bool {
        match self {
            CssNode::Stylesheet(s) => s.children.iter().all(|c| c.is_invisible()),
            CssNode::StyleRule(sr) => {
                sr.selector.is_invisible() || sr.children.iter().all(|c| c.is_invisible())
            }
            CssNode::AtRule(_) => false,
            CssNode::Comment(_) => false,
            CssNode::Declaration(_) => false,
            CssNode::Import(_) => false,
            CssNode::KeyframeBlock(k) => k.children.iter().all(|c| c.is_invisible()),
            CssNode::MediaRule(m) => m.children.iter().all(|c| c.is_invisible()),
            CssNode::SupportsRule(s) => s.children.iter().all(|c| c.is_invisible()),
        }
    }

    // Whether this node would be invisible even if style rule selectors within
    // it didn't have bogus combinators. Matches Dart:
    // `CssNode.isInvisibleOtherThanBogusCombinators` (node.dart) — `@internal`.
    pub fn is_invisible_other_than_bogus_combinators(&self) -> bool {
        match self {
            CssNode::Stylesheet(s) => s
                .children
                .iter()
                .all(|c| c.is_invisible_other_than_bogus_combinators()),
            CssNode::StyleRule(sr) => {
                sr.selector.is_invisible_other_than_bogus_combinators()
                    || sr
                        .children
                        .iter()
                        .all(|c| c.is_invisible_other_than_bogus_combinators())
            }
            CssNode::AtRule(_) => false,
            CssNode::Comment(_) => false,
            CssNode::Declaration(_) => false,
            CssNode::Import(_) => false,
            CssNode::KeyframeBlock(k) => k
                .children
                .iter()
                .all(|c| c.is_invisible_other_than_bogus_combinators()),
            CssNode::MediaRule(m) => m
                .children
                .iter()
                .all(|c| c.is_invisible_other_than_bogus_combinators()),
            CssNode::SupportsRule(s) => s
                .children
                .iter()
                .all(|c| c.is_invisible_other_than_bogus_combinators()),
        }
    }

    // Whether this node will be invisible when loud comments are stripped
    // (unpreserved `/* */` comments count as invisible). Matches Dart:
    // `CssNode.isInvisibleHidingComments` (node.dart) — `@internal`.
    pub fn is_invisible_hiding_comments(&self) -> bool {
        match self {
            CssNode::Stylesheet(s) => s.children.iter().all(|c| c.is_invisible_hiding_comments()),
            CssNode::StyleRule(sr) => {
                sr.selector.is_invisible()
                    || sr.children.iter().all(|c| c.is_invisible_hiding_comments())
            }
            CssNode::AtRule(_) => false,
            CssNode::Comment(c) => !c.is_preserved,
            CssNode::Declaration(_) => false,
            CssNode::Import(_) => false,
            CssNode::KeyframeBlock(k) => {
                k.children.iter().all(|c| c.is_invisible_hiding_comments())
            }
            CssNode::MediaRule(m) => m.children.iter().all(|c| c.is_invisible_hiding_comments()),
            CssNode::SupportsRule(s) => s.children.iter().all(|c| c.is_invisible_hiding_comments()),
        }
    }

    /// Whether this was generated from the last node in a nested Sass tree that
    /// got flattened during evaluation.
    ///
    /// Set by the evaluator on the last child of a root-level group, read by
    /// the serializer to emit an extra blank line between groups. Matches
    /// Dart: `CssNode.isGroupEnd` (node.dart).
    pub fn is_group_end(&self) -> bool {
        match self {
            CssNode::Stylesheet(s) => s.is_group_end,
            CssNode::StyleRule(sr) => sr.is_group_end,
            CssNode::AtRule(r) => r.is_group_end,
            CssNode::Comment(c) => c.is_group_end,
            CssNode::Declaration(d) => d.is_group_end,
            CssNode::Import(i) => i.is_group_end,
            CssNode::KeyframeBlock(k) => k.is_group_end,
            CssNode::MediaRule(m) => m.is_group_end,
            CssNode::SupportsRule(s) => s.is_group_end,
        }
    }

    /// Extra NESTED-output indent levels stamped by the evaluator (libsass
    /// `tabs()`); nonzero only on style/media/supports rules. Read by the
    /// serializer only for `OutputStyle::Nested`.
    pub fn tabs(&self) -> u32 {
        match self {
            CssNode::StyleRule(sr) => sr.tabs,
            CssNode::MediaRule(m) => m.tabs,
            CssNode::SupportsRule(s) => s.tabs,
            _ => 0,
        }
    }
}

impl<'parse> AstNode<'parse> for CssNode<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            CssNode::Stylesheet(node) => node.span(),
            CssNode::StyleRule(node) => node.span(),
            CssNode::AtRule(node) => node.span(),
            CssNode::Comment(node) => node.span(),
            CssNode::Declaration(node) => node.span(),
            CssNode::Import(node) => node.span(),
            CssNode::KeyframeBlock(node) => node.span(),
            CssNode::MediaRule(node) => node.span(),
            CssNode::SupportsRule(node) => node.span(),
        }
    }
}

impl<'parse> fmt::Display for CssNode<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CssNode::Stylesheet(node) => write!(f, "{node}"),
            CssNode::StyleRule(node) => write!(f, "{node}"),
            CssNode::AtRule(node) => write!(f, "{node}"),
            CssNode::Comment(node) => write!(f, "{node}"),
            CssNode::Declaration(node) => write!(f, "{node}"),
            CssNode::Import(node) => write!(f, "{node}"),
            CssNode::KeyframeBlock(node) => write!(f, "{node}"),
            CssNode::MediaRule(node) => write!(f, "{node}"),
            CssNode::SupportsRule(node) => write!(f, "{node}"),
        }
    }
}

// NOTE (Dart, node.dart): new at-rule implementations should add themselves
// to `AtRootRule`'s exclude logic.
/// A [`CssNode`] that can have child statements.
///
/// Only the six parent variants are representable here; leaf types (comment,
/// declaration, import) and childless at-rules have no child list.
#[derive(Clone, Debug)]
pub enum CssParentNode<'parse> {
    Stylesheet(CssStylesheet<'parse>),
    StyleRule(CssStyleRule<'parse>),
    AtRule(CssAtRule<'parse>),
    KeyframeBlock(CssKeyframeBlock<'parse>),
    MediaRule(CssMediaRule<'parse>),
    SupportsRule(CssSupportsRule<'parse>),
}

impl<'parse> CssParentNode<'parse> {
    /// The child statements of this node.
    pub fn children(&self) -> &[CssNode<'parse>] {
        match self {
            CssParentNode::Stylesheet(s) => &s.children,
            CssParentNode::StyleRule(sr) => &sr.children,
            CssParentNode::AtRule(r) => &r.children,
            CssParentNode::KeyframeBlock(k) => &k.children,
            CssParentNode::MediaRule(m) => &m.children,
            CssParentNode::SupportsRule(s) => &s.children,
        }
    }

    /// Whether the rule has no children and should be emitted without curly
    /// braces.
    ///
    /// This implies `children` is empty, but the reverse is not true — for a
    /// rule like `@foo {}`, [`children`](CssParentNode::children) is empty but
    /// `is_childless` is `false`. Only [`AtRule`](CssAtRule) can be childless.
    pub fn is_childless(&self) -> bool {
        match self {
            CssParentNode::AtRule(r) => r.childless,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ast_css_value::CssValue;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    fn make_val<'compile, 'parse>(arena: &'compile Bump, s: &str) -> CssValue<'parse, String>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        CssValue::new(s.into(), make_span(arena, s))
    }

    #[test]
    fn test_css_node_accept_dispatch() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let c = CssNode::Comment(CssComment::new("/* test */".into(), span));
        let mut vis = MockCssVisitor {
            visited: String::new(),
        };
        let result = c.accept(&mut vis).unwrap();
        assert_eq!(result, "CssComment");
        assert_eq!(vis.visited, "CssComment");
    }

    #[test]
    fn test_css_node_is_invisible_empty_stylesheet() {
        let arena = Bump::new();
        let span = make_span(&arena, "");
        let ss = CssStylesheet::new(vec![], span);
        let node = CssNode::Stylesheet(ss);
        assert!(node.is_invisible());
    }

    #[test]
    fn test_css_node_is_invisible_comment() {
        let arena = Bump::new();
        let span = make_span(&arena, "/* test */");
        let c = CssNode::Comment(CssComment::new("/* test */".into(), span));
        assert!(!c.is_invisible());
    }

    #[test]
    fn test_css_node_is_invisible_hiding_comments_preserved() {
        let arena = Bump::new();
        let span = make_span(&arena, "/*! test */");
        let c = CssNode::Comment(CssComment::new("/*! test */".into(), span));
        assert!(!c.is_invisible_hiding_comments());
    }

    #[test]
    fn test_css_node_is_invisible_hiding_comments_unpreserved() {
        let arena = Bump::new();
        let span = make_span(&arena, "/* test */");
        let c = CssNode::Comment(CssComment::new("/* test */".into(), span));
        assert!(c.is_invisible_hiding_comments());
    }

    #[test]
    fn test_css_node_is_invisible_at_rule() {
        let arena = Bump::new();
        let span = make_span(&arena, "@media {}");
        let name = make_val(&arena, "media");
        let r = CssNode::AtRule(CssAtRule::new(name, span, false, None));
        assert!(!r.is_invisible());
    }

    #[test]
    fn test_css_parent_node_children() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let ss = CssStylesheet::new(vec![], span);
        let parent = CssParentNode::Stylesheet(ss);
        assert_eq!(parent.children().len(), 0);
        assert!(!parent.is_childless());
    }

    #[test]
    fn test_css_parent_node_is_childless() {
        let arena = Bump::new();
        let span = make_span(&arena, "@import;");
        let name = make_val(&arena, "import");
        let r = CssAtRule::new(name, span, true, None);
        let parent = CssParentNode::AtRule(r);
        assert!(parent.is_childless());
    }

    struct MockCssVisitor {
        visited: String,
    }

    impl<'parse> CssVisitor<'parse> for MockCssVisitor {
        type Output = String;

        fn visit_css_at_rule(&mut self, _: &CssAtRule<'parse>) -> SassResult<String> {
            self.visited = "CssAtRule".into();
            Ok("CssAtRule".into())
        }
        fn visit_css_comment(&mut self, _: &CssComment<'parse>) -> SassResult<String> {
            self.visited = "CssComment".into();
            Ok("CssComment".into())
        }
        fn visit_css_declaration(&mut self, _: &CssDeclaration<'parse>) -> SassResult<String> {
            self.visited = "CssDeclaration".into();
            Ok("CssDeclaration".into())
        }
        fn visit_css_import(&mut self, _: &CssImport<'parse>) -> SassResult<String> {
            self.visited = "CssImport".into();
            Ok("CssImport".into())
        }
        fn visit_css_keyframe_block(&mut self, _: &CssKeyframeBlock<'parse>) -> SassResult<String> {
            self.visited = "CssKeyframeBlock".into();
            Ok("CssKeyframeBlock".into())
        }
        fn visit_css_media_rule(&mut self, _: &CssMediaRule<'parse>) -> SassResult<String> {
            self.visited = "CssMediaRule".into();
            Ok("CssMediaRule".into())
        }
        fn visit_css_style_rule(&mut self, _: &CssStyleRule<'parse>) -> SassResult<String> {
            self.visited = "CssStyleRule".into();
            Ok("CssStyleRule".into())
        }
        fn visit_css_stylesheet(&mut self, _: &CssStylesheet<'parse>) -> SassResult<String> {
            self.visited = "CssStylesheet".into();
            Ok("CssStylesheet".into())
        }
        fn visit_css_supports_rule(&mut self, _: &CssSupportsRule<'parse>) -> SassResult<String> {
            self.visited = "CssSupportsRule".into();
            Ok("CssSupportsRule".into())
        }
    }
}
