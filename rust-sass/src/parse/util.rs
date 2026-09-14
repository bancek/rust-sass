// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/stylesheet.dart (lookahead predicates, _urlString, _publicIdentifier, _assertPublic, _addOrInject)
// go-source: go/value/parse_stylesheet_util.go

use crate::ast::sass::expression::Expression;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::common::ast_node::AstNode;
use crate::common::file_span::FileSpan;
use crate::common::span_scanner::SpanScanner;
use crate::url::SassUrl;
use crate::util::character;

use crate::parse::parser::{
    error_impl, identifier_impl, span_from_impl, string_impl, ParseResult, ParserState,
};
use crate::parse::stylesheet::StylesheetParser;

// ======================================================================
// Scanner-only lookahead predicates
// ======================================================================

/// Returns whether the scanner is immediately before a sequence of characters
/// that could be part of an identifier. The identifier may include interpolation.
///
/// Based on the CSS identifier-start algorithm, but assumes all backslashes
/// start escapes and treats interpolation as valid in an identifier.
/// Matches Dart's `_lookingAtInterpolatedIdentifier`.
pub(crate) fn looking_at_interpolated_identifier_impl(scanner: &SpanScanner<'_>) -> bool {
    match scanner.peek_char(0) {
        ch if ch < 0 => false,
        ch if character::is_name_start(char::from_u32(ch as u32).unwrap_or('\0'))
            || ch == '\\' as i32 =>
        {
            true
        }
        ch if ch == '#' as i32 => scanner.peek_char(1) == '{' as i32,
        ch if ch == '-' as i32 => {
            let next = scanner.peek_char(1);
            if next < 0 {
                return false;
            }
            if next == '#' as i32 && scanner.peek_char(2) == '{' as i32 {
                return true;
            }
            character::is_name_start(char::from_u32(next as u32).unwrap_or('\0'))
                || next == '\\' as i32
                || next == '-' as i32
        }
        _ => false,
    }
}

/// Returns whether the scanner is immediately before a character that could
/// start a `*prop: val`, `:prop: val`, `#prop: val`, or `.prop: val` hack.
pub(crate) fn looking_at_potential_property_hack_impl(scanner: &SpanScanner<'_>) -> bool {
    match scanner.peek_char(0) {
        ch if ch == ':' as i32 || ch == '*' as i32 || ch == '.' as i32 => true,
        ch if ch == '#' as i32 => scanner.peek_char(1) != '{' as i32,
        _ => false,
    }
}

/// Returns whether the scanner is immediately before a sequence of characters
/// that could be part of a CSS identifier body. May include interpolation.
///
/// Matches Dart's `_lookingAtInterpolatedIdentifierBody`.
pub(crate) fn looking_at_interpolated_identifier_body_impl(scanner: &SpanScanner<'_>) -> bool {
    match scanner.peek_char(0) {
        ch if ch < 0 => false,
        ch if character::is_name(char::from_u32(ch as u32).unwrap_or('\0'))
            || ch == '\\' as i32 =>
        {
            true
        }
        ch if ch == '#' as i32 => scanner.peek_char(1) == '{' as i32,
        _ => false,
    }
}

/// Returns whether the scanner is immediately before a SassScript expression.
///
/// Matches Dart's `_lookingAtExpression`.
pub(crate) fn looking_at_expression_impl(scanner: &SpanScanner<'_>) -> bool {
    match scanner.peek_char(0) {
        ch if ch < 0 => false,
        ch if ch == '.' as i32 => scanner.peek_char(1) != '.' as i32,
        ch if ch == '!' as i32 => {
            let next = scanner.peek_char(1);
            next < 0
                || next == 'i' as i32
                || next == 'I' as i32
                || character::is_whitespace((next as u8) as char)
        }
        ch if ch == '(' as i32
            || ch == '/' as i32
            || ch == '[' as i32
            || ch == '\'' as i32
            || ch == '"' as i32
            || ch == '#' as i32
            || ch == '+' as i32
            || ch == '-' as i32
            || ch == '\\' as i32
            || ch == '$' as i32
            || ch == '&' as i32
            || ch == '%' as i32 =>
        {
            true
        }
        ch if character::is_name_start(char::from_u32(ch as u32).unwrap_or('\0'))
            || character::is_digit((ch as u8) as char) =>
        {
            true
        }
        _ => false,
    }
}

// ======================================================================
// url_string
// ======================================================================

/// Consumes a quoted string that contains a valid URL.
///
/// Reports "Invalid URL: ..." spanning from the opening quote on failure.
/// Matches Dart's `_urlString`.
pub(crate) fn url_string_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    parser_state: &ParserState<'parse>,
) -> ParseResult<'parse, SassUrl> {
    let start = scanner.state();
    let url_str = string_impl(scanner)?;
    let parsed = SassUrl::parse(&url_str);
    match parsed {
        Ok(url) => Ok(url),
        Err(e) => {
            let span = span_from_impl(scanner, parser_state, start)?;
            let file_span = span.file_span()?;
            Err(Box::new(error_impl(
                &format!("Invalid URL: {e}"),
                &file_span,
            )))
        }
    }
}

// ======================================================================
// public_identifier
// ======================================================================

/// Like [`identifier_impl`][crate::parse::parser::identifier_impl], but rejects identifiers that begin with `_` or `-`.
///
/// Matches Dart's `_publicIdentifier`.
pub(crate) fn public_identifier_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    parser_state: &ParserState<'parse>,
) -> ParseResult<'parse, String> {
    let start = scanner.state();
    let result = identifier_impl(scanner, parser_state, true, false)?;
    assert_public_impl(&result, || {
        let span = span_from_impl(scanner, parser_state, start).ok()?;
        span.file_span().ok()
    })?;
    Ok(result)
}

// ======================================================================
// assert_public
// ======================================================================

/// Errors if `identifier` is private (starts with `_` or `-`).
///
/// The span is provided lazily by the caller so no span is built on the
/// success path. Matches Dart's `_assertPublic`.
pub(crate) fn assert_public_impl<'parse>(
    identifier: &str,
    span: impl FnOnce() -> Option<FileSpan<'parse>>,
) -> ParseResult<'parse, ()> {
    if character::is_private(identifier) {
        if let Some(file_span) = span() {
            return Err(Box::new(error_impl(
                "Private members can't be accessed from outside their modules.",
                &file_span,
            )));
        }
    }
    Ok(())
}

// ======================================================================
// add_or_inject
// ======================================================================

/// Adds `expression` to `buffer`, or if it's an unquoted string adds the
/// interpolation it contains instead.
///
/// Matches Dart's `_addOrInject`.
pub(crate) fn add_or_inject_impl<'parse>(
    buffer: &mut InterpolationBuffer<'parse>,
    expression: &Expression<'parse>,
) -> ParseResult<'parse, ()> {
    if let Expression::String(se) = expression {
        if !se.has_quotes {
            buffer.add_interpolation(&se.text);
            return Ok(());
        }
    }
    let span = expression.span()?;
    buffer.add(expression.clone(), span);
    Ok(())
}

// ======================================================================
// Thin wrappers on StylesheetParser
// ======================================================================

impl<'parse> StylesheetParser<'parse> {
    pub fn looking_at_interpolated_identifier(&self) -> bool {
        looking_at_interpolated_identifier_impl(&self.scanner)
    }

    pub fn looking_at_potential_property_hack(&self) -> bool {
        looking_at_potential_property_hack_impl(&self.scanner)
    }

    pub fn looking_at_interpolated_identifier_body(&self) -> bool {
        looking_at_interpolated_identifier_body_impl(&self.scanner)
    }

    pub fn looking_at_expression(&self) -> bool {
        looking_at_expression_impl(&self.scanner)
    }

    pub fn url_string(&mut self) -> ParseResult<'parse, SassUrl> {
        url_string_impl(&mut self.scanner, &self.state.parser_state)
    }

    pub fn public_identifier(&mut self) -> ParseResult<'parse, String> {
        public_identifier_impl(&mut self.scanner, &self.state.parser_state)
    }

    pub fn assert_public(&self, identifier: &str) -> ParseResult<'parse, ()> {
        let span = self.scanner.empty_span();
        assert_public_impl(identifier, move || Some(span))
    }

    pub fn add_or_inject(
        buffer: &mut InterpolationBuffer<'parse>,
        expression: &Expression<'parse>,
    ) -> ParseResult<'parse, ()> {
        add_or_inject_impl(buffer, expression)
    }
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::expression_string::StringExpression;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::common::source_span_file_source::FileSource;
    use crate::parse::parser::ParseError;
    use crate::parse::stylesheet::Syntax;
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

    fn make_scss<'compile, 'parse>(arena: &'compile Bump, text: &str) -> StylesheetParser<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        make_stylesheet(arena, text, Syntax::Scss)
    }

    // --- looking_at_interpolated_identifier ---

    #[test]
    fn test_looking_at_interpolated_identifier_name_start() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "foo");
        assert!(parser.looking_at_interpolated_identifier());
    }

    #[test]
    fn test_looking_at_interpolated_identifier_backslash() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "\\41");
        assert!(parser.looking_at_interpolated_identifier());
    }

    #[test]
    fn test_looking_at_interpolated_identifier_hash_brace() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "#{");
        assert!(parser.looking_at_interpolated_identifier());
    }

    #[test]
    fn test_looking_at_interpolated_identifier_dash_name_start() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "-a");
        assert!(parser.looking_at_interpolated_identifier());
    }

    #[test]
    fn test_looking_at_interpolated_identifier_dash_hash_brace() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "-#{");
        assert!(parser.looking_at_interpolated_identifier());
    }

    #[test]
    fn test_looking_at_interpolated_identifier_double_dash() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "--");
        assert!(parser.looking_at_interpolated_identifier());
    }

    #[test]
    fn test_looking_at_interpolated_identifier_non_match() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "123");
        assert!(!parser.looking_at_interpolated_identifier());
    }

    #[test]
    fn test_looking_at_interpolated_identifier_non_match_hash() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "#foo");
        assert!(!parser.looking_at_interpolated_identifier());
    }

    #[test]
    fn test_looking_at_interpolated_identifier_eof() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "");
        assert!(!parser.looking_at_interpolated_identifier());
    }

    // --- looking_at_potential_property_hack ---

    #[test]
    fn test_looking_at_potential_property_hack_colon() {
        let arena = Bump::new();
        let parser = make_scss(&arena, ":");
        assert!(parser.looking_at_potential_property_hack());
    }

    #[test]
    fn test_looking_at_potential_property_hack_asterisk() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "*");
        assert!(parser.looking_at_potential_property_hack());
    }

    #[test]
    fn test_looking_at_potential_property_hack_dot() {
        let arena = Bump::new();
        let parser = make_scss(&arena, ".");
        assert!(parser.looking_at_potential_property_hack());
    }

    #[test]
    fn test_looking_at_potential_property_hack_hash() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "#foo");
        assert!(parser.looking_at_potential_property_hack());
    }

    #[test]
    fn test_looking_at_potential_property_hack_hash_brace() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "#{");
        assert!(!parser.looking_at_potential_property_hack());
    }

    #[test]
    fn test_looking_at_potential_property_hack_non_match() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "foo");
        assert!(!parser.looking_at_potential_property_hack());
    }

    // --- looking_at_interpolated_identifier_body ---

    #[test]
    fn test_looking_at_interpolated_identifier_body_name() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "foo");
        assert!(parser.looking_at_interpolated_identifier_body());
    }

    #[test]
    fn test_looking_at_interpolated_identifier_body_backslash() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "\\41");
        assert!(parser.looking_at_interpolated_identifier_body());
    }

    #[test]
    fn test_looking_at_interpolated_identifier_body_hash_brace() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "#{");
        assert!(parser.looking_at_interpolated_identifier_body());
    }

    #[test]
    fn test_looking_at_interpolated_identifier_body_non_match() {
        let arena = Bump::new();
        let parser = make_scss(&arena, ".");
        assert!(!parser.looking_at_interpolated_identifier_body());
    }

    #[test]
    fn test_looking_at_interpolated_identifier_body_eof() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "");
        assert!(!parser.looking_at_interpolated_identifier_body());
    }

    // --- looking_at_expression ---

    #[test]
    fn test_looking_at_expression_dot() {
        let arena = Bump::new();
        let parser = make_scss(&arena, ".5");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_double_dot() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "..");
        assert!(!parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_exclamation() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "!");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_exclamation_i() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "!important");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_exclamation_whitespace() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "! ");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_paren() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "(");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_slash() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "/");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_bracket() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "[");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_single_quote() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "'");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_double_quote() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "\"");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_hash() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "#");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_plus() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "+");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_minus() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "-");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_backslash() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "\\");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_dollar() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "$");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_ampersand() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "&");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_percent() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "%");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_name_start() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "a");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_digit() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "1");
        assert!(parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_non_match() {
        let arena = Bump::new();
        let parser = make_scss(&arena, ";");
        assert!(!parser.looking_at_expression());
    }

    #[test]
    fn test_looking_at_expression_eof() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "");
        assert!(!parser.looking_at_expression());
    }

    // --- url_string ---

    #[test]
    fn test_url_string_valid() {
        let arena = Bump::new();
        let mut parser = make_scss(&arena, "\"https://example.com/style.scss\"");
        let url = parser.url_string().unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.as_url().host_str(), Some("example.com"));
        assert_eq!(url.path(), "/style.scss");
    }

    #[test]
    fn test_url_string_invalid() {
        let arena = Bump::new();
        let mut parser = make_scss(&arena, "\"http://\"");
        let result = parser.url_string();
        assert!(result.is_err());
        match *result.unwrap_err() {
            ParseError::Format(f) => assert!(f.message.starts_with("Invalid URL:")),
            _ => panic!("expected Format error"),
        }
    }

    #[test]
    fn test_url_string_sass_scheme() {
        let arena = Bump::new();
        let mut parser = make_scss(&arena, "\"sass:color\"");
        let url = parser.url_string().unwrap();
        assert_eq!(url.scheme(), "sass");
        assert_eq!(url.path(), "color");
    }

    // --- public_identifier ---

    #[test]
    fn test_public_identifier_valid() {
        let arena = Bump::new();
        let mut parser = make_scss(&arena, "myvar");
        let result = parser.public_identifier().unwrap();
        assert_eq!(result, "myvar");
    }

    #[test]
    fn test_public_identifier_underscore_prefix() {
        let arena = Bump::new();
        let mut parser = make_scss(&arena, "_myvar");
        let result = parser.public_identifier();
        assert!(result.is_err());
        match *result.unwrap_err() {
            ParseError::Format(f) => assert_eq!(
                f.message,
                "Private members can't be accessed from outside their modules."
            ),
            _ => panic!("expected Format error"),
        }
    }

    #[test]
    fn test_public_identifier_dash_prefix() {
        let arena = Bump::new();
        let mut parser = make_scss(&arena, "-myvar");
        let result = parser.public_identifier();
        assert!(result.is_err());
        match *result.unwrap_err() {
            ParseError::Format(f) => assert_eq!(
                f.message,
                "Private members can't be accessed from outside their modules."
            ),
            _ => panic!("expected Format error"),
        }
    }

    // --- assert_public ---

    #[test]
    fn test_assert_public_public() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "");
        let result = parser.assert_public("myvar");
        assert!(result.is_ok());
    }

    #[test]
    fn test_assert_public_private() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "");
        let result = parser.assert_public("_myvar");
        assert!(result.is_err());
        match *result.unwrap_err() {
            ParseError::Format(f) => assert_eq!(
                f.message,
                "Private members can't be accessed from outside their modules."
            ),
            _ => panic!("expected Format error"),
        }
    }

    #[test]
    fn test_assert_public_private_dash() {
        let arena = Bump::new();
        let parser = make_scss(&arena, "");
        let result = parser.assert_public("-myvar");
        assert!(result.is_err());
        match *result.unwrap_err() {
            ParseError::Format(f) => assert_eq!(
                f.message,
                "Private members can't be accessed from outside their modules."
            ),
            _ => panic!("expected Format error"),
        }
    }

    // --- add_or_inject ---

    #[test]
    fn test_add_or_inject_unquoted_string() {
        let arena = Bump::new();
        let span = {
            let fs = FileSource::new_in(&arena, "hello", None);
            FileSpan::new(Some(fs), 0, 5)
        };
        let interp = Interpolation::plain("hello".into(), span);
        let expr = Expression::String(StringExpression::new(interp, false));

        let mut buffer = InterpolationBuffer::new();
        StylesheetParser::add_or_inject(&mut buffer, &expr).unwrap();
        let result = buffer.interpolation(span).unwrap();
        assert!(result.is_plain());
        assert_eq!(result.as_plain(), Some("hello"));
    }

    #[test]
    fn test_add_or_inject_quoted_string() {
        let arena = Bump::new();
        let span = {
            let fs = FileSource::new_in(&arena, "hello", None);
            FileSpan::new(Some(fs), 0, 5)
        };
        let interp = Interpolation::plain("hello".into(), span);
        let expr = Expression::String(StringExpression::new(interp, true));

        let mut buffer = InterpolationBuffer::new();
        StylesheetParser::add_or_inject(&mut buffer, &expr).unwrap();
        let result = buffer.interpolation(span).unwrap();
        assert!(!result.is_plain());
    }

    #[test]
    fn test_add_or_inject_interpolation_expression() {
        let arena = Bump::new();
        let span = {
            let fs = FileSource::new_in(&arena, "hello", None);
            FileSpan::new(Some(fs), 0, 5)
        };
        let interp = Interpolation::plain("hello".into(), span);
        let expr = Expression::String(StringExpression::new(interp, false));

        let mut buffer = InterpolationBuffer::new();
        StylesheetParser::add_or_inject(&mut buffer, &expr).unwrap();
        assert!(!buffer.is_empty());
    }
}
