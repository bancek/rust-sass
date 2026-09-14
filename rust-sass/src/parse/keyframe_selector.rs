// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/keyframe_selector.dart
// go-source: go/value/parse_keyframe_selector.go

//! Parser for `@keyframes` block selectors: `from`, `to`, and percentages
//! (`50%`, including `+`/decimal/exponent forms). `consume_newlines` is
//! always true, so newline handling needs no per-call discipline.

use crate::common::source_span_file_source::FileSource;
use crate::common::span_scanner::SpanScanner;
use crate::util::character;

use crate::parse::parser::{
    expect_identifier_impl, looking_at_identifier, scan_ident_char_impl, scan_identifier_impl,
    whitespace_impl, wrap_span_format_exception_impl, ParseResult, Parser,
};
use crate::parse::stylesheet::Syntax;

/// A parser for `@keyframes` block selectors.
pub struct KeyframeSelectorParser<'parse> {
    parser: Parser<'parse>,
}

impl<'parse> KeyframeSelectorParser<'parse> {
    pub fn new(source: &'parse FileSource<'parse>) -> Self {
        KeyframeSelectorParser {
            parser: Parser::new(source, Syntax::Scss, None),
        }
    }

    /// Consumes a comma-separated list of keyframe selectors.
    ///
    /// An identifier parses as `from` (else `to`, with the error message
    /// `'"to" or "from"'`); anything else parses as a percentage.
    pub fn parse(&mut self) -> ParseResult<'parse, Vec<String>> {
        wrap_span_format_exception_impl(
            &mut self.parser.scanner,
            &mut self.parser.state,
            |scanner, state| {
                let mut selectors = Vec::new();
                loop {
                    whitespace_impl(scanner, state, true)?;
                    if looking_at_identifier(scanner, None) {
                        let ok = scan_identifier_impl(scanner, "from", false)?;
                        if ok {
                            selectors.push("from".to_string());
                        } else {
                            expect_identifier_impl(scanner, "to", "\"to\" or \"from\"", false)?;
                            selectors.push("to".to_string());
                        }
                    } else {
                        let s = _percentage_impl(scanner)?;
                        selectors.push(s);
                    }
                    whitespace_impl(scanner, state, true)?;
                    if !scanner.scan_char(',') {
                        break;
                    }
                }
                scanner.expect_done()?;
                Ok(selectors)
            },
        )
    }
}

// Consumes one percentage keyframe selector (`50%`, with optional `+`
// prefix, decimal part, and `e`/`E` exponent) and returns its text,
// `%` included. A missing mantissa is `"Expected number."`; a dangling
// exponent is `"Expected digit."`.
fn _percentage_impl<'parse>(scanner: &mut SpanScanner<'parse>) -> ParseResult<'parse, String> {
    let mut buf = String::new();
    if scanner.scan_char('+') {
        buf.push('+');
    }

    let second = scanner.peek_char(0);
    if !character::is_digit((second as u8) as char) && second != '.' as i32 {
        return Err(Box::new(scanner.error("Expected number.", None, 0).into()));
    }

    while character::is_digit(scanner.peek_char(0) as u8 as char) {
        let ch = scanner.read_char()?;
        buf.push(ch);
    }

    if scanner.peek_char(0) == '.' as i32 {
        let ch = scanner.read_char()?;
        buf.push(ch);
        while character::is_digit(scanner.peek_char(0) as u8 as char) {
            let ch = scanner.read_char()?;
            buf.push(ch);
        }
    }

    let mut ok = scan_ident_char_impl(scanner, 'e' as i32, true)?;
    if !ok {
        ok = scan_ident_char_impl(scanner, 'E' as i32, true)?;
    }
    if ok {
        buf.push('e');
        let ch = scanner.peek_char(0);
        if ch == '+' as i32 || ch == '-' as i32 {
            let c = scanner.read_char()?;
            buf.push(c);
        }
        if !character::is_digit(scanner.peek_char(0) as u8 as char) {
            return Err(Box::new(scanner.error("Expected digit.", None, 0).into()));
        }
        loop {
            let c = scanner.read_char()?;
            buf.push(c);
            if !character::is_digit(scanner.peek_char(0) as u8 as char) {
                break;
            }
        }
    }

    scanner.expect_char('%')?;
    buf.push('%');
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bumpalo::Bump;

    fn make_parser<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
    ) -> KeyframeSelectorParser<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        KeyframeSelectorParser::new(fs)
    }

    #[test]
    fn test_parse_from() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "from");
        let selectors = parser.parse().unwrap();
        assert_eq!(selectors, vec!["from"]);
    }

    #[test]
    fn test_parse_to() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "to");
        let selectors = parser.parse().unwrap();
        assert_eq!(selectors, vec!["to"]);
    }

    #[test]
    fn test_parse_percentage() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "50%");
        let selectors = parser.parse().unwrap();
        assert_eq!(selectors, vec!["50%"]);
    }

    #[test]
    fn test_parse_percentage_decimal() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "12.5%");
        let selectors = parser.parse().unwrap();
        assert_eq!(selectors, vec!["12.5%"]);
    }

    #[test]
    fn test_parse_percentage_with_plus() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "+50%");
        let selectors = parser.parse().unwrap();
        assert_eq!(selectors[0], "+50%");
    }

    #[test]
    fn test_parse_percentage_exponent() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "1e2%");
        let selectors = parser.parse().unwrap();
        assert_eq!(selectors[0], "1e2%");
    }

    #[test]
    fn test_parse_percentage_exponent_capital() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "3E4%");
        let selectors = parser.parse().unwrap();
        assert_eq!(selectors[0], "3e4%");
    }

    #[test]
    fn test_parse_comma_separated() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "from, to");
        let selectors = parser.parse().unwrap();
        assert_eq!(selectors, vec!["from", "to"]);
    }

    #[test]
    fn test_parse_error() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "foo");
        let result = parser.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_error_empty() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "");
        let result = parser.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_percentage_exponent_minus() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "1e-2%");
        let selectors = parser.parse().unwrap();
        assert_eq!(selectors[0], "1e-2%");
    }

    #[test]
    fn test_parse_percentage_error_not_number() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "+x%");
        let result = parser.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_percentage_error_no_percent() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "50");
        let result = parser.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_comma_percentages() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "50%, 100%");
        let selectors = parser.parse().unwrap();
        assert_eq!(selectors, vec!["50%", "100%"]);
    }

    #[test]
    fn test_parse_percentage_dot_start() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".5%");
        let selectors = parser.parse().unwrap();
        assert_eq!(selectors[0], ".5%");
    }

    #[test]
    fn test_parse_percentage_exponent_error_no_digit() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "1e%");
        let result = parser.parse();
        assert!(result.is_err());
    }
}
