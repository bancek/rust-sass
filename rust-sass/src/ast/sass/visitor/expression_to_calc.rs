// Copyright 2023 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/expression_to_calc.dart
// go-source: go/value/sass_expression_to_calc.go

use crate::ast::sass::expression_if::IfExpression;
use indexmap::IndexMap;

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::binary_operator::BinaryOperator;
use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression::ExpressionVisitor;
use crate::ast::sass::expression_binary_operation::BinaryOperationExpression;
use crate::ast::sass::expression_boolean::BooleanExpression;
use crate::ast::sass::expression_color::ColorExpression;
use crate::ast::sass::expression_function::FunctionExpression;
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
use crate::ast::sass::unary_operator::UnaryOperator;
use crate::ast::sass::visitor::replace_expression::ReplaceExpressionVisitor;
use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;

/// Converts `expression` to an equivalent `calc()`.
///
/// This assumes that `expression` already returns a number. It's intended for
/// use in end-user messaging, and may not produce directly evaluable
/// expressions.
///
/// Matches Dart: expressionToCalc
pub fn expression_to_calc<'parse>(
    expression: &Expression<'parse>,
) -> SassResult<FunctionExpression<'parse>> {
    let mut visitor = MakeExpressionCalculationSafe::new();
    let expr = expression.accept(&mut visitor)?;
    Ok(FunctionExpression::new(
        "calc".into(),
        ArgumentList::new(
            vec![expr],
            IndexMap::new(),
            IndexMap::new(),
            expression.span()?,
            None,
            None,
        ),
        expression.span()?,
        None,
    ))
}

/// A visitor that replaces constructs that can't be used in a calculation with
/// those that can.
///
/// Extends the replace-each-expression traversal: binary `%` (which `calc()`
/// has no modulo operator for, and for which the `mod()` calc function has no
/// browser support yet, so it is wrapped in the `math.max` Sass function as a
/// workaround), unary `+`/`-` (which `calc()` doesn't support, rewritten to a
/// bare operand / `-1 * operand`), while plain functions and `if()` pass
/// through untouched.
pub struct MakeExpressionCalculationSafe {
    inner: ReplaceExpressionVisitor,
}

impl Default for MakeExpressionCalculationSafe {
    fn default() -> Self {
        Self::new()
    }
}

impl MakeExpressionCalculationSafe {
    pub fn new() -> Self {
        Self {
            inner: ReplaceExpressionVisitor::new(),
        }
    }
}

impl<'parse> ExpressionVisitor<'parse> for MakeExpressionCalculationSafe {
    type Output = Expression<'parse>;

    fn visit_binary_operation(
        &mut self,
        node: &BinaryOperationExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        if node.operator == BinaryOperator::Modulo {
            let span = node.span()?;
            Ok(Expression::Function(FunctionExpression::new(
                "max".into(),
                ArgumentList::new(
                    vec![Expression::BinaryOperation(node.clone())],
                    IndexMap::new(),
                    IndexMap::new(),
                    span,
                    None,
                    None,
                ),
                span,
                Some("math".into()),
            )))
        } else {
            let left = node.left.accept(self)?;
            let right = node.right.accept(self)?;
            Ok(Expression::BinaryOperation(BinaryOperationExpression::new(
                node.operator,
                left,
                right,
            )))
        }
    }

    fn visit_boolean(
        &mut self,
        node: &BooleanExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        self.inner.visit_boolean(node)
    }

    fn visit_color(&mut self, node: &ColorExpression<'parse>) -> SassResult<Expression<'parse>> {
        self.inner.visit_color(node)
    }

    fn visit_function(
        &mut self,
        node: &FunctionExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        let args = ReplaceExpressionVisitor::replace_argument_list(self, &node.arguments)?;
        Ok(Expression::Function(FunctionExpression::new(
            node.original_name.clone(),
            args,
            node.span,
            node.namespace.clone(),
        )))
    }

    fn visit_if(&mut self, node: &IfExpression<'parse>) -> SassResult<Expression<'parse>> {
        Ok(Expression::If(node.clone()))
    }

    fn visit_interpolated_function(
        &mut self,
        node: &InterpolatedFunctionExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        Ok(Expression::InterpolatedFunction(node.clone()))
    }

    fn visit_legacy_if(
        &mut self,
        node: &LegacyIfExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        let args = ReplaceExpressionVisitor::replace_argument_list(self, &node.arguments)?;
        Ok(Expression::LegacyIf(LegacyIfExpression::new(
            args, node.span,
        )))
    }

    fn visit_list(&mut self, node: &ListExpression<'parse>) -> SassResult<Expression<'parse>> {
        let mut contents = Vec::with_capacity(node.contents.len());
        for item in &node.contents {
            contents.push(item.accept(self)?);
        }
        Ok(Expression::List(ListExpression::new(
            contents,
            node.separator,
            node.span,
            node.has_brackets,
        )))
    }

    fn visit_map(&mut self, node: &MapExpression<'parse>) -> SassResult<Expression<'parse>> {
        let mut pairs = Vec::with_capacity(node.pairs.len());
        for (k, v) in &node.pairs {
            pairs.push((k.accept(self)?, v.accept(self)?));
        }
        Ok(Expression::Map(MapExpression::new(pairs, node.span)))
    }

    fn visit_null(&mut self, node: &NullExpression<'parse>) -> SassResult<Expression<'parse>> {
        self.inner.visit_null(node)
    }

    fn visit_number(&mut self, node: &NumberExpression<'parse>) -> SassResult<Expression<'parse>> {
        self.inner.visit_number(node)
    }

    fn visit_parenthesized(
        &mut self,
        node: &ParenthesizedExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        let expr = node.expression.accept(self)?;
        Ok(Expression::Parenthesized(ParenthesizedExpression::new(
            expr, node.span,
        )))
    }

    fn visit_selector(
        &mut self,
        node: &SelectorExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        self.inner.visit_selector(node)
    }

    fn visit_string(&mut self, node: &StringExpression<'parse>) -> SassResult<Expression<'parse>> {
        let text = ReplaceExpressionVisitor::replace_interpolation(self, &node.text)?;
        Ok(Expression::String(StringExpression::new(
            text,
            node.has_quotes,
        )))
    }

    fn visit_supports(
        &mut self,
        node: &SupportsExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        let cond = ReplaceExpressionVisitor::replace_supports_condition(self, &node.condition)?;
        Ok(Expression::Supports(SupportsExpression::new(cond)))
    }

    fn visit_unary_operation(
        &mut self,
        node: &UnaryOperationExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        match node.operator {
            UnaryOperator::Plus => Ok((*node.operand).clone()),
            UnaryOperator::Minus => {
                Ok(Expression::BinaryOperation(BinaryOperationExpression::new(
                    BinaryOperator::Times,
                    Expression::Number(NumberExpression::new(-1.0, node.span, None)),
                    (*node.operand).clone(),
                )))
            }
            _ => {
                let operand = node.operand.accept(self)?;
                Ok(Expression::UnaryOperation(UnaryOperationExpression::new(
                    node.operator,
                    operand,
                    node.span,
                )))
            }
        }
    }

    fn visit_value(&mut self, node: &ValueExpression<'parse>) -> SassResult<Expression<'parse>> {
        self.inner.visit_value(node)
    }

    fn visit_variable(
        &mut self,
        node: &VariableExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        self.inner.visit_variable(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_if::{
        IfBranch, IfConditionExpression, IfConditionSass, IfExpression,
    };
    use crate::ast::sass::expression_interpolated_function::InterpolatedFunctionExpression;
    use crate::ast::sass::expression_null::NullExpression;
    use crate::ast::sass::expression_number::NumberExpression;
    use crate::ast::sass::interpolation::Interpolation;
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

    #[test]
    fn test_expression_to_calc_simple() {
        let arena = Bump::new();
        let span = test_span(&arena, "42");
        let expr = Expression::Number(NumberExpression::new(42.0, span, None));
        let fe = expression_to_calc(&expr).unwrap();
        assert_eq!(fe.original_name, "calc");
        assert_eq!(fe.arguments.positional.len(), 1);
    }

    #[test]
    fn test_visit_binary_operation_modulo() {
        let arena = Bump::new();
        let span = test_span(&arena, "5 % 3");
        let left = Expression::Number(NumberExpression::new(5.0, span, None));
        let right = Expression::Number(NumberExpression::new(3.0, span, None));
        let node = BinaryOperationExpression::new(BinaryOperator::Modulo, left, right);
        let mut visitor = MakeExpressionCalculationSafe::new();
        let result = visitor.visit_binary_operation(&node).unwrap();
        match result {
            Expression::Function(fe) => {
                assert_eq!(fe.original_name, "max");
                assert_eq!(fe.namespace.as_deref(), Some("math"));
            }
            _ => panic!("expected FunctionExpression"),
        }
    }

    #[test]
    fn test_visit_binary_operation_normal() {
        let arena = Bump::new();
        let span = test_span(&arena, "5 + 3");
        let left = Expression::Number(NumberExpression::new(5.0, span, None));
        let right = Expression::Number(NumberExpression::new(3.0, span, None));
        let node = BinaryOperationExpression::new(BinaryOperator::Plus, left, right);
        let mut visitor = MakeExpressionCalculationSafe::new();
        let result = visitor.visit_binary_operation(&node).unwrap();
        assert!(matches!(result, Expression::BinaryOperation(_)));
    }

    #[test]
    fn test_visit_if_identity() {
        let arena = Bump::new();
        let span = test_span(&arena, "if(true, 1, 2)");
        let branches = vec![
            IfBranch {
                condition: Some(IfConditionExpression::Sass(IfConditionSass::new(
                    Expression::Boolean(BooleanExpression::new(true, span)),
                    span,
                ))),
                expression: Expression::Number(NumberExpression::new(1.0, span, None)),
            },
            IfBranch {
                condition: None,
                expression: Expression::Number(NumberExpression::new(2.0, span, None)),
            },
        ];
        let node = IfExpression::new(branches, span).unwrap();
        let mut visitor = MakeExpressionCalculationSafe::new();
        let result = visitor.visit_if(&node).unwrap();
        assert!(matches!(result, Expression::If(_)));
    }

    #[test]
    fn test_visit_interpolated_function_identity() {
        let arena = Bump::new();
        let span = test_span(&arena, "fn(1)");
        let name = Interpolation::plain("fn".into(), span);
        let args = ArgumentList::new(vec![], IndexMap::new(), IndexMap::new(), span, None, None);
        let node = InterpolatedFunctionExpression::new(name, args, span);
        let mut visitor = MakeExpressionCalculationSafe::new();
        let result = visitor.visit_interpolated_function(&node).unwrap();
        assert!(matches!(result, Expression::InterpolatedFunction(_)));
    }

    #[test]
    fn test_visit_unary_operation_plus() {
        let arena = Bump::new();
        let span = test_span(&arena, "+5");
        let operand = Expression::Number(NumberExpression::new(5.0, span, None));
        let node = UnaryOperationExpression::new(UnaryOperator::Plus, operand, span);
        let mut visitor = MakeExpressionCalculationSafe::new();
        let result = visitor.visit_unary_operation(&node).unwrap();
        match result {
            Expression::Number(n) => assert_eq!(n.value, 5.0),
            _ => panic!("expected NumberExpression, got {:?}", result),
        }
    }

    #[test]
    fn test_visit_unary_operation_minus() {
        let arena = Bump::new();
        let span = test_span(&arena, "-5");
        let operand = Expression::Number(NumberExpression::new(5.0, span, None));
        let node = UnaryOperationExpression::new(UnaryOperator::Minus, operand, span);
        let mut visitor = MakeExpressionCalculationSafe::new();
        let result = visitor.visit_unary_operation(&node).unwrap();
        match result {
            Expression::BinaryOperation(bin) => {
                assert_eq!(bin.operator, BinaryOperator::Times);
                match &*bin.left {
                    Expression::Number(n) => assert_eq!(n.value, -1.0),
                    _ => panic!("expected NumberExpression for left"),
                }
            }
            _ => panic!("expected BinaryOperationExpression"),
        }
    }

    #[test]
    fn test_visit_unary_operation_divide_delegates() {
        let arena = Bump::new();
        let span = test_span(&arena, "/5");
        let operand = Expression::Number(NumberExpression::new(5.0, span, None));
        let node = UnaryOperationExpression::new(UnaryOperator::Divide, operand, span);
        let mut visitor = MakeExpressionCalculationSafe::new();
        let result = visitor.visit_unary_operation(&node).unwrap();
        assert!(matches!(result, Expression::UnaryOperation(_)));
    }

    #[test]
    fn test_visit_null_delegates() {
        let arena = Bump::new();
        let span = test_span(&arena, "null");
        let node = NullExpression::new(span);
        let mut visitor = MakeExpressionCalculationSafe::new();
        let result = visitor.visit_null(&node).unwrap();
        assert!(matches!(result, Expression::Null(_)));
    }
}
