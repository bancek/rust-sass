// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/every_css.dart
// go-source: go/value/css_every.go

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
use crate::ast::css::visitor::CssVisitor;

// In Dart this is an `@internal` mixin, so it stays a plain comment here
// (rule 2): the same all-children-match traversal, without subtyping.
/// A visitor that visits each node in a CSS AST and returns `true` if all
/// individual visit methods return `true`.
///
/// Each method returns `false` by default for leaf types (Comment, Declaration,
/// Import). Parent types iterate their children.
pub struct EveryCssVisitor;

impl<'parse> CssVisitor<'parse> for EveryCssVisitor {
    type Output = bool;

    fn visit_css_at_rule(&mut self, node: &CssAtRule<'parse>) -> SassResult<bool> {
        for child in &node.children {
            if !child.accept(self)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn visit_css_comment(&mut self, _node: &CssComment<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_css_declaration(&mut self, _node: &CssDeclaration<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_css_import(&mut self, _node: &CssImport<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_css_keyframe_block(&mut self, node: &CssKeyframeBlock<'parse>) -> SassResult<bool> {
        for child in &node.children {
            if !child.accept(self)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn visit_css_media_rule(&mut self, node: &CssMediaRule<'parse>) -> SassResult<bool> {
        for child in &node.children {
            if !child.accept(self)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn visit_css_style_rule(&mut self, node: &CssStyleRule<'parse>) -> SassResult<bool> {
        for child in &node.children {
            if !child.accept(self)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn visit_css_stylesheet(&mut self, node: &CssStylesheet<'parse>) -> SassResult<bool> {
        for child in &node.children {
            if !child.accept(self)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn visit_css_supports_rule(&mut self, node: &CssSupportsRule<'parse>) -> SassResult<bool> {
        for child in &node.children {
            if !child.accept(self)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::css::node::CssNode;
    use crate::common::ast_css_value::CssValue;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::value::string::SassString;
    use crate::value::{Value, ValueKind};
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
    fn test_every_empty_stylesheet() {
        let arena = Bump::new();
        let span = make_span(&arena, "");
        let ss = CssStylesheet::new(vec![], span);
        let node = CssNode::Stylesheet(ss);
        let mut v = EveryCssVisitor;
        let result = node.accept(&mut v).unwrap();
        assert!(result);
    }

    #[test]
    fn test_every_comment_returns_false() {
        let arena = Bump::new();
        let span = make_span(&arena, "/* test */");
        let c = CssNode::Comment(CssComment::new("/* test */".into(), span));
        let mut v = EveryCssVisitor;
        let result = c.accept(&mut v).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_every_declaration_returns_false() {
        let arena = Bump::new();
        let span = make_span(&arena, "color: red;");
        let name = make_val(&arena, "color");
        let s = SassString::new("red", false);
        let val_span = make_span(&arena, "red");
        let val = CssValue::new(
            Value::new_with_arena(&arena, ValueKind::String(s)),
            val_span,
        );
        let d = CssDeclaration::new(name, val, span, true, None).unwrap();
        let node = CssNode::Declaration(d);
        let mut v = EveryCssVisitor;
        let result = node.accept(&mut v).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_every_import_returns_false() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let url = make_val(&arena, "\"foo.css\"");
        let imp = CssImport::new(url, span, None);
        let node = CssNode::Import(imp);
        let mut v = EveryCssVisitor;
        let result = node.accept(&mut v).unwrap();
        assert!(!result);
    }
}
