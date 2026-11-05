// dart-source: sdk/lib/core/uri.dart (UriData) and sdk/lib/convert/base64.dart
// (Dart 3.13.5).

//! `data:` URI parsing and normalization (Dart `UriData._parse`).

use super::{normalize, utf16_string, DartUriError, QUERY_CHAR_MASK};

const COMMA: u16 = 0x2c;
const SLASH: u16 = 0x2f;
const SEMICOLON: u16 = 0x3b;
const EQUALS: u16 = 0x3d;

/// Parses and normalizes the part of a `data:` URI after the `data:` scheme.
///
/// Mirrors `UriData._parse(text, start, ...)` when `start` is 0 (the scheme
/// has already been stripped) or 5 (the scheme is lowercase `data:`).
pub(crate) fn parse_uri_data(text: &str) -> Result<String, DartUriError> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let len = units.len();
    let mut separators: Vec<isize> = vec![-1];
    let mut slash_index: isize = -1;
    let mut i = 0usize;
    let mut ch = 0u16;
    while i < len {
        ch = units[i];
        if ch == COMMA || ch == SEMICOLON {
            break;
        }
        if ch == SLASH {
            if slash_index < 0 {
                slash_index = i as isize;
                i += 1;
                continue;
            }
            return Err(DartUriError::new("Invalid MIME type"));
        }
        i += 1;
    }
    if slash_index < 0 && i > 0 {
        return Err(DartUriError::new("Invalid MIME type"));
    }
    while ch != COMMA {
        separators.push(i as isize);
        i += 1;
        let mut equals_index: isize = -1;
        while i < len {
            ch = units[i];
            if ch == EQUALS {
                if equals_index < 0 {
                    equals_index = i as isize;
                }
            } else if ch == SEMICOLON || ch == COMMA {
                break;
            }
            i += 1;
        }
        if equals_index >= 0 {
            separators.push(equals_index);
        } else {
            let last_separator = *separators.last().unwrap();
            if ch != COMMA
                || i as isize != last_separator + 7
                || !super::starts_with_at(&units, (last_separator + 1) as usize, "base64")
            {
                return Err(DartUriError::new("Expecting '='"));
            }
            break;
        }
    }
    separators.push(i as isize);
    let is_base64 = separators.len() % 2 == 1;
    let prefix = utf16_string(&units, 0, i + 1);
    if is_base64 {
        let tail = normalize_base64(&units, i + 1, len)?;
        Ok(format!("{prefix}{tail}"))
    } else {
        match normalize(&units, i + 1, len, QUERY_CHAR_MASK, true, false)? {
            Some(data) => Ok(format!("{prefix}{data}")),
            None => Ok(text.to_string()),
        }
    }
}

fn base64_value(ch: u16) -> Option<u8> {
    match ch {
        0x41..=0x5a => Some((ch - 0x41) as u8),
        0x61..=0x7a => Some((ch - 0x61 + 26) as u8),
        0x30..=0x39 => Some((ch - 0x30 + 52) as u8),
        0x2b => Some(62),
        0x2f => Some(63),
        _ => None,
    }
}

fn normalize_base64(units: &[u16], start: usize, end: usize) -> Result<String, DartUriError> {
    let mut out = String::new();
    let mut padding = 0usize;
    let mut saw_padding = false;
    let mut i = start;
    while i < end {
        let mut ch = units[i];
        i += 1;
        if ch == 0x25 {
            if i + 2 > end {
                return Err(DartUriError::new("Invalid base64 data"));
            }
            let hi = super::hex_digit_value(units[i]);
            let lo = super::hex_digit_value(units[i + 1]);
            if hi < 0 || lo < 0 {
                return Err(DartUriError::new("Invalid base64 data"));
            }
            i += 2;
            ch = (hi * 16 + lo) as u16;
        }
        if ch == 0x2d {
            ch = 0x2b;
        } else if ch == 0x5f {
            ch = 0x2f;
        }
        if base64_value(ch).is_some() {
            if saw_padding {
                return Err(DartUriError::new("Invalid base64 data"));
            }
            out.push(char::from_u32(ch as u32).unwrap_or('\u{FFFD}'));
        } else if ch == EQUALS {
            saw_padding = true;
            padding += 1;
            out.push('=');
        } else {
            return Err(DartUriError::new("Invalid base64 data"));
        }
    }
    if saw_padding {
        if !out.len().is_multiple_of(4) || padding > 2 {
            return Err(DartUriError::new("Invalid base64 data"));
        }
        if padding > 0 && !out.ends_with('=') {
            return Err(DartUriError::new("Invalid base64 data"));
        }
    } else {
        while !out.len().is_multiple_of(4) {
            out.push('=');
        }
    }
    Ok(out)
}
