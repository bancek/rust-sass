// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/recursive_ast.dart
// go-source: go/value/sass_recursive_ast.go

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression::ExpressionVisitor;
use crate::ast::sass::expression::IfConditionExpressionVisitor;
use crate::ast::sass::expression_binary_operation::BinaryOperationExpression;
use crate::ast::sass::expression_boolean::BooleanExpression;
use crate::ast::sass::expression_color::ColorExpression;
use crate::ast::sass::expression_function::FunctionExpression;
use crate::ast::sass::expression_if::IfExpression;
use crate::ast::sass::expression_if::{
    IfConditionFunction, IfConditionNegation, IfConditionOperation, IfConditionParenthesized,
    IfConditionRaw, IfConditionSass,
};
use crate::ast::sass::expression_interpolated_function::InterpolatedFunctionExpression;
use crate::ast::sass::expression_legacy_if::LegacyIfExpression;
use crate::ast::sass::expression_list::ListExpression;
use crate::ast::sass::expression_map::MapExpression;
use crate::ast::sass::expression_null::NullExpression;
use crate::ast::sass::expression_number::NumberExpression;
use crate::ast::sass::expression_parenthesized::ParenthesizedExpression;
use crate::ast::sass::expression_selector::SelectorExpression;
use crate::ast::sass::expression_string::StringExpression;
use crate::ast::sass::expression_supports::SupportsExpression;
use crate::ast::sass::expression_unary_operation::UnaryOperationExpression;
use crate::ast::sass::expression_value::ValueExpression;
use crate::ast::sass::expression_variable::VariableExpression;
use crate::ast::sass::import::Import;
use crate::ast::sass::interpolated_selector::{
    InterpolatedAttributeSelector, InterpolatedClassSelector, InterpolatedComplexSelector,
    InterpolatedCompoundSelector, InterpolatedIDSelector, InterpolatedParentSelector,
    InterpolatedPlaceholderSelector, InterpolatedPseudoSelector, InterpolatedQualifiedName,
    InterpolatedSelectorList, InterpolatedSelectorVisitor, InterpolatedTypeSelector,
    InterpolatedUniversalSelector,
};
use crate::ast::sass::interpolation::{Interpolation, InterpolationPart};
use crate::ast::sass::parameter_list::ParameterList;
use crate::ast::sass::statement::{
    AtRootRule, AtRule, ContentBlock, ContentRule, DebugRule, Declaration, EachRule, ErrorRule,
    ExtendRule, ForRule, ForwardRule, FunctionRule, IfRule, ImportRule, IncludeRule, LoudComment,
    MediaRule, MixinRule, ReturnRule, SilentComment, Statement, StatementVisitor, StyleRule,
    Stylesheet, SupportsRule, UseRule, VariableDeclaration, WarnRule, WhileRule,
};
use crate::ast::sass::supports_condition::SupportsCondition;
use crate::common::exception::SassResult;

/// A visitor that recursively traverses each statement and expression in a
/// Sass AST.
///
/// This extends [`RecursiveStatementVisitor`] to traverse each expression in
/// addition to each statement. It adds even more helpers:
///
/// * `visit_argument_list` — visits each expression in an invocation; the
///   default visit methods call this for any argument invocation in a
///   statement.
/// * `visit_supports_condition` — visits each expression in a condition; the
///   default visit methods call this for any [`SupportsCondition`] they
///   encounter.
/// * `visit_interpolation` — visits each expression in an interpolation; the
///   default visit methods call this for any interpolation in a statement.
/// * `visit_qualified_name` — visits each interpolation in a qualified name;
///   the default visit methods call this for any qualified names in a
///   selector.
pub struct RecursiveAstVisitor;

impl Default for RecursiveAstVisitor {
    fn default() -> Self {
        Self::new()
    }
}

impl RecursiveAstVisitor {
    pub fn new() -> Self {
        RecursiveAstVisitor
    }

    fn visit_children(&mut self, children: &[Statement<'_>]) -> SassResult<()> {
        for child in children {
            child.accept(self)?;
        }
        Ok(())
    }

    fn visit_expression(&mut self, expr: &Expression<'_>) -> SassResult<()> {
        expr.accept(self)
    }

    fn visit_parameter_defaults(&mut self, parameters: &ParameterList<'_>) -> SassResult<()> {
        for param in &parameters.parameters {
            if let Some(ref default) = param.default_value {
                self.visit_expression(default)?;
            }
        }
        Ok(())
    }

    fn visit_argument_list(&mut self, args: &ArgumentList<'_>) -> SassResult<()> {
        for expr in &args.positional {
            self.visit_expression(expr)?;
        }
        for expr in args.named.values() {
            self.visit_expression(expr)?;
        }
        if let Some(ref rest) = args.rest {
            self.visit_expression(rest)?;
        }
        if let Some(ref keyword_rest) = args.keyword_rest {
            self.visit_expression(keyword_rest)?;
        }
        Ok(())
    }

    fn visit_supports_condition(&mut self, condition: &SupportsCondition<'_>) -> SassResult<()> {
        match condition {
            SupportsCondition::Operation(op) => {
                self.visit_supports_condition(&op.left)?;
                self.visit_supports_condition(&op.right)?;
            }
            SupportsCondition::Negation(neg) => {
                self.visit_supports_condition(&neg.condition)?;
            }
            SupportsCondition::Interpolation(interp) => {
                self.visit_expression(&interp.expression)?;
            }
            SupportsCondition::Declaration(decl) => {
                self.visit_expression(&decl.name)?;
                self.visit_expression(&decl.value)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn visit_interpolation(&mut self, interp: &Interpolation<'_>) -> SassResult<()> {
        for part in &interp.contents {
            if let InterpolationPart::Expression(expr) = part {
                self.visit_expression(expr)?;
            }
        }
        Ok(())
    }

    fn visit_qualified_name(&mut self, name: &InterpolatedQualifiedName<'_>) -> SassResult<()> {
        if let Some(ref ns) = name.namespace {
            self.visit_interpolation(ns)?;
        }
        self.visit_interpolation(&name.name)
    }
}

// StatementVisitor
impl<'parse> StatementVisitor<'parse> for RecursiveAstVisitor {
    type Output = ();

    fn visit_at_root_rule(&mut self, node: &AtRootRule<'parse>) -> SassResult<()> {
        if let Some(ref query) = node.query {
            self.visit_interpolation(query)?;
        }
        self.visit_children(&node.children)
    }

    fn visit_at_rule(&mut self, node: &AtRule<'parse>) -> SassResult<()> {
        self.visit_interpolation(&node.name)?;
        if let Some(ref value) = node.value {
            self.visit_interpolation(value)?;
        }
        if let Some(ref children) = node.children {
            self.visit_children(children)?;
        }
        Ok(())
    }

    fn visit_content_block(&mut self, node: &ContentBlock<'parse>) -> SassResult<()> {
        self.visit_parameter_defaults(&node.parameters)?;
        self.visit_children(&node.children)
    }

    fn visit_content_rule(&mut self, node: &ContentRule<'parse>) -> SassResult<()> {
        self.visit_argument_list(&node.arguments)
    }

    fn visit_debug_rule(&mut self, node: &DebugRule<'parse>) -> SassResult<()> {
        self.visit_expression(&node.expression)
    }

    fn visit_declaration(&mut self, node: &Declaration<'parse>) -> SassResult<()> {
        self.visit_interpolation(&node.name)?;
        if let Some(ref value) = node.value {
            self.visit_expression(value)?;
        }
        if let Some(ref children) = node.children {
            self.visit_children(children)?;
        }
        Ok(())
    }

    fn visit_each_rule(&mut self, node: &EachRule<'parse>) -> SassResult<()> {
        self.visit_expression(&node.list)?;
        self.visit_children(&node.children)
    }

    fn visit_error_rule(&mut self, node: &ErrorRule<'parse>) -> SassResult<()> {
        self.visit_expression(&node.expression)
    }

    fn visit_extend_rule(&mut self, node: &ExtendRule<'parse>) -> SassResult<()> {
        self.visit_interpolation(&node.selector)
    }

    fn visit_for_rule(&mut self, node: &ForRule<'parse>) -> SassResult<()> {
        self.visit_expression(&node.from)?;
        self.visit_expression(&node.to)?;
        self.visit_children(&node.children)
    }

    fn visit_forward_rule(&mut self, node: &ForwardRule<'parse>) -> SassResult<()> {
        for var in &node.configuration {
            self.visit_expression(&var.expression)?;
        }
        Ok(())
    }

    fn visit_function_rule(&mut self, node: &FunctionRule<'parse>) -> SassResult<()> {
        self.visit_parameter_defaults(&node.parameters)?;
        self.visit_children(&node.children)
    }

    fn visit_if_rule(&mut self, node: &IfRule<'parse>) -> SassResult<()> {
        for clause in &node.clauses {
            self.visit_expression(&clause.expression)?;
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

    fn visit_import_rule(&mut self, node: &ImportRule<'parse>) -> SassResult<()> {
        for imp in &node.imports {
            if let Import::Static(s) = imp {
                self.visit_interpolation(&s.url)?;
                if let Some(ref modifiers) = s.modifiers {
                    self.visit_interpolation(modifiers)?;
                }
            }
        }
        Ok(())
    }

    fn visit_include_rule(&mut self, node: &IncludeRule<'parse>) -> SassResult<()> {
        self.visit_argument_list(&node.arguments)?;
        if let Some(ref content) = node.content {
            self.visit_content_block(content)?;
        }
        Ok(())
    }

    fn visit_loud_comment(&mut self, node: &LoudComment<'parse>) -> SassResult<()> {
        self.visit_interpolation(&node.text)
    }

    fn visit_media_rule(&mut self, node: &MediaRule<'parse>) -> SassResult<()> {
        self.visit_interpolation(&node.query)?;
        self.visit_children(&node.children)
    }

    fn visit_mixin_rule(&mut self, node: &MixinRule<'parse>) -> SassResult<()> {
        self.visit_parameter_defaults(&node.parameters)?;
        self.visit_children(&node.children)
    }

    fn visit_return_rule(&mut self, node: &ReturnRule<'parse>) -> SassResult<()> {
        self.visit_expression(&node.expression)
    }

    fn visit_silent_comment(&mut self, _node: &SilentComment<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_stylesheet(&mut self, _node: &Stylesheet<'parse>) -> SassResult<()> {
        self.visit_children(&_node.children)
    }

    fn visit_style_rule(&mut self, node: &StyleRule<'parse>) -> SassResult<()> {
        if let Some(ref selector) = node.selector {
            self.visit_interpolation(selector)?;
        }
        self.visit_children(&node.children)
    }

    fn visit_supports_rule(&mut self, node: &SupportsRule<'parse>) -> SassResult<()> {
        self.visit_supports_condition(&node.condition)?;
        self.visit_children(&node.children)
    }

    fn visit_use_rule(&mut self, node: &UseRule<'parse>) -> SassResult<()> {
        for var in &node.configuration {
            self.visit_expression(&var.expression)?;
        }
        Ok(())
    }

    fn visit_variable_declaration(&mut self, node: &VariableDeclaration<'parse>) -> SassResult<()> {
        self.visit_expression(&node.expression)
    }

    fn visit_warn_rule(&mut self, node: &WarnRule<'parse>) -> SassResult<()> {
        self.visit_expression(&node.expression)
    }

    fn visit_while_rule(&mut self, node: &WhileRule<'parse>) -> SassResult<()> {
        self.visit_expression(&node.condition)?;
        self.visit_children(&node.children)
    }
}

// ExpressionVisitor
impl<'parse> ExpressionVisitor<'parse> for RecursiveAstVisitor {
    type Output = ();

    fn visit_binary_operation(
        &mut self,
        node: &BinaryOperationExpression<'parse>,
    ) -> SassResult<()> {
        node.left.accept(self)?;
        node.right.accept(self)
    }

    fn visit_boolean(&mut self, _node: &BooleanExpression<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_color(&mut self, _node: &ColorExpression<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_function(&mut self, node: &FunctionExpression<'parse>) -> SassResult<()> {
        self.visit_argument_list(&node.arguments)
    }

    fn visit_if(&mut self, node: &IfExpression<'parse>) -> SassResult<()> {
        for branch in &node.branches {
            if let Some(ref condition) = branch.condition {
                condition.accept(self)?;
            }
            branch.expression.accept(self)?;
        }
        Ok(())
    }

    fn visit_interpolated_function(
        &mut self,
        node: &InterpolatedFunctionExpression<'parse>,
    ) -> SassResult<()> {
        self.visit_interpolation(&node.name)?;
        self.visit_argument_list(&node.arguments)
    }

    fn visit_legacy_if(&mut self, node: &LegacyIfExpression<'parse>) -> SassResult<()> {
        self.visit_argument_list(&node.arguments)
    }

    fn visit_list(&mut self, node: &ListExpression<'parse>) -> SassResult<()> {
        for item in &node.contents {
            item.accept(self)?;
        }
        Ok(())
    }

    fn visit_map(&mut self, node: &MapExpression<'parse>) -> SassResult<()> {
        for pair in &node.pairs {
            pair.0.accept(self)?;
            pair.1.accept(self)?;
        }
        Ok(())
    }

    fn visit_null(&mut self, _node: &NullExpression<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_number(&mut self, _node: &NumberExpression<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_parenthesized(&mut self, node: &ParenthesizedExpression<'parse>) -> SassResult<()> {
        node.expression.accept(self)
    }

    fn visit_selector(&mut self, _node: &SelectorExpression<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_string(&mut self, node: &StringExpression<'parse>) -> SassResult<()> {
        self.visit_interpolation(&node.text)
    }

    fn visit_supports(&mut self, node: &SupportsExpression) -> SassResult<()> {
        self.visit_supports_condition(&node.condition)
    }

    fn visit_unary_operation(&mut self, node: &UnaryOperationExpression<'parse>) -> SassResult<()> {
        node.operand.accept(self)
    }

    fn visit_value(&mut self, _node: &ValueExpression<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_variable(&mut self, _node: &VariableExpression<'parse>) -> SassResult<()> {
        Ok(())
    }
}

// IfConditionExpressionVisitor
impl<'parse> IfConditionExpressionVisitor<'parse> for RecursiveAstVisitor {
    type Output = ();

    fn visit_parenthesized(&mut self, node: &IfConditionParenthesized<'parse>) -> SassResult<()> {
        node.expression.accept(self)
    }

    fn visit_negation(&mut self, node: &IfConditionNegation<'parse>) -> SassResult<()> {
        node.expression.accept(self)
    }

    fn visit_operation(&mut self, node: &IfConditionOperation<'parse>) -> SassResult<()> {
        for expr in &node.expressions {
            expr.accept(self)?;
        }
        Ok(())
    }

    fn visit_function(&mut self, node: &IfConditionFunction<'parse>) -> SassResult<()> {
        self.visit_interpolation(&node.name)?;
        self.visit_interpolation(&node.arguments)
    }

    fn visit_sass(&mut self, node: &IfConditionSass<'parse>) -> SassResult<()> {
        node.expression.accept(self)
    }

    fn visit_raw(&mut self, node: &IfConditionRaw<'parse>) -> SassResult<()> {
        self.visit_interpolation(&node.text)
    }
}

// InterpolatedSelectorVisitor
impl<'parse> InterpolatedSelectorVisitor<'parse> for RecursiveAstVisitor {
    type Output = ();

    fn visit_attribute_selector(
        &mut self,
        node: &InterpolatedAttributeSelector<'parse>,
    ) -> SassResult<()> {
        self.visit_qualified_name(&node.name)?;
        if let Some(ref value) = node.value {
            self.visit_interpolation(value)?;
        }
        if let Some(ref modifier) = node.modifier {
            self.visit_interpolation(modifier)?;
        }
        Ok(())
    }

    fn visit_class_selector(&mut self, node: &InterpolatedClassSelector<'parse>) -> SassResult<()> {
        self.visit_interpolation(&node.name)
    }

    fn visit_complex_selector(
        &mut self,
        node: &InterpolatedComplexSelector<'parse>,
    ) -> SassResult<()> {
        for component in &node.components {
            self.visit_compound_selector(&component.selector)?;
        }
        Ok(())
    }

    fn visit_compound_selector(
        &mut self,
        node: &InterpolatedCompoundSelector<'parse>,
    ) -> SassResult<()> {
        for simple in &node.components {
            simple.accept(self)?;
        }
        Ok(())
    }

    fn visit_id_selector(&mut self, node: &InterpolatedIDSelector<'parse>) -> SassResult<()> {
        self.visit_interpolation(&node.name)
    }

    fn visit_parent_selector(
        &mut self,
        node: &InterpolatedParentSelector<'parse>,
    ) -> SassResult<()> {
        if let Some(ref suffix) = node.suffix {
            self.visit_interpolation(suffix)?;
        }
        Ok(())
    }

    fn visit_placeholder_selector(
        &mut self,
        node: &InterpolatedPlaceholderSelector<'parse>,
    ) -> SassResult<()> {
        self.visit_interpolation(&node.name)
    }

    fn visit_pseudo_selector(
        &mut self,
        node: &InterpolatedPseudoSelector<'parse>,
    ) -> SassResult<()> {
        self.visit_interpolation(&node.name)?;
        if let Some(ref argument) = node.argument {
            self.visit_interpolation(argument)?;
        }
        if let Some(ref selector) = node.selector {
            self.visit_selector_list(selector)?;
        }
        Ok(())
    }

    fn visit_selector_list(&mut self, node: &InterpolatedSelectorList<'parse>) -> SassResult<()> {
        for component in &node.components {
            self.visit_complex_selector(component)?;
        }
        Ok(())
    }

    fn visit_type_selector(&mut self, node: &InterpolatedTypeSelector<'parse>) -> SassResult<()> {
        self.visit_qualified_name(&node.name)
    }

    fn visit_universal_selector(
        &mut self,
        node: &InterpolatedUniversalSelector<'parse>,
    ) -> SassResult<()> {
        if let Some(ref namespace) = node.namespace {
            self.visit_interpolation(namespace)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::binary_operator::BinaryOperator;
    use crate::ast::sass::configured_variable::ConfiguredVariable;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_boolean::BooleanExpression;
    use crate::ast::sass::expression_map::MapExpression;
    use crate::ast::sass::expression_null::NullExpression;
    use crate::ast::sass::expression_number::NumberExpression;
    use crate::ast::sass::expression_parenthesized::ParenthesizedExpression;
    use crate::ast::sass::expression_string::StringExpression;
    use crate::ast::sass::expression_supports::SupportsExpression;
    use crate::ast::sass::expression_unary_operation::UnaryOperationExpression;
    use crate::ast::sass::import::Import;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::ast::sass::parameter::Parameter;
    use crate::ast::sass::parameter_list::ParameterList;
    use crate::ast::sass::static_import::StaticImport;
    use crate::ast::sass::supports_condition::{
        SupportsCondition, SupportsDeclaration, SupportsNegation,
    };
    use crate::ast::sass::unary_operator::UnaryOperator;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::url::SassUrl;
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

    #[test]
    fn test_visit_debug_rule_expression() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let span = test_span(&arena, "@debug $x;");
        let expr = null_expr(&arena);
        let dr = DebugRule::new(expr, span);
        Statement::DebugRule(dr).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_variable_declaration_expression() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let span = test_span(&arena, "$x: 1 + 2;");
        let left = Expression::Number(NumberExpression::new(1.0, test_span(&arena, "1"), None));
        let right = Expression::Number(NumberExpression::new(2.0, test_span(&arena, "2"), None));
        let bin = Expression::BinaryOperation(BinaryOperationExpression::new(
            BinaryOperator::Plus,
            left,
            right,
        ));
        let vd = VariableDeclaration::new("x".into(), bin, span, None, false, false, None).unwrap();
        Statement::VariableDeclaration(vd).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_at_rule_interpolation() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let span = test_span(&arena, "@foo bar { }");
        let name = Interpolation::plain("foo".into(), test_span(&arena, "foo"));
        let value = Interpolation::plain("bar".into(), test_span(&arena, "bar"));
        let ar = AtRule::new(name, span, Some(value), None);
        Statement::AtRule(ar).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_declaration_interpolation() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let span = test_span(&arena, "color: red;");
        let name = Interpolation::plain("color".into(), test_span(&arena, "color"));
        let val = Expression::String(StringExpression::plain(
            "red",
            test_span(&arena, "red"),
            false,
        ));
        let d = Declaration::new(name, val, span);
        Statement::Declaration(d).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_content_rule_argument_list() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let span = test_span(&arena, "@content($a);");
        let positional = vec![null_expr(&arena)];
        let args = ArgumentList::new(
            positional,
            indexmap::IndexMap::new(),
            indexmap::IndexMap::new(),
            span,
            None,
            None,
        );
        let cr = ContentRule::new(args, span);
        Statement::ContentRule(cr).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_include_argument_list() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let span = test_span(&arena, "+foo(1)");
        let positional = vec![Expression::Number(NumberExpression::new(
            1.0,
            test_span(&arena, "1"),
            None,
        ))];
        let args = ArgumentList::new(
            positional,
            indexmap::IndexMap::new(),
            indexmap::IndexMap::new(),
            span,
            None,
            None,
        );
        let ir = IncludeRule::new("foo".into(), args, span, None, None);
        Statement::IncludeRule(ir).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_binary_operation() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let left = Expression::Number(NumberExpression::new(1.0, test_span(&arena, "1"), None));
        let right = Expression::Number(NumberExpression::new(2.0, test_span(&arena, "2"), None));
        let bin = Expression::BinaryOperation(BinaryOperationExpression::new(
            BinaryOperator::Plus,
            left,
            right,
        ));
        bin.accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_list_expression() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let items = vec![
            Expression::String(StringExpression::plain("a", test_span(&arena, "a"), true)),
            Expression::String(StringExpression::plain("b", test_span(&arena, "b"), true)),
        ];
        let le = ListExpression::new(
            items,
            ListSeparator::Comma,
            test_span(&arena, "(a, b)"),
            true,
        );
        Expression::List(le).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_map_expression() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let pairs = vec![(
            Expression::String(StringExpression::plain("a", test_span(&arena, "a"), false)),
            Expression::Number(NumberExpression::new(1.0, test_span(&arena, "1"), None)),
        )];
        let me = MapExpression::new(pairs, test_span(&arena, "(a: 1)"));
        Expression::Map(me).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_parenthesized_expression() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let inner = Expression::Number(NumberExpression::new(1.0, test_span(&arena, "1"), None));
        let pe = ParenthesizedExpression::new(inner, test_span(&arena, "(1)"));
        Expression::Parenthesized(pe).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_unary_operation() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let operand = Expression::Number(NumberExpression::new(1.0, test_span(&arena, "1"), None));
        let ue =
            UnaryOperationExpression::new(UnaryOperator::Minus, operand, test_span(&arena, "-1"));
        Expression::UnaryOperation(ue).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_function_expression() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let args = ArgumentList::empty(test_span(&arena, ""));
        let fe = FunctionExpression::new("foo".into(), args, test_span(&arena, "foo()"), None);
        Expression::Function(fe).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_supports_condition_negation() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let decl = SupportsCondition::Declaration(SupportsDeclaration::new(
            Expression::String(StringExpression::plain("a", test_span(&arena, "a"), false)),
            Expression::String(StringExpression::plain("b", test_span(&arena, "b"), false)),
            test_span(&arena, "(a: b)"),
        ));
        let neg = SupportsCondition::Negation(SupportsNegation::new(
            decl,
            test_span(&arena, "not (a: b)"),
        ));
        let se = Expression::Supports(SupportsExpression::new(neg));
        se.accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_forward_rule_configuration() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let span = test_span(&arena, "@forward 'foo' with ($x: 1)");
        let u = SassUrl::parse("sass:foo").unwrap();
        let expr = Expression::Number(NumberExpression::new(1.0, test_span(&arena, "1"), None));
        let cv = ConfiguredVariable::new("x".into(), expr, span, false);
        let fr = ForwardRule::new(u, span, None, vec![cv]);
        Statement::ForwardRule(fr).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_use_rule_configuration() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let span = test_span(&arena, "@use 'foo' with ($x: 1)");
        let u = SassUrl::parse("sass:foo").unwrap();
        let expr = Expression::Number(NumberExpression::new(1.0, test_span(&arena, "1"), None));
        let cv = ConfiguredVariable::new("x".into(), expr, span, false);
        let ur = UseRule::new(u, None, span, vec![cv]).unwrap();
        Statement::UseRule(ur).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_import_rule() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let span = test_span(&arena, "@import 'foo';");
        let url = Interpolation::plain("foo".into(), test_span(&arena, "foo"));
        let si = Import::Static(StaticImport::new(url, test_span(&arena, "'foo'"), None));
        let ir = ImportRule::new(vec![si], span);
        Statement::ImportRule(ir).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_function_rule_parameters() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let span = test_span(&arena, "@function f($x: 1) { }");
        let default = Some(Expression::Number(NumberExpression::new(
            1.0,
            test_span(&arena, "1"),
            None,
        )));
        let param = Parameter::new("x".into(), span, default);
        let pl = ParameterList::new(vec![param], span, None);
        let fr = FunctionRule::new("f".into(), pl, vec![], span, None);
        Statement::FunctionRule(fr).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_style_rule_selector() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let sel = Interpolation::plain(".a".into(), test_span(&arena, ".a"));
        let sr = StyleRule::new(sel, vec![], test_span(&arena, ".a { }"));
        Statement::StyleRule(sr).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_loud_comment_interpolation() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let text = Interpolation::plain("/* */".into(), test_span(&arena, "/* */"));
        let lc = LoudComment::new(text);
        Statement::LoudComment(lc).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_media_rule_interpolation() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let query = Interpolation::plain("screen".into(), test_span(&arena, "screen"));
        let mr = MediaRule::new(query, vec![], test_span(&arena, "@media screen { }"));
        Statement::MediaRule(mr).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_string_expression_interpolation() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let text = Interpolation::plain("hello".into(), test_span(&arena, "hello"));
        let se = StringExpression::new(text, true);
        Expression::String(se).accept(&mut v).unwrap();
    }

    #[test]
    fn test_visit_leaf_expressions() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let span = test_span(&arena, "leaf");

        let leaves: Vec<Expression<'_>> = vec![
            Expression::Boolean(BooleanExpression::new(true, span)),
            Expression::Null(NullExpression::new(span)),
            Expression::Number(NumberExpression::new(1.0, span, None)),
        ];
        for expr in &leaves {
            expr.accept(&mut v).unwrap();
        }
    }

    #[test]
    fn test_visit_warn_error_return() {
        let arena = Bump::new();
        let mut v = RecursiveAstVisitor::new();
        let span = test_span(&arena, "test");
        let expr = Expression::Number(NumberExpression::new(1.0, test_span(&arena, "1"), None));

        let wr = WarnRule::new(expr.clone(), span);
        Statement::WarnRule(wr).accept(&mut v).unwrap();

        let er = ErrorRule::new(expr.clone(), span);
        Statement::ErrorRule(er).accept(&mut v).unwrap();

        let rr = ReturnRule::new(expr, span);
        Statement::ReturnRule(rr).accept(&mut v).unwrap();
    }
}
