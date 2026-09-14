// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/stylesheet.dart (interpolatedIdentifier, _interpolatedIdentifierBody, singleInterpolation)
// go-source: go/value/parse_stylesheet_identifier.go

use crate::ast::sass::expression::Expression;
use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::common::file_span::FileSpan;
use crate::common::span_scanner::SpanScanner;
use crate::parse::expression::ExpressionOpts;
use crate::parse::expression::_expression_impl;
use crate::parse::parser::error_impl;
use crate::util::character;

use crate::parse::parser::{escape_impl, span_from_impl, whitespace_impl, ParseResult};
use crate::parse::stylesheet::{StylesheetParser, StylesheetState, Syntax};

// ======================================================================
// Free functions
// ======================================================================

/// Consumes an identifier that may contain interpolation.
///
/// Handles the leading `--` fast path (custom properties skip the
/// name-start check) and full `#{...}` interpolation anywhere in the name,
/// including the leading position where `--1` alone would be invalid.
/// Matches Dart's `interpolatedIdentifier`.
pub(crate) fn interpolated_identifier_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, Interpolation<'parse>> {
    let start = scanner.state();
    let mut buffer = InterpolationBuffer::new();

    if scanner.scan_char('-') {
        buffer.write_char_code('-');
        if scanner.scan_char('-') {
            buffer.write_char_code('-');
            interpolated_identifier_body_helper_impl(scanner, state, &mut buffer)?;
            let span = span_from_impl(scanner, &state.parser_state, start)?;
            return Ok(buffer.interpolation(span)?);
        }
    }

    let ch = scanner.peek_char(0);
    if ch < 0 {
        return Err(Box::new(
            scanner.error("Expected identifier.", None, 0).into(),
        ));
    }
    let ch = char::from_u32(ch as u32).unwrap_or('\0');
    if character::is_name_start(ch) {
        let c = scanner.read_char()?;
        buffer.write_char_code(c);
    } else if ch == '\\' {
        let esc = escape_impl(scanner, true)?;
        buffer.write(&esc);
    } else if ch == '#' && scanner.peek_char(1) == '{' as i32 {
        let (expr, span) = single_interpolation_impl(scanner, state)?;
        buffer.add(expr, span);
    } else {
        return Err(Box::new(
            scanner.error("Expected identifier.", None, 0).into(),
        ));
    }

    interpolated_identifier_body_helper_impl(scanner, state, &mut buffer)?;
    let span = span_from_impl(scanner, &state.parser_state, start)?;
    Ok(buffer.interpolation(span)?)
}

/// Consumes a chunk of a possibly-interpolated CSS identifier after the name
/// start.
///
/// Fails with "Expected identifier body." when nothing follows, so callers
/// must have already consumed the leading character. Matches Dart's
/// `_interpolatedIdentifierBody`.
pub(crate) fn interpolated_identifier_body_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, Interpolation<'parse>> {
    let start = scanner.state();
    let mut buffer = InterpolationBuffer::new();
    interpolated_identifier_body_helper_impl(scanner, state, &mut buffer)?;
    if buffer.is_empty() {
        return Err(Box::new(
            scanner.error("Expected identifier body.", None, 0).into(),
        ));
    }
    let span = span_from_impl(scanner, &state.parser_state, start)?;
    Ok(buffer.interpolation(span)?)
}

// Like `interpolated_identifier_body_impl`, but parses the body into the
// given buffer instead of returning a new interpolation. Matches Dart's
// `_interpolatedIdentifierBodyHelper`.
pub(crate) fn interpolated_identifier_body_helper_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    buffer: &mut InterpolationBuffer<'parse>,
) -> ParseResult<'parse, ()> {
    loop {
        let ch = scanner.peek_char(0);
        if ch < 0 {
            return Ok(());
        }
        let ch = char::from_u32(ch as u32).unwrap_or('\0');
        match ch {
            '_' | '-' => {
                let c = scanner.read_char()?;
                buffer.write_char_code(c);
            }
            _ if character::is_alphanumeric(ch) || (ch as u32) >= 0x0080 => {
                let c = scanner.read_char()?;
                buffer.write_char_code(c);
            }
            '\\' => {
                let esc = escape_impl(scanner, false)?;
                buffer.write(&esc);
            }
            '#' if scanner.peek_char(1) == '{' as i32 => {
                let (expr, span) = single_interpolation_impl(scanner, state)?;
                buffer.add(expr, span);
            }
            _ => return Ok(()),
        }
    }
}

/// Consumes a single `#{...}` interpolation and returns it along with the
/// span covering the `#{}`.
///
/// Errors in plain CSS, where interpolation is never allowed. Matches Dart's
/// `singleInterpolation`.
pub(crate) fn single_interpolation_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, (Expression<'parse>, FileSpan<'parse>)> {
    let start = scanner.state();
    scanner.expect("#{")?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let opts = ExpressionOpts {
        bracket_list: false,
        single_equals: false,
        consume_newlines: true,
        until: None,
    };
    let contents = _expression_impl(scanner, state, opts)?;
    scanner.expect_char('}')?;
    if matches!(&state.parser_state.syntax, Syntax::Css(_)) {
        let span = span_from_impl(scanner, &state.parser_state, start)?;
        let file_span = span.file_span()?;
        return Err(Box::new(error_impl(
            "Interpolation isn't allowed in plain CSS.",
            &file_span,
        )));
    }
    let span = span_from_impl(scanner, &state.parser_state, start)?;
    let file_span = span.file_span()?;
    Ok((contents, file_span))
}

// ======================================================================
// Thin wrappers on StylesheetParser
// ======================================================================

impl<'parse> StylesheetParser<'parse> {
    pub fn interpolated_identifier(&mut self) -> ParseResult<'parse, Interpolation<'parse>> {
        interpolated_identifier_impl(&mut self.scanner, &mut self.state)
    }

    pub fn interpolated_identifier_body(&mut self) -> ParseResult<'parse, Interpolation<'parse>> {
        interpolated_identifier_body_impl(&mut self.scanner, &mut self.state)
    }

    pub fn single_interpolation(
        &mut self,
    ) -> ParseResult<'parse, (Expression<'parse>, FileSpan<'parse>)> {
        single_interpolation_impl(&mut self.scanner, &mut self.state)
    }
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::source_span_file_source::FileSource;
    use crate::parse::parser::ParseError;
    use crate::parse::stylesheet::CssState;
    use bumpalo::Bump;
    use std::collections::HashSet;

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
    fn test_interpolated_identifier_simple() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "foo", Syntax::Scss);
        let interp = parser.interpolated_identifier().unwrap();
        assert!(interp.is_plain());
        assert_eq!(interp.as_plain(), Some("foo"));
    }

    #[test]
    fn test_interpolated_identifier_double_dash() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "--prop", Syntax::Scss);
        let interp = parser.interpolated_identifier().unwrap();
        assert_eq!(interp.as_plain(), Some("--prop"));
    }

    #[test]
    fn test_interpolated_identifier_underscore() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "_foo", Syntax::Scss);
        let interp = parser.interpolated_identifier().unwrap();
        assert_eq!(interp.as_plain(), Some("_foo"));
    }

    #[test]
    fn test_interpolated_identifier_escape() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "\\41", Syntax::Scss);
        let interp = parser.interpolated_identifier().unwrap();
        assert_eq!(interp.as_plain(), Some("A"));
    }

    #[test]
    fn test_interpolated_identifier_escape_body() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "f\\6fo", Syntax::Scss);
        let interp = parser.interpolated_identifier().unwrap();
        assert_eq!(interp.as_plain(), Some("foo"));
    }

    #[test]
    fn test_interpolated_identifier_error_empty() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "", Syntax::Scss);
        let result = parser.interpolated_identifier();
        assert!(result.is_err());
    }

    #[test]
    fn test_interpolated_identifier_error_digit() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "1foo", Syntax::Scss);
        let result = parser.interpolated_identifier();
        assert!(result.is_err());
    }

    #[test]
    fn test_interpolated_identifier_body_simple() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "foo", Syntax::Scss);
        parser.scanner.scan_char('f');
        let interp = parser.interpolated_identifier_body().unwrap();
        assert_eq!(interp.as_plain(), Some("oo"));
    }

    #[test]
    fn test_interpolated_identifier_body_error_empty() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, ".", Syntax::Scss);
        parser.scanner.scan_char('.');
        let result = parser.interpolated_identifier_body();
        assert!(result.is_err());
    }

    #[test]
    fn test_interpolated_identifier_interpolation() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "#{$var}rest", Syntax::Scss);
        let interp = parser.interpolated_identifier().unwrap();
        assert!(!interp.is_plain());
        assert_eq!(interp.initial_plain(), "");
    }

    #[test]
    fn test_interpolated_identifier_body_interpolation() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "f#{$var}", Syntax::Scss);
        parser.scanner.scan_char('f');
        let interp = parser.interpolated_identifier_body().unwrap();
        assert!(!interp.is_plain());
    }

    #[test]
    fn test_single_interpolation() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "#{$var}", Syntax::Scss);
        let (expr, _span) = parser.single_interpolation().unwrap();
        assert!(matches!(expr, Expression::Variable(_)));
    }

    #[test]
    fn test_single_interpolation_plain_css() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(
            &arena,
            "#{$var}",
            Syntax::Css(CssState {
                disallowed_function_names: HashSet::new(),
            }),
        );
        let result = parser.single_interpolation();
        assert!(result.is_err());
    }

    #[test]
    fn test_interpolated_identifier_error_message_empty() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "", Syntax::Scss);
        match *parser.interpolated_identifier().unwrap_err() {
            ParseError::Scan(s) => assert_eq!(s.message, "Expected identifier."),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_interpolated_identifier_dash_then_escape() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "-\\41", Syntax::Scss);
        let interp = parser.interpolated_identifier().unwrap();
        assert_eq!(interp.as_plain(), Some("-A"));
    }

    #[test]
    fn test_interpolated_identifier_dash_not_followed_by_name() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "-1", Syntax::Scss);
        match *parser.interpolated_identifier().unwrap_err() {
            ParseError::Scan(s) => assert_eq!(s.message, "Expected identifier."),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_interpolated_identifier_body_error_message_empty() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, ".", Syntax::Scss);
        parser.scanner.scan_char('.');
        match *parser.interpolated_identifier_body().unwrap_err() {
            ParseError::Scan(s) => assert_eq!(s.message, "Expected identifier body."),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_interpolated_identifier_body_non_ascii() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "caf\u{00E9}", Syntax::Scss);
        let interp = parser.interpolated_identifier().unwrap();
        assert_eq!(interp.as_plain(), Some("caf\u{00E9}"));
    }

    #[test]
    fn test_interpolated_identifier_body_escape() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "a\\62 c", Syntax::Scss);
        let interp = parser.interpolated_identifier().unwrap();
        assert_eq!(interp.as_plain(), Some("abc"));
    }

    #[test]
    fn test_interpolated_identifier_hash_without_brace() {
        let arena = Bump::new();
        let mut parser = make_stylesheet(&arena, "a#b", Syntax::Scss);
        let interp = parser.interpolated_identifier().unwrap();
        assert_eq!(interp.as_plain(), Some("a"));
    }
}
