// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/util/number.dart
// go-source: go/value/number_math.go

use crate::common::exception::{SassError, SassResult};
use crate::math::abs;
use crate::math::acos;
use crate::math::asin;
use crate::math::atan;
use crate::math::atan2;
use crate::math::cos;
use crate::math::ln;
use crate::math::pow;
use crate::math::sin;
use bumpalo::Bump;

use crate::math::sqrt;
use crate::math::tan;
use crate::value::assert_number;
use crate::value::SassNumber;
use crate::value::ValueKind;
use std::f64::consts::PI;

fn radians_to_degrees<'parse>(arena: &'parse Bump, rad: f64) -> SassNumber<'parse> {
    // Returns `rad` as a number with unit `deg`.
    // Matches Dart: _radiansToDegrees.
    // Dart `_radiansToDegrees`: `radians * (180 / math.pi)` — parenthesized
    // division first (1-ULP difference vs `rad * 180.0 / PI`, masked at
    // precision 10 but transcribed for fidelity).
    SassNumber::new(arena, rad * (180.0 / PI), Some("deg"))
}

/// Returns the square root of `number`.
///
/// `number` must be unitless; the result is unitless.
///
/// Matches Dart: sqrt / Go: SqrtNumber
pub fn sqrt_number<'parse>(
    arena: &'parse Bump,
    number: &SassNumber<'parse>,
) -> SassResult<SassNumber<'parse>> {
    number.assert_no_units(Some("number"))?;
    Ok(SassNumber::new(arena, sqrt(number.value), None))
}

/// Returns the sine of `number`.
///
/// `number` is coerced to radians; the result is unitless.
///
/// Matches Dart: sin / Go: SinNumber
pub fn sin_number<'parse>(
    arena: &'parse Bump,
    number: &SassNumber<'parse>,
) -> SassResult<SassNumber<'parse>> {
    let val = number.coerce_value_to_unit("rad", Some("number"))?;
    Ok(SassNumber::new(arena, sin(val), None))
}

/// Returns the cosine of `number`.
///
/// `number` is coerced to radians; the result is unitless.
///
/// Matches Dart: cos / Go: CosNumber
pub fn cos_number<'parse>(
    arena: &'parse Bump,
    number: &SassNumber<'parse>,
) -> SassResult<SassNumber<'parse>> {
    let val = number.coerce_value_to_unit("rad", Some("number"))?;
    Ok(SassNumber::new(arena, cos(val), None))
}

/// Returns the tangent of `number`.
///
/// `number` is coerced to radians; the result is unitless.
///
/// Matches Dart: tan / Go: TanNumber
pub fn tan_number<'parse>(
    arena: &'parse Bump,
    number: &SassNumber<'parse>,
) -> SassResult<SassNumber<'parse>> {
    let val = number.coerce_value_to_unit("rad", Some("number"))?;
    Ok(SassNumber::new(arena, tan(val), None))
}

/// Returns the arctangent of `number` in degrees.
///
/// `number` must be unitless.
///
/// Matches Dart: atan / Go: AtanNumber
pub fn atan_number<'parse>(
    arena: &'parse Bump,
    number: &SassNumber<'parse>,
) -> SassResult<SassNumber<'parse>> {
    number.assert_no_units(Some("number"))?;
    Ok(radians_to_degrees(arena, atan(number.value)))
}

/// Returns the arcsine of `number` in degrees.
///
/// `number` must be unitless.
///
/// Matches Dart: asin / Go: AsinNumber
pub fn asin_number<'parse>(
    arena: &'parse Bump,
    number: &SassNumber<'parse>,
) -> SassResult<SassNumber<'parse>> {
    number.assert_no_units(Some("number"))?;
    Ok(radians_to_degrees(arena, asin(number.value)))
}

/// Returns the arccosine of `number` in degrees.
///
/// `number` must be unitless.
///
/// Matches Dart: acos / Go: AcosNumber
pub fn acos_number<'parse>(
    arena: &'parse Bump,
    number: &SassNumber<'parse>,
) -> SassResult<SassNumber<'parse>> {
    number.assert_no_units(Some("number"))?;
    Ok(radians_to_degrees(arena, acos(number.value)))
}

/// Returns the absolute value of `number`, preserving units.
///
/// Matches Dart: abs / Go: AbsNumber
pub fn abs_number<'parse>(
    arena: &'parse Bump,
    number: &SassNumber<'parse>,
) -> SassResult<SassNumber<'parse>> {
    Ok(SassNumber::with_units(
        arena,
        abs(number.value),
        number.numerator_units.to_vec(),
        number.denominator_units.to_vec(),
    ))
}

/// Returns the logarithm of `number` with respect to `base`. If `base` is
/// `None`, the natural logarithm is returned.
///
/// Matches Dart: log / Go: LogNumber
///
/// Dart checks `$number` units before asserting `$base` is a number
/// (math.dart:154-168), so `base` arrives unasserted and is checked here —
/// only after the units check — preserving Dart's error precedence.
pub fn log_number<'parse>(
    arena: &'parse Bump,
    number: &SassNumber<'parse>,
    base: Option<&ValueKind<'parse>>,
) -> SassResult<SassNumber<'parse>> {
    if number.has_units() {
        let s = number.to_display_string()?;
        return Err(Box::new(SassError::Script {
            message: format!("Expected {s} to have no units."),
            argument_name: Some("number".into()),
        }));
    }
    match base {
        Some(b) => {
            let b = assert_number(b, Some("base"))?;
            if b.has_units() {
                let s = b.to_display_string()?;
                return Err(Box::new(SassError::Script {
                    message: format!("Expected {s} to have no units."),
                    argument_name: Some("base".into()),
                }));
            }
            Ok(SassNumber::new(arena, ln(number.value) / ln(b.value), None))
        }
        None => Ok(SassNumber::new(arena, ln(number.value), None)),
    }
}

/// Returns `base` raised to the power of `exponent`.
///
/// Matches Dart: pow / Go: PowNumber
pub fn pow_number<'parse>(
    arena: &'parse Bump,
    base: &SassNumber<'parse>,
    exponent: &SassNumber<'parse>,
) -> SassResult<SassNumber<'parse>> {
    base.assert_no_units(Some("base"))?;
    exponent.assert_no_units(Some("exponent"))?;
    Ok(SassNumber::new(
        arena,
        pow(base.value, exponent.value),
        None,
    ))
}

/// Returns the arctangent of `y`/`x` in degrees.
///
/// Matches Dart: atan2 / Go: Atan2Number
pub fn atan2_number<'parse>(
    arena: &'parse Bump,
    y: &SassNumber<'parse>,
    x: &SassNumber<'parse>,
) -> SassResult<SassNumber<'parse>> {
    let x_val = x.convert_value_to_match(y, Some("x"), Some("y"))?;
    Ok(radians_to_degrees(arena, atan2(y.value, x_val)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::ValueKind;
    use std::f64::consts::E;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn assert_script_err(err: Box<SassError>, want_msg: &str, want_arg: Option<&str>) {
        match *err {
            SassError::Script {
                message,
                argument_name,
            } => {
                assert_eq!(message, want_msg);
                assert_eq!(argument_name.as_deref(), want_arg);
            }
            other => panic!("expected Script error, got {other:?}"),
        }
    }

    #[test]
    fn test_sqrt_number() {
        let arena = Bump::new();
        let r = sqrt_number(&arena, &SassNumber::new(&arena, 9.0, None)).unwrap();
        assert!(approx(r.value, 3.0));
        let err = sqrt_number(&arena, &SassNumber::new(&arena, 10.0, Some("px"))).unwrap_err();
        assert_script_err(err, "Expected 10px to have no units.", Some("number"));
    }

    #[test]
    fn test_sin_number() {
        let arena = Bump::new();
        let r = sin_number(&arena, &SassNumber::new(&arena, 0.0, None)).unwrap();
        assert!(approx(r.value, 0.0));
        let r = sin_number(&arena, &SassNumber::new(&arena, 90.0, Some("deg"))).unwrap();
        assert!(approx(r.value, 1.0));
    }

    #[test]
    fn test_sin_number_bad_unit() {
        let arena = Bump::new();
        let err = sin_number(&arena, &SassNumber::new(&arena, 10.0, Some("px"))).unwrap_err();
        assert_script_err(
            err,
            "Expected 10px to have an angle unit (deg, grad, rad, turn).",
            Some("number"),
        );
    }

    #[test]
    fn test_cos_number() {
        let arena = Bump::new();
        let r = cos_number(&arena, &SassNumber::new(&arena, 0.0, None)).unwrap();
        assert!(approx(r.value, 1.0));
        let r = cos_number(&arena, &SassNumber::new(&arena, 90.0, Some("deg"))).unwrap();
        assert!(approx(r.value, 0.0));
    }

    #[test]
    fn test_tan_number() {
        let arena = Bump::new();
        let r = tan_number(&arena, &SassNumber::new(&arena, 0.0, None)).unwrap();
        assert!(approx(r.value, 0.0));
        let r = tan_number(&arena, &SassNumber::new(&arena, 45.0, Some("deg"))).unwrap();
        assert!(approx(r.value, 1.0));
    }

    #[test]
    fn test_atan_number() {
        let arena = Bump::new();
        let r = atan_number(&arena, &SassNumber::new(&arena, 0.0, None)).unwrap();
        assert!(approx(r.value, 0.0));
        assert!(r.has_unit("deg"));
        let err = atan_number(&arena, &SassNumber::new(&arena, 1.0, Some("px"))).unwrap_err();
        assert_script_err(err, "Expected 1px to have no units.", Some("number"));
    }

    #[test]
    fn test_asin_number() {
        let arena = Bump::new();
        let r = asin_number(&arena, &SassNumber::new(&arena, 0.0, None)).unwrap();
        assert!(approx(r.value, 0.0));
        let err = asin_number(&arena, &SassNumber::new(&arena, 1.0, Some("px"))).unwrap_err();
        assert_script_err(err, "Expected 1px to have no units.", Some("number"));
    }

    #[test]
    fn test_acos_number() {
        let arena = Bump::new();
        let r = acos_number(&arena, &SassNumber::new(&arena, 1.0, None)).unwrap();
        assert!(approx(r.value, 0.0));
        let err = acos_number(&arena, &SassNumber::new(&arena, 2.0, Some("px"))).unwrap_err();
        assert_script_err(err, "Expected 2px to have no units.", Some("number"));
    }

    #[test]
    fn test_abs_number() {
        let arena = Bump::new();
        let r = abs_number(&arena, &SassNumber::new(&arena, -10.0, Some("px"))).unwrap();
        assert!(approx(r.value, 10.0));
        assert!(r.has_unit("px"));
    }

    #[test]
    fn test_log_number() {
        let arena = Bump::new();
        let r = log_number(&arena, &SassNumber::new(&arena, E, None), None).unwrap();
        assert!(approx(r.value, 1.0));
        let base_kind = ValueKind::Number(SassNumber::new(&arena, 10.0, None));
        let r = log_number(
            &arena,
            &SassNumber::new(&arena, 100.0, None),
            Some(&base_kind),
        )
        .unwrap();
        assert!(approx(r.value, 2.0));
    }

    #[test]
    fn test_log_number_units_error() {
        let arena = Bump::new();
        let err = log_number(&arena, &SassNumber::new(&arena, 10.0, Some("px")), None).unwrap_err();
        assert_script_err(err, "Expected 10px to have no units.", Some("number"));
    }

    #[test]
    fn test_log_number_base_units_error() {
        let arena = Bump::new();
        let base_kind = ValueKind::Number(SassNumber::new(&arena, 2.0, Some("px")));
        let err = log_number(
            &arena,
            &SassNumber::new(&arena, 10.0, None),
            Some(&base_kind),
        )
        .unwrap_err();
        assert_script_err(err, "Expected 2px to have no units.", Some("base"));
    }

    #[test]
    fn test_pow_number() {
        let arena = Bump::new();
        let r = pow_number(
            &arena,
            &SassNumber::new(&arena, 2.0, None),
            &SassNumber::new(&arena, 3.0, None),
        )
        .unwrap();
        assert!(approx(r.value, 8.0));
    }

    #[test]
    fn test_pow_number_units_errors() {
        let arena = Bump::new();
        let err = pow_number(
            &arena,
            &SassNumber::new(&arena, 2.0, Some("px")),
            &SassNumber::new(&arena, 3.0, None),
        )
        .unwrap_err();
        assert_script_err(err, "Expected 2px to have no units.", Some("base"));
        let err = pow_number(
            &arena,
            &SassNumber::new(&arena, 2.0, None),
            &SassNumber::new(&arena, 3.0, Some("px")),
        )
        .unwrap_err();
        assert_script_err(err, "Expected 3px to have no units.", Some("exponent"));
    }

    #[test]
    fn test_atan2_number() {
        let arena = Bump::new();
        let r = atan2_number(
            &arena,
            &SassNumber::new(&arena, 1.0, None),
            &SassNumber::new(&arena, 0.0, None),
        )
        .unwrap();
        assert!(approx(r.value, 90.0));
        assert!(r.has_unit("deg"));
        let r = atan2_number(
            &arena,
            &SassNumber::new(&arena, 0.0, None),
            &SassNumber::new(&arena, 1.0, None),
        )
        .unwrap();
        assert!(approx(r.value, 0.0));
    }

    #[test]
    fn test_atan2_number_incompatible_units() {
        let arena = Bump::new();
        let err = atan2_number(
            &arena,
            &SassNumber::new(&arena, 1.0, Some("px")),
            &SassNumber::new(&arena, 1.0, Some("s")),
        )
        .unwrap_err();
        assert_script_err(err, "1s and $y: 1px have incompatible units.", Some("x"));
    }

    #[test]
    fn test_atan2_number_mixed_unitless() {
        let arena = Bump::new();
        let err = atan2_number(
            &arena,
            &SassNumber::new(&arena, 1.0, Some("px")),
            &SassNumber::new(&arena, 1.0, None),
        )
        .unwrap_err();
        assert_script_err(
            err,
            "1 and $y: 1px have incompatible units (one has units and the other doesn't).",
            Some("x"),
        );
    }
}
