// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/supports_condition/negation.dart
// go-source: go/value/sass_supports_condition_negation.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::common::span::Span;

use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::ast::sass::supports_condition::convert_span_error;
use crate::ast::sass::supports_condition::SupportsCondition;

#[derive(Clone, Debug)]
/// A negated condition: `not <condition>`.
pub struct SupportsNegation<'parse> {
    /// The condition that's been negated.
    pub condition: Box<SupportsCondition<'parse>>,
    /// The span covering the whole `not ...` condition.
    pub span: FileSpan<'parse>,
}

impl<'parse> SupportsNegation<'parse> {
    pub fn new(condition: SupportsCondition<'parse>, span: FileSpan<'parse>) -> Self {
        SupportsNegation {
            condition: Box::new(condition),
            span,
        }
    }

    // Rebuilds an interpolation with the same text by splicing the negated
    // condition's interpolation back between the surrounding source slices.
    pub fn to_interpolation(&self) -> SassResult<Interpolation<'parse>> {
        let inner_interp = self.condition.to_interpolation()?;
        let cond_fs = self.condition.span()?;
        let inner_s = Span::File(cond_fs);

        let before = self.span.before(&inner_s).map_err(convert_span_error)?;
        let after = self.span.after(&inner_s).map_err(convert_span_error)?;

        let mut buf = InterpolationBuffer::new();
        buf.write(before.text());
        buf.add_interpolation(&inner_interp);
        buf.write(after.text());
        buf.interpolation(self.span)
    }

    // Returns a copy of this condition with `span` as its span.
    pub fn with_span(&self, span: FileSpan<'parse>) -> Self {
        Self::new((*self.condition).clone(), span)
    }
}

impl<'parse> AstNode<'parse> for SupportsNegation<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> SupportsNegation<'parse> {
    // Serializes as `not (inner)` when the operand is itself a negation or
    // an operation (which would otherwise re-parse with different
    // precedence), and as `not inner` otherwise.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        let needs_parens = matches!(
            self.condition.as_ref(),
            SupportsCondition::Negation(_) | SupportsCondition::Operation(_)
        );
        if needs_parens {
            write!(buf, "not ({})", self.condition).unwrap();
        } else {
            write!(buf, "not {}", self.condition).unwrap();
        }
        Ok(buf)
    }
}

impl fmt::Display for SupportsNegation<'_> {
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
    use super::super::operation::SupportsOperation;
    use super::*;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_boolean::BooleanExpression;
    use crate::ast::sass::expression_if::BooleanOperator;
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
        text: &str,
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
    fn test_plain_inner() {
        let arena = Bump::new();
        let inner = make_interpolation_cond(&arena, "#{x}");
        let cond = SupportsNegation::new(inner, test_span(&arena, "not x", 0, 5));
        assert_eq!(format!("{cond}"), "not #{true}");
    }

    #[test]
    fn test_negated_inner() {
        let arena = Bump::new();
        let inner_inner = make_interpolation_cond(&arena, "#{x}");
        let inner_neg = SupportsCondition::Negation(SupportsNegation::new(
            inner_inner,
            test_span(&arena, "not x", 0, 5),
        ));
        let cond = SupportsNegation::new(inner_neg, test_span(&arena, "not (not x)", 0, 11));
        assert_eq!(format!("{cond}"), "not (not #{true})");
    }

    #[test]
    fn test_operation_inner() {
        let arena = Bump::new();
        let left = make_interpolation_cond(&arena, "#{a}");
        let right = make_interpolation_cond(&arena, "#{b}");
        let inner_op = SupportsCondition::Operation(SupportsOperation::new(
            left,
            right,
            BooleanOperator::And,
            test_span(&arena, "a and b", 0, 7),
        ));
        let cond = SupportsNegation::new(inner_op, test_span(&arena, "not (a and b)", 0, 13));
        assert_eq!(format!("{cond}"), "not (#{true} and #{true})");
    }
}
