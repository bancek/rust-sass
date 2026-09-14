// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/color.dart
// go-source: go/value/sass_expression_color.go

use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::value::color::SassColor;

use crate::ast::sass::interpolation::Interpolation;

/// A color literal.
#[derive(Clone, Debug)]
pub struct ColorExpression<'parse> {
    /// The value of this color.
    pub value: Box<SassColor>,
    pub span: FileSpan<'parse>,
}

impl<'parse> ColorExpression<'parse> {
    pub fn new(value: SassColor, span: FileSpan<'parse>) -> Self {
        ColorExpression {
            value: Box::new(value),
            span,
        }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }

    pub fn to_display_string(&self) -> SassResult<String> {
        SassColor::to_display_string(&self.value)
    }
}

impl<'parse> AstNode<'parse> for ColorExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl fmt::Display for ColorExpression<'_> {
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
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    #[test]
    fn test_construction() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "#f00", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let color = SassColor::rgb(1.0, 0.0, 0.0, 1.0);
        let expr = ColorExpression::new(color, span);
        expr.span().unwrap();
    }

    #[test]
    fn test_string() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "#f00", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let color = SassColor::rgb(1.0, 0.0, 0.0, 1.0);
        let expr = ColorExpression::new(color, span);
        let s = format!("{}", expr);
        assert_eq!(s, "#010000");
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "red", None);
        let span = FileSpan::new(Some(fs), 0, 3);
        let color = SassColor::rgb(1.0, 0.0, 0.0, 1.0);
        let expr = ColorExpression::new(color, span);
        assert!(expr.source_interpolation().is_none());
    }
}
