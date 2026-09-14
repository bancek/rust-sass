// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/return_rule.dart
// go-source: go/value/sass_statement_return_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;

#[derive(Clone, Debug)]
/// A `@return` rule.
///
/// This exits the current function body, yielding a return value.
pub struct ReturnRule<'parse> {
    /// The value to return from the enclosing function.
    pub expression: Expression<'parse>,
    pub span: FileSpan<'parse>,
}

impl<'parse> ReturnRule<'parse> {
    pub fn new(expression: Expression<'parse>, span: FileSpan<'parse>) -> Self {
        ReturnRule { expression, span }
    }
}

impl<'parse> AstNode<'parse> for ReturnRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> ReturnRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(
            buf,
            "@return {};",
            Expression::to_display_string(&self.expression)?
        )
        .unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for ReturnRule<'parse> {
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
    fn test_new() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "@return true;", None);
        let span = FileSpan::new(Some(fs), 0, 13);
        let expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 8, 12)));
        let rr = ReturnRule::new(expr, span);
        assert!(matches!(rr.expression, Expression::Boolean(_)));
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "@return true;", None);
        let span = FileSpan::new(Some(fs), 0, 13);
        let expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 8, 12)));
        let rr = ReturnRule::new(expr, span);
        assert_eq!(format!("{rr}"), "@return true;");
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "@return true;", None);
        let span = FileSpan::new(Some(fs), 0, 13);
        let expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 8, 12)));
        let rr = ReturnRule::new(expr, span);
        assert_eq!(rr.span().unwrap(), span);
    }
}
