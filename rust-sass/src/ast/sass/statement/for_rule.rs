// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/for_rule.dart
// go-source: go/value/sass_statement_for_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::statement::Statement;

/// A `@for` rule.
///
/// Iterates a set number of times.
#[derive(Clone, Debug)]
pub struct ForRule<'parse> {
    pub children: Vec<Statement<'parse>>,
    /// The name of the variable that will contain the index value.
    pub variable: String,
    /// The expression for the start index.
    pub from: Expression<'parse>,
    /// The expression for the end index.
    pub to: Expression<'parse>,
    /// Whether [`ForRule::to`] is exclusive (`to`) rather than inclusive
    /// (`through`).
    pub is_exclusive: bool,
    pub span: FileSpan<'parse>,
}

impl<'parse> ForRule<'parse> {
    pub fn new(
        variable: String,
        from: Expression<'parse>,
        to: Expression<'parse>,
        children: Vec<Statement<'parse>>,
        span: FileSpan<'parse>,
        exclusive: bool,
    ) -> Self {
        ForRule {
            children,
            variable,
            from,
            to,
            is_exclusive: exclusive,
            span,
        }
    }
}

impl<'parse> AstNode<'parse> for ForRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> ForRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        let op = if self.is_exclusive { "to" } else { "through" };
        write!(
            buf,
            "@for ${} from {} {op} {} {{",
            self.variable,
            Expression::to_display_string(&self.from)?,
            Expression::to_display_string(&self.to)?
        )
        .unwrap();
        for child in &self.children {
            write!(buf, " {child}").unwrap();
        }
        write!(buf, " }}").unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for ForRule<'parse> {
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
        let span = make_span(&arena, "@for $i from 1 to 5 { }");
        let from = Expression::Boolean(BooleanExpression::new(true, span));
        let to = Expression::Boolean(BooleanExpression::new(true, span));
        let rule = ForRule::new("i".into(), from, to, vec![], span, true);
        assert_eq!(rule.variable, "i");
        assert!(rule.is_exclusive);
    }

    #[test]
    fn test_display_exclusive() {
        let arena = Bump::new();
        let span = make_span(&arena, "@for $i from 1 to 5 { }");
        let from = Expression::Boolean(BooleanExpression::new(true, span));
        let to = Expression::Boolean(BooleanExpression::new(true, span));
        let rule = ForRule::new("i".into(), from, to, vec![], span, true);
        let s = format!("{rule}");
        assert!(s.contains("to"), "got {s:?}");
    }

    #[test]
    fn test_display_inclusive() {
        let arena = Bump::new();
        let span = make_span(&arena, "@for $i from 1 through 5 { }");
        let from = Expression::Boolean(BooleanExpression::new(true, span));
        let to = Expression::Boolean(BooleanExpression::new(true, span));
        let rule = ForRule::new("i".into(), from, to, vec![], span, false);
        let s = format!("{rule}");
        assert!(s.contains("through"), "got {s:?}");
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "@for $i from 1 to 5 { }");
        let from = Expression::Boolean(BooleanExpression::new(true, span));
        let to = Expression::Boolean(BooleanExpression::new(true, span));
        let rule = ForRule::new("i".into(), from, to, vec![], span, true);
        assert_eq!(rule.span().unwrap(), span);
    }
}
