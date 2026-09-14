// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/variable.dart
// go-source: go/value/sass_expression_variable.go

use crate::common::exception::SassError;
use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

/// A Sass variable reference expression, like `$name`.
#[derive(Clone, Debug)]
pub struct VariableExpression<'parse> {
    /// The name of this variable, with underscores converted to hyphens.
    pub name: String,
    pub span: FileSpan<'parse>,
    /// The namespace of the variable being referenced, or [`None`] if it's
    /// referenced without a namespace.
    pub namespace: Option<String>,
}

impl<'parse> VariableExpression<'parse> {
    pub fn new(name: String, span: FileSpan<'parse>, namespace: Option<String>) -> Self {
        VariableExpression {
            name,
            span,
            namespace,
        }
    }

    /// The span containing this reference's name, including the `$`.
    pub fn name_span(&self) -> SassResult<FileSpan<'parse>> {
        if self.namespace.is_none() {
            Ok(self.span)
        } else {
            // Matches Dart: withoutNamespace
            self.span.without_namespace().map_err(Into::into)
        }
    }

    /// The span containing this reference's namespace, or [`None`] if
    /// [`namespace`](Self::namespace) is [`None`].
    pub fn namespace_span(&self) -> SassResult<Option<FileSpan<'parse>>> {
        if self.namespace.is_none() {
            Ok(None)
        } else {
            // Matches Dart: initialIdentifier
            Ok(Some(
                self.span.initial_identifier(0).map_err(SassError::from)?,
            ))
        }
    }

    pub fn to_display_string(&self) -> SassResult<String> {
        Ok(self.span.text().to_string())
    }
}

impl<'parse> AstNode<'parse> for VariableExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl fmt::Display for VariableExpression<'_> {
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
    fn test_variable_expression_new() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "$var", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let expr = VariableExpression::new("var".into(), span, None);

        assert_eq!(expr.name, "var");
        assert!(expr.namespace.is_none());
        let got = expr.span().unwrap();
        assert_eq!(got.text(), "$var");
    }

    #[test]
    fn test_variable_expression_display() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "$var", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let expr = VariableExpression::new("var".into(), span, None);
        assert_eq!(format!("{expr}"), "$var");
    }

    #[test]
    fn test_variable_expression_display_interpolated() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "a.#{$expr}.b", None);
        let span = FileSpan::new(Some(fs), 4, 9);
        let expr = VariableExpression::new("expr".into(), span, None);
        assert_eq!(format!("{expr}"), "$expr");
    }
}
