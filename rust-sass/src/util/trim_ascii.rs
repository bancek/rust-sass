// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/utils.dart (trimAscii, trimAsciiLeft, trimAsciiRight, a)
// go-source: go/util/trim_ascii.go

//! ASCII-only trimming and article helpers.
//!
//! Ports `trimAscii*` (like `String.trim*` but ASCII-whitespace only) and `a`
//! from `utils.dart`. The whitespace predicate mirrors Dart's
//! `NullableCharacterExtension.isWhitespace` (space, tab, `\n`, `\r`, form
//! feed) — note this trims by byte, so non-ASCII whitespace is never touched.

/// Like `str::trim`, but only trims ASCII whitespace.
///
/// If `exclude_escape` is `true`, whitespace included in a CSS escape (a
/// trailing space after a backslash) is preserved.
/// Matches Dart: `trimAscii`.
pub fn trim_ascii(s: &str, exclude_escape: bool) -> String {
    let start = match first_non_whitespace(s) {
        Some(i) => i,
        None => return String::new(),
    };
    let end = match last_non_whitespace(s, exclude_escape) {
        Some(i) => i,
        None => return String::new(),
    };
    s[start..=end].to_string()
}

/// Like `str::trim_start`, but only trims ASCII whitespace.
///
/// Matches Dart: `trimAsciiLeft`.
pub fn trim_ascii_left(s: &str) -> String {
    let start = match first_non_whitespace(s) {
        Some(i) => i,
        None => return String::new(),
    };
    s[start..].to_string()
}

/// Like `str::trim_end`, but only trims ASCII whitespace.
///
/// If `exclude_escape` is `true`, whitespace included in a CSS escape is
/// preserved. Matches Dart: `trimAsciiRight`.
pub fn trim_ascii_right(s: &str, exclude_escape: bool) -> String {
    let end = match last_non_whitespace(s, exclude_escape) {
        Some(i) => i,
        None => return String::new(),
    };
    s[..=end].to_string()
}

// Index of the first non-ASCII-whitespace byte, or `None` if all whitespace.
// Matches Dart: `_firstNonWhitespace`.
fn first_non_whitespace(s: &str) -> Option<usize> {
    s.bytes().position(|b| !is_ascii_whitespace(b))
}

// Index of the last non-ASCII-whitespace byte, or `None` if all whitespace.
// With `exclude_escape`, stops after whitespace included in a CSS escape.
// Matches Dart: `_lastNonWhitespace`.
fn last_non_whitespace(s: &str, exclude_escape: bool) -> Option<usize> {
    let bytes = s.as_bytes();
    for i in (0..bytes.len()).rev() {
        if !is_ascii_whitespace(bytes[i]) {
            if exclude_escape && i != 0 && i != bytes.len() - 1 && bytes[i] == b'\\' {
                return Some(i + 1);
            }
            return Some(i);
        }
    }
    None
}

fn is_ascii_whitespace(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' || b == b'\x0C'
}

/// Returns `"an {word}"` when `word` starts with a vowel, else `"a {word}"`.
///
/// Matches Dart: `a`.
pub fn a(word: &str) -> String {
    match word.as_bytes().first() {
        Some(b'a' | b'e' | b'i' | b'o' | b'u') => format!("an {word}"),
        _ => format!("a {word}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_trim() {
        assert_eq!(trim_ascii("  foo  ", false), "foo");
    }

    #[test]
    fn test_tab_and_newline() {
        assert_eq!(trim_ascii("\t\nfoo\r\n", false), "foo");
    }

    #[test]
    fn test_all_whitespace() {
        assert_eq!(trim_ascii("   \t\n  ", false), "");
    }

    #[test]
    fn test_exclude_escape_preserves() {
        assert_eq!(trim_ascii("  foo\\   ", true), "foo\\ ");
    }

    #[test]
    fn test_no_exclude_escape_trims() {
        assert_eq!(trim_ascii("  foo\\   ", false), "foo\\");
    }

    #[test]
    fn test_nothing_to_trim() {
        assert_eq!(trim_ascii("foo", false), "foo");
    }
}
