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
use crate::math::sqrt;
use crate::math::tan;
use crate::value::assert_number;
use crate::value::SassNumber;
use crate::value::ValueKind;
use std::f64::consts::PI;

fn radians_to_degrees(rad: f64) -> SassNumber {
    // Returns `rad` as a number with unit `deg`.
    // Matches Dart: _radiansToDegrees.
    // Dart `_radiansToDegrees`: `radians * (180 / math.pi)` — parenthesized
    // division first (1-ULP difference vs `rad * 180.0 / PI`, masked at
    // precision 10 but transcribed for fidelity).
    SassNumber::new(rad * (180.0 / PI), Some("deg"))
}

/// Returns the square root of `number`.
///
/// `number` must be unitless; the result is unitless.
///
/// Matches Dart: sqrt / Go: SqrtNumber
pub fn sqrt_number(number: &SassNumber) -> SassResult<SassNumber> {
    number.assert_no_units(Some("number"))?;
    Ok(SassNumber::new(sqrt(number.value), None))
}

/// Returns the sine of `number`.
///
/// `number` is coerced to radians; the result is unitless.
///
/// Matches Dart: sin / Go: SinNumber
pub fn sin_number(number: &SassNumber) -> SassResult<SassNumber> {
    let val = number.coerce_value_to_unit("rad", Some("number"))?;
    Ok(SassNumber::new(sin(val), None))
}

/// Returns the cosine of `number`.
///
/// `number` is coerced to radians; the result is unitless.
///
/// Matches Dart: cos / Go: CosNumber
pub fn cos_number(number: &SassNumber) -> SassResult<SassNumber> {
    let val = number.coerce_value_to_unit("rad", Some("number"))?;
    Ok(SassNumber::new(cos(val), None))
}

/// Returns the tangent of `number`.
///
/// `number` is coerced to radians; the result is unitless.
///
/// Matches Dart: tan / Go: TanNumber
pub fn tan_number(number: &SassNumber) -> SassResult<SassNumber> {
    let val = number.coerce_value_to_unit("rad", Some("number"))?;
    Ok(SassNumber::new(tan(val), None))
}

/// Returns the arctangent of `number` in degrees.
///
/// `number` must be unitless.
///
/// Matches Dart: atan / Go: AtanNumber
pub fn atan_number(number: &SassNumber) -> SassResult<SassNumber> {
    number.assert_no_units(Some("number"))?;
    Ok(radians_to_degrees(atan(number.value)))
}

/// Returns the arcsine of `number` in degrees.
///
/// `number` must be unitless.
///
/// Matches Dart: asin / Go: AsinNumber
pub fn asin_number(number: &SassNumber) -> SassResult<SassNumber> {
    number.assert_no_units(Some("number"))?;
    Ok(radians_to_degrees(asin(number.value)))
}

/// Returns the arccosine of `number` in degrees.
///
/// `number` must be unitless.
///
/// Matches Dart: acos / Go: AcosNumber
pub fn acos_number(number: &SassNumber) -> SassResult<SassNumber> {
    number.assert_no_units(Some("number"))?;
    Ok(radians_to_degrees(acos(number.value)))
}

/// Returns the absolute value of `number`, preserving units.
///
/// Matches Dart: abs / Go: AbsNumber
pub fn abs_number(number: &SassNumber) -> SassResult<SassNumber> {
    Ok(SassNumber::with_units(
        abs(number.value),
        number.numerator_units.clone(),
        number.denominator_units.clone(),
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
pub fn log_number<'a, 'parse>(
    number: &SassNumber,
    base: Option<&'a ValueKind<'parse>>,
) -> SassResult<SassNumber> {
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
            Ok(SassNumber::new(ln(number.value) / ln(b.value), None))
        }
        None => Ok(SassNumber::new(ln(number.value), None)),
    }
}

/// Returns `base` raised to the power of `exponent`.
///
/// Matches Dart: pow / Go: PowNumber
pub fn pow_number(base: &SassNumber, exponent: &SassNumber) -> SassResult<SassNumber> {
    base.assert_no_units(Some("base"))?;
    exponent.assert_no_units(Some("exponent"))?;
    Ok(SassNumber::new(pow(base.value, exponent.value), None))
}

/// Returns the arctangent of `y`/`x` in degrees.
///
/// Matches Dart: atan2 / Go: Atan2Number
pub fn atan2_number(y: &SassNumber, x: &SassNumber) -> SassResult<SassNumber> {
    let x_val = x.convert_value_to_match(y, Some("x"), Some("y"))?;
    Ok(radians_to_degrees(atan2(y.value, x_val)))
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
        let r = sqrt_number(&SassNumber::new(9.0, None)).unwrap();
        assert!(approx(r.value, 3.0));
        let err = sqrt_number(&SassNumber::new(10.0, Some("px"))).unwrap_err();
        assert_script_err(err, "Expected 10px to have no units.", Some("number"));
    }

    #[test]
    fn test_sin_number() {
        let r = sin_number(&SassNumber::new(0.0, None)).unwrap();
        assert!(approx(r.value, 0.0));
        let r = sin_number(&SassNumber::new(90.0, Some("deg"))).unwrap();
        assert!(approx(r.value, 1.0));
    }

    #[test]
    fn test_sin_number_bad_unit() {
        let err = sin_number(&SassNumber::new(10.0, Some("px"))).unwrap_err();
        assert_script_err(
            err,
            "Expected 10px to have an angle unit (deg, grad, rad, turn).",
            Some("number"),
        );
    }

    #[test]
    fn test_cos_number() {
        let r = cos_number(&SassNumber::new(0.0, None)).unwrap();
        assert!(approx(r.value, 1.0));
        let r = cos_number(&SassNumber::new(90.0, Some("deg"))).unwrap();
        assert!(approx(r.value, 0.0));
    }

    #[test]
    fn test_tan_number() {
        let r = tan_number(&SassNumber::new(0.0, None)).unwrap();
        assert!(approx(r.value, 0.0));
        let r = tan_number(&SassNumber::new(45.0, Some("deg"))).unwrap();
        assert!(approx(r.value, 1.0));
    }

    #[test]
    fn test_atan_number() {
        let r = atan_number(&SassNumber::new(0.0, None)).unwrap();
        assert!(approx(r.value, 0.0));
        assert!(r.has_unit("deg"));
        let err = atan_number(&SassNumber::new(1.0, Some("px"))).unwrap_err();
        assert_script_err(err, "Expected 1px to have no units.", Some("number"));
    }

    #[test]
    fn test_asin_number() {
        let r = asin_number(&SassNumber::new(0.0, None)).unwrap();
        assert!(approx(r.value, 0.0));
        let err = asin_number(&SassNumber::new(1.0, Some("px"))).unwrap_err();
        assert_script_err(err, "Expected 1px to have no units.", Some("number"));
    }

    #[test]
    fn test_acos_number() {
        let r = acos_number(&SassNumber::new(1.0, None)).unwrap();
        assert!(approx(r.value, 0.0));
        let err = acos_number(&SassNumber::new(2.0, Some("px"))).unwrap_err();
        assert_script_err(err, "Expected 2px to have no units.", Some("number"));
    }

    #[test]
    fn test_abs_number() {
        let r = abs_number(&SassNumber::new(-10.0, Some("px"))).unwrap();
        assert!(approx(r.value, 10.0));
        assert!(r.has_unit("px"));
    }

    #[test]
    fn test_log_number() {
        let r = log_number(&SassNumber::new(E, None), None).unwrap();
        assert!(approx(r.value, 1.0));
        let base_kind = ValueKind::Number(SassNumber::new(10.0, None));
        let r = log_number(&SassNumber::new(100.0, None), Some(&base_kind)).unwrap();
        assert!(approx(r.value, 2.0));
    }

    #[test]
    fn test_log_number_units_error() {
        let err = log_number(&SassNumber::new(10.0, Some("px")), None).unwrap_err();
        assert_script_err(err, "Expected 10px to have no units.", Some("number"));
    }

    #[test]
    fn test_log_number_base_units_error() {
        let base_kind = ValueKind::Number(SassNumber::new(2.0, Some("px")));
        let err = log_number(&SassNumber::new(10.0, None), Some(&base_kind)).unwrap_err();
        assert_script_err(err, "Expected 2px to have no units.", Some("base"));
    }

    #[test]
    fn test_pow_number() {
        let r = pow_number(&SassNumber::new(2.0, None), &SassNumber::new(3.0, None)).unwrap();
        assert!(approx(r.value, 8.0));
    }

    #[test]
    fn test_pow_number_units_errors() {
        let err = pow_number(
            &SassNumber::new(2.0, Some("px")),
            &SassNumber::new(3.0, None),
        )
        .unwrap_err();
        assert_script_err(err, "Expected 2px to have no units.", Some("base"));
        let err = pow_number(
            &SassNumber::new(2.0, None),
            &SassNumber::new(3.0, Some("px")),
        )
        .unwrap_err();
        assert_script_err(err, "Expected 3px to have no units.", Some("exponent"));
    }

    #[test]
    fn test_atan2_number() {
        let r = atan2_number(&SassNumber::new(1.0, None), &SassNumber::new(0.0, None)).unwrap();
        assert!(approx(r.value, 90.0));
        assert!(r.has_unit("deg"));
        let r = atan2_number(&SassNumber::new(0.0, None), &SassNumber::new(1.0, None)).unwrap();
        assert!(approx(r.value, 0.0));
    }

    #[test]
    fn test_atan2_number_incompatible_units() {
        let err = atan2_number(
            &SassNumber::new(1.0, Some("px")),
            &SassNumber::new(1.0, Some("s")),
        )
        .unwrap_err();
        assert_script_err(err, "1s and $y: 1px have incompatible units.", Some("x"));
    }

    #[test]
    fn test_atan2_number_mixed_unitless() {
        let err = atan2_number(
            &SassNumber::new(1.0, Some("px")),
            &SassNumber::new(1.0, None),
        )
        .unwrap_err();
        assert_script_err(
            err,
            "1 and $y: 1px have incompatible units (one has units and the other doesn't).",
            Some("x"),
        );
    }
}
