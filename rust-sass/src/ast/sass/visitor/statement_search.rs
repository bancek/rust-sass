// Copyright 2021 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/statement_search.dart
// go-source: go/value/sass_statement_search.go

use crate::ast::sass::statement::{
    AtRootRule, AtRule, ContentBlock, ContentRule, DebugRule, Declaration, EachRule, ErrorRule,
    ExtendRule, ForRule, ForwardRule, FunctionRule, IfRule, ImportRule, IncludeRule, LoudComment,
    MediaRule, MixinRule, ReturnRule, SilentComment, Statement, StatementVisitor, StyleRule,
    Stylesheet, SupportsRule, UseRule, VariableDeclaration, WarnRule, WhileRule,
};
use crate::common::exception::SassResult;

/// A [`StatementVisitor`] that searches for the first matching node in the
/// AST.
///
/// Each `visit_*` method defaults to recursing into children and returning
/// whether any child matched (`false` for leaves), short-circuiting on the
/// first hit — the `bool` specialization of Dart's generic "first non-`null`
/// result" search. This supports the same [`RecursiveStatementVisitor`]
/// hooks. Set [`StatementSearchVisitor::content_rule_func`] to match
/// `@content` rules; extend by wrapping this visitor and delegating unmatched
/// nodes to it.
pub struct StatementSearchVisitor<'parse> {
    /// Predicate consulted for `@content` rules (which the default traversal
    /// otherwise ignores). Rust's hook replacing Dart subclass overrides.
    // Single-use hook type; a named alias would leak lifetimes into the public
    // API for one field.
    #[allow(clippy::type_complexity)]
    pub content_rule_func: Option<Box<dyn Fn(&ContentRule<'_>) -> SassResult<bool> + 'parse>>,
}

impl<'parse> Default for StatementSearchVisitor<'parse> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'parse> StatementSearchVisitor<'parse> {
    pub fn new() -> Self {
        StatementSearchVisitor {
            content_rule_func: None,
        }
    }

    fn visit_children(&mut self, children: &[Statement<'_>]) -> SassResult<bool> {
        for child in children {
            if child.accept(self)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl<'parse> StatementVisitor<'parse> for StatementSearchVisitor<'_> {
    type Output = bool;

    fn visit_at_root_rule(&mut self, node: &AtRootRule<'parse>) -> SassResult<bool> {
        self.visit_children(&node.children)
    }

    fn visit_at_rule(&mut self, node: &AtRule<'parse>) -> SassResult<bool> {
        if let Some(ref children) = node.children {
            return self.visit_children(children);
        }
        Ok(false)
    }

    fn visit_content_block(&mut self, node: &ContentBlock<'parse>) -> SassResult<bool> {
        self.visit_children(&node.children)
    }

    fn visit_content_rule(&mut self, node: &ContentRule<'parse>) -> SassResult<bool> {
        if let Some(ref f) = self.content_rule_func {
            return f(node);
        }
        Ok(false)
    }

    fn visit_debug_rule(&mut self, _node: &DebugRule<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_declaration(&mut self, node: &Declaration<'parse>) -> SassResult<bool> {
        if let Some(ref children) = node.children {
            return self.visit_children(children);
        }
        Ok(false)
    }

    fn visit_each_rule(&mut self, node: &EachRule<'parse>) -> SassResult<bool> {
        self.visit_children(&node.children)
    }

    fn visit_error_rule(&mut self, _node: &ErrorRule<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_extend_rule(&mut self, _node: &ExtendRule<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_for_rule(&mut self, node: &ForRule<'parse>) -> SassResult<bool> {
        self.visit_children(&node.children)
    }

    fn visit_forward_rule(&mut self, _node: &ForwardRule<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_function_rule(&mut self, node: &FunctionRule<'parse>) -> SassResult<bool> {
        self.visit_children(&node.children)
    }

    fn visit_if_rule(&mut self, node: &IfRule<'parse>) -> SassResult<bool> {
        for clause in &node.clauses {
            for child in &clause.children {
                if child.accept(self)? {
                    return Ok(true);
                }
            }
        }
        if let Some(ref last) = node.last_clause {
            for child in last {
                if child.accept(self)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn visit_import_rule(&mut self, _node: &ImportRule<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_include_rule(&mut self, node: &IncludeRule<'parse>) -> SassResult<bool> {
        if let Some(ref content) = node.content {
            return self.visit_content_block(content);
        }
        Ok(false)
    }

    fn visit_loud_comment(&mut self, _node: &LoudComment<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_media_rule(&mut self, node: &MediaRule<'parse>) -> SassResult<bool> {
        self.visit_children(&node.children)
    }

    fn visit_mixin_rule(&mut self, node: &MixinRule<'parse>) -> SassResult<bool> {
        self.visit_children(&node.children)
    }

    fn visit_return_rule(&mut self, _node: &ReturnRule<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_silent_comment(&mut self, _node: &SilentComment<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_stylesheet(&mut self, node: &Stylesheet<'parse>) -> SassResult<bool> {
        self.visit_children(&node.children)
    }

    fn visit_style_rule(&mut self, node: &StyleRule<'parse>) -> SassResult<bool> {
        self.visit_children(&node.children)
    }

    fn visit_supports_rule(&mut self, node: &SupportsRule<'parse>) -> SassResult<bool> {
        self.visit_children(&node.children)
    }

    fn visit_use_rule(&mut self, _node: &UseRule<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_variable_declaration(
        &mut self,
        _node: &VariableDeclaration<'parse>,
    ) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_warn_rule(&mut self, _node: &WarnRule<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_while_rule(&mut self, node: &WhileRule<'parse>) -> SassResult<bool> {
        self.visit_children(&node.children)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::argument_list::ArgumentList;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_null::NullExpression;
    use crate::ast::sass::parameter_list::ParameterList;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    fn test_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    fn null_expr<'compile, 'parse>(arena: &'compile Bump) -> Expression<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        Expression::Null(NullExpression::new(test_span(arena, "null")))
    }

    #[test]
    fn test_search_returns_false_default() {
        let arena = Bump::new();
        let mut v = StatementSearchVisitor::new();
        let span = test_span(&arena, "@warn true;");
        let wr = WarnRule::new(null_expr(&arena), span);
        let result = Statement::WarnRule(wr).accept(&mut v).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_search_debug_rule_returns_false() {
        let arena = Bump::new();
        let mut v = StatementSearchVisitor::new();
        let span = test_span(&arena, "@debug $x;");
        let dr = DebugRule::new(null_expr(&arena), span);
        let result = Statement::DebugRule(dr).accept(&mut v).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_search_content_rule_default() {
        let arena = Bump::new();
        let mut v = StatementSearchVisitor::new();
        let span = test_span(&arena, "@content;");
        let args = ArgumentList::empty(span);
        let cr = ContentRule::new(args, span);
        let result = Statement::ContentRule(cr).accept(&mut v).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_search_content_rule_func() {
        let arena = Bump::new();
        let mut v = StatementSearchVisitor::new();
        v.content_rule_func = Some(Box::new(|_cr| Ok(true)));

        let span = test_span(&arena, "@content;");
        let args = ArgumentList::empty(span);
        let cr = ContentRule::new(args, span);
        let result = Statement::ContentRule(cr).accept(&mut v).unwrap();
        assert!(result);
    }

    #[test]
    fn test_search_content_rule_func_false() {
        let arena = Bump::new();
        let mut v = StatementSearchVisitor::new();
        v.content_rule_func = Some(Box::new(|_cr| Ok(false)));

        let span = test_span(&arena, "@content;");
        let args = ArgumentList::empty(span);
        let cr = ContentRule::new(args, span);
        let result = Statement::ContentRule(cr).accept(&mut v).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_search_include_content() {
        let arena = Bump::new();
        let mut v = StatementSearchVisitor::new();
        let params = ParameterList::empty(test_span(&arena, ""));
        let cb = ContentBlock::new(params, vec![], test_span(&arena, "{ }"));
        let args = ArgumentList::empty(test_span(&arena, ""));
        let ir = IncludeRule::new(
            "foo".into(),
            args,
            test_span(&arena, "+foo"),
            None,
            Some(cb),
        );
        let result = Statement::IncludeRule(ir).accept(&mut v).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_search_include_no_content() {
        let arena = Bump::new();
        let mut v = StatementSearchVisitor::new();
        let args = ArgumentList::empty(test_span(&arena, ""));
        let ir = IncludeRule::new("foo".into(), args, test_span(&arena, "+foo"), None, None);
        let result = Statement::IncludeRule(ir).accept(&mut v).unwrap();
        assert!(!result);
    }
}
