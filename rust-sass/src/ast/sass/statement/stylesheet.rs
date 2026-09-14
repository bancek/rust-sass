// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/stylesheet.dart
// go-source: go/value/sass_statement_stylesheet.go

use crate::parse::stylesheet::CssState;
use crate::parse::stylesheet::SassIndentState;
use crate::parse::stylesheet::StylesheetParser;
use crate::parse::stylesheet::Syntax;
use crate::parse::stylesheet_parse::parse_impl;
use std::collections::HashSet;
use std::fmt;
use std::fmt::Write;

use bumpalo::Bump;
use indexmap::IndexMap;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::common::source_span_file_source::FileSource;
use crate::deprecation::Deprecation;
use crate::url::SassUrl;

use crate::ast::sass::statement::forward_rule::ForwardRule;
use crate::ast::sass::statement::use_rule::UseRule;
use crate::ast::sass::statement::Statement;

#[derive(Clone, Debug)]
/// A warning discovered while parsing a stylesheet, deferred until evaluation
/// can emit it through a logger.
pub struct ParseTimeWarning<'parse> {
    pub deprecation: Option<&'static Deprecation>,
    pub message: String,
    pub span: FileSpan<'parse>,
    /// Extra primary label carried beyond Dart's record, if any.
    pub primary_label: Option<String>,
    /// Extra secondary labels carried beyond Dart's record, if any.
    pub secondary: Vec<(FileSpan<'parse>, String)>,
}

#[derive(Clone, Debug)]
/// A Sass stylesheet.
///
/// This is the root Sass node. It contains top-level statements.
pub struct Stylesheet<'parse> {
    pub children: Vec<Statement<'parse>>,
    pub span: FileSpan<'parse>,
    // Whether this was parsed from a plain CSS stylesheet.
    pub plain_css: bool,
    /// All the `@use` rules that appear at the top of this stylesheet.
    pub uses: Vec<UseRule<'parse>>,
    /// All the `@forward` rules that appear at the top of this stylesheet.
    pub forwards: Vec<ForwardRule<'parse>>,
    // Warnings discovered while parsing, emitted during evaluation once a
    // logger is available.
    pub parse_time_warnings: Vec<ParseTimeWarning<'parse>>,
    // Normalized global variable names defined by this stylesheet, mapped to
    // the spans where they're defined.
    pub global_variables: IndexMap<String, FileSpan<'parse>>,
}

impl<'parse> Stylesheet<'parse> {
    /// Creates a stylesheet holding `children`.
    pub fn new(children: Vec<Statement<'parse>>, span: FileSpan<'parse>) -> Self {
        Stylesheet::detailed(children, span, vec![], false, IndexMap::new())
    }

    // Internal constructor that also sets warnings, the plain-CSS flag, and
    // the global-variable table.
    pub fn detailed(
        children: Vec<Statement<'parse>>,
        span: FileSpan<'parse>,
        warnings: Vec<ParseTimeWarning<'parse>>,
        plain_css: bool,
        global_variables: IndexMap<String, FileSpan<'parse>>,
    ) -> Self {
        let mut uses = Vec::new();
        let mut forwards = Vec::new();
        for child in &children {
            match child {
                Statement::UseRule(u) => uses.push(u.clone()),
                Statement::ForwardRule(f) => forwards.push(f.clone()),
                Statement::SilentComment(_)
                | Statement::LoudComment(_)
                | Statement::VariableDeclaration(_) => continue,
                _ => break,
            }
        }
        Stylesheet {
            children,
            span,
            plain_css,
            uses,
            forwards,
            parse_time_warnings: warnings,
            global_variables,
        }
    }

    /// Returns the `@use` rules at the top of this stylesheet.
    pub fn uses(&self) -> &[UseRule<'parse>] {
        &self.uses
    }

    /// Returns the `@forward` rules at the top of this stylesheet.
    pub fn forwards(&self) -> &[ForwardRule<'parse>] {
        &self.forwards
    }

    /// Parses an SCSS stylesheet from `contents`.
    ///
    /// `url` names the file `contents` comes from, when known. When
    /// `parse_selectors` holds, style rules carry parsed selectors rather
    /// than raw interpolations. Returns an error if parsing fails.
    pub fn parse_scss<'compile: 'parse>(
        contents: &str,
        url: Option<&SassUrl>,
        parse_selectors: bool,
        arena: &'compile Bump,
    ) -> SassResult<Self> {
        let source = FileSource::new_in(arena, contents, url.cloned());
        let mut parser = StylesheetParser::new(source, Syntax::Scss, None);
        parser.state.parse_selectors = parse_selectors;
        parse_impl(&mut parser.scanner, &mut parser.state).map_err(Into::into)
    }

    /// Parses an indented-syntax stylesheet from `contents`.
    ///
    /// `url` names the file `contents` comes from, when known. When
    /// `parse_selectors` holds, style rules carry parsed selectors rather
    /// than raw interpolations. Returns an error if parsing fails.
    pub fn parse_sass<'compile: 'parse>(
        contents: &str,
        url: Option<&SassUrl>,
        parse_selectors: bool,
        arena: &'compile Bump,
    ) -> SassResult<Self> {
        let source = FileSource::new_in(arena, contents, url.cloned());
        let mut parser = StylesheetParser::new(
            source,
            Syntax::Sass(SassIndentState {
                current_indentation: 0,
                next_indentation: None,
                next_indentation_end: None,
                indent_spaces: None,
            }),
            None,
        );
        parser.state.parse_selectors = parse_selectors;
        parse_impl(&mut parser.scanner, &mut parser.state).map_err(Into::into)
    }

    /// Parses a plain CSS stylesheet from `contents`.
    ///
    /// `url` names the file `contents` comes from, when known. When
    /// `parse_selectors` holds, style rules carry parsed selectors rather
    /// than raw interpolations. Returns an error if parsing fails.
    pub fn parse_css<'compile: 'parse>(
        contents: &str,
        url: Option<&SassUrl>,
        parse_selectors: bool,
        disallowed_function_names: &HashSet<String>,
        arena: &'compile Bump,
    ) -> SassResult<Self> {
        let source = FileSource::new_in(arena, contents, url.cloned());
        let mut parser = StylesheetParser::new(
            source,
            Syntax::Css(CssState {
                disallowed_function_names: disallowed_function_names.clone(),
            }),
            None,
        );
        parser.state.parse_selectors = parse_selectors;
        parse_impl(&mut parser.scanner, &mut parser.state).map_err(Into::into)
    }
}

impl<'parse> AstNode<'parse> for Stylesheet<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> Stylesheet<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        for (i, child) in self.children.iter().enumerate() {
            if i > 0 {
                write!(buf, " ").unwrap();
            }
            write!(buf, "{child}").unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for Stylesheet<'parse> {
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
        let span = make_span(&arena, "a { }");
        let ss = Stylesheet::new(vec![], span);
        assert!(!ss.plain_css);
    }

    #[test]
    fn test_detailed() {
        let arena = Bump::new();
        let span = make_span(&arena, "a { }");
        let gv: IndexMap<String, FileSpan> = IndexMap::new();
        let ss = Stylesheet::detailed(vec![], span, vec![], true, gv);
        assert!(ss.plain_css);
    }

    #[test]
    fn test_warnings() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let warnings = vec![ParseTimeWarning {
            deprecation: None,
            message: "test warning".into(),
            span,
            primary_label: None,
            secondary: vec![],
        }];
        let gv: IndexMap<String, FileSpan> = IndexMap::new();
        let ss = Stylesheet::detailed(vec![], span, warnings, false, gv);
        assert_eq!(ss.parse_time_warnings.len(), 1);
        assert_eq!(ss.parse_time_warnings[0].message, "test warning");
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "a { }");
        let ss = Stylesheet::new(vec![], span);
        assert_eq!(ss.span().unwrap(), span);
    }

    #[test]
    fn test_display_empty() {
        let arena = Bump::new();
        let span = make_span(&arena, "");
        let ss = Stylesheet::new(vec![], span);
        assert_eq!(format!("{ss}"), "");
    }

    #[test]
    fn test_parse_scss_basic() {
        let arena = Bump::new();
        let ss = Stylesheet::parse_scss(".foo { color: red }", None, false, &arena).unwrap();
        assert_eq!(ss.children.len(), 1);
    }

    #[test]
    fn test_parse_scss_with_semicolon() {
        let arena = Bump::new();
        let ss = Stylesheet::parse_scss(".foo { color: red; }", None, false, &arena).unwrap();
        assert_eq!(ss.children.len(), 1);
    }

    #[test]
    fn test_parse_scss_trailing_newline() {
        let arena = Bump::new();
        let ss = Stylesheet::parse_scss(".foo { color: red }\n", None, false, &arena).unwrap();
        assert_eq!(ss.children.len(), 1);
    }

    #[test]
    fn test_parse_scss_semicolon_trailing_newline() {
        let arena = Bump::new();
        let ss = Stylesheet::parse_scss(".foo { color: red; }\n", None, false, &arena).unwrap();
        assert_eq!(ss.children.len(), 1);
    }
}
