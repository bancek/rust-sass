// Copyright 2026 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/is_plain_css.dart
// go-source: go/value/visitor_is_plain_css.go

use crate::ast::sass::argument_list::ArgumentList;
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
use crate::common::exception::SassResult;

// Deliberately exhaustive rather than built on a default-true search: a new
// expression variant must fail closed here until it gets its own arm.

/// A visitor that determines whether an expression is valid plain CSS that
/// will produce the same result as it would in Sass.
///
/// Use through [`Expression::is_plain_css`]. Colors, numbers, namespace-less
/// plain-CSS functions, and `if()` branches of plain CSS pass; Sass-only
/// constructs (variables, operations, `supports()`, ...) do not.
///
/// If `allow_interpolation` is `true`, interpolated expressions are allowed as
/// an exception, even if they contain SassScript.
pub struct IsPlainCssVisitor {
    /// Whether to allow interpolation as an exception to allowing plain CSS.
    pub allow_interpolation: bool,
}

impl IsPlainCssVisitor {
    pub fn new(allow_interpolation: bool) -> Self {
        IsPlainCssVisitor {
            allow_interpolation,
        }
    }

    // Returns whether `list` contains only plain CSS: no named/rest
    // arguments, and every positional argument plain CSS itself.
    fn visit_argument_list(&mut self, list: &ArgumentList<'_>) -> SassResult<bool> {
        if !list.named.is_empty() || list.rest.is_some() {
            return Ok(false);
        }
        for arg in &list.positional {
            if !arg.accept(self)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

impl<'parse> ExpressionVisitor<'parse> for IsPlainCssVisitor {
    type Output = bool;

    fn visit_binary_operation(
        &mut self,
        _node: &BinaryOperationExpression<'parse>,
    ) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_boolean(&mut self, _node: &BooleanExpression<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_color(&mut self, _node: &ColorExpression<'parse>) -> SassResult<bool> {
        Ok(true)
    }

    fn visit_function(&mut self, node: &FunctionExpression<'parse>) -> SassResult<bool> {
        if node.namespace.is_some() {
            return Ok(false);
        }
        self.visit_argument_list(&node.arguments)
    }

    fn visit_if(&mut self, node: &IfExpression<'parse>) -> SassResult<bool> {
        for branch in &node.branches {
            if let Some(ref cond) = branch.condition {
                if !cond.accept(self)? {
                    return Ok(false);
                }
            }
            if !branch.expression.accept(self)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn visit_interpolated_function(
        &mut self,
        node: &InterpolatedFunctionExpression<'parse>,
    ) -> SassResult<bool> {
        if !self.allow_interpolation {
            return Ok(false);
        }
        self.visit_argument_list(&node.arguments)
    }

    fn visit_legacy_if(&mut self, _node: &LegacyIfExpression<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_list(&mut self, node: &ListExpression<'parse>) -> SassResult<bool> {
        if node.contents.is_empty() && !node.has_brackets {
            return Ok(false);
        }
        for element in &node.contents {
            if !element.accept(self)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn visit_map(&mut self, _node: &MapExpression<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_null(&mut self, _node: &NullExpression<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_number(&mut self, _node: &NumberExpression<'parse>) -> SassResult<bool> {
        Ok(true)
    }

    fn visit_parenthesized(&mut self, node: &ParenthesizedExpression<'parse>) -> SassResult<bool> {
        node.expression.accept(self)
    }

    fn visit_selector(&mut self, _node: &SelectorExpression<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_string(&mut self, node: &StringExpression<'parse>) -> SassResult<bool> {
        Ok(self.allow_interpolation || node.text.is_plain())
    }

    fn visit_supports(&mut self, _node: &SupportsExpression) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_unary_operation(
        &mut self,
        _node: &UnaryOperationExpression<'parse>,
    ) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_value(&mut self, _node: &ValueExpression<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_variable(&mut self, _node: &VariableExpression<'parse>) -> SassResult<bool> {
        Ok(false)
    }
}

impl<'parse> IfConditionExpressionVisitor<'parse> for IsPlainCssVisitor {
    type Output = bool;

    fn visit_parenthesized(&mut self, node: &IfConditionParenthesized<'parse>) -> SassResult<bool> {
        node.expression.accept(self)
    }

    fn visit_negation(&mut self, node: &IfConditionNegation<'parse>) -> SassResult<bool> {
        node.expression.accept(self)
    }

    fn visit_operation(&mut self, node: &IfConditionOperation<'parse>) -> SassResult<bool> {
        for expr in &node.expressions {
            if !expr.accept(self)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn visit_function(&mut self, node: &IfConditionFunction<'parse>) -> SassResult<bool> {
        Ok(self.allow_interpolation || (node.name.is_plain() && node.arguments.is_plain()))
    }

    fn visit_sass(&mut self, _node: &IfConditionSass<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_raw(&mut self, node: &IfConditionRaw<'parse>) -> SassResult<bool> {
        Ok(self.allow_interpolation || node.text.is_plain())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::binary_operator::BinaryOperator;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::value::color::SassColor;
    use crate::value::ListSeparator;
    use bumpalo::Bump;

    fn test_span<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
        s: usize,
        e: usize,
    ) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), s, e)
    }

    #[test]
    fn test_color() {
        let arena = Bump::new();
        let span = test_span(&arena, "green", 0, 5);
        let color = SassColor::rgb(1.0, 2.0, 3.0, 1.0);
        let expr = Expression::Color(ColorExpression::new(color, span));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_number() {
        let arena = Bump::new();
        let span = test_span(&arena, "1", 0, 1);
        let expr = Expression::Number(NumberExpression::new(1.0, span, None));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_boolean() {
        let arena = Bump::new();
        let span = test_span(&arena, "true", 0, 4);
        let expr = Expression::Boolean(BooleanExpression::new(true, span));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_null() {
        let arena = Bump::new();
        let span = test_span(&arena, "null", 0, 4);
        let expr = Expression::Null(NullExpression::new(span));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_variable() {
        let arena = Bump::new();
        let span = test_span(&arena, "$x", 0, 2);
        let expr = Expression::Variable(VariableExpression::new("x".into(), span, None));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_selector() {
        let arena = Bump::new();
        let span = test_span(&arena, "&", 0, 1);
        let expr = Expression::Selector(SelectorExpression::new(span));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_map() {
        let arena = Bump::new();
        let span = test_span(&arena, "()", 0, 2);
        let expr = Expression::Map(MapExpression::new(vec![], span));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_string_plain() {
        let arena = Bump::new();
        let span = test_span(&arena, "foo", 0, 3);
        let text = Interpolation::plain("foo".into(), span);
        let expr = Expression::String(StringExpression::new(text, false));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_string_interpolated_disallow() {
        let arena = Bump::new();
        let s = test_span(&arena, "#{$x}", 0, 5);
        let expr_span = test_span(&arena, "#{$x}", 2, 4);
        let var_expr = Expression::Variable(VariableExpression::new("x".into(), expr_span, None));
        let mut buf = InterpolationBuffer::new();
        buf.add(var_expr, expr_span);
        let text = buf.interpolation(s).unwrap();
        let expr = Expression::String(StringExpression::new(text, false));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_string_interpolated_allow() {
        let arena = Bump::new();
        let s = test_span(&arena, "#{$x}", 0, 5);
        let expr_span = test_span(&arena, "#{$x}", 2, 4);
        let var_expr = Expression::Variable(VariableExpression::new("x".into(), expr_span, None));
        let mut buf = InterpolationBuffer::new();
        buf.add(var_expr, expr_span);
        let text = buf.interpolation(s).unwrap();
        let expr = Expression::String(StringExpression::new(text, false));
        let mut v = IsPlainCssVisitor::new(true);
        assert!(expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_function_no_namespace() {
        let arena = Bump::new();
        let span = test_span(&arena, "rgb()", 0, 5);
        let args = ArgumentList::empty(span);
        let expr = Expression::Function(FunctionExpression::new("rgb".into(), args, span, None));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_function_with_namespace() {
        let arena = Bump::new();
        let span = test_span(&arena, "ns.rgb()", 0, 8);
        let args = ArgumentList::empty(span);
        let expr = Expression::Function(FunctionExpression::new(
            "rgb".into(),
            args,
            span,
            Some("ns".into()),
        ));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_binary_operation() {
        let arena = Bump::new();
        let left_span = test_span(&arena, "1 + 2", 0, 1);
        let right_span = test_span(&arena, "1 + 2", 4, 5);
        let left = Expression::Number(NumberExpression::new(1.0, left_span, None));
        let right = Expression::Number(NumberExpression::new(2.0, right_span, None));
        let expr = Expression::BinaryOperation(BinaryOperationExpression::new(
            BinaryOperator::Plus,
            left,
            right,
        ));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_parenthesized() {
        let arena = Bump::new();
        let span = test_span(&arena, "(red)", 0, 5);
        let inner_span = test_span(&arena, "red", 1, 4);
        let color = SassColor::rgb(1.0, 2.0, 3.0, 1.0);
        let inner = Expression::Color(ColorExpression::new(color, inner_span));
        let expr = Expression::Parenthesized(ParenthesizedExpression::new(inner, span));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_list_empty_unbracketed() {
        let arena = Bump::new();
        let span = test_span(&arena, "", 0, 0);
        let expr = Expression::List(ListExpression::new(
            vec![],
            ListSeparator::Space,
            span,
            false,
        ));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_list_all_numbers() {
        let arena = Bump::new();
        let span = test_span(&arena, "1 2", 0, 3);
        let contents = vec![
            Expression::Number(NumberExpression::new(1.0, span, None)),
            Expression::Number(NumberExpression::new(2.0, span, None)),
        ];
        let expr = Expression::List(ListExpression::new(
            contents,
            ListSeparator::Space,
            span,
            false,
        ));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_list_with_non_plain() {
        let arena = Bump::new();
        let span = test_span(&arena, "1 $x", 0, 4);
        let contents = vec![
            Expression::Number(NumberExpression::new(1.0, span, None)),
            Expression::Variable(VariableExpression::new("x".into(), span, None)),
        ];
        let expr = Expression::List(ListExpression::new(
            contents,
            ListSeparator::Space,
            span,
            false,
        ));
        let mut v = IsPlainCssVisitor::new(false);
        assert!(!expr.accept(&mut v).unwrap());
    }
}
