// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/util/character.dart
// go-source: go/util/character.go

//! Character classification and case-folding helpers for the Sass grammar.
//!
//! Ports Dart's `CharacterExtension` / `NullableCharacterExtension` (which are
//! defined on code-unit `int`s so they can be used in pattern matches) as free
//! functions on `char`. Code points travel as `i32` in [`super::string`] and
//! as `char` here; the ASCII-only case logic is shared with
//! [`super::string`] via the `0x20` case bit.

use crate::common::core_errors::ArgumentError;

/// The highest code point allowed in CSS.
///
/// See <https://drafts.csswg.org/css-syntax-3/#maximum-allowed-code-point>.
/// Matches Dart: `maxAllowedCharacter`.
pub const MAX_ALLOWED_CHARACTER: i32 = 0x10FFFF;

// Matches Dart: `_asciiCaseBit`. Bitwise-ORing an uppercase ASCII letter with
// this yields its lowercase equivalent.
const ASCII_CASE_BIT: i32 = 0x20;

/// Returns whether `ch` is a letter or a digit.
pub fn is_alphanumeric(ch: char) -> bool {
    is_alphabetic(ch) || is_digit(ch)
}

/// Returns whether `ch` is an ASCII letter (`a-z`, `A-Z`).
pub fn is_alphabetic(ch: char) -> bool {
    ch.is_ascii_alphabetic()
}

/// Returns whether `ch` is an ASCII digit.
pub fn is_digit(ch: char) -> bool {
    ch.is_ascii_digit()
}

/// Returns whether `ch` is ASCII whitespace (space, tab, or newline).
pub fn is_whitespace(ch: char) -> bool {
    is_space_or_tab(ch) || is_newline(ch)
}

/// Returns whether `ch` is an ASCII newline (`\n`, `\r`, or form feed).
pub fn is_newline(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\x0C')
}

/// Returns whether `ch` is a space or a tab.
pub fn is_space_or_tab(ch: char) -> bool {
    matches!(ch, ' ' | '\t')
}

/// Returns whether `identifier` is module-private (starts with `-` or `_`).
///
/// Assumes `identifier` is a valid, non-empty Sass identifier.
/// Matches Dart: `isPrivate`.
pub fn is_private(identifier: &str) -> bool {
    let first = identifier.as_bytes()[0];
    first == b'-' || first == b'_'
}

/// Returns whether `ch` is a hexadecimal digit.
pub fn is_hex(ch: char) -> bool {
    is_digit(ch) || ch.is_ascii_hexdigit()
}

/// Returns whether `ch` is legal as the start of a Sass identifier
/// (`_`, a letter, or any non-ASCII character).
pub fn is_name_start(ch: char) -> bool {
    ch == '_' || is_alphabetic(ch) || (ch as u32) >= 0x0080
}

/// Returns whether `ch` is legal in the body of a Sass identifier
/// (a name-start, a digit, or `-`).
pub fn is_name(ch: char) -> bool {
    is_name_start(ch) || is_digit(ch) || ch == '-'
}

/// Returns the value of `ch` as a hex digit.
///
/// Assumes `ch` is a hex digit; returns `0` otherwise (Dart asserts).
/// Matches Dart: `asHex`.
pub fn as_hex(ch: char) -> i32 {
    match ch {
        '0'..='9' => ch as i32 - '0' as i32,
        'A'..='F' => 10 + ch as i32 - 'A' as i32,
        'a'..='f' => 10 + ch as i32 - 'a' as i32,
        _ => 0,
    }
}

/// Returns the value of `ch` as a decimal digit.
///
/// Assumes `ch` is a decimal digit. Matches Dart: `asDecimal`.
pub fn as_decimal(ch: char) -> i32 {
    ch as i32 - '0' as i32
}

/// Returns the decimal digit for `n`.
///
/// Assumes `n` is less than 10. Matches Dart: `decimalCharFor`.
pub fn decimal_char_for(n: i32) -> char {
    char::from_u32(('0' as i32 + n) as u32).unwrap_or('0')
}

/// Returns the hexadecimal digit for `n` (lowercase).
///
/// Assumes `n` is less than 16; panics otherwise (Dart asserts).
/// Matches Dart: `hexCharFor`.
pub fn hex_char_for(n: i32) -> char {
    if n >= 0x10 {
        panic!("number must be less than 16");
    }
    if n < 0xA {
        char::from_u32(('0' as i32 + n) as u32).unwrap_or('0')
    } else {
        char::from_u32(('a' as i32 - 0xA + n) as u32).unwrap_or('a')
    }
}

/// Returns the right-hand brace matching the left-hand `ch`
/// (`(` → `)`, `{` → `}`, `[` → `]`).
///
/// Errors for anything else. Matches Dart: `opposite`.
pub fn opposite(ch: char) -> Result<char, ArgumentError> {
    match ch {
        '(' => Ok(')'),
        '{' => Ok('}'),
        '[' => Ok(']'),
        _ => Err(ArgumentError {
            name: None,
            message: format!("{ch:?} isn't a brace-like character"),
        }),
    }
}

/// Returns `ch` converted to upper case if it is an ASCII lowercase letter.
///
/// Matches Dart: `toUpperCase`.
pub fn to_upper_case(ch: char) -> char {
    if ch.is_ascii_lowercase() {
        ((ch as i32) & !ASCII_CASE_BIT) as u8 as char
    } else {
        ch
    }
}

/// Returns `ch` converted to lower case if it is an ASCII uppercase letter.
///
/// Matches Dart: `toLowerCase`.
pub fn to_lower_case(ch: char) -> char {
    if ch.is_ascii_uppercase() {
        ((ch as i32) | ASCII_CASE_BIT) as u8 as char
    } else {
        ch
    }
}

/// Returns whether `a` and `b` are the same, modulo ASCII case.
///
/// If the XOR check fails the characters are definitely different; if it
/// succeeds *and* either is an ASCII letter, they are equivalent.
/// Matches Dart: `characterEqualsIgnoreCase`.
pub fn char_equals_ignore_case(a: char, b: char) -> bool {
    if a == b {
        return true;
    }
    let ax = a as i32;
    let bx = b as i32;
    if ax ^ bx != ASCII_CASE_BIT {
        return false;
    }
    let upper = ax & !ASCII_CASE_BIT;
    upper >= ('A' as i32) && upper <= ('Z' as i32)
}

/// Like [`char_equals_ignore_case`], but optimized for `letter` being a known
/// lowercase ASCII letter (Dart asserts this).
///
/// Matches Dart: `equalsLetterIgnoreCase`.
pub fn equals_letter_ignore_case(letter: char, actual: char) -> bool {
    ((actual as i32) | ASCII_CASE_BIT) == (letter as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_digit() {
        assert!(is_digit('0'));
        assert!(is_digit('9'));
        assert!(!is_digit('a'));
        assert!(!is_digit('-'));
    }

    #[test]
    fn test_is_hex() {
        assert!(is_hex('0'));
        assert!(is_hex('a'));
        assert!(is_hex('F'));
        assert!(!is_hex('g'));
    }

    #[test]
    fn test_is_alphabetic() {
        assert!(is_alphabetic('a'));
        assert!(is_alphabetic('Z'));
        assert!(!is_alphabetic('0'));
        assert!(!is_alphabetic('-'));
    }

    #[test]
    fn test_is_alphanumeric() {
        assert!(is_alphanumeric('a'));
        assert!(is_alphanumeric('0'));
        assert!(!is_alphanumeric('-'));
        assert!(!is_alphanumeric('_'));
    }

    #[test]
    fn test_is_name_start() {
        assert!(is_name_start('_'));
        assert!(is_name_start('a'));
        assert!(is_name_start('\u{0080}'));
        assert!(!is_name_start('0'));
        assert!(!is_name_start('-'));
    }

    #[test]
    fn test_is_name() {
        assert!(is_name('_'));
        assert!(is_name('a'));
        assert!(is_name('0'));
        assert!(is_name('-'));
        assert!(is_name('\u{0080}'));
        assert!(!is_name(' '));
        assert!(!is_name('.'));
    }

    #[test]
    fn test_is_whitespace() {
        assert!(is_whitespace(' '));
        assert!(is_whitespace('\t'));
        assert!(is_whitespace('\n'));
        assert!(!is_whitespace('a'));
    }

    #[test]
    fn test_as_hex() {
        assert_eq!(as_hex('0'), 0);
        assert_eq!(as_hex('9'), 9);
        assert_eq!(as_hex('A'), 10);
        assert_eq!(as_hex('F'), 15);
        assert_eq!(as_hex('a'), 10);
    }

    #[test]
    fn test_hex_char_for() {
        assert_eq!(hex_char_for(0), '0');
        assert_eq!(hex_char_for(10), 'a');
        assert_eq!(hex_char_for(15), 'f');
    }

    #[test]
    #[should_panic]
    fn test_hex_char_for_panics() {
        hex_char_for(16);
    }

    #[test]
    fn test_opposite() {
        assert_eq!(opposite('(').unwrap(), ')');
        assert_eq!(opposite('{').unwrap(), '}');
        assert_eq!(opposite('[').unwrap(), ']');
    }

    #[test]
    fn test_opposite_error() {
        assert!(opposite('x').is_err());
    }

    #[test]
    fn test_to_upper_case() {
        assert_eq!(to_upper_case('a'), 'A');
        assert_eq!(to_upper_case('z'), 'Z');
        assert_eq!(to_upper_case('A'), 'A');
        assert_eq!(to_upper_case('0'), '0');
    }

    #[test]
    fn test_to_lower_case() {
        assert_eq!(to_lower_case('A'), 'a');
        assert_eq!(to_lower_case('Z'), 'z');
        assert_eq!(to_lower_case('a'), 'a');
        assert_eq!(to_lower_case('0'), '0');
    }

    #[test]
    fn test_char_equals_ignore_case() {
        assert!(char_equals_ignore_case('a', 'A'));
        assert!(char_equals_ignore_case('Z', 'z'));
        assert!(!char_equals_ignore_case('a', 'b'));
    }

    #[test]
    fn test_is_private() {
        assert!(is_private("-foo"));
        assert!(is_private("_foo"));
        assert!(!is_private("foo"));
    }
}
