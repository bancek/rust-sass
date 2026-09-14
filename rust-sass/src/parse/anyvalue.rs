// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/stylesheet.dart (almostAnyValue, _interpolatedDeclarationValue)
// go-source: go/value/parse_stylesheet_anyvalue.go

use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::common::span_scanner::SpanScanner;
use crate::util::character;

use crate::parse::expression::{interpolated_string_token_impl, try_url_contents};
use crate::parse::identifier::interpolated_identifier_impl;
use crate::parse::parser::{
    escape_impl, identifier_impl, looking_at_identifier, loud_comment_impl, raw_text_impl,
    scan_identifier_impl, silent_comment_impl, span_from_impl, ParseError, ParseResult,
};
use crate::parse::stylesheet::{StylesheetParser, StylesheetState, Syntax};

// ======================================================================
// DeclarationValueOpts
// ======================================================================

/// Options controlling behavior of [`interpolated_declaration_value_impl`].
///
/// Each field mirrors a named parameter of Dart's
/// `_interpolatedDeclarationValue`, with the same defaults.
#[derive(Debug, Clone)]
pub struct DeclarationValueOpts {
    /// Fails with "Expected token." when the value is empty.
    pub allow_empty: bool,
    /// Includes `;` in the output instead of stopping at top-level semicolons.
    pub allow_semicolon: bool,
    /// Includes top-level `:` in the output; when `false`, stops before them.
    pub allow_colon: bool,
    /// Includes `{` in the output; when `false`, stops before opening braces.
    pub allow_open_brace: bool,
    /// Stops *after consuming* a top-level `of` identifier (grid syntax).
    pub end_after_of: bool,
    /// Parses `//` as silent comments; otherwise preserves the slashes as text.
    pub silent_comments: bool,
    /// Lets the indented syntax consume newlines as whitespace. Only set where
    /// a statement can't end.
    pub consume_newlines: bool,
}

impl Default for DeclarationValueOpts {
    fn default() -> Self {
        DeclarationValueOpts {
            allow_empty: false,
            allow_semicolon: false,
            allow_colon: true,
            allow_open_brace: true,
            end_after_of: false,
            silent_comments: true,
            consume_newlines: false,
        }
    }
}

// ======================================================================
// almost_any_value_impl
// ======================================================================

/// Consumes tokens until a statement terminator (`!`, `;`, `{`, `}`) and
/// returns their contents as interpolated text.
///
/// Once evaluated, the result is expected to be re-parsed, so backslashes are
/// copied literally (no escape decoding) and adjacent whitespace is not
/// compressed. Newlines end the value in the indented syntax unless nested in
/// brackets. Matches Dart's `almostAnyValue`; the `omit_comments` flag still
/// consumes comments but leaves them out of the returned interpolation.
pub(crate) fn almost_any_value_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    omit_comments: bool,
) -> ParseResult<'b, Interpolation<'b>> {
    let start = scanner.state();
    let mut buffer = InterpolationBuffer::new();
    let mut brackets: Vec<char> = Vec::new();

    loop {
        let ch = scanner.peek_char(0);
        match ch {
            c if c == '\\' as i32 => {
                let c = scanner.read_char()?;
                buffer.write_char_code(c);
                let c = scanner.read_char()?;
                buffer.write_char_code(c);
            }

            c if c == '\'' as i32 || c == '"' as i32 => {
                let interp = interpolated_string_token_impl(scanner, state)?;
                buffer.add_interpolation(&interp);
            }

            c if c == '/' as i32 => match scanner.peek_char(1) as u8 as char {
                '*' => {
                    if !omit_comments {
                        let (_, text) =
                            raw_text_impl(scanner, &mut state.parser_state, |s, _st| {
                                loud_comment_impl(s)?;
                                Ok(())
                            })?;
                        buffer.write(&text);
                    } else {
                        loud_comment_impl(scanner)?;
                    }
                }
                '/' => {
                    if !omit_comments {
                        let (_, text) =
                            raw_text_impl(scanner, &mut state.parser_state, |s, st| {
                                let _ = silent_comment_impl(s, st)?;
                                Ok(())
                            })?;
                        buffer.write(&text);
                    } else {
                        let _ = silent_comment_impl(scanner, &state.parser_state)?;
                    }
                }
                _ => {
                    let c = scanner.read_char()?;
                    buffer.write_char_code(c);
                }
            },

            c if c == '#' as i32 && scanner.peek_char(1) == '{' as i32 => {
                let interp = interpolated_identifier_impl(scanner, state)?;
                buffer.add_interpolation(&interp);
            }

            c if c == '\n' as i32 || c == '\r' as i32 || c == 0x0C_i32 => {
                if matches!(state.parser_state.syntax, Syntax::Sass(_)) && brackets.is_empty() {
                    break;
                }
                let c = scanner.read_char()?;
                buffer.write_char_code(c);
            }

            c if c == '!' as i32 || c == ';' as i32 || c == '{' as i32 || c == '}' as i32 => {
                break;
            }

            c if c == 'u' as i32 || c == 'U' as i32 => {
                let before_url = scanner.state();
                let ident = identifier_impl(scanner, &state.parser_state, false, false)?;
                if ident != "url" && ident != "url-prefix" {
                    buffer.write(&ident);
                    continue;
                }
                if let Some(contents) = try_url_contents(scanner, state, before_url, &ident, false)?
                {
                    buffer.add_interpolation(&contents);
                } else {
                    scanner.set_state(before_url);
                    let c = scanner.read_char()?;
                    buffer.write_char_code(c);
                }
            }

            c if c == '(' as i32 || c == '[' as i32 => {
                let bracket = scanner.read_char()?;
                buffer.write_char_code(bracket);
                let opp = character::opposite(bracket)
                    .map_err(|e| ParseError::from(scanner.error(&e.to_string(), None, 0)))?;
                brackets.push(opp);
            }

            c if c == ')' as i32 || c == ']' as i32 => {
                if brackets.is_empty() {
                    let ch = c as u8 as char;
                    return Err(scanner
                        .error(&format!("Unexpected \"{ch}\"."), None, 0)
                        .into());
                }
                let expected = brackets.pop().unwrap();
                scanner.expect_char(expected)?;
                buffer.write_char_code(expected);
            }

            c if c < 0 => break,

            _ if looking_at_identifier(scanner, None) => {
                let ident = identifier_impl(scanner, &state.parser_state, false, false)?;
                buffer.write(&ident);
            }

            _ => {
                let c = scanner.read_char()?;
                buffer.write_char_code(c);
            }
        }
    }

    let span = span_from_impl(scanner, &state.parser_state, start)?;
    Ok(buffer.interpolation(span)?)
}

// ======================================================================
// interpolated_declaration_value_impl
// ======================================================================

/// Consumes tokens until a top-level `;`, `)`, `]`, or `}` and returns their
/// contents as an interpolated string.
///
/// Unlike plain declaration values this allows interpolation, and unlike
/// [`almost_any_value_impl`] it decodes escapes, tracks all three bracket
/// kinds, collapses whitespace and newlines, and honors the [`DeclarationValueOpts`]
/// stop conditions. Matches Dart's `_interpolatedDeclarationValue`.
pub(crate) fn interpolated_declaration_value_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    opts: DeclarationValueOpts,
) -> ParseResult<'b, Interpolation<'b>> {
    let start = scanner.state();
    let mut buffer = InterpolationBuffer::new();
    let mut brackets: Vec<char> = Vec::new();
    let mut wrote_newline = false;

    loop {
        let ch = scanner.peek_char(0);
        match ch {
            c if c == '\\' as i32 => {
                let s = escape_impl(scanner, true)?;
                buffer.write(&s);
                wrote_newline = false;
            }

            c if c == '\'' as i32 || c == '"' as i32 => {
                let interp = interpolated_string_token_impl(scanner, state)?;
                buffer.add_interpolation(&interp);
                wrote_newline = false;
            }

            c if c == '/' as i32 => match scanner.peek_char(1) as u8 as char {
                '*' => {
                    let (_, text) = raw_text_impl(scanner, &mut state.parser_state, |s, _st| {
                        loud_comment_impl(s)?;
                        Ok(())
                    })?;
                    buffer.write(&text);
                    wrote_newline = false;
                }
                '/' => {
                    if opts.silent_comments {
                        let _ = silent_comment_impl(scanner, &state.parser_state)?;
                        wrote_newline = false;
                    } else {
                        let c = scanner.read_char()?;
                        buffer.write_char_code(c);
                        wrote_newline = false;
                    }
                }
                _ => {
                    let c = scanner.read_char()?;
                    buffer.write_char_code(c);
                    wrote_newline = false;
                }
            },

            c if c == '#' as i32 && scanner.peek_char(1) == '{' as i32 => {
                let interp = interpolated_identifier_impl(scanner, state)?;
                buffer.add_interpolation(&interp);
                wrote_newline = false;
            }

            c if (c == ' ' as i32 || c == '\t' as i32)
                && !wrote_newline
                && character::is_whitespace(scanner.peek_char(1) as u8 as char) =>
            {
                let _ = scanner.read_char()?;
            }

            c if c == ' ' as i32 || c == '\t' as i32 => {
                let c = scanner.read_char()?;
                buffer.write_char_code(c);
            }

            c if (c == '\n' as i32 || c == '\r' as i32 || c == 0x0C_i32)
                && matches!(state.parser_state.syntax, Syntax::Sass(_))
                && !opts.consume_newlines
                && brackets.is_empty() =>
            {
                break;
            }

            c if c == '\n' as i32 || c == '\r' as i32 || c == 0x0C_i32 => {
                if !character::is_newline(scanner.peek_char(-1) as u8 as char) {
                    buffer.writeln("");
                }
                let _ = scanner.read_char()?;
                wrote_newline = true;
            }

            c if c == '{' as i32 && !opts.allow_open_brace => {
                break;
            }

            c if c == '(' as i32 || c == '{' as i32 || c == '[' as i32 => {
                let bracket = scanner.read_char()?;
                buffer.write_char_code(bracket);
                let opp = character::opposite(bracket)
                    .map_err(|e| ParseError::from(scanner.error(&e.to_string(), None, 0)))?;
                brackets.push(opp);
                wrote_newline = false;
            }

            c if c == ')' as i32 || c == '}' as i32 || c == ']' as i32 => {
                if brackets.is_empty() {
                    break;
                }
                let expected = brackets.pop().unwrap();
                scanner.expect_char(expected)?;
                buffer.write_char_code(expected);
                wrote_newline = false;
            }

            c if c == ';' as i32 => {
                if !opts.allow_semicolon && brackets.is_empty() {
                    break;
                }
                let c = scanner.read_char()?;
                buffer.write_char_code(c);
                wrote_newline = false;
            }

            c if c == ':' as i32 => {
                if !opts.allow_colon && brackets.is_empty() {
                    break;
                }
                let c = scanner.read_char()?;
                buffer.write_char_code(c);
                wrote_newline = false;
            }

            c if c == 'u' as i32 || c == 'U' as i32 => {
                let before_url = scanner.state();
                let ident = identifier_impl(scanner, &state.parser_state, false, false)?;
                if ident != "url" && ident != "url-prefix" {
                    buffer.write(&ident);
                    wrote_newline = false;
                    continue;
                }
                if let Some(contents) = try_url_contents(scanner, state, before_url, &ident, false)?
                {
                    buffer.add_interpolation(&contents);
                } else {
                    scanner.set_state(before_url);
                    let c = scanner.read_char()?;
                    buffer.write_char_code(c);
                }
                wrote_newline = false;
            }

            c if c == 'o' as i32 || c == 'O' as i32 => {
                if opts.end_after_of && brackets.is_empty() {
                    let (matched, text) =
                        raw_text_impl(scanner, &mut state.parser_state, |s, _st| {
                            scan_identifier_impl(s, "of", false)
                        })?;
                    if matched {
                        buffer.write(&text);
                        break;
                    }
                }
                let c = scanner.read_char()?;
                buffer.write_char_code(c);
                wrote_newline = false;
            }

            c if c < 0 => break,

            _ if looking_at_identifier(scanner, None) => {
                let ident = identifier_impl(scanner, &state.parser_state, false, false)?;
                buffer.write(&ident);
                wrote_newline = false;
            }

            _ => {
                let c = scanner.read_char()?;
                buffer.write_char_code(c);
                wrote_newline = false;
            }
        }
    }

    if let Some(&expected) = brackets.last() {
        scanner.expect_char(expected)?;
    }
    if !opts.allow_empty && buffer.is_empty() {
        return Err(Box::new(scanner.error("Expected token.", None, 0).into()));
    }
    let span = span_from_impl(scanner, &state.parser_state, start)?;
    Ok(buffer.interpolation(span)?)
}

// ======================================================================
// Thin wrappers on StylesheetParser
// ======================================================================

impl<'parse> StylesheetParser<'parse> {
    pub fn almost_any_value(
        &mut self,
        omit_comments: bool,
    ) -> ParseResult<'parse, Interpolation<'parse>> {
        almost_any_value_impl(&mut self.scanner, &mut self.state, omit_comments)
    }

    pub fn interpolated_declaration_value(
        &mut self,
        opts: DeclarationValueOpts,
    ) -> ParseResult<'parse, Interpolation<'parse>> {
        interpolated_declaration_value_impl(&mut self.scanner, &mut self.state, opts)
    }
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::source_span_file_source::FileSource;
    use crate::parse::parser::{ParseError, ParserState};
    use crate::parse::stylesheet::SassIndentState;
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
            warnings: Vec::new(),
            last_silent_comment: None,
        };
        (scanner, state)
    }

    fn make_sass_state<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
    ) -> (SpanScanner<'parse>, StylesheetState<'parse>)
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let (scanner, mut state) = make_state(arena, text);
        state.parser_state.syntax = Syntax::Sass(SassIndentState {
            current_indentation: 0,
            next_indentation: None,
            next_indentation_end: None,
            indent_spaces: None,
        });
        (scanner, state)
    }

    // =========================================================================
    // almost_any_value tests
    // =========================================================================

    #[test]
    fn test_almost_any_value_plain_text() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "foo");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        assert_eq!(interp.as_plain(), Some("foo"));
    }

    #[test]
    fn test_almost_any_value_quoted_string() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "\"hello\"");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "\"hello\"");
    }

    #[test]
    fn test_almost_any_value_single_quoted() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "'world'");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "'world'");
    }

    #[test]
    fn test_almost_any_value_interpolation() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "#{foo}");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        assert!(!interp.is_plain());
    }

    #[test]
    fn test_almost_any_value_escape_backslash() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "\\a");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        assert_eq!(interp.as_plain(), Some("\\a"));
    }

    #[test]
    fn test_almost_any_value_loud_comment_included() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a/* comment */b");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "a/* comment */b");
    }

    #[test]
    fn test_almost_any_value_loud_comment_omitted() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a/* comment */b");
        let interp = almost_any_value_impl(&mut s, &mut st, true).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "ab");
    }

    #[test]
    fn test_almost_any_value_silent_comment_included() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a// comment\nb");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "a// comment\nb");
    }

    #[test]
    fn test_almost_any_value_silent_comment_omitted() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a// comment\nb");
        let interp = almost_any_value_impl(&mut s, &mut st, true).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "a\nb");
    }

    #[test]
    fn test_almost_any_value_url_function() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "url(foo)");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "url(foo)");
    }

    #[test]
    fn test_almost_any_value_url_prefix_function() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "url-prefix(foo)");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "url-prefix(foo)");
    }

    #[test]
    fn test_almost_any_value_u_not_url() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "underline");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "underline");
    }

    #[test]
    fn test_almost_any_value_terminates_on_bang() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "foo!bar");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "foo");
    }

    #[test]
    fn test_almost_any_value_terminates_on_semicolon() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "foo;bar");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "foo");
    }

    #[test]
    fn test_almost_any_value_terminates_on_open_brace() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "foo{bar");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "foo");
    }

    #[test]
    fn test_almost_any_value_terminates_on_close_brace() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "foo}bar");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "foo");
    }

    #[test]
    fn test_almost_any_value_bracket_tracking() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "(foo)bar");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "(foo)bar");
    }

    #[test]
    fn test_almost_any_value_square_bracket_tracking() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[foo]bar");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "[foo]bar");
    }

    #[test]
    fn test_almost_any_value_open_brace_terminates_even_in_parens() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "(foo{bar})baz");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "(foo");
    }

    #[test]
    fn test_almost_any_value_mixed_brackets() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "([x])");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "([x])");
    }

    #[test]
    fn test_almost_any_value_newline_scss() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello\nworld");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello\nworld");
    }

    #[test]
    fn test_almost_any_value_newline_indented_terminates() {
        let arena = Bump::new();
        let (mut s, mut st) = make_sass_state(&arena, "hello\nworld");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_almost_any_value_newline_indented_inside_brackets() {
        let arena = Bump::new();
        let (mut s, mut st) = make_sass_state(&arena, "(hello\nworld)");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "(hello\nworld)");
    }

    #[test]
    fn test_almost_any_value_empty() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        assert_eq!(interp.as_plain(), Some(""));
    }

    #[test]
    fn test_almost_any_value_identifier() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "my-var");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "my-var");
    }

    #[test]
    fn test_almost_any_value_unexpected_close_paren_error() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ")");
        let err = almost_any_value_impl(&mut s, &mut st, false).unwrap_err();
        match &*err {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "Unexpected \")\".");
            }
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    #[test]
    fn test_almost_any_value_unexpected_close_bracket_error() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "]");
        let err = almost_any_value_impl(&mut s, &mut st, false).unwrap_err();
        match &*err {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "Unexpected \"]\".");
            }
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    #[test]
    fn test_almost_any_value_empty_parens_no_close() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "(");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "(");
    }

    #[test]
    fn test_almost_any_value_nested_brackets() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "((foo))");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "((foo))");
    }

    #[test]
    fn test_almost_any_value_slash() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "/");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "/");
    }

    #[test]
    fn test_almost_any_value_special_chars() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "@%.");
        let interp = almost_any_value_impl(&mut s, &mut st, false).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "@%.");
    }

    // =========================================================================
    // interpolated_declaration_value tests
    // =========================================================================

    fn default_opts() -> DeclarationValueOpts {
        DeclarationValueOpts::default()
    }

    #[test]
    fn test_interpolated_declaration_value_basic() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_interpolated_declaration_value_colon_terminates_when_disallowed() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello:world");
        let opts = DeclarationValueOpts {
            allow_colon: false,
            allow_open_brace: true,
            silent_comments: true,
            ..Default::default()
        };
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, opts).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_interpolated_declaration_value_colon_allowed_by_default() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello:world");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello:world");
    }

    #[test]
    fn test_interpolated_declaration_value_colon_inside_brackets() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "(a:b)");
        let opts = DeclarationValueOpts {
            allow_colon: false,
            allow_open_brace: true,
            silent_comments: true,
            ..Default::default()
        };
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, opts).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "(a:b)");
    }

    #[test]
    fn test_interpolated_declaration_value_semicolon_terminates() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello;world");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_interpolated_declaration_value_semicolon_allowed_when_true() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello;world");
        let opts = DeclarationValueOpts {
            allow_semicolon: true,
            allow_colon: true,
            allow_open_brace: true,
            silent_comments: true,
            ..Default::default()
        };
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, opts).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello;world");
    }

    #[test]
    fn test_interpolated_declaration_value_open_brace_terminates_when_disallowed() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello{world");
        let opts = DeclarationValueOpts {
            allow_open_brace: false,
            allow_colon: true,
            silent_comments: true,
            ..Default::default()
        };
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, opts).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_interpolated_declaration_value_open_brace_allowed_by_default() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello+world");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello+world");
    }

    #[test]
    fn test_interpolated_declaration_value_open_brace_inside_parens() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "(a{b})");
        let opts = DeclarationValueOpts {
            allow_open_brace: true,
            allow_colon: true,
            silent_comments: true,
            ..Default::default()
        };
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, opts).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "(a{b})");
    }

    #[test]
    fn test_interpolated_declaration_value_space_collapsing() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello   world");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn test_interpolated_declaration_value_space_collapse_tab_sequence() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello\t \tworld");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello\tworld");
    }

    #[test]
    fn test_interpolated_declaration_value_space_not_collapsed_after_newline() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello\n  world");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello\n  world");
    }

    #[test]
    fn test_interpolated_declaration_value_escape() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "\\61 bc");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "abc");
    }

    #[test]
    fn test_interpolated_declaration_value_quoted_string() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "\"hello\"");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "\"hello\"");
    }

    #[test]
    fn test_interpolated_declaration_value_interpolation() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "before#{x}after");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "before#{x}after");
    }

    #[test]
    fn test_interpolated_declaration_value_loud_comment() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a/* comment */b");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "a/* comment */b");
    }

    #[test]
    fn test_interpolated_declaration_value_silent_comment_omitted() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello// comment\nworld");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello\nworld");
    }

    #[test]
    fn test_interpolated_declaration_value_silent_comment_passed_as_text() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello// comment\nworld");
        let opts = DeclarationValueOpts {
            allow_colon: true,
            allow_open_brace: true,
            silent_comments: false,
            ..Default::default()
        };
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, opts).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello// comment\nworld");
    }

    #[test]
    fn test_interpolated_declaration_value_url_function() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "url(http://x.com)");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "url(http://x.com)");
    }

    #[test]
    fn test_interpolated_declaration_value_url_prefix_function() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "url-prefix(http://)");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "url-prefix(http://)");
    }

    #[test]
    fn test_interpolated_declaration_value_end_after_of_terminates() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "1 of 2");
        let opts = DeclarationValueOpts {
            allow_colon: true,
            allow_open_brace: true,
            silent_comments: true,
            end_after_of: true,
            ..Default::default()
        };
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, opts).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "1 of");
    }

    #[test]
    fn test_interpolated_declaration_value_end_after_of_no_match() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "offset");
        let opts = DeclarationValueOpts {
            allow_colon: true,
            allow_open_brace: true,
            silent_comments: true,
            end_after_of: true,
            ..Default::default()
        };
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, opts).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "offset");
    }

    #[test]
    fn test_interpolated_declaration_value_end_after_of_inside_brackets() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "(1 of 2)");
        let opts = DeclarationValueOpts {
            allow_colon: true,
            allow_open_brace: true,
            silent_comments: true,
            end_after_of: true,
            ..Default::default()
        };
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, opts).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "(1 of 2)");
    }

    #[test]
    fn test_interpolated_declaration_value_allow_empty_false_error() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "");
        let err =
            interpolated_declaration_value_impl(&mut s, &mut st, Default::default()).unwrap_err();
        match &*err {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "Expected token.");
            }
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    #[test]
    fn test_interpolated_declaration_value_allow_empty_true() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "");
        let opts = DeclarationValueOpts {
            allow_empty: true,
            allow_colon: true,
            allow_open_brace: true,
            silent_comments: true,
            ..Default::default()
        };
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, opts).unwrap();
        assert_eq!(interp.as_plain(), Some(""));
    }

    #[test]
    fn test_interpolated_declaration_value_nested_brackets() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "(a (b) c)");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "(a (b) c)");
    }

    #[test]
    fn test_interpolated_declaration_value_mixed_bracket_types() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "([x])");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "([x])");
    }

    #[test]
    fn test_interpolated_declaration_value_newline_scss() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "hello\nworld");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello\nworld");
    }

    #[test]
    fn test_interpolated_declaration_value_newline_indented_terminates() {
        let arena = Bump::new();
        let (mut s, mut st) = make_sass_state(&arena, "hello\nworld");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_interpolated_declaration_value_newline_indented_consume_newlines_true() {
        let arena = Bump::new();
        let (mut s, mut st) = make_sass_state(&arena, "hello\nworld");
        let opts = DeclarationValueOpts {
            allow_colon: true,
            allow_open_brace: true,
            silent_comments: true,
            consume_newlines: true,
            ..Default::default()
        };
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, opts).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "hello\nworld");
    }

    #[test]
    fn test_interpolated_declaration_value_newline_indented_inside_brackets() {
        let arena = Bump::new();
        let (mut s, mut st) = make_sass_state(&arena, "(hello\nworld)");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "(hello\nworld)");
    }

    #[test]
    fn test_interpolated_declaration_value_consecutive_newlines_collapsed() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a\n\nb");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "a\nb");
    }

    #[test]
    fn test_interpolated_declaration_value_unclosed_bracket_error() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "(");
        let err = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap_err();
        match &*err {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "expected \")\".");
            }
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    #[test]
    fn test_interpolated_declaration_value_semicolon_inside_brackets() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "(a;b)");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "(a;b)");
    }

    #[test]
    fn test_interpolated_declaration_value_complex_mixed() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a url(x) \"b\" c/* comment */d #{e} f");
        let interp = interpolated_declaration_value_impl(&mut s, &mut st, default_opts()).unwrap();
        let text = interp.to_display_string().unwrap();
        assert_eq!(text, "a url(x) \"b\" c/* comment */d #{e} f");
    }
}
