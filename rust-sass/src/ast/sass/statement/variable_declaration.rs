// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/variable_declaration.dart
// go-source: go/value/sass_statement_variable_declaration.go

use crate::common::span_error::SpanError;
use crate::util::trim_ascii::trim_ascii_right;
use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::core_errors::ArgumentError;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::statement::silent_comment::SilentComment;

#[derive(Clone, Debug)]
/// A variable declaration.
///
/// This defines or assigns a variable.
pub struct VariableDeclaration<'parse> {
    /// The variable name, with underscores converted to hyphens.
    pub name: String,
    /// The value being assigned.
    pub expression: Expression<'parse>,
    pub span: FileSpan<'parse>,
    /// The namespace of the variable being set, if it belongs to another
    /// module.
    pub namespace: Option<String>,
    /// Whether this is a guarded assignment, which only applies when the
    /// variable is undefined or null.
    pub guarded: bool,
    /// Whether this assignment always targets the global scope.
    pub is_global: bool,
    /// The comment immediately preceding this declaration, if any.
    pub comment: Option<Box<SilentComment<'parse>>>,
}

impl<'parse> VariableDeclaration<'parse> {
    pub fn new(
        name: String,
        expression: Expression<'parse>,
        span: FileSpan<'parse>,
        namespace: Option<String>,
        guarded: bool,
        global: bool,
        comment: Option<SilentComment<'parse>>,
    ) -> SassResult<Self> {
        if namespace.is_some() && global {
            return Err(Box::new(SassError::from(ArgumentError {
                name: None,
                message: "Other modules' members can't be defined with !global.".into(),
            })));
        }
        Ok(VariableDeclaration {
            name,
            expression,
            span,
            namespace,
            guarded,
            is_global: global,
            comment: comment.map(Box::new),
        })
    }

    /// The variable name as written in the document, including the leading
    /// `$` and without underscore-to-hyphen conversion.
    ///
    /// This is relatively expensive; reserve it for error messages.
    pub fn original_name(&self) -> SassResult<String> {
        let text = self.span.text();
        let colon_idx = text.find(':').unwrap_or(text.len());
        Ok(trim_ascii_right(&text[..colon_idx], false))
    }

    pub fn name_span(&self) -> SassResult<FileSpan<'parse>> {
        let span = if self.namespace.is_some() {
            self.span.without_namespace().map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: "without_namespace failed".into(),
                    argument_name: None,
                }),
            })?
        } else {
            self.span
        };
        span.initial_identifier(1).map_err(Into::into)
    }

    pub fn namespace_span(&self) -> SassResult<Option<FileSpan<'parse>>> {
        if self.namespace.is_none() {
            return Ok(None);
        }
        let result = self
            .span
            .initial_identifier(0)
            .map_err(Into::<SassError>::into)?;
        Ok(Some(result))
    }
}

impl<'parse> AstNode<'parse> for VariableDeclaration<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> VariableDeclaration<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        if let Some(ref ns) = self.namespace {
            write!(buf, "{ns}.").unwrap();
        }
        write!(
            buf,
            "${}: {};",
            self.name,
            Expression::to_display_string(&self.expression)?
        )
        .unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for VariableDeclaration<'parse> {
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

    #[test]
    fn test_new() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "$name: true;", None);
        let span = FileSpan::new(Some(fs), 0, 11);
        let expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 7, 11)));
        let vd =
            VariableDeclaration::new("name".into(), expr, span, None, false, false, None).unwrap();
        assert_eq!(vd.name, "name");
    }

    #[test]
    fn test_name_span() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "$name: true;", None);
        let span = FileSpan::new(Some(fs), 0, 11);
        let expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 7, 11)));
        let vd =
            VariableDeclaration::new("name".into(), expr, span, None, false, false, None).unwrap();
        let got = vd.name_span().unwrap();
        assert_eq!(got.text(), "$name");
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "$name: true;", None);
        let span = FileSpan::new(Some(fs), 0, 11);
        let expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 7, 11)));
        let vd =
            VariableDeclaration::new("name".into(), expr, span, None, false, false, None).unwrap();
        assert_eq!(format!("{vd}"), "$name: true;");
    }

    #[test]
    fn test_global_namespace_error() {
        let span = FileSpan::new(None, 0, 0);
        let expr = Expression::Boolean(BooleanExpression::new(true, span));
        let result = VariableDeclaration::new(
            "name".into(),
            expr,
            span,
            Some("mod".into()),
            false,
            true,
            None,
        );
        assert!(result.is_err());
    }
}
