// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/function.dart
// go-source: go/value/sass_expression_function.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::interpolation::Interpolation;

/// A function invocation.
///
/// This may be a plain CSS function or a Sass function, but may not include
/// interpolation.
#[derive(Clone, Debug)]
pub struct FunctionExpression<'parse> {
    /// The namespace of the function being invoked, or [`None`] if it's
    /// invoked without a namespace.
    pub namespace: Option<String>,
    /// The name of the function being invoked, with underscores converted to
    /// hyphens.
    ///
    /// If this function is a plain CSS function, use
    /// [`original_name`](Self::original_name) instead.
    pub name: String,
    /// The name of the function being invoked, with underscores left as-is.
    pub original_name: String,
    /// The arguments to pass to the function.
    pub arguments: ArgumentList<'parse>,
    pub span: FileSpan<'parse>,
}

impl<'parse> FunctionExpression<'parse> {
    pub fn new(
        original_name: String,
        arguments: ArgumentList<'parse>,
        span: FileSpan<'parse>,
        namespace: Option<String>,
    ) -> Self {
        let name = original_name.replace('_', "-");
        FunctionExpression {
            namespace,
            name,
            original_name,
            arguments,
            span,
        }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }

    /// The span containing this invocation's name.
    pub fn name_span(&self) -> SassResult<FileSpan<'parse>> {
        if self.namespace.is_none() {
            self.span.initial_identifier(0).map_err(Into::into)
        } else {
            let without_ns = self.span.without_namespace()?;
            without_ns.initial_identifier(0).map_err(Into::into)
        }
    }

    /// The span containing this invocation's namespace, or [`None`] if
    /// [`namespace`](Self::namespace) is [`None`].
    pub fn namespace_span(&self) -> SassResult<Option<FileSpan<'parse>>> {
        if self.namespace.is_none() {
            Ok(None)
        } else {
            self.span
                .initial_identifier(0)
                .map(Some)
                .map_err(Into::into)
        }
    }
}

impl<'parse> AstNode<'parse> for FunctionExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> FunctionExpression<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        if let Some(ref ns) = self.namespace {
            write!(buf, "{ns}.").unwrap();
        }
        write!(buf, "{}", self.original_name).unwrap();
        write!(buf, "{}", self.arguments).unwrap();
        Ok(buf)
    }
}

impl fmt::Display for FunctionExpression<'_> {
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
        let fs = FileSource::new_in(&arena, "fn()", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let expr = FunctionExpression::new("fn".into(), ArgumentList::empty(span), span, None);
        assert_eq!(expr.original_name, "fn");
        assert_eq!(expr.name, "fn");
    }

    #[test]
    fn test_underscore_normalization() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "fn_name()", None);
        let span = FileSpan::new(Some(fs), 0, 10);
        let expr = FunctionExpression::new("fn_name".into(), ArgumentList::empty(span), span, None);
        assert_eq!(expr.name, "fn-name");
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "fn()", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let expr = FunctionExpression::new("fn".into(), ArgumentList::empty(span), span, None);
        assert!(expr.source_interpolation().is_none());
    }

    #[test]
    fn test_string() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "fn()", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let expr = FunctionExpression::new("fn".into(), ArgumentList::empty(span), span, None);
        assert_eq!(format!("{expr}"), "fn()");
    }
}
