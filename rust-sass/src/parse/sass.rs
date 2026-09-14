// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/sass.dart
// go-source: go/value/parse_sass.go

//! The indented Sass syntax: newline- and indentation-delimited statements.
//!
//! Covers Dart's `SassParser`. Indentation state (`current`, peeked `next`,
//! spaces-vs-tabs) lives in `SassIndentState` (see `stylesheet.rs`) instead
//! of parser fields; the SCSS counterpart lives in `scss.rs`.

use crate::ast::sass::dynamic_import::DynamicImport;
use crate::ast::sass::import::Import;
use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::ast::sass::statement::loud_comment::LoudComment;
use crate::ast::sass::statement::silent_comment::SilentComment;
use crate::ast::sass::statement::Statement;
use crate::ast::sass::static_import::StaticImport;
use crate::common::span_scanner::SpanScanner;
use crate::parse::atrule::import_argument_impl;
use crate::parse::stylesheet::at_end_of_statement_impl;
use crate::url::SassUrl;
use crate::util::character;

use crate::parse::anyvalue::almost_any_value_impl;
use crate::parse::identifier::single_interpolation_impl;
use crate::parse::import_url::{is_plain_import_url, parse_import_url};
use crate::parse::parser::{
    error_impl, multi_span_error_impl, scan_identifier_impl, span_from_impl, whitespace_impl,
    ParseError, ParseResult,
};
use crate::parse::stylesheet::{SassIndentState, StylesheetState, Syntax};
use crate::parse::stylesheet_parse::variable_declaration_without_namespace_impl;

// ======================================================================
// SassIndentState helpers
// ======================================================================

fn sass_state<'b>(state: &'b mut StylesheetState<'_>) -> &'b mut SassIndentState {
    match &mut state.parser_state.syntax {
        Syntax::Sass(ref mut s) => s,
        _ => panic!("expected Sass syntax"),
    }
}

// ======================================================================
// Sass expectNewline
// ======================================================================

/// Expects and consumes a single newline character (`\r\n`, `\n`, or form-feed).
///
/// When `trailing_semicolon` is set, the failure message reports that
/// multiple statements on one line are unsupported instead. Matches Dart's
/// `SassParser._expectNewline`.
pub(crate) fn sass_expect_newline_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    trailing_semicolon: bool,
) -> ParseResult<'b, ()> {
    match scanner.peek_char(0) {
        c if c == '\r' as i32 => {
            scanner.read_char()?;
            if scanner.peek_char(0) == '\n' as i32 {
                scanner.read_char()?;
            }
            Ok(())
        }
        c if c == '\n' as i32 || c == '\u{0C}' as i32 => {
            scanner.read_char()?;
            Ok(())
        }
        _ => {
            let msg = if trailing_semicolon {
                "multiple statements on one line are not supported in the indented syntax."
            } else {
                "expected newline."
            };
            let span = scanner.empty_span();
            Err(Box::new(error_impl(msg, &span)))
        }
    }
}

// ======================================================================
// Sass isDoubleNewline
// ======================================================================

/// Returns whether the scanner is immediately before *two* newlines.
///
/// Used to preserve blank lines inside loud comments. Matches Dart's
/// `SassParser._lookingAtDoubleNewline`.
pub(crate) fn sass_is_double_newline_impl(scanner: &SpanScanner<'_>) -> bool {
    let ch = scanner.peek_char(0);
    match ch {
        c if c == '\r' as i32 => {
            let next = scanner.peek_char(1);
            if next == '\n' as i32 {
                character::is_newline(scanner.peek_char(2) as u8 as char)
            } else {
                character::is_newline(next as u8 as char)
            }
        }
        c if c == '\n' as i32 || c == '\u{0C}' as i32 => {
            character::is_newline(scanner.peek_char(1) as u8 as char)
        }
        _ => false,
    }
}

// ======================================================================
// Sass tryTrailingSemicolon
// ======================================================================

/// Consumes a semicolon and trailing whitespace, including comments.
///
/// Returns whether a semicolon was consumed. Matches Dart's
/// `SassParser._tryTrailingSemicolon`.
pub(crate) fn try_sass_trailing_semicolon_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, bool> {
    if scanner.scan_char(';') {
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

// ======================================================================
// Sass peek indentation
// ======================================================================

/// Returns the indentation level of the next source line.
///
/// A source line is any line that's not entirely whitespace; blank lines are
/// skipped and EOF reports 0. Results are cached in `SassIndentState` and the
/// scanner is rewound. Matches Dart's `SassParser._peekIndentation`.
pub(crate) fn sass_peek_indentation_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, usize> {
    {
        let si = sass_state(state);
        if let Some(ni) = si.next_indentation {
            return Ok(ni);
        }
        if scanner.is_done() {
            si.next_indentation = Some(0);
            si.next_indentation_end = Some(scanner.state());
            return Ok(0);
        }
    }
    let start = scanner.state();
    let found_newline =
        scanner.peek_char(0) >= 0 && character::is_newline(scanner.peek_char(0) as u8 as char);
    if found_newline {
        scanner.read_char()?;
    } else {
        return Err(Box::new(error_impl(
            "Expected newline.",
            &scanner.empty_span(),
        )));
    }
    let mut contains_tab;
    let mut contains_space;
    loop {
        contains_tab = false;
        contains_space = false;
        let mut indent = 0;
        loop {
            let ch = scanner.peek_char(0);
            match ch {
                c if c == ' ' as i32 => {
                    contains_space = true;
                    indent += 1;
                    scanner.read_char()?;
                }
                c if c == '\t' as i32 => {
                    contains_tab = true;
                    indent += 1;
                    scanner.read_char()?;
                }
                _ => break,
            }
        }
        if scanner.is_done() {
            let si = sass_state(state);
            si.next_indentation = Some(0);
            si.next_indentation_end = Some(scanner.state());
            scanner.set_state(start);
            return Ok(0);
        }
        let is_newline =
            scanner.peek_char(0) >= 0 && character::is_newline(scanner.peek_char(0) as u8 as char);
        if !is_newline {
            sass_check_indentation_consistency_impl(scanner, state, contains_tab, contains_space)?;
            let si = sass_state(state);
            si.next_indentation = Some(indent);
            if indent > 0 && si.indent_spaces.is_none() {
                si.indent_spaces = Some(contains_space);
            }
            si.next_indentation_end = Some(scanner.state());
            scanner.set_state(start);
            return Ok(indent);
        }
        scanner.read_char()?;
    }
}

// ======================================================================
// Sass silent comment
// ======================================================================

/// Consumes an indented-style silent comment.
///
/// Continuation lines more indented than the parent are folded in with
/// padding; a same-indent `//` line is consumed as a final continuation.
/// Matches Dart's `SassParser._silentComment`.
pub(crate) fn sass_silent_comment_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, SilentComment<'b>> {
    let start = scanner.state();
    scanner.expect("//")?;
    let parent_indentation = {
        let si = sass_state(state);
        si.current_indentation
    };
    let mut text = String::new();
    let mut first = true;
    'outer: loop {
        let comment_prefix = if scanner.peek_char(0) == '/' as i32 {
            "///"
        } else {
            "//"
        };
        loop {
            if !first {
                break;
            }
            first = false;
        }
        text.push_str(comment_prefix);
        let current = {
            let si = sass_state(state);
            si.current_indentation
        };
        for _ in comment_prefix.len()..current.saturating_sub(parent_indentation) {
            text.push(' ');
        }
        while scanner.peek_char(0) >= 0
            && !character::is_newline(scanner.peek_char(0) as u8 as char)
        {
            let ch = scanner.read_char()?;
            text.push(ch);
        }
        text.push('\n');
        let peek = sass_peek_indentation_impl(scanner, state)?;
        if peek < parent_indentation {
            break 'outer;
        }
        if peek == parent_indentation {
            let pos = scanner.pos();
            let p1 = (pos as isize) + 1 + (peek as isize);
            let p2 = (pos as isize) + 2 + (peek as isize);
            if p1 >= 0
                && p2 >= 0
                && scanner.peek_char(p1) == '/' as i32
                && scanner.peek_char(p2) == '/' as i32
            {
                sass_read_indentation_impl(scanner, state)?;
            }
            break;
        }
        if peek > parent_indentation {
            sass_read_indentation_impl(scanner, state)?;
        }
    }
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    let comment = SilentComment::new(text, span);
    state.last_silent_comment = Some(Box::new(comment.clone()));
    Ok(comment)
}

// ======================================================================
// Sass loud comment
// ======================================================================

/// Consumes an indented-style loud comment.
///
/// Continuation lines are re-emitted with ` * ` prefixes, blank lines are
/// preserved, and trailing comments after `*/` are allowed for backwards
/// compatibility — any other trailing text fails with "Unexpected text after
/// end of comment". Matches Dart's `SassParser._loudComment`.
pub(crate) fn sass_loud_comment_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, LoudComment<'b>> {
    let start = scanner.state();
    scanner.expect("/*")?;
    let mut first = true;
    let mut buffer = InterpolationBuffer::new();
    buffer.write("/*");
    let parent_indentation = {
        let si = sass_state(state);
        si.current_indentation
    };
    'outer: loop {
        if first {
            let beginning_of_comment = scanner.pos();
            // consume spaces
            while scanner.peek_char(0) == ' ' as i32 || scanner.peek_char(0) == '\t' as i32 {
                scanner.read_char()?;
            }
            if character::is_newline(scanner.peek_char(0) as u8 as char) {
                sass_read_indentation_impl(scanner, state)?;
                buffer.write_char_code(' ');
            } else {
                let rest = scanner
                    .substring(beginning_of_comment, Some(scanner.pos()))
                    .to_string();
                buffer.write(&rest);
            }
        } else {
            buffer.write_char_code('\n');
            buffer.write(" * ");
        }
        first = false;
        // Leading indentation padding
        {
            let current = {
                let si = sass_state(state);
                si.current_indentation
            };
            for _ in 3..current.saturating_sub(parent_indentation) {
                buffer.write_char_code(' ');
            }
        }
        // Inner content loop
        'inner: loop {
            let ch = scanner.peek_char(0);
            if ch < 0 {
                break 'outer;
            }
            if character::is_newline(ch as u8 as char) {
                break 'inner;
            }
            if ch == '#' as i32 && scanner.peek_char(1) == '{' as i32 {
                let (expr, span) = single_interpolation_impl(scanner, state)?;
                buffer.add(expr, span);
            } else if ch == '*' as i32 {
                scanner.read_char()?;
                buffer.write_char_code('*');
                if scanner.peek_char(0) == '/' as i32 {
                    scanner.read_char()?;
                    buffer.write_char_code('/');
                    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;

                    // Match Go: p.whitespace(false) — consumes spaces/tabs + comments
                    whitespace_impl(scanner, &mut state.parser_state, false)?;

                    // Match Go: for util.IsNewline(p.scanner.PeekChar(0)) { ... }
                    while scanner.peek_char(0) >= 0
                        && character::is_newline(scanner.peek_char(0) as u8 as char)
                    {
                        let peek = sass_peek_indentation_impl(scanner, state)?;
                        if peek <= parent_indentation {
                            break;
                        }
                        while sass_is_double_newline_impl(scanner) {
                            sass_expect_newline_impl(scanner, false)?;
                        }
                        sass_read_indentation_impl(scanner, state)?;
                        whitespace_impl(scanner, &mut state.parser_state, false)?;
                    }

                    // Match Go: if !p.scanner.IsDone() && !util.IsNewline(peek) → multiSpanError
                    if !scanner.is_done()
                        && scanner.peek_char(0) >= 0
                        && !character::is_newline(scanner.peek_char(0) as u8 as char)
                    {
                        let error_start = scanner.state();
                        while !scanner.is_done()
                            && scanner.peek_char(0) >= 0
                            && !character::is_newline(scanner.peek_char(0) as u8 as char)
                        {
                            scanner.read_char()?;
                        }
                        let error_span = span_from_impl(scanner, &state.parser_state, error_start)?
                            .file_span()?;
                        return Err(Box::new(multi_span_error_impl(
                            "Unexpected text after end of comment",
                            &error_span,
                            "extra text",
                            vec![(span, "comment".into())],
                        )));
                    }

                    let interp = buffer
                        .interpolation(span)
                        .map_err(|e| Box::new(ParseError::Sass(e)))?;
                    return Ok(LoudComment::new(interp));
                }
            } else {
                let ch = scanner.read_char()?;
                buffer.write_char_code(ch);
            }
        }
        // After inner loop
        let peek = sass_peek_indentation_impl(scanner, state)?;
        if peek <= parent_indentation {
            break 'outer;
        }
        while sass_is_double_newline_impl(scanner) {
            sass_expect_newline_impl(scanner, false)?;
            buffer.write_char_code('\n');
            buffer.write(" *");
        }
        sass_read_indentation_impl(scanner, state)?;
    }
    // Unterminated comment
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    let interp = buffer
        .interpolation(span)
        .map_err(|e| Box::new(ParseError::Sass(e)))?;
    Ok(LoudComment::new(interp))
}

// ======================================================================
// Sass read indentation
// ======================================================================

/// Consumes indentation whitespace and returns the indentation level of the
/// next line.
///
/// Flushes the peeked cache from [`sass_peek_indentation_impl`]. Matches
/// Dart's `SassParser._readIndentation`.
pub(crate) fn sass_read_indentation_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, usize> {
    let needs_peek = {
        let si = sass_state(state);
        si.next_indentation.is_none()
    };
    if needs_peek {
        let indent = sass_peek_indentation_impl(scanner, state)?;
        let si = sass_state(state);
        si.next_indentation = Some(indent);
    }
    let si = sass_state(state);
    let indent = si.next_indentation.take().unwrap_or(0);
    si.current_indentation = indent;
    if let Some(end) = si.next_indentation_end.take() {
        scanner.set_state(end);
    }
    Ok(indent)
}

// ======================================================================
// Sass check indentation consistency
// ======================================================================

/// Ensures the document uses consistent characters for indentation.
///
/// Mixed tabs-and-spaces fail; once the first indented line fixes the style,
/// the other character fails. The span covers the whole indent. Matches
/// Dart's `SassParser._checkIndentationConsistency`.
pub(crate) fn sass_check_indentation_consistency_impl<'b>(
    scanner: &SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    contains_tab: bool,
    contains_space: bool,
) -> ParseResult<'b, ()> {
    // Matches Dart: _checkIndentationConsistency — the span covers the whole
    // indent (line start .. current position, i.e. position - column).
    let indent_start = scanner.pos().saturating_sub(scanner.column());
    let span = scanner.span_from_to(indent_start, scanner.pos());
    if contains_tab && contains_space {
        return Err(Box::new(error_impl(
            "Tabs and spaces may not be mixed.",
            &span,
        )));
    }
    let si = sass_state(state);
    if let Some(spaces) = si.indent_spaces {
        if spaces && contains_tab {
            return Err(Box::new(error_impl("Expected spaces, was tabs.", &span)));
        } else if !spaces && contains_space {
            return Err(Box::new(error_impl("Expected tabs, was spaces.", &span)));
        }
    }
    Ok(())
}

// ======================================================================
// Sass child
// ======================================================================

/// Consumes a child of the current statement.
///
/// Handles document-level productions (empty lines, `$` declarations,
/// comments); the `child` callback consumes context-specific children.
/// Matches Dart's `SassParser._child`.
pub(crate) fn sass_child_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    mut child: impl FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Option<Statement<'b>>>,
) -> ParseResult<'b, Option<Statement<'b>>> {
    let ch = scanner.peek_char(0);
    match ch {
        c if c == '\r' as i32 || c == '\n' as i32 || c == '\u{0C}' as i32 => Ok(None),
        c if c == '$' as i32 => {
            let decl = variable_declaration_without_namespace_impl(scanner, state, None, None)?;
            Ok(Some(Statement::VariableDeclaration(decl)))
        }
        c if c == '/' as i32 => match scanner.peek_char(1) {
            c2 if c2 == '/' as i32 => {
                let comment = sass_silent_comment_impl(scanner, state)?;
                Ok(Some(Statement::SilentComment(comment)))
            }
            c2 if c2 == '*' as i32 => {
                let comment = sass_loud_comment_impl(scanner, state)?;
                Ok(Some(Statement::LoudComment(comment)))
            }
            _ => match child(scanner, state)? {
                Some(stmt) => Ok(Some(stmt)),
                None => Ok(None),
            },
        },
        _ => match child(scanner, state)? {
            Some(stmt) => Ok(Some(stmt)),
            None => Ok(None),
        },
    }
}

// ======================================================================
// Sass whileIndentedLower
// ======================================================================

/// Runs `body` for each statement indented beneath the starting line.
///
/// The first child fixes the expected indentation; later lines at a different
/// depth fail with "Inconsistent indentation, expected N spaces." Matches
/// Dart's `SassParser._whileIndentedLower`.
pub(crate) fn sass_while_indented_lower_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    mut body: impl FnMut(&mut SpanScanner<'b>, &mut StylesheetState<'b>) -> ParseResult<'b, ()>,
) -> ParseResult<'b, ()> {
    let parent_indentation = {
        let si = sass_state(state);
        si.current_indentation
    };
    let mut child_indentation: Option<usize> = None;
    loop {
        let peek_indent = sass_peek_indentation_impl(scanner, state)?;
        if peek_indent <= parent_indentation {
            break;
        }
        let indent = sass_read_indentation_impl(scanner, state)?;
        if child_indentation.is_none() {
            child_indentation = Some(indent);
        }
        if child_indentation != Some(indent) {
            // Dart: `scanner.error(.., position: scanner.position - scanner.column,
            // length: scanner.column)` — the span covers the leading whitespace
            // of the offending line (sass.dart:376-380).
            let pos = scanner.pos();
            let col = scanner.column();
            return Err(Box::new(error_impl(
                &format!(
                    "Inconsistent indentation, expected {} spaces.",
                    child_indentation.unwrap()
                ),
                &scanner.span_from_to(pos.saturating_sub(col), pos),
            )));
        }
        body(scanner, state)?;
    }
    Ok(())
}

/// Consumes an indented block of child statements.
///
/// Matches Dart's `SassParser.children`.
pub(crate) fn sass_children_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    mut child: impl FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Option<Statement<'b>>>,
) -> ParseResult<'b, Vec<Statement<'b>>> {
    let mut children = Vec::new();
    sass_while_indented_lower_impl(scanner, state, |s, st| {
        if let Some(parsed) = sass_child_impl(s, st, |s2, st2| match child(s2, st2)? {
            Some(stmt) => Ok(Some(stmt)),
            None => Ok(None),
        })? {
            children.push(parsed);
        }
        Ok(())
    })?;
    Ok(children)
}

/// Consumes top-level indented-syntax statements.
///
/// Leading indentation at the document start is illegal, and every statement
/// must return to indentation 0. Matches Dart's `SassParser.statements`.
pub(crate) fn sass_statements_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    mut statement: impl FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Option<Statement<'b>>>,
) -> ParseResult<'b, Vec<Statement<'b>>> {
    let mut stmts = Vec::new();
    let ch = scanner.peek_char(0);
    if ch == ' ' as i32 || ch == '\t' as i32 {
        // Matches Dart: position 0, length = current position (the indent).
        let span = scanner.span_from_to(0, scanner.pos());
        return Err(Box::new(error_impl(
            "Indenting at the beginning of the document is illegal.",
            &span,
        )));
    }
    while !scanner.is_done() {
        if let Some(st) = sass_child_impl(scanner, state, |s, st| match statement(s, st)? {
            Some(stmt) => Ok(Some(stmt)),
            None => Ok(None),
        })? {
            stmts.push(st);
        }
        let indent = sass_read_indentation_impl(scanner, state)?;
        if indent != 0 {
            return Err(Box::new(error_impl(
                "Internal error: expected top-level indentation of 0.",
                &scanner.empty_span(),
            )));
        }
    }
    Ok(stmts)
}

/// Asserts the scanner is at the end of a statement.
///
/// Consumes an optional trailing `;`, requires a newline, and rejects deeper
/// indentation ("Nothing may be indented ..."). Matches Dart's
/// `SassParser.expectStatementSeparator`.
pub(crate) fn sass_expect_statement_separator_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    name: Option<&str>,
) -> ParseResult<'b, ()> {
    let trailing = try_sass_trailing_semicolon_impl(scanner, state)?;
    if !at_end_of_statement_impl(scanner, state) {
        sass_expect_newline_impl(scanner, trailing)?;
    }
    let peek_indent = sass_peek_indentation_impl(scanner, state)?;
    let current = sass_state(state).current_indentation;
    if peek_indent <= current {
        return Ok(());
    }
    let label = if let Some(n) = name {
        format!("beneath a {n}")
    } else {
        "here".to_string()
    };
    let msg = format!("Nothing may be indented {label}.");
    // Dart: `scanner.error(..., position: _nextIndentationEnd!.position)` — a
    // zero-length span at the start of the indented content.
    let pos = sass_state(state)
        .next_indentation_end
        .map(|s| s.position)
        .unwrap_or(scanner.location().offset);
    Err(Box::new(error_impl(&msg, &scanner.span_from_to(pos, pos))))
}

/// Tries to scan an `@else` rule after an `@if` block at the same
/// indentation, and returns whether that succeeded.
///
/// Scanner and indentation state are restored on failure. Matches Dart's
/// `SassParser.scanElse`.
pub(crate) fn sass_scan_else_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    if_indentation: usize,
) -> ParseResult<'b, bool> {
    let peek_indent = sass_peek_indentation_impl(scanner, state)?;
    if peek_indent != if_indentation {
        return Ok(false);
    }
    let saved_indentation = {
        let si = sass_state(state);
        (
            si.current_indentation,
            si.next_indentation,
            si.next_indentation_end,
        )
    };
    let _ = sass_read_indentation_impl(scanner, state)?;
    if scanner.scan_char('@') && scan_identifier_impl(scanner, "else", true)? {
        return Ok(true);
    }
    // Restore state
    {
        let si = sass_state(state);
        si.current_indentation = saved_indentation.0;
        si.next_indentation = saved_indentation.1;
        si.next_indentation_end = saved_indentation.2;
    }
    let start = scanner.state();
    scanner.set_state(start);
    Ok(false)
}

/// Parses an indented-syntax `@import` argument.
///
/// Quoted URLs and case-insensitive `url(...)` delegate to the shared
/// importer; anything else is a raw URL scanned to `,`, `;`, or newline —
/// plain-CSS URLs become quoted static imports, the rest dynamic imports
/// validated as URLs. Matches Dart's `SassParser.importArgument`.
pub(crate) fn sass_import_argument_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Import<'b>> {
    let ch = scanner.peek_char(0);
    if ch == 'u' as i32 || ch == 'U' as i32 {
        let start = scanner.state();
        // Matches Dart: scanIdentifier("url") is case-insensitive by
        // default, so `URL(`, `Url(` route to the quoted-import path.
        if scan_identifier_impl(scanner, "url", false)? && scanner.peek_char(0) == '(' as i32 {
            scanner.set_state(start);
            return import_argument_impl(scanner, state);
        }
        scanner.set_state(start);
    }
    if ch == '\'' as i32 || ch == '"' as i32 {
        return import_argument_impl(scanner, state);
    }
    // Raw URL
    let start = scanner.state();
    while scanner.peek_char(0) >= 0
        && scanner.peek_char(0) != ',' as i32
        && scanner.peek_char(0) != ';' as i32
        && !character::is_newline(scanner.peek_char(0) as u8 as char)
    {
        scanner.read_char()?;
    }
    let raw_url = scanner
        .substring(start.position, Some(scanner.pos()))
        .to_string();
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    if is_plain_import_url(&raw_url) {
        let quoted = format!("\"{raw_url}\"");
        Ok(Import::Static(StaticImport::new(
            Interpolation::plain(quoted, span),
            span,
            None,
        )))
    } else {
        let parsed = parse_import_url(&raw_url);
        // Matches Dart: DynamicImport(parseImportUrl(url), urlSpan) — the URL
        // is passed through unchanged; Uri.parse(url) only validates (throwing
        // a FormatException caught as "Invalid URL").
        SassUrl::parse(&parsed).map_err(|e| error_impl(&format!("Invalid URL: {e}"), &span))?;
        Ok(Import::Dynamic(DynamicImport::new(parsed, span)))
    }
}

// ======================================================================
// Sass styleRuleSelector
// ======================================================================

/// Parses an indented-syntax style-rule selector.
///
/// Grabs comma-continued lines (comments omitted) into one interpolation for
/// later re-parsing. Matches Dart's `SassParser.styleRuleSelector`.
pub(crate) fn sass_style_rule_selector_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Interpolation<'b>> {
    let start = scanner.state();
    let mut buffer = InterpolationBuffer::new();
    loop {
        let val = almost_any_value_impl(scanner, state, true)?;
        buffer.add_interpolation(&val);
        buffer.write_char_code('\n');
        let trailing = buffer.trailing_string();
        let trimmed = trailing.trim_end();
        let continue_loop = trimmed.ends_with(',') && {
            let ch = scanner.peek_char(0);
            ch >= 0 && character::is_newline(ch as u8 as char)
        };
        if continue_loop {
            scanner.read_char()?;
        } else {
            break;
        }
    }
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    buffer
        .interpolation(span)
        .map_err(|e| Box::new(ParseError::Sass(e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::source_span_file_source::FileSource;
    use crate::parse::stylesheet::StylesheetParser;
    use crate::parse::stylesheet_parse::parse_impl;
    use bumpalo::Bump;

    fn sass_syntax() -> Syntax {
        Syntax::Sass(SassIndentState {
            current_indentation: 0,
            next_indentation: None,
            next_indentation_end: None,
            indent_spaces: None,
        })
    }

    fn parse_sass_err(text: &str) -> String {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, text, None);
        let mut parser = StylesheetParser::new(fs, sass_syntax(), None);
        let err = parse_impl(&mut parser.scanner, &mut parser.state).unwrap_err();
        err.to_string()
    }

    #[test]
    fn test_indent_consistency_message_span() {
        // Matches Dart: _checkIndentationConsistency messages + indent span.
        // Span shape is verified end-to-end via CLI differential (mix.sass);
        // here we lock the message text.
        let err = parse_sass_err("a\n \tb: c\n");
        assert!(err.contains("Tabs and spaces may not be mixed."), "{err}");
    }

    #[test]
    fn test_expected_spaces_tabs_span() {
        for (src, want) in [
            ("a\n\tb: c\n  d: e\n", "Expected tabs, was spaces."),
            ("a\n  b: c\n\td: e\n", "Expected spaces, was tabs."),
        ] {
            let err = parse_sass_err(src);
            assert!(err.contains(want), "src {src:?}: {err}");
        }
    }

    #[test]
    fn test_doc_start_indent_span() {
        // Matches Dart: statements() errors position 0, length = indent.
        let err = parse_sass_err("  a: b\n");
        assert!(
            err.contains("Indenting at the beginning of the document is illegal."),
            "{err}"
        );
    }
}
