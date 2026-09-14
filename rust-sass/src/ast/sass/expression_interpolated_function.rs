// Copyright 2021 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/interpolated_function.dart
// go-source: go/value/sass_expression_interpolated_function.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::interpolation::Interpolation;

/// An interpolated function invocation.
///
/// This is always a plain CSS function.
#[derive(Clone, Debug)]
pub struct InterpolatedFunctionExpression<'parse> {
    /// The name of the function being invoked.
    pub name: Interpolation<'parse>,
    /// The arguments to pass to the function.
    pub arguments: ArgumentList<'parse>,
    pub span: FileSpan<'parse>,
}

impl<'parse> InterpolatedFunctionExpression<'parse> {
    pub fn new(
        name: Interpolation<'parse>,
        arguments: ArgumentList<'parse>,
        span: FileSpan<'parse>,
    ) -> Self {
        InterpolatedFunctionExpression {
            name,
            arguments,
            span,
        }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }
}

impl<'parse> AstNode<'parse> for InterpolatedFunctionExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> InterpolatedFunctionExpression<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "{}{}", self.name, self.arguments).unwrap();
        Ok(buf)
    }
}

impl fmt::Display for InterpolatedFunctionExpression<'_> {
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
        let fs = FileSource::new_in(&arena, "#{$fn}()", None);
        let span = FileSpan::new(Some(fs), 0, 8);
        let name = Interpolation::plain("fn".into(), span);
        let expr = InterpolatedFunctionExpression::new(name, ArgumentList::empty(span), span);
        expr.span().unwrap();
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "x()", None);
        let span = FileSpan::new(Some(fs), 0, 3);
        let name = Interpolation::plain("x".into(), span);
        let expr = InterpolatedFunctionExpression::new(name, ArgumentList::empty(span), span);
        assert!(expr.source_interpolation().is_none());
    }

    #[test]
    fn test_string() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "fn()", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let name = Interpolation::plain("fn".into(), span);
        let expr = InterpolatedFunctionExpression::new(name, ArgumentList::empty(span), span);
        assert_eq!(format!("{expr}"), "fn()");
    }
}
