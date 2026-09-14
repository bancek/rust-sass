// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/configured_variable.dart
// go-source: go/value/sass_configured_variable.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;

/// A variable configured by a `with` clause in a `@use` or `@forward` rule.
#[derive(Clone, Debug)]
pub struct ConfiguredVariable<'parse> {
    /// The name of the variable being configured, without the leading `$`.
    pub name: String,
    /// The variable's value.
    pub expression: Expression<'parse>,
    pub span: FileSpan<'parse>,
    /// Whether the variable can be further configured by outer modules.
    ///
    /// This is always `false` for `@use` rules.
    pub is_guarded: bool,
}

impl<'parse> ConfiguredVariable<'parse> {
    /// Creates a configured variable; `guarded` is true for `!default`-marked
    /// `@forward ... with` entries.
    pub fn new(
        name: String,
        expression: Expression<'parse>,
        span: FileSpan<'parse>,
        guarded: bool,
    ) -> Self {
        ConfiguredVariable {
            name,
            expression,
            span,
            is_guarded: guarded,
        }
    }

    /// The span of the variable name, including the leading `$`.
    pub fn name_span(&self) -> SassResult<FileSpan<'parse>> {
        self.span.initial_identifier(1).map_err(Into::into)
    }
}

impl<'parse> AstNode<'parse> for ConfiguredVariable<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> ConfiguredVariable<'parse> {
    /// Renders `$name: value`, with ` !default` appended when guarded.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(
            buf,
            "${}: {}",
            self.name,
            Expression::to_display_string(&self.expression)?
        )
        .unwrap();
        if self.is_guarded {
            write!(buf, " !default").unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for ConfiguredVariable<'parse> {
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

    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    #[test]
    fn test_new() {
        let arena = Bump::new();
        let span = make_span(&arena, "$name: true");
        let bool_expr = Expression::Boolean(BooleanExpression::new(true, span));
        let cv = ConfiguredVariable::new("name".into(), bool_expr.clone(), span, false);

        assert_eq!(cv.name, "name");
        assert!(!cv.is_guarded);
    }

    #[test]
    fn test_new_guarded() {
        let arena = Bump::new();
        let span = make_span(&arena, "$name: true !default");
        let bool_expr = Expression::Boolean(BooleanExpression::new(true, span));
        let cv = ConfiguredVariable::new("name".into(), bool_expr.clone(), span, true);

        assert!(cv.is_guarded);
    }

    #[test]
    fn test_name_span() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "$name: true", None);
        let span = FileSpan::new(Some(fs), 0, 11);
        let bool_expr =
            Expression::Boolean(BooleanExpression::new(true, FileSpan::new(Some(fs), 7, 11)));
        let cv = ConfiguredVariable::new("name".into(), bool_expr, span, false);
        let got = cv.name_span().unwrap();
        assert_eq!(got.text(), "$name");
    }

    #[test]
    fn test_display_unguarded() {
        let arena = Bump::new();
        let span = make_span(&arena, "$name: true");
        let bool_expr = Expression::Boolean(BooleanExpression::new(true, span));
        let cv = ConfiguredVariable::new("name".into(), bool_expr, span, false);
        assert_eq!(format!("{cv}"), "$name: true");
    }

    #[test]
    fn test_display_guarded() {
        let arena = Bump::new();
        let span = make_span(&arena, "$name: true !default");
        let bool_expr = Expression::Boolean(BooleanExpression::new(true, span));
        let cv = ConfiguredVariable::new("name".into(), bool_expr, span, true);
        assert_eq!(format!("{cv}"), "$name: true !default");
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "$name: true");
        let bool_expr = Expression::Boolean(BooleanExpression::new(true, span));
        let cv = ConfiguredVariable::new("name".into(), bool_expr, span, false);
        assert_eq!(cv.span().unwrap(), span);
    }
}
