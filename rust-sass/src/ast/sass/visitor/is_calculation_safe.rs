// Copyright 2024 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/is_calculation_safe.dart
// go-source: go/value/visitor_is_calculation_safe.go

use crate::ast::sass::binary_operator::BinaryOperator;
use crate::ast::sass::expression::ExpressionVisitor;
use crate::ast::sass::expression_binary_operation::BinaryOperationExpression;
use crate::ast::sass::expression_boolean::BooleanExpression;
use crate::ast::sass::expression_color::ColorExpression;
use crate::ast::sass::expression_function::FunctionExpression;
use crate::ast::sass::expression_if::IfExpression;
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
use crate::value::ListSeparator;

// Deliberately exhaustive rather than built on a default-true search: a new
// expression variant must fail closed here until it gets its own arm.

/// A visitor that determines whether an expression is valid in a calculation
/// context.
///
/// Use through [`Expression::is_calculation_safe`]: each arm reports whether
/// that node can appear inside a `calc()` — arithmetic operators only when
/// both sides are safe, functions/variables/numbers/`if()` unconditionally,
/// and space-separated multi-element lists of safe elements.
pub struct IsCalculationSafeVisitor;

impl IsCalculationSafeVisitor {
    pub fn new() -> Self {
        IsCalculationSafeVisitor
    }
}

impl Default for IsCalculationSafeVisitor {
    fn default() -> Self {
        Self::new()
    }
}

impl<'parse> ExpressionVisitor<'parse> for IsCalculationSafeVisitor {
    type Output = bool;

    fn visit_binary_operation(
        &mut self,
        node: &BinaryOperationExpression<'parse>,
    ) -> SassResult<bool> {
        match node.operator {
            BinaryOperator::Times
            | BinaryOperator::DividedBy
            | BinaryOperator::Plus
            | BinaryOperator::Minus => {
                let safe = node.left.accept(self)?;
                if !safe {
                    return Ok(false);
                }
                node.right.accept(self)
            }
            _ => Ok(false),
        }
    }

    fn visit_boolean(&mut self, _node: &BooleanExpression<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_color(&mut self, _node: &ColorExpression<'parse>) -> SassResult<bool> {
        Ok(false)
    }

    fn visit_function(&mut self, _node: &FunctionExpression<'parse>) -> SassResult<bool> {
        Ok(true)
    }

    fn visit_if(&mut self, _node: &IfExpression<'parse>) -> SassResult<bool> {
        Ok(true)
    }

    fn visit_interpolated_function(
        &mut self,
        _node: &InterpolatedFunctionExpression<'parse>,
    ) -> SassResult<bool> {
        Ok(true)
    }

    fn visit_legacy_if(&mut self, _node: &LegacyIfExpression<'parse>) -> SassResult<bool> {
        Ok(true)
    }

    fn visit_list(&mut self, node: &ListExpression<'parse>) -> SassResult<bool> {
        if node.separator != ListSeparator::Space {
            return Ok(false);
        }
        if node.has_brackets {
            return Ok(false);
        }
        if node.contents.len() <= 1 {
            return Ok(false);
        }
        for expr in &node.contents {
            let safe = expr.accept(self)?;
            if !safe {
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
        if node.has_quotes {
            return Ok(false);
        }

        let text = node.text.initial_plain();
        // Dart indexes UTF-16 code units (`codeUnitAtOrNull(1/3)`); Rust
        // indexes chars. Byte indexing would disagree on non-ASCII text
        // (`é+foo` etc.) — compare the 2nd/4th chars, not bytes.
        let mut chars = text.chars();
        let second = chars.nth(1);
        let fourth = chars.nth(1);
        Ok(!text.starts_with('!')
            && !text.starts_with('#')
            && second != Some('+')
            && fourth != Some('('))
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
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::argument_list::ArgumentList;
    use crate::ast::sass::binary_operator::BinaryOperator;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::ast::sass::unary_operator::UnaryOperator;
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
    fn test_number() {
        let arena = Bump::new();
        let span = test_span(&arena, "1", 0, 1);
        let expr = Expression::Number(NumberExpression::new(1.0, span, None));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_function() {
        let arena = Bump::new();
        let span = test_span(&arena, "calc()", 0, 6);
        let args = ArgumentList::empty(span);
        let expr = Expression::Function(FunctionExpression::new("calc".into(), args, span, None));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_variable() {
        let arena = Bump::new();
        let span = test_span(&arena, "$x", 0, 2);
        let expr = Expression::Variable(VariableExpression::new("x".into(), span, None));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_null() {
        let arena = Bump::new();
        let span = test_span(&arena, "null", 0, 4);
        let expr = Expression::Null(NullExpression::new(span));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_boolean() {
        let arena = Bump::new();
        let span = test_span(&arena, "true", 0, 4);
        let expr = Expression::Boolean(BooleanExpression::new(true, span));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_color() {
        let arena = Bump::new();
        let span = test_span(&arena, "red", 0, 3);
        let color = SassColor::rgb(1.0, 2.0, 3.0, 1.0);
        let expr = Expression::Color(ColorExpression::new(color, span));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_binary_op_plus() {
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
        let mut v = IsCalculationSafeVisitor::new();
        assert!(expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_binary_op_equals() {
        let arena = Bump::new();
        let left_span = test_span(&arena, "1 == 2", 0, 1);
        let right_span = test_span(&arena, "1 == 2", 5, 6);
        let left = Expression::Number(NumberExpression::new(1.0, left_span, None));
        let right = Expression::Number(NumberExpression::new(2.0, right_span, None));
        let expr = Expression::BinaryOperation(BinaryOperationExpression::new(
            BinaryOperator::Equals,
            left,
            right,
        ));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_list_space_two_plus() {
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
        let mut v = IsCalculationSafeVisitor::new();
        assert!(expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_list_comma() {
        let arena = Bump::new();
        let span = test_span(&arena, "1, 2", 0, 4);
        let contents = vec![
            Expression::Number(NumberExpression::new(1.0, span, None)),
            Expression::Number(NumberExpression::new(2.0, span, None)),
        ];
        let expr = Expression::List(ListExpression::new(
            contents,
            ListSeparator::Comma,
            span,
            false,
        ));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_list_bracketed() {
        let arena = Bump::new();
        let span = test_span(&arena, "[1 2]", 0, 5);
        let contents = vec![
            Expression::Number(NumberExpression::new(1.0, span, None)),
            Expression::Number(NumberExpression::new(2.0, span, None)),
        ];
        let expr = Expression::List(ListExpression::new(
            contents,
            ListSeparator::Space,
            span,
            true,
        ));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_list_single() {
        let arena = Bump::new();
        let span = test_span(&arena, "1", 0, 1);
        let contents = vec![Expression::Number(NumberExpression::new(1.0, span, None))];
        let expr = Expression::List(ListExpression::new(
            contents,
            ListSeparator::Space,
            span,
            false,
        ));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_string_unquoted() {
        let arena = Bump::new();
        let span = test_span(&arena, "foo", 0, 3);
        let text = Interpolation::plain("foo".into(), span);
        let expr = Expression::String(StringExpression::new(text, false));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_string_quoted() {
        let arena = Bump::new();
        let span = test_span(&arena, "\"foo\"", 0, 5);
        let text = Interpolation::plain("foo".into(), span);
        let expr = Expression::String(StringExpression::new(text, true));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_string_important() {
        let arena = Bump::new();
        let span = test_span(&arena, "!important", 0, 10);
        let text = Interpolation::plain("!important".into(), span);
        let expr = Expression::String(StringExpression::new(text, false));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_unary_operation() {
        let arena = Bump::new();
        let span = test_span(&arena, "-1", 0, 2);
        let inner_span = test_span(&arena, "-1", 1, 2);
        let inner = Expression::Number(NumberExpression::new(1.0, inner_span, None));
        let expr = Expression::UnaryOperation(UnaryOperationExpression::new(
            UnaryOperator::Minus,
            inner,
            span,
        ));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[test]
    fn test_selector() {
        let arena = Bump::new();
        let span = test_span(&arena, "&", 0, 1);
        let expr = Expression::Selector(SelectorExpression::new(span));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(!expr.accept(&mut v).unwrap());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_calc_safe_non_ascii_indexing() {
        // Dart checks UTF-16 units 1 and 3; byte indexing
        // disagrees on non-ASCII text. `é+foo`: byte[1] is a UTF-8
        // continuation (not `+`) but char[1] IS `+` → unsafe (false).
        let arena = Bump::new();
        let span = test_span(&arena, "é+foo", 0, 6);
        let text = Interpolation::plain("é+foo".into(), span);
        let expr = Expression::String(StringExpression::new(text, false));
        let mut v = IsCalculationSafeVisitor::new();
        assert!(
            !expr.accept(&mut v).unwrap(),
            "char[1] == '+' must make é+foo calculation-unsafe"
        );
    }
}
