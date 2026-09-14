// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/supports_condition/interpolation.dart
// go-source: go/value/sass_supports_condition_interpolation.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::core_errors::ArgumentError;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::interpolation::{Interpolation, InterpolationPart};

#[derive(Clone, Debug)]
/// An interpolated condition: a SassScript expression embedded as `#{...}`.
pub struct SupportsInterpolation<'parse> {
    /// The expression in the interpolation.
    pub expression: Expression<'parse>,
    /// The span covering the whole `#{...}` condition.
    pub span: FileSpan<'parse>,
}

impl<'parse> SupportsInterpolation<'parse> {
    pub fn new(expression: Expression<'parse>, span: FileSpan<'parse>) -> Self {
        SupportsInterpolation { expression, span }
    }

    // Wraps the embedded expression in a single-expression interpolation
    // covering this condition's span.
    pub fn to_interpolation(&self) -> SassResult<Interpolation<'parse>> {
        Interpolation::new(
            vec![InterpolationPart::Expression(Box::new(
                self.expression.clone(),
            ))],
            vec![Some(self.span)],
            self.span,
        )
        .map_err(|e: ArgumentError| {
            Box::new(SassError::Script {
                message: e.message,
                argument_name: e.name,
            })
        })
    }

    // Returns a copy of this condition with `span` as its span.
    pub fn with_span(&self, span: FileSpan<'parse>) -> Self {
        Self::new(self.expression.clone(), span)
    }
}

impl<'parse> AstNode<'parse> for SupportsInterpolation<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl SupportsInterpolation<'_> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "#{{{}}}", self.expression).unwrap();
        Ok(buf)
    }
}

impl fmt::Display for SupportsInterpolation<'_> {
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
    fn test_construction_and_display() {
        let arena = Bump::new();
        let span = test_span(&arena, "#{x}", 0, 4);
        let expr = Expression::Boolean(BooleanExpression::new(true, test_span(&arena, "x", 2, 3)));
        let cond = SupportsInterpolation::new(expr, span);
        assert_eq!(format!("{cond}"), "#{true}");
        cond.span().unwrap();
    }
}
