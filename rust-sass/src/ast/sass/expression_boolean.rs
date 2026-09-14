// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/boolean.dart
// go-source: go/value/sass_expression_boolean.go

use crate::ast::sass::interpolation::Interpolation;
use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

/// A boolean literal expression, `true` or `false`.
#[derive(Clone, Debug)]
pub struct BooleanExpression<'parse> {
    /// The value of this expression.
    pub value: bool,
    pub span: FileSpan<'parse>,
}

impl<'parse> BooleanExpression<'parse> {
    pub fn new(value: bool, span: FileSpan<'parse>) -> Self {
        BooleanExpression { value, span }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }

    pub fn to_display_string(&self) -> SassResult<String> {
        Ok(self.value.to_string())
    }
}

impl<'parse> AstNode<'parse> for BooleanExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl fmt::Display for BooleanExpression<'_> {
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
        let fs = FileSource::new_in(&arena, "true", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let expr = BooleanExpression::new(true, span);
        expr.span().unwrap();
    }

    #[test]
    fn test_true_string() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "true", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let expr = BooleanExpression::new(true, span);
        assert_eq!(format!("{expr}"), "true");
    }

    #[test]
    fn test_false_string() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "false", None);
        let span = FileSpan::new(Some(fs), 0, 5);
        let expr = BooleanExpression::new(false, span);
        assert_eq!(format!("{expr}"), "false");
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "true", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let expr = BooleanExpression::new(true, span);
        assert!(expr.source_interpolation().is_none());
    }
}
