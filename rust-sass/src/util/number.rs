// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/util/number.dart (fuzzy math; trig/unit wrappers live in value/number_math.rs — see the precision note on PRECISION)
// go-source: go/util/number.go (+ go/util/number_write.go for the serialization section below)

//! Fuzzy float comparison and CSS number serialization.
//!
//! Ports Dart's `util/number.dart`: floats compare equal when they agree past
//! the 10 significant digits Sass emits. All thresholds derive from
//! [`PRECISION`]; the serializer half of this file (`write_number_to*`,
//! `remove_exponent`, `write_rounded_to`) ports `_writeNumberToString` from
//! `visitor/serialize.dart` (see the second `dart-source:` line below).

use crate::common::core_errors::RangeError;
use std::fmt::Write;

/// The number of distinct digits emitted when converting a number to CSS.
///
/// Declared on Dart's `SassNumber` (`value/number.dart:148`) but consumed
/// here: fuzzy equality rounds to the 11th decimal digit. This resolves the
/// Wave-1 V2 deferral — document it on the owner, not the declaration site.
/// Matches Dart: `SassNumber.precision`.
pub const PRECISION: i32 = 10;

// `10^(-precision - 1)`: the minimum distance `a - b > epsilon` implies `a`
// isn't fuzzy-equal to `b` (the converse need not hold — see Dart's doc on
// `_epsilon`). Matches Dart: `_epsilon`.
fn epsilon() -> f64 {
    10f64.powi(-PRECISION - 1)
}

// `1 / epsilon`, cached since `pow` may not constant-fold.
// Matches Dart: `_inverseEpsilon`.
fn inverse_epsilon() -> f64 {
    10f64.powi(PRECISION + 1)
}

/// Returns whether `a` and `b` are equal up to the 11th decimal digit.
///
/// Matches Dart: `fuzzyEquals`.
pub fn fuzzy_equals(a: f64, b: f64) -> bool {
    if a == b {
        return true;
    }
    let diff = (a - b).abs();
    diff <= epsilon() && (a * inverse_epsilon()).round() == (b * inverse_epsilon()).round()
}

/// Like [`fuzzy_equals`], but `m1`/`m2` flag missing sides: two missing
/// values are equal, one missing never equals a present value.
///
/// (Dart takes nullable numbers; Rust threads presence as separate bools.)
/// Matches Dart: `fuzzyEqualsNullable`.
pub fn fuzzy_equals_nullable(v1: f64, v2: f64, m1: bool, m2: bool) -> bool {
    match (m1, m2) {
        (true, true) => true,
        (true, false) | (false, true) => false,
        (false, false) => fuzzy_equals(v1, v2),
    }
}

/// Returns a hash code for `n` consistent with [`fuzzy_equals`].
///
/// Non-finite inputs hash to `0` here (Dart returns their own hash code —
/// diverged deliberately since Rust floats have no matching `hashCode`).
/// Matches Dart: `fuzzyHashCode`.
pub fn fuzzy_hash_code(n: f64) -> i32 {
    if !n.is_infinite() && !n.is_nan() {
        (n * inverse_epsilon()).round() as i32
    } else {
        0
    }
}

/// Returns whether `a` is less than `b`, and not [`fuzzy_equals`].
pub fn fuzzy_less_than(a: f64, b: f64) -> bool {
    a < b && !fuzzy_equals(a, b)
}

/// Returns whether `a` is less than `b`, or [`fuzzy_equals`].
pub fn fuzzy_less_than_or_equals(a: f64, b: f64) -> bool {
    a < b || fuzzy_equals(a, b)
}

/// Returns whether `a` is greater than `b`, and not [`fuzzy_equals`].
pub fn fuzzy_greater_than(a: f64, b: f64) -> bool {
    a > b && !fuzzy_equals(a, b)
}

/// Returns whether `a` is greater than `b`, or [`fuzzy_equals`].
pub fn fuzzy_greater_than_or_equals(a: f64, b: f64) -> bool {
    a > b || fuzzy_equals(a, b)
}

/// Returns whether `v` is within `min` and `max` inclusive, using fuzzy
/// equality.
pub fn fuzzy_in_range(v: f64, min: f64, max: f64) -> bool {
    fuzzy_greater_than_or_equals(v, min) && fuzzy_less_than_or_equals(v, max)
}

/// Returns whether `n` is [`fuzzy_equals`] to an integer.
pub fn fuzzy_is_int(n: f64) -> bool {
    if n.is_infinite() || n.is_nan() {
        return false;
    }
    fuzzy_equals(n, n.round())
}

/// If `n` is an integer per [`fuzzy_is_int`], returns it as an `i64`.
///
pub fn fuzzy_as_int(n: f64) -> Option<i64> {
    if n.is_infinite() || n.is_nan() {
        return None;
    }
    let r = n.round();
    if fuzzy_equals(n, r) {
        // Matches Dart: fuzzyAsInt returns number.round() (a Dart int, which
        // is 64-bit); values outside int64 are unrepresentable and yield
        // None. Note `r as i64` saturates, so range-check first. The low side
        // is `<` (exactly i64::MIN is representable), the high side is `>=`
        // (no f64 holds exactly i64::MAX, so anything reaching it is out of
        // range).
        if r >= i64::MAX as f64 || r < i64::MIN as f64 {
            return None;
        }
        return Some(r as i64);
    }
    None
}

/// If `n` is close enough to an integer, returns it as an `i64`.
///
/// Normally "close enough" includes fuzzy matching, but outside inspect mode
/// "close enough" requires literally being an integer (the general decimal
/// path below still rounds fuzzy values); in inspect mode full precision is
/// shown so fuzzy matching applies.
///
/// Matches Dart: `_SerializeVisitor._asInt` (serialize.dart).
pub fn as_int_for_serialize(n: f64, inspect: bool) -> Option<i64> {
    if inspect {
        return fuzzy_as_int(n);
    }
    if n.is_infinite() || n.is_nan() {
        return None;
    }
    let r = n.round();
    if r != n {
        return None;
    }
    if r >= i64::MAX as f64 || r < i64::MIN as f64 {
        return None;
    }
    Some(r as i64)
}

/// Returns `n` if it is within `min` and `max`, or `None` if it is not.
///
/// Values [`fuzzy_equals`] to an endpoint are clamped to that endpoint.
/// Matches Dart: `fuzzyCheckRange`.
pub fn fuzzy_check_range(n: f64, min: f64, max: f64) -> Option<f64> {
    if fuzzy_equals(n, min) {
        return Some(min);
    }
    if fuzzy_equals(n, max) {
        return Some(max);
    }
    if n > min && n < max {
        return Some(n);
    }
    None
}

/// Like [`fuzzy_check_range`], but returns a [`RangeError`] naming `name`
/// instead of `None` when out of range.
///
/// Matches Dart: `fuzzyAssertRange`.
pub fn fuzzy_assert_range(
    number: f64,
    min: i32,
    max: i32,
    name: Option<&str>,
) -> Result<f64, RangeError> {
    if let Some(result) = fuzzy_check_range(number, min as f64, max as f64) {
        return Ok(result);
    }
    let name_str = name.unwrap_or("");
    Err(RangeError {
        name: name_str.to_string(),
        message: format!("must be between {min} and {max}: {number}"),
    })
}

/// Rounds `n` to the nearest integer, rounding values [`fuzzy_equals`] to
/// `X.5` up (down for negatives — see the sign split below).
///
/// Matches Dart: `fuzzyRound`.
pub fn fuzzy_round(n: f64) -> Result<i64, String> {
    if n < i64::MIN as f64 || n > i64::MAX as f64 {
        return Err(format!("cannot round {n} to i64: out of range"));
    }
    if n > 0.0 {
        if fuzzy_less_than(n - n.floor(), 0.5) {
            return Ok(n.floor() as i64);
        }
        return Ok(n.ceil() as i64);
    }
    let m = n - n.floor();
    if fuzzy_less_than_or_equals(m, 0.5) {
        return Ok(n.floor() as i64);
    }
    Ok(n.ceil() as i64)
}

/// Returns `a` modulo `b` under Sass's floored-division semantics (inherited
/// from Ruby), which differ from both Rust's `%` and Dart's `%` for negative
/// operands.
///
/// See <https://en.wikipedia.org/wiki/Modulo_operation#Variants_of_the_definition>.
/// Matches Dart: `moduloLikeSass`.
pub fn modulo_like_sass(a: f64, b: f64) -> f64 {
    if a.is_infinite() {
        return f64::NAN;
    }
    if b.is_infinite() {
        if sign_including_zero(a) == b.signum() {
            return a;
        }
        return f64::NAN;
    }
    if b > 0.0 {
        let mut result = a % b;
        if result < 0.0 {
            result += b;
        }
        if result == 0.0 {
            return 0.0;
        }
        return result;
    }
    if b == 0.0 {
        return f64::NAN;
    }
    let mut result = a % b;
    if result < 0.0 {
        result += -b;
    }
    if result == 0.0 {
        return 0.0;
    }
    result + b
}

/// Returns whether `f` is the special value negative zero.
///
/// Matches Dart: `DoubleWithSignedZero.isNegativeZero` (util/number.dart),
/// which compares with `crossPlatformIdentical` (util/cross_platform.dart —
/// `identical` on the VM, `Object.is` on JS). Rust targets one platform, so
/// a bit comparison suffices for the same observable behavior.
pub fn is_negative_zero(f: f64) -> bool {
    f.to_bits() == (-0.0f64).to_bits()
}

/// Maps `NaN` and negative zero to `0.0`, passing everything else through.
///
/// Matches Dart: `SassColor._normalizeLinear` (value/color.dart).
pub fn normalize_linear(v: f64) -> f64 {
    if v == 0.0 || v.is_nan() {
        0.0
    } else {
        v
    }
}

/// Returns the sign of `f`'s value, distinguishing `-0.0` (`-1.0`) from
/// `+0.0` (`1.0`).
///
/// Matches Dart: `DoubleWithSignedZero.signIncludingZero`.
pub fn sign_including_zero(f: f64) -> f64 {
    if is_negative_zero(f) {
        return -1.0;
    }
    if f == 0.0 {
        return 1.0;
    }
    f.signum()
}

/// Returns `v` clamped between `lower` and `upper`, with `NaN` preferring the
/// lower bound (unlike Rust's `clamp`, which prefers the upper).
///
/// Like Dart's `num.clamp`, `-0.0` clamps to `+0.0` (#2840).
///
/// Matches Dart: `clampLikeCss`.
pub fn clamp_like_css(v: f64, lower: f64, upper: f64) -> f64 {
    if v.is_nan() {
        return lower;
    }
    if v < lower {
        return lower;
    }
    if v > upper {
        return upper;
    }
    if v == 0.0 {
        return 0.0;
    }
    v
}

// --- Number serialization for CSS output ---

// dart-source: lib/src/visitor/serialize.dart
// go-source: go/util/number_write.go

/// Formats v as a CSS numeric string (non-inspect, non-compressed).
///
/// This is the standalone equivalent of Sass's `_writeNumberToString`.
pub fn write_number_to_string(v: f64) -> String {
    let mut buf = String::new();
    write_number_to(&mut buf, v, false, false).unwrap();
    buf
}

/// Writes v without exponent notation and with at most PRECISION digits
/// after the decimal point.
///
/// # Fast path
/// Values within epsilon of an integer are written as that integer.
///
/// # Fidelity
/// Uses `ryu` for shortest round-tripping representation, converts
/// exponent notation to plain decimal, then rounds to PRECISION
/// fractional digits if needed.
///
/// # Arguments
/// - `inspect`: if true, skip rounding and integer fast-path.
/// - `compressed`: if true, strip leading zero (e.g. `0.5` → `.5`).
pub fn write_number_to(
    buf: &mut impl Write,
    v: f64,
    inspect: bool,
    compressed: bool,
) -> std::fmt::Result {
    if v.is_nan() {
        return buf.write_str("NaN");
    }
    if v.is_infinite() {
        if v.is_sign_positive() {
            return buf.write_str("infinity");
        } else {
            return buf.write_str("-infinity");
        }
    }

    // Since dart-sass 1.104 (#2840), negative zero serializes as `-0`
    // (Dart `_writeNumber`). This precedes the integer fast-path: `-0.0`
    // is an exact integer, but must not print as `0`.
    if is_negative_zero(v) {
        return buf.write_str("-0");
    }

    // Integer fast-path (Dart `_asInt`): fuzzy matching in inspect mode,
    // exact integers only otherwise.
    if let Some(int_val) = as_int_for_serialize(v, inspect) {
        return write!(buf, "{}", int_val);
    }

    // Dart `num.ceil()`/`floor()`/`round()` return 64-bit ints saturating at
    // ±2⁶³∓1; no f64 holds those extremes exactly (`i64::MAX as f64` is 2⁶³),
    // so the math builtins return the extreme f64 as a sentinel. Print the
    // exact digit string Dart's `integer.toString()` produces.
    if v == i64::MAX as f64 {
        return buf.write_str("9223372036854775807");
    }
    if v == i64::MIN as f64 {
        return buf.write_str("-9223372036854775808");
    }

    let mut ryu_buf = ryu::Buffer::new();
    let formatted = ryu_buf.format(v);
    let plain = remove_exponent(formatted);

    if inspect {
        return buf.write_str(&plain);
    }

    let rounded = if plain.len() > PRECISION as usize + 2 && !plain.ends_with(".0") {
        write_rounded_to(&plain, PRECISION as usize)
    } else {
        plain
    };

    let stripped = if rounded.ends_with(".0") {
        rounded[..rounded.len() - 2].to_string()
    } else {
        rounded
    };

    // Matches Go's writeRoundedTo -0 guard at digitisIdx==2
    if stripped == "-0" || stripped == "-0." {
        return buf.write_str("0");
    }

    if compressed {
        return buf.write_str(&strip_leading_zero(&stripped));
    }
    buf.write_str(&stripped)
}

/// Converts exponent notation to plain decimal (port of
/// `_SerializeVisitor._removeExponent`).
fn remove_exponent(s: &str) -> String {
    let lower = s.to_lowercase();
    let e_pos = match lower.find('e') {
        Some(p) => p,
        None => return s.to_string(),
    };

    let mantissa = &s[..e_pos];
    let exp_str = &s[e_pos + 1..];
    let exponent: i32 = exp_str.parse().unwrap_or(0);

    let (negative, rest) = if let Some(stripped) = mantissa.strip_prefix('-') {
        (true, stripped)
    } else {
        (false, mantissa)
    };

    let (int_part, frac_part) = match rest.find('.') {
        Some(dot) => (&rest[..dot], &rest[dot + 1..]),
        None => (rest, ""),
    };

    let mut digits = String::with_capacity(int_part.len() + frac_part.len());
    digits.push_str(int_part);
    digits.push_str(frac_part);

    let mut result = String::new();
    if negative {
        result.push('-');
    }

    if exponent > 0 {
        let exp = exponent as usize;
        if exp >= frac_part.len() {
            result.push_str(&digits);
            let zeros_needed = exp - frac_part.len();
            for _ in 0..zeros_needed {
                result.push('0');
            }
        } else {
            result.push_str(&digits[..int_part.len() + exp]);
            let remaining = &digits[int_part.len() + exp..];
            if !remaining.is_empty() && !remaining.chars().all(|c| c == '0') {
                result.push('.');
                result.push_str(remaining);
            }
        }
    } else if exponent < 0 {
        let exp = (-exponent) as usize;
        result.push_str("0.");
        for _ in 1..exp {
            result.push('0');
        }
        result.push_str(&digits);
        while result.ends_with('0') && result.contains('.') {
            let last = result.pop().unwrap();
            if last == '.' {
                result.push('.');
                break;
            }
        }
    } else {
        result.push_str(int_part);
        if !frac_part.is_empty() && !frac_part.chars().all(|c| c == '0') {
            result.push('.');
            result.push_str(frac_part);
        }
    }

    if result == "-0" {
        return "0".to_string();
    }
    result
}

/// Rounds a plain-decimal string to `precision` fractional digits
/// with ripple-carry propagation.
fn write_rounded_to(s: &str, precision: usize) -> String {
    if let Some(stripped) = s.strip_suffix(".0") {
        return stripped.to_string();
    }

    let negative = s.starts_with('-');
    let mut text_idx = if negative { 1 } else { 0 };

    // Matches Go: digits array with leading zero at index 0
    let mut digits: Vec<u8> = Vec::with_capacity(s.len() + 1);
    digits.push(0);

    // Read integer digits in normal order (matching Go, not reversed)
    loop {
        if text_idx >= s.len() {
            return s.to_string();
        }
        let ch = s.as_bytes()[text_idx];
        text_idx += 1;
        if ch == b'.' {
            break;
        }
        digits.push(ch - b'0');
    }

    let first_frac = digits.len();

    // If we don't have more than precision fractional digits, return as-is
    if text_idx + precision >= s.len() {
        return s.to_string();
    }

    // Read precision fractional digits into the array
    let index_after_precision = text_idx + precision;
    while text_idx < index_after_precision {
        digits.push(s.as_bytes()[text_idx] - b'0');
        text_idx += 1;
    }

    // Rounding trigger: the (precision+1)th fractional digit
    if s.as_bytes()[text_idx] - b'0' >= 5 {
        // Matches Go's carry loop: propagate backward through all digits
        let mut digits_idx = digits.len();
        loop {
            digits[digits_idx - 1] += 1;
            if digits[digits_idx - 1] != 10 {
                break;
            }
            digits_idx -= 1;
        }
        digits.truncate(digits_idx);
    } else {
        digits.truncate(first_frac + precision);
    }

    // Zero-pad between current position and first_frac (matching Go)
    while digits.len() < first_frac {
        digits.push(0);
    }
    // Trim trailing zeros in fractional part (matching Go)
    while digits.len() > first_frac && *digits.last().unwrap() == 0 {
        digits.pop();
    }

    // Write output: forward iteration matching Go
    let mut result = String::new();
    if negative {
        result.push('-');
    }

    let written_idx = if digits[0] == 0 { 1 } else { 0 };
    for &d in digits.iter().take(first_frac).skip(written_idx) {
        result.push((d + b'0') as char);
    }

    if digits.len() > first_frac {
        result.push('.');
        for &d in digits.iter().skip(first_frac) {
            result.push((d + b'0') as char);
        }
    }
    result
}

fn strip_leading_zero(s: &str) -> String {
    // Matches Dart's short path: `if (_isCompressed && text.codeUnitAt(0)
    // == $0) text = text.substring(1)` — strips one leading `0`, so `0.5`
    // becomes `.5`. (Negative values keep their sign: the check is on the
    // first char, which is `-` for `-0.5`. The long rounded path handles its
    // own compressed leading-zero omission separately.)
    if let Some(stripped) = s.strip_prefix('0') {
        return stripped.to_string();
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_equals() {
        assert!(fuzzy_equals(1.0, 1.0));
        assert!(fuzzy_equals(1.0, 1.0 + 1e-12));
        assert!(!fuzzy_equals(1.0, 1.0 + 1e-10));
    }

    #[test]
    fn test_fuzzy_equals_nullable() {
        assert!(fuzzy_equals_nullable(1.0, 1.0, false, false));
        assert!(fuzzy_equals_nullable(0.0, 0.0, true, true));
        assert!(!fuzzy_equals_nullable(1.0, 1.0, true, false));
    }

    #[test]
    fn test_fuzzy_hash_code() {
        let h1 = fuzzy_hash_code(1.0);
        let h2 = fuzzy_hash_code(1.0 + 1e-12);
        assert_eq!(h1, h2);
        assert_eq!(fuzzy_hash_code(f64::INFINITY), 0);
    }

    #[test]
    fn test_fuzzy_less_than() {
        assert!(fuzzy_less_than(1.0, 2.0));
        assert!(!fuzzy_less_than(2.0, 1.0));
        assert!(!fuzzy_less_than(1.0, 1.0 + 1e-12));
    }

    #[test]
    fn test_fuzzy_greater_than() {
        assert!(fuzzy_greater_than(2.0, 1.0));
        assert!(!fuzzy_greater_than(1.0, 2.0));
    }

    #[test]
    fn test_fuzzy_in_range() {
        assert!(fuzzy_in_range(5.0, 0.0, 10.0));
        assert!(fuzzy_in_range(0.0, 0.0, 10.0));
        assert!(!fuzzy_in_range(11.0, 0.0, 10.0));
    }

    #[test]
    fn test_fuzzy_is_int() {
        assert!(fuzzy_is_int(5.0));
        assert!(fuzzy_is_int(5.0 + 1e-12));
        assert!(!fuzzy_is_int(5.5));
        assert!(!fuzzy_is_int(f64::INFINITY));
    }

    #[test]
    fn test_fuzzy_as_int() {
        assert_eq!(fuzzy_as_int(5.0), Some(5));
        assert_eq!(fuzzy_as_int(5.0 + 1e-12), Some(5));
        assert_eq!(fuzzy_as_int(5.5), None);
        assert_eq!(fuzzy_as_int(f64::INFINITY), None);
    }

    #[test]
    fn test_fuzzy_as_int_min() {
        // Exactly i64::MIN is representable (low side is `<`, not `<=`).
        // Note: -9223372036854775808.0f64 == i64::MIN as f64 exactly, and
        // round() is identity there.
        assert_eq!(fuzzy_as_int(-9223372036854775808.0), Some(i64::MIN));
        assert_eq!(fuzzy_as_int(9223372036854775807.0), None);
    }

    #[test]
    fn test_fuzzy_round() {
        assert_eq!(fuzzy_round(1.0).unwrap(), 1);
        assert_eq!(fuzzy_round(1.4).unwrap(), 1);
        assert_eq!(fuzzy_round(1.6).unwrap(), 2);
        assert_eq!(fuzzy_round(-1.4).unwrap(), -1);
        assert_eq!(fuzzy_round(-1.6).unwrap(), -2);
    }

    #[test]
    fn test_fuzzy_round_0_5() {
        assert_eq!(fuzzy_round(0.5).unwrap(), 1);
        assert_eq!(fuzzy_round(1.5).unwrap(), 2);
    }

    #[test]
    fn test_modulo_like_sass() {
        assert_eq!(modulo_like_sass(7.0, 3.0), 1.0);
        assert_eq!(modulo_like_sass(-7.0, 3.0), 2.0);
        assert_eq!(modulo_like_sass(7.0, -3.0), -2.0);
        assert_eq!(modulo_like_sass(-7.0, -3.0), -1.0);
    }

    #[test]
    fn test_modulo_like_sass_zero() {
        assert!(modulo_like_sass(1.0, 0.0).is_nan());
    }

    #[test]
    fn test_sign_including_zero() {
        assert_eq!(sign_including_zero(1.0), 1.0);
        assert_eq!(sign_including_zero(-1.0), -1.0);
        assert_eq!(sign_including_zero(0.0), 1.0);
        assert_eq!(sign_including_zero(-0.0), -1.0);
    }

    #[test]
    fn test_as_int_for_serialize() {
        // Dart `_asInt` (#2800): exact integers in both modes, fuzzy
        // integers only in inspect mode.
        assert_eq!(as_int_for_serialize(2.0, false), Some(2));
        assert_eq!(as_int_for_serialize(2.0, true), Some(2));
        assert_eq!(as_int_for_serialize(2.000000000001, false), None);
        assert_eq!(as_int_for_serialize(2.000000000001, true), Some(2));
        assert_eq!(as_int_for_serialize(2.5, false), None);
        assert_eq!(as_int_for_serialize(2.5, true), None);
        assert_eq!(as_int_for_serialize(f64::NAN, false), None);
        assert_eq!(as_int_for_serialize(f64::INFINITY, true), None);
    }

    #[test]
    fn test_is_negative_zero() {
        // Dart `isNegativeZero` via `crossPlatformIdentical` (#2840).
        assert!(is_negative_zero(-0.0));
        assert!(!is_negative_zero(0.0));
        assert!(!is_negative_zero(1.0));
        assert!(!is_negative_zero(-1.0));
        assert!(!is_negative_zero(f64::NAN));
    }

    #[test]
    fn test_normalize_linear() {
        // Dart `SassColor._normalizeLinear` (#2840).
        assert_eq!(normalize_linear(f64::NAN).to_bits(), 0.0f64.to_bits());
        assert_eq!(normalize_linear(-0.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(normalize_linear(0.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(normalize_linear(1.5), 1.5);
        assert_eq!(normalize_linear(f64::INFINITY), f64::INFINITY);
    }

    #[test]
    fn test_write_number_negative_zero() {
        // Since dart-sass 1.104 (#2840) negative zero serializes as `-0`.
        let mut buf = String::new();
        write_number_to(&mut buf, -0.0, false, false).unwrap();
        assert_eq!(buf, "-0");
        buf.clear();
        write_number_to(&mut buf, -0.0, true, true).unwrap();
        assert_eq!(buf, "-0");
        buf.clear();
        write_number_to(&mut buf, 0.0, false, false).unwrap();
        assert_eq!(buf, "0");
    }

    #[test]
    fn test_clamp_like_css() {
        assert_eq!(clamp_like_css(5.0, 0.0, 10.0), 5.0);
        assert_eq!(clamp_like_css(-1.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp_like_css(15.0, 0.0, 10.0), 10.0);
        assert!(
            clamp_like_css(f64::NAN, 0.0, 10.0) == 0.0
                || clamp_like_css(f64::NAN, 0.0, 10.0).is_nan()
        );
        // Dart's `num.clamp` yields `+0.0` for `-0.0` (#2840).
        assert_eq!(clamp_like_css(-0.0, 0.0, 1.0).to_bits(), 0.0f64.to_bits());
    }

    #[test]
    fn test_write_number_to_string_basic() {
        let tests: Vec<(f64, &str)> = vec![
            (0.0, "0"),
            (1.0, "1"),
            (255.0, "255"),
            (0.5, "0.5"),
            (100.0, "100"),
            (-42.0, "-42"),
            (-0.5, "-0.5"),
            (0.1, "0.1"),
            (1.5, "1.5"),
            (0.123456789, "0.123456789"),
            (0.1234567890, "0.123456789"),
            (0.12345678919, "0.1234567892"),
            (0.0, "0"),
        ];
        for (v, want) in tests {
            let got = write_number_to_string(v);
            assert_eq!(got, want, "write_number_to_string({v:?}) returned {got:?}");
        }
    }

    #[test]
    fn test_write_number_to_string_integer_fuzzy() {
        let vals = [
            0.000000000001,
            -0.000000000001,
            255.000000000001,
            1.000000000001,
        ];
        for v in vals {
            let got = write_number_to_string(v);
            let int_v = v.round() as i64;
            let want = write_number_to_string(int_v as f64);
            assert_eq!(
                got, want,
                "write_number_to_string({v:?}) should match integer {int_v}"
            );
        }
    }

    #[test]
    fn test_write_number_to_string_exponent() {
        let tests: Vec<(f64, &str)> = vec![
            (0.000001, "0.000001"),
            (1e-7, "0.0000001"),
            (1000000.0, "1000000"),
            (0.123456789012345, "0.123456789"),
            (0.12345678906789, "0.1234567891"),
        ];
        for (v, want) in tests {
            let got = write_number_to_string(v);
            assert_eq!(got, want, "write_number_to_string({v:?}) returned {got:?}");
        }
    }

    #[test]
    fn test_write_number_to_inspect() {
        // In inspect mode, exact integers use fast-path, fuzzy ones do not.
        let mut buf = String::new();
        write_number_to(&mut buf, 42.0, true, false).unwrap();
        assert_eq!(buf, "42");

        let mut buf = String::new();
        write_number_to(&mut buf, 0.5, true, false).unwrap();
        assert_eq!(buf, "0.5");

        // Fuzzy integer in inspect mode writes full precision (e.g. "42.000000000001")
        let mut buf = String::new();
        write_number_to(&mut buf, 42.000000000001, true, false).unwrap();
        assert!(buf.starts_with("42"));
    }

    #[test]
    fn test_write_number_to_compressed() {
        let mut buf = String::new();
        write_number_to(&mut buf, 0.5, false, true).unwrap();
        assert_eq!(buf, ".5");
    }

    #[test]
    fn test_compressed_keeps_negative_zero_prefix() {
        // Matches Dart's short path (`text.codeUnitAt(0) == $0`): only a
        // leading `0` is stripped, so `-0.5` stays `-0.5` (not `-.5`).
        let mut buf = String::new();
        write_number_to(&mut buf, -0.5, false, true).unwrap();
        assert_eq!(buf, "-0.5");
        buf.clear();
        write_number_to(&mut buf, 0.05, false, true).unwrap();
        assert_eq!(buf, ".05");
    }

    #[test]
    fn test_remove_exponent() {
        assert_eq!(remove_exponent("1.23e5"), "123000");
        assert_eq!(remove_exponent("1.23e2"), "123");
        assert_eq!(remove_exponent("4.56e-3"), "0.00456");
        assert_eq!(remove_exponent("-1.23e2"), "-123");
        assert_eq!(remove_exponent("3.14e1"), "31.4");
    }

    #[test]
    fn test_write_number_nan_inf() {
        let mut buf = String::new();
        write_number_to(&mut buf, f64::NAN, false, false).unwrap();
        assert_eq!(buf, "NaN");
        buf.clear();
        write_number_to(&mut buf, f64::INFINITY, false, false).unwrap();
        assert_eq!(buf, "infinity");
        buf.clear();
        write_number_to(&mut buf, f64::NEG_INFINITY, false, false).unwrap();
        assert_eq!(buf, "-infinity");
    }
}
