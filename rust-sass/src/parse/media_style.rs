// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/stylesheet.dart (media query sections)
// go-source: go/value/parse_stylesheet_media.go

//! Stylesheet-level `@media` query parsing into interpolation.
//!
//! Unlike `media_query.rs` (which resolves plain CSS text for the CSS AST),
//! these functions build an `Interpolation` that may still contain Sass
//! expressions such as `(width: $n)` or range comparisons
//! `(100px < width < 200px)`.
// Somewhat duplicated in `media_query.rs` (`MediaQueryParser._mediaQuery`);
// changes to the query shape should be mirrored there and vice versa.

use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::common::ast_node::AstNode;
use crate::common::span_scanner::SpanScanner;

use crate::parse::expression::{
    _expression_impl, expression_until_comparison_impl, ExpressionOpts,
};
use crate::parse::identifier::{interpolated_identifier_impl, single_interpolation_impl};
use crate::parse::parser::{
    expect_whitespace_impl, scan_identifier_impl, span_from_impl, whitespace_impl, ParseError,
    ParseResult,
};
use crate::parse::stylesheet::StylesheetState;
use crate::parse::util::looking_at_interpolated_identifier_impl;

// Consumes a `MediaInParens` expression and writes it to `buffer`.
//
// Handles nested queries, `not <media-or-interp>`, `feature: value`
// declarations, and `<`/`>`/`=` range comparisons (including the two-sided
// form). Newlines are consumed inside the parens; the closing `)` is
// followed by non-newline whitespace only.
pub(crate) fn media_in_parens_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    buffer: &mut InterpolationBuffer<'b>,
) -> ParseResult<'b, ()> {
    if !scanner.scan_char('(') {
        return Err(scanner
            .error("expected media condition in parentheses.", None, 0)
            .into());
    }
    buffer.write_char_code('(');
    whitespace_impl(scanner, &mut state.parser_state, true)?;

    let next = scanner.peek_char(0);
    if next == '(' as i32 {
        media_in_parens_impl(scanner, state, buffer)?;
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        if scan_identifier_impl(scanner, "and", false)? {
            buffer.write(" and ");
            expect_whitespace_impl(scanner, &mut state.parser_state, true)?;
            media_logic_sequence_impl(scanner, state, buffer, "and")?;
        } else if scan_identifier_impl(scanner, "or", false)? {
            buffer.write(" or ");
            expect_whitespace_impl(scanner, &mut state.parser_state, true)?;
            media_logic_sequence_impl(scanner, state, buffer, "or")?;
        }
    } else if scan_identifier_impl(scanner, "not", false)? {
        buffer.write("not ");
        expect_whitespace_impl(scanner, &mut state.parser_state, true)?;
        media_or_interp_impl(scanner, state, buffer)?;
    } else {
        let expression_before = expression_until_comparison_impl(scanner, state)?;
        let span = expression_before.span()?;
        buffer.add(expression_before, span);
        if scanner.scan_char(':') {
            whitespace_impl(scanner, &mut state.parser_state, true)?;
            buffer.write_char_code(':');
            buffer.write_char_code(' ');
            let expression_after = _expression_impl(
                scanner,
                state,
                ExpressionOpts {
                    bracket_list: false,
                    single_equals: false,
                    consume_newlines: true,
                    until: None,
                },
            )?;
            let after_span = expression_after.span()?;
            buffer.add(expression_after, after_span);
        } else {
            let ch = scanner.peek_char(0);
            if ch == '<' as i32 || ch == '>' as i32 || ch == '=' as i32 {
                buffer.write_char_code(' ');
                let read = scanner.read_char()?;
                buffer.write_char_code(read);
                if (ch == '<' as i32 || ch == '>' as i32) && scanner.scan_char('=') {
                    buffer.write_char_code('=');
                }
                buffer.write_char_code(' ');
                whitespace_impl(scanner, &mut state.parser_state, true)?;
                let expression_middle = expression_until_comparison_impl(scanner, state)?;
                let mid_span = expression_middle.span()?;
                buffer.add(expression_middle, mid_span);

                let next2 = scanner.peek_char(0);
                if (ch == '<' as i32 || ch == '>' as i32) && next2 == ch {
                    buffer.write_char_code(' ');
                    let read2 = scanner.read_char()?;
                    buffer.write_char_code(read2);
                    if scanner.scan_char('=') {
                        buffer.write_char_code('=');
                    }
                    buffer.write_char_code(' ');
                    whitespace_impl(scanner, &mut state.parser_state, true)?;
                    let expression_after2 = expression_until_comparison_impl(scanner, state)?;
                    let after2_span = expression_after2.span()?;
                    buffer.add(expression_after2, after2_span);
                }
            }
        }
    }

    scanner.expect_char(')')?;
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    buffer.write_char_code(')');
    Ok(())
}

// ======================================================================
// media_or_interp_impl
// ======================================================================

// Consumes a `MediaOrInterp` expression and writes it to `buffer`: either
// a single `#{}` interpolation or a parenthesized media condition.
pub(crate) fn media_or_interp_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    buffer: &mut InterpolationBuffer<'b>,
) -> ParseResult<'b, ()> {
    if scanner.peek_char(0) == '#' as i32 {
        let (expr, span) = single_interpolation_impl(scanner, state)?;
        buffer.add(expr, span);
    } else {
        media_in_parens_impl(scanner, state, buffer)?;
    }
    Ok(())
}

// ======================================================================
// media_logic_sequence_impl
// ======================================================================

// Consumes one or more `MediaOrInterp` expressions separated by `operator`
// and writes them to `buffer`.
pub(crate) fn media_logic_sequence_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    buffer: &mut InterpolationBuffer<'b>,
    operator: &str,
) -> ParseResult<'b, ()> {
    loop {
        media_or_interp_impl(scanner, state, buffer)?;
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        if !scan_identifier_impl(scanner, operator, false)? {
            return Ok(());
        }
        expect_whitespace_impl(scanner, &mut state.parser_state, false)?;
        buffer.write_char_code(' ');
        buffer.write(operator);
        buffer.write_char_code(' ');
    }
}

// ======================================================================
// media_query_impl
// ======================================================================

// Consumes a single media query: a parenthesized condition with optional
// `and`/`or` tail, or a `not`/`only`/`<type>` prefix form such as
// `@media screen and ...` (this shape is somewhat duplicated in
// `media_query.rs`).
pub(crate) fn media_query_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    buffer: &mut InterpolationBuffer<'b>,
) -> ParseResult<'b, ()> {
    if scanner.peek_char(0) == '(' as i32 {
        media_in_parens_impl(scanner, state, buffer)?;
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        if scan_identifier_impl(scanner, "and", false)? {
            buffer.write(" and ");
            expect_whitespace_impl(scanner, &mut state.parser_state, false)?;
            return media_logic_sequence_impl(scanner, state, buffer, "and");
        } else if scan_identifier_impl(scanner, "or", false)? {
            buffer.write(" or ");
            expect_whitespace_impl(scanner, &mut state.parser_state, false)?;
            return media_logic_sequence_impl(scanner, state, buffer, "or");
        }
        return Ok(());
    }

    let identifier1 = interpolated_identifier_impl(scanner, state)?;
    if let Some(plain) = identifier1.as_plain() {
        if plain.eq_ignore_ascii_case("not") {
            expect_whitespace_impl(scanner, &mut state.parser_state, false)?;
            if !looking_at_interpolated_identifier_impl(scanner) {
                buffer.write("not ");
                return media_or_interp_impl(scanner, state, buffer);
            }
        }
    }

    whitespace_impl(scanner, &mut state.parser_state, false)?;
    buffer.add_interpolation(&identifier1);
    if !looking_at_interpolated_identifier_impl(scanner) {
        return Ok(());
    }

    buffer.write_char_code(' ');
    let identifier2 = interpolated_identifier_impl(scanner, state)?;
    if let Some(plain2) = identifier2.as_plain() {
        if plain2.eq_ignore_ascii_case("and") {
            expect_whitespace_impl(scanner, &mut state.parser_state, false)?;
            buffer.write(" and ");
        } else {
            whitespace_impl(scanner, &mut state.parser_state, false)?;
            buffer.add_interpolation(&identifier2);
            if scan_identifier_impl(scanner, "and", false)? {
                expect_whitespace_impl(scanner, &mut state.parser_state, false)?;
                buffer.write(" and ");
            } else {
                return Ok(());
            }
        }
    } else {
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        buffer.add_interpolation(&identifier2);
        if !scan_identifier_impl(scanner, "and", false)? {
            return Ok(());
        }
        expect_whitespace_impl(scanner, &mut state.parser_state, false)?;
        buffer.write(" and ");
    }

    if scan_identifier_impl(scanner, "not", false)? {
        expect_whitespace_impl(scanner, &mut state.parser_state, false)?;
        buffer.write("not ");
        return media_or_interp_impl(scanner, state, buffer);
    }

    media_logic_sequence_impl(scanner, state, buffer, "and")
}

// ======================================================================
// media_query_list_impl
// ======================================================================

// Consumes a comma-separated list of media queries into one interpolation.
//
// Entries are joined with `", "`; surrounding whitespace never consumes
// newlines.
pub(crate) fn media_query_list_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Interpolation<'b>> {
    let start = scanner.state();
    let mut buffer = InterpolationBuffer::new();
    loop {
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        media_query_impl(scanner, state, &mut buffer)?;
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        if !scanner.scan_char(',') {
            break;
        }
        buffer.write_char_code(',');
        buffer.write_char_code(' ');
    }
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    buffer
        .interpolation(span)
        .map_err(|e| ParseError::from(e).into())
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::source_span_file_source::FileSource;
    use crate::parse::parser::ParserState;
    use crate::parse::stylesheet::Syntax;
    use bumpalo::Bump;
    use std::collections::HashMap;

    fn make_state<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
    ) -> (SpanScanner<'parse>, StylesheetState<'parse>)
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        let scanner = SpanScanner::new(fs);
        let state = StylesheetState {
            parser_state: ParserState {
                syntax: Syntax::Scss,
                interpolation_map: None,
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
            warnings: vec![],
            last_silent_comment: None,
        };
        (scanner, state)
    }

    #[test]
    fn test_media_query_list_simple_type() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "screen");
        let result = media_query_list_impl(&mut s, &mut st).unwrap();
        match result.as_plain() {
            Some(p) => assert_eq!(p, "screen"),
            None => panic!("expected plain interpolation"),
        }
    }

    #[test]
    fn test_media_query_list_comma() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "screen, print");
        let result = media_query_list_impl(&mut s, &mut st).unwrap();
        match result.as_plain() {
            Some(p) => assert_eq!(p, "screen, print"),
            None => panic!("expected plain interpolation"),
        }
    }

    #[test]
    fn test_media_query_list_type_and_feature() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "screen and (color)");
        let result = media_query_list_impl(&mut s, &mut st).unwrap();
        assert!(result.as_plain().is_none());
    }

    #[test]
    fn test_media_query_list_not() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "not screen");
        let result = media_query_list_impl(&mut s, &mut st).unwrap();
        let display = format!("{}", result);
        assert_eq!(display, "not screen");
    }

    #[test]
    fn test_media_query_list_only() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "only screen and (color)");
        let result = media_query_list_impl(&mut s, &mut st).unwrap();
        assert!(result.as_plain().is_none());
    }

    #[test]
    fn test_media_in_parens_feature_comparison() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(width > 100px)");
        let result = media_query_list_impl(&mut s, &mut st).unwrap();
        assert!(result.as_plain().is_none());
    }

    #[test]
    fn test_media_in_parens_colon() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(width: 100px)");
        let result = media_query_list_impl(&mut s, &mut st).unwrap();
        assert!(result.as_plain().is_none());
    }

    #[test]
    fn test_media_in_parens_and() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(width > 100px) and (height > 100px)");
        let result = media_query_list_impl(&mut s, &mut st).unwrap();
        let display = format!("{}", result);
        assert!(display.contains("and"));
    }

    #[test]
    fn test_media_in_parens_or() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(width > 100px) or (height > 100px)");
        let result = media_query_list_impl(&mut s, &mut st).unwrap();
        let display = format!("{}", result);
        assert!(display.contains("or"));
    }

    #[test]
    fn test_media_in_parens_nested() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "((width > 100px))");
        let result = media_query_list_impl(&mut s, &mut st).unwrap();
        assert!(result.as_plain().is_none());
    }

    #[test]
    fn test_media_in_parens_not() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(not (color))");
        let result = media_query_list_impl(&mut s, &mut st).unwrap();
        let display = format!("{}", result);
        assert!(display.contains("not"));
    }

    #[test]
    fn test_media_error_missing_close_paren() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(width > 100px");
        match *media_query_list_impl(&mut s, &mut st).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "expected \")\".");
            }
            _ => panic!("expected ParseError::Scan"),
        };
    }

    #[test]
    fn test_media_error_type_and_no_value() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "screen and");
        match *media_query_list_impl(&mut s, &mut st).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "Expected whitespace.");
            }
            _ => panic!("expected ParseError::Scan"),
        };
    }
}
