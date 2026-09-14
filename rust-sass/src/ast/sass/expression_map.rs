// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/map.dart
// go-source: go/value/sass_expression_map.go

use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::interpolation::Interpolation;

/// A map literal.
#[derive(Clone, Debug)]
pub struct MapExpression<'parse> {
    /// The pairs in this map.
    ///
    /// This is a list of pairs rather than a map because a map may have two
    /// keys with the same expression (e.g. `(unique-id(): 1, unique-id(): 2)`).
    pub pairs: Vec<(Expression<'parse>, Expression<'parse>)>,
    pub span: FileSpan<'parse>,
}

impl<'parse> MapExpression<'parse> {
    pub fn new(
        pairs: Vec<(Expression<'parse>, Expression<'parse>)>,
        span: FileSpan<'parse>,
    ) -> Self {
        MapExpression { pairs, span }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }
}

impl<'parse> MapExpression<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut parts = Vec::new();
        for (k, v) in &self.pairs {
            parts.push(format!(
                "{}: {}",
                k.to_display_string()?,
                v.to_display_string()?
            ));
        }
        Ok(format!("({})", parts.join(", ")))
    }
}

impl<'parse> AstNode<'parse> for MapExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for MapExpression<'parse> {
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
        let span = test_span(&arena, "(a: b)", 0, 6);
        let pairs = vec![(
            Expression::String(StringExpression::new(
                Interpolation::plain("a".into(), span),
                false,
            )),
            Expression::String(StringExpression::new(
                Interpolation::plain("b".into(), span),
                false,
            )),
        )];
        let expr = MapExpression::new(pairs, span);
        assert_eq!(expr.pairs.len(), 1);
    }

    #[test]
    fn test_string() {
        let arena = Bump::new();
        let span = test_span(&arena, "(a: b)", 0, 6);
        let pairs = vec![(
            Expression::String(StringExpression::new(
                Interpolation::plain("a".into(), span),
                false,
            )),
            Expression::String(StringExpression::new(
                Interpolation::plain("b".into(), span),
                false,
            )),
        )];
        let expr = MapExpression::new(pairs, span);
        assert_eq!(format!("{expr}"), "(a: b)");
    }
}
