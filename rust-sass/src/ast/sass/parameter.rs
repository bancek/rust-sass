// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/parameter.dart
// go-source: go/value/sass_parameter.go

use crate::util::trim_ascii::trim_ascii_right;
use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;

/// A parameter declared as part of a [`ParameterList`](super::parameter_list::ParameterList).
#[derive(Clone, Debug)]
pub struct Parameter<'parse> {
    /// The parameter name, with underscores converted to hyphens and without
    /// the leading `$`.
    pub name: String,
    /// The default value of this parameter, or `None` if none was declared.
    pub default_value: Option<Expression<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> Parameter<'parse> {
    /// Creates a parameter with the given normalized `name` and optional
    /// default value.
    pub fn new(
        name: String,
        span: FileSpan<'parse>,
        default_value: Option<Expression<'parse>>,
    ) -> Self {
        Parameter {
            name,
            default_value,
            span,
        }
    }

    /// The span of the parameter name: the whole span when there is no
    /// default, otherwise just the leading `$name`.
    pub fn name_span(&self) -> SassResult<FileSpan<'parse>> {
        if self.default_value.is_none() {
            return Ok(self.span);
        }
        self.span.initial_identifier(1).map_err(Into::into)
    }

    /// Returns the variable name as written in the document, without underscores
    /// converted to hyphens and including the leading `$`.
    ///
    /// This isn't particularly efficient, and should only be used for error
    /// messages.
    ///
    /// Matches Dart: Parameter.originalName
    pub fn original_name(&self) -> String {
        if self.default_value.is_none() {
            return self.span.text().to_string();
        }
        declaration_name(&self.span)
    }
}

/// Returns the variable name (including the leading `$`) from a span that
/// covers a variable declaration, which includes the variable name as well as
/// the colon and expression following it.
///
/// Matches Dart: utils.dart#declarationName
fn declaration_name(span: &FileSpan<'_>) -> String {
    let text = span.text();
    let colon_idx = text.find(':').unwrap_or(text.len());
    trim_ascii_right(&text[..colon_idx], false)
}

impl<'parse> AstNode<'parse> for Parameter<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> Parameter<'parse> {
    /// Renders `name` or `name: default`, without the leading `$`.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        if let Some(ref default) = self.default_value {
            write!(
                buf,
                "{}: {}",
                self.name,
                Expression::to_display_string(default)?
            )
            .unwrap();
        } else {
            write!(buf, "{}", self.name).unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for Parameter<'parse> {
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

    fn make_span<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
        start: usize,
        end: usize,
    ) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), start, end)
    }

    #[test]
    fn test_new_without_default() {
        let arena = Bump::new();
        let span = make_span(&arena, "$name", 0, 5);
        let p = Parameter::new("name".into(), span, None);
        assert_eq!(p.name, "name");
        assert!(p.default_value.is_none());
    }

    #[test]
    fn test_new_with_default() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "$name: true", None);
        let span = FileSpan::new(Some(fs), 0, 11);
        let bool_expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 7, 11)));
        let p = Parameter::new("name".into(), span, Some(bool_expr));
        assert_eq!(p.name, "name");
        assert!(p.default_value.is_some());
    }

    #[test]
    fn test_name_span_no_default() {
        let arena = Bump::new();
        let span = make_span(&arena, "$name", 0, 5);
        let p = Parameter::new("name".into(), span, None);
        let got = p.name_span().unwrap();
        assert_eq!(got.text(), "$name");
    }

    #[test]
    fn test_name_span_with_default() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "$name: true", None);
        let span = FileSpan::new(Some(fs), 0, 11);
        let bool_expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 7, 11)));
        let p = Parameter::new("name".into(), span, Some(bool_expr));
        let got = p.name_span().unwrap();
        assert_eq!(got.text(), "$name");
    }

    #[test]
    fn test_original_name_no_default() {
        let arena = Bump::new();
        let span = make_span(&arena, "$name", 0, 5);
        let p = Parameter::new("name".into(), span, None);
        assert_eq!(p.original_name(), "$name");
    }

    #[test]
    fn test_original_name_with_default() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "$name: true", None);
        let span = FileSpan::new(Some(fs), 0, 11);
        let bool_expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 7, 11)));
        let p = Parameter::new("name".into(), span, Some(bool_expr));
        assert_eq!(p.original_name(), "$name");
    }

    #[test]
    fn test_display_no_default() {
        let arena = Bump::new();
        let span = make_span(&arena, "$name", 0, 5);
        let p = Parameter::new("name".into(), span, None);
        assert_eq!(format!("{p}"), "name");
    }

    #[test]
    fn test_display_with_default() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "$name: true", None);
        let span = FileSpan::new(Some(fs), 0, 11);
        let bool_expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 7, 11)));
        let p = Parameter::new("name".into(), span, Some(bool_expr));
        assert_eq!(format!("{p}"), "name: true");
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "$name", 0, 5);
        let p = Parameter::new("name".into(), span, None);
        assert_eq!(p.span().unwrap(), span);
    }
}
