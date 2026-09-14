// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/binary_operation.dart
// go-source: go/value/sass_expression_binary_operation.go

use crate::common::exception::SassError;
use crate::common::source_span_file_source::FileSource;
use crate::common::span_error::SpanError;
use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::common::span::Span;

use crate::ast::sass::binary_operator::BinaryOperator;
use crate::ast::sass::expression::Expression;
use crate::ast::sass::interpolation::Interpolation;

/// A binary operator expression, as in `1 + 2` or `$this and $other`.
#[derive(Clone, Debug)]
pub struct BinaryOperationExpression<'parse> {
    /// The operator being invoked.
    pub operator: BinaryOperator,
    /// The left-hand operand.
    pub left: Box<Expression<'parse>>,
    /// The right-hand operand.
    pub right: Box<Expression<'parse>>,
    /// Whether this is a [`BinaryOperator::DividedBy`] operation that may be
    /// interpreted as slash-separated numbers.
    pub allows_slash: bool,
}

impl<'parse> BinaryOperationExpression<'parse> {
    pub fn new(
        operator: BinaryOperator,
        left: Expression<'parse>,
        right: Expression<'parse>,
    ) -> Self {
        BinaryOperationExpression {
            operator,
            left: Box::new(left),
            right: Box::new(right),
            allows_slash: false,
        }
    }

    // Creates a [`BinaryOperator::DividedBy`] operation that may be
    // interpreted as slash-separated numbers.
    //
    // Matches Dart: `BinaryOperationExpression.slash` (@nodoc/@internal).
    pub fn new_slash(left: Expression<'parse>, right: Expression<'parse>) -> Self {
        BinaryOperationExpression {
            operator: BinaryOperator::DividedBy,
            left: Box::new(left),
            right: Box::new(right),
            allows_slash: true,
        }
    }

    // Whether this is a [`BinaryOperator::DividedBy`] operation that may be
    // interpreted as slash-separated numbers.
    //
    // Matches Dart: `allowsSlash` (@nodoc/@internal).
    pub fn allows_slash(&self) -> bool {
        self.allows_slash
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }

    // Returns the span that covers only the operator.
    //
    // Matches Dart: `operatorSpan` (@nodoc/@internal).
    pub fn operator_span(&self) -> SassResult<FileSpan<'parse>> {
        let left_span = self.left.span()?;
        let right_span = self.right.span()?;

        // Matches Dart `left.span.file == right.span.file`
        // (`binary_operation.dart`): `SourceFile` defines no `operator ==`,
        // so `==` on files is identity — use `FileSource::identical`, not
        // structural `PartialEq`.
        if FileSource::identical(left_span.file(), right_span.file()) {
            let left_end = left_span.end_location();
            let right_start = right_span.start_location();
            if left_end.offset < right_start.offset {
                let file = left_span.file();
                let result = FileSpan::new(file, left_end.offset, right_start.offset)
                    .trim()
                    .map_err(|e| match e {
                        SpanError::Sass(e) => e,
                        _ => Box::new(SassError::Script {
                            message: "trim failed".into(),
                            argument_name: None,
                        }),
                    })?;
                return Ok(result);
            }
        }
        self.span()
    }
}

impl<'parse> AstNode<'parse> for BinaryOperationExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        let mut left: &Expression = self.left.as_ref();
        while let Expression::BinaryOperation(bin) = left {
            left = bin.left.as_ref();
        }
        let mut right: &Expression = self.right.as_ref();
        while let Expression::BinaryOperation(bin) = right {
            right = bin.right.as_ref();
        }

        let left_span = left.span()?;
        let right_span = right.span()?;

        left_span
            .expand(&Span::File(right_span))
            .map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: "span expansion failed".into(),
                    argument_name: None,
                }),
            })
    }
}

impl<'parse> BinaryOperationExpression<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        let left_needs_parens = if let Expression::BinaryOperation(bin) = self.left.as_ref() {
            bin.operator.precedence() < self.operator.precedence()
        } else if let Expression::List(list) = self.left.as_ref() {
            !list.has_brackets && list.contents.len() >= 2
        } else {
            false
        };

        if left_needs_parens {
            write!(buf, "({})", Expression::to_display_string(&self.left)?).unwrap();
        } else {
            write!(buf, "{}", Expression::to_display_string(&self.left)?).unwrap();
        }

        write!(buf, " {} ", self.operator.operator_syntax()).unwrap();

        let right_needs_parens = if let Expression::BinaryOperation(bin) = self.right.as_ref() {
            bin.operator.precedence() <= self.operator.precedence()
                && !(bin.operator == self.operator && bin.operator.is_associative())
        } else if let Expression::List(list) = self.right.as_ref() {
            !list.has_brackets && list.contents.len() >= 2
        } else {
            false
        };

        if right_needs_parens {
            write!(buf, "({})", Expression::to_display_string(&self.right)?).unwrap();
        } else {
            write!(buf, "{}", Expression::to_display_string(&self.right)?).unwrap();
        }

        Ok(buf)
    }
}

impl<'parse> fmt::Display for BinaryOperationExpression<'parse> {
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
        let left = NumberExpression::new(1.0, test_span(&arena, "1 + 2", 0, 1), None);
        let right = NumberExpression::new(2.0, test_span(&arena, "1 + 2", 4, 5), None);
        let expr = BinaryOperationExpression::new(
            BinaryOperator::Plus,
            Expression::Number(left),
            Expression::Number(right),
        );
        assert_eq!(expr.operator, BinaryOperator::Plus);
        assert!(!expr.allows_slash());
    }

    #[test]
    fn test_slash() {
        let arena = Bump::new();
        let left = NumberExpression::new(1.0, test_span(&arena, "1 / 2", 0, 1), None);
        let right = NumberExpression::new(2.0, test_span(&arena, "1 / 2", 4, 5), None);
        let expr = BinaryOperationExpression::new_slash(
            Expression::Number(left),
            Expression::Number(right),
        );
        assert_eq!(expr.operator, BinaryOperator::DividedBy);
        assert!(expr.allows_slash());
    }

    #[test]
    fn test_string() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "1 + 2", None);
        let span = FileSpan::new(Some(fs), 0, 5);
        let left = Expression::Number(NumberExpression::new(1.0, span, None));
        let right = Expression::Number(NumberExpression::new(2.0, span, None));
        let expr = BinaryOperationExpression::new(BinaryOperator::Plus, left, right);
        assert_eq!(format!("{}", expr), "1 + 2");
    }

    #[test]
    fn test_operator_span_same_allocation_trims() {
        // Left and right in the same `FileSource` allocation: Dart
        // `left.span.file == right.span.file` (`SourceFile ==` = identity)
        // holds, so `operator_span` trims the between-span to `"+"`.
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "1 + 2", None);
        let left = NumberExpression::new(1.0, FileSpan::new(Some(fs), 0, 1), None);
        let right = NumberExpression::new(2.0, FileSpan::new(Some(fs), 4, 5), None);
        let expr = BinaryOperationExpression::new(
            BinaryOperator::Plus,
            Expression::Number(left),
            Expression::Number(right),
        );
        let op = expr.operator_span().unwrap();
        assert_eq!(op.text(), "+");
        assert_eq!(op.start_location().offset, 2);
        assert_eq!(op.end_location().offset, 3);
    }

    #[test]
    fn test_operator_span_distinct_allocation_same_content() {
        // Left and right in separate `FileSource` allocations with identical
        // content are structurally `==` but never `identical`: Dart
        // `SourceFile ==` is identity, so the guard must miss and
        // `operator_span` falls back to the full span (`0..5`, `"1 + 2"`),
        // not the trimmed operator (`2..3`, `"+"`).
        let arena = Bump::new();
        let left = NumberExpression::new(1.0, test_span(&arena, "1 + 2", 0, 1), None);
        let right = NumberExpression::new(2.0, test_span(&arena, "1 + 2", 4, 5), None);
        assert_eq!(
            left.span.file().map(|f| f.text()),
            right.span.file().map(|f| f.text()),
            "setup: distinct allocations with equal content"
        );
        assert!(
            !FileSource::identical(left.span.file(), right.span.file()),
            "setup: allocations must not be identical"
        );
        let expr = BinaryOperationExpression::new(
            BinaryOperator::Plus,
            Expression::Number(left),
            Expression::Number(right),
        );
        let op = expr.operator_span().unwrap();
        assert_eq!(op.text(), "1 + 2");
        assert_eq!(op.start_location().offset, 0);
        assert_eq!(op.end_location().offset, 5);
    }

    #[test]
    fn test_binary_span() {
        let arena = Bump::new();
        let left = NumberExpression::new(1.0, test_span(&arena, "1 + 2", 0, 1), None);
        let right = NumberExpression::new(2.0, test_span(&arena, "1 + 2", 4, 5), None);
        let expr = BinaryOperationExpression::new(
            BinaryOperator::Plus,
            Expression::Number(left),
            Expression::Number(right),
        );
        expr.span().unwrap();
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let left = NumberExpression::new(1.0, test_span(&arena, "1 + 2", 0, 1), None);
        let right = NumberExpression::new(2.0, test_span(&arena, "1 + 2", 4, 5), None);
        let expr = BinaryOperationExpression::new(
            BinaryOperator::Plus,
            Expression::Number(left),
            Expression::Number(right),
        );
        assert!(expr.source_interpolation().is_none());
    }
}
