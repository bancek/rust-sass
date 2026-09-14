// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/scss.dart
// go-source: go/value/parse_scss.go

//! The CSS-compatible (SCSS) syntax: brace-delimited children, `;`-separated
//! statements, and both comment styles.
//!
//! Covers Dart's `ScssParser`. The base-class `CssParser` narrows this further
//! (see `css.rs`); the indented-syntax counterpart lives in `sass.rs`.

use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::ast::sass::statement::loud_comment::LoudComment;
use crate::ast::sass::statement::silent_comment::SilentComment;
use crate::ast::sass::statement::stylesheet::ParseTimeWarning;
use crate::ast::sass::statement::Statement;
use crate::common::span_scanner::SpanScanner;
use crate::deprecation;
use crate::parse::parser::spaces_impl;
use crate::util::character;

use crate::parse::anyvalue::almost_any_value_impl;
use crate::parse::identifier::single_interpolation_impl;
use crate::parse::parser::{
    error_impl, scan_identifier_impl, span_from_impl, whitespace_impl,
    whitespace_without_comments_impl, ParseError, ParseResult,
};
use crate::parse::stylesheet::{StylesheetState, Syntax};
use crate::parse::stylesheet_parse::variable_declaration_without_namespace_impl;

// ======================================================================
// SCSS silent comment
// ======================================================================

/// Consumes a statement-level silent comment block.
///
/// Consecutive `//` lines are merged into one comment (leading spaces after
/// each newline are skipped). Fails in plain CSS. Matches Dart's
/// `ScssParser._silentComment`.
pub(crate) fn scss_silent_comment_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, SilentComment<'b>> {
    let start = scanner.state();
    scanner.expect("//")?;
    loop {
        while !scanner.is_done() && !character::is_newline(scanner.peek_char(0) as u8 as char) {
            scanner.read_char()?;
        }
        if scanner.is_done() {
            break;
        }
        scanner.read_char()?;
        spaces_impl(scanner)?;
        if scanner.peek_char(0) != '/' as i32 || scanner.peek_char(1) != '/' as i32 {
            break;
        }
        scanner.read_char()?;
        scanner.read_char()?;
    }
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    if matches!(state.parser_state.syntax, Syntax::Css(_)) {
        return Err(Box::new(error_impl(
            "Silent comments aren't allowed in plain CSS.",
            &span,
        )));
    }
    let text = scanner.substring(start.position, None).to_string();
    let comment = SilentComment::new(text, span);
    state.last_silent_comment = Some(Box::new(comment.clone()));
    Ok(comment)
}

// ======================================================================
// SCSS loud comment
// ======================================================================

/// Consumes a statement-level loud comment block.
///
/// `\r` and form-feeds normalize to `\n`; `#{...}` becomes interpolation.
/// Ends at the first `*/`. Matches Dart's `ScssParser._loudComment`.
pub(crate) fn scss_loud_comment_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, LoudComment<'b>> {
    let start = scanner.state();
    scanner.expect("/*")?;
    let mut buffer = InterpolationBuffer::new();
    buffer.write("/*");
    loop {
        let ch = scanner.peek_char(0);
        match ch {
            c if c == '#' as i32 => {
                if scanner.peek_char(1) == '{' as i32 {
                    let (expr, span) = single_interpolation_impl(scanner, state)?;
                    buffer.add(expr, span);
                } else {
                    let ch = scanner.read_char()?;
                    buffer.write_char_code(ch);
                }
            }
            c if c == '*' as i32 => {
                let ch = scanner.read_char()?;
                buffer.write_char_code(ch);
                if scanner.peek_char(0) != '/' as i32 {
                    continue;
                }
                let ch = scanner.read_char()?;
                buffer.write_char_code(ch);
                let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                let interp = buffer
                    .interpolation(span)
                    .map_err(|e| Box::new(ParseError::Sass(e)))?;
                return Ok(LoudComment::new(interp));
            }
            c if c == '\r' as i32 => {
                scanner.read_char()?;
                if scanner.peek_char(0) != '\n' as i32 {
                    buffer.write_char_code('\n');
                }
            }
            c if c == '\u{0C}' as i32 => {
                scanner.read_char()?;
                buffer.write_char_code('\n');
            }
            _ => {
                let ch = scanner.read_char()?;
                buffer.write_char_code(ch);
            }
        }
    }
}

// ======================================================================
// SCSS expectStatementSeparator
// ======================================================================

/// Asserts the scanner is before a statement separator (`;`, `}`, or EOF).
///
/// Consumes whitespace but nothing else, including comments. Matches Dart's
/// `ScssParser.expectStatementSeparator`.
pub(crate) fn scss_expect_statement_separator_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, ()> {
    whitespace_without_comments_impl(scanner, &state.parser_state, true)?;
    if scanner.is_done() {
        return Ok(());
    }
    let next = scanner.peek_char(0);
    if next == ';' as i32 || next == '}' as i32 {
        return Ok(());
    }
    scanner.expect_char(';')?;
    Ok(())
}

// ======================================================================
// SCSS scanElse
// ======================================================================

/// Tries to scan an `@else` (or deprecated `@elseif`) rule after an `@if`
/// block, and returns whether that succeeded.
///
/// Only the rule name is scanned. `@elseif` emits the deprecation warning and
/// rewinds two characters so the caller re-reads `if` as `@else if`. The
/// `if_indentation` parameter is unused here (indented-syntax only). Matches
/// Dart's `ScssParser.scanElse`.
pub(crate) fn scss_scan_else_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    _if_indentation: usize,
) -> ParseResult<'b, bool> {
    let start = scanner.state();
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let before_at = scanner.state();
    if scanner.scan_char('@') {
        let ok = scan_identifier_impl(scanner, "else", true)?;
        if ok {
            return Ok(true);
        }
        let ok = scan_identifier_impl(scanner, "elseif", true)?;
        if ok {
            let span = span_from_impl(scanner, &state.parser_state, before_at)?.file_span()?;
            state.warnings.push(ParseTimeWarning {
                deprecation: Some(&deprecation::ELSEIF),
                message: "@elseif is deprecated and will not be supported in future Sass versions.\n\nRecommendation: @else if".into(),
                span,
                primary_label: None,
                secondary: vec![],
            });
            scanner.set_position(scanner.pos() - 2)?;
            return Ok(true);
        }
    }
    scanner.set_state(start);
    Ok(false)
}

// ======================================================================
// SCSS children
// ======================================================================

/// Consumes a `{...}` block of child statements.
///
/// `$`-leading lines parse as variable declarations without a namespace;
/// stray `;` are skipped. Unlike most consumers this does *not* consume
/// trailing whitespace, so the parent rule's span doesn't cover whitespace
/// after the rule. Matches Dart's `ScssParser.children`.
pub(crate) fn scss_children_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    child: &mut dyn FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Statement<'b>>,
) -> ParseResult<'b, Vec<Statement<'b>>> {
    scanner.expect_char('{')?;
    whitespace_without_comments_impl(scanner, &state.parser_state, true)?;
    let mut children = Vec::new();
    // TODO is this correct? this used to be let mut child_fn. is something missing?
    let child_fn = child;
    loop {
        let ch = scanner.peek_char(0);
        match ch {
            c if c == '$' as i32 => {
                let decl = variable_declaration_without_namespace_impl(scanner, state, None, None)?;
                children.push(Statement::VariableDeclaration(decl));
            }
            c if c == '/' as i32 => match scanner.peek_char(1) {
                c2 if c2 == '/' as i32 => {
                    let comment = scss_silent_comment_impl(scanner, state)?;
                    children.push(Statement::SilentComment(comment));
                    whitespace_without_comments_impl(scanner, &state.parser_state, true)?;
                }
                c2 if c2 == '*' as i32 => {
                    let comment = scss_loud_comment_impl(scanner, state)?;
                    children.push(Statement::LoudComment(comment));
                    whitespace_without_comments_impl(scanner, &state.parser_state, true)?;
                }
                _ => {
                    let stmt = child_fn(scanner, state)?;
                    children.push(stmt);
                }
            },
            c if c == ';' as i32 => {
                scanner.read_char()?;
                whitespace_without_comments_impl(scanner, &state.parser_state, true)?;
            }
            c if c == '}' as i32 => {
                scanner.expect_char('}')?;
                return Ok(children);
            }
            c if c < 0 => {
                return Err(Box::new(error_impl(
                    "expected \"}\".",
                    &scanner.empty_span(),
                )));
            }
            _ => {
                let stmt = child_fn(scanner, state)?;
                children.push(stmt);
            }
        }
    }
}

// ======================================================================
// SCSS statements
// ======================================================================

/// Consumes top-level statements until EOF.
///
/// The `statement` callback may return `None` for consumed-but-unlisted
/// statements. Matches Dart's `ScssParser.statements`.
pub(crate) fn scss_statements_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    statement: &mut dyn FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Option<Statement<'b>>>,
) -> ParseResult<'b, Vec<Statement<'b>>> {
    let mut stmts = Vec::new();
    let statement_fn = statement;
    whitespace_without_comments_impl(scanner, &state.parser_state, true)?;
    while !scanner.is_done() {
        let ch = scanner.peek_char(0);
        match ch {
            c if c == '$' as i32 => {
                let decl = variable_declaration_without_namespace_impl(scanner, state, None, None)?;
                stmts.push(Statement::VariableDeclaration(decl));
            }
            c if c == '/' as i32 => match scanner.peek_char(1) {
                c2 if c2 == '/' as i32 => {
                    let comment = scss_silent_comment_impl(scanner, state)?;
                    stmts.push(Statement::SilentComment(comment));
                    whitespace_without_comments_impl(scanner, &state.parser_state, true)?;
                }
                c2 if c2 == '*' as i32 => {
                    let comment = scss_loud_comment_impl(scanner, state)?;
                    stmts.push(Statement::LoudComment(comment));
                    whitespace_without_comments_impl(scanner, &state.parser_state, true)?;
                }
                _ => {
                    if let Some(st) = statement_fn(scanner, state)? {
                        stmts.push(st);
                    }
                }
            },
            c if c == ';' as i32 => {
                scanner.read_char()?;
                whitespace_without_comments_impl(scanner, &state.parser_state, true)?;
            }
            _ => {
                if let Some(st) = statement_fn(scanner, state)? {
                    stmts.push(st);
                }
            }
        }
    }
    Ok(stmts)
}

// ======================================================================
// SCSS styleRuleSelector
// ======================================================================

/// Parses a style-rule selector as raw interpolated text.
///
/// SCSS selectors are re-parsed after evaluation, so no selector structure is
/// built here. Matches Dart's `ScssParser.styleRuleSelector`.
pub(crate) fn scss_style_rule_selector_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Interpolation<'b>> {
    almost_any_value_impl(scanner, state, false)
}
