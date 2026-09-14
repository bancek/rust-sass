// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/number.dart
// go-source: go/value/sass_expression_number.go

use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::value::SassNumber;

use crate::ast::sass::interpolation::Interpolation;

/// A number literal.
#[derive(Clone, Debug)]
pub struct NumberExpression<'parse> {
    /// The numeric value.
    pub value: f64,
    /// The number's unit, or [`None`] if it is unitless.
    pub unit: Option<String>,
    pub span: FileSpan<'parse>,
}

impl<'parse> NumberExpression<'parse> {
    pub fn new(value: f64, span: FileSpan<'parse>, unit: Option<String>) -> Self {
        NumberExpression { value, unit, span }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }

    pub fn to_display_string(&self) -> SassResult<String> {
        let num = SassNumber::new(self.value, self.unit.as_deref());
        num.to_display_string()
    }
}

impl<'parse> AstNode<'parse> for NumberExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl fmt::Display for NumberExpression<'_> {
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
        let fs = FileSource::new_in(&arena, "42px", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let expr = NumberExpression::new(42.0, span, Some("px".into()));
        assert_eq!(expr.value, 42.0);
        assert_eq!(expr.unit.as_deref(), Some("px"));
    }

    #[test]
    fn test_string() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "42", None);
        let span = FileSpan::new(Some(fs), 0, 2);
        let expr = NumberExpression::new(42.0, span, None);
        assert_eq!(format!("{}", expr), "42");
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "42", None);
        let span = FileSpan::new(Some(fs), 0, 2);
        let expr = NumberExpression::new(42.0, span, None);
        assert!(expr.source_interpolation().is_none());
    }
}
