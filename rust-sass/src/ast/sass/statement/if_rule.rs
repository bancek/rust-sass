// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/if_rule.dart
// go-source: go/value/sass_statement_if_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::statement::Statement;

/// An `@if` rule.
///
/// Conditionally executes a block of code.
#[derive(Clone, Debug)]
pub struct IfRule<'parse> {
    /// The `@if` and `@else if` clauses.
    ///
    /// The first clause whose expression evaluates to true has its
    /// statements executed. If none matches, [`IfRule::last_clause`] runs
    /// when present.
    pub clauses: Vec<IfClause<'parse>>,
    /// The final, unconditional `@else` clause.
    ///
    /// `None` when there is no unconditional `@else`.
    pub last_clause: Option<Vec<Statement<'parse>>>,
    pub span: FileSpan<'parse>,
}

/// An `@if` or `@else if` clause in an [`IfRule`].
///
/// The final unconditional `@else`, when present, is stored directly as
/// [`IfRule::last_clause`] rather than as a separate clause node.
//
// Matches Dart: `IfRuleClause.hasDeclarations` (if_rule.dart) lives in the
// evaluator as the `has_declarations()` helper, not on the AST.
#[derive(Clone, Debug)]
pub struct IfClause<'parse> {
    /// The expression to evaluate to determine whether to run this rule.
    pub expression: Expression<'parse>,
    pub children: Vec<Statement<'parse>>,
}

impl<'parse> IfRule<'parse> {
    pub fn new(
        clauses: Vec<IfClause<'parse>>,
        span: FileSpan<'parse>,
        last_clause: Option<Vec<Statement<'parse>>>,
    ) -> Self {
        IfRule {
            clauses,
            last_clause,
            span,
        }
    }
}

impl<'parse> IfClause<'parse> {
    pub fn new(expression: Expression<'parse>, children: Vec<Statement<'parse>>) -> Self {
        IfClause {
            expression,
            children,
        }
    }
}

impl<'parse> AstNode<'parse> for IfRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> IfRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        for (i, clause) in self.clauses.iter().enumerate() {
            if i > 0 {
                write!(buf, " ").unwrap();
            }
            if i == 0 {
                write!(buf, "@if ").unwrap();
            } else {
                write!(buf, "@else if ").unwrap();
            }
            write!(
                buf,
                "{} {{",
                Expression::to_display_string(&clause.expression)?
            )
            .unwrap();
            for child in &clause.children {
                write!(buf, " {child}").unwrap();
            }
            write!(buf, " }}").unwrap();
        }
        if let Some(ref last) = self.last_clause {
            write!(buf, " @else {{").unwrap();
            for child in last {
                write!(buf, " {child}").unwrap();
            }
            write!(buf, " }}").unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for IfRule<'parse> {
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
        let span = make_span(&arena, "@if true { }");
        let cond = Expression::Boolean(BooleanExpression::new(true, span));
        let clause = IfClause::new(cond, vec![]);
        let ir = IfRule::new(vec![clause], span, None);
        assert_eq!(ir.clauses.len(), 1);
    }

    #[test]
    fn test_with_else() {
        let arena = Bump::new();
        let span = make_span(&arena, "@if true { } @else { }");
        let cond = Expression::Boolean(BooleanExpression::new(true, span));
        let clause = IfClause::new(cond, vec![]);
        let ir = IfRule::new(vec![clause], span, Some(vec![]));
        assert!(ir.last_clause.is_some());
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "@if true { }");
        let cond = Expression::Boolean(BooleanExpression::new(true, span));
        let clause = IfClause::new(cond, vec![]);
        let ir = IfRule::new(vec![clause], span, None);
        let s = format!("{ir}");
        assert!(s.contains("@if"), "got {s:?}");
    }
}
