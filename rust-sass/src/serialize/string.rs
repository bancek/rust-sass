// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

//! Quoted/unquoted string emission: quote auto-detection, backslash and
//! control-char escapes, and private-use escapes (Dart's `_visitQuotedString`,
//! `_visitUnquotedString`, `_tryPrivateUseCharacter`, `_writeEscape`).
//!
//! Compressed output passes private-use characters through raw; expanded mode
//! escapes them since glyph fonts benefit from distinguishable escapes.

// dart-source: lib/src/visitor/serialize.dart (_visitQuotedString, _visitUnquotedString, _tryPrivateUseCharacter, _writeEscape)
// go-source: go/value/visitor_string.go

use std::fmt::Write;

use crate::source_map_buffer::SourceMapBuffer;
use crate::util::string::{combine_surrogates, is_private_use_bmp, is_private_use_high_surrogate};

use crate::serialize::{OutputStyle, SerializeState};

/// Writes `s` with auto-detected quotes (double unless the text contains
/// one, single if it contains a double but no single; Dart's
/// `_visitQuotedString` default).
pub(crate) fn visit_quoted_string(buf: &mut SourceMapBuffer<'_>, state: &SerializeState, s: &str) {
    visit_quoted_string_force(buf, state, s, false);
}

/// Quoted-string writer; `force_double` always uses `"` (escaping inner
/// doubles). When both quote kinds appear, restarts in force-double mode so
/// `"` is escaped and `'` stays raw (Dart's `forceDoubleQuote`).
pub(crate) fn visit_quoted_string_force(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    s: &str,
    force_double: bool,
) {
    let mut includes_single_quote = false;
    let mut includes_double_quote = false;
    let mut content_buf = String::new();
    let cbuf = &mut content_buf;
    if force_double {
        cbuf.push('"');
    }

    for (i, ch) in s.char_indices() {
        match ch {
            '\'' if force_double => {
                cbuf.push('\'');
            }
            '\'' if includes_double_quote => {
                visit_quoted_string_force(buf, state, s, true);
                return;
            }
            '\'' => {
                includes_single_quote = true;
                cbuf.push('\'');
            }
            '"' if force_double => {
                cbuf.push('\\');
                cbuf.push('"');
            }
            '"' if includes_single_quote => {
                visit_quoted_string_force(buf, state, s, true);
                return;
            }
            '"' => {
                includes_double_quote = true;
                cbuf.push('"');
            }
            '\\' => {
                cbuf.push('\\');
                cbuf.push('\\');
            }
            _ if ch != '\t' && (ch as u32 <= 0x1F || ch as u32 == 0x7F) => {
                let next_ch = s[i + ch.len_utf8()..].chars().next();
                write_escape(cbuf, ch as i32, next_ch);
            }
            _ => {
                let (scalar, next_ch, new_idx) = try_private_use(state, ch, s, i);
                match new_idx {
                    Some(ni) => {
                        write_escape(cbuf, scalar, next_ch);
                        if ni > i {
                            continue;
                        }
                    }
                    None => {
                        cbuf.push(ch);
                    }
                }
            }
        }
    }

    if force_double {
        cbuf.push('"');
        write!(buf, "{}", content_buf).unwrap();
    } else {
        let quote = if includes_double_quote { '\'' } else { '"' };
        buf.write_char(quote).unwrap();
        write!(buf, "{}", content_buf).unwrap();
        buf.write_char(quote).unwrap();
    }
}

/// Writes `s` raw, folding newlines to single spaces and collapsing spaces
/// after newlines (Dart's `_visitUnquotedString`).
pub(crate) fn visit_unquoted_string(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    s: &str,
) {
    let mut after_newline = false;
    for (i, ch) in s.char_indices() {
        match ch {
            '\n' => {
                buf.write_char(' ').unwrap();
                after_newline = true;
            }
            ' ' => {
                if !after_newline {
                    buf.write_char(' ').unwrap();
                }
            }
            _ => {
                after_newline = false;
                let (scalar, next_ch, new_idx) = try_private_use(state, ch, s, i);
                match new_idx {
                    Some(ni) => {
                        write_escape(buf, scalar, next_ch);
                        if ni > i {
                            continue;
                        }
                    }
                    None => {
                        write!(buf, "{}", ch).unwrap();
                    }
                }
            }
        }
    }
}

/// Returns `(scalar-to-escape, char-after-escape, resume-index)`.
///
/// Dart `_tryPrivateUseCharacter` tests UTF-16 code units: BMP PUA
/// (`U+E000–U+F8FF`) escapes the unit itself, and supplementary-PUA high
/// surrogates (`0xDB80–0xDBFF`) escape the combined scalar. Rust iterates
/// scalar `char`s, so the supplementary planes (`U+F0000–U+FFFFD`,
/// `U+100000–U+10FFFD`) match directly as scalars — no surrogate halves
/// exist to combine.
fn try_private_use(
    state: &SerializeState,
    ch: char,
    s: &str,
    i: usize,
) -> (i32, Option<char>, Option<usize>) {
    if matches!(state.style, OutputStyle::Compressed) {
        return (ch as i32, None, None);
    }
    let code_unit = ch as i32;
    if is_private_use_bmp(code_unit) {
        let next_ch = s[i + char_len_at(s, i)..].chars().next();
        return (code_unit, next_ch, Some(i));
    }
    let scalar = ch as u32;
    if (0xF0000..=0xFFFFD).contains(&scalar) || (0x100000..=0x10FFFD).contains(&scalar) {
        let skip = char_len_at(s, i);
        let after_next = s[i + skip..].chars().next();
        return (scalar as i32, after_next, Some(i));
    }
    if is_private_use_high_surrogate(code_unit) {
        let skip = char_len_at(s, i);
        let rest = &s[i + skip..];
        if let Some(next_ch) = rest.chars().next() {
            let _combined = combine_surrogates(code_unit, next_ch as i32);
            let after_next = rest[next_ch.len_utf8()..].chars().next();
            return (_combined, after_next, Some(i));
        }
    }
    (code_unit, None, None)
}

/// Writes `character` as a `\hex` escape, with a disambiguating trailing
/// space when the next char is hex, space, or tab (Dart's `_writeEscape`).
fn write_escape(buf: &mut impl Write, character: i32, next_char: Option<char>) {
    buf.write_char('\\').unwrap();
    write!(buf, "{:x}", character).unwrap();
    if let Some(next) = next_char {
        if next.is_ascii_hexdigit() || next == ' ' || next == '\t' {
            buf.write_char(' ').unwrap();
        }
    }
}

fn char_len_at(s: &str, i: usize) -> usize {
    s[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::LINE_FEED_LF;
    use crate::serialize::{OutputStyle, SerializeState};
    use crate::source_map_buffer::SourceMapBuffer;

    fn quoted(s: &str, compressed: bool) -> String {
        let state = SerializeState {
            indentation: 0,
            style: if compressed {
                OutputStyle::Compressed
            } else {
                OutputStyle::Expanded
            },
            source_comments: false,
            inspect: false,
            quote: true,
            line_feed: LINE_FEED_LF,
            indent_char: ' ',
            indent_width: 2,
        };
        let mut buf = SourceMapBuffer::new_plain();
        visit_quoted_string(&mut buf, &state, s);
        buf.into_string()
    }

    #[rust_sass_macros::maybe_test]
    async fn test_astral_pua_escaped() {
        // Supplementary-plane PUA chars escape as
        // `\f0000` like Dart (whose UTF-16 high-surrogate test fires);
        // the scalar code-point test missed them and emitted raw.
        assert_eq!(quoted("󰀀", false), "\"\\f0000\"");
        // BMP PUA still escapes; plain astral non-PUA stays raw.
        assert_eq!(quoted("", false), "\"\\e000\"");
        assert_eq!(quoted("😀", false), "\"😀\"");
    }
}
