// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/util/character.dart (surrogate/private-use members of CharacterExtension + combineSurrogates; toCssIdentifier itself lives in lib/src/util/string.dart — see super::css_identifier)
// go-source: go/util/string.go

//! UTF-16 surrogate-pair helpers.
//!
//! Ports the surrogate/private-use members of Dart's `CharacterExtension`
//! (in `util/character.dart`) plus `combineSurrogates`. The `i32` code-point
//! typing matches Go's port; on valid UTF-8 these never trigger — they exist
//! for the CSS-identifier path ([`super::css_identifier`]), which must reject
//! lone surrogates, and for the string builtins' code-point indexing.

/// Returns whether `ch` is the beginning of a UTF-16 surrogate pair
/// (matches `0b110110XXXXXXXXXX`).
pub fn is_high_surrogate(ch: i32) -> bool {
    ch >> 10 == 0x36
}

/// Returns whether `ch` is the end of a UTF-16 surrogate pair
/// (matches `0b110111XXXXXXXXXX`).
pub fn is_low_surrogate(ch: i32) -> bool {
    ch >> 10 == 0x37
}

/// Returns whether `ch` is a Unicode private-use code point in the Basic
/// Multilingual Plane (`U+E000`–`U+F8FF`).
///
/// See <https://en.wikipedia.org/wiki/Private_Use_Areas>.
pub fn is_private_use_bmp(ch: i32) -> bool {
    (0xE000..=0xF8FF).contains(&ch)
}

/// Returns whether `ch` is the high surrogate for a code point in a Unicode
/// private-use supplementary plane (`0xDB80`–`0xDBFF`, i.e.
/// `0b110110111XXXXXXX`).
///
/// See <https://en.wikipedia.org/wiki/Private_Use_Areas>.
pub fn is_private_use_high_surrogate(ch: i32) -> bool {
    ch >> 7 == 0x1B7
}

/// Combines a UTF-16 high/low surrogate pair into a single code point.
///
/// The `0x3FF` mask keeps the ten payload bits of each surrogate.
/// See <https://en.wikipedia.org/wiki/UTF-16>.
pub fn combine_surrogates(high: i32, low: i32) -> i32 {
    0x10000 + ((high & 0x3FF) << 10) + (low & 0x3FF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_high_surrogate() {
        assert!(is_high_surrogate(0xD800));
        assert!(is_high_surrogate(0xDBFF));
        assert!(!is_high_surrogate(0xD7FF));
        assert!(!is_high_surrogate(0xDC00));
    }

    #[test]
    fn test_is_low_surrogate() {
        assert!(is_low_surrogate(0xDC00));
        assert!(is_low_surrogate(0xDFFF));
        assert!(!is_low_surrogate(0xDBFF));
        assert!(!is_low_surrogate(0xE000));
    }

    #[test]
    fn test_is_private_use_bmp() {
        assert!(is_private_use_bmp(0xE000));
        assert!(is_private_use_bmp(0xF8FF));
        assert!(!is_private_use_bmp(0xDFFF));
        assert!(!is_private_use_bmp(0xF900));
    }

    #[test]
    fn test_is_private_use_high_surrogate() {
        assert!(is_private_use_high_surrogate(0xDB80));
        assert!(is_private_use_high_surrogate(0xDBFF));
        assert!(!is_private_use_high_surrogate(0xD800));
    }

    #[test]
    fn test_combine_surrogates() {
        assert_eq!(combine_surrogates(0xD800, 0xDC00), 0x10000);
        assert_eq!(combine_surrogates(0xDBFF, 0xDFFF), 0x10FFFF);
    }
}
