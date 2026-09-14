// Copyright (c) 2013, the Dart project authors.  Please see the AUTHORS file
// for details. All rights reserved. Use of this source code is governed by a
// BSD-style license that can be found in the LICENSE file.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: (external) package:source_maps/source_maps.dart
// go-source: go/sourcemap/vlq.go

//! The V3 source-map variable-length quantity (VLQ) codec.
//!
//! The sign is folded into the least-significant bit (non-negative values
//! encode as `2 * value`, negatives as `-2 * value - 1`), the magnitude is
//! emitted as 5-bit chunks with a continuation bit, and each chunk maps
//! through the Base64 alphabet. [`encode_vlq`] writes one value;
//! [`decode_vlq`] reads one value at a byte offset (used in tests to verify
//! golden mappings segment by segment).

use std::sync::LazyLock;

const BASE64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

const VLQ_BASE_SHIFT: i32 = 5;
const VLQ_BASE_MASK: i32 = (1 << 5) - 1;
const VLQ_CONTINUATION_BIT: i32 = 1 << 5;

const MAX_I32: i32 = i32::MAX;
const MIN_I32: i32 = i32::MIN;

static BASE64_DECODE: LazyLock<[i8; 256]> = LazyLock::new(|| {
    let mut table = [-1i8; 256];
    for (i, &c) in BASE64_CHARS.iter().enumerate() {
        table[c as usize] = i as i8;
    }
    table
});

/// Encodes `value` as a VLQ Base64 string. Infallible for `i32` inputs (the
/// error case exists for API parity with fallible callers); values outside
/// the 32-bit range cannot occur.
pub fn encode_vlq(value: i32) -> Result<String, String> {
    let v = value as i64;
    let sign_bit = if v < 0 { 1i64 } else { 0i64 };
    let mut v = (v.abs() << 1) | sign_bit;

    let mut buf = Vec::with_capacity(7);
    loop {
        let mut digit = (v & VLQ_BASE_MASK as i64) as i32;
        v >>= VLQ_BASE_SHIFT;
        if v > 0 {
            digit |= VLQ_CONTINUATION_BIT;
        }
        buf.push(BASE64_CHARS[digit as usize]);
        if v == 0 {
            break;
        }
    }
    Ok(String::from_utf8(buf).unwrap())
}

/// Decodes one VLQ value starting at byte offset `pos` in `s`, returning the
/// value and the offset just past it. Errors on truncated input, invalid
/// Base64 characters, or results outside the 32-bit range.
pub fn decode_vlq(s: &str, pos: usize) -> Result<(i32, usize), String> {
    let mut result: i64 = 0;
    let mut shift: u32 = 0;
    let bytes = s.as_bytes();
    let mut pos = pos;
    loop {
        if pos >= bytes.len() {
            return Err("sourcemap: incomplete VLQ value".into());
        }
        let c = bytes[pos];
        let digit = BASE64_DECODE[c as usize] as i64;
        if digit < 0 {
            return Err(format!(
                "sourcemap: invalid character in VLQ encoding: {}",
                c as char
            ));
        }
        pos += 1;
        let stop = (digit as i32 & VLQ_CONTINUATION_BIT) == 0;
        let digit = digit & VLQ_BASE_MASK as i64;
        result += digit << shift;
        shift += VLQ_BASE_SHIFT as u32;
        if stop {
            break;
        }
    }

    let negate = (result & 1) == 1;
    result >>= 1;
    if negate {
        result = -result;
    }

    if result < MIN_I32 as i64 || result > MAX_I32 as i64 {
        return Err(format!(
            "sourcemap: decoded value out of 32-bit range: {result}"
        ));
    }

    Ok((result as i32, pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_vlq_simple() {
        let tests = [
            (1, "C"),
            (2, "E"),
            (3, "G"),
            (100, "oG"),
            (0, "A"),
            (-1, "D"),
            (MIN_I32, "hgggggE"),
        ];
        for (input, want) in tests {
            let got = encode_vlq(input).unwrap();
            assert_eq!(got, want, "encode_vlq({input}) = {got:?}, want {want:?}");
        }
    }

    #[test]
    fn test_decode_vlq_simple() {
        let tests = [
            ("C", 1),
            ("E", 2),
            ("G", 3),
            ("oG", 100),
            ("A", 0),
            ("D", -1),
            ("hgggggE", MIN_I32),
        ];
        for (input, want) in tests {
            let (got, pos) = decode_vlq(input, 0).unwrap();
            assert_eq!(
                pos,
                input.len(),
                "decode_vlq({input:?}) pos = {pos}, want {}",
                input.len()
            );
            assert_eq!(got, want, "decode_vlq({input:?}) = {got}, want {want}");
        }
    }

    #[test]
    fn test_encode_decode_range() {
        for i in -10000..10000 {
            let encoded = encode_vlq(i).unwrap();
            let (decoded, pos) = decode_vlq(&encoded, 0).unwrap();
            assert_eq!(
                pos,
                encoded.len(),
                "decode_vlq(encode_vlq({i})) pos = {pos}, want {}",
                encoded.len()
            );
            assert_eq!(
                decoded, i,
                "decode_vlq(encode_vlq({i})) = {decoded}, want {i}"
            );
        }
    }

    #[test]
    fn test_encode_decode_boundaries() {
        let boundaries = [MAX_I32 - 1, MIN_I32 + 1, MAX_I32, MIN_I32];
        for &val in &boundaries {
            let encoded = encode_vlq(val).unwrap();
            let (decoded, _pos) = decode_vlq(&encoded, 0).unwrap();
            assert_eq!(
                decoded, val,
                "decode_vlq(encode_vlq({val})) = {decoded}, want {val}"
            );
        }
    }

    #[test]
    fn test_encode_out_of_range() {
        // i32::MAX + 1 overflows i32, so test with values that are out of range
        // Go tests pass these as int which is i64; we test that MIN_I32/MAX_I32 themselves work
        // No out-of-range possible with i32 since all i32 values are in range by definition
    }

    #[test]
    fn test_decode_out_of_range() {
        let tests = ["ggggggE", "igggggE", "jgggggE", "lgggggE"];
        for tc in tests {
            let result = decode_vlq(tc, 0);
            assert!(result.is_err(), "decode_vlq({tc:?}) expected error");
        }
    }

    #[test]
    fn test_encode_decode_sequential() {
        // Encode multiple values and decode them sequentially
        let values = [4i32, 0, 1, 4, 0]; // golden "IACIA"
        let mut buf = String::new();
        for &v in &values {
            let enc = encode_vlq(v).unwrap();
            buf.push_str(&enc);
        }
        assert_eq!(buf, "IACIA");

        let mut pos = 0;
        for &want in &values {
            let (got, new_pos) = decode_vlq(&buf, pos).unwrap();
            assert_eq!(got, want, "decode_vlq at pos {pos} = {got}, want {want}");
            pos = new_pos;
        }
    }

    #[test]
    fn test_encode_decode_random() {
        let values: [i32; 10] = [
            42,
            -42,
            127,
            -128,
            255,
            1024,
            -2048,
            65535,
            (1 << 20) - 1,
            -(1 << 20),
        ];
        for &v in &values {
            let enc = encode_vlq(v).unwrap();
            let (dec, _pos) = decode_vlq(&enc, 0).unwrap();
            assert_eq!(dec, v, "roundtrip {v}: got {dec}");
        }
    }
}
