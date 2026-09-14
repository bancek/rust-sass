// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/recursive_statement.dart
// go-source: go/value/sass_statement_recursive_statement.go

use crate::ast::sass::statement::{
    AtRootRule, AtRule, ContentBlock, ContentRule, DebugRule, Declaration, EachRule, ErrorRule,
    ExtendRule, ForRule, ForwardRule, FunctionRule, IfRule, ImportRule, IncludeRule, LoudComment,
    MediaRule, MixinRule, ReturnRule, SilentComment, Statement, StatementVisitor, StyleRule,
    Stylesheet, SupportsRule, UseRule, VariableDeclaration, WarnRule, WhileRule,
};
use crate::common::exception::SassResult;

/// A visitor that recursively traverses each statement in a Sass AST.
///
/// Leaf statements do nothing; parent statements recurse into their children.
/// Override [`RecursiveStatementVisitor::visit_children`] (visits each child
/// in `children`; called by every parent-statement default) to add behavior
/// for a wide variety of AST nodes.
pub struct RecursiveStatementVisitor;

impl Default for RecursiveStatementVisitor {
    fn default() -> Self {
        Self::new()
    }
}

impl RecursiveStatementVisitor {
    pub fn new() -> Self {
        RecursiveStatementVisitor
    }

    pub(crate) fn visit_children(&mut self, children: &[Statement<'_>]) -> SassResult<()> {
        for child in children {
            child.accept(self)?;
        }
        Ok(())
    }
}

impl<'parse> StatementVisitor<'parse> for RecursiveStatementVisitor {
    type Output = ();

    fn visit_at_root_rule(&mut self, node: &AtRootRule<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_at_rule(&mut self, node: &AtRule<'parse>) -> SassResult<()> {
        if let Some(ref children) = node.children {
            self.visit_children(children)?;
        }
        Ok(())
    }

    fn visit_content_block(&mut self, node: &ContentBlock<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_content_rule(&mut self, _node: &ContentRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_debug_rule(&mut self, _node: &DebugRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_declaration(&mut self, node: &Declaration<'parse>) -> SassResult<()> {
        if let Some(ref children) = node.children {
            self.visit_children(children)?;
        }
        Ok(())
    }

    fn visit_each_rule(&mut self, node: &EachRule<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_error_rule(&mut self, _node: &ErrorRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_extend_rule(&mut self, _node: &ExtendRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_for_rule(&mut self, node: &ForRule<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_forward_rule(&mut self, _node: &ForwardRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_function_rule(&mut self, node: &FunctionRule<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_if_rule(&mut self, node: &IfRule<'parse>) -> SassResult<()> {
        for clause in &node.clauses {
            for child in &clause.children {
                child.accept(self)?;
            }
        }
        if let Some(ref last) = node.last_clause {
            for child in last {
                child.accept(self)?;
            }
        }
        Ok(())
    }

    fn visit_import_rule(&mut self, _node: &ImportRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_include_rule(&mut self, node: &IncludeRule<'parse>) -> SassResult<()> {
        if let Some(ref content) = node.content {
            self.visit_content_block(content)?;
        }
        Ok(())
    }

    fn visit_loud_comment(&mut self, _node: &LoudComment<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_media_rule(&mut self, node: &MediaRule<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_mixin_rule(&mut self, node: &MixinRule<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_return_rule(&mut self, _node: &ReturnRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_silent_comment(&mut self, _node: &SilentComment<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_stylesheet(&mut self, node: &Stylesheet<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_style_rule(&mut self, node: &StyleRule<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_supports_rule(&mut self, node: &SupportsRule<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_use_rule(&mut self, _node: &UseRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_variable_declaration(
        &mut self,
        _node: &VariableDeclaration<'parse>,
    ) -> SassResult<()> {
        Ok(())
    }

    fn visit_warn_rule(&mut self, _node: &WarnRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_while_rule(&mut self, node: &WhileRule<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::argument_list::ArgumentList;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_boolean::BooleanExpression;
    use crate::ast::sass::expression_list::ListExpression;
    use crate::ast::sass::expression_null::NullExpression;
    use crate::ast::sass::expression_number::NumberExpression;
    use crate::ast::sass::expression_string::StringExpression;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::ast::sass::parameter_list::ParameterList;
    use crate::ast::sass::statement::IfClause;
    use crate::ast::sass::supports_condition::{SupportsCondition, SupportsDeclaration};
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::value::ListSeparator;
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

    fn bool_expr<'compile, 'parse>(arena: &'compile Bump, val: bool) -> Expression<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        Expression::Boolean(BooleanExpression::new(val, test_span(arena, "true")))
    }

    fn var_decl<'compile, 'parse>(arena: &'compile Bump, name: &str) -> Statement<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        Statement::VariableDeclaration(
            VariableDeclaration::new(
                name.to_string(),
                null_expr(arena),
                test_span(arena, name),
                None,
                false,
                false,
                None,
            )
            .unwrap(),
        )
    }

    #[test]
    fn test_visit_children() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let child = var_decl(&arena, "x");
        let children = vec![child];
        v.visit_children(&children).unwrap();
    }

    #[test]
    fn test_visit_children_empty() {
        let mut v = RecursiveStatementVisitor::new();
        v.visit_children(&[]).unwrap();
    }

    #[test]
    fn test_visit_stylesheet() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let child = var_decl(&arena, "x");
        let ss = Stylesheet::new(vec![child], test_span(&arena, "$x: null;"));
        Statement::Stylesheet(ss).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_at_rule_nil_children() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let name = Interpolation::plain("foo".into(), test_span(&arena, "foo"));
        let ar = AtRule::new(name, test_span(&arena, "@foo;"), None, None);
        Statement::AtRule(ar).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_at_rule_empty_children() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let name = Interpolation::plain("foo".into(), test_span(&arena, "foo"));
        let ar = AtRule::new(name, test_span(&arena, "@foo { }"), None, Some(vec![]));
        Statement::AtRule(ar).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_at_rule_with_children() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let name = Interpolation::plain("foo".into(), test_span(&arena, "foo"));
        let child = var_decl(&arena, "x");
        let ar = AtRule::new(name, test_span(&arena, "@foo { }"), None, Some(vec![child]));
        Statement::AtRule(ar).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_declaration_nil_children() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let name = Interpolation::plain("color".into(), test_span(&arena, "color"));
        let val = null_expr(&arena);
        let d = Declaration::new(name, val, test_span(&arena, "color: null;"));
        Statement::Declaration(d).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_declaration_with_children() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let name = Interpolation::plain("color".into(), test_span(&arena, "color"));
        let _val = null_expr(&arena);
        let child = var_decl(&arena, "x");
        let d = Declaration::nested(
            name,
            vec![child],
            test_span(&arena, "color: null { }"),
            None,
        );
        Statement::Declaration(d).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_if_rule() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let child = var_decl(&arena, "x");
        let clause = IfClause {
            expression: bool_expr(&arena, true),
            children: vec![child],
        };
        let if_rule = IfRule::new(vec![clause], test_span(&arena, "@if true { }"), None);
        Statement::IfRule(if_rule).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_if_rule_with_else() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let child = var_decl(&arena, "x");
        let clause = IfClause {
            expression: bool_expr(&arena, true),
            children: vec![child.clone()],
        };
        let if_rule = IfRule::new(
            vec![clause],
            test_span(&arena, "@if true { } @else { }"),
            Some(vec![child]),
        );
        Statement::IfRule(if_rule).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_include_content() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
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
        Statement::IncludeRule(ir).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_include_no_content() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let args = ArgumentList::empty(test_span(&arena, ""));
        let ir = IncludeRule::new("foo".into(), args, test_span(&arena, "+foo"), None, None);
        Statement::IncludeRule(ir).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_function_rule() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let params = ParameterList::empty(test_span(&arena, ""));
        let child = var_decl(&arena, "x");
        let fr = FunctionRule::new(
            "f".into(),
            params,
            vec![child],
            test_span(&arena, "@function f() { }"),
            None,
        );
        Statement::FunctionRule(fr).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_mixin_rule() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let params = ParameterList::empty(test_span(&arena, ""));
        let child = var_decl(&arena, "x");
        let mr = MixinRule::new(
            "m".into(),
            params,
            vec![child],
            test_span(&arena, "@mixin m() { }"),
            None,
        );
        Statement::MixinRule(mr).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_leaf_nodes() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();

        let base_span = test_span(&arena, "test");
        let null = null_expr(&arena);
        let plain = Interpolation::plain("a".into(), test_span(&arena, "a"));

        let leaves: Vec<Statement<'_>> = vec![
            Statement::ContentRule(ContentRule::new(ArgumentList::empty(base_span), base_span)),
            Statement::DebugRule(DebugRule::new(null.clone(), base_span)),
            Statement::ErrorRule(ErrorRule::new(null.clone(), base_span)),
            Statement::WarnRule(WarnRule::new(null.clone(), base_span)),
            Statement::ReturnRule(ReturnRule::new(null.clone(), base_span)),
            Statement::ExtendRule(ExtendRule::new(plain, base_span, false)),
            Statement::ImportRule(ImportRule::new(vec![], base_span)),
            Statement::LoudComment(LoudComment::new(Interpolation::plain(
                "/* */".into(),
                base_span,
            ))),
            Statement::SilentComment(SilentComment::new("//".into(), base_span)),
            Statement::VariableDeclaration(
                VariableDeclaration::new("x".into(), null, base_span, None, false, false, None)
                    .unwrap(),
            ),
        ];

        for leaf in &leaves {
            leaf.accept(&mut v).unwrap();
        }
    }

    #[test]
    fn test_visit_style_rule() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let sel = Interpolation::plain(".a".into(), test_span(&arena, ".a"));
        let child = var_decl(&arena, "x");
        let sr = StyleRule::new(sel, vec![child], test_span(&arena, ".a { }"));
        Statement::StyleRule(sr).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_media_rule() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let query = Interpolation::plain("screen".into(), test_span(&arena, "screen"));
        let mr = MediaRule::new(query, vec![], test_span(&arena, "@media screen { }"));
        Statement::MediaRule(mr).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_supports_rule() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let decl = SupportsCondition::Declaration(SupportsDeclaration::new(
            Expression::String(StringExpression::plain("a", test_span(&arena, "a"), false)),
            Expression::String(StringExpression::plain("b", test_span(&arena, "b"), false)),
            test_span(&arena, "(a: b)"),
        ));
        let sr = SupportsRule::new(decl, vec![], test_span(&arena, "@supports (a: b) { }"));
        Statement::SupportsRule(sr).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_each_rule() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let list = Expression::List(ListExpression::new(
            vec![
                Expression::String(StringExpression::plain("a", test_span(&arena, "a"), true)),
                Expression::String(StringExpression::plain("b", test_span(&arena, "b"), true)),
            ],
            ListSeparator::Comma,
            test_span(&arena, "(a, b)"),
            true,
        ));
        let child = var_decl(&arena, "x");
        let er = EachRule::new(
            vec!["x".into()],
            list,
            vec![child],
            test_span(&arena, "@each $x in (a, b) { }"),
        );
        Statement::EachRule(er).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_for_rule() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let from = Expression::Number(NumberExpression::new(1.0, test_span(&arena, "1"), None));
        let to = Expression::Number(NumberExpression::new(3.0, test_span(&arena, "3"), None));
        let fr = ForRule::new(
            "i".into(),
            from,
            to,
            vec![],
            test_span(&arena, "@for $i from 1 through 3 { }"),
            true,
        );
        Statement::ForRule(fr).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_while_rule() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let cond = bool_expr(&arena, true);
        let wr = WhileRule::new(cond, vec![], test_span(&arena, "@while true { }"));
        Statement::WhileRule(wr).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_at_root_rule() {
        let arena = Bump::new();
        let mut v = RecursiveStatementVisitor::new();
        let child = var_decl(&arena, "x");
        let ar = AtRootRule::new(vec![child], test_span(&arena, "@at-root { }"), None);
        Statement::AtRootRule(ar).accept(&mut v).unwrap();
    }
}
