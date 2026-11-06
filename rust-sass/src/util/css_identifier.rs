// Copyright 2024 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/util/string.dart (StringExtension.toCssIdentifier,
//   resolveCssEscapes, _isSimpleIdentifier)
// go-source: go/util/css_identifier.go

//! Minimally-escaped CSS identifier encoding.
//!
//! Ports Dart's `StringExtension.toCssIdentifier`: the lone `"-"` becomes
//! `\2d `, a leading `--` passes through unescaped, control characters and
//! digits at an identifier start are hex-escaped with a trailing space, other
//! characters are backslash-escaped literally, and lone surrogates / `U+0000` /
//! the empty string are errors.

use std::fmt::Write;

use bumpalo::Bump;

use crate::common::exception::{SassError, SassResult};
use crate::common::source_span_file_source::FileSource;
use crate::common::span_scanner::SpanScanner;
use crate::parse::parser::parse_identifier;
use crate::util::character;
use crate::util::string;

/// Returns a minimally-escaped CSS identifier whose contents evaluate to
/// `text`.
///
/// Errors if `text` cannot be represented as a CSS identifier (the empty
/// string, `U+0000`, or a lone surrogate).
/// Matches Dart: `StringExtension.toCssIdentifier`.
pub fn to_css_identifier(text: &str) -> SassResult<String> {
    to_css_identifier_with_body(text, false)
}

/// Like [`to_css_identifier`], but treats `text` as an identifier body: a
/// leading `-` is emitted literally and leading digits are allowed.
///
/// Matches Dart: `StringExtension.toCssIdentifier(body: true)`.
pub fn to_css_identifier_body(text: &str) -> SassResult<String> {
    to_css_identifier_with_body(text, true)
}

/// Interprets `text` as a CSS identifier and resolves any escape codes it
/// contains.
///
/// Errors with Dart's `ArgumentError` message when `text` isn't a valid CSS
/// identifier.
/// Matches Dart: `StringExtension.resolveCssEscapes`.
pub fn resolve_css_escapes(text: &str) -> SassResult<String> {
    if is_simple_identifier(text) {
        return Ok(text.to_string());
    }
    parse_identifier(text).map_err(|_| {
        Box::new(SassError::Script {
            message: format!("\"{text}\" isn't a valid CSS identifier"),
            argument_name: None,
        }) as Box<SassError>
    })
}

/// Returns whether `text` is a valid CSS identifier that has the same semantic
/// content as the string itself.
///
/// This is a quick check used for optimization. It can return false negatives,
/// but not false positives.
fn is_simple_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !character::is_name_start(first) || string::is_private_use_bmp(first as i32) {
        return false;
    }
    chars.all(|ch| character::is_name(ch) && !string::is_private_use_bmp(ch as i32))
}

fn to_css_identifier_with_body(text: &str, body: bool) -> SassResult<String> {
    if is_simple_identifier(text) {
        return Ok(text.to_string());
    }

    let arena = Bump::new();
    let source = FileSource::new_in(&arena, text, None);
    let scanner = SpanScanner::new(source);
    do_to_css_identifier(scanner, body)
}

fn do_to_css_identifier(mut scanner: SpanScanner, body: bool) -> SassResult<String> {
    let mut buf = String::new();
    let mut double_dash = false;

    if !body && scanner.scan_char('-') {
        if scanner.is_done() {
            return Ok("\\2d ".into());
        }
        buf.push('-');
        if scanner.scan_char('-') {
            buf.push('-');
            double_dash = true;
        }
    }

    if !double_dash {
        match scanner.peek_char(0) {
            -1 => {
                return Err(scanner
                    .error(
                        "The empty string can't be represented as a CSS identifier.",
                        None,
                        0,
                    )
                    .into());
            }
            0 => {
                return Err(scanner
                    .error("U+0000 can't be represented as a CSS identifier.", None, 0)
                    .into());
            }
            ch if string::is_high_surrogate(ch) => {
                consume_surrogate_pair(&mut scanner, &mut buf)?;
            }
            ch if string::is_low_surrogate(ch) => {
                scanner.read_char()?;
                return Err(scanner
                    .error(
                        "An individual surrogates can't be represented as a CSS identifier.",
                        Some(scanner.pos() - 1),
                        1,
                    )
                    .into());
            }
            ch if {
                let c = char::from_u32(ch as u32).unwrap_or('\0');
                character::is_name(c) && !string::is_private_use_bmp(ch) && body
            } =>
            {
                let c = scanner.read_char()?;
                buf.push(c);
            }
            ch if {
                let c = char::from_u32(ch as u32).unwrap_or('\0');
                character::is_name_start(c) && !string::is_private_use_bmp(ch) && !body
            } =>
            {
                let c = scanner.read_char()?;
                buf.push(c);
            }
            ch if ch <= 0x1F
                || ch == 0x7F
                || character::is_digit(char::from_u32(ch as u32).unwrap_or('\0'))
                || string::is_private_use_bmp(ch) =>
            {
                write_hex_escape(&mut buf, scanner.read_char()? as i32);
            }
            _ => {
                let c = scanner.read_char()?;
                buf.push('\\');
                buf.push(c);
            }
        }
    }

    write_body(&mut scanner, &mut buf)?;
    Ok(buf)
}

// Writes the identifier tail: name characters verbatim, control/private-use
// code points as `\hex `, and everything else as a literal backslash escape.
// Matches Dart: `toCssIdentifier`'s trailing loop.
fn write_body(scanner: &mut SpanScanner, buf: &mut String) -> SassResult<()> {
    loop {
        match scanner.peek_char(0) {
            -1 => return Ok(()),
            0 => {
                return Err(scanner
                    .error(
                        "The U+0000 can't be represented as a CSS identifier.",
                        None,
                        0,
                    )
                    .into());
            }
            ch if string::is_high_surrogate(ch) => {
                consume_surrogate_pair(scanner, buf)?;
            }
            ch if string::is_low_surrogate(ch) => {
                scanner.read_char()?;
                return Err(scanner
                    .error(
                        "An individual surrogate can't be represented as a CSS identifier.",
                        Some(scanner.pos() - 1),
                        1,
                    )
                    .into());
            }
            ch if {
                let c = char::from_u32(ch as u32).unwrap_or('\0');
                character::is_name(c) && !string::is_private_use_bmp(ch)
            } =>
            {
                let c = scanner.read_char()?;
                buf.push(c);
            }
            ch if ch <= 0x1F || ch == 0x7F || string::is_private_use_bmp(ch) => {
                write_hex_escape(buf, scanner.read_char()? as i32);
            }
            _ => {
                let c = scanner.read_char()?;
                buf.push('\\');
                buf.push(c);
            }
        }
    }
}

// Writes `\hex` followed by a space (Dart's `writeHexEscape` closure).
fn write_hex_escape(buf: &mut String, ch: i32) {
    buf.push('\\');
    write!(buf, "{ch:x}").unwrap();
    buf.push(' ');
}

// Consumes a high/low surrogate pair: private-use supplementary code points
// are hex-escaped, other pairs pass through verbatim; a lone high surrogate
// is an error (Dart's `consumeSurrogatePair` closure).
fn consume_surrogate_pair(scanner: &mut SpanScanner, buf: &mut String) -> SassResult<()> {
    let high = scanner.read_char()? as i32;
    let next = scanner.peek_char(0);
    if next < 0 || !string::is_low_surrogate(next) {
        return Err(scanner
            .error(
                "An individual surrogates can't be represented as a CSS identifier.",
                Some(scanner.pos()),
                1,
            )
            .into());
    }
    let low = scanner.read_char()? as i32;
    if string::is_private_use_high_surrogate(high) {
        write_hex_escape(buf, string::combine_surrogates(high, low));
    } else {
        buf.push(char::from_u32(high as u32).unwrap_or('\0'));
        buf.push(char::from_u32(low as u32).unwrap_or('\0'));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple() {
        assert_eq!(to_css_identifier("foo").unwrap(), "foo");
    }

    #[test]
    fn test_leading_dash() {
        assert_eq!(to_css_identifier("-foo").unwrap(), "-foo");
    }

    #[test]
    fn test_double_dash() {
        assert_eq!(to_css_identifier("--foo").unwrap(), "--foo");
    }

    #[test]
    fn test_dash_alone() {
        assert_eq!(to_css_identifier("-").unwrap(), "\\2d ");
    }

    #[test]
    fn test_empty() {
        assert!(to_css_identifier("").is_err());
    }

    #[test]
    fn test_starts_with_digit() {
        assert_eq!(to_css_identifier("1foo").unwrap(), "\\31 foo");
    }

    #[test]
    fn test_underscore() {
        assert_eq!(to_css_identifier("_foo").unwrap(), "_foo");
    }

    #[test]
    fn test_punctuation() {
        assert_eq!(to_css_identifier("foo.bar").unwrap(), "foo\\.bar");
        assert_eq!(to_css_identifier("%").unwrap(), "\\%");
    }

    #[test]
    fn test_control_and_private_use() {
        assert_eq!(to_css_identifier("\n").unwrap(), "\\a ");
        assert_eq!(to_css_identifier("\u{00E9}").unwrap(), "\u{00E9}");
        assert_eq!(to_css_identifier("\u{E000}").unwrap(), "\\e000 ");
    }

    #[test]
    fn test_u0000() {
        assert!(to_css_identifier("\0").is_err());
    }

    #[test]
    fn test_body() {
        assert_eq!(to_css_identifier_body("-1foo").unwrap(), "-1foo");
        assert_eq!(to_css_identifier_body("1foo").unwrap(), "1foo");
        assert_eq!(to_css_identifier_body("%").unwrap(), "\\%");
    }

    #[test]
    fn test_resolve_css_escapes() {
        assert_eq!(resolve_css_escapes("foo").unwrap(), "foo");
        assert_eq!(resolve_css_escapes("\\66 oo").unwrap(), "foo");
        assert_eq!(resolve_css_escapes("\\5f").unwrap(), "_");
    }

    #[test]
    fn test_resolve_invalid() {
        let err = resolve_css_escapes("%%").unwrap_err();
        assert_eq!(err.message(), "\"%%\" isn't a valid CSS identifier");
    }
}
