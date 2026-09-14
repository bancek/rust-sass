// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/declaration.dart
// go-source: go/value/sass_statement_declaration.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression_string::StringExpression;
use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::statement::Statement;

/// A declaration (that is, a `name: value` pair).
#[derive(Clone, Debug)]
pub struct Declaration<'parse> {
    pub children: Option<Vec<Statement<'parse>>>,
    /// The name of this declaration.
    pub name: Interpolation<'parse>,
    /// The value of this declaration.
    ///
    /// Always present when [`Declaration::children`] is `None`; may or may
    /// not be present on a nested declaration.
    pub value: Option<Expression<'parse>>,
    // Whether this declaration's value was parsed as SassScript.
    //
    // This is `false` for custom properties as well as the `result` property
    // of the plain-CSS `@function` rule. Note that this can be `true` for
    // declarations that will ultimately serialize as custom properties if
    // they weren't *parsed as* custom properties, such as `#{--foo}: ...`.
    //
    // When `false`, `value` is an unquoted `StringExpression`.
    pub parsed_as_sass_script: bool,
    pub span: FileSpan<'parse>,
}

impl<'parse> Declaration<'parse> {
    /// Creates a declaration with no children.
    pub fn new(
        name: Interpolation<'parse>,
        value: Expression<'parse>,
        span: FileSpan<'parse>,
    ) -> Self {
        Declaration {
            children: None,
            name,
            value: Some(value),
            parsed_as_sass_script: true,
            span,
        }
    }

    /// Creates a declaration with no children whose value is not parsed as
    /// SassScript.
    pub fn not_sass_script(
        name: Interpolation<'parse>,
        value: StringExpression<'parse>,
        span: FileSpan<'parse>,
    ) -> Self {
        Declaration {
            children: None,
            name,
            value: Some(Expression::String(value)),
            parsed_as_sass_script: false,
            span,
        }
    }

    /// Creates a declaration with children.
    ///
    /// For these declarations, a value is optional.
    pub fn nested(
        name: Interpolation<'parse>,
        children: Vec<Statement<'parse>>,
        span: FileSpan<'parse>,
        value: Option<Expression<'parse>>,
    ) -> Self {
        Declaration {
            children: Some(children),
            name,
            value,
            parsed_as_sass_script: true,
            span,
        }
    }
}

impl<'parse> AstNode<'parse> for Declaration<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> Declaration<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "{}:", self.name).unwrap();
        if let Some(ref value) = self.value {
            if self.parsed_as_sass_script {
                write!(buf, " ").unwrap();
            }
            write!(buf, "{}", Expression::to_display_string(value)?).unwrap();
        }
        match &self.children {
            None => write!(buf, ";").unwrap(),
            Some(children) => {
                write!(buf, " {{").unwrap();
                for child in children {
                    write!(buf, " {child}").unwrap();
                }
                write!(buf, " }}").unwrap();
            }
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for Declaration<'parse> {
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

    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    fn make_interp<'compile, 'parse>(arena: &'compile Bump, text: &str) -> Interpolation<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let span = make_span(arena, text);
        Interpolation::plain(text.into(), span)
    }

    #[test]
    fn test_new() {
        let arena = Bump::new();
        let span = make_span(&arena, "color: red;");
        let name = make_interp(&arena, "color");
        let val = Expression::String(StringExpression::plain(
            "red",
            FileSpan::new(None, 0, 0),
            true,
        ));
        let d = Declaration::new(name, val, span);
        assert!(d.parsed_as_sass_script);
        assert!(d.children.is_none());
    }

    #[test]
    fn test_not_sass_script() {
        let arena = Bump::new();
        let span = make_span(&arena, "color: red;");
        let name = make_interp(&arena, "color");
        let val = StringExpression::plain("red", FileSpan::new(None, 0, 0), true);
        let d = Declaration::not_sass_script(name, val, span);
        assert!(!d.parsed_as_sass_script);
    }

    #[test]
    fn test_nested() {
        let arena = Bump::new();
        let span = make_span(&arena, "color: red { }");
        let name = make_interp(&arena, "color");
        let d = Declaration::nested(name, vec![], span, None);
        assert!(d.children.is_some());
    }

    #[test]
    fn test_display_simple() {
        let arena = Bump::new();
        let span = make_span(&arena, "color: red;");
        let name = make_interp(&arena, "color");
        let val = Expression::String(StringExpression::plain(
            "red",
            FileSpan::new(None, 0, 0),
            true,
        ));
        let d = Declaration::new(name, val, span);
        let s = format!("{d}");
        assert!(s.ends_with(';'), "got {s:?}");
    }
}
