// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/function_rule.dart
// go-source: go/value/sass_statement_function_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::parameter_list::ParameterList;
use crate::ast::sass::statement::silent_comment::SilentComment;
use crate::ast::sass::statement::Statement;

/// A function declaration.
///
/// Declares a function that's invoked using normal CSS function syntax.
#[derive(Clone, Debug)]
pub struct FunctionRule<'parse> {
    /// The function name, with underscores converted to hyphens.
    pub name: String,
    /// The function name as written, without underscore-to-hyphen conversion.
    pub original_name: String,
    /// The parameters the function accepts.
    pub parameters: ParameterList<'parse>,
    pub children: Vec<Statement<'parse>>,
    pub span: FileSpan<'parse>,
    /// The comment immediately preceding this declaration, if any.
    pub comment: Option<Box<SilentComment<'parse>>>,
    pub is_plain_css_function: bool,
    pub namespace: Option<String>,
}

impl<'parse> FunctionRule<'parse> {
    pub fn new(
        original_name: String,
        parameters: ParameterList<'parse>,
        children: Vec<Statement<'parse>>,
        span: FileSpan<'parse>,
        comment: Option<SilentComment<'parse>>,
    ) -> Self {
        let name = original_name.replace('_', "-");
        FunctionRule {
            name,
            original_name,
            parameters,
            children,
            span,
            comment: comment.map(Box::new),
            is_plain_css_function: false,
            namespace: None,
        }
    }

    /// The span covering this function's name within its `@function` header.
    pub fn name_span(&self) -> SassResult<FileSpan<'parse>> {
        self.span
            .without_initial_at_rule()
            .and_then(|s| s.initial_identifier(0))
            .map_err(Into::into)
    }
}

impl<'parse> AstNode<'parse> for FunctionRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> FunctionRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "@function {}(", self.name).unwrap();
        write!(buf, "{}", self.parameters).unwrap();
        write!(buf, ") {{").unwrap();
        for child in &self.children {
            write!(buf, " {child}").unwrap();
        }
        write!(buf, " }}").unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for FunctionRule<'parse> {
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

    #[test]
    fn test_new() {
        let arena = Bump::new();
        let span = make_span(&arena, "@function foo() { }");
        let params = ParameterList::empty(span);
        let fr = FunctionRule::new("foo".into(), params, vec![], span, None);
        assert_eq!(fr.name, "foo");
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "@function foo() { }");
        let params = ParameterList::empty(span);
        let fr = FunctionRule::new("foo".into(), params, vec![], span, None);
        let s = format!("{fr}");
        assert!(s.contains("@function"), "got {s:?}");
    }
}
