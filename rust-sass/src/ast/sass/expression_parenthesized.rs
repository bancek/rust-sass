// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/parenthesized.dart
// go-source: go/value/sass_expression_parenthesized.go

use crate::ast::sass::interpolation::Interpolation;
use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;

/// An expression wrapped in parentheses.
#[derive(Clone, Debug)]
pub struct ParenthesizedExpression<'parse> {
    /// The internal expression.
    pub expression: Box<Expression<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> ParenthesizedExpression<'parse> {
    pub fn new(expression: Expression<'parse>, span: FileSpan<'parse>) -> Self {
        ParenthesizedExpression {
            expression: Box::new(expression),
            span,
        }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }
}

impl<'parse> ParenthesizedExpression<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        Ok(format!(
            "({})",
            Expression::to_display_string(&self.expression)?
        ))
    }
}

impl<'parse> AstNode<'parse> for ParenthesizedExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for ParenthesizedExpression<'parse> {
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
    use crate::ast::sass::expression_boolean::BooleanExpression;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    #[test]
    fn test_construction() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "(true)", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let inner =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 1, 5)));
        let expr = ParenthesizedExpression::new(inner, span);
        expr.span().unwrap();
    }

    #[test]
    fn test_string() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "(true)", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let inner =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 1, 5)));
        let expr = ParenthesizedExpression::new(inner, span);
        assert_eq!(format!("{expr}"), "(true)");
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "(x)", None);
        let span = FileSpan::new(Some(fs), 0, 3);
        let inner =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 1, 2)));
        let expr = ParenthesizedExpression::new(inner, span);
        assert!(expr.source_interpolation().is_none());
    }
}
