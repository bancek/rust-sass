// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/number.dart (conversion tables, conversionFactor, canonical/unit-string helpers) + lib/src/value/number/single_unit.dart (known compatibilities)
// go-source: go/value/number_util.go

use std::collections::HashMap;
use std::collections::HashSet;
use std::f64::consts::PI;
use std::sync::LazyLock;

/// A nested map containing unit conversion rates.
///
/// `1unit1 * CONVERSION_FACTORS[unit2][unit1] = 1unit2`.
///
/// Matches Dart: _conversions (number.dart).
pub static CONVERSION_FACTORS: LazyLock<HashMap<&'static str, HashMap<&'static str, f64>>> =
    LazyLock::new(|| {
        let mut m = HashMap::new();
        // Length
        m.insert("in", {
            let mut inner = HashMap::new();
            inner.insert("in", 1.0);
            inner.insert("cm", 1.0 / 2.54);
            inner.insert("pc", 1.0 / 6.0);
            inner.insert("mm", 1.0 / 25.4);
            inner.insert("q", 1.0 / 101.6);
            inner.insert("pt", 1.0 / 72.0);
            inner.insert("px", 1.0 / 96.0);
            inner
        });
        m.insert("cm", {
            let mut inner = HashMap::new();
            inner.insert("in", 2.54);
            inner.insert("cm", 1.0);
            inner.insert("pc", 2.54 / 6.0);
            inner.insert("mm", 1.0 / 10.0);
            inner.insert("q", 1.0 / 40.0);
            inner.insert("pt", 2.54 / 72.0);
            inner.insert("px", 2.54 / 96.0);
            inner
        });
        m.insert("pc", {
            let mut inner = HashMap::new();
            inner.insert("in", 6.0);
            inner.insert("cm", 6.0 / 2.54);
            inner.insert("pc", 1.0);
            inner.insert("mm", 6.0 / 25.4);
            inner.insert("q", 6.0 / 101.6);
            inner.insert("pt", 1.0 / 12.0);
            inner.insert("px", 1.0 / 16.0);
            inner
        });
        m.insert("mm", {
            let mut inner = HashMap::new();
            inner.insert("in", 25.4);
            inner.insert("cm", 10.0);
            inner.insert("pc", 25.4 / 6.0);
            inner.insert("mm", 1.0);
            inner.insert("q", 1.0 / 4.0);
            inner.insert("pt", 25.4 / 72.0);
            inner.insert("px", 25.4 / 96.0);
            inner
        });
        m.insert("q", {
            let mut inner = HashMap::new();
            inner.insert("in", 101.6);
            inner.insert("cm", 40.0);
            inner.insert("pc", 101.6 / 6.0);
            inner.insert("mm", 4.0);
            inner.insert("q", 1.0);
            inner.insert("pt", 101.6 / 72.0);
            inner.insert("px", 101.6 / 96.0);
            inner
        });
        m.insert("pt", {
            let mut inner = HashMap::new();
            inner.insert("in", 72.0);
            inner.insert("cm", 72.0 / 2.54);
            inner.insert("pc", 12.0);
            inner.insert("mm", 72.0 / 25.4);
            inner.insert("q", 72.0 / 101.6);
            inner.insert("pt", 1.0);
            inner.insert("px", 3.0 / 4.0);
            inner
        });
        m.insert("px", {
            let mut inner = HashMap::new();
            inner.insert("in", 96.0);
            inner.insert("cm", 96.0 / 2.54);
            inner.insert("pc", 16.0);
            inner.insert("mm", 96.0 / 25.4);
            inner.insert("q", 96.0 / 101.6);
            inner.insert("pt", 4.0 / 3.0);
            inner.insert("px", 1.0);
            inner
        });
        // Rotation
        m.insert("deg", {
            let mut inner = HashMap::new();
            inner.insert("deg", 1.0);
            inner.insert("grad", 9.0 / 10.0);
            inner.insert("rad", 180.0 / PI);
            inner.insert("turn", 360.0);
            inner
        });
        m.insert("grad", {
            let mut inner = HashMap::new();
            inner.insert("deg", 10.0 / 9.0);
            inner.insert("grad", 1.0);
            inner.insert("rad", 200.0 / PI);
            inner.insert("turn", 400.0);
            inner
        });
        m.insert("rad", {
            let mut inner = HashMap::new();
            inner.insert("deg", PI / 180.0);
            inner.insert("grad", PI / 200.0);
            inner.insert("rad", 1.0);
            inner.insert("turn", 2.0 * PI);
            inner
        });
        m.insert("turn", {
            let mut inner = HashMap::new();
            inner.insert("deg", 1.0 / 360.0);
            inner.insert("grad", 1.0 / 400.0);
            inner.insert("rad", 1.0 / (2.0 * PI));
            inner.insert("turn", 1.0);
            inner
        });
        // Time
        m.insert("s", {
            let mut inner = HashMap::new();
            inner.insert("s", 1.0);
            inner.insert("ms", 1.0 / 1000.0);
            inner
        });
        m.insert("ms", {
            let mut inner = HashMap::new();
            inner.insert("s", 1000.0);
            inner.insert("ms", 1.0);
            inner
        });
        // Frequency
        m.insert("Hz", {
            let mut inner = HashMap::new();
            inner.insert("Hz", 1.0);
            inner.insert("kHz", 1000.0);
            inner
        });
        m.insert("kHz", {
            let mut inner = HashMap::new();
            inner.insert("Hz", 1.0 / 1000.0);
            inner.insert("kHz", 1.0);
            inner
        });
        // Pixel density
        m.insert("dpi", {
            let mut inner = HashMap::new();
            inner.insert("dpi", 1.0);
            inner.insert("dpcm", 2.54);
            inner.insert("dppx", 96.0);
            inner
        });
        m.insert("dpcm", {
            let mut inner = HashMap::new();
            inner.insert("dpi", 1.0 / 2.54);
            inner.insert("dpcm", 1.0);
            inner.insert("dppx", 96.0 / 2.54);
            inner
        });
        m.insert("dppx", {
            let mut inner = HashMap::new();
            inner.insert("dpi", 1.0 / 96.0);
            inner.insert("dpcm", 2.54 / 96.0);
            inner.insert("dppx", 1.0);
            inner
        });
        m
    });

/// Sets of units that are known to be compatible with one another in the
/// browser.
///
/// These units are likewise known to be *incompatible* with units in other
/// sets in this list.
///
/// Matches Dart: _knownCompatibilities (value/number/single_unit.dart)
pub static KNOWN_COMPATIBILITY_SETS: LazyLock<Vec<HashSet<&'static str>>> = LazyLock::new(|| {
    vec![
        HashSet::from([
            "em", "rem", "ex", "rex", "cap", "rcap", "ch", "rch", "ic", "ric", "lh", "rlh", "vw",
            "lvw", "svw", "dvw", "vh", "lvh", "svh", "dvh", "vi", "lvi", "svi", "dvi", "vb", "lvb",
            "svb", "dvb", "vmin", "lvmin", "svmin", "dvmin", "vmax", "lvmax", "svmax", "dvmax",
            "cqw", "cqh", "cqi", "cqb", "cqmin", "cqmax", "cm", "mm", "q", "in", "pt", "pc", "px",
        ]),
        HashSet::from(["deg", "grad", "rad", "turn"]),
        HashSet::from(["s", "ms"]),
        HashSet::from(["hz", "khz"]),
        HashSet::from(["dpi", "dpcm", "dppx"]),
    ]
});

/// Returns the compatibility set containing `unit` (case-insensitive), if it is a known unit.
///
/// Matches Dart: _knownCompatibilitiesByUnit lookup (value/number/single_unit.dart).
pub fn known_compatibility_set(unit: &str) -> Option<&'static HashSet<&'static str>> {
    let unit_lower = unit.to_lowercase();
    KNOWN_COMPATIBILITY_SETS
        .iter()
        .find(|&set| set.contains(unit_lower.as_str()))
        .map(|v| v as _)
}

/// A map from human-readable names of unit types to the convertible units that
/// fall into those types.
///
/// Matches Dart: _unitsByType (number.dart) / Go: unitsByType
pub fn units_by_type(typ: &str) -> &'static [&'static str] {
    match typ {
        "length" => &["in", "cm", "pc", "mm", "q", "pt", "px"],
        "angle" => &["deg", "grad", "rad", "turn"],
        "time" => &["s", "ms"],
        "frequency" => &["Hz", "kHz"],
        "pixel density" => &["dpi", "dpcm", "dppx"],
        _ => &[],
    }
}

/// A map from units to the human-readable names of those unit types.
///
/// Matches Dart: _typesByUnit (number.dart) / Go: typesByUnit
pub fn type_by_unit(unit: &str) -> Option<&'static str> {
    ["length", "angle", "time", "frequency", "pixel density"]
        .into_iter()
        .find(|&typ| units_by_type(typ).contains(&unit))
}

/// Returns the number of `unit1`s per `unit2`.
///
/// Equivalently, `1unit2 * conversion_factor(unit1, unit2) = 1unit1`.
/// Returns [`None`] when the units are not convertible.
///
/// Matches Dart: conversionFactor (number.dart).
pub fn conversion_factor(from: &str, to: &str) -> Option<f64> {
    if from == to {
        return Some(1.0);
    }
    CONVERSION_FACTORS.get(from)?.get(to).copied()
}

/// Returns a multiplier that encapsulates unit equivalence with `unit`.
///
/// That is, if `X unit1 == Y unit2`, then `X * canonical_multiplier_for_unit(unit1)
/// == Y * canonical_multiplier_for_unit(unit2)`.
/// Unknown units map to `1.0`.
///
/// Matches Dart: SassNumber.canonicalMultiplierForUnit (number.dart):
/// `1 / innerMap.values.first`, the reciprocal of the canonical row's first
/// entry.
pub fn canonical_multiplier_for_unit(unit: &str) -> f64 {
    match unit {
        "in" => 1.0,
        "cm" => 1.0 / 2.54,
        "pc" => 1.0 / 6.0,
        "mm" => 1.0 / 25.4,
        "q" => 1.0 / 101.6,
        "pt" => 1.0 / 72.0,
        "px" => 1.0 / 96.0,
        "deg" => 1.0,
        "grad" => 1.0 / (10.0 / 9.0),
        "rad" => 1.0 / (PI / 180.0),
        "turn" => 1.0 / (1.0 / 360.0),
        "s" => 1.0,
        "ms" => 1.0 / 1000.0,
        "Hz" => 1.0,
        "kHz" => 1000.0,
        "dpi" => 1.0,
        "dpcm" => 2.54,
        "dppx" => 96.0,
        _ => 1.0,
    }
}

/// Returns a multiplier that encapsulates unit equivalence in `units`.
///
/// That is, if `X units1 == Y units2`, then `X * canonical_multiplier_for_list(units1)
/// == Y * canonical_multiplier_for_list(units2)`.
///
/// Matches Dart: SassNumber._canonicalMultiplier (number.dart).
pub fn canonical_multiplier_for_list(units: &[String]) -> f64 {
    units
        .iter()
        .fold(1.0, |m, u| m * canonical_multiplier_for_unit(u))
}

/// Converts a unit list into an equivalent list in a canonical form, to make
/// it easier to check whether two numbers have compatible units.
///
/// Each convertible unit maps to the first unit of its type; unknown units
/// pass through unchanged.
///
/// Matches Dart: SassNumber._canonicalizeUnitList (number.dart).
pub fn canonicalize_unit_list(units: &[String]) -> Vec<String> {
    if units.is_empty() {
        return vec![];
    }
    let mut result: Vec<String> = units
        .iter()
        .map(|u| match type_by_unit(u) {
            Some(typ) => units_by_type(typ)[0].to_string(),
            None => u.clone(),
        })
        .collect();
    result.sort();
    result
}

/// Returns a human-readable string representation of `numerators` and `denominators`.
///
/// Note this differs from [`SassNumber::unit_string`](super::SassNumber::unit_string):
/// an empty list pair renders as `"no units"` here (used in error text),
/// while the method renders it as `""`.
///
/// Matches Dart: SassNumber._unitString (number.dart).
pub fn unit_string(numerators: &[String], denominators: &[String]) -> String {
    match (numerators.is_empty(), denominators.is_empty()) {
        (true, true) => String::new(),
        (true, false) if denominators.len() == 1 => format!("{}^-1", denominators[0]),
        (true, false) => format!("({})^-1", denominators.join("*")),
        (false, true) => numerators.join("*"),
        (false, false) if denominators.len() == 1 => {
            format!("{}/{}", numerators.join("*"), denominators[0])
        }
        (false, false) => format!("{}/({})", numerators.join("*"), denominators.join("*")),
    }
}

/// Order-sensitive slice equality for unit lists.
///
/// Matches Dart's `listEquals` use in `_coerceOrConvertValue` (number.dart).
pub fn slice_equal(a: &[String], b: &[String]) -> bool {
    a == b
}

/// Returns whether there exists a unit in `units1` that can be converted to a
/// unit in `units2`.
///
/// Matches Dart: SassNumber._areAnyConvertible (number.dart).
pub fn units_are_convertible(units1: &[String], units2: &[String]) -> bool {
    for u1 in units1 {
        if let Some(m) = CONVERSION_FACTORS.get(u1.as_str()) {
            for u2 in units2 {
                if m.contains_key(u2.as_str()) {
                    return true;
                }
            }
        } else if units2.contains(u1) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_multipliers() {
        // Dart: 1 / innerMap.values.first — first entry is the canonical
        // unit's row, so kHz/dpcm/dppx are >1, not reciprocals.
        assert_eq!(canonical_multiplier_for_unit("kHz"), 1000.0);
        assert_eq!(canonical_multiplier_for_unit("Hz"), 1.0);
        assert_eq!(canonical_multiplier_for_unit("dpcm"), 2.54);
        assert_eq!(canonical_multiplier_for_unit("dppx"), 96.0);
        assert_eq!(canonical_multiplier_for_unit("dpi"), 1.0);
        // Equality direction: 1kHz == 1000Hz means X*mult(X-unit) equal.
        assert_eq!(
            1.0 * canonical_multiplier_for_unit("kHz"),
            1000.0 * canonical_multiplier_for_unit("Hz")
        );
        assert_eq!(
            1.0 * canonical_multiplier_for_unit("dppx"),
            96.0 * canonical_multiplier_for_unit("dpi")
        );
    }

    #[test]
    fn test_conversion_factor() {
        assert_eq!(conversion_factor("px", "px"), Some(1.0));
        assert!(conversion_factor("px", "in").is_some());
        assert!(conversion_factor("in", "px").is_some());
        assert_eq!(conversion_factor("px", "deg"), None);
    }

    #[test]
    fn test_canonical_multiplier_for_unit() {
        let m = canonical_multiplier_for_unit("px");
        assert_ne!(m, 0.0);
        assert_eq!(canonical_multiplier_for_unit("unknown"), 1.0);
    }

    #[test]
    fn test_canonical_multiplier_for_list() {
        assert_eq!(canonical_multiplier_for_list(&[]), 1.0);
        let m = canonical_multiplier_for_list(&["px".to_string()]);
        assert_eq!(m, canonical_multiplier_for_unit("px"));
    }

    #[test]
    fn test_canonicalize_unit_list() {
        let result = canonicalize_unit_list(&["px".to_string(), "pt".to_string()]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], result[1], "both should be canonical form (in)");
        assert!(canonicalize_unit_list(&[]).is_empty());
    }

    #[test]
    fn test_unit_string() {
        let tests: &[(&[&str], &[&str], &str)] = &[
            (&["px"], &[], "px"),
            (&[], &[], ""),
            (&["px", "s"], &[], "px*s"),
            (&["px"], &["s"], "px/s"),
            (&[], &["s"], "s^-1"),
            (&["px"], &["s", "ms"], "px/(s*ms)"),
            (&["s", "ms"], &[], "s*ms"),
        ];
        for (num, den, expected) in tests {
            let num: Vec<String> = num.iter().map(|s| s.to_string()).collect();
            let den: Vec<String> = den.iter().map(|s| s.to_string()).collect();
            assert_eq!(unit_string(&num, &den), *expected);
        }
    }

    #[test]
    fn test_slice_equal() {
        assert!(slice_equal(
            &["a".to_string(), "b".to_string()],
            &["a".to_string(), "b".to_string()]
        ));
        assert!(!slice_equal(
            &["a".to_string()],
            &["a".to_string(), "b".to_string()]
        ));
    }

    #[test]
    fn test_units_are_convertible() {
        let px = &["px".to_string()];
        let inch = &["in".to_string()];
        let deg = &["deg".to_string()];
        assert!(units_are_convertible(px, inch));
        assert!(units_are_convertible(px, px));
        assert!(!units_are_convertible(px, deg));
        assert!(units_are_convertible(deg, &["rad".to_string()]));
    }

    #[test]
    fn test_type_by_unit() {
        assert_eq!(type_by_unit("px"), Some("length"));
        assert_eq!(type_by_unit("in"), Some("length"));
        assert_eq!(type_by_unit("rad"), Some("angle"));
        assert_eq!(type_by_unit("deg"), Some("angle"));
        assert_eq!(type_by_unit("s"), Some("time"));
        assert_eq!(type_by_unit("Hz"), Some("frequency"));
        assert_eq!(type_by_unit("dpi"), Some("pixel density"));
        assert_eq!(type_by_unit("foo"), None);
        assert_eq!(type_by_unit("em"), None);
    }

    #[test]
    fn test_units_by_type() {
        assert_eq!(
            units_by_type("length"),
            &["in", "cm", "pc", "mm", "q", "pt", "px"]
        );
        assert_eq!(units_by_type("angle"), &["deg", "grad", "rad", "turn"]);
        assert_eq!(units_by_type("time"), &["s", "ms"]);
        assert_eq!(units_by_type("frequency"), &["Hz", "kHz"]);
        assert_eq!(units_by_type("pixel density"), &["dpi", "dpcm", "dppx"]);
        assert!(units_by_type("bogus").is_empty());
    }
}
