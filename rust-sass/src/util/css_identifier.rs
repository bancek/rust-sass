// Copyright 2024 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/util/string.dart (StringExtension.toCssIdentifier)
// go-source: go/util/css_identifier.go

//! Minimally-escaped CSS identifier encoding.
//!
//! Ports Dart's `StringExtension.toCssIdentifier`: the lone `"-"` becomes
//! `\2d`, a leading `--` passes through unescaped, and lone surrogates /
//! `U+0000` / the empty string are errors. Escapes are lowercase hex with a
//! disambiguating space when the next character is itself hex.

use std::fmt::Write;

use bumpalo::Bump;

use crate::common::exception::SassResult;
use crate::common::source_span_file_source::FileSource;
use crate::common::span_scanner::SpanScanner;

use crate::util::character;
use crate::util::string;

/// Returns a minimally-escaped CSS identifier whose contents evaluate to
/// `text`.
///
/// Errors if `text` cannot be represented as a CSS identifier (the empty
/// string, `U+0000`, or a lone surrogate).
/// Matches Dart: `StringExtension.toCssIdentifier`.
pub fn to_css_identifier(text: &str) -> SassResult<String> {
    let arena = Bump::new();
    let source = FileSource::new_in(&arena, text, None);
    let scanner = SpanScanner::new(source);
    do_to_css_identifier(scanner)
}

fn do_to_css_identifier(mut scanner: SpanScanner) -> SassResult<String> {
    let mut buf = String::new();

    if scanner.scan_char('-') {
        if scanner.is_done() {
            return Ok("\\2d".into());
        }
        buf.push('-');
        if scanner.scan_char('-') {
            buf.push('-');
            write_body(&mut scanner, &mut buf)?;
            return Ok(buf);
        }
    }

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
                .error(
                    "The U+0000 can't be represented as a CSS identifier.",
                    None,
                    0,
                )
                .into());
        }
        ch if string::is_high_surrogate(ch) => {
            consume_surrogate_pair(&mut scanner, &mut buf)?;
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
        ch if character::is_name_start(char::from_u32(ch as u32).unwrap_or('\0'))
            && !string::is_private_use_bmp(ch) =>
        {
            let c = scanner.read_char()?;
            buf.push(c);
        }
        _ => {
            let c = scanner.read_char()?;
            write_escape(&mut buf, c as i32, &scanner);
        }
    }

    write_body(&mut scanner, &mut buf)?;
    Ok(buf)
}

// Writes the identifier tail: name characters verbatim, everything else as
// `\hex` escapes (Dart `toCssIdentifier` trailing loop).
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
            _ => {
                let c = scanner.read_char()?;
                write_escape(buf, c as i32, scanner);
            }
        }
    }
}

// Writes `\hex` for `ch`, plus a disambiguating space when the following
// character is itself hex (Dart `writeEscape` closure).
fn write_escape(buf: &mut String, ch: i32, scanner: &SpanScanner) {
    buf.push('\\');
    write!(buf, "{ch:x}").unwrap();
    let next = scanner.peek_char(0);
    if next >= 0 && character::is_hex(char::from_u32(next as u32).unwrap_or('\0')) {
        buf.push(' ');
    }
}

// Consumes a high/low surrogate pair: private-use supplementary code points
// are hex-escaped, other pairs pass through verbatim; a lone high surrogate
// is an error (Dart `consumeSurrogatePair` closure).
fn consume_surrogate_pair(scanner: &mut SpanScanner, buf: &mut String) -> SassResult<()> {
    let high = scanner.read_char()? as i32;
    let next = scanner.peek_char(0);
    if next < 0 || !string::is_low_surrogate(next) {
        return Err(scanner
            .error(
                "An individual surrogate can't be represented as a CSS identifier.",
                Some(scanner.pos()),
                1,
            )
            .into());
    }
    let low = scanner.read_char()? as i32;
    if string::is_private_use_high_surrogate(high) {
        write_escape(buf, string::combine_surrogates(high, low), scanner);
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
        assert_eq!(to_css_identifier("-").unwrap(), "\\2d");
    }

    #[test]
    fn test_empty() {
        assert!(to_css_identifier("").is_err());
    }

    #[test]
    fn test_starts_with_digit() {
        let out = to_css_identifier("1foo").unwrap();
        assert!(out.contains('\\'), "should escape leading digit: {out}");
        assert!(out.contains("31"), "should contain hex code 31: {out}");
    }

    #[test]
    fn test_underscore() {
        assert_eq!(to_css_identifier("_foo").unwrap(), "_foo");
    }
}
