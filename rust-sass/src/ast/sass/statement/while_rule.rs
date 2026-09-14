// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/while_rule.dart
// go-source: go/value/sass_statement_while_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::statement::Statement;

#[derive(Clone, Debug)]
/// A `@while` rule.
///
/// This repeatedly runs a block while its condition evaluates to true.
pub struct WhileRule<'parse> {
    pub children: Vec<Statement<'parse>>,
    /// The condition controlling whether the block runs again.
    pub condition: Expression<'parse>,
    pub span: FileSpan<'parse>,
}

impl<'parse> WhileRule<'parse> {
    pub fn new(
        condition: Expression<'parse>,
        children: Vec<Statement<'parse>>,
        span: FileSpan<'parse>,
    ) -> Self {
        WhileRule {
            children,
            condition,
            span,
        }
    }
}

impl<'parse> AstNode<'parse> for WhileRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> WhileRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(
            buf,
            "@while {} {{",
            Expression::to_display_string(&self.condition)?
        )
        .unwrap();
        for child in &self.children {
            write!(buf, " {child}").unwrap();
        }
        write!(buf, " }}").unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for WhileRule<'parse> {
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

    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    #[test]
    fn test_new() {
        let arena = Bump::new();
        let span = make_span(&arena, "@while true { }");
        let cond = Expression::Boolean(BooleanExpression::new(true, span));
        let rule = WhileRule::new(cond, vec![], span);
        assert!(matches!(rule.condition, Expression::Boolean(_)));
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "@while true { }");
        let cond = Expression::Boolean(BooleanExpression::new(true, span));
        let rule = WhileRule::new(cond, vec![], span);
        let s = format!("{rule}");
        assert!(s.contains("@while"), "got {s:?}");
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "@while true { }");
        let cond = Expression::Boolean(BooleanExpression::new(true, span));
        let rule = WhileRule::new(cond, vec![], span);
        assert_eq!(rule.span().unwrap(), span);
    }
}
