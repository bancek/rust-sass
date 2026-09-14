// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/unary_operation.dart
// go-source: go/value/sass_expression_unary_operation.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::unary_operator::UnaryOperator;

/// A unary operator expression, as in `+$var` or `not fn()`.
#[derive(Clone, Debug)]
pub struct UnaryOperationExpression<'parse> {
    /// The operator being invoked.
    pub operator: UnaryOperator,
    /// The operand.
    pub operand: Box<Expression<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> UnaryOperationExpression<'parse> {
    pub fn new(
        operator: UnaryOperator,
        operand: Expression<'parse>,
        span: FileSpan<'parse>,
    ) -> Self {
        UnaryOperationExpression {
            operator,
            operand: Box::new(operand),
            span,
        }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }
}

impl<'parse> AstNode<'parse> for UnaryOperationExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> UnaryOperationExpression<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "{}", self.operator.operator_syntax()).unwrap();
        if self.operator == UnaryOperator::Not {
            write!(buf, " ").unwrap();
        }
        let needs_parens = matches!(self.operand.as_ref(), Expression::BinaryOperation(_))
            || matches!(self.operand.as_ref(), Expression::UnaryOperation(_))
            || matches!(self.operand.as_ref(), Expression::List(list)
                if !list.has_brackets && list.contents.len() >= 2);
        if needs_parens {
            write!(buf, "({})", Expression::to_display_string(&self.operand)?).unwrap();
        } else {
            write!(buf, "{}", Expression::to_display_string(&self.operand)?).unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for UnaryOperationExpression<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::expression_number::NumberExpression;
    use crate::common::source_span_file_source::FileSource;
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
    fn test_construction() {
        let arena = Bump::new();
        let span = test_span(&arena, "-1", 0, 2);
        let inner = NumberExpression::new(1.0, span, None);
        let expr =
            UnaryOperationExpression::new(UnaryOperator::Minus, Expression::Number(inner), span);
        assert_eq!(expr.operator, UnaryOperator::Minus);
    }

    #[test]
    fn test_string_minus() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "-5", None);
        let span = FileSpan::new(Some(fs), 0, 2);
        let operand = Expression::Number(NumberExpression::new(5.0, span, None));
        let expr = UnaryOperationExpression::new(UnaryOperator::Minus, operand, span);
        assert_eq!(format!("{}", expr), "-5");
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let span = test_span(&arena, "-1", 0, 2);
        let inner = NumberExpression::new(1.0, span, None);
        let expr =
            UnaryOperationExpression::new(UnaryOperator::Minus, Expression::Number(inner), span);
        assert!(expr.source_interpolation().is_none());
    }
}
