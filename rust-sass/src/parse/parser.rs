// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/parser.dart
// go-source: go/value/parse_parser.go
//
// Backend note: this file ports only `parser.dart`'s token-level base parser
// (whitespace, comments, identifiers, strings, numbers, escapes, lookahead,
// spans, error wrapping). Whitespace/comment overrides and all statement-level
// parsing dispatch on `Syntax` (SCSS/Sass/CSS) via match arms in
// `parse/stylesheet.rs` / `parse/{scss,sass,css}.rs`; there are no subclass
// hooks to document here.

use crate::ast::sass::interpolation_map::InterpolationMap;
use crate::common::core_errors::ArgumentError;
use crate::common::core_errors::RangeError;
use std::fmt::Display;
use std::fmt::Formatter;
use thiserror::Error;

use crate::common::exception::{SassError, SassResult, ScanError, SpanScannerError};
use crate::common::file_span::{FileSpan, SourceLocation};
use crate::common::source_span_file_source::FileSource;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::common::span::LazyFileSpan;
use crate::common::span::Span;
use crate::common::span_error::SpanError;
use crate::common::span_scanner::{LineScannerState, SpanScanner};
use crate::parse::stylesheet::Syntax;
use crate::util::character;
use crate::util::character::MAX_ALLOWED_CHARACTER;

/// The abstract base class for all parsers (Dart: `class Parser`), providing
/// utility methods and common token parsing.
///
/// A parse method throws unless specified otherwise; in Rust that failure is
/// a [`ParseError`] (converted to [`SassError`] at the
/// [`wrap_span_format_exception_impl`] boundary) rather than a thrown
/// `SassFormatException`.
///
/// Split into a scanner-carrying wrapper plus [`ParserState`]: all logic lives
/// in free `*_impl` functions taking `(&mut SpanScanner, &mut ParserState)`
/// separately, enabling closures that need simultaneous access to both.
///
/// (Borrow ergonomics: splitting scanner from state lets closure-heavy
/// callers such as [`Parser::raw_text`] hold both disjoint borrows at once
/// instead of re-borrowing `&mut self` twice.)
pub struct ParserState<'parse> {
    /// Which syntax is being parsed. Controls newline handling in
    /// [`Parser::whitespace_without_comments`] and silent-comment rejection.
    pub syntax: Syntax,
    /// Maps spans in the text being parsed back to their original locations
    /// when parsing generated (e.g. interpolated) source rather than source
    /// directly. `None` when parsing source text itself.
    pub interpolation_map: Option<&'parse InterpolationMap<'parse>>,
    /// Set while inside a plain-CSS expression context; makes
    /// [`Parser::silent_comment`] reject `//` with "Silent comments aren't
    /// allowed in plain CSS." instead of consuming it.
    pub in_expression: bool,
}

pub struct Parser<'parse> {
    /// The scanner that scans through the text being parsed.
    pub scanner: SpanScanner<'parse>,
    pub(crate) state: ParserState<'parse>,
}

impl<'parse> Parser<'parse> {
    /// Creates a parser over `source` for the given `syntax`, with an
    /// optional [`interpolation map`](ParserState::interpolation_map) for
    /// parsing generated (e.g. interpolated) text.
    ///
    /// Dart's protected `Parser(contents, {url, interpolationMap})` takes raw
    /// text; the Rust port takes an arena-allocated `FileSource` (which owns
    /// the text and URL) plus the `Syntax` that Dart expressed via subclasses.
    pub fn new(
        source: &'parse FileSource<'parse>,
        syntax: Syntax,
        interpolation_map: Option<&'parse InterpolationMap<'parse>>,
    ) -> Self {
        Parser {
            scanner: SpanScanner::new(source),
            state: ParserState {
                syntax,
                interpolation_map,
                in_expression: false,
            },
        }
    }
}

// ======================================================================
// Static free functions (no Parser needed)
// ======================================================================

/// Parses `text` as a CSS identifier and returns the result.
///
/// Throws unless the whole input is exactly one identifier (the trailing
/// `expect_done` check); in Rust the failure surfaces as `Err`.
pub fn parse_identifier(text: &str) -> SassResult<String> {
    let arena = bumpalo::Bump::new();
    let fs = FileSource::new_in(&arena, text, None);
    let mut parser = Parser::new(fs, Syntax::Scss, None);
    let result =
        wrap_span_format_exception_impl(&mut parser.scanner, &mut parser.state, |s, st| {
            let result = identifier_impl(s, st, false, false)?;
            s.expect_done()?;
            Ok(result)
        });
    match result {
        Ok(v) => Ok(v),
        Err(e) => Err(e.into()),
    }
}

/// Returns whether `text` is a valid CSS identifier.
pub fn is_identifier(text: &str) -> bool {
    parse_identifier(text).is_ok()
}

/// Returns whether `text` starts like a variable declaration (`$name:`).
///
/// Ignores everything after the `:`.
pub fn is_variable_declaration_like(text: &str) -> SassResult<bool> {
    let arena = bumpalo::Bump::new();
    let fs = FileSource::new_in(&arena, text, None);
    let mut parser = Parser::new(fs, Syntax::Scss, None);
    if !parser.scanner.scan_char('$') {
        return Ok(false);
    }
    if !looking_at_identifier(&parser.scanner, None) {
        return Ok(false);
    }
    if identifier_impl(&mut parser.scanner, &parser.state, false, false).is_err() {
        return Ok(false);
    }
    if whitespace_impl(&mut parser.scanner, &mut parser.state, true).is_err() {
        return Ok(false);
    }
    Ok(parser.scanner.scan_char(':'))
}

// ======================================================================
// Scanner delegates (thin wrappers)
// ======================================================================

impl<'parse> Parser<'parse> {
    /// Thin scanner delegate: consumes and returns the next character.
    pub fn read_char(&mut self) -> ParseResult<'parse, char> {
        Ok(self.scanner.read_char()?)
    }

    /// Thin scanner delegate: restores a saved offset (e.g. backtracking in
    /// [`Parser::try_url`]).
    pub fn set_position(&mut self, pos: usize) -> ParseResult<'parse, ()> {
        Ok(self.scanner.set_position(pos)?)
    }

    /// Thin scanner delegate: consumes `ch` or errors ("expected ...").
    pub fn expect_char(&mut self, ch: char) -> ParseResult<'parse, ()> {
        Ok(self.scanner.expect_char(ch)?)
    }

    /// Thin scanner delegate: consumes `ch` or errors with `name` ("expected
    /// {name}."). Has no Dart counterpart (Rust-side convenience used where
    /// Dart inlines `scanner.error` with a custom message).
    pub fn expect_char_name(&mut self, ch: char, name: &str) -> ParseResult<'parse, ()> {
        if self.scanner.scan_char(ch) {
            return Ok(());
        }
        Err(self
            .scanner
            .error(&format!("expected {name}."), None, 0)
            .into())
    }

    /// Thin scanner delegate: consumes the literal `s` or errors.
    pub fn expect(&mut self, s: &str) -> ParseResult<'parse, ()> {
        Ok(self.scanner.expect(s)?)
    }
}

// ======================================================================
// Tokens
// ======================================================================

// ## Tokens

/// Consumes whitespace, including any comments.
///
/// If `consume_newlines` is `true`, the indented syntax consumes newlines as
/// whitespace. Only set it in positions where a statement can't end.
pub(crate) fn whitespace_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
    consume_newlines: bool,
) -> ParseResult<'parse, ()> {
    loop {
        whitespace_without_comments_impl(scanner, state, consume_newlines)?;
        if !scan_comment_impl(scanner, state)? {
            break;
        }
    }
    Ok(())
}

impl<'parse> Parser<'parse> {
    /// Consumes whitespace, including any comments. See [`whitespace_impl`].
    pub fn whitespace(&mut self, consume_newlines: bool) -> ParseResult<'parse, ()> {
        whitespace_impl(&mut self.scanner, &mut self.state, consume_newlines)
    }
}

/// Consumes whitespace, but not comments.
///
/// If `consume_newlines` is `true`, the indented syntax consumes newlines as
/// whitespace. Only set it in positions where a statement can't end.
///
/// Syntax split (replaces Dart's subclass overrides): SCSS/CSS consume all
/// [`character::is_whitespace`]; Sass consumes spaces/tabs only unless
/// `consume_newlines`.
pub(crate) fn whitespace_without_comments_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &ParserState<'parse>,
    consume_newlines: bool,
) -> ParseResult<'parse, ()> {
    match state.syntax {
        Syntax::Scss | Syntax::Css(_) => {
            while !scanner.is_done() && character::is_whitespace(scanner.peek_char(0) as u8 as char)
            {
                scanner.read_char()?;
            }
            Ok(())
        }
        Syntax::Sass(_) => {
            while !scanner.is_done() {
                let next = scanner.peek_char(0);
                if next < 0 {
                    break;
                }
                let ch = next as u8 as char;
                if consume_newlines {
                    if !character::is_whitespace(ch) {
                        break;
                    }
                } else if !character::is_space_or_tab(ch) {
                    break;
                }
                scanner.read_char()?;
            }
            Ok(())
        }
    }
}

impl<'parse> Parser<'parse> {
    /// Consumes whitespace, but not comments. See
    /// [`whitespace_without_comments_impl`].
    pub fn whitespace_without_comments(
        &mut self,
        consume_newlines: bool,
    ) -> ParseResult<'parse, ()> {
        whitespace_without_comments_impl(&mut self.scanner, &self.state, consume_newlines)
    }
}

/// Consumes spaces and tabs.
pub(crate) fn spaces_impl<'parse>(scanner: &mut SpanScanner<'parse>) -> ParseResult<'parse, ()> {
    while !scanner.is_done() && character::is_space_or_tab(scanner.peek_char(0) as u8 as char) {
        scanner.read_char()?;
    }
    Ok(())
}

impl<'parse> Parser<'parse> {
    /// Consumes spaces and tabs. See [`spaces_impl`].
    pub fn spaces(&mut self) -> ParseResult<'parse, ()> {
        spaces_impl(&mut self.scanner)
    }
}

// ======================================================================
// Comments
// ======================================================================

/// Consumes and ignores a comment if possible.
///
/// `//` routes to silent comments, `/*` to loud comments; anything else
/// (including a lone `/`) is left unconsumed.
///
/// Returns whether the comment was consumed.
pub(crate) fn scan_comment_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
) -> ParseResult<'parse, bool> {
    if scanner.peek_char(0) != '/' as i32 {
        return Ok(false);
    }
    match scanner.peek_char(1) {
        c if c == '/' as i32 => silent_comment_impl(scanner, state),
        c if c == '*' as i32 => {
            loud_comment_impl(scanner)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

impl<'parse> Parser<'parse> {
    /// Consumes and ignores a comment if possible. See [`scan_comment_impl`].
    pub fn scan_comment(&mut self) -> ParseResult<'parse, bool> {
        scan_comment_impl(&mut self.scanner, &mut self.state)
    }
}

/// Consumes and ignores a single silent (Sass-style) comment, not including
/// the trailing newline.
///
/// In plain CSS this is an error ("Silent comments aren't allowed in plain
/// CSS."), except inside an expression context where it is not a comment at
/// all. (Dart's plain-CSS rejection lives in a subclass override; here it is
/// a `Syntax::Css` match arm.)
///
/// Returns whether the comment was consumed.
pub(crate) fn silent_comment_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &ParserState<'parse>,
) -> ParseResult<'parse, bool> {
    match state.syntax {
        Syntax::Css(_) => {
            if state.in_expression {
                return Ok(false);
            }
            let start = scanner.state();
            scanner.expect("//")?;
            while !scanner.is_done() && !character::is_newline(scanner.peek_char(0) as u8 as char) {
                scanner.read_char()?;
            }
            let span = scanner.span_from(start);
            let msg = "Silent comments aren't allowed in plain CSS.";
            Err(Box::new(ParseError::Format(ParseFormatError {
                message: msg.to_string(),
                span,
                cause: None,
            })))
        }
        _ => {
            scanner.expect("//")?;
            while !scanner.is_done() && !character::is_newline(scanner.peek_char(0) as u8 as char) {
                scanner.read_char()?;
            }
            Ok(true)
        }
    }
}

impl<'parse> Parser<'parse> {
    /// Consumes and ignores a single silent comment. See
    /// [`silent_comment_impl`].
    pub fn silent_comment(&mut self) -> ParseResult<'parse, bool> {
        silent_comment_impl(&mut self.scanner, &self.state)
    }
}

/// Consumes and ignores a loud (CSS-style) comment.
pub(crate) fn loud_comment_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
) -> ParseResult<'parse, ()> {
    scanner.expect("/*")?;
    loop {
        let outer = scanner.read_char()?;
        if outer != '*' {
            continue;
        }
        loop {
            let next = scanner.read_char()?;
            if next != '*' {
                if next == '/' {
                    return Ok(());
                }
                break;
            }
        }
    }
}

impl<'parse> Parser<'parse> {
    /// Consumes and ignores a loud comment. See [`loud_comment_impl`].
    pub fn loud_comment(&mut self) -> ParseResult<'parse, ()> {
        loud_comment_impl(&mut self.scanner)
    }
}

/// Like [`Parser::whitespace`], but throws an error ("Expected whitespace.")
/// if no whitespace is consumed.
///
/// A comment counts as whitespace. If `consume_newlines` is `true`, the
/// indented syntax consumes newlines as whitespace. Only set it in positions
/// where a statement can't end.
pub(crate) fn expect_whitespace_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
    consume_newlines: bool,
) -> ParseResult<'parse, ()> {
    if scanner.is_done() {
        return Err(Box::new(
            scanner.error("Expected whitespace.", None, 0).into(),
        ));
    }
    if !character::is_whitespace(scanner.peek_char(0) as u8 as char) {
        let has_comment = scan_comment_impl(scanner, state)?;
        if !has_comment {
            return Err(Box::new(
                scanner.error("Expected whitespace.", None, 0).into(),
            ));
        }
    }
    whitespace_impl(scanner, state, consume_newlines)
}

impl<'parse> Parser<'parse> {
    /// Like [`Parser::whitespace`], but errors if no whitespace is consumed.
    /// See [`expect_whitespace_impl`].
    pub fn expect_whitespace(&mut self, consume_newlines: bool) -> ParseResult<'parse, ()> {
        expect_whitespace_impl(&mut self.scanner, &mut self.state, consume_newlines)
    }
}

// ======================================================================
// Identifiers
// ======================================================================

// Identifier logic here is largely duplicated in
// `StylesheetParser::interpolated_identifier` (see `parse/stylesheet.rs` /
// `parse/identifier.rs`): most changes here should be mirrored there.

/// Consumes a plain CSS identifier.
///
/// If `normalize` is `true`, underscores are converted into hyphens.
///
/// If `unit` is `true`, a `-` followed by a digit (or dot) is not parsed as
/// part of the identifier. This ensures that `1px-2px` parses as subtraction
/// rather than the unit `px-2px`.
pub(crate) fn identifier_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    _state: &ParserState<'parse>,
    normalize: bool,
    unit: bool,
) -> ParseResult<'parse, String> {
    let mut sb = String::new();

    if scanner.scan_char('-') {
        sb.push('-');
        if scanner.scan_char('-') {
            sb.push('-');
            _identifier_body_impl(scanner, &mut sb, normalize, unit)?;
            return Ok(sb);
        }
    }

    let ch = scanner.peek_char(0);
    if ch < 0 {
        return Err(Box::new(
            scanner.error("Expected identifier.", None, 0).into(),
        ));
    }
    let ch = char::from_u32(ch as u32).unwrap_or('\0');
    if ch == '_' && normalize {
        scanner.read_char()?;
        sb.push('-');
    } else if character::is_name_start(ch) {
        let c = scanner.read_char()?;
        sb.push(c);
    } else if ch == '\\' {
        let s = escape_impl(scanner, true)?;
        sb.push_str(&s);
    } else {
        return Err(Box::new(
            scanner.error("Expected identifier.", None, 0).into(),
        ));
    }

    _identifier_body_impl(scanner, &mut sb, normalize, unit)?;
    Ok(sb)
}

impl<'parse> Parser<'parse> {
    /// Consumes a plain CSS identifier. See [`identifier_impl`].
    pub fn identifier(&mut self, normalize: bool, unit: bool) -> ParseResult<'parse, String> {
        identifier_impl(&mut self.scanner, &self.state, normalize, unit)
    }
}

/// Consumes a chunk of a plain CSS identifier after the name start.
///
/// Errors with "Expected identifier body." if no body characters follow.
pub(crate) fn identifier_body_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
) -> ParseResult<'parse, String> {
    let mut sb = String::new();
    _identifier_body_impl(scanner, &mut sb, false, false)?;
    if sb.is_empty() {
        return Err(Box::new(
            scanner.error("Expected identifier body.", None, 0).into(),
        ));
    }
    Ok(sb)
}

impl<'parse> Parser<'parse> {
    /// Consumes a chunk of a plain CSS identifier after the name start. See
    /// [`identifier_body_impl`].
    pub fn identifier_body(&mut self) -> ParseResult<'parse, String> {
        identifier_body_impl(&mut self.scanner)
    }
}

/// Parses the identifier body into `sb` (the shared loop behind
/// [`Parser::identifier`] and [`Parser::identifier_body`]), honoring
/// `normalize`/`unit` as above.
pub(crate) fn _identifier_body_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    sb: &mut String,
    normalize: bool,
    unit: bool,
) -> ParseResult<'parse, ()> {
    loop {
        let ch = scanner.peek_char(0);
        if ch < 0 {
            return Ok(());
        }
        let ch = char::from_u32(ch as u32).unwrap_or('\0');
        match ch {
            '-' if unit => {
                let next = scanner.peek_char(1);
                if next == '.' as i32 || character::is_digit(next as u8 as char) {
                    return Ok(());
                }
                scanner.read_char()?;
                sb.push('-');
            }
            '_' if normalize => {
                scanner.read_char()?;
                sb.push('-');
            }
            _ if character::is_name(ch) => {
                let c = scanner.read_char()?;
                sb.push(c);
            }
            '\\' => {
                let s = escape_impl(scanner, false)?;
                sb.push_str(&s);
            }
            _ => return Ok(()),
        }
    }
}

// ======================================================================
// String
// ======================================================================

// This logic is largely duplicated in
// `StylesheetParser::interpolated_string` and
// `StylesheetParser::interpolated_string_token`: most changes here should be
// mirrored there.

/// Consumes a plain CSS string.
///
/// Returns the parsed contents of the string — quotes excluded, escapes
/// resolved. A backslash-newline is a line continuation (both chars dropped).
pub(crate) fn string_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
) -> ParseResult<'parse, String> {
    let quote = scanner.read_char()?;
    if quote != '\'' && quote != '"' {
        let pos = scanner.pos();
        let start = pos.saturating_sub(quote.len_utf8());
        return Err(Box::new(
            scanner.error("Expected string.", Some(start), 0).into(),
        ));
    }
    let mut sb = String::new();
    loop {
        let next = scanner.peek_char(0);
        if next == quote as i32 {
            scanner.read_char()?;
            return Ok(sb);
        }
        if next < 0 || character::is_newline(next as u8 as char) {
            return Err(scanner.error(&format!("Expected {quote}."), None, 0).into());
        }
        if next == '\\' as i32 {
            if character::is_newline(scanner.peek_char(1) as u8 as char) {
                scanner.read_char()?;
                scanner.read_char()?;
            } else {
                let ch = consume_escaped_character(scanner)?;
                sb.push(char::from_u32(ch as u32).unwrap_or('\u{FFFD}'));
            }
        } else {
            let c = scanner.read_char()?;
            sb.push(c);
        }
    }
}

impl<'parse> Parser<'parse> {
    /// Consumes a plain CSS string. See [`string_impl`].
    pub fn string(&mut self) -> ParseResult<'parse, String> {
        string_impl(&mut self.scanner)
    }
}

// ======================================================================
// Numbers
// ======================================================================

/// Consumes and returns a natural number (a non-negative integer) as a double.
///
/// Doesn't support scientific notation.
pub(crate) fn natural_number_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
) -> ParseResult<'parse, f64> {
    let first = scanner.read_char()?;
    if !character::is_digit(first) {
        let pos = scanner.pos();
        let start = pos.saturating_sub(first.len_utf8());
        return Err(Box::new(
            scanner.error("Expected digit.", Some(start), 0).into(),
        ));
    }
    let mut number = character::as_decimal(first) as f64;
    while !scanner.is_done() && character::is_digit(scanner.peek_char(0) as u8 as char) {
        number *= 10.0;
        let c = scanner.read_char()?;
        number += character::as_decimal(c) as f64;
    }
    Ok(number)
}

impl<'parse> Parser<'parse> {
    /// Consumes a natural number as a double. See [`natural_number_impl`].
    pub fn natural_number(&mut self) -> ParseResult<'parse, f64> {
        natural_number_impl(&mut self.scanner)
    }
}

// ======================================================================
// Declaration value
// ======================================================================

// This logic is largely duplicated in
// `StylesheetParser::_interpolated_declaration_value`: most changes here
// should be mirrored there.

/// Consumes tokens until a top-level `";"`, `")"`, `"]"`, or `"}"` and
/// returns their contents as a string.
///
/// Runs of whitespace collapse to one space (newlines to `\n`);
/// `url(...)` contents re-scan through [`Parser::try_url`] so raw URLs survive
/// intact.
///
/// If `allow_empty` is `false` (the default), requires at least one token
/// ("Expected token."); unclosed brackets re-expect the innermost opener.
pub(crate) fn declaration_value_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
    allow_empty: bool,
) -> ParseResult<'parse, String> {
    let mut sb = String::new();
    let mut brackets: Vec<u8> = Vec::new();
    let mut wrote_newline = false;

    loop {
        let next = scanner.peek_char(0);
        if next < 0 {
            break;
        }
        let ch = next as u8 as char;
        match ch {
            '\\' => {
                let s = escape_impl(scanner, true)?;
                sb.push_str(&s);
                wrote_newline = false;
            }
            '"' | '\'' => {
                let (_, text) = raw_text_impl(scanner, state, |s, _st| {
                    string_impl(s)?;
                    Ok(())
                })?;
                sb.push_str(&text);
                wrote_newline = false;
            }
            '/' => {
                if scanner.peek_char(1) == '*' as i32 {
                    let (_, text) = raw_text_impl(scanner, state, |s, _st| {
                        loud_comment_impl(s)?;
                        Ok(())
                    })?;
                    sb.push_str(&text);
                } else {
                    scanner.read_char()?;
                    sb.push('/');
                }
                wrote_newline = false;
            }
            ' ' | '\t' => {
                if wrote_newline || !character::is_whitespace(scanner.peek_char(1) as u8 as char) {
                    sb.push(' ');
                }
                scanner.read_char()?;
            }
            '\n' | '\r' | '\x0C' => {
                if !character::is_newline(scanner.peek_char(-1) as u8 as char) {
                    sb.push('\n');
                }
                scanner.read_char()?;
                wrote_newline = true;
            }
            '(' | '{' | '[' => {
                sb.push(ch);
                let c = scanner.read_char()?;
                let opp = character::opposite(c)?;
                brackets.push(opp as u8);
                wrote_newline = false;
            }
            ')' | '}' | ']' => {
                if brackets.is_empty() {
                    break;
                }
                sb.push(ch);
                let expected = *brackets.last().unwrap() as char;
                scanner.expect_char(expected)?;
                brackets.pop();
                wrote_newline = false;
            }
            ';' => {
                if brackets.is_empty() {
                    break;
                }
                scanner.read_char()?;
                sb.push(';');
            }
            'u' | 'U' => {
                let url = try_url_impl(scanner)?;
                if !url.is_empty() {
                    sb.push_str(&url);
                } else {
                    let c = scanner.read_char()?;
                    sb.push(c);
                }
                wrote_newline = false;
            }
            _ => {
                if looking_at_identifier(scanner, None) {
                    let ident = identifier_impl(scanner, state, false, false)?;
                    sb.push_str(&ident);
                } else {
                    let c = scanner.read_char()?;
                    sb.push(c);
                }
                wrote_newline = false;
            }
        }
    }

    if !brackets.is_empty() {
        scanner.expect_char(brackets[brackets.len() - 1] as char)?;
    }
    if !allow_empty && sb.is_empty() {
        return Err(Box::new(scanner.error("Expected token.", None, 0).into()));
    }
    Ok(sb)
}

impl<'parse> Parser<'parse> {
    /// Consumes a declaration value up to a top-level terminator. See
    /// [`declaration_value_impl`].
    pub fn declaration_value(&mut self, allow_empty: bool) -> ParseResult<'parse, String> {
        declaration_value_impl(&mut self.scanner, &mut self.state, allow_empty)
    }
}

// ======================================================================
// URL
// ======================================================================

// This logic is largely duplicated in `ScssParser::_try_url_contents`: most
// changes here should be mirrored there.

/// Consumes a `url()` token if possible, and returns an empty string
/// otherwise (the Rust encoding of Dart's `null`).
///
/// Backtracks to the start (consuming nothing) unless the input is a bare
/// `url(<raw-url>)`: on bare failure Ruby Sass behavior applies — return
/// empty so the caller re-parses it as a function expression.
pub(crate) fn try_url_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
) -> ParseResult<'parse, String> {
    let start = scanner.state();
    if !scan_identifier_impl(scanner, "url", false)? {
        return Ok(String::new());
    }
    if !scanner.scan_char('(') {
        scanner.set_state(start);
        return Ok(String::new());
    }
    let mut temp_state = ParserState {
        syntax: Syntax::Scss,
        interpolation_map: None,
        in_expression: false,
    };
    whitespace_impl(scanner, &mut temp_state, true)?;

    let mut sb = String::from("url(");
    loop {
        let ch = scanner.peek_char(0);
        if ch < 0 {
            scanner.set_state(start);
            return Ok(String::new());
        }
        match ch as u8 as char {
            '\\' => {
                let s = escape_impl(scanner, false)?;
                sb.push_str(&s);
            }
            '%' | '&' | '#' => {
                let c = scanner.read_char()?;
                sb.push(c);
            }
            c if (c as u32) >= '*' as u32 && (c as u32) <= '~' as u32 => {
                let c = scanner.read_char()?;
                sb.push(c);
            }
            c if (c as u32) >= 0x0080 => {
                let c = scanner.read_char()?;
                sb.push(c);
            }
            c if character::is_whitespace(c) => {
                whitespace_impl(scanner, &mut temp_state, true)?;
                if scanner.peek_char(0) != ')' as i32 {
                    scanner.set_state(start);
                    return Ok(String::new());
                }
            }
            ')' => {
                scanner.read_char()?;
                sb.push(')');
                return Ok(sb);
            }
            _ => {
                scanner.set_state(start);
                return Ok(String::new());
            }
        }
    }
}

// ======================================================================
// Parse error types
// ======================================================================

/// Parser-level format error carrying a [`FileSpan`] for pre-conversion span
/// adjustment. Becomes [`SassError::Format`] at the [`ParseError`]→`SassError`
/// boundary (see [`wrap_span_format_exception_impl`], phase 3).
#[derive(Debug)]
pub struct ParseFormatError<'parse> {
    pub message: String,
    pub span: FileSpan<'parse>,
    pub cause: Option<Box<SassError>>,
}

impl Display for ParseFormatError<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl<'parse> std::error::Error for ParseFormatError<'parse> {}

/// Parser-level multi-span error carrying [`FileSpan`]s for pre-conversion
/// span adjustment. Becomes [`SassError::MultiSpan`] at the boundary.
#[derive(Debug)]
pub struct ParseMultiSpanError<'parse> {
    pub message: String,
    pub span: FileSpan<'parse>,
    pub primary_label: Option<String>,
    pub secondary: Vec<(FileSpan<'parse>, String)>,
    pub original_source: Option<String>,
    pub cause: Option<Box<SassError>>,
}

impl Display for ParseMultiSpanError<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl<'parse> std::error::Error for ParseMultiSpanError<'parse> {}

/// The parser's error type: [`SassError`]s pass through untouched while
/// span-carrying parse failures ([`ParseFormatError`]/[`ParseMultiSpanError`]/
/// [`ScanError`]) stay in [`FileSpan`] form through the map (phase 1) and
/// adjust (phase 2) steps of [`wrap_span_format_exception_impl`] before
/// converting to [`SassError`] (phase 3).
#[derive(Debug, Error)]
pub enum ParseError<'parse> {
    #[error(transparent)]
    Sass(Box<SassError>),
    #[error(transparent)]
    Format(ParseFormatError<'parse>),
    #[error(transparent)]
    MultiSpan(ParseMultiSpanError<'parse>),
    #[error(transparent)]
    Scan(ScanError<'parse>),
}

/// The result of a parse step: `Ok(T)` or a [`ParseError`] (Dart throws).
/// Boxed like [`SassResult`]: parse recursion is as deep as eval recursion,
/// and [`ParseError`] nests [`SassError`].
pub type ParseResult<'parse, T> = Result<T, Box<ParseError<'parse>>>;

impl<'parse> From<SassError> for ParseError<'parse> {
    fn from(e: SassError) -> Self {
        ParseError::Sass(Box::new(e))
    }
}

impl<'parse> From<Box<SassError>> for ParseError<'parse> {
    fn from(e: Box<SassError>) -> Self {
        ParseError::Sass(e)
    }
}

impl<'parse> From<Box<ParseError<'parse>>> for ParseError<'parse> {
    fn from(e: Box<ParseError<'parse>>) -> Self {
        *e
    }
}

impl<'parse> From<SpanScannerError<'parse>> for Box<ParseError<'parse>> {
    fn from(e: SpanScannerError<'parse>) -> Self {
        Box::new(e.into())
    }
}

impl<'parse> From<ScanError<'parse>> for Box<ParseError<'parse>> {
    fn from(e: ScanError<'parse>) -> Self {
        Box::new(e.into())
    }
}

impl<'parse> From<ParseFormatError<'parse>> for Box<ParseError<'parse>> {
    fn from(e: ParseFormatError<'parse>) -> Self {
        Box::new(e.into())
    }
}

impl<'parse> From<ParseMultiSpanError<'parse>> for Box<ParseError<'parse>> {
    fn from(e: ParseMultiSpanError<'parse>) -> Self {
        Box::new(e.into())
    }
}

impl<'parse> From<SpanError> for Box<ParseError<'parse>> {
    fn from(e: SpanError) -> Self {
        Box::new(e.into())
    }
}

impl From<ArgumentError> for Box<ParseError<'_>> {
    fn from(e: ArgumentError) -> Self {
        Box::new(e.into())
    }
}

impl From<RangeError> for Box<ParseError<'_>> {
    fn from(e: RangeError) -> Self {
        Box::new(e.into())
    }
}

impl<'parse> From<Box<SpanScannerError<'parse>>> for Box<ParseError<'parse>> {
    fn from(e: Box<SpanScannerError<'parse>>) -> Self {
        Box::new((*e).into())
    }
}

impl<'parse> From<SassError> for Box<ParseError<'parse>> {
    fn from(e: SassError) -> Self {
        Box::new(e.into())
    }
}

impl<'parse> From<Box<SassError>> for Box<ParseError<'parse>> {
    fn from(e: Box<SassError>) -> Self {
        Box::new(e.into())
    }
}

impl From<Box<ParseError<'_>>> for Box<SassError> {
    fn from(e: Box<ParseError<'_>>) -> Self {
        Box::new((*e).into())
    }
}

impl From<Box<ParseError<'_>>> for SassError {
    fn from(e: Box<ParseError<'_>>) -> Self {
        (*e).into()
    }
}

impl<'parse> From<SpanScannerError<'parse>> for ParseError<'parse> {
    fn from(e: SpanScannerError<'parse>) -> Self {
        match e {
            SpanScannerError::Sass(s) => ParseError::Sass(s),
            SpanScannerError::Scan(s) => ParseError::Scan(s),
        }
    }
}

impl<'parse> From<ScanError<'parse>> for ParseError<'parse> {
    fn from(e: ScanError<'parse>) -> Self {
        ParseError::Scan(e)
    }
}

impl<'parse> From<ParseFormatError<'parse>> for ParseError<'parse> {
    fn from(e: ParseFormatError<'parse>) -> Self {
        ParseError::Format(e)
    }
}

impl<'parse> From<ParseMultiSpanError<'parse>> for ParseError<'parse> {
    fn from(e: ParseMultiSpanError<'parse>) -> Self {
        ParseError::MultiSpan(e)
    }
}

impl From<ParseError<'_>> for SassError {
    fn from(e: ParseError<'_>) -> Self {
        let fallback_ctx = || {
            SourceSpanWithContext::new(
                SourceLocation {
                    offset: 0,
                    line: 0,
                    column: 0,
                },
                SourceLocation {
                    offset: 0,
                    line: 0,
                    column: 0,
                },
                String::new(),
                String::new(),
                None,
            )
            .unwrap_or_else(|_| unreachable!())
        };
        match e {
            ParseError::Sass(s) => *s,
            ParseError::Format(f) => SassError::Format {
                message: f.message,
                span: SourceSpanWithContext::from_file_span(&f.span)
                    .unwrap_or_else(|_| fallback_ctx()),
                original_source: None,
                cause: f.cause,
                loaded_urls: vec![],
            },
            ParseError::MultiSpan(m) => SassError::MultiSpan {
                message: m.message,
                span: SourceSpanWithContext::from_file_span(&m.span)
                    .unwrap_or_else(|_| fallback_ctx()),
                primary_label: m.primary_label,
                secondary: m
                    .secondary
                    .into_iter()
                    .map(|(s, desc)| {
                        (
                            SourceSpanWithContext::from_file_span(&s)
                                .unwrap_or_else(|_| fallback_ctx()),
                            desc,
                        )
                    })
                    .collect(),
                original_source: m.original_source,
                cause: m.cause,
                loaded_urls: vec![],
                trace: Default::default(),
            },
            ParseError::Scan(s) => SassError::Format {
                message: s.message,
                span: SourceSpanWithContext::from_file_span(&s.span)
                    .unwrap_or_else(|_| fallback_ctx()),
                original_source: None,
                cause: None,
                loaded_urls: vec![],
            },
        }
    }
}

impl From<ParseError<'_>> for Box<SassError> {
    fn from(e: ParseError<'_>) -> Self {
        Box::new(e.into())
    }
}

impl<'parse> From<SpanError> for ParseError<'parse> {
    fn from(e: SpanError) -> Self {
        ParseError::Sass(e.into())
    }
}

impl From<ArgumentError> for ParseError<'_> {
    fn from(e: ArgumentError) -> Self {
        ParseError::Sass(e.into())
    }
}

impl From<RangeError> for ParseError<'_> {
    fn from(e: RangeError) -> Self {
        let span_err: SpanError = e.into();
        span_err.into()
    }
}

impl<'parse> Parser<'parse> {
    /// Consumes a `url()` token if possible. See [`try_url_impl`]: the empty
    /// string means "not a raw URL" (Dart's `null`).
    pub fn try_url(&mut self) -> ParseResult<'parse, String> {
        try_url_impl(&mut self.scanner)
    }
}

// ======================================================================
// Variable name
// ======================================================================

/// Consumes a Sass variable name, and returns its name without the dollar
/// sign (identifiers normalize `_` → `-`).
pub(crate) fn variable_name_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &ParserState<'parse>,
) -> ParseResult<'parse, String> {
    scanner.expect_char('$')?;
    identifier_impl(scanner, state, true, false)
}

impl<'parse> Parser<'parse> {
    /// Consumes a Sass variable name. See [`variable_name_impl`].
    pub fn variable_name(&mut self) -> ParseResult<'parse, String> {
        variable_name_impl(&mut self.scanner, &self.state)
    }
}

// ======================================================================
// Characters
// ======================================================================

// ## Characters

/// Consumes an escape sequence and returns the text that defines it.
///
/// Follows <https://drafts.csswg.org/css-syntax-3/#consume-escaped-code-point>:
/// hex escapes consume up to 6 hex digits plus one trailing whitespace;
/// non-hex escapes are the backslash plus the escaped char verbatim.
///
/// If `identifier_start` is `true`, the escape is normalized as though at the
/// beginning of an identifier: escapes resolving to a name-start char decode
/// to that char, while control chars / `0x7F` / leading digits stay in
/// `\XX ` hex form (so `$-\31 ` re-serializes safely).
pub(crate) fn escape_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    identifier_start: bool,
) -> ParseResult<'parse, String> {
    let start = scanner.pos();
    scanner.expect_char('\\')?;
    let ch = scanner.peek_char(0);
    if ch < 0 || character::is_newline(ch as u8 as char) {
        return Err(Box::new(
            scanner.error("Expected escape sequence.", None, 0).into(),
        ));
    }
    let value: i32;
    if character::is_hex(ch as u8 as char) {
        let mut v = 0i32;
        for _ in 0..6 {
            let next = scanner.peek_char(0);
            if next < 0 || !character::is_hex(next as u8 as char) {
                break;
            }
            let hex = scanner.read_char()?;
            v = (v << 4) + character::as_hex(hex);
        }
        if character::is_whitespace(scanner.peek_char(0) as u8 as char) {
            scanner.read_char()?;
        }
        value = v;
    } else {
        value = scanner.read_char()? as i32;
    }

    let is_name_check = {
        let raw_value = value as u32;
        raw_value >= 0x0080
            || match char::from_u32(raw_value) {
                Some(ch) => {
                    if identifier_start {
                        character::is_name_start(ch)
                    } else {
                        character::is_name(ch)
                    }
                }
                None => false,
            }
    };

    if is_name_check {
        if value < 0 || (0xD800..=0xDFFF).contains(&value) || value > char::MAX as i32 {
            return Err(scanner
                .error(
                    "Invalid Unicode code point.",
                    Some(start),
                    (scanner.pos() - start) as isize,
                )
                .into());
        }
        Ok(char::from_u32(value as u32)
            .unwrap_or('\u{FFFD}')
            .to_string())
    } else if value <= 0x1F
        || value == 0x7F
        || (identifier_start && character::is_digit(value as u8 as char))
    {
        let mut sb = String::new();
        sb.push('\\');
        if value > 0xF {
            sb.push(character::hex_char_for(value >> 4));
        }
        sb.push(character::hex_char_for(value & 0xF));
        sb.push(' ');
        Ok(sb)
    } else {
        let c = char::from_u32(value as u32).unwrap_or('\u{FFFD}');
        Ok(format!("\\{c}"))
    }
}

impl<'parse> Parser<'parse> {
    /// Consumes an escape sequence. See [`escape_impl`].
    pub fn escape(&mut self, identifier_start: bool) -> ParseResult<'parse, String> {
        escape_impl(&mut self.scanner, identifier_start)
    }
}

/// Consumes an escape sequence and returns the character it represents
/// (delegates to [`consume_escaped_character`]).
pub(crate) fn escape_character_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
) -> ParseResult<'parse, i32> {
    consume_escaped_character(scanner)
}

impl<'parse> Parser<'parse> {
    /// Consumes an escape sequence. See [`escape_character_impl`].
    pub fn escape_character(&mut self) -> ParseResult<'parse, i32> {
        escape_character_impl(&mut self.scanner)
    }
}

// ======================================================================
// Character matching
// ======================================================================

// Consumes the next character if it matches `condition`.
///
/// Returns whether or not the character was consumed.
pub(crate) fn scan_char_if_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    condition: fn(i32) -> bool,
) -> ParseResult<'parse, bool> {
    let next = scanner.peek_char(0);
    if !condition(next) {
        return Ok(false);
    }
    scanner.read_char()?;
    Ok(true)
}

impl<'parse> Parser<'parse> {
    /// Consumes the next character if it matches `condition`. See
    /// [`scan_char_if_impl`].
    pub fn scan_char_if(&mut self, condition: fn(i32) -> bool) -> ParseResult<'parse, bool> {
        scan_char_if_impl(&mut self.scanner, condition)
    }
}

/// Consumes the next character or escape sequence if it matches `ch`.
///
/// Matching is case-insensitive unless `case_sensitive` is `true`
/// ([`character::char_equals_ignore_case`]). An escape that fails to match is
/// rewound; a malformed escape propagates its error.
pub(crate) fn scan_ident_char_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    ch: i32,
    case_sensitive: bool,
) -> ParseResult<'parse, bool> {
    let matches = |actual: i32| -> bool {
        if case_sensitive {
            actual == ch
        } else {
            character::char_equals_ignore_case(ch as u8 as char, actual as u8 as char)
        }
    };

    let next = scanner.peek_char(0);
    if next >= 0 && matches(next) {
        scanner.read_char()?;
        return Ok(true);
    }
    if next == '\\' as i32 {
        let start = scanner.state();
        let escaped = consume_escaped_character(scanner);
        match escaped {
            Ok(val) if matches(val) => return Ok(true),
            Ok(_) => {
                scanner.set_state(start);
            }
            Err(e) => {
                scanner.set_state(start);
                return Err(e);
            }
        }
    }
    Ok(false)
}

/// Consumes the next character or escape sequence and asserts it matches
/// `letter` ("Expected \"x\".").
///
/// Matching is case-insensitive unless `case_sensitive` is `true`.
pub(crate) fn expect_ident_char_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    letter: i32,
    case_sensitive: bool,
) -> ParseResult<'parse, ()> {
    if scan_ident_char_impl(scanner, letter, case_sensitive)? {
        return Ok(());
    }
    let c = char::from_u32(letter as u32).unwrap_or('?');
    Err(scanner
        .error(&format!("Expected \"{c}\"."), Some(scanner.pos()), 0)
        .into())
}

impl<'parse> Parser<'parse> {
    /// Consumes the next character or escape sequence if it matches `ch`.
    /// See [`scan_ident_char_impl`].
    pub fn scan_ident_char(&mut self, ch: i32, case_sensitive: bool) -> ParseResult<'parse, bool> {
        scan_ident_char_impl(&mut self.scanner, ch, case_sensitive)
    }

    /// Consumes the next character or escape sequence, asserting it matches
    /// `ch`. See [`expect_ident_char_impl`].
    pub fn expect_ident_char(&mut self, ch: i32, case_sensitive: bool) -> ParseResult<'parse, ()> {
        expect_ident_char_impl(&mut self.scanner, ch, case_sensitive)
    }
}

// ======================================================================
// Utilities
// ======================================================================

// ## Utilities

/// Returns whether the scanner is immediately before a number.
///
/// Follows <https://drafts.csswg.org/css-syntax-3/#starts-with-a-number>:
/// digit, `.`+digit, or sign followed by digit / `.`+digit.
pub fn looking_at_number<'parse>(scanner: &SpanScanner<'parse>) -> bool {
    let ch = scanner.peek_char(0);
    if character::is_digit(ch as u8 as char) {
        return true;
    }
    if ch == '.' as i32 {
        let next = scanner.peek_char(1);
        return next >= 0 && character::is_digit(next as u8 as char);
    }
    if ch == '+' as i32 || ch == '-' as i32 {
        let next = scanner.peek_char(1);
        if character::is_digit(next as u8 as char) {
            return true;
        }
        if next == '.' as i32 {
            let next2 = scanner.peek_char(2);
            return next2 >= 0 && character::is_digit(next2 as u8 as char);
        }
        return false;
    }
    false
}

impl<'parse> Parser<'parse> {
    /// Returns whether the scanner is immediately before a number. See
    /// [`looking_at_number`].
    pub fn looking_at_number(&self) -> bool {
        looking_at_number(&self.scanner)
    }
}

/// Returns whether the scanner is immediately before a plain CSS identifier.
///
/// If `forward` is passed, looks that many characters forward instead.
///
/// Based on <https://drafts.csswg.org/css-syntax-3/#would-start-an-identifier>,
/// but assumes all backslashes start escapes. (See also
/// `ScssParser::_lookingAtInterpolatedIdentifier` in `parse/scss.rs`-level
/// logic, which additionally allows `#{`.)
pub fn looking_at_identifier<'parse>(
    scanner: &SpanScanner<'parse>,
    forward: Option<usize>,
) -> bool {
    let f = forward.unwrap_or(0) as isize;
    let ch = scanner.peek_char(f);
    if ch < 0 {
        return false;
    }
    if character::is_name_start(char::from_u32(ch as u32).unwrap_or('\0')) || ch == '\\' as i32 {
        return true;
    }
    if ch == '-' as i32 {
        let next = scanner.peek_char(f + 1);
        return character::is_name_start(char::from_u32(next as u32).unwrap_or('\0'))
            || next == '\\' as i32
            || next == '-' as i32;
    }
    false
}

impl<'parse> Parser<'parse> {
    /// Returns whether the scanner is immediately before a plain CSS
    /// identifier. See [`looking_at_identifier`].
    pub fn looking_at_identifier(&self, forward: Option<usize>) -> bool {
        looking_at_identifier(&self.scanner, forward)
    }
}

/// Returns whether the scanner is immediately before a sequence of characters
/// that could be part of a plain CSS identifier body (name char or `\`).
pub fn looking_at_identifier_body<'parse>(scanner: &SpanScanner<'parse>) -> bool {
    let next = scanner.peek_char(0);
    next >= 0
        && (character::is_name(char::from_u32(next as u32).unwrap_or('\0')) || next == '\\' as i32)
}

impl<'parse> Parser<'parse> {
    /// Returns whether the scanner is immediately before identifier-body
    /// text. See [`looking_at_identifier_body`].
    pub fn looking_at_identifier_body(&self) -> bool {
        looking_at_identifier_body(&self.scanner)
    }
}

// ======================================================================
// Identifier scanning
// ======================================================================

/// Consumes an identifier if its name exactly matches `text` — and nothing
/// more: a trailing identifier-body char rewinds and returns `false`.
///
/// Returns whether the identifier was consumed; backtracks on mismatch.
pub(crate) fn scan_identifier_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    text: &str,
    case_sensitive: bool,
) -> ParseResult<'parse, bool> {
    if !looking_at_identifier(scanner, None) {
        return Ok(false);
    }
    let start = scanner.state();
    if _consume_identifier(scanner, text, case_sensitive) && !looking_at_identifier_body(scanner) {
        return Ok(true);
    }
    scanner.set_state(start);
    Ok(false)
}

impl<'parse> Parser<'parse> {
    /// Consumes an identifier exactly matching `text`. See
    /// [`scan_identifier_impl`].
    pub fn scan_identifier(
        &mut self,
        text: &str,
        case_sensitive: bool,
    ) -> ParseResult<'parse, bool> {
        scan_identifier_impl(&mut self.scanner, text, case_sensitive)
    }
}

/// Consumes `text` as an identifier, but doesn't verify whether there's
/// additional identifier text afterwards.
///
/// Returns `true` if the full `text` is consumed and `false` otherwise, but
/// doesn't reset the scan pointer. Escape-aware: each letter matches via
/// [`scan_ident_char_impl`], so `\66oo` consumes as `foo`.
fn _consume_identifier<'parse>(
    scanner: &mut SpanScanner<'parse>,
    text: &str,
    case_sensitive: bool,
) -> bool {
    for letter in text.chars() {
        match scan_ident_char_impl(scanner, letter as i32, case_sensitive) {
            Ok(true) => continue,
            _ => return false,
        }
    }
    true
}

/// Returns whether an identifier whose name exactly matches `text` is at the
/// current scanner position.
///
/// This doesn't move the scan pointer forward: the match runs and the state
/// is restored.
pub(crate) fn matches_identifier<'parse>(
    scanner: &SpanScanner<'parse>,
    text: &str,
    case_sensitive: bool,
) -> bool {
    if !looking_at_identifier(scanner, None) {
        return false;
    }
    // Matches Dart: matchesIdentifier consumes via _consumeIdentifier (which
    // is escape-aware through scanIdentChar) on a scratch offset, then checks
    // the trailing body. Rust replays the scan manually over `scanner.text()`
    // instead of save/restore because SpanScanner state save/restore is
    // fallible here; behavior (escapes, case folding, trailing-body check)
    // matches Dart exactly.
    let mut pos = scanner.pos();
    let mut ok = true;
    for letter in text.chars() {
        let remaining = &scanner.text()[pos..];
        let mut chars = remaining.chars();
        match chars.next() {
            None => {
                ok = false;
                break;
            }
            Some('\\') => {
                // Escape-aware: decode the escape like scanIdentChar does.
                // Parse hex or single-char escape to get the matched char.
                let after = &remaining[1..];
                let mut hex_val: u32 = 0;
                let mut hex_len = 0;
                for c in after.chars().take(6) {
                    if c.is_ascii_hexdigit() {
                        hex_val = (hex_val << 4) + c.to_digit(16).unwrap_or(0);
                        hex_len += c.len_utf8();
                    } else {
                        break;
                    }
                }
                let decoded = if hex_len > 0 {
                    char::from_u32(hex_val).unwrap_or('\u{FFFD}')
                } else {
                    after.chars().next().unwrap_or('\0')
                };
                let matched = if case_sensitive {
                    decoded == letter
                } else {
                    character::char_equals_ignore_case(decoded, letter)
                };
                if !matched {
                    ok = false;
                    break;
                }
                // Advance past backslash + escape body (+ optional whitespace
                // after a hex escape, mirroring consumeEscapedCharacter).
                pos += 1;
                if hex_len > 0 {
                    pos += hex_len;
                    if scanner.text()[pos..].starts_with([' ', '\t', '\n', '\r']) {
                        pos += 1;
                    }
                } else {
                    pos += after.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                }
            }
            Some(actual) => {
                let matched = if case_sensitive {
                    actual == letter
                } else {
                    character::char_equals_ignore_case(actual, letter)
                };
                if !matched {
                    ok = false;
                    break;
                }
                pos += actual.len_utf8();
            }
        }
    }
    if !ok {
        return false;
    }
    // After matching all text chars, check that we don't have identifier body chars
    let remaining = &scanner.text()[pos..];
    let next = remaining.chars().next().map(|c| c as i32).unwrap_or(-1);
    !(next >= 0
        && (character::is_name(char::from_u32(next as u32).unwrap_or('\0')) || next == '\\' as i32))
}

impl<'parse> Parser<'parse> {
    /// Returns whether the identifier at the current position exactly matches
    /// `text`, without moving the scan pointer. See [`matches_identifier`].
    pub fn matches_identifier(&self, text: &str, case_sensitive: bool) -> bool {
        matches_identifier(&self.scanner, text, case_sensitive)
    }
}

/// Consumes an identifier and asserts that its name exactly matches `text`
/// ("Expected {name}."), erroring as well on trailing identifier-body text.
///
/// Defaults the display `name` to `"text"` when the caller passes `""`
/// (Dart's `name ??= '"$text"'`).
pub(crate) fn expect_identifier_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    text: &str,
    name: &str,
    case_sensitive: bool,
) -> ParseResult<'parse, ()> {
    let display_name = if name.is_empty() {
        format!("{text:?}")
    } else {
        name.to_string()
    };
    let start = scanner.pos();
    for letter in text.chars() {
        let ok = scan_ident_char_impl(scanner, letter as i32, case_sensitive)?;
        if ok {
            continue;
        }
        return Err(scanner
            .error(&format!("Expected {display_name}."), Some(start), 0)
            .into());
    }
    if looking_at_identifier_body(scanner) {
        return Err(scanner
            .error(&format!("Expected {display_name}"), Some(start), 0)
            .into());
    }
    Ok(())
}

impl<'parse> Parser<'parse> {
    /// Consumes an identifier, asserting it exactly matches `text`. See
    /// [`expect_identifier_impl`].
    pub fn expect_identifier(
        &mut self,
        text: &str,
        name: &str,
        case_sensitive: bool,
    ) -> ParseResult<'parse, ()> {
        expect_identifier_impl(&mut self.scanner, text, name, case_sensitive)
    }
}

// ======================================================================
// Utilities
// ======================================================================

/// Runs `consumer` and returns the source text that it consumes.
pub(crate) fn raw_text_impl<'parse, T>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
    consumer: impl FnOnce(&mut SpanScanner<'parse>, &mut ParserState<'parse>) -> ParseResult<'parse, T>,
) -> ParseResult<'parse, (T, String)> {
    let start = scanner.pos();
    let result = consumer(scanner, state)?;
    let text = scanner.substring(start, None).to_owned();
    Ok((result, text))
}

impl<'parse> Parser<'parse> {
    /// Runs `consumer` and returns the source text that it consumes.
    ///
    /// The free [`raw_text_impl`] splits the borrows (`&mut scanner`, `&mut
    /// state`) so the consumer can parse more input while the start position
    /// is held; this thin wrapper re-borrows `self` twice for callers that
    /// only have `&mut self`.
    pub fn raw_text<F>(&mut self, consumer: F) -> ParseResult<'parse, String>
    where
        F: FnOnce(&mut Self) -> ParseResult<'parse, ()>,
    {
        let start = self.scanner.pos();
        consumer(self)?;
        let text = self.scanner.substring(start, None).to_owned();
        Ok(text)
    }
}

/// Like `scanner.span_from`, but passes the span through the interpolation
/// map if one is available (deferred via a lazy span so mapping happens only
/// if the span is actually read).
pub(crate) fn span_from_impl<'parse>(
    scanner: &SpanScanner<'parse>,
    state: &ParserState<'parse>,
    start: LineScannerState,
) -> ParseResult<'parse, Span<'parse>> {
    span_from_to_impl(scanner, state, start, None)
}

pub(crate) fn span_from_to_impl<'parse>(
    scanner: &SpanScanner<'parse>,
    state: &ParserState<'parse>,
    start: LineScannerState,
    end: Option<&LineScannerState>,
) -> ParseResult<'parse, Span<'parse>> {
    let span = if let Some(e) = end {
        scanner.span_from_to(start.position, e.position)
    } else {
        scanner.span_from(start)
    };
    if state.interpolation_map.is_none() {
        return Ok(Span::File(span));
    }
    let map = state.interpolation_map.unwrap();
    Ok(Span::Lazy(LazyFileSpan::new(move || {
        let mapped = map.map_span(&Span::File(span))?;
        match mapped {
            Span::File(fs) => Ok(fs),
            _ => Ok(span),
        }
    })))
}

/// Like `scanner.span_from_position`, but passes the span through the
/// interpolation map if one is available.
pub(crate) fn span_from_position_impl<'parse>(
    scanner: &SpanScanner<'parse>,
    state: &ParserState<'parse>,
    start_pos: usize,
    end: Option<usize>,
) -> ParseResult<'parse, Span<'parse>> {
    let span = if let Some(e) = end {
        scanner.span_from_to(start_pos, e)
    } else {
        scanner.span_from_pos(start_pos)
    };
    if state.interpolation_map.is_none() {
        return Ok(Span::File(span));
    }
    let map = state.interpolation_map.unwrap();
    Ok(Span::Lazy(LazyFileSpan::new(move || {
        let mapped = map.map_span(&Span::File(span))?;
        match mapped {
            Span::File(fs) => Ok(fs),
            _ => Ok(span),
        }
    })))
}

impl<'parse> Parser<'parse> {
    /// Like `scanner.span_from`, but passes the span through the interpolation
    /// map if one is available. See [`span_from_impl`].
    pub fn span_from(&self, start: LineScannerState) -> ParseResult<'parse, Span<'parse>> {
        span_from_impl(&self.scanner, &self.state, start)
    }

    /// Like `scanner.span_from` with an explicit end, mapped as above. See
    /// [`span_from_to_impl`].
    pub fn span_from_to(
        &self,
        start: LineScannerState,
        end: Option<&LineScannerState>,
    ) -> ParseResult<'parse, Span<'parse>> {
        span_from_to_impl(&self.scanner, &self.state, start, end)
    }

    /// Like `scanner.span_from_position`, mapped as above. See
    /// [`span_from_position_impl`].
    pub fn span_from_position(
        &self,
        start_pos: usize,
        end: Option<usize>,
    ) -> ParseResult<'parse, Span<'parse>> {
        span_from_position_impl(&self.scanner, &self.state, start_pos, end)
    }
}

// ======================================================================
// Error methods
// ======================================================================

/// Throws an error associated with `span` (Dart's `Never error(...)`).
///
/// In Rust the error is returned as [`ParseError::Format`]; there is no
/// `StackTrace` out-param (Dart's `throwWithTrace` trace plumbing does not
/// exist in the port).
pub(crate) fn error_impl<'parse>(msg: &str, span: &FileSpan<'parse>) -> ParseError<'parse> {
    ParseError::Format(ParseFormatError {
        message: msg.to_string(),
        span: *span,
        cause: None,
    })
}

/// Multi-span variant of [`Parser::error`]: primary `span` plus labeled
/// `secondary` spans (surfaces as [`ParseError::MultiSpan`]).
pub(crate) fn multi_span_error_impl<'parse>(
    msg: &str,
    span: &FileSpan<'parse>,
    primary_label: &str,
    secondary: Vec<(FileSpan<'parse>, String)>,
) -> ParseError<'parse> {
    ParseError::MultiSpan(ParseMultiSpanError {
        message: msg.to_string(),
        span: *span,
        primary_label: Some(primary_label.to_string()),
        secondary,
        original_source: None,
        cause: None,
    })
}

/// Runs `callback` and, if it throws (returns) a span-carrying error,
/// rethrows it with `msg` as its message, keeping span and cause.
pub(crate) fn with_error_message_impl<'parse, T>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
    msg: &str,
    callback: impl FnOnce(&mut SpanScanner<'parse>, &mut ParserState<'parse>) -> ParseResult<'parse, T>,
) -> ParseResult<'parse, T> {
    callback(scanner, state).map_err(|err| match *err {
        ParseError::Format(fmt) => Box::new(ParseError::Format(ParseFormatError {
            message: msg.to_string(),
            ..fmt
        })),
        ParseError::MultiSpan(ms) => Box::new(ParseError::MultiSpan(ParseMultiSpanError {
            message: msg.to_string(),
            ..ms
        })),
        ParseError::Scan(scan) => Box::new(ParseError::Format(ParseFormatError {
            message: msg.to_string(),
            span: scan.span,
            cause: scan.cause,
        })),
        ParseError::Sass(e) => Box::new(ParseError::Sass(match *e {
            SassError::Format {
                span,
                original_source,
                cause,
                ..
            } => Box::new(SassError::Format {
                message: msg.to_string(),
                span,
                original_source,
                cause,
                loaded_urls: vec![],
            }),
            other => Box::new(other),
        })),
    })
}

/// Runs `callback` and wraps any span-carrying error it throws in the
/// [`SassError`] boundary type.
///
/// Three phases (see `docs/ref/parse.md`): 1. map — apply the interpolation
/// map at the [`FileSpan`] level (errors inside an interpolation become
/// multi-span via `map_exception`); 2. adjust — for "expected" messages move
/// zero-length spans to the preceding newline
/// ([`adjust_exception_span_impl`]); 3. convert — [`FileSpan`] →
/// `SourceSpanWithContext`, yielding a [`SassError`].
///
/// Already-converted [`SassError`]s pass through phases 1–2 untouched.
pub(crate) fn wrap_span_format_exception_impl<'parse, T>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
    callback: impl FnOnce(&mut SpanScanner<'parse>, &mut ParserState<'parse>) -> ParseResult<'parse, T>,
) -> ParseResult<'parse, T> {
    let mut err = match callback(scanner, state) {
        Ok(v) => return Ok(v),
        Err(e) => e,
    };

    // Phase 1: Map through interpolation map FIRST (before span adjustment).
    // Dart's inner `on SourceSpanFormatException` maps via `mapException`
    // (rethrown unchanged when the map reports identity); MultiSpan errors
    // bypass the map. Change-detection is by file identity, not structural
    // equality: mapped and original spans can share content and offsets in
    // distinct allocations.
    if let Some(map) = state.interpolation_map {
        err = match *err {
            ParseError::Format(fmt) => {
                let mapped = map.map_file_span(fmt.span)?;
                match mapped {
                    Span::File(fs) => {
                        // Change-detection via file identity (not structural
                        // `==`): mapped and original spans can share content
                        // and offsets in distinct allocations.
                        if fs.start_location().offset != fmt.span.start_location().offset
                            || !FileSource::identical(fs.file(), fmt.span.file())
                        {
                            // Dart `mapException`: an error falling within an
                            // interpolation expression becomes a MultiSpan with
                            // a secondary "error in interpolated output"
                            // pointing at the generated output; otherwise the
                            // mapped span replaces the original.
                            if map.has_expression_between(
                                &fmt.span.start_location(),
                                &fmt.span.end_location(),
                            ) {
                                let sass = SassError::Format {
                                    message: fmt.message.clone(),
                                    span: SourceSpanWithContext::from_file_span(&fmt.span)
                                        .unwrap_or_else(|_| bogus_span_ctx()),
                                    original_source: None,
                                    cause: None,
                                    loaded_urls: vec![],
                                };
                                Box::new(ParseError::Sass(Box::new(map.map_exception(&sass))))
                            } else {
                                Box::new(ParseError::Format(ParseFormatError { span: fs, ..fmt }))
                            }
                        } else {
                            Box::new(ParseError::Format(fmt))
                        }
                    }
                    _ => {
                        // Map produced Multi/Lazy — convert via SassError boundary
                        let sass = SassError::Format {
                            message: fmt.message.clone(),
                            span: SourceSpanWithContext::from_file_span(&fmt.span)
                                .unwrap_or_else(|_| bogus_span_ctx()),
                            original_source: None,
                            cause: None,
                            loaded_urls: vec![],
                        };
                        Box::new(ParseError::Sass(Box::new(map.map_exception(&sass))))
                    }
                }
            }
            ParseError::MultiSpan(ms) => {
                let mut changed = false;
                let primary = map.map_file_span(ms.span)?;
                let new_primary = match primary {
                    Span::File(fs)
                        if fs.start_location().offset == ms.span.start_location().offset
                            && FileSource::identical(fs.file(), ms.span.file()) =>
                    {
                        ms.span
                    }
                    Span::File(fs) => {
                        changed = true;
                        fs
                    }
                    _ => ms.span,
                };
                let mut new_sec = Vec::new();
                for (s, desc) in ms.secondary.iter() {
                    match map.map_file_span(*s) {
                        Ok(Span::File(fs)) => {
                            if fs.start_location().offset != s.start_location().offset
                                || !FileSource::identical(fs.file(), s.file())
                            {
                                changed = true;
                            }
                            new_sec.push((fs, desc.clone()));
                        }
                        _ => {
                            new_sec.push((*s, desc.clone()));
                        }
                    }
                }
                if changed {
                    Box::new(ParseError::MultiSpan(ParseMultiSpanError {
                        span: new_primary,
                        secondary: new_sec,
                        ..ms
                    }))
                } else {
                    Box::new(ParseError::MultiSpan(ms))
                }
            }
            ParseError::Scan(scan) => {
                let mapped = map.map_file_span(scan.span)?;
                match mapped {
                    Span::File(fs) => {
                        if fs.start_location().offset != scan.span.start_location().offset
                            || !FileSource::identical(fs.file(), scan.span.file())
                        {
                            if map.has_expression_between(
                                &scan.span.start_location(),
                                &scan.span.end_location(),
                            ) {
                                let sass = SassError::Format {
                                    message: scan.message.clone(),
                                    span: SourceSpanWithContext::from_file_span(&scan.span)
                                        .unwrap_or_else(|_| bogus_span_ctx()),
                                    original_source: None,
                                    cause: None,
                                    loaded_urls: vec![],
                                };
                                Box::new(ParseError::Sass(Box::new(map.map_exception(&sass))))
                            } else {
                                Box::new(ParseError::Scan(ScanError { span: fs, ..scan }))
                            }
                        } else {
                            Box::new(ParseError::Scan(scan))
                        }
                    }
                    _ => {
                        let sass = SassError::Format {
                            message: scan.message.clone(),
                            span: SourceSpanWithContext::from_file_span(&scan.span)
                                .unwrap_or_else(|_| bogus_span_ctx()),
                            original_source: None,
                            cause: None,
                            loaded_urls: vec![],
                        };
                        Box::new(ParseError::Sass(Box::new(map.map_exception(&sass))))
                    }
                }
            }
            other => Box::new(other),
        };
    }

    // Phase 2: Adjust spans for "expected" prefix messages (Dart's outer
    // handlers: `startsWithIgnoreCase(message, "expected")` triggers
    // `_adjustExceptionSpan` on the primary and every secondary span).
    err = match *err {
        ParseError::MultiSpan(ms) => {
            if has_prefix_ignore_case(&ms.message, "expected") {
                let adjusted = adjust_exception_span_impl(ms.span)?;
                let mut sec_adjusted = Vec::new();
                for (s, desc) in &ms.secondary {
                    let adj = adjust_exception_span_impl(*s)?;
                    sec_adjusted.push((adj, desc.clone()));
                }
                Box::new(ParseError::MultiSpan(ParseMultiSpanError {
                    span: adjusted,
                    secondary: sec_adjusted,
                    ..ms
                }))
            } else {
                Box::new(ParseError::MultiSpan(ms))
            }
        }
        ParseError::Format(fmt) => {
            if has_prefix_ignore_case(&fmt.message, "expected") {
                let adjusted = adjust_exception_span_impl(fmt.span)?;
                Box::new(ParseError::Format(ParseFormatError {
                    span: adjusted,
                    ..fmt
                }))
            } else {
                Box::new(ParseError::Format(fmt))
            }
        }
        ParseError::Scan(scan) => {
            if has_prefix_ignore_case(&scan.message, "expected") {
                let adjusted = adjust_exception_span_impl(scan.span)?;
                Box::new(ParseError::Scan(ScanError {
                    span: adjusted,
                    ..scan
                }))
            } else {
                Box::new(ParseError::Scan(scan))
            }
        }
        other => Box::new(other),
    };

    // Phase 3: Convert to SassError at the boundary.
    Err(err)
}

fn bogus_span_ctx() -> SourceSpanWithContext {
    SourceSpanWithContext::new(
        SourceLocation {
            offset: 0,
            line: 0,
            column: 0,
        },
        SourceLocation {
            offset: 0,
            line: 0,
            column: 0,
        },
        String::new(),
        String::new(),
        None,
    )
    .unwrap_or_else(|_| unreachable!())
}

/// Moves `span` to [`first_newline_before_impl`] if necessary: non-empty
/// spans pass through; zero-length spans separated from the previous
/// non-whitespace char by newlines rewind to the last separating newline.
pub(crate) fn adjust_exception_span_impl(span: FileSpan<'_>) -> ParseResult<'_, FileSpan<'_>> {
    if !span.is_empty() {
        return Ok(span);
    }
    let start = span.start_location();
    let adjusted = first_newline_before_impl(span, start)?;
    if adjusted.offset == start.offset {
        return Ok(span);
    }
    let file = span.file();
    Ok(FileSpan::new(file, adjusted.offset, adjusted.offset))
}

/// If `location` is separated from the previous non-whitespace character by
/// one or more newlines, returns the location of the last separating newline.
///
/// Otherwise returns `location`. This keeps missing-token errors pointing at
/// the line where the problem actually occurred rather than at the next
/// closing bracket.
///
/// If the document contains only whitespace before `location`, always returns
/// `location`.
fn first_newline_before_impl(
    span: FileSpan<'_>,
    location: SourceLocation,
) -> ParseResult<'_, SourceLocation> {
    let file = match span.file() {
        Some(f) => f,
        None => return Ok(location),
    };
    let text = file.text();
    if text.is_empty() {
        return Ok(location);
    }
    if location.offset == 0 {
        return Ok(location);
    }
    let mut index = location.offset - 1;
    if index >= text.len() {
        index = text.len() - 1;
    }
    let mut last_newline: Option<usize> = None;
    loop {
        let ch = text.as_bytes()[index];
        if !character::is_whitespace(ch as char) {
            return match last_newline {
                Some(nl) => Ok(file.location(nl)),
                None => Ok(location),
            };
        }
        if character::is_newline(ch as char) {
            last_newline = Some(index);
        }
        if index == 0 {
            break;
        }
        index -= 1;
    }
    Ok(location)
}

impl<'parse> Parser<'parse> {
    /// Throws an error associated with `span`. See [`error_impl`].
    pub fn error(&self, msg: &str, span: &FileSpan<'parse>) -> ParseError<'parse> {
        error_impl(msg, span)
    }

    /// Multi-span variant of [`error`](Parser::error). See
    /// [`multi_span_error_impl`].
    pub fn multi_span_error(
        &self,
        msg: &str,
        span: &FileSpan<'parse>,
        primary_label: &str,
        secondary: Vec<(FileSpan<'parse>, String)>,
    ) -> ParseError<'parse> {
        multi_span_error_impl(msg, span, primary_label, secondary)
    }
}

// ======================================================================
// Pure free functions
// ======================================================================

/// Consumes an escape sequence from the scanner and returns the character it
/// represents (Dart's `consumeEscapedCharacter` in `utils.dart`).
///
/// Follows <https://drafts.csswg.org/css-syntax-3/#consume-escaped-code-point>:
/// EOF yields `0xFFFD` (only a newline errors); up to 6 hex digits plus one
/// trailing whitespace are consumed; `0` / surrogates / values above
/// `MAX_ALLOWED_CHARACTER` yield `0xFFFD`.
pub fn consume_escaped_character<'parse>(
    scanner: &mut SpanScanner<'parse>,
) -> ParseResult<'parse, i32> {
    scanner.expect_char('\\')?;
    let ch = scanner.peek_char(0);
    // Dart returns 0xFFFD on EOF (null); only a newline throws.
    if ch < 0 {
        return Ok(0xFFFD);
    }
    if character::is_newline(ch as u8 as char) {
        return Err(scanner
            .error("Expected escape sequence.", Some(scanner.pos()), 0)
            .into());
    }
    if character::is_hex(ch as u8 as char) {
        let mut value = 0i32;
        for _ in 0..6 {
            let next = scanner.peek_char(0);
            if next < 0 || !character::is_hex(next as u8 as char) {
                break;
            }
            let hex = scanner.read_char()?;
            value = (value << 4) + character::as_hex(hex);
        }
        if character::is_whitespace(scanner.peek_char(0) as u8 as char) {
            scanner.read_char()?;
        }
        if value == 0 || (0xD800..=0xDFFF).contains(&value) || value >= MAX_ALLOWED_CHARACTER {
            return Ok(0xFFFD);
        }
        return Ok(value);
    }
    Ok(scanner.read_char()? as i32)
}

/// Returns whether `s` starts with `prefix`, ignoring ASCII case (Dart's
/// `startsWithIgnoreCase` in `utils.dart`).
pub(crate) fn has_prefix_ignore_case(s: &str, prefix: &str) -> bool {
    if s.len() < prefix.len() {
        return false;
    }
    s.as_bytes()
        .iter()
        .zip(prefix.as_bytes().iter())
        .all(|(a, b)| ascii_char_equals_ignore_case(*a, *b))
}

/// ASCII case-insensitive byte equality (Dart's `characterEqualsIgnoreCase`
/// in `util/character.dart`): equal bytes match; bytes differing only in the
/// `0x20` bit match when both fold into `A`–`Z`.
pub(crate) fn ascii_char_equals_ignore_case(a: u8, b: u8) -> bool {
    if a == b {
        return true;
    }
    if a ^ b != 0x20 {
        return false;
    }
    let c = a & !0x20;
    c.is_ascii_uppercase()
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::ast::sass::interpolation_map::InterpolationMap;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    fn make_parser<'compile, 'parse>(arena: &'compile Bump, text: &str) -> Parser<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        Parser::new(fs, Syntax::Scss, None)
    }

    fn make_scanner<'compile, 'parse>(arena: &'compile Bump, text: &str) -> SpanScanner<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        SpanScanner::new(fs)
    }

    #[test]
    fn test_parse_identifier() {
        assert_eq!(parse_identifier("foo").unwrap(), "foo");
        assert_eq!(parse_identifier("foo-bar").unwrap(), "foo-bar");
        assert_eq!(parse_identifier("café").unwrap(), "café");
        assert_eq!(parse_identifier("--custom-prop").unwrap(), "--custom-prop");
        assert_eq!(parse_identifier("_private").unwrap(), "_private");
        assert!(parse_identifier("").is_err());
        // "1invalid" starts with digit, but identifier() will read it as
        // name-start? No, digit is not name-start. So error.
        assert!(parse_identifier("1invalid").is_err());
    }

    #[test]
    fn test_is_identifier() {
        assert!(is_identifier("foo"));
        assert!(!is_identifier("1foo"));
        assert!(!is_identifier(""));
    }

    #[test]
    fn test_is_variable_declaration_like() {
        assert!(is_variable_declaration_like("$var: val").unwrap());
        assert!(is_variable_declaration_like("$var : val").unwrap());
        assert!(!is_variable_declaration_like("$var val").unwrap());
        assert!(!is_variable_declaration_like("x: 1").unwrap());
        assert!(!is_variable_declaration_like("").unwrap());
        assert!(!is_variable_declaration_like("$: val").unwrap());
    }

    #[test]
    fn test_identifier_plain() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "foo");
        assert_eq!(p.identifier(false, false).unwrap(), "foo");
    }

    #[test]
    fn test_identifier_normalized() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "_bar");
        assert_eq!(p.identifier(true, false).unwrap(), "-bar");
    }

    #[test]
    fn test_identifier_underscore_body_normalized() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "foo_bar");
        assert_eq!(p.identifier(true, false).unwrap(), "foo-bar");
    }

    #[test]
    fn test_identifier_double_dash() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "--my-prop");
        assert_eq!(p.identifier(false, false).unwrap(), "--my-prop");
    }

    #[test]
    fn test_identifier_unit_mode() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "foo-bar");
        assert_eq!(p.identifier(false, true).unwrap(), "foo-bar");

        let mut p2 = make_parser(&arena, "foo-5");
        assert_eq!(p2.identifier(false, true).unwrap(), "foo");
    }

    #[test]
    fn test_identifier_escape() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "\\41 bc");
        assert_eq!(p.identifier(false, false).unwrap(), "Abc");
    }

    #[test]
    fn test_identifier_body() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "foo");
        p.scanner.scan_char('f');
        assert_eq!(p.identifier_body().unwrap(), "oo");
    }

    #[test]
    fn test_identifier_body_empty() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, ".");
        p.scanner.scan_char('.');
        assert!(p.identifier_body().is_err());
    }

    #[test]
    fn test_string_single_quote() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "'hello'");
        assert_eq!(p.string().unwrap(), "hello");
    }

    #[test]
    fn test_string_double_quote() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "\"world\"");
        assert_eq!(p.string().unwrap(), "world");
    }

    #[test]
    fn test_string_escape() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "\"\\41\"");
        assert_eq!(p.string().unwrap(), "A");
    }

    #[test]
    fn test_string_escape_newline() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "\"a\\\nb\"");
        assert_eq!(p.string().unwrap(), "ab");
    }

    #[test]
    fn test_natural_number() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "42");
        assert_eq!(p.natural_number().unwrap(), 42.0);
        let mut p2 = make_parser(&arena, "0");
        assert_eq!(p2.natural_number().unwrap(), 0.0);
        let mut p3 = make_parser(&arena, "999");
        assert_eq!(p3.natural_number().unwrap(), 999.0);
    }

    #[test]
    fn test_natural_number_error() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "abc");
        assert!(p.natural_number().is_err());
    }

    #[test]
    fn test_variable_name() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "$foo-bar");
        assert_eq!(p.variable_name().unwrap(), "foo-bar");
    }

    #[test]
    fn test_variable_name_error() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "foo");
        assert!(p.variable_name().is_err());
    }

    #[test]
    fn test_escape_hex() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "\\41");
        assert_eq!(p.escape(true).unwrap(), "A");
    }

    #[test]
    fn test_escape_hex_trailing_space() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "\\41 ");
        assert_eq!(p.escape(true).unwrap(), "A");
    }

    #[test]
    fn test_escape_control_char() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "\\1");
        let got = p.escape(false).unwrap();
        assert!(got.starts_with('\\'));
        assert!(got.len() >= 3);
    }

    #[test]
    fn test_escape_non_hex() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "\\@");
        assert_eq!(p.escape(true).unwrap(), "\\@");
    }

    #[test]
    fn test_escape_newline_error() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "\\\n");
        assert!(p.escape(false).is_err());
    }

    #[test]
    fn test_escape_character() {
        let arena = Bump::new();
        assert_eq!(
            consume_escaped_character(&mut make_scanner(&arena, "\\41")).unwrap(),
            65
        );
        assert_eq!(
            consume_escaped_character(&mut make_scanner(&arena, "\\a")).unwrap(),
            10
        );
        assert_eq!(
            consume_escaped_character(&mut make_scanner(&arena, "\\0")).unwrap(),
            0xFFFD
        );
        assert_eq!(
            consume_escaped_character(&mut make_scanner(&arena, "\\*")).unwrap(),
            '*' as i32
        );
        assert_eq!(
            consume_escaped_character(&mut make_scanner(&arena, "\\9")).unwrap(),
            9
        );
    }

    #[test]
    fn test_scan_char_if() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "42");
        assert!(p
            .scan_char_if(|ch| ch >= '0' as i32 && ch <= '9' as i32)
            .unwrap());
        assert_eq!(p.scanner.pos(), 1);
    }

    #[test]
    fn test_scan_char_if_no_match() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "abc");
        assert!(!p
            .scan_char_if(|ch| ch >= '0' as i32 && ch <= '9' as i32)
            .unwrap());
        assert_eq!(p.scanner.pos(), 0);
    }

    #[test]
    fn test_scan_ident_char_exact() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "abc");
        assert!(p.scan_ident_char('a' as i32, true).unwrap());
    }

    #[test]
    fn test_scan_ident_char_escape_match() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "\\41");
        assert!(p.scan_ident_char('a' as i32, false).unwrap());
    }

    #[test]
    fn test_scan_ident_char_escape_no_match() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "\\42");
        assert!(!p.scan_ident_char('a' as i32, false).unwrap());
        assert_eq!(p.scanner.pos(), 0);
    }

    #[test]
    fn test_looking_at_number() {
        let arena = Bump::new();
        let p = make_parser(&arena, "1");
        assert!(p.looking_at_number());
        let p2 = make_parser(&arena, ".5");
        assert!(p2.looking_at_number());
        let p3 = make_parser(&arena, "+.5");
        assert!(p3.looking_at_number());
        let p4 = make_parser(&arena, "a");
        assert!(!p4.looking_at_number());
        let p5 = make_parser(&arena, ".");
        assert!(!p5.looking_at_number());
        let p6 = make_parser(&arena, "");
        assert!(!p6.looking_at_number());
    }

    #[test]
    fn test_looking_at_identifier() {
        let arena = Bump::new();
        let p = make_parser(&arena, "_a");
        assert!(p.looking_at_identifier(None));
        let p2 = make_parser(&arena, "-a");
        assert!(p2.looking_at_identifier(None));
        let p3 = make_parser(&arena, "1a");
        assert!(!p3.looking_at_identifier(None));
        let p4 = make_parser(&arena, "");
        assert!(!p4.looking_at_identifier(None));
    }

    #[test]
    fn test_looking_at_identifier_body() {
        let arena = Bump::new();
        let p = make_parser(&arena, "a");
        assert!(p.looking_at_identifier_body());
        let p2 = make_parser(&arena, ".");
        assert!(!p2.looking_at_identifier_body());
        let p3 = make_parser(&arena, "");
        assert!(!p3.looking_at_identifier_body());
    }

    #[test]
    fn test_scan_identifier() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "foo");
        assert!(p.scan_identifier("foo", true).unwrap());
        let mut p2 = make_parser(&arena, "foobar");
        assert!(!p2.scan_identifier("foo", true).unwrap());
        let mut p3 = make_parser(&arena, "bar");
        assert!(!p3.scan_identifier("foo", true).unwrap());
        let mut p4 = make_parser(&arena, "foo");
        assert!(!p4.scan_identifier("f", true).unwrap());
    }

    #[test]
    fn test_matches_identifier() {
        let arena = Bump::new();
        let p = make_parser(&arena, "foobar");
        assert!(!p.matches_identifier("foo", true));
        assert_eq!(p.scanner.pos(), 0);
        let p2 = make_parser(&arena, "foo bar");
        assert!(p2.matches_identifier("foo", true));
        assert_eq!(p2.scanner.pos(), 0);
    }

    #[test]
    fn test_matches_identifier_escape_aware() {
        // Matches Dart: matchesIdentifier routes through _consumeIdentifier
        // (escape-aware via scanIdentChar), so `\66oo` matches "foo".
        let arena = Bump::new();
        let p = make_parser(&arena, "\\66oo bar");
        assert!(p.matches_identifier("foo", false));
        assert_eq!(p.scanner.pos(), 0);
        let p2 = make_parser(&arena, "\\66oox");
        assert!(!p2.matches_identifier("foo", false));
    }

    #[test]
    fn test_expect_identifier_success() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "foo bar");
        p.expect_identifier("foo", "a name", true).unwrap();
        assert_eq!(p.scanner.pos(), 3);
    }

    #[test]
    fn test_expect_identifier_error() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "foo");
        assert!(p.expect_identifier("bar", "a name", true).is_err());
    }

    #[test]
    fn test_raw_text() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "hello world");
        let text = p
            .raw_text(|pp| {
                for _ in 0..5 {
                    pp.read_char()?;
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_declaration_value_brackets() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "(foo);");
        assert_eq!(p.declaration_value(false).unwrap(), "(foo)");
    }

    #[test]
    fn test_declaration_value_nested_brackets() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "a(b(c)d);");
        assert_eq!(p.declaration_value(false).unwrap(), "a(b(c)d)");
    }

    #[test]
    fn test_declaration_value_semicolon_terminate() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "foo; bar");
        assert_eq!(p.declaration_value(false).unwrap(), "foo");
    }

    #[test]
    fn test_declaration_value_unmatched_close() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "foo);");
        assert_eq!(p.declaration_value(false).unwrap(), "foo");
    }

    #[test]
    fn test_declaration_value_space_collapse() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "a  b;");
        assert_eq!(p.declaration_value(false).unwrap(), "a b");
    }

    #[test]
    fn test_try_url() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "url(foo)bar");
        assert_eq!(p.try_url().unwrap(), "url(foo)");
    }

    #[test]
    fn test_try_url_not_url() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "xyz bar");
        assert_eq!(p.try_url().unwrap(), "");
        assert_eq!(p.scanner.pos(), 0);
    }

    #[test]
    fn test_whitespace() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "  \t\nx");
        p.whitespace(true).unwrap();
        assert_eq!(p.scanner.pos(), 4);
    }

    #[test]
    fn test_whitespace_with_comment() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "  // comment\nx");
        p.whitespace(true).unwrap();
        assert_eq!(p.scanner.pos(), 13);
    }

    #[test]
    fn test_spaces() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "  \tx");
        p.spaces().unwrap();
        assert_eq!(p.scanner.pos(), 3);
    }

    #[test]
    fn test_loud_comment() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "/*hello*/x");
        p.loud_comment().unwrap();
        assert_eq!(p.scanner.pos(), 9);
    }

    #[test]
    fn test_silent_comment() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "// hello\nx");
        assert!(p.silent_comment().unwrap());
        assert_eq!(p.scanner.pos(), 8);
    }

    #[test]
    fn test_scan_comment_double_slash() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "//hello\nx");
        assert!(p.scan_comment().unwrap());
    }

    #[test]
    fn test_scan_comment_slash_star() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "/*hello*/x");
        assert!(p.scan_comment().unwrap());
    }

    #[test]
    fn test_scan_comment_not_comment() {
        let arena = Bump::new();
        let mut p = make_parser(&arena, "x");
        assert!(!p.scan_comment().unwrap());
    }

    #[test]
    fn test_consume_escaped_character_surrogate() {
        let arena = Bump::new();
        assert_eq!(
            consume_escaped_character(&mut make_scanner(&arena, "\\D800")).unwrap(),
            0xFFFD
        );
    }

    #[test]
    fn test_has_prefix_ignore_case() {
        assert!(has_prefix_ignore_case("Expected foo", "expected"));
        assert!(has_prefix_ignore_case("expected", "Expected"));
        assert!(has_prefix_ignore_case("Foo", "foo"));
        assert!(has_prefix_ignore_case("Foo", "Foo"));
        assert!(has_prefix_ignore_case("Foo", ""));
        assert!(!has_prefix_ignore_case("Foo", "foobar"));
    }

    #[test]
    fn test_ascii_char_equals_ignore_case() {
        assert!(ascii_char_equals_ignore_case(b'a', b'A'));
        assert!(ascii_char_equals_ignore_case(b'Z', b'z'));
        assert!(!ascii_char_equals_ignore_case(b'a', b'b'));
        assert!(!ascii_char_equals_ignore_case(b'0', b'P'));
    }

    // ================================================================
    // Scanner delegates
    // ================================================================

    #[test]
    fn test_read_char() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "abc");
        let ch = parser.read_char().unwrap();
        assert_eq!(ch, 'a');
        assert_eq!(parser.scanner.pos(), 1);
    }

    #[test]
    fn test_read_char_eof() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "");
        let result = parser.read_char();
        assert!(result.is_err());
    }

    #[test]
    fn test_set_position() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "hello");
        parser.read_char().unwrap();
        parser.read_char().unwrap();
        parser.read_char().unwrap();
        parser.set_position(0).unwrap();
        assert_eq!(parser.scanner.pos(), 0);
    }

    #[test]
    fn test_set_position_beyond() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "abc");
        let result = parser.set_position(100);
        assert!(result.is_err());
    }

    #[test]
    fn test_expect_char() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "$var");
        parser.expect_char('$').unwrap();
        assert_eq!(parser.scanner.pos(), 1);
    }

    #[test]
    fn test_expect_char_error() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "x");
        match *parser.expect_char('$').unwrap_err() {
            ParseError::Scan(s) => assert!(s.message.contains("expected \"$\"")),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_expect_char_name() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "$var");
        parser.expect_char_name('$', "dollar sign").unwrap();
    }

    #[test]
    fn test_expect_char_name_error() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "x");
        match *parser.expect_char_name('$', "dollar sign").unwrap_err() {
            ParseError::Scan(s) => assert!(s.message.contains("expected dollar sign")),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_expect() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "/*hello*/");
        parser.expect("/*").unwrap();
        assert_eq!(parser.scanner.pos(), 2);
    }

    #[test]
    fn test_expect_error() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "x");
        match *parser.expect("//").unwrap_err() {
            ParseError::Scan(s) => assert!(s.message.contains("\"//\"")),
            _ => panic!("expected Scan error"),
        }
    }

    // ================================================================
    // Whitespace without comments
    // ================================================================

    #[test]
    fn test_whitespace_without_comments() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "  \t  x");
        parser.whitespace_without_comments(false).unwrap();
        assert_eq!(parser.scanner.pos(), 5);
    }

    #[test]
    fn test_whitespace_without_comments_no_ws() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "x");
        parser.whitespace_without_comments(true).unwrap();
        assert_eq!(parser.scanner.pos(), 0);
    }

    // ================================================================
    // Comments — edge cases
    // ================================================================

    #[test]
    fn test_loud_comment_nested_star() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "/* a ** b */x");
        parser.loud_comment().unwrap();
        assert_eq!(parser.scanner.pos(), 12);
    }

    #[test]
    fn test_loud_comment_unterminated() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "/* unterminated");
        assert!(parser.loud_comment().is_err());
    }

    #[test]
    fn test_loud_comment_not_comment() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "x");
        match *parser.loud_comment().unwrap_err() {
            ParseError::Scan(s) => assert!(s.message.contains("\"/*\"")),
            _ => panic!("expected Scan error"),
        }
    }

    // ================================================================
    // Expect whitespace
    // ================================================================

    #[test]
    fn test_expect_whitespace_eof() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "");
        match *parser.expect_whitespace(false).unwrap_err() {
            ParseError::Scan(s) => assert_eq!(s.message, "Expected whitespace."),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_expect_whitespace_no_ws() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "abc");
        match *parser.expect_whitespace(false).unwrap_err() {
            ParseError::Scan(s) => assert_eq!(s.message, "Expected whitespace."),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_expect_whitespace_spaces() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "  x");
        parser.expect_whitespace(false).unwrap();
        assert_eq!(parser.scanner.pos(), 2);
    }

    #[test]
    fn test_expect_whitespace_comment() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "// cmt\nx");
        parser.expect_whitespace(false).unwrap();
        assert_eq!(parser.scanner.pos(), 7);
    }

    #[test]
    fn test_expect_whitespace_mixed() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "  /* cmt */ x");
        parser.expect_whitespace(false).unwrap();
        assert_eq!(parser.scanner.pos(), 12);
    }

    // ================================================================
    // Identifier edge cases
    // ================================================================

    #[test]
    fn test_identifier_empty() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "");
        match *parser.identifier(false, false).unwrap_err() {
            ParseError::Scan(s) => assert_eq!(s.message, "Expected identifier."),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_identifier_digit_start() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "1foo");
        match *parser.identifier(false, false).unwrap_err() {
            ParseError::Scan(s) => assert_eq!(s.message, "Expected identifier."),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_identifier_unit_dash_before_dot() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "foo-.");
        let result = parser.identifier(false, true).unwrap();
        assert_eq!(result, "foo");
    }

    #[test]
    fn test_identifier_escape_only() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "\\41 ");
        let result = parser.identifier(false, false).unwrap();
        assert_eq!(result, "A");
    }

    #[test]
    fn test_identifier_uppercase_start() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "Escaped");
        let result = parser.identifier(false, false).unwrap();
        assert_eq!(result, "Escaped");
    }

    #[test]
    fn test_identifier_underscore_to_dash() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "_var");
        let result = parser.identifier(true, false).unwrap();
        assert_eq!(result, "-var");
    }

    #[test]
    fn test_identifier_bare_dash() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "-");
        match *parser.identifier(false, false).unwrap_err() {
            ParseError::Scan(s) => assert_eq!(s.message, "Expected identifier."),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_identifier_body_escape() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "f\\41o");
        parser.scanner.scan_char('f');
        let result = parser.identifier_body().unwrap();
        assert_eq!(result, "Ao");
    }

    // ================================================================
    // String edge cases
    // ================================================================

    #[test]
    fn test_string_empty() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "");
        assert!(parser.string().is_err());
    }

    #[test]
    fn test_string_not_a_quote() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "abc");
        match *parser.string().unwrap_err() {
            ParseError::Scan(s) => assert_eq!(s.message, "Expected string."),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_string_unterminated_single() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "'unterminated");
        match *parser.string().unwrap_err() {
            ParseError::Scan(s) => assert_eq!(s.message, "Expected '."),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_string_unterminated_double() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "\"unterminated");
        match *parser.string().unwrap_err() {
            ParseError::Scan(s) => assert_eq!(s.message, "Expected \"."),
            _ => panic!("expected Scan error"),
        }
    }

    // ================================================================
    // Natural number edge cases
    // ================================================================

    #[test]
    fn test_natural_number_stops_at_non_digit() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "1a");
        let result = parser.natural_number().unwrap();
        assert_eq!(result, 1.0);
        assert_eq!(parser.scanner.pos(), 1);
    }

    // ================================================================
    // Declaration value edge cases
    // ================================================================

    #[test]
    fn test_declaration_value_allow_empty_true() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ";");
        let result = parser.declaration_value(true).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_declaration_value_allow_empty_false() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ";");
        match *parser.declaration_value(false).unwrap_err() {
            ParseError::Scan(s) => assert_eq!(s.message, "Expected token."),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_declaration_value_escape() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "\\41  b;");
        let result = parser.declaration_value(false).unwrap();
        assert_eq!(result, "A b");
    }

    #[test]
    fn test_declaration_value_loud_comment() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "a /* cmt */ b;");
        let result = parser.declaration_value(false).unwrap();
        assert_eq!(result, "a /* cmt */ b");
    }

    #[test]
    fn test_declaration_value_newline_collapse() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "a\n\nb;");
        let result = parser.declaration_value(false).unwrap();
        assert_eq!(result, "a\nb");
    }

    #[test]
    fn test_declaration_value_crlf() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "a\r\nb;");
        let result = parser.declaration_value(false).unwrap();
        assert_eq!(result, "a\nb");
    }

    #[test]
    fn test_declaration_value_quadruple_nested() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "a(b(c(d)e)f);");
        let result = parser.declaration_value(false).unwrap();
        assert_eq!(result, "a(b(c(d)e)f)");
    }

    #[test]
    fn test_declaration_value_unmatched_open() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "a(b;");
        assert!(parser.declaration_value(false).is_err());
    }

    // ================================================================
    // URL edge cases
    // ================================================================

    #[test]
    fn test_try_url_full() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "url(http://example.com)");
        let result = parser.try_url().unwrap();
        assert_eq!(result, "url(http://example.com)");
    }

    #[test]
    fn test_try_url_escape() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "url(\\41 bc)");
        let result = parser.try_url().unwrap();
        assert_eq!(result, "url(Abc)");
    }

    #[test]
    fn test_try_url_non_ascii() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "url(\u{00E9})");
        let result = parser.try_url().unwrap();
        assert_eq!(result, "url(\u{00E9})");
    }

    #[test]
    fn test_try_url_whitespace() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "url(  foo  )bar");
        let result = parser.try_url().unwrap();
        assert_eq!(result, "url(foo)");
    }

    #[test]
    fn test_try_url_restore_no_paren() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "urlNot");
        let result = parser.try_url().unwrap();
        assert_eq!(result, "");
        assert_eq!(parser.scanner.pos(), 0);
    }

    #[test]
    fn test_try_url_restore_invalid_char() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "url(abc!)");
        let result = parser.try_url().unwrap();
        assert_eq!(result, "");
        assert_eq!(parser.scanner.pos(), 0);
    }

    #[test]
    fn test_try_url_case_insensitive() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "URL(foo)");
        let result = parser.try_url().unwrap();
        assert_eq!(result, "url(foo)");
    }

    // ================================================================
    // Escape edge cases
    // ================================================================

    #[test]
    fn test_escape_surrogate() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "\\D800 ");
        match *parser.escape(false).unwrap_err() {
            ParseError::Scan(s) => assert_eq!(s.message, "Invalid Unicode code point."),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_escape_digit_at_start() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "\\31 ");
        let result = parser.escape(true).unwrap();
        assert_eq!(result, "\\31 ");
    }

    #[test]
    fn test_escape_control_char_7f() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "\\7F");
        let result = parser.escape(false).unwrap();
        assert_eq!(result, "\\7f ");
    }

    #[test]
    fn test_escape_non_hex_name() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "\\g");
        let result = parser.escape(true).unwrap();
        assert_eq!(result, "g");
    }

    #[test]
    fn test_escape_six_hex_digits() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "\\000041");
        let result = parser.escape(false).unwrap();
        assert_eq!(result, "A");
    }

    // ================================================================
    // Scan ident char
    // ================================================================

    #[test]
    fn test_scan_ident_char_case_insensitive_match() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "Abc");
        let result = parser.scan_ident_char('a' as i32, false).unwrap();
        assert!(result);
    }

    #[test]
    fn test_scan_ident_char_escape_error() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "\\\n");
        assert!(parser.scan_ident_char('a' as i32, false).is_err());
    }

    // ================================================================
    // Lookahead edge cases
    // ================================================================

    #[test]
    fn test_looking_at_number_plus_alone() {
        let arena = Bump::new();
        let parser = make_parser(&arena, "+");
        assert!(!parser.looking_at_number());
    }

    #[test]
    fn test_looking_at_number_minus_alone() {
        let arena = Bump::new();
        let parser = make_parser(&arena, "-");
        assert!(!parser.looking_at_number());
    }

    #[test]
    fn test_looking_at_number_bad_dot() {
        let arena = Bump::new();
        let parser = make_parser(&arena, "+.abc");
        assert!(!parser.looking_at_number());
    }

    #[test]
    fn test_looking_at_identifier_double_dash() {
        let arena = Bump::new();
        let parser = make_parser(&arena, "--a");
        assert!(parser.looking_at_identifier(Some(0)));
    }

    #[test]
    fn test_looking_at_identifier_escape() {
        let arena = Bump::new();
        let parser = make_parser(&arena, "\\a");
        assert!(parser.looking_at_identifier(None));
    }

    #[test]
    fn test_looking_at_identifier_forward() {
        let arena = Bump::new();
        let parser = make_parser(&arena, "abc-def");
        assert!(parser.looking_at_identifier(Some(3)));
    }

    #[test]
    fn test_looking_at_identifier_body_dash() {
        let arena = Bump::new();
        let parser = make_parser(&arena, "-");
        assert!(parser.looking_at_identifier_body());
    }

    #[test]
    fn test_looking_at_identifier_body_non_name() {
        let arena = Bump::new();
        let parser = make_parser(&arena, "{");
        assert!(!parser.looking_at_identifier_body());
    }

    // ================================================================
    // Identifier scanning edge cases
    // ================================================================

    #[test]
    fn test_scan_identifier_case_insensitive() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "Foo");
        let result = parser.scan_identifier("foo", false).unwrap();
        assert!(result);
        assert_eq!(parser.scanner.pos(), 3);
    }

    #[test]
    fn test_scan_identifier_not_identifier() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "!foo");
        let result = parser.scan_identifier("foo", true).unwrap();
        assert!(!result);
        assert_eq!(parser.scanner.pos(), 0);
    }

    #[test]
    fn test_expect_identifier_trailing_body() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "foo123");
        match *parser.expect_identifier("foo", "", true).unwrap_err() {
            ParseError::Scan(s) => assert!(s.message.contains("Expected \"foo\"")),
            _ => panic!("expected Scan error"),
        }
    }

    #[test]
    fn test_expect_identifier_custom_name() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "foo");
        match *parser.expect_identifier("bar", "a name", true).unwrap_err() {
            ParseError::Scan(s) => assert!(s.message.contains("Expected a name")),
            _ => panic!("expected Scan error"),
        }
    }

    // ================================================================
    // Span utilities
    // ================================================================

    #[test]
    fn test_span_from() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "hello world");
        let start = parser.scanner.state();
        for _ in 0..5 {
            parser.read_char().unwrap();
        }
        let span = parser.span_from(start).unwrap();
        let file_span = span.file_span().unwrap();
        assert_eq!(file_span.text(), "hello");
    }

    #[test]
    fn test_span_from_to() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "foobar");
        let s1 = parser.scanner.state();
        parser.read_char().unwrap();
        parser.read_char().unwrap();
        parser.read_char().unwrap();
        let s2 = parser.scanner.state();
        let span = parser.span_from_to(s1, Some(&s2)).unwrap();
        let file_span = span.file_span().unwrap();
        assert_eq!(file_span.text(), "foo");
    }

    #[test]
    fn test_span_from_position() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "hello");
        for _ in 0..5 {
            parser.read_char().unwrap();
        }
        let span = parser.span_from_position(0, None).unwrap();
        let file_span = span.file_span().unwrap();
        assert_eq!(file_span.text(), "hello");
    }

    #[test]
    fn test_span_from_position_end() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "hello");
        for _ in 0..5 {
            parser.read_char().unwrap();
        }
        let span = parser.span_from_position(0, Some(2)).unwrap();
        let file_span = span.file_span().unwrap();
        assert_eq!(file_span.text(), "he");
    }

    // ================================================================
    // Error methods
    // ================================================================

    #[test]
    fn test_error_impl() {
        let arena = Bump::new();
        let parser = make_parser(&arena, "x");
        let span = parser.scanner.empty_span();
        let err = error_impl("test message", &span);
        match err {
            ParseError::Format(f) => assert_eq!(f.message, "test message"),
            _ => panic!("expected Format error"),
        }
    }

    #[test]
    fn test_multi_span_error_impl() {
        let arena = Bump::new();
        let parser = make_parser(&arena, "x");
        let span = parser.scanner.empty_span();
        let secondary = vec![(span, "secondary desc".to_string())];
        let err = multi_span_error_impl("main msg", &span, "primary label", secondary);
        match err {
            ParseError::MultiSpan(m) => {
                assert_eq!(m.message, "main msg");
                assert_eq!(m.primary_label.as_deref(), Some("primary label"));
                assert_eq!(m.secondary.len(), 1);
            }
            _ => panic!("expected MultiSpan error"),
        }
    }

    // ================================================================
    // Consume escaped character edge cases
    // ================================================================

    #[test]
    fn test_consume_escaped_character_eof() {
        // Matches Dart: consumeEscapedCharacter returns 0xFFFD on EOF.
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "\\", None);
        let mut scanner = SpanScanner::new(fs);
        assert_eq!(consume_escaped_character(&mut scanner).unwrap(), 0xFFFD);
    }

    #[test]
    fn test_consume_escaped_character_overflow() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "\\FFFFFF", None);
        let mut scanner = SpanScanner::new(fs);
        let result = consume_escaped_character(&mut scanner).unwrap();
        assert_eq!(result, 0xFFFD);
    }

    #[test]
    fn test_consume_escaped_character_plain() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "\\!", None);
        let mut scanner = SpanScanner::new(fs);
        let result = consume_escaped_character(&mut scanner).unwrap();
        assert_eq!(result, '!' as i32);
    }

    #[test]
    fn test_consume_escaped_character_hex_zero() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "\\0 ", None);
        let mut scanner = SpanScanner::new(fs);
        let result = consume_escaped_character(&mut scanner).unwrap();
        assert_eq!(result, 0xFFFD);
    }

    /// Builds a `Parser` over generated-output text `generated` with an
    /// interpolation map for the empty-contents interpolation
    /// `interp_text` (allocation A). Generated and interpolation sources
    /// carry identical content (`url: None`) in distinct allocations, so
    /// structural `==` and `FileSource::identical` disagree.
    fn make_mapped_parser<'compile, 'parse>(
        arena: &'compile Bump,
        generated: &str,
        interp_text: &str,
    ) -> (
        Parser<'parse>,
        &'parse InterpolationMap<'parse>,
        &'parse FileSource<'parse>,
    )
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let gen_fs: &'parse FileSource<'parse> = FileSource::new_in(arena, generated, None);
        let interp_fs = FileSource::new_in(arena, interp_text, None);
        assert!(
            !FileSource::identical(Some(gen_fs), Some(interp_fs)),
            "setup: allocations must not be identical"
        );
        let interp_span = FileSpan::new(Some(interp_fs), 0, interp_text.len());
        let interp = Interpolation::new(vec![], vec![], interp_span).unwrap();
        let map = InterpolationMap::new(interp, vec![]).unwrap();
        let map_ref: &'parse InterpolationMap<'parse> = arena.alloc(map);
        let parser = Parser::new(gen_fs, Syntax::Scss, Some(map_ref));
        (parser, map_ref, gen_fs)
    }

    fn mapped_span_file<'a>(res: &'a ParseError<'a>) -> Option<&'a FileSource<'a>> {
        match res {
            ParseError::Format(fmt) => fmt.span.file(),
            ParseError::Scan(scan) => scan.span.file(),
            ParseError::MultiSpan(ms) => ms.span.file(),
            ParseError::Sass(_) => None,
        }
    }

    #[test]
    fn test_wrap_format_detects_remap_identical_content() {
        // Generated `1..3` in B, interpolation `"hello"` `0..5` in A:
        // mapping lands on the interp file, so the error must come back
        // mapped (file `ptr::eq` to A). Structural `==` sees equal content
        // and wrongly returns the error unchanged. (`0..2` would land back
        // at `0..2` and be a genuine no-op; `1..3` remaps to `1..3` in a
        // different file, so identity is the only signal.)
        let arena = Bump::new();
        let (mut parser, _, gen_file) = make_mapped_parser(&arena, "hello", "hello");
        let err_span = FileSpan::new(Some(gen_file), 1, 3);
        let res: ParseResult<'_, ()> =
            wrap_span_format_exception_impl(&mut parser.scanner, &mut parser.state, |_, _| {
                Err(Box::new(ParseError::Format(ParseFormatError {
                    message: "boom".into(),
                    span: err_span,
                    cause: None,
                })))
            });
        let err = res.unwrap_err();
        let file = mapped_span_file(&err).expect("Format error keeps a FileSpan");
        assert!(
            !FileSource::identical(Some(file), Some(gen_file)),
            "error must be remapped off the generated file"
        );
        assert_eq!(file.text(), "hello");
    }

    #[test]
    fn test_wrap_scan_detects_remap_identical_content() {
        // Same setup through the `Scan` arm (`parser.rs` Scan
        // change-detection): mapped span must live in the interpolation
        // file, not the generated file.
        let arena = Bump::new();
        let (mut parser, _, gen_file) = make_mapped_parser(&arena, "hello", "hello");
        let err_span = FileSpan::new(Some(gen_file), 1, 3);
        let res: ParseResult<'_, ()> =
            wrap_span_format_exception_impl(&mut parser.scanner, &mut parser.state, |_, _| {
                Err(Box::new(ParseError::Scan(ScanError {
                    message: "boom".into(),
                    span: err_span,
                    cause: None,
                })))
            });
        let err = res.unwrap_err();
        let file = mapped_span_file(&err).expect("Scan error keeps a FileSpan");
        assert!(
            !FileSource::identical(Some(file), Some(gen_file)),
            "error must be remapped off the generated file"
        );
        assert_eq!(file.text(), "hello");
    }

    #[test]
    fn test_wrap_multispan_primary_detects_remap_identical_content() {
        // MultiSpan primary through its change-detection arm: mapped
        // primary must live in the interpolation file.
        let arena = Bump::new();
        let (mut parser, _, gen_file) = make_mapped_parser(&arena, "hello", "hello");
        let err_span = FileSpan::new(Some(gen_file), 1, 3);
        let res: ParseResult<'_, ()> =
            wrap_span_format_exception_impl(&mut parser.scanner, &mut parser.state, |_, _| {
                Err(Box::new(ParseError::MultiSpan(ParseMultiSpanError {
                    message: "boom".into(),
                    span: err_span,
                    primary_label: Some(String::new()),
                    secondary: vec![],
                    original_source: None,
                    cause: None,
                })))
            });
        match *res.unwrap_err() {
            ParseError::MultiSpan(ms) => {
                assert!(
                    !FileSource::identical(Some(ms.span.file().unwrap()), Some(gen_file)),
                    "primary must be remapped off the generated file"
                );
            }
            other => panic!("expected MultiSpan, got {other:?}"),
        }
    }

    #[test]
    fn test_wrap_multispan_secondary_detects_remap_identical_content() {
        // MultiSpan secondary through its change-detection arm: mapped
        // secondary must live in the interpolation file.
        let arena = Bump::new();
        let (mut parser, _, gen_file) = make_mapped_parser(&arena, "hello", "hello");
        let err_span = FileSpan::new(Some(gen_file), 1, 3);
        let sec_span = FileSpan::new(Some(gen_file), 2, 4);
        let res: ParseResult<'_, ()> =
            wrap_span_format_exception_impl(&mut parser.scanner, &mut parser.state, |_, _| {
                Err(Box::new(ParseError::MultiSpan(ParseMultiSpanError {
                    message: "boom".into(),
                    span: err_span,
                    primary_label: Some(String::new()),
                    secondary: vec![(sec_span, "note".into())],
                    original_source: None,
                    cause: None,
                })))
            });
        match *res.unwrap_err() {
            ParseError::MultiSpan(ms) => {
                assert_eq!(ms.secondary.len(), 1);
                assert!(
                    !FileSource::identical(Some(ms.secondary[0].0.file().unwrap()), Some(gen_file)),
                    "secondary must be remapped off the generated file"
                );
            }
            other => panic!("expected MultiSpan, got {other:?}"),
        }
    }
}
