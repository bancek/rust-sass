// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/media_query.dart
// go-source: go/value/parse_media_query.go

//! Parser for resolved `@media` queries (the CSS AST pass).
//!
//! Consumes plain CSS text into [`CssMediaQuery`]s: a parenthesized
//! condition with an optional `and`/`or` tail, or a `not`/`only`/`<type>`
//! prefix form. Somewhat duplicated in `media_style.rs`; changes to the
//! query shape should be mirrored there and vice versa. Newline handling
//! is irrelevant here (`consume_newlines` is always true).

use crate::ast::css::media_query::CssMediaQuery;
use crate::ast::sass::interpolation_map::InterpolationMap;
use crate::common::source_span_file_source::FileSource;
use crate::common::span_scanner::SpanScanner;
use crate::parse::parser::ParserState;

use crate::parse::parser::{
    declaration_value_impl, expect_whitespace_impl, identifier_impl, looking_at_identifier,
    scan_identifier_impl, whitespace_impl, wrap_span_format_exception_impl, ParseResult, Parser,
};
use crate::parse::stylesheet::Syntax;

/// A parser for `@media` queries.
///
/// Created from already-interpolated CSS text; the interpolation map
/// threads through so errors inside `@media #{}` point back at the
/// interpolation site.
pub struct CssMediaQueryParser<'parse> {
    parser: Parser<'parse>,
}

impl<'parse> CssMediaQueryParser<'parse> {
    pub fn new(source: &'parse FileSource<'parse>) -> Self {
        CssMediaQueryParser {
            parser: Parser::new(source, Syntax::Scss, None),
        }
    }

    /// Matches Dart: `MediaQueryParser(contents, url:, interpolationMap:)`.
    /// The map threads through so parse errors inside `@media #{}`
    /// interpolation point back at the interpolation site (with the
    /// "error in interpolated output" secondary span).
    pub fn new_with_map(
        source: &'parse FileSource<'parse>,
        interpolation_map: Option<&'parse InterpolationMap<'parse>>,
    ) -> Self {
        CssMediaQueryParser {
            parser: Parser::new(source, Syntax::Scss, interpolation_map),
        }
    }

    /// Consumes a comma-separated list of media queries.
    ///
    /// Fails on trailing input once the queries no longer match (e.g. a
    /// stray `and (…)` after `not (…)`), since the tail is parsed as part
    /// of the condition, not as a new query.
    pub fn parse(&mut self) -> ParseResult<'parse, Vec<CssMediaQuery>> {
        wrap_span_format_exception_impl(
            &mut self.parser.scanner,
            &mut self.parser.state,
            |scanner, state| {
                let mut queries = Vec::new();
                loop {
                    whitespace_impl(scanner, state, true)?;
                    let q = _media_query_impl(scanner, state)?;
                    queries.push(q);
                    whitespace_impl(scanner, state, true)?;
                    if !scanner.scan_char(',') {
                        break;
                    }
                }
                scanner.expect_done()?;
                Ok(queries)
            },
        )
    }
}

// Consumes a single media query: a parenthesized condition with an optional
// `and`/`or` tail, or a `not`/`only`/`<type>` prefix form such as
// `@media screen and ...` or `@media only screen and ...`. A `not` followed
// by an identifier is a modifier (`not screen`); followed by `(` it wraps the
// condition (`(not (color))`).
fn _media_query_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
) -> ParseResult<'parse, CssMediaQuery> {
    if scanner.peek_char(0) == '(' as i32 {
        let cond = _media_in_parens_impl(scanner, state)?;
        let mut conditions = vec![cond];
        whitespace_impl(scanner, state, true)?;

        let mut conjunction = true;
        if scan_identifier_impl(scanner, "and", false)? {
            expect_whitespace_impl(scanner, state, false)?;
            let seq = _media_logic_sequence_impl(scanner, state, "and")?;
            conditions.extend(seq);
        } else if scan_identifier_impl(scanner, "or", false)? {
            expect_whitespace_impl(scanner, state, false)?;
            conjunction = false;
            let seq = _media_logic_sequence_impl(scanner, state, "or")?;
            conditions.extend(seq);
        }

        return Ok(CssMediaQuery::new_condition(conditions, Some(conjunction)).unwrap());
    }

    let identifier1 = identifier_impl(scanner, state, false, false)?;

    if identifier1.eq_ignore_ascii_case("not") {
        expect_whitespace_impl(scanner, state, false)?;
        if !looking_at_identifier(scanner, None) {
            let cond = _media_in_parens_impl(scanner, state)?;
            return Ok(CssMediaQuery::new_condition(vec![format!("(not {cond})")], None).unwrap());
        }
    }

    whitespace_impl(scanner, state, true)?;
    if !looking_at_identifier(scanner, None) {
        return Ok(CssMediaQuery::new_type(Some(identifier1), None, vec![]));
    }

    let identifier2 = identifier_impl(scanner, state, false, false)?;

    if identifier2.eq_ignore_ascii_case("and") {
        expect_whitespace_impl(scanner, state, false)?;
        // identifier1 is type
        if scan_identifier_impl(scanner, "not", false)? {
            expect_whitespace_impl(scanner, state, false)?;
            let cond = _media_in_parens_impl(scanner, state)?;
            return Ok(CssMediaQuery::new_type(
                Some(identifier1),
                None,
                vec![format!("(not {cond})")],
            ));
        }
        let seq = _media_logic_sequence_impl(scanner, state, "and")?;
        return Ok(CssMediaQuery::new_type(Some(identifier1), None, seq));
    }

    whitespace_impl(scanner, state, true)?;
    // identifier1 is modifier, identifier2 is type
    if !scan_identifier_impl(scanner, "and", false)? {
        return Ok(CssMediaQuery::new_type(
            Some(identifier2),
            Some(identifier1),
            vec![],
        ));
    }

    expect_whitespace_impl(scanner, state, false)?;
    if scan_identifier_impl(scanner, "not", false)? {
        expect_whitespace_impl(scanner, state, false)?;
        let cond = _media_in_parens_impl(scanner, state)?;
        return Ok(CssMediaQuery::new_type(
            Some(identifier2),
            Some(identifier1),
            vec![format!("(not {cond})")],
        ));
    }

    let seq = _media_logic_sequence_impl(scanner, state, "and")?;
    Ok(CssMediaQuery::new_type(
        Some(identifier2),
        Some(identifier1),
        seq,
    ))
}

// Consumes one or more `<media-in-parens>` expressions separated by
// `operator` and returns them.
fn _media_logic_sequence_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
    operator: &str,
) -> ParseResult<'parse, Vec<String>> {
    let mut result = Vec::new();
    loop {
        let cond = _media_in_parens_impl(scanner, state)?;
        result.push(cond);
        whitespace_impl(scanner, state, true)?;
        if !scan_identifier_impl(scanner, operator, false)? {
            return Ok(result);
        }
        expect_whitespace_impl(scanner, state, false)?;
    }
}

// Consumes a `<media-in-parens>` expression and returns it, parentheses
// included.
//
// The inner value parses as an unrestricted declaration value, so Sass
// expressions are preserved verbatim for later evaluation.
fn _media_in_parens_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
) -> ParseResult<'parse, String> {
    if !scanner.scan_char('(') {
        return Err(scanner
            .error("expected media condition in parentheses.", None, 0)
            .into());
    }
    let value = declaration_value_impl(scanner, state, false)?;
    scanner.expect_char(')')?;
    Ok(format!("({value})"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bumpalo::Bump;

    fn make_parser<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
    ) -> CssMediaQueryParser<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        CssMediaQueryParser::new(fs)
    }

    #[test]
    fn test_parse_type_only() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "screen");
        let queries = parser.parse().unwrap();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].media_type.as_deref(), Some("screen"));
    }

    #[test]
    fn test_parse_type_with_modifier() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "only screen");
        let queries = parser.parse().unwrap();
        assert_eq!(queries[0].modifier.as_deref(), Some("only"));
        assert_eq!(queries[0].media_type.as_deref(), Some("screen"));
    }

    #[test]
    fn test_parse_not_condition() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "not (color)");
        let queries = parser.parse().unwrap();
        assert_eq!(queries[0].conditions, vec!["(not (color))"]);
    }

    #[test]
    fn test_parse_condition() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "(color)");
        let queries = parser.parse().unwrap();
        assert_eq!(queries[0].conditions, vec!["(color)"]);
    }

    #[test]
    fn test_parse_and() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "screen and (color)");
        let queries = parser.parse().unwrap();
        assert_eq!(queries[0].media_type.as_deref(), Some("screen"));
        assert_eq!(queries[0].conditions, vec!["(color)"]);
    }

    #[test]
    fn test_parse_or() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "(color) or (monochrome)");
        let queries = parser.parse().unwrap();
        assert!(!queries[0].conjunction);
        assert_eq!(queries[0].conditions.len(), 2);
    }

    #[test]
    fn test_parse_comma_separated() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "screen, print");
        let queries = parser.parse().unwrap();
        assert_eq!(queries.len(), 2);
    }

    #[test]
    fn test_parse_modifier_and_not() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "screen and not (color)");
        let queries = parser.parse().unwrap();
        assert_eq!(queries[0].modifier, None);
        assert_eq!(queries[0].conditions, vec!["(not (color))"]);
    }

    #[test]
    fn test_parse_only_and_not() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "only screen and not (color)");
        let queries = parser.parse().unwrap();
        assert_eq!(queries[0].modifier.as_deref(), Some("only"));
        assert_eq!(queries[0].media_type.as_deref(), Some("screen"));
        assert_eq!(queries[0].conditions, vec!["(not (color))"]);
    }

    #[test]
    fn test_parse_error_empty() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "");
        let result = parser.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_not_type() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "not screen");
        let queries = parser.parse().unwrap();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].modifier.as_deref(), Some("not"));
        assert_eq!(queries[0].media_type.as_deref(), Some("screen"));
    }

    #[test]
    fn test_parse_only_type() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "only screen");
        let queries = parser.parse().unwrap();
        assert_eq!(queries[0].modifier.as_deref(), Some("only"));
        assert_eq!(queries[0].media_type.as_deref(), Some("screen"));
    }

    #[test]
    fn test_parse_error_missing_close_paren() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "(color");
        let result = parser.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_triple_and() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "(a) and (b) and (c)");
        let queries = parser.parse().unwrap();
        assert_eq!(queries[0].conditions.len(), 3);
    }

    #[test]
    fn test_parse_single_condition() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "(a)");
        let queries = parser.parse().unwrap();
        assert_eq!(queries[0].conditions.len(), 1);
    }

    #[test]
    fn test_parse_error_no_open_paren() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "color)");
        let result = parser.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_error_trailing_comma() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "screen,");
        let result = parser.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_not_condition_with_and() {
        // "not (color) and (monochrome)" is parsed as a single condition
        // "(not (color))" with "and (monochrome)" as trailing — expect_done fails.
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "not (color) and (monochrome)");
        let result = parser.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_only_type_with_and() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "only screen and (color)");
        let queries = parser.parse().unwrap();
        assert_eq!(queries[0].modifier.as_deref(), Some("only"));
        assert_eq!(queries[0].media_type.as_deref(), Some("screen"));
        assert_eq!(queries[0].conditions.len(), 1);
    }

    #[test]
    fn test_parse_type_and_not() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "screen and not (color)");
        let queries = parser.parse().unwrap();
        assert_eq!(queries[0].media_type.as_deref(), Some("screen"));
        assert_eq!(queries[0].modifier, None);
        assert_eq!(queries[0].conditions, vec!["(not (color))"]);
    }
}
