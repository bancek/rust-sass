// Copyright 2021 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/calculation.dart
// go-source: go/value/calculation.go

use crate::common::SassError;
use crate::logger::NoOpWarnLogger;
use crate::math::abs;
use crate::math::cos;
use crate::math::sin;
use crate::math::sqrt;
use crate::math::tan;
use crate::value::ListSeparator;
use crate::value::Value;
use bumpalo::Bump;
use std::cell::Cell;
use std::f64::consts::E;
use std::f64::consts::PI;
use std::fmt;

use crate::common::file_span::FileSpan;
use crate::common::SassResult;
use crate::deprecation;
use crate::logger::WarnLogger;
use crate::util::number;
use crate::value::ValueKind;

use crate::serialize::SerializeVisitor;
use crate::value::hash::{hash_combine, string_hash_code};
use crate::value::number_math;
use crate::value::{SassNumber, ValueVisitor};

/// A SassScript calculation.
///
/// Although calculations can in principle have any name or any number of
/// arguments, only the specific calculations supported by the Sass spec are
/// exposed through the constructors below. This ensures that every calculation
/// a caller works with is always fully simplified.
#[derive(Clone, Debug)]
pub struct SassCalculation {
    /// The calculation's name, such as `"calc"`.
    pub name: String,
    /// The calculation's arguments.
    ///
    /// Each argument is either a [`SassNumber`][super::SassNumber], a
    /// [`SassCalculation`], an unquoted string, or a [`CalculationOperation`].
    pub arguments: Vec<CalcArgument>,
    cached_hash: Cell<Option<usize>>,
}

/// A single argument that may appear inside a [`SassCalculation`].
///
/// Mirrors the `Object` union in Dart: a number, a nested calculation, an
/// (un)quoted string, an operation, or interpolated text.
#[derive(Clone, Debug)]
pub enum CalcArgument {
    /// A numeric argument.
    Number(SassNumber),
    /// A nested calculation argument.
    Calculation(Box<SassCalculation>),
    /// A string argument; the `bool` records whether it was quoted.
    /// Only unquoted strings are valid in a simplified calculation.
    String(String, bool),
    /// A binary operation argument.
    Operation(Box<CalculationOperation>),
    /// Interpolated text injected into the calculation.
    Interpolation(String),
}

/// A binary operation that can appear in a [`SassCalculation`].
#[derive(Clone, Debug)]
pub struct CalculationOperation {
    /// The operator.
    pub operator: CalculationOperator,
    /// The left-hand operand: a number, calculation, unquoted string, or operation.
    pub left: CalcArgument,
    /// The right-hand operand: a number, calculation, unquoted string, or operation.
    pub right: CalcArgument,
    cached_hash: Cell<Option<usize>>,
}

/// An enumeration of possible operators for [`CalculationOperation`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CalculationOperator {
    /// The addition operator.
    Plus,
    /// The subtraction operator.
    Minus,
    /// The multiplication operator.
    Times,
    /// The division operator.
    DividedBy,
}

/// A deprecated representation of a string injected into a [`SassCalculation`]
/// using interpolation.
///
/// This only exists for backwards-compatibility with an older version of Dart
/// Sass. It is now equivalent to creating an unquoted string whose value is
/// wrapped in parentheses.
///
/// Deprecated: prefer an unquoted string instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalculationInterpolation {
    /// The interpolated text.
    pub value: String,
}

// ----- CalculationOperator methods -----

impl CalculationOperator {
    /// The English name of `self`, such as `"divided by"`.
    pub fn name(&self) -> &'static str {
        match self {
            CalculationOperator::Plus => "plus",
            CalculationOperator::Minus => "minus",
            CalculationOperator::Times => "times",
            CalculationOperator::DividedBy => "divided by",
        }
    }

    /// The CSS syntax for `self`, such as `"+"` or `"/"`.
    pub fn operator_str(&self) -> &'static str {
        match self {
            CalculationOperator::Plus => "+",
            CalculationOperator::Minus => "-",
            CalculationOperator::Times => "*",
            CalculationOperator::DividedBy => "/",
        }
    }

    // The precedence of `self`. An operator with higher precedence binds
    // tighter. Matches Dart: internal-only in Dart, so plain comment here.
    pub fn precedence(&self) -> u32 {
        match self {
            CalculationOperator::Plus | CalculationOperator::Minus => 1,
            CalculationOperator::Times | CalculationOperator::DividedBy => 2,
        }
    }
}

impl fmt::Display for CalculationOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ----- CalcArgument methods -----

impl CalcArgument {
    fn hash_code(&self) -> usize {
        match self {
            CalcArgument::Number(n) => n.hash_code() as usize,
            CalcArgument::Calculation(c) => c.hash_code(),
            CalcArgument::String(s, _) => string_hash_code(s) as usize,
            CalcArgument::Operation(op) => op.hash_code(),
            CalcArgument::Interpolation(s) => string_hash_code(s) as usize,
        }
    }

    fn equals(&self, other: &CalcArgument) -> bool {
        match (self, other) {
            (CalcArgument::Number(a), CalcArgument::Number(b)) => a.equals(b),
            (CalcArgument::Calculation(a), CalcArgument::Calculation(b)) => a.equals(b),
            (CalcArgument::String(a, qa), CalcArgument::String(b, qb)) => a == b && qa == qb,
            (CalcArgument::Operation(a), CalcArgument::Operation(b)) => a.equals(b),
            _ => false,
        }
    }
}

// ----- SassCalculation methods -----

impl SassCalculation {
    // Creates a calculation with the given `name` and `arguments` without
    // simplifying. Internal-only in Dart (`@internal unsimplified`).
    pub fn new_unsimplified(name: &str, arguments: Vec<CalcArgument>) -> Self {
        SassCalculation {
            name: name.to_string(),
            arguments,
            cached_hash: Cell::new(None),
        }
    }

    /// Serializes this calculation as CSS, quoting strings when `quote` holds.
    pub fn to_css_string(&self, quote: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(quote, false);
        visitor.visit_calculation(self)?;
        Ok(visitor.into_string())
    }

    /// Serializes this calculation in inspect (debug) form.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(true, true);
        visitor.visit_calculation(self)?;
        Ok(visitor.into_string())
    }

    // Matches Dart: internal `isSpecialNumber` (calculations count as numbers
    // for slash-separation and function-argument checks).
    pub fn is_special_number(&self) -> bool {
        true
    }

    /// Calculations are always truthy.
    pub fn is_truthy(&self) -> bool {
        true
    }

    /// A calculation's list separator is always undecided.
    pub fn list_separator(&self) -> ListSeparator {
        ListSeparator::Undecided
    }

    /// Calculations never have brackets.
    pub fn has_brackets(&self) -> bool {
        false
    }

    /// A calculation counts as a single-element list.
    pub fn length_as_list(&self) -> usize {
        1
    }

    /// Calculations are never blank.
    pub fn is_blank(&self) -> bool {
        false
    }

    /// Combines the name hash with the argument-list hash.
    pub fn hash_code(&self) -> usize {
        if let Some(h) = self.cached_hash.get() {
            return h;
        }
        let args_hash = self
            .arguments
            .iter()
            .fold(0i32, |acc, arg| hash_combine(acc, arg.hash_code() as i32));
        let h = (string_hash_code(&self.name) ^ args_hash) as usize;
        self.cached_hash.set(Some(h));
        h
    }

    /// Whether `other` is a calculation with the same name and equal arguments.
    pub fn equals(&self, other: &SassCalculation) -> bool {
        if self.name != other.name || self.arguments.len() != other.arguments.len() {
            return false;
        }
        self.arguments
            .iter()
            .zip(other.arguments.iter())
            .all(|(a, b)| a.equals(b))
    }
}

// ----- CalculationOperation methods -----

impl CalculationOperation {
    /// Creates an operation without simplifying. Simplification happens in
    /// [`operate`] / [`operate_internal`] instead.
    pub fn new(operator: CalculationOperator, left: CalcArgument, right: CalcArgument) -> Self {
        CalculationOperation {
            operator,
            left,
            right,
            cached_hash: Cell::new(None),
        }
    }

    /// Serializes this operation by wrapping it in a nameless calculation and
    /// stripping the outer parentheses.
    pub fn to_display_string(&self) -> SassResult<String> {
        let parenthesized = SassCalculation::new_unsimplified(
            "",
            vec![CalcArgument::Operation(Box::new(self.clone()))],
        )
        .to_display_string()?;
        Ok(parenthesized[1..parenthesized.len() - 1].to_string())
    }

    /// Combines the operator and operand hashes.
    pub fn hash_code(&self) -> usize {
        if let Some(h) = self.cached_hash.get() {
            return h;
        }
        let h = (self.operator as usize) ^ self.left.hash_code() ^ self.right.hash_code();
        self.cached_hash.set(Some(h));
        h
    }

    /// Whether `other` has the same operator and equal operands.
    pub fn equals(&self, other: &CalculationOperation) -> bool {
        self.operator == other.operator
            && self.left.equals(&other.left)
            && self.right.equals(&other.right)
    }
}

// ----- CalculationInterpolation methods -----

impl CalculationInterpolation {
    /// Whether `other` wraps the same interpolated text.
    pub fn equals(&self, other: &CalculationInterpolation) -> bool {
        self.value == other.value
    }
}

// ===========================================================================
// Constructors
// ===========================================================================

/// Creates a `calc()` calculation with the given `argument`.
///
/// The argument must be a number, calculation, unquoted string, operation, or
/// interpolation. Automatically simplifies, so the returned value may be a
/// number rather than a calculation. Returns an `Err` if the calculation is
/// known to produce invalid CSS (for example a quoted string argument).
pub fn new_calc<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    argument: CalcArgument,
) -> SassResult<Value<'parse>> {
    let v = simplify(argument)?;
    if let CalcArgument::Number(num) = v {
        return Ok(Value::new_with_arena(arena, ValueKind::Number(num)));
    }
    if let CalcArgument::Calculation(calc) = v {
        return Ok(Value::new_with_arena(arena, ValueKind::Calculation(calc)));
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified("calc", vec![v]))),
    ))
}

/// Creates a `min()` calculation with the given `args` (at least one).
///
/// Each argument must be a number, calculation, unquoted string, operation, or
/// interpolation. Returns the smallest value directly when every argument is
/// a mutually comparable number; otherwise returns a calculation. Returns an
/// `Err` if the arguments are known to produce invalid CSS.
pub fn new_min<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    args: Vec<CalcArgument>,
) -> SassResult<Value<'parse>> {
    let simplified = simplify_arguments(args)?;
    if simplified.is_empty() {
        return Err(Box::new(SassError::Script {
            message: "min() must have at least one argument.".into(),
            argument_name: None,
        }));
    }
    let mut minimum: Option<SassNumber> = None;
    for arg in &simplified {
        if let CalcArgument::Number(num) = arg {
            if let Some(ref min) = minimum {
                if !min.is_comparable_to(num) {
                    minimum = None;
                    break;
                }
                let gt = min.greater_than_num(num)?;
                if gt {
                    minimum = Some(num.clone());
                }
            } else {
                minimum = Some(num.clone());
            }
        } else {
            minimum = None;
            break;
        }
    }
    if let Some(num) = minimum {
        return Ok(Value::new_with_arena(arena, ValueKind::Number(num)));
    }
    verify_compatible_numbers(&simplified)?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
            "min", simplified,
        ))),
    ))
}

/// Creates a `max()` calculation with the given `args` (at least one).
///
/// Each argument must be a number, calculation, unquoted string, operation, or
/// interpolation. Returns the largest value directly when every argument is
/// a mutually comparable number; otherwise returns a calculation. Returns an
/// `Err` if the arguments are known to produce invalid CSS.
pub fn new_max<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    args: Vec<CalcArgument>,
) -> SassResult<Value<'parse>> {
    let simplified = simplify_arguments(args)?;
    if simplified.is_empty() {
        return Err(Box::new(SassError::Script {
            message: "max() must have at least one argument.".into(),
            argument_name: None,
        }));
    }
    let mut maximum: Option<SassNumber> = None;
    for arg in &simplified {
        if let CalcArgument::Number(num) = arg {
            if let Some(ref max) = maximum {
                if !max.is_comparable_to(num) {
                    maximum = None;
                    break;
                }
                let lt = max.less_than_num(num)?;
                if lt {
                    maximum = Some(num.clone());
                }
            } else {
                maximum = Some(num.clone());
            }
        } else {
            maximum = None;
            break;
        }
    }
    if let Some(num) = maximum {
        return Ok(Value::new_with_arena(arena, ValueKind::Number(num)));
    }
    verify_compatible_numbers(&simplified)?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
            "max", simplified,
        ))),
    ))
}

/// Creates a `hypot()` calculation with the given `args` (at least one).
///
/// Returns the Euclidean length directly when every argument is a number with
/// units compatible with the first (and the first has no `%` unit);
/// otherwise returns an unsimplified `hypot` calculation.
pub fn new_hypot<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    args: Vec<CalcArgument>,
) -> SassResult<Value<'parse>> {
    let simplified = simplify_arguments(args)?;
    if simplified.is_empty() {
        return Err(Box::new(SassError::Script {
            message: "hypot() must have at least one argument.".into(),
            argument_name: None,
        }));
    }
    verify_compatible_numbers(&simplified)?;
    let first = match &simplified[0] {
        CalcArgument::Number(n) => n,
        _ => {
            return Ok(Value::new_with_arena(
                arena,
                ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                    "hypot", simplified,
                ))),
            ));
        }
    };
    if first.has_unit("%") {
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                "hypot", simplified,
            ))),
        ));
    }
    let mut subtotal = 0.0f64;
    for (i, arg) in simplified.iter().enumerate() {
        if let CalcArgument::Number(num) = arg {
            if !num.has_compatible_units(first) {
                return Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                        "hypot", simplified,
                    ))),
                ));
            }
            // Matches Dart: convertValueToMatch(first, "numbers[i+1]",
            // "numbers[1]") — threads arg names for error attribution.
            let val = num.convert_value_to_match(
                first,
                Some(&format!("numbers[{}]", i + 1)),
                Some("numbers[1]"),
            )?;
            subtotal += val * val;
        } else {
            return Ok(Value::new_with_arena(
                arena,
                ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                    "hypot", simplified,
                ))),
            ));
        }
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Number(SassNumber::with_units(
            subtotal.sqrt(),
            first.numerator_units.clone(),
            first.denominator_units.clone(),
        )),
    ))
}

/// Creates a `sqrt()` calculation. Requires a unitless argument.
pub fn new_sqrt<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    argument: CalcArgument,
) -> SassResult<Value<'parse>> {
    single_argument(arena, "sqrt", argument, calc_sqrt, true)
}

/// Creates a `sin()` calculation. Coerces an angle argument to radians.
pub fn new_sin<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    argument: CalcArgument,
) -> SassResult<Value<'parse>> {
    single_argument(arena, "sin", argument, calc_sin, false)
}

/// Creates a `cos()` calculation. Coerces an angle argument to radians.
pub fn new_cos<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    argument: CalcArgument,
) -> SassResult<Value<'parse>> {
    single_argument(arena, "cos", argument, calc_cos, false)
}

/// Creates a `tan()` calculation. Coerces an angle argument to radians.
pub fn new_tan<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    argument: CalcArgument,
) -> SassResult<Value<'parse>> {
    single_argument(arena, "tan", argument, calc_tan, false)
}

/// Creates an `atan()` calculation returning degrees. Requires a unitless argument.
pub fn new_atan<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    argument: CalcArgument,
) -> SassResult<Value<'parse>> {
    single_argument(arena, "atan", argument, calc_atan, true)
}

/// Creates an `asin()` calculation returning degrees. Requires a unitless argument.
pub fn new_asin<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    argument: CalcArgument,
) -> SassResult<Value<'parse>> {
    single_argument(arena, "asin", argument, calc_asin, true)
}

/// Creates an `acos()` calculation returning degrees. Requires a unitless argument.
pub fn new_acos<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    argument: CalcArgument,
) -> SassResult<Value<'parse>> {
    single_argument(arena, "acos", argument, calc_acos, true)
}

/// Creates an `abs()` calculation (no deprecation reporting).
///
/// See [`new_abs_internal`] for the variant that warns on `%` arguments.
pub fn new_abs<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    argument: CalcArgument,
) -> SassResult<Value<'parse>> {
    new_abs_internal(arena, argument, &NoOpWarnLogger, None)
}

/// Creates an `abs()` calculation, warning when the argument has `%` units.
///
/// Passing percentages to the global `abs()` is deprecated: a future version
/// will emit a plain CSS `abs()` instead, so callers are pointed at
/// `math.abs()` or at interpolating the argument to opt into CSS now.
pub fn new_abs_internal<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    argument: CalcArgument,
    warn: &dyn WarnLogger<'parse>,
    warn_span: Option<FileSpan<'parse>>,
) -> SassResult<Value<'parse>> {
    let arg = simplify(argument)?;
    if let CalcArgument::Number(ref num) = arg {
        if num.has_unit("%") {
            let s = num.to_display_string()?;
            warn.warn_deprecation(
                &format!(
                    "Passing percentage units to the global abs() function is deprecated.\n\
                     In the future, this will emit a CSS abs() function to be resolved by the browser.\n\
                     To preserve current behavior: math.abs({s})\n\
                     To emit a CSS abs() now: abs(#{{{s}}})\n\
                     More info: https://sass-lang.com/d/abs-percent"
                ),
                &deprecation::ABS_PERCENT,
                warn_span,
            );
        }
        let val = num_abs(num);
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::Number(SassNumber::with_units(
                val,
                num.numerator_units.clone(),
                num.denominator_units.clone(),
            )),
        ));
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
            "abs",
            vec![arg],
        ))),
    ))
}

/// Creates an `exp()` calculation: `e` raised to the unitless `argument`.
pub fn new_exp<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    argument: CalcArgument,
) -> SassResult<Value<'parse>> {
    let arg = simplify(argument)?;
    if let CalcArgument::Number(ref num) = arg {
        num.assert_no_units(None)?;
        let e = SassNumber::new(E, None);
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::Number(calc_pow(&e, num)?),
        ));
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
            "exp",
            vec![arg],
        ))),
    ))
}

/// Creates a `sign()` calculation.
///
/// `NaN` and zero return the argument unchanged; other non-`%` numbers return
/// `-1`/`1` coerced to the argument's units. `%` arguments stay symbolic.
pub fn new_sign<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    argument: CalcArgument,
) -> SassResult<Value<'parse>> {
    let arg = simplify(argument)?;
    if let CalcArgument::Number(ref n) = arg {
        if n.value.is_nan() || n.value == 0.0 {
            return Ok(Value::new_with_arena(arena, ValueKind::Number(n.clone())));
        }
        if !n.has_unit("%") {
            let val = number::sign_including_zero(n.value);
            return Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(SassNumber::with_units(
                    val,
                    n.numerator_units.clone(),
                    n.denominator_units.clone(),
                )),
            ));
        }
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
            "sign",
            vec![arg],
        ))),
    ))
}

/// Creates a `clamp()` calculation with the given `min`, `value`, and `max`.
///
/// Resolves directly to one of the three numbers when all are numbers with
/// mutually compatible units. Fewer than three arguments are accepted only
/// when one of them is an unquoted `var()` string.
pub fn new_clamp<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    min: CalcArgument,
    value: Option<CalcArgument>,
    max: Option<CalcArgument>,
) -> SassResult<Value<'parse>> {
    if value.is_none() && max.is_some() {
        return Err(Box::new(SassError::Script {
            message: "If value is null, max must also be null.".into(),
            argument_name: None,
        }));
    }
    let min = simplify(min)?;
    let value = value.map(simplify).transpose()?;
    let max = max.map(simplify).transpose()?;

    if let CalcArgument::Number(ref min_num) = min {
        if let Some(CalcArgument::Number(ref val_num)) = value {
            if let Some(CalcArgument::Number(ref max_num)) = max {
                if min_num.has_compatible_units(val_num) && min_num.has_compatible_units(max_num) {
                    if val_num.less_than_or_equals_num(min_num)? {
                        return Ok(Value::new_with_arena(
                            arena,
                            ValueKind::Number(min_num.clone()),
                        ));
                    }
                    if val_num.greater_than_or_equals_num(max_num)? {
                        return Ok(Value::new_with_arena(
                            arena,
                            ValueKind::Number(max_num.clone()),
                        ));
                    }
                    return Ok(Value::new_with_arena(
                        arena,
                        ValueKind::Number(val_num.clone()),
                    ));
                }
            }
        }
    }

    let mut args = vec![min];
    if let Some(v) = value {
        args.push(v);
    }
    if let Some(m) = max {
        args.push(m);
    }
    verify_compatible_numbers(&args)?;
    verify_length(&args, 3)?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified("clamp", args))),
    ))
}

/// Creates a `pow()` calculation with the given unitless `base` and `exponent`.
///
/// Fewer than two arguments are accepted only when one of them is an
/// unquoted `var()` string.
pub fn new_pow<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    base: CalcArgument,
    exponent: Option<CalcArgument>,
) -> SassResult<Value<'parse>> {
    let mut args = vec![base.clone()];
    if let Some(ref exp) = exponent {
        args.push(exp.clone());
    }
    verify_length(&args, 2)?;
    let base = simplify(base)?;
    let exp = exponent.map(simplify).transpose()?;
    if let CalcArgument::Number(ref base_num) = base {
        if let Some(CalcArgument::Number(ref exp_num)) = exp {
            base_num.assert_no_units(None)?;
            exp_num.assert_no_units(None)?;
            return Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(calc_pow(base_num, exp_num)?),
            ));
        }
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified("pow", args))),
    ))
}

/// Creates a `log()` calculation with the given unitless `number` and `base`.
///
/// When `base` is `None`, the natural logarithm (base `e`) is used.
pub fn new_log<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    number: CalcArgument,
    base: Option<CalcArgument>,
) -> SassResult<Value<'parse>> {
    let number = simplify(number)?;
    let base = base.map(simplify).transpose()?;
    if let CalcArgument::Number(ref num) = number {
        if base
            .as_ref()
            .map(|b| matches!(b, CalcArgument::Number(_)))
            .unwrap_or(true)
        {
            num.assert_no_units(None)?;
            if let Some(CalcArgument::Number(ref base_num)) = base {
                base_num.assert_no_units(None)?;
                return Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Number(calc_log(num, Some(base_num))?),
                ));
            }
            return Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(calc_log(num, None)?),
            ));
        }
    }
    let mut args = vec![number];
    if let Some(b) = base {
        args.push(b);
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified("log", args))),
    ))
}

/// Creates an `atan2()` calculation for `y` and `x`, returning degrees.
///
/// Both arguments must have mutually compatible non-`%` units; incompatible
/// numbers stay symbolic. Fewer than two arguments are accepted only when one
/// of them is an unquoted `var()` string.
pub fn new_atan2<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    y: CalcArgument,
    x: Option<CalcArgument>,
) -> SassResult<Value<'parse>> {
    let y = simplify(y)?;
    let x = x.map(simplify).transpose()?;
    let mut args = vec![y.clone()];
    if let Some(ref xv) = x {
        args.push(xv.clone());
    }
    verify_length(&args, 2)?;
    verify_compatible_numbers(&args)?;
    if let CalcArgument::Number(ref y_num) = y {
        if let Some(CalcArgument::Number(ref x_num)) = x {
            if y_num.has_unit("%") || x_num.has_unit("%") || !y_num.has_compatible_units(x_num) {
                return Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                        "atan2", args,
                    ))),
                ));
            }
            return Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(calc_atan2(y_num, x_num)?),
            ));
        }
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified("atan2", args))),
    ))
}

/// Creates a `rem()` calculation with the given `dividend` and `modulus`.
///
/// Unlike [`new_mod`], the result takes the sign of the dividend (matching
/// CSS `rem()` semantics), including the infinite-modulus and negative-zero
/// adjustments. Fewer than two arguments are accepted only when one of them
/// is an unquoted `var()` string.
pub fn new_rem<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    dividend: CalcArgument,
    modulus: Option<CalcArgument>,
) -> SassResult<Value<'parse>> {
    let dividend = simplify(dividend)?;
    let modulus = modulus.map(simplify).transpose()?;
    let mut args = vec![dividend.clone()];
    if let Some(ref m) = modulus {
        args.push(m.clone());
    }
    verify_length(&args, 2)?;
    verify_compatible_numbers(&args)?;
    if let CalcArgument::Number(ref div_num) = dividend {
        if let Some(CalcArgument::Number(ref mod_num)) = modulus {
            if !div_num.has_compatible_units(mod_num) {
                return Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                        "rem", args,
                    ))),
                ));
            }
            let result = div_num.modulo_num(mod_num)?;
            let mod_sign = number::sign_including_zero(mod_num.value);
            let div_sign = number::sign_including_zero(div_num.value);
            if mod_sign != div_sign {
                if mod_num.value.is_infinite() {
                    return Ok(Value::new_with_arena(
                        arena,
                        ValueKind::Number(div_num.clone()),
                    ));
                }
                if result.value == 0.0 {
                    return Ok(Value::new_with_arena(
                        arena,
                        ValueKind::Number(result.unary_minus_num()),
                    ));
                }
                return Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Number(result.minus_num(mod_num)?),
                ));
            }
            return Ok(Value::new_with_arena(arena, ValueKind::Number(result)));
        }
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified("rem", args))),
    ))
}

/// Creates a `mod()` calculation with the given `dividend` and `modulus`.
///
/// The result takes the sign of the modulus (Sass modulo semantics); see
/// [`new_rem`] for the dividend-signed CSS variant. Fewer than two arguments
/// are accepted only when one of them is an unquoted `var()` string.
pub fn new_mod<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    dividend: CalcArgument,
    modulus: Option<CalcArgument>,
) -> SassResult<Value<'parse>> {
    let dividend = simplify(dividend)?;
    let modulus = modulus.map(simplify).transpose()?;
    let mut args = vec![dividend.clone()];
    if let Some(ref m) = modulus {
        args.push(m.clone());
    }
    verify_length(&args, 2)?;
    verify_compatible_numbers(&args)?;
    if let CalcArgument::Number(ref div_num) = dividend {
        if let Some(CalcArgument::Number(ref mod_num)) = modulus {
            if !div_num.has_compatible_units(mod_num) {
                return Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                        "mod", args,
                    ))),
                ));
            }
            return Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(div_num.modulo_num(mod_num)?),
            ));
        }
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified("mod", args))),
    ))
}

/// Creates a `round()` calculation with the given strategy/number/step.
///
/// `strategy_or_number` is either a strategy (`nearest`, `up`, `down`,
/// `to-zero`) or the number to round; `number_or_step` and `step` supply the
/// remaining operands. Fewer than the full arguments are accepted only when
/// one of them is an unquoted `var()` string. See [`new_round_internal`] for
/// the legacy-function compatibility variant.
pub fn new_round<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    strategy_or_number: CalcArgument,
    number_or_step: Option<CalcArgument>,
    step: Option<CalcArgument>,
) -> SassResult<Value<'parse>> {
    new_round_internal(
        arena,
        strategy_or_number,
        number_or_step,
        step,
        None,
        &NoOpWarnLogger,
        None,
    )
}

// Like [`new_round`], with internal-only compatibility parameters.
//
// When `in_legacy_sass_function` is set, unitless numbers may be mixed with
// numbers with units for backwards-compatibility with the old global `round`
// behavior, emitting a deprecation warning named after that function.
// `warn` surfaces deprecation warnings at `warn_span`.
//
// Matches Dart: `@internal roundInternal`; internal-only in Dart.
pub fn new_round_internal<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    strategy_or_number: CalcArgument,
    number_or_step: Option<CalcArgument>,
    step: Option<CalcArgument>,
    in_legacy_sass_function: Option<&str>,
    warn: &dyn WarnLogger<'parse>,
    warn_span: Option<FileSpan<'parse>>,
) -> SassResult<Value<'parse>> {
    let strategy = simplify(strategy_or_number)?;
    let number = number_or_step.clone().map(simplify).transpose()?;
    let step_val = step.map(simplify).transpose()?;

    if is_unitless_number(&strategy) && number.is_none() && step_val.is_none() {
        if let CalcArgument::Number(ref n) = strategy {
            return Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(SassNumber::new(n.value.round(), None)),
            ));
        }
    }
    if is_number(&strategy)
        && number.is_none()
        && step_val.is_none()
        && in_legacy_sass_function.is_some()
    {
        warn.warn_deprecation(
            "In future versions of Sass, round() will be interpreted as a CSS round() calculation. This requires an explicit modulus when rounding numbers with units. If you want to use the Sass function, call math.round() instead.\n\nSee https://sass-lang.com/d/import",
            &deprecation::GLOBAL_BUILTIN,
            warn_span,
        );
        if let CalcArgument::Number(ref n) = strategy {
            return Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(match_units(n.value.round(), n)),
            ));
        }
    }
    if is_number(&strategy) && number.is_some() && step_val.is_none() {
        if let CalcArgument::Number(ref strat_num) = strategy {
            if let Some(CalcArgument::Number(ref num)) = number {
                if !strat_num.has_compatible_units(num) {
                    verify_compatible_numbers(&[
                        CalcArgument::Number(strat_num.clone()),
                        CalcArgument::Number(num.clone()),
                    ])?;
                    return Ok(Value::new_with_arena(
                        arena,
                        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                            "round",
                            vec![
                                CalcArgument::Number(strat_num.clone()),
                                CalcArgument::Number(num.clone()),
                            ],
                        ))),
                    ));
                }
                verify_compatible_numbers(&[
                    CalcArgument::Number(strat_num.clone()),
                    CalcArgument::Number(num.clone()),
                ])?;
                return Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Number(round_with_step("nearest", strat_num, num)?),
                ));
            }
        }
    }
    if is_rounding_strategy(&strategy)
        && number
            .as_ref()
            .map(|n| matches!(n, CalcArgument::Number(_)))
            .unwrap_or(false)
        && step_val
            .as_ref()
            .map(|s| matches!(s, CalcArgument::Number(_)))
            .unwrap_or(false)
    {
        if let CalcArgument::String(ref strat_text, _) = strategy {
            if let Some(CalcArgument::Number(ref num)) = number {
                if let Some(CalcArgument::Number(ref stp)) = step_val {
                    if !num.has_compatible_units(stp) {
                        verify_compatible_numbers(&[
                            CalcArgument::Number(num.clone()),
                            CalcArgument::Number(stp.clone()),
                        ])?;
                        return Ok(Value::new_with_arena(
                            arena,
                            ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                                "round",
                                vec![
                                    CalcArgument::String(strat_text.clone(), false),
                                    CalcArgument::Number(num.clone()),
                                    CalcArgument::Number(stp.clone()),
                                ],
                            ))),
                        ));
                    }
                    verify_compatible_numbers(&[
                        CalcArgument::Number(num.clone()),
                        CalcArgument::Number(stp.clone()),
                    ])?;
                    return Ok(Value::new_with_arena(
                        arena,
                        ValueKind::Number(round_with_step(strat_text, num, stp)?),
                    ));
                }
            }
        }
    }
    if is_rounding_strategy(&strategy)
        && number
            .as_ref()
            .map(|n| matches!(n, CalcArgument::String(..)))
            .unwrap_or(false)
        && step_val.is_none()
    {
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                "round",
                vec![strategy, number.unwrap()],
            ))),
        ));
    }
    if is_rounding_strategy(&strategy) && number.is_some() && step_val.is_none() {
        return Err(Box::new(SassError::Script {
            message: "If strategy is not null, step is required.".into(),
            argument_name: None,
        }));
    }
    if is_rounding_strategy(&strategy) && number.is_none() && step_val.is_none() {
        return Err(Box::new(SassError::Script {
            message: "Number to round and step arguments are required.".into(),
            argument_name: None,
        }));
    }
    if !is_rounding_strategy(&strategy) && number.is_none() && step_val.is_none() {
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                "round",
                vec![strategy],
            ))),
        ));
    }
    if !is_rounding_strategy(&strategy) && step_val.is_none() {
        if let Some(num) = number {
            return Ok(Value::new_with_arena(
                arena,
                ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                    "round",
                    vec![strategy, num],
                ))),
            ));
        }
    }
    if (is_rounding_strategy(&strategy) || is_special_variable_string(&strategy))
        && number.is_some()
        && step_val.is_some()
    {
        // `unwrap` is sound here: the outer condition verified both are `Some`,
        // and the move happens only on this return path (later code re-checks
        // `is_some`, which is why `if let (Some,..) = (number, step_val)` would
        // move unconditionally and break E0382 below).
        #[allow(clippy::unnecessary_unwrap)]
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                "round",
                vec![strategy, number.unwrap(), step_val.unwrap()],
            ))),
        ));
    }
    if number.is_some() && step_val.is_some() {
        let display = calc_arg_to_string(&strategy)?;
        return Err(Box::new(SassError::Script {
            message: format!("{display} must be either nearest, up, down or to-zero."),
            argument_name: None,
        }));
    }
    if step_val.is_some() && number_or_step.clone().is_none() {
        return Err(Box::new(SassError::Script {
            message: "Invalid parameters.".into(),
            argument_name: None,
        }));
    }
    Err(Box::new(SassError::Script {
        message: "Invalid parameters.".into(),
        argument_name: None,
    }))
}

/// Creates a `calc-size()` calculation with the given `basis` and `value`.
///
/// Unlike the other constructors this does no numeric simplification: both
/// arguments are kept as-is (after simplification of each side).
pub fn new_calc_size(
    basis: CalcArgument,
    value: Option<CalcArgument>,
) -> SassResult<Box<SassCalculation>> {
    let mut args = vec![basis.clone()];
    if let Some(ref v) = value {
        args.push(v.clone());
    }
    verify_length(&args, 2)?;
    let basis = simplify(basis)?;
    let value = value.map(simplify).transpose()?;
    let mut result_args = vec![basis];
    if let Some(v) = value {
        result_args.push(v);
    }
    Ok(Box::new(SassCalculation::new_unsimplified(
        "calc-size",
        result_args,
    )))
}

/// Creates and simplifies a [`CalculationOperation`] with the given
/// `operator`, `left`, and `right`.
///
/// Each side must be a number, calculation, unquoted string, operation, or
/// interpolation. Automatically simplifies, so the result may be a number
/// rather than an operation. See [`operate_internal`] for the
/// legacy-function compatibility variant.
pub fn operate(
    operator: CalculationOperator,
    left: CalcArgument,
    right: CalcArgument,
) -> SassResult<CalcArgument> {
    operate_internal(operator, left, right, None, true, &NoOpWarnLogger, None)
}

// Like [`operate`], with internal-only compatibility parameters.
//
// When `in_legacy_sass_function` is set, unitless numbers may be added to
// and subtracted from numbers with units for backwards-compatibility with
// the old global `min()`/`max()` functions, emitting a deprecation warning
// named after that function. When `do_simplify` is `false`, no
// simplification is done. `warn` surfaces deprecation warnings at
// `warn_span`.
//
// Matches Dart: `@internal operateInternal`; internal-only in Dart.
pub fn operate_internal<'parse>(
    operator: CalculationOperator,
    left: CalcArgument,
    right: CalcArgument,
    in_legacy_sass_function: Option<&str>,
    do_simplify: bool,
    warn: &dyn WarnLogger<'parse>,
    warn_span: Option<FileSpan<'parse>>,
) -> SassResult<CalcArgument> {
    if !do_simplify {
        return Ok(CalcArgument::Operation(Box::new(CalculationOperation {
            operator,
            left,
            right,
            cached_hash: Cell::new(None),
        })));
    }
    let left = simplify(left)?;
    let right = simplify(right)?;
    if operator == CalculationOperator::Plus || operator == CalculationOperator::Minus {
        if let CalcArgument::Number(ref left_num) = left {
            if let CalcArgument::Number(ref right_num) = right {
                let mut compatible = left_num.has_compatible_units(right_num);
                if !compatible {
                    if let Some(legacy_fn) = in_legacy_sass_function {
                        if left_num.is_comparable_to(right_num) {
                            warn.warn_deprecation(
                                &format!(
                                    "In future versions of Sass, {}() will be interpreted as the CSS {}() calculation. This doesn't allow unitless numbers to be mixed with numbers with units. If you want to use the Sass function, call math.{}() instead.\n\nSee https://sass-lang.com/d/import",
                                    legacy_fn,
                                    legacy_fn,
                                    legacy_fn
                                ),
                                &deprecation::GLOBAL_BUILTIN,
                                warn_span,
                            );
                            compatible = true;
                        }
                    }
                }
                if compatible {
                    return Ok(if operator == CalculationOperator::Plus {
                        CalcArgument::Number(left_num.plus_num(right_num)?)
                    } else {
                        CalcArgument::Number(left_num.minus_num(right_num)?)
                    });
                }
            }
        }
        verify_compatible_numbers(&[left.clone(), right.clone()])?;
        let mut operator = operator;
        let mut right = right;
        // Folds a negative right operand into the operator (`+-n` → `--n`)
        // so serialized output never contains a double sign.
        if let CalcArgument::Number(ref right_num) = right {
            if number::fuzzy_less_than(right_num.value, 0.0) {
                let neg = right_num.times_num(&SassNumber::new(-1.0, None))?;
                right = CalcArgument::Number(neg);
                operator = if operator == CalculationOperator::Plus {
                    CalculationOperator::Minus
                } else {
                    CalculationOperator::Plus
                };
            }
        }
        return Ok(CalcArgument::Operation(Box::new(CalculationOperation {
            operator,
            left,
            right,
            cached_hash: Cell::new(None),
        })));
    } else if let CalcArgument::Number(ref left_num) = left {
        if let CalcArgument::Number(ref right_num) = right {
            return Ok(if operator == CalculationOperator::Times {
                CalcArgument::Number(left_num.times_num(right_num)?)
            } else {
                CalcArgument::Number(left_num.divided_by_num(right_num)?)
            });
        }
    }
    Ok(CalcArgument::Operation(Box::new(CalculationOperation {
        operator,
        left,
        right,
        cached_hash: Cell::new(None),
    })))
}

// ===========================================================================
// Private helpers
// ===========================================================================

// Simplifies a calculation argument.
//
// Numbers and operations pass through; interpolation is parenthesized into
// an unquoted string; quoted strings and non-numeric values produce an
// `Err`; a single-argument `calc()` unwraps (parenthesizing raw text that
// needs it, such as text containing whitespace, `/`, `*`, or `var(`).
pub(crate) fn simplify(arg: CalcArgument) -> SassResult<CalcArgument> {
    match arg {
        CalcArgument::Number(_) | CalcArgument::Operation(_) => Ok(arg),
        CalcArgument::Interpolation(value) => {
            Ok(CalcArgument::String(format!("({})", value), false))
        }
        CalcArgument::String(ref s, has_quotes) => {
            if has_quotes {
                Err(Box::new(SassError::Script {
                    message: format!("Quoted string \"{}\" can't be used in a calculation.", s),
                    argument_name: None,
                }))
            } else {
                Ok(arg)
            }
        }
        CalcArgument::Calculation(ref c) => {
            if c.name == "calc" && c.arguments.len() == 1 {
                if let CalcArgument::String(ref s, false) = c.arguments[0] {
                    if needs_parentheses(s) {
                        return Ok(CalcArgument::String(format!("({})", s), false));
                    }
                }
                return Ok(c.arguments[0].clone());
            }
            Ok(arg)
        }
    }
}

// Returns `args` with each argument simplified.
pub(crate) fn simplify_arguments(args: Vec<CalcArgument>) -> SassResult<Vec<CalcArgument>> {
    let mut result = Vec::with_capacity(args.len());
    for arg in args {
        result.push(simplify(arg)?);
    }
    Ok(result)
}

// Verifies that the numbers in `args` are compatible with CSS calculations.
//
// Returns an unspanned `Script` error (spans are attached by the eval
// caller): complex units are rejected outright, and pairwise-incompatible
// numbers are rejected. Non-number arguments are skipped.
pub(crate) fn verify_compatible_numbers(args: &[CalcArgument]) -> SassResult<()> {
    for arg in args {
        if let CalcArgument::Number(ref num) = arg {
            if num.has_complex_units() {
                return Err(Box::new(SassError::Script {
                    message: format!(
                        "Number {} isn't compatible with CSS calculations.",
                        num.to_display_string()?
                    ),
                    argument_name: None,
                }));
            }
        }
    }
    for i in 0..args.len() {
        if let CalcArgument::Number(ref num1) = args[i] {
            for j in (i + 1)..args.len() {
                if let CalcArgument::Number(ref num2) = args[j] {
                    if num1.has_possibly_compatible_units(num2) {
                        continue;
                    }
                    return Err(Box::new(SassError::Script {
                        // Matches Dart: "$number1 and $number2 are
                        // incompatible." interpolates via SassNumber.toString
                        // (precision/fuzzy/-0 handling), not Rust {} on f64.
                        message: format!(
                            "{} and {} are incompatible.",
                            num1.to_display_string()?,
                            num2.to_display_string()?,
                        ),
                        argument_name: None,
                    }));
                }
            }
        }
    }
    Ok(())
}

// Converts `arg` to its display string for error messages and the
// `round()` strategy check. Interpolation renders as its raw text.
//
// Matches Dart: `sprint_any`-style display via `SassNumber.toString`,
// never raw float formatting.
pub(crate) fn calc_arg_to_string(arg: &CalcArgument) -> SassResult<String> {
    match arg {
        CalcArgument::Number(n) => n.to_display_string(),
        CalcArgument::String(s, _) => Ok(s.clone()),
        CalcArgument::Calculation(c) => c.to_display_string(),
        CalcArgument::Operation(op) => op.to_display_string(),
        CalcArgument::Interpolation(s) => Ok(s.clone()),
    }
}

// Returns an `Err` unless `args` has `expected_length` entries or contains a
// string of any flavor (which may stand in for unknown arguments such as
// `var()`), mirroring Dart's `_verifyLength` exemption for `SassString`.
fn verify_length(args: &[CalcArgument], expected_length: usize) -> SassResult<()> {
    if args.len() == expected_length {
        return Ok(());
    }
    // Matches Dart: `args.any((arg) => arg is SassString)` — any string
    // flavor (incl. the deprecated Interpolation form) exempts the check.
    for arg in args {
        if matches!(
            arg,
            CalcArgument::String(..) | CalcArgument::Interpolation(..)
        ) {
            return Ok(());
        }
    }
    Err(Box::new(SassError::Script {
        message: format!(
            "{} arguments required, but only {} {} passed.",
            expected_length,
            args.len(),
            if args.len() == 1 { "was" } else { "were" }
        ),
        argument_name: None,
    }))
}

// Returns whether `text` needs parentheses when the contents of a `calc()`
// are embedded in another calculation. Mirrors Dart's `_needsParentheses`:
// whitespace, `/`, or `*` anywhere forces parentheses (Dart's
// `_charNeedsParentheses`), plus the case-insensitive `var(` prefix check.
fn needs_parentheses(text: &str) -> bool {
    if text
        .chars()
        .any(|ch| ch.is_whitespace() || ch == '/' || ch == '*')
    {
        return true;
    }
    let lower = text.to_lowercase();
    lower.starts_with("var(") && text.chars().count() >= 4
}

// Whether `arg` is any number (used by the `round()` single-number fast paths).
fn is_number(arg: &CalcArgument) -> bool {
    matches!(arg, CalcArgument::Number(_))
}

// Whether `arg` is a unitless number (the plain `round($number)` fast path).
fn is_unitless_number(arg: &CalcArgument) -> bool {
    if let CalcArgument::Number(ref n) = arg {
        return !n.has_units();
    }
    false
}

// Whether `arg` is an unquoted rounding-strategy string (`nearest`, `up`,
// `down`, or `to-zero`). Quoted strings never count as strategies.
fn is_rounding_strategy(arg: &CalcArgument) -> bool {
    if let CalcArgument::String(ref s, has_quotes) = arg {
        if *has_quotes {
            return false;
        }
        return s == "nearest" || s == "up" || s == "down" || s == "to-zero";
    }
    false
}

// Whether `arg` is an unquoted special-variable string (`var(` / `attr(` /
// `if(` prefix, case-insensitive): arguments the CSS engine resolves later,
// so calculations containing them stay symbolic.
fn is_special_variable_string(arg: &CalcArgument) -> bool {
    if let CalcArgument::String(ref s, has_quotes) = arg {
        if *has_quotes {
            return false;
        }
        let lower = s.to_lowercase();
        return lower.starts_with("var(") || lower.starts_with("attr(") || lower.starts_with("if(");
    }
    false
}

// Returns a callable-like single-argument math helper: simplifies `arg`,
// applies `f` to numbers (rejecting units when `forbid_units` holds), and
// otherwise returns an unsimplified calculation named `name`. Mirrors Dart's
// `_singleArgument`.
fn single_argument<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    name: &str,
    arg: CalcArgument,
    f: fn(&SassNumber) -> SassResult<SassNumber>,
    forbid_units: bool,
) -> SassResult<Value<'parse>> {
    let arg = simplify(arg)?;
    if let CalcArgument::Number(ref num) = arg {
        if forbid_units {
            num.assert_no_units(None)?;
        }
        return Ok(Value::new_with_arena(arena, ValueKind::Number(f(num)?)));
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(name, vec![arg]))),
    ))
}

// Returns `value` coerced to `number`'s units. Mirrors Dart's `_matchUnits`.
fn match_units(value: f64, number: &SassNumber) -> SassNumber {
    SassNumber::with_units(
        value,
        number.numerator_units.clone(),
        number.denominator_units.clone(),
    )
}

// Returns `number` rounded to the nearest integer multiple of `step` per
// `strategy` (`nearest`, `up`, `down`, or `to-zero`), in `number`'s units.
// Mirrors Dart's `_roundWithStep`, including the NaN/infinite edge cases.
fn round_with_step(
    strategy: &str,
    number: &SassNumber,
    step: &SassNumber,
) -> SassResult<SassNumber> {
    if strategy != "nearest" && strategy != "up" && strategy != "down" && strategy != "to-zero" {
        return Err(Box::new(SassError::Script {
            message: format!("{} must be either nearest, up, down or to-zero.", strategy),
            argument_name: None,
        }));
    }
    if (number.value.is_infinite() && step.value.is_infinite())
        || step.value == 0.0
        || number.value.is_nan()
        || step.value.is_nan()
    {
        return Ok(SassNumber::with_units(
            f64::NAN,
            number.numerator_units.clone(),
            number.denominator_units.clone(),
        ));
    }
    if number.value.is_infinite() {
        return Ok(number.clone());
    }
    if step.value.is_infinite() {
        return match strategy {
            _ if number.value == 0.0 => Ok(number.clone()),
            "nearest" | "to-zero" => {
                if number.value > 0.0 {
                    Ok(SassNumber::with_units(
                        0.0,
                        number.numerator_units.clone(),
                        number.denominator_units.clone(),
                    ))
                } else {
                    Ok(SassNumber::with_units(
                        -0.0,
                        number.numerator_units.clone(),
                        number.denominator_units.clone(),
                    ))
                }
            }
            "up" => {
                if number.value > 0.0 {
                    Ok(SassNumber::with_units(
                        f64::INFINITY,
                        number.numerator_units.clone(),
                        number.denominator_units.clone(),
                    ))
                } else {
                    Ok(SassNumber::with_units(
                        -0.0,
                        number.numerator_units.clone(),
                        number.denominator_units.clone(),
                    ))
                }
            }
            "down" => {
                if number.value < 0.0 {
                    Ok(SassNumber::with_units(
                        f64::NEG_INFINITY,
                        number.numerator_units.clone(),
                        number.denominator_units.clone(),
                    ))
                } else {
                    Ok(SassNumber::with_units(
                        0.0,
                        number.numerator_units.clone(),
                        number.denominator_units.clone(),
                    ))
                }
            }
            _ => Ok(SassNumber::with_units(
                f64::NAN,
                number.numerator_units.clone(),
                number.denominator_units.clone(),
            )),
        };
    }
    let step_val = step.convert_value_to_match(number, None, None)?;
    let result = match strategy {
        "nearest" => (number.value / step_val).round() * step_val,
        "up" => {
            if step.value < 0.0 {
                (number.value / step_val).floor() * step_val
            } else {
                (number.value / step_val).ceil() * step_val
            }
        }
        "down" => {
            if step.value < 0.0 {
                (number.value / step_val).ceil() * step_val
            } else {
                (number.value / step_val).floor() * step_val
            }
        }
        "to-zero" => {
            if number.value < 0.0 {
                (number.value / step_val).ceil() * step_val
            } else {
                (number.value / step_val).floor() * step_val
            }
        }
        _ => f64::NAN,
    };
    Ok(SassNumber::with_units(
        result,
        number.numerator_units.clone(),
        number.denominator_units.clone(),
    ))
}

// ----- Calc math wrappers -----

fn num_abs(n: &SassNumber) -> f64 {
    abs(n.value)
}

fn calc_sqrt(n: &SassNumber) -> SassResult<SassNumber> {
    Ok(SassNumber::new(sqrt(n.value), None))
}

fn calc_sin(n: &SassNumber) -> SassResult<SassNumber> {
    let val = n.coerce_value_to_unit("rad", Some("number"))?;
    Ok(SassNumber::new(sin(val), None))
}

fn calc_cos(n: &SassNumber) -> SassResult<SassNumber> {
    let val = n.coerce_value_to_unit("rad", Some("number"))?;
    Ok(SassNumber::new(cos(val), None))
}

fn calc_tan(n: &SassNumber) -> SassResult<SassNumber> {
    let val = n.coerce_value_to_unit("rad", Some("number"))?;
    Ok(SassNumber::new(tan(val), None))
}

fn calc_atan(n: &SassNumber) -> SassResult<SassNumber> {
    Ok(radians_to_degrees(n.value.atan()))
}

fn calc_asin(n: &SassNumber) -> SassResult<SassNumber> {
    Ok(radians_to_degrees(n.value.asin()))
}

fn calc_acos(n: &SassNumber) -> SassResult<SassNumber> {
    Ok(radians_to_degrees(n.value.acos()))
}

fn calc_pow(base: &SassNumber, exp: &SassNumber) -> SassResult<SassNumber> {
    let result = number_math::pow_number(base, exp)?;
    Ok(SassNumber::new(result.value, None))
}

fn calc_log(number: &SassNumber, base: Option<&SassNumber>) -> SassResult<SassNumber> {
    // Route through `log_number` so the units-order contract is shared; the
    // calc path pre-asserts unitlessness, so order is unobservable here.
    let result = match base {
        Some(b) => {
            let base_kind = ValueKind::Number(b.clone());
            number_math::log_number(number, Some(&base_kind))?
        }
        None => number_math::log_number(number, None)?,
    };
    Ok(SassNumber::new(result.value, None))
}

fn calc_atan2(y: &SassNumber, x: &SassNumber) -> SassResult<SassNumber> {
    let result = number_math::atan2_number(y, x)?;
    Ok(result)
}

fn radians_to_degrees(radians: f64) -> SassNumber {
    SassNumber::new(radians * (180.0 / PI), Some("deg"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bumpalo::Bump;

    #[test]
    fn test_calculation_operator_name() {
        assert_eq!(CalculationOperator::Plus.name(), "plus");
        assert_eq!(CalculationOperator::Minus.name(), "minus");
        assert_eq!(CalculationOperator::Times.name(), "times");
        assert_eq!(CalculationOperator::DividedBy.name(), "divided by");
    }

    #[test]
    fn test_calculation_operator_precedence() {
        assert_eq!(CalculationOperator::Plus.precedence(), 1);
        assert_eq!(CalculationOperator::Minus.precedence(), 1);
        assert_eq!(CalculationOperator::Times.precedence(), 2);
        assert_eq!(CalculationOperator::DividedBy.precedence(), 2);
    }

    #[test]
    fn test_calculation_is_special_number() {
        let c = SassCalculation::new_unsimplified(
            "calc",
            vec![CalcArgument::Number(SassNumber::new(1.0, None))],
        );
        assert!(c.is_special_number());
    }

    #[test]
    fn test_calculation_is_truthy() {
        let c = SassCalculation::new_unsimplified(
            "calc",
            vec![CalcArgument::Number(SassNumber::new(1.0, None))],
        );
        assert!(c.is_truthy());
    }

    #[test]
    fn test_calculation_equals() {
        let c1 = SassCalculation::new_unsimplified(
            "calc",
            vec![CalcArgument::Number(SassNumber::new(1.0, None))],
        );
        let c2 = SassCalculation::new_unsimplified(
            "calc",
            vec![CalcArgument::Number(SassNumber::new(1.0, None))],
        );
        assert!(c1.equals(&c2));
        let c3 = SassCalculation::new_unsimplified(
            "min",
            vec![CalcArgument::Number(SassNumber::new(1.0, None))],
        );
        assert!(!c1.equals(&c3));
    }

    #[test]
    fn test_calculation_hash_code() {
        let c1 = SassCalculation::new_unsimplified(
            "calc",
            vec![CalcArgument::Number(SassNumber::new(1.0, None))],
        );
        let c2 = SassCalculation::new_unsimplified(
            "calc",
            vec![CalcArgument::Number(SassNumber::new(1.0, None))],
        );
        assert_eq!(c1.hash_code(), c2.hash_code());
    }

    #[test]
    fn test_calc_arg_equals() {
        let a = CalcArgument::Number(SassNumber::new(1.0, None));
        let b = CalcArgument::Number(SassNumber::new(1.0, None));
        assert!(a.equals(&b));
        let c = CalcArgument::Number(SassNumber::new(2.0, None));
        assert!(!a.equals(&c));
    }

    #[test]
    fn test_new_abs_unitless() {
        let bump = Bump::new();
        let v = new_abs(&bump, CalcArgument::Number(SassNumber::new(-5.0, None))).unwrap();
        match &*v {
            ValueKind::Number(n) => assert!((n.value - 5.0).abs() < 1e-9),
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn test_new_abs_non_number() {
        let bump = Bump::new();
        let v = new_abs(&bump, CalcArgument::String("x".to_string(), false)).unwrap();
        match &*v {
            ValueKind::Calculation(_) => {}
            _ => panic!("expected Calculation"),
        }
    }

    #[test]
    fn test_new_hypot_unitless() {
        let bump = Bump::new();
        let v = new_hypot(
            &bump,
            vec![
                CalcArgument::Number(SassNumber::new(3.0, None)),
                CalcArgument::Number(SassNumber::new(4.0, None)),
            ],
        )
        .unwrap();
        match &*v {
            ValueKind::Number(n) => {
                assert!((n.value - 5.0).abs() < 1e-9, "hypot(3,4) = {}", n.value)
            }
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn test_new_hypot_zero_zero() {
        let bump = Bump::new();
        let v = new_hypot(
            &bump,
            vec![
                CalcArgument::Number(SassNumber::new(0.0, None)),
                CalcArgument::Number(SassNumber::new(0.0, None)),
            ],
        )
        .unwrap();
        match &*v {
            ValueKind::Number(n) => assert!((n.value).abs() < 1e-9),
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn test_new_calc_with_quoted_string_errors() {
        let bump = Bump::new();
        let result = new_calc(&bump, CalcArgument::String("hello".to_string(), true));
        assert!(result.is_err(), "expected error for quoted string in calc");
    }

    #[test]
    fn test_new_min_with_quoted_string_errors() {
        let bump = Bump::new();
        let result = new_min(&bump, vec![CalcArgument::String("hello".to_string(), true)]);
        assert!(result.is_err(), "expected error for quoted string in min");
    }

    #[test]
    fn test_quoted_strategy_is_not_rounding() {
        let q = CalcArgument::String("nearest".to_string(), true);
        assert!(!is_rounding_strategy(&q));
        let uq = CalcArgument::String("nearest".to_string(), false);
        assert!(is_rounding_strategy(&uq));
    }

    #[test]
    fn test_quoted_string_not_special_variable() {
        let q = CalcArgument::String("var(--x)".to_string(), true);
        assert!(!is_special_variable_string(&q));
        let uq = CalcArgument::String("var(--x)".to_string(), false);
        assert!(is_special_variable_string(&uq));
    }

    #[test]
    fn test_new_pow() {
        let bump = Bump::new();
        let v = new_pow(
            &bump,
            CalcArgument::Number(SassNumber::new(2.0, None)),
            Some(CalcArgument::Number(SassNumber::new(3.0, None))),
        )
        .unwrap();
        match &*v {
            ValueKind::Number(n) => assert!((n.value - 8.0).abs() < 1e-9),
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn test_new_pow_with_units_errors() {
        let bump = Bump::new();
        let result = new_pow(
            &bump,
            CalcArgument::Number(SassNumber::new(2.0, Some("px"))),
            Some(CalcArgument::Number(SassNumber::new(3.0, None))),
        );
        assert!(result.is_err());
    }
}
