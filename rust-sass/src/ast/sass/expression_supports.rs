// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/supports.dart
// go-source: go/value/sass_expression_supports.go

use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::supports_condition::SupportsCondition;

/// An expression-level `@supports` condition.
///
/// This appears only in the modifiers that come after a plain-CSS `@import`.
/// It doesn't include the function name wrapping the condition.
#[derive(Clone, Debug)]
pub struct SupportsExpression<'parse> {
    /// The condition itself.
    pub condition: Box<SupportsCondition<'parse>>,
}

impl<'parse> SupportsExpression<'parse> {
    pub fn new(condition: SupportsCondition<'parse>) -> Self {
        SupportsExpression {
            condition: Box::new(condition),
        }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }
}

impl<'parse> SupportsExpression<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        Ok(format!("{}", self.condition))
    }
}

impl<'parse> AstNode<'parse> for SupportsExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        self.condition.span()
    }
}

impl fmt::Display for SupportsExpression<'_> {
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
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_boolean::BooleanExpression;
    use crate::ast::sass::supports_condition::interpolation::SupportsInterpolation;
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
        let span = test_span(&arena, "#{x}", 0, 4);
        let expr = Expression::Boolean(BooleanExpression::new(true, test_span(&arena, "x", 2, 3)));
        let cond = SupportsCondition::Interpolation(SupportsInterpolation::new(expr, span));
        let supports = SupportsExpression::new(cond);
        assert_eq!(format!("{supports}"), "#{true}");
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let span = test_span(&arena, "#{x}", 0, 4);
        let expr = Expression::Boolean(BooleanExpression::new(true, test_span(&arena, "x", 2, 3)));
        let cond = SupportsCondition::Interpolation(SupportsInterpolation::new(expr, span));
        let supports = SupportsExpression::new(cond);
        assert!(supports.source_interpolation().is_none());
    }
}
