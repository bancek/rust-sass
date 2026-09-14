// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/error_rule.dart
// go-source: go/value/sass_statement_error_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;

/// An `@error` rule.
///
/// Emits an error and stops execution.
#[derive(Clone, Debug)]
pub struct ErrorRule<'parse> {
    /// The expression to evaluate for the error message.
    pub expression: Expression<'parse>,
    pub span: FileSpan<'parse>,
}

impl<'parse> ErrorRule<'parse> {
    pub fn new(expression: Expression<'parse>, span: FileSpan<'parse>) -> Self {
        ErrorRule { expression, span }
    }
}

impl<'parse> AstNode<'parse> for ErrorRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> ErrorRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(
            buf,
            "@error {};",
            Expression::to_display_string(&self.expression)?
        )
        .unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for ErrorRule<'parse> {
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
    use crate::ast::sass::expression_string::StringExpression;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    #[test]
    fn test_new() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "@error \"msg\";", None);
        let span = FileSpan::new(Some(fs), 0, 14);
        let expr = Expression::String(StringExpression::plain(
            "msg",
            FileSpan::new(Some(fs), 7, 12),
            true,
        ));
        let er = ErrorRule::new(expr, span);
        assert!(matches!(er.expression, Expression::String(_)));
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "@error \"msg\";", None);
        let span = FileSpan::new(Some(fs), 0, 14);
        let expr = Expression::String(StringExpression::plain(
            "msg",
            FileSpan::new(Some(fs), 7, 12),
            true,
        ));
        let er = ErrorRule::new(expr, span);
        assert_eq!(format!("{er}"), "@error \"msg\";");
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "@error \"msg\";", None);
        let span = FileSpan::new(Some(fs), 0, 14);
        let expr = Expression::String(StringExpression::plain(
            "msg",
            FileSpan::new(Some(fs), 7, 12),
            true,
        ));
        let er = ErrorRule::new(expr, span);
        assert_eq!(er.span().unwrap(), span);
    }
}
