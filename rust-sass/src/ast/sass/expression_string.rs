// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/string.dart
// go-source: go/value/sass_expression_string.go

use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::interpolation::{Interpolation, InterpolationPart};

/// A string literal.
#[derive(Clone, Debug)]
pub struct StringExpression<'parse> {
    /// Interpolation that, when evaluated, produces the contents of this string.
    ///
    /// If this is a quoted string, escapes are resolved and quotes are not
    /// included in this text (unlike [`as_interpolation`](Self::as_interpolation)).
    /// If it's an unquoted string, escapes are *not* resolved.
    pub text: Interpolation<'parse>,
    /// Whether `self` has quotes.
    pub has_quotes: bool,
}

impl<'parse> StringExpression<'parse> {
    pub fn new(text: Interpolation<'parse>, has_quotes: bool) -> Self {
        StringExpression { text, has_quotes }
    }

    /// Returns a string expression with no interpolation.
    pub fn plain(text: &str, span: FileSpan<'parse>, has_quotes: bool) -> Self {
        StringExpression {
            text: Interpolation::plain(text.into(), span),
            has_quotes,
        }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        Some(&self.text)
    }

    /// Interpolation that, when evaluated, produces the syntax of this string.
    ///
    /// Unlike [`text`](Self::text), this doesn't resolve escapes and does
    /// include quotes for quoted strings.
    ///
    /// If `static_` is true, this escapes any `#{` sequences in the string. If
    /// `quote` is passed, it uses that character as the quote mark; otherwise,
    /// it determines the best quote to use by looking at the string.
    pub fn as_interpolation(
        &self,
        static_: bool,
        quote: Option<char>,
    ) -> SassResult<Interpolation<'parse>> {
        if !self.has_quotes {
            return Ok(self.text.clone());
        }

        let texts: Vec<&str> = self
            .text
            .contents
            .iter()
            .filter_map(|p| match p {
                InterpolationPart::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        let q = quote.unwrap_or_else(|| best_quote(&texts));

        let mut buf = InterpolationBuffer::new();
        buf.write_char_code(q);

        for (i, part) in self.text.contents.iter().enumerate() {
            match part {
                InterpolationPart::Expression(expr) => {
                    let sp = self.text.span_for_element(i)?;
                    buf.add(*expr.clone(), sp);
                }
                InterpolationPart::Text(s) => {
                    let mut sb = String::new();
                    quote_inner_text(s, q, &mut sb, static_);
                    buf.write(&sb);
                }
            }
        }
        buf.write_char_code(q);

        let span = self.text.span()?;
        buf.interpolation(span)
    }
}

impl<'parse> AstNode<'parse> for StringExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        self.text.span()
    }
}

impl<'parse> StringExpression<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let interp = self.as_interpolation(false, None)?;
        Ok(format!("{interp}"))
    }
}

impl<'parse> fmt::Display for StringExpression<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

/// Returns the quote character that is best to use when converting `contents`
/// to Sass source.
pub fn best_quote(contents: &[&str]) -> char {
    let mut contains_double_quote = false;
    for s in contents {
        for ch in s.chars() {
            if ch == '\'' {
                return '"';
            }
            if ch == '"' {
                contains_double_quote = true;
            }
        }
    }
    if contains_double_quote {
        '\''
    } else {
        '"'
    }
}

/// Writes to `sb` the contents of a string (without quotes) that evaluates
/// to `text` according to Sass's parsing logic.
///
/// This always adds an escape sequence before `quote`. If `static_` is true,
/// it also escapes any `#{` sequences in the string.
//
// Matches Dart: `StringExpression._quoteInnerText` (private static).
pub fn quote_inner_text(text: &str, quote: char, sb: &mut String, static_: bool) {
    // Dart `_quoteInnerText` iterates UTF-16 code units (`text.codeUnitAt(i)`);
    // Rust iterates `chars` (scalar values). For BMP text the two agree
    // element-wise (surrogate halves only differ for astral chars, which take
    // the default push path on both sides). A byte-wise walk mojibakes
    // non-ASCII text (`café` → `cafÃ©`).
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        match if ch == '\n'
            || ch == '\r'
            || ch == '\x0C'
            || ch == '\\'
            || ch == quote
            || (ch == '#' && static_ && chars.get(i + 1) == Some(&'{'))
        {
            Some(())
        } else {
            None
        } {
            Some(()) if ch == '\n' || ch == '\r' || ch == '\x0C' => {
                sb.push('\\');
                sb.push('a');
                if i != chars.len() - 1 {
                    let next = chars[i + 1];
                    if next == ' '
                        || next == '\t'
                        || next == '\n'
                        || next == '\r'
                        || next == '\x0C'
                        || next.is_ascii_digit()
                        || ('a'..='f').contains(&next)
                        || ('A'..='F').contains(&next)
                    {
                        sb.push(' ');
                    }
                }
            }
            Some(()) => {
                sb.push('\\');
                sb.push(ch);
            }
            None => {
                sb.push(ch);
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    fn test_span<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
        s: usize,
        e: usize,
    ) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), s, e)
    }

    #[test]
    fn test_construction() {
        let arena = Bump::new();
        let span = test_span(&arena, "\"hello\"", 0, 7);
        let expr = StringExpression::plain("hello", span, true);
        assert!(expr.has_quotes);
    }

    #[test]
    fn test_string() {
        let arena = Bump::new();
        let span = test_span(&arena, "\"hello\"", 0, 7);
        let expr = StringExpression::plain("hello", span, true);
        let result = format!("{expr}");
        assert_eq!(result, "\"hello\"");
    }

    #[test]
    fn test_unquoted() {
        let arena = Bump::new();
        let span = test_span(&arena, "hello", 0, 5);
        let expr = StringExpression::plain("hello", span, false);
        assert_eq!(format!("{expr}"), "hello");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_quote_inner_text_non_ascii() {
        // `quote_inner_text` walked bytes, mojibaking
        // non-ASCII (`café` → `cafÃ©`) on on the plain-CSS `@import url()`
        // path (`parse/css.rs` → `as_interpolation`). Dart iterates UTF-16
        // units. End-to-end CLI probe: `@import url("café.css")` as plain CSS
        // must round-trip byte-identically.
        let mut sb = String::new();
        quote_inner_text("café", '"', &mut sb, false);
        assert_eq!(sb, "café", "non-ASCII text must pass through unmangled");
        let mut sb = String::new();
        quote_inner_text("a\nb", '"', &mut sb, false);
        assert_eq!(sb, "a\\a b", "newline escape path unchanged");
        let mut sb = String::new();
        quote_inner_text("say \"hi\"", '"', &mut sb, false);
        assert_eq!(sb, "say \\\"hi\\\"", "quote escape path unchanged");
        let mut sb = String::new();
        quote_inner_text("#{x}", '"', &mut sb, true);
        assert_eq!(sb, "\\#{x}", "static interpolation escape path unchanged");
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let span = test_span(&arena, "\"hello\"", 0, 7);
        let expr = StringExpression::plain("hello", span, true);
        assert!(expr.source_interpolation().is_some());
    }
}
