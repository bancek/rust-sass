// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/number.dart + lib/src/value/number/single_unit.dart + lib/src/value/number/unitless.dart + lib/src/value/number/complex.dart (subclasses folded into one struct)
// go-source: go/value/number.go + go/value/number_unitless.go + go/value/number_single_unit.go + go/value/number_complex.go

use crate::common::exception::{SassError, SassResult};
use crate::util::number;
use crate::util::trim_ascii::a;
use crate::util::utils::pluralize;
use crate::value::number_util::canonical_multiplier_for_list;
use crate::value::number_util::canonical_multiplier_for_unit;
use crate::value::number_util::canonicalize_unit_list;
use crate::value::number_util::conversion_factor;
use crate::value::number_util::known_compatibility_set;
use crate::value::number_util::slice_equal;
use crate::value::number_util::type_by_unit;
use crate::value::number_util::unit_string;
use crate::value::number_util::units_are_convertible;
use crate::value::number_util::units_by_type;

use crate::serialize::SerializeVisitor;
use crate::value::ValueVisitor;

#[derive(Clone, Debug)]
/// A SassScript number.
///
/// Numbers can have units. Although there is no literal syntax for it, numbers
/// support numerator and denominator units (for example, `miles/hour`). These
/// are expected to be resolved before being emitted to CSS.
///
/// This is a single struct folding Dart's three subclasses (`Unitless`,
/// `SingleUnit`, `Complex`): arithmetic dispatches at runtime on whether the
/// operands have units.
pub struct SassNumber {
    /// The value of this number.
    ///
    /// Sass stores all numbers as [`f64`] even when `self` is an integer from
    /// Sass's perspective. Use [`is_int`](Self::is_int) to test for
    /// integer-ness, [`as_int`](Self::as_int) to read it, or
    /// [`assert_int`](Self::assert_int) to do both at once.
    pub value: f64,
    /// This number's numerator units.
    pub numerator_units: Vec<String>,
    /// This number's denominator units.
    pub denominator_units: Vec<String>,
    // The representation of this number as two slash-separated numbers, if it
    // has one.
    pub as_slash: Option<Box<(SassNumber, SassNumber)>>,
}

impl SassNumber {
    /// Creates a number, optionally with a single numerator unit.
    ///
    /// This matches the numbers that can be written as literals.
    /// [`with_units`](Self::with_units) can be used for more complex units.
    pub fn new(value: f64, unit: Option<&str>) -> Self {
        match unit {
            Some(u) => SassNumber {
                value,
                numerator_units: vec![u.to_string()],
                denominator_units: vec![],
                as_slash: None,
            },
            None => SassNumber {
                value,
                numerator_units: vec![],
                denominator_units: vec![],
                as_slash: None,
            },
        }
    }

    /// Creates a number with full numerator and denominator units.
    ///
    /// Units that cancel (a denominator convertible to a numerator) are
    /// simplified away, folding the conversion factor into the value; a fully
    /// cancelled result becomes unitless or single-unit.
    pub fn with_units(value: f64, num_units: Vec<String>, den_units: Vec<String>) -> Self {
        let mut num = num_units;
        let mut den = vec![];
        let mut val = value;
        let remaining_den = den_units;
        for rden in remaining_den {
            let mut simplified = false;
            for i in 0..num.len() {
                if let Some(factor) = conversion_factor(&rden, &num[i]) {
                    val *= factor;
                    num.remove(i);
                    simplified = true;
                    break;
                }
            }
            if !simplified {
                den.push(rden);
            }
        }
        SassNumber {
            value: val,
            numerator_units: num,
            denominator_units: den,
            as_slash: None,
        }
    }

    /// Whether `self` is an integer, up to fuzzy equality.
    ///
    /// This may return `false` for very large floats even when mathematically
    /// integral, where no platform has an exact integer representation.
    pub fn is_int(&self) -> bool {
        number::fuzzy_is_int(self.value)
    }

    /// If `self` is an integer per [`is_int`](Self::is_int), returns [`value`](Self::value) as an [`i64`].
    ///
    /// Otherwise returns [`None`].
    pub fn as_int(&self) -> Option<i64> {
        number::fuzzy_as_int(self.value)
    }

    /// Serializes this number in CSS mode.
    pub fn to_css_string(&self, quote: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(quote, false);
        visitor.visit_number(self)?;
        Ok(visitor.into_string())
    }

    /// Serializes this number in inspect mode.
    ///
    /// Matches Go: SassNumber.String() (SerializeValueInspect)
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(true, true);
        visitor.visit_number(self)?;
        Ok(visitor.into_string())
    }

    /// Returns [`value`](Self::value) as an [`i64`] if it is an integer per [`is_int`](Self::is_int).
    ///
    /// Returns a script error otherwise. If this came from a function
    /// argument, `name` is the argument name (without `$`) used for error
    /// reporting.
    pub fn assert_int(&self, name: Option<&str>) -> SassResult<i64> {
        match self.as_int() {
            Some(i) => Ok(i),
            None => {
                let s = self.to_display_string()?;
                Err(Box::new(SassError::Script {
                    message: format!("{s} is not an int."),
                    argument_name: name.map(str::to_string),
                }))
            }
        }
    }

    /// Whether `self` has any units.
    ///
    /// If a function expects a number to have no units, it should use
    /// [`assert_no_units`](Self::assert_no_units). If it expects a particular
    /// unit, it should use [`assert_unit`](Self::assert_unit).
    pub fn has_units(&self) -> bool {
        !self.numerator_units.is_empty() || !self.denominator_units.is_empty()
    }

    /// Whether `self` has more than one numerator unit, or any denominator units.
    ///
    /// This is `true` for numbers whose units make them unrepresentable as CSS
    /// lengths.
    pub fn has_complex_units(&self) -> bool {
        // Dart/Go: complex unless unitless or exactly one numerator unit with no denominator.
        if self.numerator_units.is_empty() && self.denominator_units.is_empty() {
            return false; // unitless
        }
        if self.numerator_units.len() == 1 && self.denominator_units.is_empty() {
            return false; // single unit (e.g., 1px)
        }
        true
    }

    /// Returns whether `self` has `unit` as its only unit (and as a numerator).
    pub fn has_unit(&self, unit: &str) -> bool {
        self.numerator_units.len() == 1
            && self.denominator_units.is_empty()
            && self.numerator_units[0] == unit
    }

    /// Returns a human-readable string representation of this number's units.
    pub fn unit_string(&self) -> String {
        unit_string(&self.numerator_units, &self.denominator_units)
    }

    /// Throws a script error unless `self` has no units.
    ///
    /// If this came from a function argument, `name` is the argument name
    /// (without `$`) used for error reporting.
    pub fn assert_no_units(&self, name: Option<&str>) -> SassResult<()> {
        if !self.has_units() {
            Ok(())
        } else {
            let s = self.to_display_string()?;
            Err(Box::new(SassError::Script {
                message: format!("Expected {s} to have no units."),
                argument_name: name.map(str::to_string),
            }))
        }
    }

    /// Throws a script error unless `self` has `unit` as its only unit (and as a numerator).
    ///
    /// If this came from a function argument, `name` is the argument name
    /// (without `$`) used for error reporting.
    pub fn assert_unit(&self, unit: &str, name: Option<&str>) -> SassResult<()> {
        if self.has_unit(unit) {
            Ok(())
        } else {
            let s = self.to_display_string()?;
            Err(Box::new(SassError::Script {
                message: format!("Expected {s} to have unit \"{unit}\"."),
                argument_name: name.map(str::to_string),
            }))
        }
    }

    /// Returns whether `self` has units that are compatible with `other`.
    ///
    /// Unlike [`is_comparable_to`](Self::is_comparable_to), unitless numbers
    /// are only considered compatible with other unitless numbers.
    pub fn has_compatible_units(&self, other: &SassNumber) -> bool {
        if self.numerator_units.len() != other.numerator_units.len()
            || self.denominator_units.len() != other.denominator_units.len()
        {
            return false;
        }
        self.is_comparable_to(other)
    }

    /// Returns whether `self` can be coerced to the given `unit`.
    ///
    /// This always returns `true` for a unitless number.
    pub fn compatible_with_unit(&self, unit: &str) -> bool {
        if !self.has_units() {
            return true;
        }
        if self.has_complex_units() {
            return false;
        }
        conversion_factor(unit, &self.numerator_units[0]).is_some()
    }

    // Returns whether this number can be compared to `other`.
    //
    // Two numbers can be compared if they have compatible units, or if either
    // number has no units.
    //
    // Matches Dart: SassNumber.isComparableTo (number.dart).
    pub fn is_comparable_to(&self, other: &SassNumber) -> bool {
        if !self.has_units() || !other.has_units() {
            return true;
        }
        // Try coerce
        let result = other.coerce_value_to_match(self, None, None);
        result.is_ok()
    }

    /// Returns [`value`](Self::value), converted to the same units as `other`.
    ///
    /// Unlike [`convert_value_to_match`](Self::convert_value_to_match), this
    /// does *not* throw an error if this number is unitless and `other` is
    /// not, or vice versa. Instead, it treats all unitless numbers as
    /// convertible to and from all units without changing the value.
    ///
    /// Throws a script error if this number's units aren't compatible with
    /// `other`'s units. If this came from a function argument, `name` is the
    /// argument name (without `$`) and `other_name` is the argument name for
    /// `other`.
    pub fn coerce_value_to_match(
        &self,
        other: &SassNumber,
        name: Option<&str>,
        other_name: Option<&str>,
    ) -> SassResult<f64> {
        self.coerce_or_convert_value(
            &other.numerator_units,
            &other.denominator_units,
            true,
            name,
            Some(other),
            other_name,
        )
    }

    /// Returns [`value`](Self::value), converted to the same units as `other`.
    ///
    /// Throws a script error if this number's units aren't compatible with
    /// `other`'s units, or if either number is unitless but the other is not.
    /// If this came from a function argument, `name` is the argument name
    /// (without `$`) and `other_name` is the argument name for `other`.
    pub fn convert_value_to_match(
        &self,
        other: &SassNumber,
        name: Option<&str>,
        other_name: Option<&str>,
    ) -> SassResult<f64> {
        self.coerce_or_convert_value(
            &other.numerator_units,
            &other.denominator_units,
            false,
            name,
            Some(other),
            other_name,
        )
    }

    /// A shorthand for [`convert_value`](Self::convert_value) with only one numerator unit.
    pub fn convert_value_to_unit(&self, unit: &str, name: Option<&str>) -> SassResult<f64> {
        self.coerce_or_convert_value(&[unit.to_string()], &[], false, name, None, None)
    }

    /// A shorthand for [`coerce_value`](Self::coerce_value) with only one numerator unit.
    pub fn coerce_value_to_unit(&self, unit: &str, name: Option<&str>) -> SassResult<f64> {
        self.coerce_or_convert_value(&[unit.to_string()], &[], true, name, None, None)
    }

    /// Builds the error for incompatible unit conversions.
    ///
    /// Matches Go: numCompatibilityError
    fn compatibility_error(
        &self,
        new_numerators: &[String],
        new_denominators: &[String],
        name: Option<&str>,
        other: Option<&SassNumber>,
        other_name: Option<&str>,
    ) -> Box<SassError> {
        let argument_name = name.filter(|n| !n.is_empty()).map(str::to_string);
        let other_has_units = !new_numerators.is_empty() || !new_denominators.is_empty();
        if let Some(other_num) = other {
            let n_str = match self.to_display_string() {
                Ok(s) => s,
                Err(e) => return e,
            };
            let other_str = match other_num.to_display_string() {
                Ok(s) => s,
                Err(e) => return e,
            };
            let mut msg = format!("{n_str} and");
            if let Some(on) = other_name.filter(|n| !n.is_empty()) {
                msg.push_str(&format!(" ${on}:"));
            }
            msg.push_str(&format!(" {other_str} have incompatible units"));
            if !self.has_units() || !other_has_units {
                msg.push_str(" (one has units and the other doesn't)");
            }
            return Box::new(SassError::Script {
                message: format!("{msg}."),
                argument_name,
            });
        }
        if !other_has_units {
            let n_str = match self.to_display_string() {
                Ok(s) => s,
                Err(e) => return e,
            };
            return Box::new(SassError::Script {
                message: format!("Expected {n_str} to have no units."),
                argument_name,
            });
        }
        if new_numerators.len() == 1 && new_denominators.is_empty() {
            if let Some(typ) = type_by_unit(&new_numerators[0]) {
                let n_str = match self.to_display_string() {
                    Ok(s) => s,
                    Err(e) => return e,
                };
                return Box::new(SassError::Script {
                    message: format!(
                        "Expected {n_str} to have {} unit ({}).",
                        a(typ),
                        units_by_type(typ).join(", ")
                    ),
                    argument_name,
                });
            }
        }
        let n_str = match self.to_display_string() {
            Ok(s) => s,
            Err(e) => return e,
        };
        let unit_str = unit_string(new_numerators, new_denominators);
        Box::new(SassError::Script {
            message: format!(
                "Expected {n_str} to have {} {unit_str}.",
                pluralize(
                    "unit",
                    (new_numerators.len() + new_denominators.len()) as i32,
                    None
                )
            ),
            argument_name,
        })
    }

    /// Converts [`value`](Self::value) to `new_numerators` and `new_denominators`.
    ///
    /// If `coerce_unitless` is `true`, this considers unitless numbers
    /// convertible to and from any unit. Otherwise it returns a script error
    /// for such a conversion. If `other` is passed, it should be the number
    /// from which the new units are derived; `name` and `other_name` are the
    /// Sass function parameter names of `self` and `other` for error
    /// reporting.
    pub fn coerce_or_convert_value(
        &self,
        new_numerators: &[String],
        new_denominators: &[String],
        coerce_unitless: bool,
        name: Option<&str>,
        other: Option<&SassNumber>,
        other_name: Option<&str>,
    ) -> SassResult<f64> {
        if slice_equal(&self.numerator_units, new_numerators)
            && slice_equal(&self.denominator_units, new_denominators)
        {
            return Ok(self.value);
        }
        let other_has_units = !new_numerators.is_empty() || !new_denominators.is_empty();
        if coerce_unitless && (!self.has_units() || !other_has_units) {
            return Ok(self.value);
        }

        let mut v = self.value;
        let mut old_numerators: Vec<String> = self.numerator_units.clone();

        for new_num in new_numerators {
            let mut found = false;
            for i in 0..old_numerators.len() {
                if let Some(factor) = conversion_factor(new_num, &old_numerators[i]) {
                    v *= factor;
                    old_numerators.remove(i);
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(self.compatibility_error(
                    new_numerators,
                    new_denominators,
                    name,
                    other,
                    other_name,
                ));
            }
        }

        let mut old_denominators: Vec<String> = self.denominator_units.clone();
        for new_den in new_denominators {
            let mut found = false;
            for i in 0..old_denominators.len() {
                if let Some(factor) = conversion_factor(new_den, &old_denominators[i]) {
                    v /= factor;
                    old_denominators.remove(i);
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(self.compatibility_error(
                    new_numerators,
                    new_denominators,
                    name,
                    other,
                    other_name,
                ));
            }
        }

        if !old_numerators.is_empty() || !old_denominators.is_empty() {
            return Err(self.compatibility_error(
                new_numerators,
                new_denominators,
                name,
                other,
                other_name,
            ));
        }

        Ok(v)
    }

    // Returns a number with the same units as `self` but with `v` as its value.
    //
    // Matches Dart: SassNumber.withValue (number.dart).
    pub fn with_value(&self, v: f64) -> Self {
        SassNumber {
            value: v,
            numerator_units: self.numerator_units.clone(),
            denominator_units: self.denominator_units.clone(),
            as_slash: None,
        }
    }

    /// Returns a copy of `self` with the given numerator/denominator units and the same value.
    ///
    /// Unlike [`with_units`](Self::with_units) (the constructor), no
    /// simplification is performed.
    pub fn with_units_vec(&self, num_units: Vec<String>, den_units: Vec<String>) -> Self {
        SassNumber {
            value: self.value,
            numerator_units: num_units,
            denominator_units: den_units,
            as_slash: self.as_slash.clone(),
        }
    }

    // Returns a copy of `self` with `as_slash` set to `num` and `den`.
    //
    // Matches Dart: SassNumber.withSlash (number.dart).
    pub fn with_slash(&self, num: SassNumber, den: SassNumber) -> Self {
        SassNumber {
            value: self.value,
            numerator_units: self.numerator_units.clone(),
            denominator_units: self.denominator_units.clone(),
            as_slash: Some(Box::new((num, den))),
        }
    }

    /// Whether `self` carries a slash-separated representation.
    pub fn has_slash(&self) -> bool {
        self.as_slash.is_some()
    }

    /// The slash-separated numerator/denominator pair, if one is set.
    pub fn slash_pair(&self) -> Option<(&SassNumber, &SassNumber)> {
        self.as_slash.as_ref().map(|b| (&b.0, &b.1))
    }

    // Returns a copy of `self` without `as_slash` set.
    //
    // Matches Dart: SassNumber.withoutSlash (number.dart).
    pub fn without_slash(&self) -> Self {
        SassNumber {
            value: self.value,
            numerator_units: self.numerator_units.clone(),
            denominator_units: self.denominator_units.clone(),
            as_slash: None,
        }
    }

    // Returns a suggested Sass snippet for converting a variable named `name`
    // (without `$`) containing this number into a number with the same value
    // and the given `unit`.
    //
    // If `unit` is `None`, this forces the number to be unitless. Used for
    // deprecation warnings when restricting which units a function allows.
    //
    // Matches Dart: SassNumber.unitSuggestion (number.dart).
    pub fn unit_suggestion(&self, name: &str, unit: Option<&str>) -> String {
        let mut result = format!("${}", name);
        for den in &self.denominator_units {
            result.push_str(&format!(" * 1{}", den));
        }
        for num in &self.numerator_units {
            result.push_str(&format!(" / 1{}", num));
        }
        if let Some(u) = unit {
            if !u.is_empty() {
                result.push_str(&format!(" * 1{}", u));
            }
        }
        if !self.numerator_units.is_empty() {
            format!("calc({})", result)
        } else {
            result
        }
    }

    /// Sass equality: same canonical units and fuzzily equal values.
    ///
    /// Unitless numbers only equal other unitless numbers; convertible units
    /// (such as `1in` and `96px`) compare through their canonical multipliers.
    pub fn equals(&self, other: &SassNumber) -> bool {
        if self.numerator_units.len() != other.numerator_units.len()
            || self.denominator_units.len() != other.denominator_units.len()
        {
            return false;
        }
        if !self.has_units() {
            return number::fuzzy_equals(self.value, other.value);
        }
        let canon_num = canonicalize_unit_list(&self.numerator_units);
        let other_canon_num = canonicalize_unit_list(&other.numerator_units);
        let canon_den = canonicalize_unit_list(&self.denominator_units);
        let other_canon_den = canonicalize_unit_list(&other.denominator_units);
        if canon_num != other_canon_num || canon_den != other_canon_den {
            return false;
        }
        number::fuzzy_equals(
            self.value * canonical_multiplier_for_list(&self.numerator_units)
                / canonical_multiplier_for_list(&self.denominator_units),
            other.value * canonical_multiplier_for_list(&other.numerator_units)
                / canonical_multiplier_for_list(&other.denominator_units),
        )
    }

    /// A hash code matching [`equals`](Self::equals).
    ///
    /// Hashes the value scaled into canonical units, so `1in` and `96px`
    /// hash equally.
    pub fn hash_code(&self) -> i32 {
        let mut multiplier = 1.0;
        for unit in &self.numerator_units {
            multiplier *= canonical_multiplier_for_unit(unit);
        }
        for unit in &self.denominator_units {
            multiplier /= canonical_multiplier_for_unit(unit);
        }
        number::fuzzy_hash_code(self.value * multiplier)
    }

    // Converts `other`'s value to be compatible with this number's, and calls
    // `op` with the resulting values.
    //
    // Returns a script error if the two numbers' units are incompatible. If
    // the conversion fails, it is re-run in the other direction so the error
    // message prints `self` before `other`.
    //
    // Matches Dart: SassNumber._coerceUnits (number.dart).
    pub fn coerce_units<T>(&self, other: &SassNumber, op: impl Fn(f64, f64) -> T) -> SassResult<T> {
        match other.coerce_value_to_match(self, None, None) {
            Ok(coerced) => Ok(op(self.value, coerced)),
            Err(err) => match self.coerce_value_to_match(other, None, None) {
                Err(err2) => Err(err2),
                Ok(_) => Err(err),
            },
        }
    }

    // Sass `+` between two numbers. A unitless operand takes on the other's
    // units; two unitful operands are coerced to match.
    //
    // Matches Dart: SassNumber.plus (number.dart); color-mixing and string
    // concatenation live on the `Value` dispatch, not here.
    pub fn plus_num(&self, other: &SassNumber) -> SassResult<SassNumber> {
        if !self.has_units() {
            if other.has_units() {
                return Ok(other.with_value(self.value + other.value));
            }
            return Ok(SassNumber::new(self.value + other.value, None));
        }
        if !other.has_units() {
            return Ok(self.with_value(self.value + other.value));
        }
        self.coerce_units(other, |a, b| a + b)
            .map(|v| self.with_value(v))
    }

    // Sass `-` between two numbers. Same unit rules as `plus_num`.
    //
    // Matches Dart: SassNumber.minus (number.dart).
    pub fn minus_num(&self, other: &SassNumber) -> SassResult<SassNumber> {
        if !self.has_units() {
            if other.has_units() {
                return Ok(other.with_value(self.value - other.value));
            }
            return Ok(SassNumber::new(self.value - other.value, None));
        }
        if !other.has_units() {
            return Ok(self.with_value(self.value - other.value));
        }
        self.coerce_units(other, |a, b| a - b)
            .map(|v| self.with_value(v))
    }

    // Sass `*` between two numbers. A unitless operand preserves the other's
    // units; two unitful operands multiply their unit lists.
    //
    // Matches Dart: SassNumber.times (number.dart).
    pub fn times_num(&self, other: &SassNumber) -> SassResult<SassNumber> {
        if !self.has_units() {
            if other.has_units() {
                return Ok(other.with_value(self.value * other.value));
            }
            return Ok(SassNumber::new(self.value * other.value, None));
        }
        if !other.has_units() {
            return Ok(self.with_value(self.value * other.value));
        }
        Ok(self.multiply_units(
            self.value * other.value,
            &other.numerator_units,
            &other.denominator_units,
        ))
    }

    // Sass `/` between two numbers. A unitless divisor preserves units; a
    // unitful divisor inverts its units via `multiply_units`.
    //
    // Matches Dart: SassNumber.dividedBy (number.dart).
    pub fn divided_by_num(&self, other: &SassNumber) -> SassResult<SassNumber> {
        if !self.has_units() {
            if other.has_units() {
                return Ok(SassNumber::with_units(
                    self.value / other.value,
                    other.denominator_units.clone(),
                    other.numerator_units.clone(),
                ));
            }
            return Ok(SassNumber::new(self.value / other.value, None));
        }
        if !other.has_units() {
            return Ok(self.with_value(self.value / other.value));
        }
        Ok(self.multiply_units(
            self.value / other.value,
            &other.denominator_units,
            &other.numerator_units,
        ))
    }

    // Sass `%` between two numbers, using floored-division modulo semantics.
    // A unitless operand takes on the other's units.
    //
    // Matches Dart: SassNumber.modulo (number.dart).
    pub fn modulo_num(&self, other: &SassNumber) -> SassResult<SassNumber> {
        if !self.has_units() {
            let result_val = number::modulo_like_sass(self.value, other.value);
            if other.has_units() {
                return Ok(other.with_value(result_val));
            }
            return Ok(SassNumber::new(result_val, None));
        }
        if !other.has_units() {
            return Ok(self.with_value(number::modulo_like_sass(self.value, other.value)));
        }
        self.coerce_units(other, number::modulo_like_sass)
            .map(|v| self.with_value(v))
    }

    // Sass `>` between two numbers, coerced to match; unitless operands
    // compare by raw value with fuzzy equality.
    //
    // Matches Dart: SassNumber.greaterThan (number.dart).
    pub fn greater_than_num(&self, other: &SassNumber) -> SassResult<bool> {
        if !self.has_units() {
            return Ok(number::fuzzy_greater_than(self.value, other.value));
        }
        if !other.has_units() {
            return Ok(number::fuzzy_greater_than(self.value, other.value));
        }
        self.coerce_units(other, number::fuzzy_greater_than)
    }

    // Sass `>=` between two numbers. Same coercion rules as `greater_than_num`.
    //
    // Matches Dart: SassNumber.greaterThanOrEquals (number.dart).
    pub fn greater_than_or_equals_num(&self, other: &SassNumber) -> SassResult<bool> {
        if !self.has_units() {
            return Ok(number::fuzzy_greater_than_or_equals(
                self.value,
                other.value,
            ));
        }
        if !other.has_units() {
            return Ok(number::fuzzy_greater_than_or_equals(
                self.value,
                other.value,
            ));
        }
        self.coerce_units(other, number::fuzzy_greater_than_or_equals)
    }

    // Sass `<` between two numbers. Same coercion rules as `greater_than_num`.
    //
    // Matches Dart: SassNumber.lessThan (number.dart).
    pub fn less_than_num(&self, other: &SassNumber) -> SassResult<bool> {
        if !self.has_units() {
            return Ok(number::fuzzy_less_than(self.value, other.value));
        }
        if !other.has_units() {
            return Ok(number::fuzzy_less_than(self.value, other.value));
        }
        self.coerce_units(other, number::fuzzy_less_than)
    }

    // Sass `<=` between two numbers. Same coercion rules as `greater_than_num`.
    //
    // Matches Dart: SassNumber.lessThanOrEquals (number.dart).
    pub fn less_than_or_equals_num(&self, other: &SassNumber) -> SassResult<bool> {
        if !self.has_units() {
            return Ok(number::fuzzy_less_than_or_equals(self.value, other.value));
        }
        if !other.has_units() {
            return Ok(number::fuzzy_less_than_or_equals(self.value, other.value));
        }
        self.coerce_units(other, number::fuzzy_less_than_or_equals)
    }

    // Unary `-`, preserving units.
    //
    // Matches Dart: SassNumber.unaryMinus (number.dart).
    pub fn unary_minus_num(&self) -> SassNumber {
        self.with_value(-self.value)
    }

    /// If [`value`](Self::value) is between `min` and `max`, returns it.
    ///
    /// If the value is fuzzily equal to `min` or `max`, it is clamped to that
    /// bound. Otherwise this returns a script error. If this came from a
    /// function argument, `name` is the argument name (without `$`) used for
    /// error reporting.
    pub fn value_in_range(&self, min: f64, max: f64, name: Option<&str>) -> SassResult<f64> {
        if let Some(v) = number::fuzzy_check_range(self.value, min, max) {
            return Ok(v);
        }
        let s = self.to_display_string()?;
        let unit_str = self.unit_string();
        let msg = format!("Expected {s} to be within {min}{unit_str} and {max}{unit_str}.");
        Err(Box::new(SassError::Script {
            message: msg,
            argument_name: name.map(|s| s.to_string()),
        }))
    }

    // Like `value_in_range`, but with an explicit unit for the expected upper
    // and lower bounds. Exists for a clearer error message where unitless
    // values are required.
    //
    // Matches Dart: SassNumber.valueInRangeWithUnit (number.dart).
    pub fn value_in_range_with_unit(
        &self,
        min: f64,
        max: f64,
        name: &str,
        unit: &str,
    ) -> SassResult<f64> {
        if let Some(v) = number::fuzzy_check_range(self.value, min, max) {
            return Ok(v);
        }
        let s = self.to_display_string()?;
        let msg = format!("Expected {s} to be within {min}{unit} and {max}{unit}.");
        Err(Box::new(SassError::Script {
            message: msg,
            argument_name: Some(name.to_string()),
        }))
    }

    // Returns whether `self` has units that are possibly-compatible with
    // `other`, per the Sass spec.
    //
    // Unitless numbers are only possibly-compatible with other unitless
    // numbers. Complex units fall back to comparability (Dart's `Complex`
    // override throws instead); single units consult the known
    // browser-compatibility sets, where an unknown unit is assumed compatible
    // with everything.
    //
    // Matches Dart: SassNumber.hasPossiblyCompatibleUnits (number.dart).
    pub fn has_possibly_compatible_units(&self, other: &SassNumber) -> bool {
        if !self.has_units() {
            return !other.has_units();
        }
        if !other.has_units() {
            return false;
        }
        if self.has_complex_units() || other.has_complex_units() {
            return self.is_comparable_to(other);
        }
        let my_unit = self.numerator_units[0].to_lowercase();
        let other_unit = other.numerator_units[0].to_lowercase();
        let my_set = known_compatibility_set(&my_unit);
        match my_set {
            Some(set) => {
                let other_known = known_compatibility_set(&other_unit).is_some();
                set.contains(other_unit.as_str()) || !other_known
            }
            None => true,
        }
    }

    /// Returns a copy of this number, converted to the units represented by `new_numerators` and `new_denominators`.
    ///
    /// Note that [`convert_value`](Self::convert_value) is generally more
    /// efficient if the value is going to be accessed directly. Throws a
    /// script error if this number's units aren't compatible, or if either
    /// side is unitless but the other is not. If this came from a function
    /// argument, `name` is the argument name (without `$`) used for error
    /// reporting.
    pub fn convert(
        &self,
        new_numerators: &[String],
        new_denominators: &[String],
        name: Option<&str>,
    ) -> SassResult<SassNumber> {
        let val = self.convert_value(new_numerators, new_denominators, name)?;
        Ok(SassNumber::with_units(
            val,
            new_numerators.to_vec(),
            new_denominators.to_vec(),
        ))
    }

    /// Returns a copy of this number, converted to the units represented by `new_numerators` and `new_denominators`.
    ///
    /// This does *not* throw an error if this number is unitless and the new
    /// units are not empty, or vice versa. Instead, it treats all unitless
    /// numbers as convertible to and from all units without changing the
    /// value. Note that [`coerce_value`](Self::coerce_value) is generally more
    /// efficient if the value is going to be accessed directly. Throws a
    /// script error if this number's units aren't compatible. If this came
    /// from a function argument, `name` is the argument name (without `$`)
    /// used for error reporting.
    pub fn coerce(
        &self,
        new_numerators: &[String],
        new_denominators: &[String],
        name: Option<&str>,
    ) -> SassResult<SassNumber> {
        if !self.has_units() {
            return Ok(SassNumber::with_units(
                self.value,
                new_numerators.to_vec(),
                new_denominators.to_vec(),
            ));
        }
        let val = self.coerce_value(new_numerators, new_denominators, name)?;
        Ok(SassNumber::with_units(
            val,
            new_numerators.to_vec(),
            new_denominators.to_vec(),
        ))
    }

    /// Returns a copy of this number, converted to the same units as `other`.
    ///
    /// Unlike [`convert_to_match`](Self::convert_to_match), this does *not*
    /// throw an error if this number is unitless and `other` is not, or vice
    /// versa. Instead, it treats all unitless numbers as convertible to and
    /// from all units without changing the value. Note that
    /// [`coerce_value_to_match`](Self::coerce_value_to_match) is generally
    /// more efficient if the value is going to be accessed directly. Throws a
    /// script error if this number's units aren't compatible with `other`'s
    /// units. `name`/`other_name` are the Sass argument names for error
    /// reporting.
    pub fn coerce_to_match(
        &self,
        other: &SassNumber,
        name: Option<&str>,
        other_name: Option<&str>,
    ) -> SassResult<SassNumber> {
        if !self.has_units() {
            return Ok(other.with_value(self.value));
        }
        let val = self.coerce_or_convert_value(
            &other.numerator_units,
            &other.denominator_units,
            true,
            name,
            Some(other),
            other_name,
        )?;
        Ok(SassNumber::with_units(
            val,
            other.numerator_units.clone(),
            other.denominator_units.clone(),
        ))
    }

    /// Returns a copy of this number, converted to the same units as `other`.
    ///
    /// Note that [`convert_value_to_match`](Self::convert_value_to_match) is
    /// generally more efficient if the value is going to be accessed directly.
    /// Throws a script error if this number's units aren't compatible with
    /// `other`'s units, or if either number is unitless but the other is not.
    /// `name`/`other_name` are the Sass argument names for error reporting.
    pub fn convert_to_match(
        &self,
        other: &SassNumber,
        name: Option<&str>,
        other_name: Option<&str>,
    ) -> SassResult<SassNumber> {
        if !self.has_units() && !other.has_units() {
            return Ok(self.clone());
        }
        let val = self.coerce_or_convert_value(
            &other.numerator_units,
            &other.denominator_units,
            false,
            name,
            Some(other),
            other_name,
        )?;
        Ok(SassNumber::with_units(
            val,
            other.numerator_units.clone(),
            other.denominator_units.clone(),
        ))
    }

    /// Returns [`value`](Self::value), converted to the units represented by `new_numerators` and `new_denominators`.
    ///
    /// This does *not* throw an error if this number is unitless and the new
    /// units are not empty, or vice versa. Instead, it treats all unitless
    /// numbers as convertible to and from all units without changing the
    /// value. Throws a script error if this number's units aren't compatible.
    /// If this came from a function argument, `name` is the argument name
    /// (without `$`) used for error reporting.
    pub fn coerce_value(
        &self,
        new_numerators: &[String],
        new_denominators: &[String],
        name: Option<&str>,
    ) -> SassResult<f64> {
        if !self.has_units() {
            return Ok(self.value);
        }
        self.coerce_or_convert_value(new_numerators, new_denominators, true, name, None, None)
    }

    /// Returns [`value`](Self::value), converted to the units represented by `new_numerators` and `new_denominators`.
    ///
    /// Throws a script error if this number's units aren't compatible or if
    /// this number is unitless. If this came from a function argument, `name`
    /// is the argument name (without `$`) used for error reporting.
    pub fn convert_value(
        &self,
        new_numerators: &[String],
        new_denominators: &[String],
        name: Option<&str>,
    ) -> SassResult<f64> {
        if !self.has_units() && new_numerators.is_empty() && new_denominators.is_empty() {
            return Ok(self.value);
        }
        self.coerce_or_convert_value(new_numerators, new_denominators, false, name, None, None)
    }

    // Returns a new number equivalent to `n` with `self`'s units multiplied by
    // `other_numerators`/`other_denominators`, cancelling convertible pairs.
    //
    // Matches Dart: SassNumber.multiplyUnits (number.dart).
    pub fn multiply_units(
        &self,
        n: f64,
        other_numerators: &[String],
        other_denominators: &[String],
    ) -> SassNumber {
        if other_numerators.is_empty() && other_denominators.is_empty() {
            return SassNumber::with_units(
                n,
                self.numerator_units.clone(),
                self.denominator_units.clone(),
            );
        }
        if self.numerator_units.is_empty() && self.denominator_units.is_empty() {
            return SassNumber::with_units(
                n,
                other_numerators.to_vec(),
                other_denominators.to_vec(),
            );
        }
        if self.numerator_units.is_empty()
            && other_denominators.is_empty()
            && !units_are_convertible(&self.denominator_units, other_numerators)
        {
            return SassNumber::with_units(
                n,
                other_numerators.to_vec(),
                self.denominator_units.clone(),
            );
        }
        if self.denominator_units.is_empty()
            && other_numerators.is_empty()
            && !units_are_convertible(&self.numerator_units, other_denominators)
        {
            return SassNumber::with_units(
                n,
                self.numerator_units.clone(),
                other_denominators.to_vec(),
            );
        }
        let mut val = n;
        let mut mutable_other_den: Vec<String> = other_denominators.to_vec();
        let mut new_numerators: Vec<String> = vec![];

        for num in &self.numerator_units {
            let mut found = false;
            for i in 0..mutable_other_den.len() {
                if let Some(factor) = conversion_factor(num, &mutable_other_den[i]) {
                    val /= factor;
                    mutable_other_den.remove(i);
                    found = true;
                    break;
                }
            }
            if !found {
                new_numerators.push(num.clone());
            }
        }

        let mut mutable_den: Vec<String> = self.denominator_units.clone();
        for num in other_numerators {
            let mut found = false;
            for i in 0..mutable_den.len() {
                if let Some(factor) = conversion_factor(num, &mutable_den[i]) {
                    val /= factor;
                    mutable_den.remove(i);
                    found = true;
                    break;
                }
            }
            if !found {
                new_numerators.push(num.clone());
            }
        }

        mutable_den.extend(mutable_other_den);
        SassNumber::with_units(val, new_numerators, mutable_den)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        common::exception::SassError,
        value::{Value, ValueKind},
    };
    use bumpalo::Bump;

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
    fn test_new_unitless() {
        let n = SassNumber::new(42.0, None);
        assert!(!n.has_units());
        assert!(!n.has_complex_units());
        assert_eq!(n.value, 42.0);
    }

    #[test]
    fn test_new_single_unit() {
        let n = SassNumber::new(10.0, Some("px"));
        assert!(n.has_units());
        assert!(!n.has_complex_units());
        assert_eq!(n.numerator_units, vec!["px".to_string()]);
    }

    #[test]
    fn test_new_complex() {
        let n = SassNumber::with_units(10.0, vec!["m".to_string()], vec!["s".to_string()]);
        assert!(n.has_units());
        assert!(n.has_complex_units());
    }

    #[test]
    fn test_with_units_simplify() {
        let n = SassNumber::with_units(
            1.0,
            vec!["px".to_string(), "in".to_string()],
            vec!["px".to_string()],
        );
        assert!(!n.has_complex_units());
        assert!(n.has_unit("in") || n.numerator_units.contains(&"in".to_string()));
    }

    #[test]
    fn test_is_int() {
        assert!(SassNumber::new(5.0, None).is_int());
        assert!(!SassNumber::new(5.5, None).is_int());
    }

    #[test]
    fn test_as_int() {
        assert_eq!(SassNumber::new(5.0, None).as_int(), Some(5));
        assert_eq!(SassNumber::new(5.5, None).as_int(), None);
    }

    #[test]
    fn test_assert_int() {
        assert!(SassNumber::new(5.0, None).assert_int(None).is_ok());
        assert!(SassNumber::new(5.5, None).assert_int(None).is_err());
    }

    #[test]
    fn test_has_unit() {
        assert!(SassNumber::new(5.0, Some("px")).has_unit("px"));
        assert!(!SassNumber::new(5.0, None).has_unit("px"));
    }

    #[test]
    fn test_assert_unit() {
        assert!(SassNumber::new(5.0, Some("px"))
            .assert_unit("px", None)
            .is_ok());
        assert!(SassNumber::new(5.0, None).assert_unit("px", None).is_err());
    }

    #[test]
    fn test_assert_no_units() {
        assert!(SassNumber::new(5.0, None).assert_no_units(None).is_ok());
        assert!(SassNumber::new(5.0, Some("px"))
            .assert_no_units(None)
            .is_err());
    }

    #[test]
    fn test_unit_string() {
        assert_eq!(SassNumber::new(5.0, None).unit_string(), "");
        assert_eq!(SassNumber::new(5.0, Some("px")).unit_string(), "px");
    }

    #[test]
    fn test_has_compatible_units() {
        let a = SassNumber::new(5.0, Some("px"));
        let b = SassNumber::new(10.0, Some("px"));
        assert!(a.has_compatible_units(&b));
        let c = SassNumber::new(1.0, Some("in"));
        assert!(a.has_compatible_units(&c));
        let d = SassNumber::new(90.0, Some("deg"));
        assert!(!a.has_compatible_units(&d));
    }

    #[test]
    fn test_compatible_with_unit() {
        assert!(SassNumber::new(5.0, None).compatible_with_unit("px"));
        assert!(SassNumber::new(5.0, Some("px")).compatible_with_unit("in"));
        assert!(!SassNumber::new(5.0, Some("px")).compatible_with_unit("deg"));
    }

    #[test]
    fn test_convert_value_to_unit() {
        let n = SassNumber::new(1.0, Some("in"));
        let v = n.convert_value_to_unit("px", None).unwrap();
        assert!(approx(v, 96.0), "1in should be 96px, got {}", v);
    }

    #[test]
    fn test_coerce_value_to_unit() {
        let v = SassNumber::new(1.0, Some("in"))
            .coerce_value_to_unit("px", None)
            .unwrap();
        assert!(approx(v, 96.0));
        let v = SassNumber::new(5.0, None)
            .coerce_value_to_unit("px", None)
            .unwrap();
        assert!(approx(v, 5.0));
    }

    #[test]
    fn test_convert_value_to_unit_unitless_fails() {
        let n = SassNumber::new(5.0, None);
        assert!(n.convert_value_to_unit("px", None).is_err());
    }

    #[test]
    fn test_slash() {
        let n = SassNumber::new(5.0, None)
            .with_slash(SassNumber::new(2.0, None), SassNumber::new(3.0, None));
        assert!(n.has_slash());
        let (a, b) = n.slash_pair().unwrap();
        assert!(approx(a.value, 2.0));
        assert!(approx(b.value, 3.0));
        let without = n.without_slash();
        assert!(!without.has_slash());
    }

    #[test]
    fn test_with_value() {
        let n = SassNumber::new(5.0, Some("px")).with_value(10.0);
        assert!(approx(n.value, 10.0));
        assert!(n.has_unit("px"));
    }

    #[test]
    fn test_with_units() {
        let n = SassNumber::new(5.0, Some("px")).with_units_vec(vec!["cm".to_string()], vec![]);
        assert!(n.has_unit("cm"));
        assert!(approx(n.value, 5.0));
    }

    #[test]
    fn test_unit_suggestion() {
        let s = SassNumber::new(1.0, None).unit_suggestion("foo", None);
        assert!(!s.is_empty());
        assert!(s.contains("foo"));
    }

    #[test]
    fn test_number_equals() {
        let a = SassNumber::new(5.0, Some("px"));
        let b = SassNumber::new(5.0, Some("px"));
        assert!(a.equals(&b));
        let c = SassNumber::new(96.0, Some("px"));
        let d = SassNumber::new(1.0, Some("in"));
        assert!(c.equals(&d), "96px should equal 1in");
    }

    #[test]
    fn test_number_hash_code() {
        let a = SassNumber::new(96.0, Some("px"));
        let b = SassNumber::new(1.0, Some("in"));
        assert_eq!(a.hash_code(), b.hash_code());
    }

    #[test]
    fn test_khz_dppx_equality_and_hash() {
        // Dart canonicalMultiplierForUnit: 1 / innerMap.values.first, so
        // kHz→1000, dpcm→2.54, dppx→96. Previously stored inverted.
        let a = SassNumber::new(1.0, Some("kHz"));
        let b = SassNumber::new(1000.0, Some("Hz"));
        assert!(a.equals(&b), "1kHz should equal 1000Hz");
        assert_eq!(a.hash_code(), b.hash_code());
        let c = SassNumber::new(1.0, Some("dppx"));
        let d = SassNumber::new(96.0, Some("dpi"));
        assert!(c.equals(&d), "1dppx should equal 96dpi");
        assert_eq!(c.hash_code(), d.hash_code());
        let e = SassNumber::new(1.0, Some("dpcm"));
        let f = SassNumber::new(2.54, Some("dpi"));
        assert!(e.equals(&f), "1dpcm should equal 2.54dpi");
        assert_eq!(e.hash_code(), f.hash_code());
    }

    #[test]
    fn test_plus_unitless() {
        let a = SassNumber::new(5.0, None);
        let b = SassNumber::new(3.0, None);
        let r = a.plus_num(&b).unwrap();
        assert!(approx(r.value, 8.0));
        assert!(!r.has_units());
    }

    #[test]
    fn test_plus_unitful() {
        let a = SassNumber::new(5.0, Some("px"));
        let b = SassNumber::new(3.0, Some("px"));
        let r = a.plus_num(&b).unwrap();
        assert!(approx(r.value, 8.0));
        assert!(r.has_unit("px"));
    }

    #[test]
    fn test_plus_unitless_and_unitful() {
        let a = SassNumber::new(5.0, None);
        let b = SassNumber::new(10.0, Some("px"));
        let r = a.plus_num(&b).unwrap();
        assert!(approx(r.value, 15.0));
        assert!(r.has_unit("px"));
    }

    #[test]
    fn test_plus_incompatible_units() {
        let a = SassNumber::new(5.0, Some("px"));
        let b = SassNumber::new(3.0, Some("deg"));
        assert!(a.plus_num(&b).is_err());
    }

    #[test]
    fn test_times() {
        let a = SassNumber::new(5.0, Some("px"));
        let b = SassNumber::new(3.0, None);
        let r = a.times_num(&b).unwrap();
        assert!(approx(r.value, 15.0));
        assert!(r.has_unit("px"));
    }

    #[test]
    fn test_times_unitful() {
        let a = SassNumber::new(2.0, Some("in"));
        let b = SassNumber::new(3.0, Some("px"));
        let r = a.times_num(&b).unwrap();
        assert!(approx(r.value, 6.0));
    }

    #[test]
    fn test_divided_by() {
        let a = SassNumber::new(10.0, Some("px"));
        let b = SassNumber::new(2.0, None);
        let r = a.divided_by_num(&b).unwrap();
        assert!(approx(r.value, 5.0));
        assert!(r.has_unit("px"));
    }

    #[test]
    fn test_divided_by_unitful() {
        let a = SassNumber::new(10.0, Some("px"));
        let b = SassNumber::new(5.0, Some("px"));
        let r = a.divided_by_num(&b).unwrap();
        assert!(approx(r.value, 2.0));
    }

    #[test]
    fn test_minus() {
        let a = SassNumber::new(5.0, Some("px"));
        let b = SassNumber::new(3.0, Some("px"));
        let r = a.minus_num(&b).unwrap();
        assert!(approx(r.value, 2.0));
        assert!(r.has_unit("px"));
    }

    #[test]
    fn test_modulo() {
        let a = SassNumber::new(7.0, Some("px"));
        let b = SassNumber::new(3.0, Some("px"));
        let r = a.modulo_num(&b).unwrap();
        assert!(approx(r.value, 1.0));
    }

    #[test]
    fn test_modulo_negative() {
        let a = SassNumber::new(-7.0, Some("px"));
        let b = SassNumber::new(3.0, Some("px"));
        let r = a.modulo_num(&b).unwrap();
        assert!(approx(r.value, 2.0));
    }

    #[test]
    fn test_unary_minus() {
        let n = SassNumber::new(5.0, Some("px")).unary_minus_num();
        assert!(approx(n.value, -5.0));
        assert!(n.has_unit("px"));
    }

    #[test]
    fn test_greater_than() {
        let a = SassNumber::new(5.0, Some("px"));
        let b = SassNumber::new(3.0, Some("px"));
        assert!(a.greater_than_num(&b).unwrap());
        assert!(!b.greater_than_num(&a).unwrap());
    }

    #[test]
    fn test_greater_than_equal() {
        let a = SassNumber::new(96.0, Some("px"));
        let b = SassNumber::new(1.0, Some("in"));
        assert!(!a.greater_than_num(&b).unwrap());
    }

    #[test]
    fn test_multiply_units() {
        let a = SassNumber::new(2.0, Some("in"));
        let r = a.multiply_units(6.0, &["px".to_string()], &[]);
        assert!(approx(r.value, 6.0));
        assert_eq!(r.numerator_units.len(), 2);
    }

    #[test]
    fn test_number_to_css_string() {
        let arena = Bump::new();
        let v = ValueKind::unitless_number(&arena, 42.0);
        assert_eq!(v.to_css_string(true).unwrap(), "42");
    }

    #[test]
    fn test_number_to_css_string_slash() {
        let arena = Bump::new();
        let slash = SassNumber {
            value: 1.0,
            numerator_units: vec![],
            denominator_units: vec![],
            as_slash: Some(Box::new((
                SassNumber::new(1.0, None),
                SassNumber::new(2.0, None),
            ))),
        };
        let v = Value::new_with_arena(&arena, ValueKind::Number(slash));
        assert_eq!(v.to_css_string(true).unwrap(), "1/2");
    }

    #[test]
    fn test_number_to_string() {
        let arena = Bump::new();
        let v = ValueKind::unitless_number(&arena, 42.0);
        assert_eq!(v.to_display_string().unwrap(), "42");
    }

    #[test]
    fn test_value_in_range() {
        let n = SassNumber::new(5.0, None);
        let v = n.value_in_range(0.0, 10.0, Some("test")).unwrap();
        assert!(approx(v, 5.0));
        assert!(n.value_in_range(10.0, 20.0, Some("test")).is_err());
    }

    #[test]
    fn test_value_in_range_with_unit() {
        let n = SassNumber::new(30.0, Some("deg"));
        let v = n
            .value_in_range_with_unit(0.0, 360.0, "test", "deg")
            .unwrap();
        assert!(approx(v, 30.0));
        assert!(n
            .value_in_range_with_unit(0.0, 10.0, "test", "deg")
            .is_err());
    }

    #[test]
    fn test_has_possibly_compatible_units() {
        assert!(
            SassNumber::new(5.0, None).has_possibly_compatible_units(&SassNumber::new(10.0, None))
        );
        assert!(!SassNumber::new(5.0, None)
            .has_possibly_compatible_units(&SassNumber::new(10.0, Some("px"))));
        assert!(SassNumber::new(5.0, Some("px"))
            .has_possibly_compatible_units(&SassNumber::new(10.0, Some("px"))));
        assert!(SassNumber::new(5.0, Some("px"))
            .has_possibly_compatible_units(&SassNumber::new(10.0, Some("in"))));
        assert!(SassNumber::new(5.0, Some("px"))
            .has_possibly_compatible_units(&SassNumber::new(10.0, Some("em"))));
        assert!(!SassNumber::new(5.0, Some("px"))
            .has_possibly_compatible_units(&SassNumber::new(10.0, Some("deg"))));
        assert!(SassNumber::new(5.0, Some("deg"))
            .has_possibly_compatible_units(&SassNumber::new(10.0, Some("rad"))));
        assert!(SassNumber::new(5.0, Some("s"))
            .has_possibly_compatible_units(&SassNumber::new(10.0, Some("ms"))));
        assert!(SassNumber::new(5.0, Some("dpi"))
            .has_possibly_compatible_units(&SassNumber::new(10.0, Some("dppx"))));
        assert!(SassNumber::new(5.0, Some("custom"))
            .has_possibly_compatible_units(&SassNumber::new(10.0, Some("px"))));
        assert!(SassNumber::new(5.0, Some("custom1"))
            .has_possibly_compatible_units(&SassNumber::new(10.0, Some("custom2"))));
    }

    #[test]
    fn test_convert_sass_number() {
        let n = SassNumber::new(1.0, Some("in"));
        let result = n.convert(&["px".to_string()], &[], Some("test")).unwrap();
        assert!(approx(result.value, 96.0));
        assert!(result.has_unit("px"));
    }

    #[test]
    fn test_coerce_sass_number() {
        let n = SassNumber::new(5.0, None);
        let result = n.coerce(&["px".to_string()], &[], Some("test")).unwrap();
        assert!(approx(result.value, 5.0));
        assert!(result.has_unit("px"));

        let m = SassNumber::new(1.0, Some("in"));
        let result = m.coerce(&["px".to_string()], &[], Some("test")).unwrap();
        assert!(approx(result.value, 96.0));
    }

    #[test]
    fn test_coerce_to_match() {
        let a = SassNumber::new(5.0, None);
        let b = SassNumber::new(10.0, Some("px"));
        let result = a.coerce_to_match(&b, Some("a"), Some("b")).unwrap();
        assert!(approx(result.value, 5.0));
        assert!(result.has_unit("px"));

        let c = SassNumber::new(1.0, Some("in"));
        let result = c.coerce_to_match(&b, Some("c"), Some("b")).unwrap();
        assert!(approx(result.value, 96.0));
    }

    #[test]
    fn test_convert_to_match() {
        let a = SassNumber::new(1.0, Some("in"));
        let b = SassNumber::new(2.0, Some("px"));
        let result = a.convert_to_match(&b, Some("a"), Some("b")).unwrap();
        assert!(approx(result.value, 96.0));
        assert!(result.has_unit("px"));

        let c = SassNumber::new(5.0, None);
        assert!(c.convert_to_match(&b, Some("c"), Some("b")).is_err());
    }

    #[test]
    fn test_coerce_value() {
        let n = SassNumber::new(5.0, None);
        let v = n
            .coerce_value(&["px".to_string()], &[], Some("test"))
            .unwrap();
        assert!(approx(v, 5.0));

        let m = SassNumber::new(1.0, Some("in"));
        let v = m
            .coerce_value(&["px".to_string()], &[], Some("test"))
            .unwrap();
        assert!(approx(v, 96.0));
    }

    #[test]
    fn test_convert_value() {
        let n = SassNumber::new(5.0, None);
        let v = n.convert_value(&[], &[], Some("test")).unwrap();
        assert!(approx(v, 5.0));

        let m = SassNumber::new(1.0, Some("in"));
        let v = m
            .convert_value(&["px".to_string()], &[], Some("test"))
            .unwrap();
        assert!(approx(v, 96.0));

        assert!(n
            .convert_value(&["px".to_string()], &[], Some("test"))
            .is_err());
    }

    // --- Exact error message tests (locked in Go: number_test.go) ---

    #[test]
    fn test_assert_int_error_message() {
        let err = SassNumber::new(2.5, None)
            .assert_int(Some("limit"))
            .unwrap_err();
        assert_script_err(err, "2.5 is not an int.", Some("limit"));

        let err = SassNumber::new(2.5, Some("px"))
            .assert_int(None)
            .unwrap_err();
        assert_script_err(err, "2.5px is not an int.", None);
    }

    #[test]
    fn test_assert_no_units_error_message() {
        let err = SassNumber::new(5.0, Some("px"))
            .assert_no_units(Some("number"))
            .unwrap_err();
        assert_script_err(err, "Expected 5px to have no units.", Some("number"));

        let err = SassNumber::new(5.0, Some("px"))
            .assert_no_units(None)
            .unwrap_err();
        assert_script_err(err, "Expected 5px to have no units.", None);
    }

    #[test]
    fn test_assert_unit_error_message() {
        let err = SassNumber::new(5.0, None)
            .assert_unit("px", Some("length"))
            .unwrap_err();
        assert_script_err(err, "Expected 5 to have unit \"px\".", Some("length"));
    }

    #[test]
    fn test_value_in_range_error_message() {
        let err = SassNumber::new(150.0, None)
            .value_in_range(0.0, 100.0, Some("amount"))
            .unwrap_err();
        assert_script_err(err, "Expected 150 to be within 0 and 100.", Some("amount"));

        let err = SassNumber::new(150.0, Some("px"))
            .value_in_range(0.0, 100.0, None)
            .unwrap_err();
        assert_script_err(err, "Expected 150px to be within 0px and 100px.", None);
    }

    #[test]
    fn test_value_in_range_with_unit_error_message() {
        let err = SassNumber::new(150.0, None)
            .value_in_range_with_unit(0.0, 100.0, "alpha", "%")
            .unwrap_err();
        assert_script_err(err, "Expected 150 to be within 0% and 100%.", Some("alpha"));
    }

    #[test]
    fn test_convert_value_to_match_error_messages() {
        // Incompatible units with names.
        let err = SassNumber::new(1.0, Some("s"))
            .convert_value_to_match(&SassNumber::new(1.0, Some("px")), Some("x"), Some("y"))
            .unwrap_err();
        assert_script_err(err, "1s and $y: 1px have incompatible units.", Some("x"));

        // One unitless.
        let err = SassNumber::new(1.0, None)
            .convert_value_to_match(&SassNumber::new(1.0, Some("px")), Some("x"), Some("y"))
            .unwrap_err();
        assert_script_err(
            err,
            "1 and $y: 1px have incompatible units (one has units and the other doesn't).",
            Some("x"),
        );

        // No names.
        let err = SassNumber::new(1.0, Some("px"))
            .convert_value_to_match(&SassNumber::new(2.0, Some("s")), None, None)
            .unwrap_err();
        assert_script_err(err, "1px and 2s have incompatible units.", None);
    }

    #[test]
    fn test_coerce_value_to_unit_error_messages() {
        // Known unit type: lists the compatible units.
        let err = SassNumber::new(10.0, Some("px"))
            .coerce_value_to_unit("rad", Some("number"))
            .unwrap_err();
        assert_script_err(
            err,
            "Expected 10px to have an angle unit (deg, grad, rad, turn).",
            Some("number"),
        );

        // Unknown unit.
        let err = SassNumber::new(5.0, Some("px"))
            .coerce_value_to_unit("foo", Some("x"))
            .unwrap_err();
        assert_script_err(err, "Expected 5px to have unit foo.", Some("x"));
    }

    #[test]
    fn test_convert_value_error_messages() {
        // Multiple units.
        let err = SassNumber::new(5.0, Some("px"))
            .convert_value(&["px".to_string(), "s".to_string()], &[], None)
            .unwrap_err();
        assert_script_err(err, "Expected 5px to have units px*s.", None);

        // Unitful to unitless.
        let err = SassNumber::new(5.0, Some("px"))
            .convert_value(&[], &[], None)
            .unwrap_err();
        assert_script_err(err, "Expected 5px to have no units.", None);
    }

    #[test]
    fn test_coerce_units_error_picks_self_coercion_error() {
        // Matches Go numCoerceUnits: when both coercions fail, the error from
        // n.CoerceValueToMatch(other) wins — the receiver appears first.
        let a = SassNumber::new(1.0, Some("px"));
        let b = SassNumber::new(2.0, Some("s"));
        let err = a.less_than_num(&b).unwrap_err();
        assert_script_err(err, "1px and 2s have incompatible units.", None);
    }

    #[test]
    fn test_number_to_display_string() {
        assert_eq!(
            SassNumber::new(12.0, Some("px"))
                .to_display_string()
                .unwrap(),
            "12px"
        );
        assert_eq!(
            SassNumber::new(2.5, None).to_display_string().unwrap(),
            "2.5"
        );
        assert_eq!(
            SassNumber::with_units(5.0, vec!["px".into()], vec!["s".into()])
                .to_display_string()
                .unwrap(),
            "calc(5px / 1s)"
        );
    }
}
