// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/debug_rule.dart
// go-source: go/value/sass_statement_debug_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;

/// A `@debug` rule.
///
/// Prints a Sass value for debugging purposes.
#[derive(Clone, Debug)]
pub struct DebugRule<'parse> {
    /// The expression to print.
    pub expression: Expression<'parse>,
    pub span: FileSpan<'parse>,
}

impl<'parse> DebugRule<'parse> {
    pub fn new(expression: Expression<'parse>, span: FileSpan<'parse>) -> Self {
        DebugRule { expression, span }
    }
}

impl<'parse> DebugRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(
            buf,
            "@debug {};",
            Expression::to_display_string(&self.expression)?
        )
        .unwrap();
        Ok(buf)
    }
}

impl<'parse> AstNode<'parse> for DebugRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for DebugRule<'parse> {
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
        let fs = FileSource::new_in(&arena, "@debug true;", None);
        let span = FileSpan::new(Some(fs), 0, 12);
        let expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 7, 11)));
        let dr = DebugRule::new(expr, span);
        assert!(matches!(dr.expression, Expression::Boolean(_)));
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "@debug true;", None);
        let span = FileSpan::new(Some(fs), 0, 12);
        let expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 7, 11)));
        let dr = DebugRule::new(expr, span);
        assert_eq!(dr.span().unwrap(), span);
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "@debug true;", None);
        let span = FileSpan::new(Some(fs), 0, 12);
        let expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 7, 11)));
        let dr = DebugRule::new(expr, span);
        assert_eq!(format!("{dr}"), "@debug true;");
    }
}
