// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/null.dart
// go-source: go/value/sass_expression_null.go

use crate::ast::sass::interpolation::Interpolation;
use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

/// A null literal expression.
#[derive(Clone, Debug)]
pub struct NullExpression<'parse> {
    pub span: FileSpan<'parse>,
}

impl<'parse> NullExpression<'parse> {
    pub fn new(span: FileSpan<'parse>) -> Self {
        NullExpression { span }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }

    pub fn to_display_string(&self) -> SassResult<String> {
        Ok("null".to_string())
    }
}

impl<'parse> AstNode<'parse> for NullExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl fmt::Display for NullExpression<'_> {
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
        let fs = FileSource::new_in(&arena, "null", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let expr = NullExpression::new(span);
        expr.span().unwrap();
    }

    #[test]
    fn test_string() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "null", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let expr = NullExpression::new(span);
        assert_eq!(format!("{expr}"), "null");
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "null", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let expr = NullExpression::new(span);
        assert!(expr.source_interpolation().is_none());
    }
}
