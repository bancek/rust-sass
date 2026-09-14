// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/functions/math.dart
// go-source: go/functions/math.go + go/functions/functions_math_init.go

use crate::eval::warn::warn_deprecation;
use crate::math::abs;
use crate::math::pow;
use crate::math::sqrt;
use std::f64::consts::E;
use std::f64::consts::PI;
use std::rc::Rc;

use bumpalo::Bump;
use indexmap::IndexMap;

use crate::callable::{BuiltInCallable, Callable, CallableKind};
use crate::common::exception::{SassError, SassResult};
use crate::deprecation::{ABS_PERCENT, FUNCTION_UNITS};
use crate::eval::warn::warn;
use crate::eval::{EvalConfig, EvalState};
use crate::functions::helpers::{random_float, random_int, warn_for_global_builtin};
use crate::module::BuiltInModule;
use crate::value::number_math::{
    acos_number, asin_number, atan2_number, atan_number, cos_number, log_number, pow_number,
    sin_number, sqrt_number, tan_number,
};
use crate::value::{
    assert_number, SassNumber, SassString, Value, ValueKind, SASS_FALSE, SASS_TRUE,
};

/// Returns all globally-available math functions.
///
/// Ports Dart's `global` list (math.dart:19-53): the module callables
/// wrapped with a `math` deprecation warning, except `abs` (which warns
/// `ABS_PERCENT` for `%` units and a global-builtin warning otherwise) and
/// the renames `comparable` (for `compatible`) and `unitless` (for
/// `is-unitless`).
///
/// Matches Go: GlobalMathFunctions
pub fn global_math_functions<'compile, 'parse>(
    arena: &'compile Bump,
) -> Vec<Callable<'compile, 'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    macro_rules! c {
        ($arena:expr, $f:expr) => {
            Callable::new($arena, CallableKind::BuiltIn($f))
        };
    }
    vec![
        c!(arena, abs_function_global(arena)),
        c!(
            arena,
            ceil_function(arena).with_deprecation_warning("math", None)
        ),
        c!(
            arena,
            floor_function(arena).with_deprecation_warning("math", None)
        ),
        c!(
            arena,
            max_function(arena).with_deprecation_warning("math", None)
        ),
        c!(
            arena,
            min_function(arena).with_deprecation_warning("math", None)
        ),
        c!(
            arena,
            percentage_function(arena).with_deprecation_warning("math", None)
        ),
        c!(
            arena,
            random_function(arena).with_deprecation_warning("math", None)
        ),
        c!(
            arena,
            round_function(arena).with_deprecation_warning("math", None)
        ),
        c!(
            arena,
            unit_function(arena).with_deprecation_warning("math", None)
        ),
        c!(
            arena,
            comparable_function_global(arena).with_deprecation_warning("math", Some("compatible"))
        ),
        c!(
            arena,
            unitless_function_global(arena).with_deprecation_warning("math", Some("is-unitless"))
        ),
    ]
}

/// Returns the sass:math built-in module.
///
/// Ports Dart's `module` (math.dart:56-73) in declaration order: bounding,
/// distance, exponential, trigonometric, unit, and other functions, with
/// `div` last (observable via `meta.module-functions("math")`), plus the
/// `$e`/`$pi`/`$epsilon`/`$max-safe-integer`/`$min-safe-integer`/
/// `$max-number`/`$min-number` variables. `$min-number` is 5e-324, the
/// smallest subnormal — not `f64::MIN_POSITIVE`.
///
/// Matches Go: MathModule
pub fn math_module<'compile, 'parse>(arena: &'compile Bump) -> BuiltInModule<'compile, 'parse>
where
    'compile: 'parse,
{
    macro_rules! c {
        ($arena:expr, $f:expr) => {
            Callable::new($arena, CallableKind::BuiltIn($f))
        };
    }
    let fns: Vec<Callable<'compile, 'parse>> = vec![
        c!(arena, abs_function(arena)),
        c!(arena, acos_function(arena)),
        c!(arena, asin_function(arena)),
        c!(arena, atan_function(arena)),
        c!(arena, atan2_function(arena)),
        c!(arena, ceil_function(arena)),
        c!(arena, clamp_function(arena)),
        c!(arena, cos_function(arena)),
        c!(arena, compatible_function(arena)),
        c!(arena, floor_function(arena)),
        c!(arena, hypot_function(arena)),
        c!(arena, is_unitless_module_function(arena)),
        c!(arena, log_function(arena)),
        c!(arena, max_function(arena)),
        c!(arena, min_function(arena)),
        c!(arena, percentage_function(arena)),
        c!(arena, pow_function(arena)),
        c!(arena, random_function(arena)),
        c!(arena, round_function(arena)),
        c!(arena, sin_function(arena)),
        c!(arena, sqrt_function(arena)),
        c!(arena, tan_function(arena)),
        c!(arena, unit_function(arena)),
        // Dart declares `_div` last (math.dart:60-62); observable via
        // `meta.module-functions("math")` order.
        c!(arena, div_function(arena)),
    ];
    let mut vars: IndexMap<String, Value<'parse>> = IndexMap::new();
    vars.insert("e".into(), ValueKind::unitless_number(arena, E));
    vars.insert("pi".into(), ValueKind::unitless_number(arena, PI));
    vars.insert(
        "epsilon".into(),
        ValueKind::unitless_number(arena, 2.220446049250313e-16),
    );
    vars.insert(
        "max-safe-integer".into(),
        ValueKind::unitless_number(arena, 9007199254740991.0),
    );
    vars.insert(
        "min-safe-integer".into(),
        ValueKind::unitless_number(arena, -9007199254740991.0),
    );
    vars.insert(
        "max-number".into(),
        ValueKind::unitless_number(arena, f64::MAX),
    );
    // Go: math.SmallestNonzeroFloat64 / Dart: double.minPositive (5e-324, the
    // smallest positive subnormal — NOT Rust's f64::MIN_POSITIVE).
    vars.insert(
        "min-number".into(),
        ValueKind::unitless_number(arena, 5e-324),
    );
    BuiltInModule::new(arena, "math".into(), &fns, &[], vars)
}

// ---- Helpers ----
//
// Ports Dart's `_numberFunction` (math.dart:291-300), `_singleArgumentMathFunc`
// (math.dart:279-287), and `_function` (math.dart:303-308: fixes the URL to
// `sass:math` instead of taking it as a parameter). Both helpers fix the
// signature to `$number`; `_singleArgumentMathFunc` returns the callback's
// result directly, `_numberFunction` applies a value transform and preserves
// the input units.

/// Creates a `sass:math` callable named `name` with signature `$number`.
///
/// Transforms a number's value using `transform` and preserves its units.
///
/// Matches Dart: _numberFunction (math.dart:291-300) / Go: numberFunction
fn number_function<'compile, 'parse>(
    name: &str,
    arena: &'compile Bump,
    transform: fn(f64) -> SassResult<f64>,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        name,
        "$number",
        "sass:math",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let number = assert_number(&args[0], Some("number"))?;
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Number(SassNumber::with_units(
                        transform(number.value)?,
                        number.numerator_units.clone(),
                        number.denominator_units.clone(),
                    )),
                ))
            },
        ),
    )
}

/// Creates a `sass:math` callable named `name` with signature `$number`.
///
/// Calls a single-argument math function: asserts `$number` and returns the
/// callback's `SassNumber` directly.
///
/// Matches Dart: _singleArgumentMathFunc (math.dart:279-287) / Go: singleArgumentMathFunc
fn single_argument_math_func<'compile, 'parse>(
    name: &str,
    arena: &'compile Bump,
    f: fn(&SassNumber) -> SassResult<SassNumber>,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        name,
        "$number",
        "sass:math",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let number = assert_number(&args[0], Some("number"))?;
                Ok(Value::new_with_arena(arena, ValueKind::Number(f(number)?)))
            },
        ),
    )
}

// ---- Global-only functions ----
//
// Ports Dart's hand-authored global `abs` closure (math.dart:20-42): warns
// ABS_PERCENT when `$number` has `%` units, `warn_for_global_builtin`
// otherwise, and returns `|value|` with the input units preserved.

fn abs_function_global<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "abs",
        "$number",
        "sass:math",
        arena,
        Rc::new(
            move |config: &EvalConfig<'compile, 'parse>,
                  state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let number = assert_number(&args[0], Some("number"))?;
                if number.has_unit("%") {
                    let num_str = number.to_display_string()?;
                    warn_deprecation(config, state,
                        &format!(
                            "Passing percentage units to the global abs() function is deprecated.\n\
                             In the future, this will emit a CSS abs() function to be resolved by the browser.\n\
                             To preserve current behavior: math.abs({num_str})\n\
                             To emit a CSS abs() now: abs(#{{{num_str}}})\n\
                             More info: https://sass-lang.com/d/abs-percent"
                        ),
                        &ABS_PERCENT,
                    )?;
                } else {
                    warn_for_global_builtin(config, state, "math", "abs")?;
                }
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Number(SassNumber::with_units(
                        abs(number.value),
                        number.numerator_units.clone(),
                        number.denominator_units.clone(),
                    )),
                ))
            },
        ),
    )
}

/// Creates the `math.abs` callable (`$number`).
///
/// Returns `|$number|` with the input units preserved (Dart's
/// `_numberFunction("abs", (value) => value.abs())`, math.dart:59).
fn abs_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    number_function("abs", arena, |v| Ok(abs(v)))
}

/// Creates the `math.ceil` callable (`$number`).
///
/// Rounds up to the next integer, preserving units (Dart's
/// `_numberFunction("ceil", (value) => value.ceil().toDouble())`,
/// math.dart:79). Saturates at the int64 extremes; see [`dart_int_op`].
fn ceil_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    number_function("ceil", arena, dart_ceil)
}

/// Creates the `math.floor` callable (`$number`).
///
/// Rounds down to the previous integer, preserving units (Dart's
/// `_numberFunction("floor", (value) => value.floor().toDouble())`,
/// math.dart:98). Saturates at the int64 extremes; see [`dart_int_op`].
fn floor_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    number_function("floor", arena, dart_floor)
}

/// Creates the `math.round` callable (`$number`).
///
/// Rounds to the nearest integer (half away from zero), preserving units
/// (Dart's `_numberFunction("round", (number) => number.round().toDouble())`,
/// math.dart:120). Saturates at the int64 extremes; see [`dart_int_op`].
fn round_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    number_function("round", arena, dart_round)
}

/// Rounds up, matching Dart's `num.ceil()` (math.dart:79).
///
/// See [`dart_int_op`] for the saturation/error contract.
fn dart_ceil(v: f64) -> SassResult<f64> {
    dart_int_op(v, f64::ceil)
}

/// Rounds down, matching Dart's `num.floor()` (math.dart:98).
///
/// See [`dart_int_op`] for the saturation/error contract.
fn dart_floor(v: f64) -> SassResult<f64> {
    dart_int_op(v, f64::floor)
}

/// Rounds half away from zero, matching Dart's `num.round()` (math.dart:120).
///
/// See [`dart_int_op`] for the saturation/error contract.
fn dart_round(v: f64) -> SassResult<f64> {
    dart_int_op(v, f64::round)
}

/// Applies an integer-valued op with Dart `num.toInt()` semantics.
///
/// Out-of-range finite values saturate at ±2⁶³∓1 (returned as an f64
/// sentinel — no f64 holds 2⁶³−1 exactly, so the serializer maps it back to
/// the exact digit string); non-finite input throws
/// `Unsupported operation: Infinity or NaN toInt`.
fn dart_int_op(v: f64, op: fn(f64) -> f64) -> SassResult<f64> {
    if !v.is_finite() {
        return Err(Box::new(SassError::Script {
            message: "Unsupported operation: Infinity or NaN toInt".into(),
            argument_name: None,
        }));
    }
    // Dart `num.ceil()` returns a 64-bit int (saturating at ±2⁶³∓1), and the
    // serializer prints full integers via `integer.toString()`. No f64 can
    // hold 2⁶³-1 exactly, so return the int64 extreme as a sentinel: the
    // serializer maps it back to the exact digit string (see
    // `write_number_to`).
    let r = op(v);
    if r >= i64::MAX as f64 {
        return Ok(i64::MAX as f64);
    }
    if r <= i64::MIN as f64 {
        return Ok(i64::MIN as f64);
    }
    // Dart's `num.ceil()/floor()/round()` return 64-bit ints, which have no
    // negative zero; the `.toDouble()` round-trip yields `+0.0`. Rust's
    // `f64::{ceil, floor, round}` can return `-0.0` instead (#2840).
    if r == 0.0 {
        return Ok(0.0);
    }
    Ok(r)
}

/// Creates the `math.max` callable (`$numbers...`).
///
/// Returns the greatest of the arguments (compared with `<`, so the first
/// maximum wins ties); throws `At least one argument must be passed.` when
/// empty (math.dart:100-108).
fn max_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "max",
        "$numbers...",
        "sass:math",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  _arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let numbers = args[0].as_list(arena)?;
                if numbers.is_empty() {
                    return Err(Box::new(SassError::Script {
                        message: "At least one argument must be passed.".into(),
                        argument_name: None,
                    }));
                }
                let mut max_idx: Option<usize> = None;
                for (i, v) in numbers.iter().enumerate() {
                    assert_number(v, None)?;
                    match max_idx {
                        None => max_idx = Some(i),
                        Some(mi) => {
                            let less = numbers[mi].less_than(arena, v)?;
                            if less.is_truthy() {
                                max_idx = Some(i);
                            }
                        }
                    }
                }
                let mi = max_idx.expect("numbers is non-empty");
                Ok(numbers[mi])
            },
        ),
    )
}

/// Creates the `math.min` callable (`$numbers...`).
///
/// Returns the least of the arguments (compared with `>`, so the first
/// minimum wins ties); throws `At least one argument must be passed.` when
/// empty (math.dart:110-118).
fn min_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "min",
        "$numbers...",
        "sass:math",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  _arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let numbers = args[0].as_list(arena)?;
                if numbers.is_empty() {
                    return Err(Box::new(SassError::Script {
                        message: "At least one argument must be passed.".into(),
                        argument_name: None,
                    }));
                }
                let mut min_idx: Option<usize> = None;
                for (i, v) in numbers.iter().enumerate() {
                    assert_number(v, None)?;
                    match min_idx {
                        None => min_idx = Some(i),
                        Some(mi) => {
                            let gt = numbers[mi].greater_than(arena, v)?;
                            if gt.is_truthy() {
                                min_idx = Some(i);
                            }
                        }
                    }
                }
                let mi = min_idx.expect("numbers is non-empty");
                Ok(numbers[mi])
            },
        ),
    )
}

/// Creates the `math.percentage` callable (`$number`).
///
/// Requires a unitless `$number` and returns `value * 100` with `%` units
/// (math.dart:224-228).
fn percentage_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "percentage",
        "$number",
        "sass:math",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let number = assert_number(&args[0], Some("number"))?;
                number.assert_no_units(Some("number"))?;
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Number(SassNumber::new(number.value * 100.0, Some("%"))),
                ))
            },
        ),
    )
}

// ---- Module functions ----
//
// Ports Dart's section groups (math.dart:75-222): bounding, distance,
// exponential, trigonometric, unit, and other functions.

/// Creates the `math.acos` callable (`$number`).
///
/// Inverse cosine in degrees; `$number` must be unitless (math.dart:182).
/// Angle conversion lives in [`acos_number`].
fn acos_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    single_argument_math_func("acos", arena, acos_number)
}

/// Creates the `math.asin` callable (`$number`).
///
/// Inverse sine in degrees; `$number` must be unitless (math.dart:184).
/// Angle conversion lives in [`asin_number`].
fn asin_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    single_argument_math_func("asin", arena, asin_number)
}

/// Creates the `math.atan` callable (`$number`).
///
/// Inverse tangent in degrees; `$number` must be unitless (math.dart:186).
/// Angle conversion lives in [`atan_number`].
fn atan_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    single_argument_math_func("atan", arena, atan_number)
}

/// Creates the `math.atan2` callable (`$y, $x`).
///
/// Two-argument arctangent in degrees; `$x` is converted to `$y`'s units
/// (math.dart:188-192). Conversion lives in [`atan2_number`].
fn atan2_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "atan2",
        "$y, $x",
        "sass:math",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let y = assert_number(&args[0], Some("y"))?;
                let x = assert_number(&args[1], Some("x"))?;
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Number(atan2_number(y, x)?),
                ))
            },
        ),
    )
}

/// Creates the `math.clamp` callable (`$min, $number, $max`).
///
/// Returns `$min` when `$min >= $max` or `$min >= $number`, `$max` when
/// `$number >= $max`, else `$number`. Unit checks run through
/// `convert_value_to_match` for parameter-name context even though the
/// converted values are unused (math.dart:81-96).
fn clamp_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "clamp",
        "$min, $number, $max",
        "sass:math",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  _arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let min = assert_number(&args[0], Some("min"))?;
                let number = assert_number(&args[1], Some("number"))?;
                let max = assert_number(&args[2], Some("max"))?;
                // Even though we don't use the resulting values,
                // `convert_value_to_match` generates more user-friendly
                // exceptions than `greater_than_or_equals` since it has more
                // context about parameter names.
                number.convert_value_to_match(min, Some("number"), Some("min"))?;
                max.convert_value_to_match(min, Some("max"), Some("min"))?;
                if min.greater_than_or_equals_num(max)? {
                    return Ok(args[0]);
                }
                if min.greater_than_or_equals_num(number)? {
                    return Ok(args[0]);
                }
                if number.greater_than_or_equals_num(max)? {
                    return Ok(args[2]);
                }
                Ok(args[1])
            },
        ),
    )
}

/// Creates the `math.cos` callable (`$number`).
///
/// Cosine; `$number` is coerced to radians and the result is unitless
/// (math.dart:194). Coercion lives in [`cos_number`].
fn cos_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    single_argument_math_func("cos", arena, cos_number)
}

/// Creates the `math.div` callable (`$number1, $number2`).
///
/// Divides `$number1` by `$number2`. Non-number arguments warn that only
/// numbers will be supported in the future (use `list.slash()` for a slash
/// separator) and still divide (math.dart:259-271). Declared last in the
/// module so it sorts last in `meta.module-functions("math")`.
fn div_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "div",
        "$number1, $number2",
        "sass:math",
        arena,
        Rc::new(
            move |config: &EvalConfig<'compile, 'parse>,
                  state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  _arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let ok1 = matches!(&*args[0], ValueKind::Number(_));
                let ok2 = matches!(&*args[1], ValueKind::Number(_));
                if !ok1 || !ok2 {
                    let span = state
                        .import_span
                        .or(state.callable_span)
                        .unwrap_or(state.default_warn_span);
                    warn(
                        config,
                        state,
                        "math.div() will only support number arguments in a future release.\n\
                         Use list.slash() instead for a slash separator.",
                        span,
                    )?;
                }
                args[0].divided_by(arena, &args[1])
            },
        ),
    )
}

/// Creates the `math.hypot` callable (`$numbers...`).
///
/// Euclidean length: each argument is converted to the first argument's
/// units (with `numbers[i]`/`numbers[1]` parameter context) and the result
/// carries the first argument's units; throws `At least one argument must
/// be passed.` when empty (math.dart:126-148).
fn hypot_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "hypot",
        "$numbers...",
        "sass:math",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let numbers = args[0].as_list(arena)?;
                if numbers.is_empty() {
                    return Err(Box::new(SassError::Script {
                        message: "At least one argument must be passed.".into(),
                        argument_name: None,
                    }));
                }
                let mut parsed: Vec<SassNumber> = Vec::with_capacity(numbers.len());
                for v in &numbers {
                    parsed.push(assert_number(v, None)?.clone());
                }
                let mut subtotal = 0.0;
                for (i, n) in parsed.iter().enumerate() {
                    let name = format!("numbers[{}]", i + 1);
                    let val =
                        n.convert_value_to_match(&parsed[0], Some(&name), Some("numbers[1]"))?;
                    subtotal += pow(val, 2.0);
                }
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Number(SassNumber::with_units(
                        sqrt(subtotal),
                        parsed[0].numerator_units.clone(),
                        parsed[0].denominator_units.clone(),
                    )),
                ))
            },
        ),
    )
}

/// Standalone is-unitless callable. Unused by the module/global lists (they
/// use `is_unitless_module_function` / `unitless_function_global`), but kept
/// to mirror the Go source structure.
///
/// Matches Go: isUnitlessFunction
#[allow(dead_code)]
fn is_unitless_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "is-unitless",
        "$number",
        "sass:math",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let number = assert_number(&args[0], Some("number"))?;
                if !number.has_units() {
                    return Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_TRUE)));
                }
                Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)))
            },
        ),
    )
}

/// Creates the `math.log` callable (`$number, $base: null`).
///
/// Natural logarithm of a unitless `$number`, or logarithm to `$base` when
/// given; both must be unitless. `$number` units are checked before `$base`
/// is asserted (math.dart:154-168); the arithmetic lives in [`log_number`].
fn log_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "log",
        "$number, $base: null",
        "sass:math",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let number = assert_number(&args[0], Some("number"))?;
                // Dart checks `$number` units before asserting `$base`
                // (math.dart:154-168); `log_number` preserves that order.
                let base_kind;
                let base = if !matches!(&*args[1], ValueKind::Null) {
                    base_kind = (*args[1]).clone();
                    Some(&base_kind)
                } else {
                    None
                };
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Number(log_number(number, base)?),
                ))
            },
        ),
    )
}

/// Creates the `math.pow` callable (`$base, $exponent`).
///
/// Both arguments must be unitless; delegates to [`pow_number`]
/// (math.dart:170-174).
fn pow_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "pow",
        "$base, $exponent",
        "sass:math",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let base = assert_number(&args[0], Some("base"))?;
                let exponent = assert_number(&args[1], Some("exponent"))?;
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Number(pow_number(base, exponent)?),
                ))
            },
        ),
    )
}

/// Creates the `math.sin` callable (`$number`).
///
/// Sine; `$number` is coerced to radians and the result is unitless
/// (math.dart:196). Coercion lives in [`sin_number`].
fn sin_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    single_argument_math_func("sin", arena, sin_number)
}

/// Creates the `math.sqrt` callable (`$number`).
///
/// Square root; `$number` must be unitless (math.dart:176). Delegates to
/// [`sqrt_number`].
fn sqrt_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    single_argument_math_func("sqrt", arena, sqrt_number)
}

/// Creates the `math.tan` callable (`$number`).
///
/// Tangent; `$number` is coerced to radians and the result is unitless
/// (math.dart:198). Coercion lives in [`tan_number`].
fn tan_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    single_argument_math_func("tan", arena, tan_number)
}

/// Creates the `math.unit` callable (`$number`).
///
/// Returns the unit string of `$number` as a quoted string, `""` when
/// unitless (math.dart:215-218). Formatting lives in [`unit_string`].
fn unit_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "unit",
        "$number",
        "sass:math",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let number = assert_number(&args[0], Some("number"))?;
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::String(SassString::new(arena.alloc_str(&unit_string(number)), true)),
                ))
            },
        ),
    )
}

/// Renders the unit string of `n` (`"px"`, `"px*em"`, `"px/s"`,
/// `"px/(s*ms)"`, `"s^-1"`, `"(s*ms)^-1"`, `""` when unitless).
///
/// Ports Dart's `SassNumber.unitString` combination logic as used by `unit`
/// (math.dart:215-218); the `^-1` denominator forms live only here, not in
/// `number_util`'s `unit_string`.
///
/// Matches Go: unitString (in math.go — distinct from number_util's
/// unit_string, whose "^-1" denominator forms only appear here)
fn unit_string(n: &SassNumber) -> String {
    let num_units = &n.numerator_units;
    let den_units = &n.denominator_units;
    if num_units.is_empty() && den_units.is_empty() {
        return String::new();
    }
    if num_units.is_empty() {
        if den_units.len() == 1 {
            return format!("{}^-1", den_units[0]);
        }
        return format!("({})^-1", join_strings(den_units, "*"));
    }
    if den_units.is_empty() {
        return join_strings(num_units, "*");
    }
    if den_units.len() == 1 {
        return format!("{}/{}", join_strings(num_units, "*"), den_units[0]);
    }
    format!(
        "{}/({})",
        join_strings(num_units, "*"),
        join_strings(den_units, "*")
    )
}

/// Joins unit names with `sep`.
///
/// Rust rendering of Dart string interpolation over unit lists; no Dart
/// counterpart of its own.
///
/// Matches Go: joinStrings
fn join_strings(strs: &[String], sep: &str) -> String {
    let mut result = String::new();
    for (i, s) in strs.iter().enumerate() {
        if i > 0 {
            result.push_str(sep);
        }
        result.push_str(s);
    }
    result
}

/// Creates the global `comparable` callable (`$number1, $number2`).
///
/// Deprecated alias of `math.compatible` (Dart's
/// `_compatible.withDeprecationWarning('math').withName("comparable")`,
/// math.dart:51).
fn comparable_function_global<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    // Dart: _compatible.withDeprecationWarning('math').withName("comparable")
    BuiltInCallable::function(
        "comparable",
        "$number1, $number2",
        "sass:math",
        arena,
        Rc::new(compatible_impl),
    )
}

/// Creates the `math.compatible` callable (`$number1, $number2`).
///
/// Returns whether the two numbers have comparable units
/// (math.dart:204-208). Shared implementation in [`compatible_impl`].
fn compatible_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    // Dart: module uses internal name "compatible"
    BuiltInCallable::function(
        "compatible",
        "$number1, $number2",
        "sass:math",
        arena,
        Rc::new(compatible_impl),
    )
}

/// Implements `compatible`/`comparable`: asserts `$number1`/`$number2` and
/// returns whether they are comparable (math.dart:204-208).
fn compatible_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let number1 = assert_number(&args[0], Some("number1"))?;
    let number2 = assert_number(&args[1], Some("number2"))?;
    if number1.has_compatible_units(number2) {
        return Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_TRUE)));
    }
    if number1.greater_than_num(number2).is_ok() {
        return Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_TRUE)));
    }
    Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)))
}

/// Creates the global `unitless` callable (`$number`).
///
/// Deprecated alias of `math.is-unitless` (Dart's
/// `_isUnitless.withDeprecationWarning('math').withName("unitless")`,
/// math.dart:52).
fn unitless_function_global<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    // Dart: _isUnitless.withDeprecationWarning('math').withName("unitless")
    BuiltInCallable::function(
        "unitless",
        "$number",
        "sass:math",
        arena,
        Rc::new(unitless_impl),
    )
}

/// Creates the `math.is-unitless` callable (`$number`).
///
/// Returns whether `$number` has no units (math.dart:210-213). Shared
/// implementation in [`unitless_impl`].
fn is_unitless_module_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    // Dart: module uses internal name "is-unitless"
    BuiltInCallable::function(
        "is-unitless",
        "$number",
        "sass:math",
        arena,
        Rc::new(unitless_impl),
    )
}

/// Implements `is-unitless`/`unitless`: asserts `$number` and returns
/// whether it has no units (math.dart:210-213).
fn unitless_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let number = assert_number(&args[0], Some("number"))?;
    if !number.has_units() {
        return Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_TRUE)));
    }
    Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)))
}

/// Creates the `math.random` callable (`$limit: null`).
///
/// Without `$limit` returns a unitless double in `[0, 1)`. With `$limit`,
/// warns FUNCTION_UNITS when it has units (the units are ignored), asserts
/// an integer `$limit`, throws `$limit: Must be greater than 0, was ...`
/// when `< 1`, and returns an integer in `[1, $limit]`
/// (math.dart:232-257).
fn random_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "random",
        "$limit: null",
        "sass:math",
        arena,
        Rc::new(
            move |config: &EvalConfig<'compile, 'parse>,
                  state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                if matches!(&*args[0], ValueKind::Null) {
                    return Ok(ValueKind::unitless_number(arena, random_float()));
                }
                let limit = assert_number(&args[0], Some("limit"))?;
                if limit.has_units() {
                    let lim_str = limit.to_display_string()?;
                    let us = unit_string(limit);
                    warn_deprecation(config, state,
                        &format!(
                            "math.random() will no longer ignore $limit units ({lim_str}) in a future release.\n\
                             \n\
                             Recommendation: math.random(math.div($limit, 1{us})) * 1{us}\n\
                             \n\
                             To preserve current behavior: math.random(math.div($limit, 1{us}))\n\
                             \n\
                             More info: https://sass-lang.com/d/function-units"
                        ),
                        &FUNCTION_UNITS,
                    )?;
                }
                let limit_scalar = limit.assert_int(Some("limit"))?;
                if limit_scalar < 1 {
                    let lim_str = limit.to_display_string()?;
                    return Err(Box::new(SassError::Script {
                        message: format!("Must be greater than 0, was {lim_str}."),
                        argument_name: Some("limit".into()),
                    }));
                }
                Ok(ValueKind::unitless_number(
                    arena,
                    (random_int(limit_scalar) + 1) as f64,
                ))
            },
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile_string;
    use crate::compile::CompileOptions;
    use crate::io::Io;
    use crate::io::VirtualIo;
    use crate::serialize::serialize_value_inspect;
    use std::f64::consts::E;
    use std::f64::consts::FRAC_PI_2;
    use std::f64::consts::PI;

    use crate::functions::test_utils::{
        assert_is_false, assert_is_true, assert_no_warnings, assert_single_warning, eval,
        eval_recorded as eval_warn,
    };
    use crate::value::{ListSeparator, SassList};

    // --- helpers ---

    fn num<'compile: 'parse, 'parse>(arena: &'compile Bump, v: f64) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(v, None)))
    }

    fn unit_num<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        v: f64,
        unit: &str,
    ) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(v, Some(unit))))
    }

    fn complex_num<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        v: f64,
        num_units: &[&str],
        den_units: &[&str],
    ) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::Number(SassNumber::with_units(
                v,
                num_units.iter().map(|s| s.to_string()).collect(),
                den_units.iter().map(|s| s.to_string()).collect(),
            )),
        )
    }

    fn string<'compile: 'parse, 'parse>(arena: &'compile Bump, s: &str) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(s), false)),
        )
    }

    fn quoted<'compile: 'parse, 'parse>(arena: &'compile Bump, s: &str) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(s), true)),
        )
    }

    fn comma_list<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        items: Vec<Value<'parse>>,
    ) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::List(SassList::new(items, ListSeparator::Comma, false)),
        )
    }

    fn inspect(v: &Value<'_>) -> String {
        // Mirrors Go: value.SerializeValueInspect (math_test.go assertions).
        serialize_value_inspect(v).unwrap()
    }

    fn assert_inspect(got: &Value<'_>, want: &str) {
        assert_eq!(inspect(got), want);
    }

    /// Go's SassScriptException.Error() string: "$name: message" when an
    /// argument name is set, otherwise the message.
    fn go_err_string(err: &SassError) -> String {
        match err {
            SassError::Script { .. } => err.full_message(),
            other => panic!("expected Script error, got {other:?}"),
        }
    }

    fn assert_err_msg(err: Box<SassError>, want: &str) {
        assert_eq!(go_err_string(&err), want);
    }

    // --- global_math_functions ---

    #[rust_sass_macros::maybe_test]
    async fn test_global_math_functions_names() {
        let arena = Bump::new();
        let fns = global_math_functions(&arena);
        let want = [
            "abs",
            "ceil",
            "floor",
            "max",
            "min",
            "percentage",
            "random",
            "round",
            "unit",
            "comparable",
            "unitless",
        ];
        assert_eq!(fns.len(), want.len());
        for (i, name) in want.iter().enumerate() {
            assert_eq!(fns[i].name(), *name, "fns[{i}]");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_math_functions_deprecation_metadata() {
        let arena = Bump::new();
        // Each wrapped global also stores (module, name) metadata for
        // introspection; the warning itself is emitted by the wrapped
        // callback (see test_global_math_functions_emit_deprecation_warnings).
        let fns = global_math_functions(&arena);
        let want: [(usize, &str, &str); 10] = [
            (1, "ceil", "ceil"),
            (2, "floor", "floor"),
            (3, "max", "max"),
            (4, "min", "min"),
            (5, "percentage", "percentage"),
            (6, "random", "random"),
            (7, "round", "round"),
            (8, "unit", "unit"),
            (9, "comparable", "compatible"),
            (10, "unitless", "is-unitless"),
        ];
        for (i, name, dep_name) in want {
            let CallableKind::BuiltIn(b) = fns[i].kind() else {
                panic!("expected BuiltIn callable");
            };
            assert_eq!(b.name(), name);
            let dw = b
                .deprecation_warning()
                .expect("deprecation warning should be set");
            assert_eq!(dw.0, "math");
            assert_eq!(dw.1, dep_name);
        }
        // abs warns inline (percent vs global-builtin), not via the wrapper.
        let CallableKind::BuiltIn(abs) = fns[0].kind() else {
            panic!("expected BuiltIn callable");
        };
        assert!(abs.deprecation_warning().is_none());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_math_functions_emit_deprecation_warnings() {
        let arena = Bump::new();
        // Mirrors Go: TestGlobalMathFunctionsEmitDeprecationWarnings — every
        // wrapped global emits the GlobalBuiltin warning naming the module
        // function, then delegates to the original callback.
        let fns = global_math_functions(&arena);
        let args = [
            (1, "math.ceil", vec![num(&arena, 4.2)], "5"),
            (2, "math.floor", vec![num(&arena, 4.8)], "4"),
            (
                3,
                "math.max",
                vec![comma_list(&arena, vec![num(&arena, 1.0)])],
                "1",
            ),
            (
                4,
                "math.min",
                vec![comma_list(&arena, vec![num(&arena, 1.0)])],
                "1",
            ),
            (5, "math.percentage", vec![num(&arena, 0.5)], "50%"),
            (
                6,
                "math.random",
                vec![Value::new_with_arena(&arena, ValueKind::Null)],
                "",
            ),
            (7, "math.round", vec![num(&arena, 1.4)], "1"),
            (8, "math.unit", vec![num(&arena, 1.0)], "\"\""),
            (
                9,
                "math.compatible",
                vec![num(&arena, 1.0), num(&arena, 2.0)],
                "true",
            ),
            (10, "math.is-unitless", vec![num(&arena, 1.0)], "true"),
        ];
        for (i, dep_name, a, want) in args {
            let CallableKind::BuiltIn(b) = fns[i].kind() else {
                panic!("expected BuiltIn callable");
            };
            let (result, logger) = eval_warn(&arena, b, &a).await;
            let got = result.unwrap();
            if want.is_empty() {
                // random(): just check it's a number in [0, 1).
                let ValueKind::Number(n) = &*got else {
                    panic!("expected number");
                };
                assert!(n.value >= 0.0 && n.value < 1.0);
            } else {
                assert_inspect(&got, want);
            }
            let want_warning = [
                "Global built-in functions are deprecated and will be removed in Dart Sass 3.0.0.",
                &format!("Use {dep_name} instead."),
                "",
                "More info and automated migrator: https://sass-lang.com/d/import",
            ]
            .join("\n");
            assert_single_warning(&logger, &want_warning, Some("global-builtin"));
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_math_abs_warns_global_builtin() {
        let arena = Bump::new();
        let fns = global_math_functions(&arena);
        let CallableKind::BuiltIn(abs) = fns[0].kind() else {
            panic!("expected BuiltIn callable");
        };
        let (result, logger) = eval_warn(&arena, abs, &[unit_num(&arena, -3.0, "px")]).await;
        let got = result.unwrap();
        assert_inspect(&got, "3px");
        let want = [
            "Global built-in functions are deprecated and will be removed in Dart Sass 3.0.0.",
            "Use math.abs instead.",
            "",
            "More info and automated migrator: https://sass-lang.com/d/import",
        ]
        .join("\n");
        assert_single_warning(&logger, &want, Some("global-builtin"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_math_abs_percent_deprecation() {
        let arena = Bump::new();
        let fns = global_math_functions(&arena);
        let CallableKind::BuiltIn(abs) = fns[0].kind() else {
            panic!("expected BuiltIn callable");
        };
        let (result, logger) = eval_warn(&arena, abs, &[unit_num(&arena, -50.0, "%")]).await;
        let got = result.unwrap();
        assert_inspect(&got, "50%");
        let want = [
            "Passing percentage units to the global abs() function is deprecated.",
            "In the future, this will emit a CSS abs() function to be resolved by the browser.",
            "To preserve current behavior: math.abs(-50%)",
            "To emit a CSS abs() now: abs(#{-50%})",
            "More info: https://sass-lang.com/d/abs-percent",
        ]
        .join("\n");
        assert_single_warning(&logger, &want, Some("abs-percent"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_math_abs_non_number() {
        let arena = Bump::new();
        let fns = global_math_functions(&arena);
        let CallableKind::BuiltIn(abs) = fns[0].kind() else {
            panic!("expected BuiltIn callable");
        };
        let (result, _) = eval_warn(&arena, abs, &[string(&arena, "foo")]).await;
        assert_err_msg(result.unwrap_err(), "$number: foo is not a number.");
    }

    // --- math_module ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_module_url() {
        let arena = Bump::new();
        let m = math_module(&arena);
        assert_eq!(m.url, "sass:math");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_module_function_names() {
        let arena = Bump::new();
        let m = math_module(&arena);
        // Dart declares `_div` last (math.dart:60-62).
        let want = [
            "abs",
            "acos",
            "asin",
            "atan",
            "atan2",
            "ceil",
            "clamp",
            "cos",
            "compatible",
            "floor",
            "hypot",
            "is-unitless",
            "log",
            "max",
            "min",
            "percentage",
            "pow",
            "random",
            "round",
            "sin",
            "sqrt",
            "tan",
            "unit",
            "div",
        ];
        assert_eq!(m.functions.len(), want.len());
        for (i, (name, _)) in m.functions.iter().enumerate() {
            assert_eq!(name, want[i], "functions[{i}]");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_module_no_mixins() {
        let arena = Bump::new();
        let m = math_module(&arena);
        assert_eq!(m.mixins.len(), 0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_module_variables() {
        let arena = Bump::new();
        let m = math_module(&arena);
        let want_order = [
            "e",
            "pi",
            "epsilon",
            "max-safe-integer",
            "min-safe-integer",
            "max-number",
            "min-number",
        ];
        assert_eq!(m.variables.len(), want_order.len());
        for (i, (name, _)) in m.variables.iter().enumerate() {
            assert_eq!(name, want_order[i], "variables[{i}]");
        }
        let want_values: [(&str, f64); 7] = [
            ("e", E),
            ("pi", PI),
            ("epsilon", 2.220446049250313e-16),
            ("max-safe-integer", 9007199254740991.0),
            ("min-safe-integer", -9007199254740991.0),
            ("max-number", f64::MAX),
            ("min-number", 5e-324),
        ];
        for (name, want_val) in want_values {
            let v = m.variables.get(name).unwrap();
            let ValueKind::Number(n) = &**v else {
                panic!("variable {name} is not a number");
            };
            assert_eq!(n.value, want_val, "variable {name}");
            assert!(!n.has_units(), "variable {name} should be unitless");
        }
        // min-number is the smallest positive subnormal (Go's
        // math.SmallestNonzeroFloat64), not Rust's f64::MIN_POSITIVE.
        let ValueKind::Number(n) = &**m.variables.get("min-number").unwrap() else {
            panic!("expected number");
        };
        assert_eq!(n.value.to_bits(), 1);
    }

    // --- abs (module) ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_abs() {
        let arena = Bump::new();
        let got = eval(
            &arena,
            &abs_function(&arena),
            &[unit_num(&arena, -10.0, "px")],
        )
        .await
        .unwrap();
        assert_inspect(&got, "10px");

        let got = eval(
            &arena,
            &abs_function(&arena),
            &[unit_num(&arena, 10.0, "px")],
        )
        .await
        .unwrap();
        assert_inspect(&got, "10px");

        let got = eval(&arena, &abs_function(&arena), &[num(&arena, -5.0)])
            .await
            .unwrap();
        assert_inspect(&got, "5");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_abs_preserves_complex_units() {
        let arena = Bump::new();
        let got = eval(
            &arena,
            &abs_function(&arena),
            &[complex_num(&arena, -5.0, &["px"], &["s"])],
        )
        .await
        .unwrap();
        assert_inspect(&got, "calc(5px / 1s)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_abs_module_does_not_warn() {
        let arena = Bump::new();
        let (result, logger) = eval_warn(
            &arena,
            &abs_function(&arena),
            &[unit_num(&arena, -50.0, "%")],
        )
        .await;
        result.unwrap();
        assert_no_warnings(&logger);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_abs_non_number() {
        let arena = Bump::new();
        let err = eval(&arena, &abs_function(&arena), &[string(&arena, "foo")])
            .await
            .unwrap_err();
        assert_err_msg(err, "$number: foo is not a number.");
    }

    // --- ceil ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_ceil() {
        let arena = Bump::new();
        let got = eval(&arena, &ceil_function(&arena), &[num(&arena, 4.2)])
            .await
            .unwrap();
        assert_inspect(&got, "5");

        let got = eval(
            &arena,
            &ceil_function(&arena),
            &[unit_num(&arena, 4.2, "px")],
        )
        .await
        .unwrap();
        assert_inspect(&got, "5px");

        let got = eval(&arena, &ceil_function(&arena), &[num(&arena, -4.2)])
            .await
            .unwrap();
        assert_inspect(&got, "-4");

        let got = eval(&arena, &ceil_function(&arena), &[num(&arena, 4.0)])
            .await
            .unwrap();
        assert_inspect(&got, "4");

        // Dart's int round-trip yields +0.0, never -0.0 (see `dart_int_op`).
        let got = eval(&arena, &ceil_function(&arena), &[num(&arena, -0.1)])
            .await
            .unwrap();
        assert_inspect(&got, "0");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_ceil_preserves_complex_units() {
        let arena = Bump::new();
        let got = eval(
            &arena,
            &ceil_function(&arena),
            &[complex_num(&arena, 5.5, &["px"], &["s"])],
        )
        .await
        .unwrap();
        assert_inspect(&got, "calc(6px / 1s)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_ceil_non_number() {
        let arena = Bump::new();
        let err = eval(&arena, &ceil_function(&arena), &[string(&arena, "foo")])
            .await
            .unwrap_err();
        assert_err_msg(err, "$number: foo is not a number.");
    }

    // --- floor ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_floor() {
        let arena = Bump::new();
        let got = eval(&arena, &floor_function(&arena), &[num(&arena, 4.8)])
            .await
            .unwrap();
        assert_inspect(&got, "4");

        let got = eval(
            &arena,
            &floor_function(&arena),
            &[unit_num(&arena, -4.2, "px")],
        )
        .await
        .unwrap();
        assert_inspect(&got, "-5px");

        let got = eval(&arena, &floor_function(&arena), &[num(&arena, 4.0)])
            .await
            .unwrap();
        assert_inspect(&got, "4");

        let got = eval(&arena, &floor_function(&arena), &[num(&arena, 0.9)])
            .await
            .unwrap();
        assert_inspect(&got, "0");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_floor_non_number() {
        let arena = Bump::new();
        let err = eval(&arena, &floor_function(&arena), &[string(&arena, "foo")])
            .await
            .unwrap_err();
        assert_err_msg(err, "$number: foo is not a number.");
    }

    // --- round ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_round() {
        let arena = Bump::new();
        let got = eval(&arena, &round_function(&arena), &[num(&arena, 2.5)])
            .await
            .unwrap();
        assert_inspect(&got, "3");

        let got = eval(&arena, &round_function(&arena), &[num(&arena, -2.5)])
            .await
            .unwrap();
        assert_inspect(&got, "-3");

        let got = eval(&arena, &round_function(&arena), &[num(&arena, 2.4)])
            .await
            .unwrap();
        assert_inspect(&got, "2");

        let got = eval(
            &arena,
            &round_function(&arena),
            &[unit_num(&arena, 2.6, "px")],
        )
        .await
        .unwrap();
        assert_inspect(&got, "3px");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_round_half_cases() {
        let arena = Bump::new();
        // Rounds half away from zero.
        let tests: [(f64, &str); 4] = [(0.5, "1"), (-0.5, "-1"), (1.5, "2"), (-1.5, "-2")];
        for (input, want) in tests {
            let got = eval(&arena, &round_function(&arena), &[num(&arena, input)])
                .await
                .unwrap();
            assert_inspect(&got, want);
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_round_non_number() {
        let arena = Bump::new();
        let err = eval(&arena, &round_function(&arena), &[string(&arena, "foo")])
            .await
            .unwrap_err();
        assert_err_msg(err, "$number: foo is not a number.");
    }

    // --- max ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_max() {
        let arena = Bump::new();
        let got = eval(
            &arena,
            &max_function(&arena),
            &[comma_list(
                &arena,
                vec![num(&arena, 1.0), num(&arena, 2.0), num(&arena, 3.0)],
            )],
        )
        .await
        .unwrap();
        assert_inspect(&got, "3");

        let got = eval(
            &arena,
            &max_function(&arena),
            &[comma_list(
                &arena,
                vec![unit_num(&arena, 1.0, "px"), unit_num(&arena, 2.0, "px")],
            )],
        )
        .await
        .unwrap();
        assert_inspect(&got, "2px");

        // 1in == 96px > 50px: keeps original 1in.
        let got = eval(
            &arena,
            &max_function(&arena),
            &[comma_list(
                &arena,
                vec![unit_num(&arena, 1.0, "in"), unit_num(&arena, 50.0, "px")],
            )],
        )
        .await
        .unwrap();
        assert_inspect(&got, "1in");

        // Unitless coerces to unitful.
        let got = eval(
            &arena,
            &max_function(&arena),
            &[comma_list(
                &arena,
                vec![num(&arena, 1.0), unit_num(&arena, 2.0, "px")],
            )],
        )
        .await
        .unwrap();
        assert_inspect(&got, "2px");

        let got = eval(
            &arena,
            &max_function(&arena),
            &[comma_list(&arena, vec![unit_num(&arena, 1.0, "px")])],
        )
        .await
        .unwrap();
        assert_inspect(&got, "1px");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_max_empty() {
        let arena = Bump::new();
        let err = eval(&arena, &max_function(&arena), &[comma_list(&arena, vec![])])
            .await
            .unwrap_err();
        assert_err_msg(err, "At least one argument must be passed.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_max_non_number_element() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &max_function(&arena),
            &[comma_list(
                &arena,
                vec![num(&arena, 1.0), string(&arena, "foo")],
            )],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "foo is not a number.");

        let err = eval(
            &arena,
            &max_function(&arena),
            &[comma_list(
                &arena,
                vec![num(&arena, 1.0), quoted(&arena, "foo")],
            )],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "\"foo\" is not a number.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_max_incompatible_units() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &max_function(&arena),
            &[comma_list(
                &arena,
                vec![unit_num(&arena, 1.0, "px"), unit_num(&arena, 2.0, "s")],
            )],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "1px and 2s have incompatible units.");
    }

    // --- min ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_min() {
        let arena = Bump::new();
        let got = eval(
            &arena,
            &min_function(&arena),
            &[comma_list(
                &arena,
                vec![num(&arena, 3.0), num(&arena, 1.0), num(&arena, 2.0)],
            )],
        )
        .await
        .unwrap();
        assert_inspect(&got, "1");

        // 1000ms < 2s: keeps original 1000ms.
        let got = eval(
            &arena,
            &min_function(&arena),
            &[comma_list(
                &arena,
                vec![unit_num(&arena, 2.0, "s"), unit_num(&arena, 1000.0, "ms")],
            )],
        )
        .await
        .unwrap();
        assert_inspect(&got, "1000ms");

        let got = eval(
            &arena,
            &min_function(&arena),
            &[comma_list(&arena, vec![unit_num(&arena, 3.0, "px")])],
        )
        .await
        .unwrap();
        assert_inspect(&got, "3px");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_min_empty() {
        let arena = Bump::new();
        let err = eval(&arena, &min_function(&arena), &[comma_list(&arena, vec![])])
            .await
            .unwrap_err();
        assert_err_msg(err, "At least one argument must be passed.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_min_non_number_element() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &min_function(&arena),
            &[comma_list(
                &arena,
                vec![num(&arena, 1.0), string(&arena, "foo")],
            )],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "foo is not a number.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_min_incompatible_units() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &min_function(&arena),
            &[comma_list(
                &arena,
                vec![unit_num(&arena, 1.0, "px"), unit_num(&arena, 2.0, "s")],
            )],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "1px and 2s have incompatible units.");
    }

    // --- percentage ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_percentage() {
        let arena = Bump::new();
        let got = eval(&arena, &percentage_function(&arena), &[num(&arena, 0.5)])
            .await
            .unwrap();
        assert_inspect(&got, "50%");

        let got = eval(&arena, &percentage_function(&arena), &[num(&arena, 1.0)])
            .await
            .unwrap();
        assert_inspect(&got, "100%");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_percentage_with_units() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &percentage_function(&arena),
            &[unit_num(&arena, 0.5, "px")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$number: Expected 0.5px to have no units.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_percentage_non_number() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &percentage_function(&arena),
            &[string(&arena, "foo")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$number: foo is not a number.");
    }

    // --- acos ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_acos() {
        let arena = Bump::new();
        let got = eval(&arena, &acos_function(&arena), &[num(&arena, 1.0)])
            .await
            .unwrap();
        assert_inspect(&got, "0deg");

        let got = eval(&arena, &acos_function(&arena), &[num(&arena, -1.0)])
            .await
            .unwrap();
        assert_inspect(&got, "180deg");

        // Out of domain: NaN with deg unit.
        let got = eval(&arena, &acos_function(&arena), &[num(&arena, 2.0)])
            .await
            .unwrap();
        assert_inspect(&got, "calc(NaN * 1deg)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_acos_with_units() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &acos_function(&arena),
            &[unit_num(&arena, 2.0, "px")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$number: Expected 2px to have no units.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_acos_non_number() {
        let arena = Bump::new();
        let err = eval(&arena, &acos_function(&arena), &[string(&arena, "foo")])
            .await
            .unwrap_err();
        assert_err_msg(err, "$number: foo is not a number.");
    }

    // --- asin ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_asin() {
        let arena = Bump::new();
        let got = eval(&arena, &asin_function(&arena), &[num(&arena, 0.0)])
            .await
            .unwrap();
        assert_inspect(&got, "0deg");

        let got = eval(&arena, &asin_function(&arena), &[num(&arena, 1.0)])
            .await
            .unwrap();
        assert_inspect(&got, "90deg");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_asin_with_units() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &asin_function(&arena),
            &[unit_num(&arena, 1.0, "px")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$number: Expected 1px to have no units.");
    }

    // --- atan ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_atan() {
        let arena = Bump::new();
        let got = eval(&arena, &atan_function(&arena), &[num(&arena, 0.0)])
            .await
            .unwrap();
        assert_inspect(&got, "0deg");

        let got = eval(&arena, &atan_function(&arena), &[num(&arena, 1.0)])
            .await
            .unwrap();
        assert_inspect(&got, "45deg");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_atan_with_units() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &atan_function(&arena),
            &[unit_num(&arena, 1.0, "px")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$number: Expected 1px to have no units.");
    }

    // --- atan2 ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_atan2() {
        let arena = Bump::new();
        let got = eval(
            &arena,
            &atan2_function(&arena),
            &[num(&arena, 1.0), num(&arena, 0.0)],
        )
        .await
        .unwrap();
        assert_inspect(&got, "90deg");

        let got = eval(
            &arena,
            &atan2_function(&arena),
            &[num(&arena, 0.0), num(&arena, -1.0)],
        )
        .await
        .unwrap();
        assert_inspect(&got, "180deg");

        // Units are converted to match: 96px == 1in.
        let got = eval(
            &arena,
            &atan2_function(&arena),
            &[unit_num(&arena, 1.0, "in"), unit_num(&arena, 96.0, "px")],
        )
        .await
        .unwrap();
        assert_inspect(&got, "45deg");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_atan2_incompatible_units() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &atan2_function(&arena),
            &[unit_num(&arena, 1.0, "px"), unit_num(&arena, 1.0, "s")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$x: 1s and $y: 1px have incompatible units.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_atan2_mixed_unitless() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &atan2_function(&arena),
            &[unit_num(&arena, 1.0, "px"), num(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_err_msg(
            err,
            "$x: 1 and $y: 1px have incompatible units (one has units and the other doesn't).",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_atan2_non_number() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &atan2_function(&arena),
            &[string(&arena, "foo"), num(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$y: foo is not a number.");

        let err = eval(
            &arena,
            &atan2_function(&arena),
            &[num(&arena, 1.0), string(&arena, "foo")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$x: foo is not a number.");
    }

    // --- clamp ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_clamp() {
        let arena = Bump::new();
        let got = eval(
            &arena,
            &clamp_function(&arena),
            &[num(&arena, 1.0), num(&arena, 2.0), num(&arena, 3.0)],
        )
        .await
        .unwrap();
        assert_inspect(&got, "2");

        let got = eval(
            &arena,
            &clamp_function(&arena),
            &[num(&arena, 1.0), num(&arena, 0.0), num(&arena, 3.0)],
        )
        .await
        .unwrap();
        assert_inspect(&got, "1");

        let got = eval(
            &arena,
            &clamp_function(&arena),
            &[num(&arena, 1.0), num(&arena, 5.0), num(&arena, 3.0)],
        )
        .await
        .unwrap();
        assert_inspect(&got, "3");

        // min >= max: returns min.
        let got = eval(
            &arena,
            &clamp_function(&arena),
            &[num(&arena, 3.0), num(&arena, 2.0), num(&arena, 1.0)],
        )
        .await
        .unwrap();
        assert_inspect(&got, "3");

        let got = eval(
            &arena,
            &clamp_function(&arena),
            &[
                unit_num(&arena, 1.0, "px"),
                unit_num(&arena, 2.0, "px"),
                unit_num(&arena, 3.0, "px"),
            ],
        )
        .await
        .unwrap();
        assert_inspect(&got, "2px");

        // Compatible units are converted: 50px < 1in (96px), so min is returned.
        let got = eval(
            &arena,
            &clamp_function(&arena),
            &[
                unit_num(&arena, 1.0, "in"),
                unit_num(&arena, 50.0, "px"),
                unit_num(&arena, 2.0, "in"),
            ],
        )
        .await
        .unwrap();
        assert_inspect(&got, "1in");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_clamp_incompatible_units() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &clamp_function(&arena),
            &[
                unit_num(&arena, 1.0, "px"),
                unit_num(&arena, 2.0, "s"),
                unit_num(&arena, 3.0, "px"),
            ],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$number: 2s and $min: 1px have incompatible units.");

        let err = eval(
            &arena,
            &clamp_function(&arena),
            &[
                unit_num(&arena, 1.0, "px"),
                unit_num(&arena, 2.0, "px"),
                unit_num(&arena, 3.0, "s"),
            ],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$max: 3s and $min: 1px have incompatible units.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_clamp_mixed_unitless() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &clamp_function(&arena),
            &[
                num(&arena, 0.0),
                unit_num(&arena, 1.0, "px"),
                num(&arena, 2.0),
            ],
        )
        .await
        .unwrap_err();
        assert_err_msg(
            err,
            "$number: 1px and $min: 0 have incompatible units (one has units and the other doesn't).",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_clamp_non_number() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &clamp_function(&arena),
            &[string(&arena, "foo"), num(&arena, 2.0), num(&arena, 3.0)],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$min: foo is not a number.");

        let err = eval(
            &arena,
            &clamp_function(&arena),
            &[num(&arena, 1.0), string(&arena, "foo"), num(&arena, 3.0)],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$number: foo is not a number.");

        let err = eval(
            &arena,
            &clamp_function(&arena),
            &[num(&arena, 1.0), num(&arena, 2.0), string(&arena, "foo")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$max: foo is not a number.");
    }

    // --- cos ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_cos() {
        let arena = Bump::new();
        let got = eval(&arena, &cos_function(&arena), &[num(&arena, 0.0)])
            .await
            .unwrap();
        assert_inspect(&got, "1");

        let got = eval(
            &arena,
            &cos_function(&arena),
            &[unit_num(&arena, 0.0, "deg")],
        )
        .await
        .unwrap();
        assert_inspect(&got, "1");

        let got = eval(
            &arena,
            &cos_function(&arena),
            &[unit_num(&arena, 180.0, "deg")],
        )
        .await
        .unwrap();
        assert_inspect(&got, "-1");

        let got = eval(&arena, &cos_function(&arena), &[num(&arena, PI)])
            .await
            .unwrap();
        assert_inspect(&got, "-1");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_cos_non_angle_unit() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &cos_function(&arena),
            &[unit_num(&arena, 10.0, "px")],
        )
        .await
        .unwrap_err();
        assert_err_msg(
            err,
            "$number: Expected 10px to have an angle unit (deg, grad, rad, turn).",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_cos_non_number() {
        let arena = Bump::new();
        let err = eval(&arena, &cos_function(&arena), &[string(&arena, "foo")])
            .await
            .unwrap_err();
        assert_err_msg(err, "$number: foo is not a number.");
    }

    // --- sin ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_sin() {
        let arena = Bump::new();
        let got = eval(&arena, &sin_function(&arena), &[num(&arena, 0.0)])
            .await
            .unwrap();
        assert_inspect(&got, "0");

        let got = eval(&arena, &sin_function(&arena), &[num(&arena, FRAC_PI_2)])
            .await
            .unwrap();
        assert_inspect(&got, "1");

        let got = eval(
            &arena,
            &sin_function(&arena),
            &[unit_num(&arena, 90.0, "deg")],
        )
        .await
        .unwrap();
        assert_inspect(&got, "1");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_sin_non_angle_unit() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &sin_function(&arena),
            &[unit_num(&arena, 10.0, "px")],
        )
        .await
        .unwrap_err();
        assert_err_msg(
            err,
            "$number: Expected 10px to have an angle unit (deg, grad, rad, turn).",
        );
    }

    // --- tan ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_tan() {
        let arena = Bump::new();
        let got = eval(&arena, &tan_function(&arena), &[num(&arena, 0.0)])
            .await
            .unwrap();
        assert_inspect(&got, "0");

        // C libm value, matching Dart and Go-with-sassmathcgo. The raw value
        // is 0.9999999999999999, but inspect serializes fuzzy integers as
        // integers (Dart `_asInt`, #2800) — Dart 1.104 prints `1` here too.
        let got = eval(
            &arena,
            &tan_function(&arena),
            &[unit_num(&arena, 45.0, "deg")],
        )
        .await
        .unwrap();
        assert_inspect(&got, "1");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_tan_non_angle_unit() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &tan_function(&arena),
            &[unit_num(&arena, 10.0, "px")],
        )
        .await
        .unwrap_err();
        assert_err_msg(
            err,
            "$number: Expected 10px to have an angle unit (deg, grad, rad, turn).",
        );
    }

    // --- compatible (module) ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_compatible() {
        let arena = Bump::new();
        let got = eval(
            &arena,
            &compatible_function(&arena),
            &[unit_num(&arena, 1.0, "px"), unit_num(&arena, 1.0, "in")],
        )
        .await
        .unwrap();
        assert_is_true(&got);

        let got = eval(
            &arena,
            &compatible_function(&arena),
            &[unit_num(&arena, 1.0, "px"), unit_num(&arena, 1.0, "s")],
        )
        .await
        .unwrap();
        assert_is_false(&got);

        let got = eval(
            &arena,
            &compatible_function(&arena),
            &[num(&arena, 1.0), unit_num(&arena, 1.0, "px")],
        )
        .await
        .unwrap();
        assert_is_true(&got);

        let got = eval(
            &arena,
            &compatible_function(&arena),
            &[
                unit_num(&arena, 1.0, "px"),
                complex_num(&arena, 1.0, &["px"], &["s"]),
            ],
        )
        .await
        .unwrap();
        assert_is_false(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_compatible_non_number() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &compatible_function(&arena),
            &[string(&arena, "foo"), num(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$number1: foo is not a number.");

        let err = eval(
            &arena,
            &compatible_function(&arena),
            &[num(&arena, 1.0), string(&arena, "foo")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$number2: foo is not a number.");
    }

    // --- div ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_div() {
        let arena = Bump::new();
        let (result, logger) = eval_warn(
            &arena,
            &div_function(&arena),
            &[num(&arena, 10.0), num(&arena, 2.0)],
        )
        .await;
        assert_inspect(&result.unwrap(), "5");
        assert_no_warnings(&logger);

        let (result, logger) = eval_warn(
            &arena,
            &div_function(&arena),
            &[unit_num(&arena, 10.0, "px"), num(&arena, 2.0)],
        )
        .await;
        assert_inspect(&result.unwrap(), "5px");
        assert_no_warnings(&logger);

        let (result, logger) = eval_warn(
            &arena,
            &div_function(&arena),
            &[unit_num(&arena, 10.0, "px"), unit_num(&arena, 5.0, "px")],
        )
        .await;
        assert_inspect(&result.unwrap(), "2");
        assert_no_warnings(&logger);

        let (result, logger) = eval_warn(
            &arena,
            &div_function(&arena),
            &[unit_num(&arena, 10.0, "px"), unit_num(&arena, 2.0, "s")],
        )
        .await;
        assert_inspect(&result.unwrap(), "calc(5px / 1s)");
        assert_no_warnings(&logger);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_div_non_number_warns() {
        let arena = Bump::new();
        let want = [
            "math.div() will only support number arguments in a future release.",
            "Use list.slash() instead for a slash separator.",
        ]
        .join("\n");

        let (result, logger) = eval_warn(
            &arena,
            &div_function(&arena),
            &[string(&arena, "a"), string(&arena, "b")],
        )
        .await;
        assert_inspect(&result.unwrap(), "a/b");
        // A plain warning (no deprecation), matching Go's WarnWithDeprecation(msg, false).
        assert_single_warning(&logger, &want, None);

        let (result, logger) = eval_warn(
            &arena,
            &div_function(&arena),
            &[num(&arena, 10.0), string(&arena, "b")],
        )
        .await;
        assert_inspect(&result.unwrap(), "10/b");
        assert_single_warning(&logger, &want, None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_div_by_zero() {
        let arena = Bump::new();
        // Division by zero yields Infinity/NaN numbers, not errors.
        let (result, logger) = eval_warn(
            &arena,
            &div_function(&arena),
            &[num(&arena, 1.0), num(&arena, 0.0)],
        )
        .await;
        assert_inspect(&result.unwrap(), "calc(infinity)");
        assert_no_warnings(&logger);

        let (result, logger) = eval_warn(
            &arena,
            &div_function(&arena),
            &[num(&arena, -1.0), num(&arena, 0.0)],
        )
        .await;
        assert_inspect(&result.unwrap(), "calc(-infinity)");
        assert_no_warnings(&logger);

        let (result, logger) = eval_warn(
            &arena,
            &div_function(&arena),
            &[num(&arena, 0.0), num(&arena, 0.0)],
        )
        .await;
        assert_inspect(&result.unwrap(), "calc(NaN)");
        assert_no_warnings(&logger);
    }

    // --- hypot ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_hypot() {
        let arena = Bump::new();
        let got = eval(
            &arena,
            &hypot_function(&arena),
            &[comma_list(&arena, vec![num(&arena, 3.0), num(&arena, 4.0)])],
        )
        .await
        .unwrap();
        assert_inspect(&got, "5");

        let got = eval(
            &arena,
            &hypot_function(&arena),
            &[comma_list(
                &arena,
                vec![unit_num(&arena, 3.0, "px"), unit_num(&arena, 4.0, "px")],
            )],
        )
        .await
        .unwrap();
        assert_inspect(&got, "5px");

        // Units are converted to match the first number's units.
        let got = eval(
            &arena,
            &hypot_function(&arena),
            &[comma_list(
                &arena,
                vec![unit_num(&arena, 1.0, "in"), unit_num(&arena, 96.0, "px")],
            )],
        )
        .await
        .unwrap();
        assert_inspect(&got, "1.4142135623730951in");

        let got = eval(
            &arena,
            &hypot_function(&arena),
            &[comma_list(&arena, vec![unit_num(&arena, 5.0, "px")])],
        )
        .await
        .unwrap();
        assert_inspect(&got, "5px");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_hypot_three_args() {
        let arena = Bump::new();
        // hypot(2, 3, 6) = sqrt(4 + 9 + 36) = 7
        let got = eval(
            &arena,
            &hypot_function(&arena),
            &[comma_list(
                &arena,
                vec![num(&arena, 2.0), num(&arena, 3.0), num(&arena, 6.0)],
            )],
        )
        .await
        .unwrap();
        assert_inspect(&got, "7");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_hypot_empty() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &hypot_function(&arena),
            &[comma_list(&arena, vec![])],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "At least one argument must be passed.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_hypot_non_number_element() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &hypot_function(&arena),
            &[comma_list(
                &arena,
                vec![num(&arena, 3.0), string(&arena, "foo")],
            )],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "foo is not a number.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_hypot_incompatible_units() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &hypot_function(&arena),
            &[comma_list(
                &arena,
                vec![unit_num(&arena, 3.0, "px"), unit_num(&arena, 4.0, "s")],
            )],
        )
        .await
        .unwrap_err();
        assert_err_msg(
            err,
            "$numbers[2]: 4s and $numbers[1]: 3px have incompatible units.",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_hypot_mixed_unitless() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &hypot_function(&arena),
            &[comma_list(
                &arena,
                vec![unit_num(&arena, 3.0, "px"), num(&arena, 4.0)],
            )],
        )
        .await
        .unwrap_err();
        assert_err_msg(
            err,
            "$numbers[2]: 4 and $numbers[1]: 3px have incompatible units (one has units and the other doesn't).",
        );
    }

    // --- is-unitless ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_is_unitless() {
        let arena = Bump::new();
        let got = eval(
            &arena,
            &is_unitless_module_function(&arena),
            &[num(&arena, 5.0)],
        )
        .await
        .unwrap();
        assert_is_true(&got);

        let got = eval(
            &arena,
            &is_unitless_module_function(&arena),
            &[unit_num(&arena, 5.0, "px")],
        )
        .await
        .unwrap();
        assert_is_false(&got);

        let got = eval(
            &arena,
            &is_unitless_module_function(&arena),
            &[complex_num(&arena, 5.0, &["px"], &["s"])],
        )
        .await
        .unwrap();
        assert_is_false(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_is_unitless_non_number() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &is_unitless_module_function(&arena),
            &[string(&arena, "foo")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$number: foo is not a number.");
    }

    // is_unitless_function is currently unused by the module/global lists but
    // kept to match the Go/Dart source structure; lock its behavior too.
    #[rust_sass_macros::maybe_test]
    async fn test_math_is_unitless_standalone_function() {
        let arena = Bump::new();
        let got = eval(&arena, &is_unitless_function(&arena), &[num(&arena, 5.0)])
            .await
            .unwrap();
        assert_is_true(&got);

        let got = eval(
            &arena,
            &is_unitless_function(&arena),
            &[unit_num(&arena, 5.0, "px")],
        )
        .await
        .unwrap();
        assert_is_false(&got);

        let err = eval(
            &arena,
            &is_unitless_function(&arena),
            &[string(&arena, "foo")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$number: foo is not a number.");
    }

    // --- log ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_log() {
        let arena = Bump::new();
        let got = eval(&arena, &log_function(&arena), &[num(&arena, E)])
            .await
            .unwrap();
        assert_inspect(&got, "1");

        let got = eval(&arena, &log_function(&arena), &[num(&arena, 1.0)])
            .await
            .unwrap();
        assert_inspect(&got, "0");

        let got = eval(
            &arena,
            &log_function(&arena),
            &[num(&arena, 100.0), num(&arena, 10.0)],
        )
        .await
        .unwrap();
        assert_inspect(&got, "2");

        let got = eval(
            &arena,
            &log_function(&arena),
            &[num(&arena, 8.0), num(&arena, 2.0)],
        )
        .await
        .unwrap();
        assert_inspect(&got, "3");

        let got = eval(&arena, &log_function(&arena), &[num(&arena, 0.0)])
            .await
            .unwrap();
        assert_inspect(&got, "calc(-infinity)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_log_with_units() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &log_function(&arena),
            &[unit_num(&arena, 10.0, "px")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$number: Expected 10px to have no units.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_log_base_with_units() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &log_function(&arena),
            &[num(&arena, 10.0), unit_num(&arena, 2.0, "px")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$base: Expected 2px to have no units.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_log_non_number() {
        let arena = Bump::new();
        let err = eval(&arena, &log_function(&arena), &[string(&arena, "foo")])
            .await
            .unwrap_err();
        assert_err_msg(err, "$number: foo is not a number.");

        let err = eval(&arena, &log_function(&arena), &[quoted(&arena, "foo")])
            .await
            .unwrap_err();
        assert_err_msg(err, "$number: \"foo\" is not a number.");

        let err = eval(
            &arena,
            &log_function(&arena),
            &[num(&arena, 10.0), string(&arena, "foo")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$base: foo is not a number.");
    }

    // --- pow ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_pow() {
        let arena = Bump::new();
        let got = eval(
            &arena,
            &pow_function(&arena),
            &[num(&arena, 2.0), num(&arena, 10.0)],
        )
        .await
        .unwrap();
        assert_inspect(&got, "1024");

        let got = eval(
            &arena,
            &pow_function(&arena),
            &[num(&arena, 4.0), num(&arena, 0.5)],
        )
        .await
        .unwrap();
        assert_inspect(&got, "2");

        let got = eval(
            &arena,
            &pow_function(&arena),
            &[num(&arena, 2.0), num(&arena, -2.0)],
        )
        .await
        .unwrap();
        assert_inspect(&got, "0.25");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_pow_with_units() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &pow_function(&arena),
            &[unit_num(&arena, 2.0, "px"), num(&arena, 3.0)],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$base: Expected 2px to have no units.");

        let err = eval(
            &arena,
            &pow_function(&arena),
            &[num(&arena, 2.0), unit_num(&arena, 3.0, "px")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$exponent: Expected 3px to have no units.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_pow_non_number() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &pow_function(&arena),
            &[string(&arena, "foo"), num(&arena, 2.0)],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$base: foo is not a number.");

        let err = eval(
            &arena,
            &pow_function(&arena),
            &[num(&arena, 2.0), string(&arena, "foo")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$exponent: foo is not a number.");
    }

    // --- sqrt ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_sqrt() {
        let arena = Bump::new();
        let got = eval(&arena, &sqrt_function(&arena), &[num(&arena, 9.0)])
            .await
            .unwrap();
        assert_inspect(&got, "3");

        let got = eval(&arena, &sqrt_function(&arena), &[num(&arena, 0.0)])
            .await
            .unwrap();
        assert_inspect(&got, "0");

        let got = eval(&arena, &sqrt_function(&arena), &[num(&arena, -1.0)])
            .await
            .unwrap();
        assert_inspect(&got, "calc(NaN)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_sqrt_with_units() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &sqrt_function(&arena),
            &[unit_num(&arena, 9.0, "px")],
        )
        .await
        .unwrap_err();
        assert_err_msg(err, "$number: Expected 9px to have no units.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_sqrt_non_number() {
        let arena = Bump::new();
        let err = eval(&arena, &sqrt_function(&arena), &[string(&arena, "foo")])
            .await
            .unwrap_err();
        assert_err_msg(err, "$number: foo is not a number.");
    }

    // --- unit ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_unit() {
        let arena = Bump::new();
        let tests = [
            (num(&arena, 5.0), ""),
            (unit_num(&arena, 5.0, "px"), "px"),
            (complex_num(&arena, 5.0, &["px", "s"], &[]), "px*s"),
            (complex_num(&arena, 5.0, &["px"], &["s"]), "px/s"),
            (complex_num(&arena, 5.0, &[], &["s"]), "s^-1"),
            (complex_num(&arena, 5.0, &[], &["s", "ms"]), "(s*ms)^-1"),
            (complex_num(&arena, 5.0, &["px"], &["s", "ms"]), "px/(s*ms)"),
        ];
        for (arg, want) in tests {
            let got = eval(&arena, &unit_function(&arena), &[arg]).await.unwrap();
            let ValueKind::String(s) = &*got else {
                panic!("expected String, got {got:?}");
            };
            assert_eq!(s.text, want);
            assert!(s.has_quotes, "unit() should return a quoted string");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_unit_non_number() {
        let arena = Bump::new();
        let err = eval(&arena, &unit_function(&arena), &[string(&arena, "foo")])
            .await
            .unwrap_err();
        assert_err_msg(err, "$number: foo is not a number.");
    }

    // --- unit_string / join_strings helpers ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_unit_string_helper() {
        let _arena = Bump::new();
        let mk = |v: f64, nums: &[&str], dens: &[&str]| {
            SassNumber::with_units(
                v,
                nums.iter().map(|s| s.to_string()).collect(),
                dens.iter().map(|s| s.to_string()).collect(),
            )
        };
        let tests: [(SassNumber, &str); 7] = [
            (SassNumber::new(1.0, None), ""),
            (SassNumber::new(1.0, Some("px")), "px"),
            (mk(1.0, &["px", "s"], &[]), "px*s"),
            (mk(1.0, &["px"], &["s"]), "px/s"),
            (mk(1.0, &[], &["s"]), "s^-1"),
            (mk(1.0, &[], &["s", "ms"]), "(s*ms)^-1"),
            (mk(1.0, &["px"], &["s", "ms"]), "px/(s*ms)"),
        ];
        for (n, want) in tests {
            assert_eq!(unit_string(&n), want);
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_join_strings() {
        assert_eq!(join_strings(&[], "*"), "");
        assert_eq!(join_strings(&["a".to_string()], "*"), "a");
        assert_eq!(
            join_strings(&["a".to_string(), "b".to_string(), "c".to_string()], "*"),
            "a*b*c"
        );
    }

    // --- comparable (global) ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_comparable_global_name() {
        let arena = Bump::new();
        assert_eq!(comparable_function_global(&arena).name(), "comparable");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_comparable_global() {
        let arena = Bump::new();
        let got = eval(
            &arena,
            &comparable_function_global(&arena),
            &[unit_num(&arena, 1.0, "px"), unit_num(&arena, 1.0, "in")],
        )
        .await
        .unwrap();
        assert_is_true(&got);

        let got = eval(
            &arena,
            &comparable_function_global(&arena),
            &[unit_num(&arena, 1.0, "px"), unit_num(&arena, 1.0, "s")],
        )
        .await
        .unwrap();
        assert_is_false(&got);
    }

    // --- unitless (global) ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_unitless_global_name() {
        let arena = Bump::new();
        assert_eq!(unitless_function_global(&arena).name(), "unitless");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_unitless_global() {
        let arena = Bump::new();
        let got = eval(
            &arena,
            &unitless_function_global(&arena),
            &[num(&arena, 5.0)],
        )
        .await
        .unwrap();
        assert_is_true(&got);

        let got = eval(
            &arena,
            &unitless_function_global(&arena),
            &[unit_num(&arena, 5.0, "px")],
        )
        .await
        .unwrap();
        assert_is_false(&got);
    }

    // --- random ---

    #[rust_sass_macros::maybe_test]
    async fn test_math_random_null() {
        let arena = Bump::new();
        for _ in 0..10 {
            let got = eval(&arena, &random_function(&arena), &[]).await.unwrap();
            let ValueKind::Number(n) = &*got else {
                panic!("expected Number, got {got:?}");
            };
            assert!(
                n.value >= 0.0 && n.value < 1.0,
                "random() = {}, want in [0, 1)",
                n.value
            );
            assert!(!n.has_units(), "random() should be unitless");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_random_limit_one() {
        let arena = Bump::new();
        let got = eval(&arena, &random_function(&arena), &[num(&arena, 1.0)])
            .await
            .unwrap();
        assert_inspect(&got, "1");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_random_limit() {
        let arena = Bump::new();
        for _ in 0..20 {
            let got = eval(&arena, &random_function(&arena), &[num(&arena, 10.0)])
                .await
                .unwrap();
            let ValueKind::Number(n) = &*got else {
                panic!("expected Number, got {got:?}");
            };
            assert!(n.is_int(), "random(10) = {}, want an int", n.value);
            assert!(
                n.value >= 1.0 && n.value <= 10.0,
                "random(10) = {}, want in [1, 10]",
                n.value
            );
            assert!(!n.has_units(), "random(10) should be unitless");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_random_zero_limit() {
        let arena = Bump::new();
        let err = eval(&arena, &random_function(&arena), &[num(&arena, 0.0)])
            .await
            .unwrap_err();
        assert_err_msg(err, "$limit: Must be greater than 0, was 0.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_random_negative_limit() {
        let arena = Bump::new();
        let err = eval(&arena, &random_function(&arena), &[num(&arena, -5.0)])
            .await
            .unwrap_err();
        assert_err_msg(err, "$limit: Must be greater than 0, was -5.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_random_non_int_limit() {
        let arena = Bump::new();
        let err = eval(&arena, &random_function(&arena), &[num(&arena, 2.5)])
            .await
            .unwrap_err();
        assert_err_msg(err, "$limit: 2.5 is not an int.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_random_non_number() {
        let arena = Bump::new();
        let err = eval(&arena, &random_function(&arena), &[string(&arena, "foo")])
            .await
            .unwrap_err();
        assert_err_msg(err, "$limit: foo is not a number.");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_random_units_deprecation() {
        let arena = Bump::new();
        let (result, logger) = eval_warn(
            &arena,
            &random_function(&arena),
            &[unit_num(&arena, 10.0, "px")],
        )
        .await;
        let got = result.unwrap();
        let ValueKind::Number(n) = &*got else {
            panic!("expected Number, got {got:?}");
        };
        assert!(
            n.value >= 1.0 && n.value <= 10.0,
            "random(10px) = {}, want in [1, 10]",
            n.value
        );
        assert!(!n.has_units(), "random(10px) result should ignore units");
        let want = [
            "math.random() will no longer ignore $limit units (10px) in a future release.",
            "",
            "Recommendation: math.random(math.div($limit, 1px)) * 1px",
            "",
            "To preserve current behavior: math.random(math.div($limit, 1px))",
            "",
            "More info: https://sass-lang.com/d/function-units",
        ]
        .join("\n");
        assert_single_warning(&logger, &want, Some("function-units"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_math_random_units_zero_still_errors() {
        let arena = Bump::new();
        // The units deprecation warning fires before the range check.
        let (result, logger) = eval_warn(
            &arena,
            &random_function(&arena),
            &[unit_num(&arena, 0.0, "px")],
        )
        .await;
        assert_err_msg(
            result.unwrap_err(),
            "$limit: Must be greater than 0, was 0px.",
        );
        assert_eq!(
            logger.messages().len(),
            1,
            "deprecation warning should fire before the error"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_math_random_units_emits_both_warnings() {
        let arena = Bump::new();
        // The wrapped global random() emits the GlobalBuiltin warning first,
        // then the FunctionUnits warning from the callback body.
        let fns = global_math_functions(&arena);
        let CallableKind::BuiltIn(rnd) = fns[6].kind() else {
            panic!("expected BuiltIn callable");
        };
        let (result, logger) = eval_warn(&arena, rnd, &[unit_num(&arena, 10.0, "px")]).await;
        let got = result.unwrap();
        let ValueKind::Number(n) = &*got else {
            panic!("expected Number, got {got:?}");
        };
        assert!(
            n.value >= 1.0 && n.value <= 10.0,
            "random(10px) = {}, want in [1, 10]",
            n.value
        );
        let messages = logger.messages();
        let deps = logger.deprecation_ids();
        assert_eq!(messages.len(), 2, "warnings = {messages:?}, want 2");
        let want_global = [
            "Global built-in functions are deprecated and will be removed in Dart Sass 3.0.0.",
            "Use math.random instead.",
            "",
            "More info and automated migrator: https://sass-lang.com/d/import",
        ]
        .join("\n");
        assert_eq!(messages[0], want_global);
        assert_eq!(deps[0].as_deref(), Some("global-builtin"));
        let want_units = [
            "math.random() will no longer ignore $limit units (10px) in a future release.",
            "",
            "Recommendation: math.random(math.div($limit, 1px)) * 1px",
            "",
            "To preserve current behavior: math.random(math.div($limit, 1px))",
            "",
            "More info: https://sass-lang.com/d/function-units",
        ]
        .join("\n");
        assert_eq!(messages[1], want_units);
        assert_eq!(deps[1].as_deref(), Some("function-units"));
    }

    // --- int64 saturation + non-finite errors ---

    // Dart `num.ceil()` returns a 64-bit int: `math.ceil(math.$max-number)`
    // saturates to 9223372036854775807, and non-finite input throws
    // `Unsupported operation: Infinity or NaN toInt`.
    #[rust_sass_macros::maybe_test]
    async fn test_ceil_floor_round_saturation() {
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        for (name, want) in [
            ("ceil", "9223372036854775807"),
            ("floor", "9223372036854775807"),
            ("round", "9223372036854775807"),
        ] {
            let mut opts = CompileOptions::new(&arena);
            let _ = &mut opts;
            let src = format!("@use \"sass:math\";\na {{ b: math.{name}(math.$max-number); }}");
            let result = compile_string(&src, io.clone(), opts, &arena)
                .await
                .unwrap();
            assert!(result.css().contains(want), "{name}: got: {}", result.css());
        }
        // Negative extreme.
        let src = "@use \"sass:math\";\na { b: math.floor(math.$max-number * -1); }";
        let result = compile_string(src, io.clone(), CompileOptions::new(&arena), &arena)
            .await
            .unwrap();
        assert!(
            result.css().contains("-9223372036854775808"),
            "got: {}",
            result.css()
        );

        // Non-finite input errors like Dart.
        for name in ["ceil", "floor", "round"] {
            let src = format!("@use \"sass:math\";\na {{ b: math.{name}(math.div(1, 0)); }}");
            let err = compile_string(&src, io.clone(), CompileOptions::new(&arena), &arena)
                .await
                .unwrap_err();
            match *err {
                SassError::Runtime { message, .. } => {
                    assert_eq!(message, "Unsupported operation: Infinity or NaN toInt")
                }
                other => panic!("{name}: expected Runtime error, got {other:?}"),
            }
        }
    }

    // --- log() argument-check order ---

    // Dart checks `$number` units before asserting `$base` (math.dart:154-168):
    // `math.log(10px, "foo")` reports the units error, not the base error.
    #[rust_sass_macros::maybe_test]
    async fn test_log_check_order() {
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@use \"sass:math\";\na { b: math.log(10px, \"foo\"); }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, .. } => {
                assert_eq!(message, "$number: Expected 10px to have no units.")
            }
            other => panic!("expected Runtime error, got {other:?}"),
        }
    }

    // --- div declared last ---

    // Dart declares `_div` last in the math module (math.dart:60-62);
    // observable via `meta.module-functions("math")` order.
    #[rust_sass_macros::maybe_test]
    async fn test_div_member_order() {
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let result = compile_string(
            "@use \"sass:meta\";\n@use \"sass:math\";\na { b: meta.inspect(meta.module-functions(\"math\")); }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        let css = result.css();
        let div_pos = css.find("\"div\"").expect("div missing");
        let unit_pos = css.find("\"unit\"").expect("unit missing");
        assert!(div_pos > unit_pos, "div must come last, got: {css}");
    }
}
