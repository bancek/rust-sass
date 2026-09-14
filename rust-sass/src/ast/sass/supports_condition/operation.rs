// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/supports_condition/operation.dart
// go-source: go/value/sass_supports_condition_operation.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::common::span::Span;

use crate::ast::sass::expression_if::BooleanOperator;
use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::ast::sass::supports_condition::convert_span_error;
use crate::ast::sass::supports_condition::SupportsCondition;

#[derive(Clone, Debug)]
/// An operation defining the relationship between two conditions:
/// `<left> and|or <right>`.
pub struct SupportsOperation<'parse> {
    /// The left-hand operand.
    pub left: Box<SupportsCondition<'parse>>,
    /// The right-hand operand.
    pub right: Box<SupportsCondition<'parse>>,
    /// The operator.
    pub operator: BooleanOperator,
    /// The span covering the whole `left <op> right` condition.
    pub span: FileSpan<'parse>,
}

impl<'parse> SupportsOperation<'parse> {
    pub fn new(
        left: SupportsCondition<'parse>,
        right: SupportsCondition<'parse>,
        operator: BooleanOperator,
        span: FileSpan<'parse>,
    ) -> Self {
        SupportsOperation {
            left: Box::new(left),
            right: Box::new(right),
            operator,
            span,
        }
    }

    // Rebuilds an interpolation with the same text by splicing both
    // operands' interpolations back between the `before`/`between`/`after`
    // source slices.
    pub fn to_interpolation(&self) -> SassResult<Interpolation<'parse>> {
        let left_interp = self.left.to_interpolation()?;
        let right_interp = self.right.to_interpolation()?;
        let left_fs = self.left.span()?;
        let right_fs = self.right.span()?;
        let left_s = Span::File(left_fs);
        let right_s = Span::File(right_fs);

        let before = self.span.before(&left_s).map_err(convert_span_error)?;
        let between = left_fs.between(&right_s).map_err(convert_span_error)?;
        let after = self.span.after(&right_s).map_err(convert_span_error)?;

        let mut buf = InterpolationBuffer::new();
        buf.write(before.text());
        buf.add_interpolation(&left_interp);
        buf.write(between.text());
        buf.add_interpolation(&right_interp);
        buf.write(after.text());
        buf.interpolation(self.span)
    }

    // Returns a copy of this condition with `span` as its span.
    pub fn with_span(&self, span: FileSpan<'parse>) -> Self {
        Self::new(
            (*self.left).clone(),
            (*self.right).clone(),
            self.operator,
            span,
        )
    }

    // Parenthesizes a nested operand that would otherwise re-parse with
    // different precedence: any negation, or an operation with the same
    // operator (operations are displayed left-associatively, so a same-operator
    // child needs explicit parens to keep the tree shape).
    fn parenthesize(&self, condition: &SupportsCondition<'_>) -> String {
        match condition {
            SupportsCondition::Negation(_) => format!("({condition})"),
            SupportsCondition::Operation(op) if op.operator == self.operator => {
                format!("({condition})")
            }
            _ => format!("{condition}"),
        }
    }
}

impl<'parse> AstNode<'parse> for SupportsOperation<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> SupportsOperation<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(
            buf,
            "{} {} {}",
            self.parenthesize(&self.left),
            self.operator,
            self.parenthesize(&self.right),
        )
        .unwrap();
        Ok(buf)
    }
}

impl fmt::Display for SupportsOperation<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::interpolation::SupportsInterpolation;
    use super::*;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_boolean::BooleanExpression;
    use crate::ast::sass::supports_condition::SupportsCondition;
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

    fn make_interpolation_cond<'compile, 'parse>(
        arena: &'compile Bump,
        text: &'parse str,
    ) -> SupportsCondition<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let span = test_span(arena, text, 0, text.len());
        let expr = Expression::Boolean(BooleanExpression::new(true, span));
        SupportsCondition::Interpolation(SupportsInterpolation::new(expr, span))
    }

    #[test]
    fn test_basic() {
        let arena = Bump::new();
        let left = make_interpolation_cond(&arena, "#{a}");
        let right = make_interpolation_cond(&arena, "#{b}");
        let cond = SupportsOperation::new(
            left,
            right,
            BooleanOperator::And,
            test_span(&arena, "a and b", 0, 7),
        );
        assert_eq!(format!("{cond}"), "#{true} and #{true}");
    }

    #[test]
    fn test_nested_same_op() {
        let arena = Bump::new();
        let inner_left = make_interpolation_cond(&arena, "#{a}");
        let inner_right = make_interpolation_cond(&arena, "#{b}");
        let inner = SupportsCondition::Operation(SupportsOperation::new(
            inner_left,
            inner_right,
            BooleanOperator::And,
            test_span(&arena, "a and b", 0, 7),
        ));
        let right = make_interpolation_cond(&arena, "#{c}");
        let cond = SupportsOperation::new(
            inner,
            right,
            BooleanOperator::And,
            test_span(&arena, "(a and b) and c", 0, 15),
        );
        assert_eq!(format!("{cond}"), "(#{true} and #{true}) and #{true}");
    }
}
