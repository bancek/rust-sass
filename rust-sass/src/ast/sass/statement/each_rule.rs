// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/each_rule.dart
// go-source: go/value/sass_statement_each_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::statement::Statement;

/// An `@each` rule.
///
/// Iterates over the values in a list or map.
#[derive(Clone, Debug)]
pub struct EachRule<'parse> {
    pub children: Vec<Statement<'parse>>,
    /// The variables assigned for each iteration.
    pub variables: Vec<String>,
    /// The expression whose value this iterates through.
    pub list: Expression<'parse>,
    pub span: FileSpan<'parse>,
}

impl<'parse> EachRule<'parse> {
    pub fn new(
        variables: Vec<String>,
        list: Expression<'parse>,
        children: Vec<Statement<'parse>>,
        span: FileSpan<'parse>,
    ) -> Self {
        EachRule {
            children,
            variables,
            list,
            span,
        }
    }
}

impl<'parse> AstNode<'parse> for EachRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> EachRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        let vars: Vec<String> = self.variables.iter().map(|v| format!("${v}")).collect();
        write!(
            buf,
            "@each {} in {} {{",
            vars.join(", "),
            Expression::to_display_string(&self.list)?
        )
        .unwrap();
        for child in &self.children {
            write!(buf, " {child}").unwrap();
        }
        write!(buf, " }}").unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for EachRule<'parse> {
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
        let span = make_span(&arena, "@each $v in list { }");
        let list = Expression::Boolean(BooleanExpression::new(true, span));
        let rule = EachRule::new(vec!["v".into()], list, vec![], span);
        assert_eq!(rule.variables.len(), 1);
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "@each $v in true { }");
        let list = Expression::Boolean(BooleanExpression::new(true, span));
        let rule = EachRule::new(vec!["v".into()], list, vec![], span);
        let s = format!("{rule}");
        assert!(s.contains("@each"), "got {s:?}");
        assert!(s.contains("$v"), "got {s:?}");
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "@each $v in list { }");
        let list = Expression::Boolean(BooleanExpression::new(true, span));
        let rule = EachRule::new(vec!["v".into()], list, vec![], span);
        assert_eq!(rule.span().unwrap(), span);
    }
}
