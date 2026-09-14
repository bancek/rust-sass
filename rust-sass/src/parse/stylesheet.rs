// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/stylesheet.dart (StylesheetParser state fields,
//   abstract syntax accessors; concrete statement/at-rule logic lives in
//   stylesheet_parse.rs and atrule.rs)
// go-source: go/value/parse_stylesheet.go
//
// Matches Dart: `abstract class StylesheetParser extends Parser`
// (stylesheet.dart:38). Dart spreads the three syntaxes over subclasses
// (`ScssParser`, `SassParser`, `CssParser` in scss.dart/sass.dart/css.dart);
// Rust flattens that hierarchy into the [`Syntax`] enum below plus free
// functions that `match` on it, so a change here applies to all syntaxes
// unless it sits behind a syntax arm. Statement-level productions
// (`parse`, `_statement`, `_styleRule`, …) live in
// stylesheet_parse.rs; the `@`-rule productions (`atRule`, `_useRule`, …)
// live in atrule.rs.

use crate::ast::sass::interpolation_map::InterpolationMap;
use crate::parse::sass::sass_children_impl;
use crate::parse::sass::sass_expect_statement_separator_impl;
use crate::parse::sass::sass_scan_else_impl;
use crate::parse::sass::sass_statements_impl;
use crate::parse::sass::sass_style_rule_selector_impl;
use crate::parse::scss::scss_children_impl;
use crate::parse::scss::scss_expect_statement_separator_impl;
use crate::parse::scss::scss_scan_else_impl;
use crate::parse::scss::scss_statements_impl;
use crate::parse::scss::scss_style_rule_selector_impl;
use crate::util::character::is_newline;
use std::collections::HashMap;
use std::collections::HashSet;

use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::statement::silent_comment::SilentComment;
use crate::ast::sass::statement::stylesheet::ParseTimeWarning;
use crate::ast::sass::statement::Statement;
use crate::common::file_span::FileSpan;
use crate::common::source_span_file_source::FileSource;
use crate::common::span_scanner::{LineScannerState, SpanScanner};

use crate::parse::parser::error_impl;
use crate::parse::parser::ParseResult;
use crate::parse::parser::ParserState;

// ======================================================================
// Syntax enum + variant state types
// ======================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Syntax {
    /// The SCSS syntax. The default; concrete behavior lives in scss.rs.
    Scss,
    /// The indented Sass syntax, with its indentation-tracking state.
    Sass(SassIndentState),
    /// Plain CSS, with the set of function names rejected at parse time.
    Css(CssState),
}

/// Indentation-tracking state for the indented Sass syntax.
///
/// Matches Dart: the `_currentIndentation` / `_nextIndentation` /
/// `_nextIndentationEnd` lookahead cache on `SassParser` (sass.dart).
/// The next-indent cache exists so `looking_at_children` can peek the
/// upcoming indentation without consuming it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SassIndentState {
    /// The indentation level of the current block.
    pub current_indentation: usize,
    /// Peeked indentation of the upcoming line, if already scanned.
    pub next_indentation: Option<usize>,
    /// Scanner state at the end of the peeked indentation, for rewinding.
    pub next_indentation_end: Option<LineScannerState>,
    /// Whether the document indents with spaces (`false` = tabs); `None`
    /// before the first indented line is seen.
    pub indent_spaces: Option<bool>,
}

/// State for plain-CSS parsing.
///
/// Matches Dart: the disallowed-function set on `CssParser` (css.dart),
/// derived from the global function names — those functions are parsed as
/// plain CSS functions instead of Sass expressions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssState {
    /// Names that may not be parsed as Sass function calls in plain CSS.
    pub disallowed_function_names: HashSet<String>,
}

// ======================================================================
// StylesheetState + StylesheetParser
// ======================================================================

/// Parser state for stylesheet-level parsing.
///
/// Matches Dart: the private `_`-prefixed instance fields on
/// `StylesheetParser` (stylesheet.dart:41-91). Nests `ParserState` so that
/// Parser-level free functions (`whitespace_impl`, `span_from_impl`, etc.)
/// can be called directly via `&mut state.parser_state`.
pub struct StylesheetState<'parse> {
    /// Shared scanner-level state: syntax, interpolation map, expression flag.
    pub parser_state: ParserState<'parse>,
    /// Whether to parse style-rule selectors as resolved selectors rather
    /// than raw interpolation. Matches Dart: `_parseSelectors`.
    pub parse_selectors: bool,
    /// Whether `@use`/`@forward` are still allowed here. Starts `true` and
    /// is cleared by the first other rule. Matches Dart: `_isUseAllowed`.
    pub is_use_allowed: bool,
    /// Whether currently inside a `@mixin` body. Matches Dart: `_inMixin`.
    pub in_mixin: bool,
    /// Whether currently inside a mixin content block. Matches Dart:
    /// `_inContentBlock`.
    pub in_content_block: bool,
    /// Whether currently inside a control directive (`@if`, `@each`, …).
    /// Matches Dart: `_inControlDirective`.
    pub in_control_directive: bool,
    /// Whether currently inside an unknown at-rule. Matches Dart:
    /// `_inUnknownAtRule`.
    pub in_unknown_at_rule: bool,
    /// Whether currently inside a plain-CSS `@function` rule. Matches Dart:
    /// `_inPlainCssFunction`.
    pub in_plain_css_function: bool,
    /// Whether currently inside a style rule. Matches Dart: `_inStyleRule`.
    pub in_style_rule: bool,
    /// Whether currently inside a parenthesized expression. Matches Dart:
    /// `_inParentheses`.
    pub in_parentheses: bool,
    /// Names assigned with `!global` mapped to their definition spans.
    ///
    /// Collected at parse time because they affect the module generated for
    /// this stylesheet even when never evaluated. Matches Dart:
    /// `_globalVariables`.
    pub global_variables: HashMap<String, FileSpan<'parse>>,
    /// Warnings found while parsing, emitted at evaluation once a logger
    /// exists. Matches Dart: `warnings`.
    pub warnings: Vec<ParseTimeWarning<'parse>>,
    /// The most recently seen silent comment, attached to the next
    /// declaration that opts into it. Matches Dart: `lastSilentComment`.
    pub last_silent_comment: Option<Box<SilentComment<'parse>>>,
}

pub struct StylesheetParser<'parse> {
    pub scanner: SpanScanner<'parse>,
    pub state: StylesheetState<'parse>,
}

/// Scanner + state bundle for stylesheet-level parsing.
///
/// Matches Dart: a `StylesheetParser` instance (stylesheet.dart:38). Rust
/// splits the Dart class into this wrapper plus the [`StylesheetState`]
/// struct so statement-level free functions can borrow scanner and state
/// as two disjoint mutable references.
impl<'parse> StylesheetParser<'parse> {
    pub fn new(
        source: &'parse FileSource<'parse>,
        syntax: Syntax,
        interpolation_map: Option<&'parse InterpolationMap<'parse>>,
    ) -> Self {
        StylesheetParser {
            scanner: SpanScanner::new(source),
            state: StylesheetState {
                parser_state: ParserState {
                    syntax,
                    interpolation_map,
                    in_expression: false,
                },
                parse_selectors: false,
                is_use_allowed: true,
                in_mixin: false,
                in_content_block: false,
                in_control_directive: false,
                in_unknown_at_rule: false,
                in_plain_css_function: false,
                in_style_rule: false,
                in_parentheses: false,
                global_variables: HashMap::new(),
                warnings: Vec::new(),
                last_silent_comment: None,
            },
        }
    }
}

// ======================================================================
// Syntax-dispatched free functions.
//
// Matches Dart: the `@protected` abstract accessors at the bottom of
// stylesheet.dart (`indented`, `plainCss`, `currentIndentation`,
// `styleRuleSelector`, `expectStatementSeparator`, `atEndOfStatement`,
// `lookingAtChildren`, `scanElse`, `children`, `statements`), whose concrete
// bodies live in scss.dart/sass.dart/css.dart. Rust implements each as a
// `match` on [`Syntax`] delegating to the syntax modules.
// ======================================================================

/// Returns the current indentation level.
///
/// Matches Dart: `currentIndentation` (stylesheet.dart:4839). Sass reads
/// from [`SassIndentState`]; SCSS/CSS always return 0. The value is only
/// passed to `scan_else`, never used directly by statement parsing.
pub(crate) fn current_indentation_impl(state: &StylesheetState<'_>) -> usize {
    match &state.parser_state.syntax {
        Syntax::Sass(s) => s.current_indentation,
        _ => 0,
    }
}

/// Checks whether the scanner is at the end of a statement.
///
/// Matches Dart: `atEndOfStatement` (stylesheet.dart:4856).
/// Sass: newline or done. SCSS/CSS: `;`, `}`, `{`, or done.
pub(crate) fn at_end_of_statement_impl(
    scanner: &SpanScanner<'_>,
    state: &StylesheetState<'_>,
) -> bool {
    match &state.parser_state.syntax {
        Syntax::Sass(_) => {
            let next = scanner.peek_char(0);
            next < 0 || is_newline(next as u8 as char)
        }
        _ => {
            let next = scanner.peek_char(0);
            next < 0 || next == ';' as i32 || next == '}' as i32 || next == '{' as i32
        }
    }
}

/// Checks whether children are expected next (after a statement header).
///
/// Matches Dart: `lookingAtChildren` (stylesheet.dart:4861).
/// Sass: newline/done then indentation > current. SCSS/CSS: `{` at current position.
pub(crate) fn looking_at_children_impl<'r, 'parse>(
    scanner: &'r mut SpanScanner<'parse>,
    state: &'r mut StylesheetState<'parse>,
) -> ParseResult<'parse, bool>
where
    'parse: 'r,
{
    match &mut state.parser_state.syntax {
        Syntax::Sass(_) => {
            if !at_end_of_statement_impl(scanner, state) {
                return Ok(false);
            }
            sass_peek_indentation_impl(scanner, state)
                .map(|indent| indent > current_indentation_impl(state))
        }
        _ => Ok(scanner.peek_char(0) == '{' as i32),
    }
}

// ======================================================================
// Sass indentation helpers (preliminary — full implementation in sass.rs)
// ======================================================================

fn sass_peek_indentation_impl<'r, 'parse>(
    scanner: &'r mut SpanScanner<'parse>,
    state: &'r mut StylesheetState<'parse>,
) -> ParseResult<'parse, usize>
where
    'parse: 'r,
{
    super::sass::sass_peek_indentation_impl(scanner, state)
}

// ======================================================================
// Syntax-dispatched: expectStatementSeparator, scanElse, children, statements.
//
// Matches Dart: `expectStatementSeparator` consumes whitespace but nothing
// else, including comments; `children` — unlike most production consumers —
// does *not* consume trailing whitespace so the parent rule's span doesn't
// cover it; `statements` takes a callback that may return `None` for a
// consumed-but-discarded statement such as `@charset` (stylesheet.dart:4845+).
// ======================================================================

pub(crate) fn expect_statement_separator_impl<'r, 'parse>(
    scanner: &'r mut SpanScanner<'parse>,
    state: &'r mut StylesheetState<'parse>,
    _name: &str,
) -> ParseResult<'parse, ()>
where
    'parse: 'r,
{
    match &state.parser_state.syntax {
        Syntax::Sass(_) => sass_expect_statement_separator_impl(scanner, state, Some(_name)),
        _ => scss_expect_statement_separator_impl(scanner, state),
    }
}

pub(crate) fn scan_else_impl<'r, 'parse>(
    scanner: &'r mut SpanScanner<'parse>,
    state: &'r mut StylesheetState<'parse>,
    if_indentation: usize,
) -> ParseResult<'parse, bool>
where
    'parse: 'r,
{
    match &state.parser_state.syntax {
        Syntax::Sass(_) => sass_scan_else_impl(scanner, state, if_indentation),
        _ => scss_scan_else_impl(scanner, state, if_indentation),
    }
}

pub(crate) fn children_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    child: &mut dyn FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Option<Statement<'b>>>,
) -> ParseResult<'b, Vec<Statement<'b>>> {
    match &state.parser_state.syntax {
        Syntax::Sass(_) => sass_children_impl(scanner, state, child),
        _ => scss_children_impl(scanner, state, &mut move |s, st| match child(s, st)? {
            Some(stmt) => Ok(stmt),
            None => Err(Box::new(error_impl("expected statement.", &s.empty_span()))),
        }),
    }
}

pub(crate) fn statements_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    statement: &mut dyn FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Option<Statement<'b>>>,
) -> ParseResult<'b, Vec<Statement<'b>>> {
    match &state.parser_state.syntax {
        Syntax::Sass(_) => sass_statements_impl(scanner, state, statement),
        _ => scss_statements_impl(scanner, state, statement),
    }
}

pub(crate) fn style_rule_selector_impl<'r, 'parse>(
    scanner: &'r mut SpanScanner<'parse>,
    state: &'r mut StylesheetState<'parse>,
) -> ParseResult<'parse, Interpolation<'parse>>
where
    'parse: 'r,
{
    match &state.parser_state.syntax {
        Syntax::Sass(_) => sass_style_rule_selector_impl(scanner, state),
        _ => scss_style_rule_selector_impl(scanner, state),
    }
}

// ======================================================================
// Thin wrappers on StylesheetParser
// ======================================================================

impl<'parse> StylesheetParser<'parse> {
    /// Returns [`current_indentation_impl`].
    pub fn current_indentation(&self) -> usize {
        current_indentation_impl(&self.state)
    }

    /// Returns [`at_end_of_statement_impl`].
    pub fn at_end_of_statement(&self) -> bool {
        at_end_of_statement_impl(&self.scanner, &self.state)
    }

    /// Returns [`looking_at_children_impl`].
    pub fn looking_at_children(&mut self) -> ParseResult<'parse, bool> {
        looking_at_children_impl(&mut self.scanner, &mut self.state)
    }
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use bumpalo::Bump;

    fn make_stylesheet<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
        syntax: Syntax,
    ) -> StylesheetParser<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        StylesheetParser::new(fs, syntax, None)
    }

    #[test]
    fn test_constructor_defaults() {
        let arena = Bump::new();
        let parser = make_stylesheet(&arena, "", Syntax::Scss);
        assert!(parser.state.is_use_allowed);
        assert!(!parser.state.in_mixin);
        assert!(!parser.state.in_content_block);
        assert!(!parser.state.parser_state.in_expression);
        assert!(parser.state.warnings.is_empty());
        assert!(parser.state.global_variables.is_empty());
    }

    #[test]
    fn test_current_indentation_scss() {
        let arena = Bump::new();
        let parser = make_stylesheet(&arena, "", Syntax::Scss);
        assert_eq!(parser.current_indentation(), 0);
    }

    #[test]
    fn test_current_indentation_css() {
        let arena = Bump::new();
        let parser = make_stylesheet(
            &arena,
            "",
            Syntax::Css(CssState {
                disallowed_function_names: HashSet::new(),
            }),
        );
        assert_eq!(parser.current_indentation(), 0);
    }

    #[test]
    fn test_current_indentation_sass() {
        let arena = Bump::new();
        let parser = make_stylesheet(
            &arena,
            "",
            Syntax::Sass(SassIndentState {
                current_indentation: 2,
                next_indentation: None,
                next_indentation_end: None,
                indent_spaces: None,
            }),
        );
        assert_eq!(parser.current_indentation(), 2);
    }

    #[test]
    fn test_at_end_of_statement_scss_semicolon() {
        let arena = Bump::new();
        let parser = make_stylesheet(&arena, ";x", Syntax::Scss);
        assert!(parser.at_end_of_statement());
    }

    #[test]
    fn test_at_end_of_statement_scss_brace() {
        let arena = Bump::new();
        let parser = make_stylesheet(&arena, "}x", Syntax::Scss);
        assert!(parser.at_end_of_statement());
    }

    #[test]
    fn test_at_end_of_statement_scss_open_brace() {
        let arena = Bump::new();
        let parser = make_stylesheet(&arena, "{x", Syntax::Scss);
        assert!(parser.at_end_of_statement());
    }

    #[test]
    fn test_at_end_of_statement_scss_not_end() {
        let arena = Bump::new();
        let parser = make_stylesheet(&arena, "abc", Syntax::Scss);
        assert!(!parser.at_end_of_statement());
    }

    #[test]
    fn test_at_end_of_statement_scss_eof() {
        let arena = Bump::new();
        let parser = make_stylesheet(&arena, "", Syntax::Scss);
        assert!(parser.at_end_of_statement());
    }

    #[test]
    fn test_at_end_of_statement_sass_newline() {
        let arena = Bump::new();
        let parser = make_stylesheet(
            &arena,
            "\nabc",
            Syntax::Sass(SassIndentState {
                current_indentation: 0,
                next_indentation: None,
                next_indentation_end: None,
                indent_spaces: None,
            }),
        );
        assert!(parser.at_end_of_statement());
    }

    #[test]
    fn test_at_end_of_statement_sass_not_end() {
        let arena = Bump::new();
        let parser = make_stylesheet(
            &arena,
            "abc",
            Syntax::Sass(SassIndentState {
                current_indentation: 0,
                next_indentation: None,
                next_indentation_end: None,
                indent_spaces: None,
            }),
        );
        assert!(!parser.at_end_of_statement());
    }

    #[test]
    fn test_looking_at_children_scss_brace() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "{x", Syntax::Scss);
        assert!(parser.looking_at_children().unwrap());
    }

    #[test]
    fn test_looking_at_children_scss_not() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "abc", Syntax::Scss);
        assert!(!parser.looking_at_children().unwrap());
    }

    #[test]
    fn test_at_end_of_statement_sass_eof() {
        let arena = Bump::new();
        let parser = make_stylesheet(
            &arena,
            "",
            Syntax::Sass(SassIndentState {
                current_indentation: 0,
                next_indentation: None,
                next_indentation_end: None,
                indent_spaces: None,
            }),
        );
        assert!(parser.at_end_of_statement());
    }

    #[test]
    fn test_at_end_of_statement_sass_not_newline() {
        let arena = Bump::new();
        let parser = make_stylesheet(
            &arena,
            ";",
            Syntax::Sass(SassIndentState {
                current_indentation: 0,
                next_indentation: None,
                next_indentation_end: None,
                indent_spaces: None,
            }),
        );
        assert!(!parser.at_end_of_statement());
    }

    #[test]
    fn test_looking_at_children_css_brace() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(
            &arena,
            "{",
            Syntax::Css(CssState {
                disallowed_function_names: HashSet::new(),
            }),
        );
        assert!(parser.looking_at_children().unwrap());
    }

    #[test]
    fn test_looking_at_children_css_not_brace() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(
            &arena,
            "x",
            Syntax::Css(CssState {
                disallowed_function_names: HashSet::new(),
            }),
        );
        assert!(!parser.looking_at_children().unwrap());
    }

    #[test]
    fn test_at_end_of_statement_scss_eof_extra() {
        let arena = Bump::new();
        let parser = make_stylesheet(&arena, "", Syntax::Scss);
        assert!(parser.at_end_of_statement());
    }
}
