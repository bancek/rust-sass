// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/at_root_query.dart
// go-source: go/value/parse_at_root_query.go

use crate::ast::sass::interpolation_map::InterpolationMap;
use std::collections::HashSet;

use crate::ast::sass::at_root_query::AtRootQuery;
use crate::common::source_span_file_source::FileSource;

use crate::parse::parser::{
    expect_identifier_impl, identifier_impl, looking_at_identifier, scan_identifier_impl,
    whitespace_impl, wrap_span_format_exception_impl, ParseResult, Parser,
};
use crate::parse::stylesheet::Syntax;

/// A parser for `@at-root` queries.
pub struct AtRootQueryParser<'parse> {
    parser: Parser<'parse>,
}

impl<'parse> AtRootQueryParser<'parse> {
    pub fn new(
        source: &'parse FileSource<'parse>,
        interpolation_map: Option<&'parse InterpolationMap<'parse>>,
    ) -> Self {
        AtRootQueryParser {
            parser: Parser::new(source, Syntax::Scss, interpolation_map),
        }
    }

    pub fn parse(&mut self) -> ParseResult<'parse, AtRootQuery> {
        wrap_span_format_exception_impl(
            &mut self.parser.scanner,
            &mut self.parser.state,
            |scanner, state| {
                scanner.expect_char('(')?;
                whitespace_impl(scanner, state, true)?;

                let include = scan_identifier_impl(scanner, "with", false)?;
                if !include {
                    expect_identifier_impl(scanner, "without", "\"with\" or \"without\"", false)?;
                }

                whitespace_impl(scanner, state, true)?;
                scanner.expect_char(':')?;
                whitespace_impl(scanner, state, true)?;

                let mut at_rules = HashSet::new();
                loop {
                    let rule = identifier_impl(scanner, state, false, false)?;
                    at_rules.insert(rule.to_lowercase());
                    whitespace_impl(scanner, state, true)?;
                    if !looking_at_identifier(scanner, None) {
                        break;
                    }
                }

                scanner.expect_char(')')?;
                scanner.expect_done()?;

                Ok(AtRootQuery::new(at_rules, include))
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bumpalo::Bump;

    fn make_parser<'compile, 'parse>(arena: &'compile Bump, text: &str) -> AtRootQueryParser<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        AtRootQueryParser::new(fs, None)
    }

    #[test]
    fn test_parse_with() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "(with: rule)");
        let q = parser.parse().unwrap();
        assert!(q.include);
        assert!(!q.excludes_style_rules());
    }

    #[test]
    fn test_parse_without() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "(without: media)");
        let q = parser.parse().unwrap();
        assert!(!q.include);
        assert!(q.excludes_name("media"));
        assert!(!q.excludes_name("supports"));
    }

    #[test]
    fn test_parse_multiple() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "(with: rule media)");
        let q = parser.parse().unwrap();
        assert!(q.include);
        assert!(!q.excludes_style_rules());
        assert!(!q.excludes_name("media"));
        assert!(q.excludes_name("supports"));
    }

    #[test]
    fn test_parse_case_insensitive() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "(without: MEDIA)");
        let q = parser.parse().unwrap();
        assert!(q.excludes_name("media"));
    }

    #[test]
    fn test_parse_error_no_paren() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "with: rule");
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
    fn test_parse_error_missing_colon() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "(with rule)");
        let result = parser.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_error_missing_keyword() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "(: rule)");
        let result = parser.parse();
        assert!(result.is_err());
    }
}
