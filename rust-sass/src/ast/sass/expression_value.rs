// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/value.dart
// go-source: go/value/sass_expression_value.go

use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::value::{Value, ValueKind};

use crate::ast::sass::interpolation::Interpolation;

/// An expression that directly embeds a value.
///
/// This is never constructed by the parser. It's only used when ASTs are
/// constructed dynamically, as for the `call()` function.
#[derive(Clone, Debug)]
pub struct ValueExpression<'parse> {
    /// The embedded value.
    pub value: Box<Value<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> ValueExpression<'parse> {
    pub fn new(value: Value<'parse>, span: FileSpan<'parse>) -> Self {
        ValueExpression {
            value: Box::new(value),
            span,
        }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }

    pub fn to_display_string(&self) -> SassResult<String> {
        ValueKind::to_display_string(&self.value)
    }
}

impl<'parse> AstNode<'parse> for ValueExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl fmt::Display for ValueExpression<'_> {
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
        let fs = FileSource::new_in(&arena, "42", None);
        let span = FileSpan::new(Some(fs), 0, 2);
        let val = ValueKind::unitless_number(&arena, 42.0);
        let expr = ValueExpression::new(val, span);
        expr.span().unwrap();
    }

    #[test]
    fn test_string() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "42", None);
        let span = FileSpan::new(Some(fs), 0, 2);
        let val = ValueKind::unitless_number(&arena, 42.0);
        let expr = ValueExpression::new(val, span);
        assert_eq!(format!("{}", expr), "42");
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "x", None);
        let span = FileSpan::new(Some(fs), 0, 1);
        let val = ValueKind::unitless_number(&arena, 1.0);
        let expr = ValueExpression::new(val, span);
        assert!(expr.source_interpolation().is_none());
    }
}
