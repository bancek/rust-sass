// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/functions/color.dart (helpers: _colorInSpace, _channelName,
//   _angleValue, _checkPercent, _percentageOrUnitless, _channelFunction,
//   _removedColorFunction, _function, _forcePercent is in color_spaces_shared.rs)
// go-source: go/functions/color_helpers.go

use std::rc::Rc;

use crate::callable::{BuiltInCallable, SyncBuiltInCallback};
use crate::common::exception::{SassError, SassResult};
use crate::deprecation::{COLOR_FUNCTIONS, FUNCTION_UNITS};
use crate::eval::warn::warn_deprecation;
use crate::eval::{EvalConfig, EvalState};
use crate::value::Value;
use bumpalo::Bump;

use crate::value::color_conversions::convert_color;
use crate::value::{self, color::ColorSpace, SassColor, SassNumber, ValueKind};

/// Attaches an argument name to an assert_quoted/assert_unquoted error.
/// Matches Go: sasscommon.NewSassScriptException(err.Error(), new("name")) —
///
/// Rust-only helper (no Dart counterpart): Dart threads the name through the
/// cascade operator (`..assertUnquoted("space")`); here the name is attached
/// after the fact at each call site.
pub(crate) fn wrap_unquoted_err(e: Box<SassError>, name: &str) -> Box<SassError> {
    match *e {
        SassError::Script { message, .. } => Box::new(SassError::Script {
            message,
            argument_name: Some(name.to_string()),
        }),
        other => Box::new(other),
    }
}

/// Returns `color` converted to the space named by `space_val`.
///
/// A `null` space returns the input color unchanged. Otherwise asserts `$space`
/// is an unquoted string naming a color space (carries `$space` attribution)
/// and converts, defaulting `legacy_missing` to `true`.
///
/// Ports Dart's `_colorInSpace` (color.dart:1623), including the
/// `legacyMissing` flag: when `false`, missing channels in legacy spaces
/// convert to zero on conversion (used by `to-space`/`to-gamut` callers).
pub fn color_in_space(
    color: &SassColor,
    space_val: &Value<'_>,
    legacy_missing: Option<bool>,
) -> SassResult<SassColor> {
    let lm = legacy_missing.unwrap_or(true);
    if matches!(&**space_val, ValueKind::Null) {
        return Ok(color.clone());
    }
    let s = value::assert_string(space_val, Some("space"))?;
    s.assert_unquoted()
        .map_err(|e| wrap_unquoted_err(e, "space"))?;
    let space = ColorSpace::from_name(s.text, Some("space"))?;
    color.to_space(space, Some(lm))
}

/// Converts `color` to xyz-d65 with missing channels resolved to zero.
///
/// Fast path when already in xyz-d65 without missing channels; otherwise
/// converts explicitly so missing channels become `0` without allocating
/// intermediate colors.
///
/// Ports Dart's `toXyzNoMissing` local helper inside the `same()` closure
/// (color.dart:719): the three pattern arms (already clean, xyz-d65 with
/// missing, other space via manual `convert`).
pub fn to_xyz_no_missing(col: &SassColor) -> SassResult<SassColor> {
    if col.space == ColorSpace::XyzD65 && !col.has_missing_channel() {
        return Ok(col.clone());
    }
    if col.space == ColorSpace::XyzD65 {
        return SassColor::xyz_d65(col.channel0, col.channel1, col.channel2, col.alpha);
    }
    convert_color(
        col.space,
        ColorSpace::XyzD65,
        col.channel0,
        col.channel1,
        col.channel2,
        col.alpha,
        [false, false, false, false],
        None,
    )
}

/// Asserts `$channel` is a quoted string and returns its text.
///
/// Ports Dart's `_channelName` (color.dart:2050): `assertString("channel")`
/// cascaded with `assertQuoted("channel")`, so both errors carry `$channel`
/// attribution.
pub fn channel_name_from_arg(v: &Value<'_>) -> SassResult<String> {
    let s = value::assert_string(v, Some("channel"))?;
    s.assert_quoted()
        .map_err(|e| wrap_unquoted_err(e, "channel"))?;
    Ok(s.text.to_string())
}

/// Normalizes a channel number that accepts `%` or no units.
///
/// Unitless values pass through as-is; percentages scale so `0%` is `0` and
/// `100%` is `max`. Any other unit throws with `$name` attribution.
///
/// Ports Dart's `_percentageOrUnitless` (color.dart:1492); `name` identifies
/// the argument in the error message.
pub fn percentage_or_unitless(n: &SassNumber, max: f64, name: &str) -> SassResult<f64> {
    if !n.has_units() {
        return Ok(n.value);
    }
    if n.has_unit("%") {
        return Ok(max * n.value / 100.0);
    }
    let n_str = n.to_display_string()?;
    Err(Box::new(SassError::Script {
        message: format!("Expected {n_str} to have unit \"%\" or no units."),
        argument_name: Some(name.to_string()),
    }))
}

/// Warns `FUNCTION_UNITS` when `$name` is a number without `%` units.
///
/// No-op when the number already has `%`. Emits the "pass `%` explicitly"
/// suggestion pointing at sass-lang.com/d/function-units.
///
/// Ports Dart's `_checkPercent` (color.dart:1471).
pub fn check_percent<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    n: &SassNumber,
    name: &str,
) -> SassResult<()>
where
    'compile: 'parse,
{
    if n.has_unit("%") {
        return Ok(());
    }
    let n_str = n.to_display_string()?;
    warn_deprecation(
        config,
        state,
        &format!(
            "${name}: Passing a number without unit % ({n_str}) is deprecated.\n\n\
             To preserve current behavior: {suggestion}\n\n\
             More info: https://sass-lang.com/d/function-units",
            name = name,
            n_str = n_str,
            suggestion = n.unit_suggestion(name, Some("%")),
        ),
        &FUNCTION_UNITS,
    )
}

/// Asserts `$name` is a number and returns its value in degrees.
///
/// Numbers compatible with `deg` coerce directly; any other unit warns
/// `FUNCTION_UNITS` (with a `unit_suggestion`) and returns the raw value.
///
/// Ports Dart's `_angleValue` (color.dart:1455), including the
/// deprecation-period branch that still accepts non-`deg` hue adjustments.
pub fn angle_value<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    v: &Value<'_>,
    name: &str,
) -> SassResult<f64>
where
    'compile: 'parse,
{
    let n = value::assert_number(v, Some(name))?;
    if n.compatible_with_unit("deg") {
        return n.coerce_value_to_unit("deg", Some(name));
    }
    let n_str = n.to_display_string()?;
    let suggestion = n.unit_suggestion(name, None);
    warn_deprecation(
        config,
        state,
        &format!(
            "${name}: Passing a unit other than deg ({n_str}) is deprecated.\n\n\
             To preserve current behavior: {suggestion}\n\n\
             See https://sass-lang.com/d/function-units",
        ),
        &FUNCTION_UNITS,
    )?;
    Ok(n.value)
}

/// Returns `n` with unit `%`, preserving value and denominator units.
///
/// A number already in `%` passes through unchanged; `null` input stays `null`
/// at the call site (`Option` wrapper there).
///
/// Ports Dart's `_forcePercent` (color.dart:1913).
pub fn force_percent(n: &SassNumber) -> SassNumber {
    SassNumber::with_units(n.value, vec!["%".into()], n.denominator_units.clone())
}

/// Returns whether `v` is a proprietary Microsoft filter invocation.
///
/// Matches an unquoted string longer than 2 chars that starts with an ASCII
/// letter and contains `=`, e.g. `alpha(opacity=50)`.
///
/// Ports Dart's `_microsoftFilterStart` regexp test (color.dart:23) used by
/// the `alpha()`/`opacity()` overloads: `RegExp(r'^[a-zA-Z]+\s*=')` plus the
/// `text.contains(...)` call-site checks.
pub fn is_microsoft_filter(v: &Value<'_>) -> bool {
    match &**v {
        ValueKind::String(s) => {
            !s.has_quotes
                && s.text.len() > 2
                && s.text
                    .bytes()
                    .next()
                    .is_some_and(|b| b.is_ascii_alphabetic())
                && s.text.contains('=')
        }
        _ => false,
    }
}

/// Creates a `sass:color` stub that redirects `name()` to `color.adjust()`.
///
/// Calling it always throws: names the missing function, suggests
/// `color.adjust($color, $<argument>: <amount>)`, and links the function docs.
///
/// Ports Dart's `_removedColorFunction` (color.dart:1343); `negative` in Dart
/// becomes the separate [`removed_color_function_negative`] constructor here.
pub fn removed_color_function<'compile, 'parse>(
    name: &str,
    argument: &str,
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    let n = name.to_string();
    let a = argument.to_string();
    BuiltInCallable::function(
        name,
        "$color, $amount",
        "sass:color",
        arena,
        Rc::new(move |_config, _state, args, _arena: &'compile Bump| {
            let arg0_str = args[0].to_display_string()?;
            let arg1_str = args[1].to_display_string()?;
            Err(Box::new(SassError::Script {
                message: format!(
                    "The function {n}() isn't in the sass:color module.\n\n\
                     Recommendation: color.adjust({arg0_str}, ${a}: {arg1_str})\n\n\
                     More info: https://sass-lang.com/documentation/functions/color#{n}",
                ),
                argument_name: None,
            }))
        }),
    )
}

/// Creates a `sass:color` stub like [`removed_color_function`], but the
/// suggested `color.adjust()` call negates the amount (`$<argument>: -<amount>`).
///
/// Ports Dart's `_removedColorFunction(..., negative: true)` variant
/// (color.dart:1343).
pub fn removed_color_function_negative<'compile, 'parse>(
    name: &str,
    argument: &str,
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    let n = name.to_string();
    let a = argument.to_string();
    BuiltInCallable::function(
        name,
        "$color, $amount",
        "sass:color",
        arena,
        Rc::new(move |_config, _state, args, _arena: &'compile Bump| {
            let arg0_str = args[0].to_display_string()?;
            let arg1_str = args[1].to_display_string()?;
            Err(Box::new(SassError::Script {
                message: format!(
                    "The function {n}() isn't in the sass:color module.\n\n\
                     Recommendation: color.adjust({arg0_str}, ${a}: -{arg1_str})\n\n\
                     More info: https://sass-lang.com/documentation/functions/color#{n}",
                ),
                argument_name: None,
            }))
        }),
    )
}

/// Creates the module-syntax channel getter `name` (`$color`) in `space`.
///
/// Returns the channel value unitless. Emits a `COLOR_FUNCTIONS` deprecation
/// suggesting `color.channel($color, "name", $space: <space>)`.
///
/// Ports Dart's `_channelFunction` (color.dart:1964) with `global: false`;
/// the `unit` variants are [`channel_function_with_unit`].
pub fn channel_function<'compile, 'parse>(
    name: &str,
    space: ColorSpace,
    getter: fn(&SassColor) -> SassResult<f64>,
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    new_built_in_channel_function(name, space, getter, "", false, arena)
}

/// Creates the module-syntax channel getter `name` (`$color`), returned with `unit`.
///
/// Same deprecation suggestion as [`channel_function`]; the result carries
/// `unit` (`"deg"` for hue, `"%"` for saturation/lightness/whiteness/blackness).
///
/// Ports Dart's `_channelFunction(..., unit: ...)` arm (color.dart:1964).
pub fn channel_function_with_unit<'compile, 'parse>(
    name: &str,
    space: ColorSpace,
    getter: fn(&SassColor) -> SassResult<f64>,
    unit: &str,
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    new_built_in_channel_function(name, space, getter, unit, false, arena)
}

/// Creates the deprecated-global channel getter `name` (`$color`) in `space`.
///
/// Same body as [`channel_function`], except the deprecation message names the
/// bare global (`red()`, not `color.red()`).
///
/// Ports Dart's `_channelFunction(..., global: true)` arm (color.dart:1964);
/// call sites add `.with_deprecation_warning("color", None)` like Dart's
/// `.withDeprecationWarning("color")` on the globals.
#[allow(dead_code)]
pub fn channel_function_global<'compile, 'parse>(
    name: &str,
    space: ColorSpace,
    getter: fn(&SassColor) -> SassResult<f64>,
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    new_built_in_channel_function(name, space, getter, "", true, arena)
}

/// Creates the deprecated-global channel getter `name` (`$color`), returned with `unit`.
///
/// Global-syntax variant of [`channel_function_with_unit`]; see
/// [`channel_function_global`] for the message difference.
///
/// Ports Dart's `_channelFunction(..., unit: ..., global: true)` arm
/// (color.dart:1964).
#[allow(dead_code)]
pub fn channel_function_global_with_unit<'compile, 'parse>(
    name: &str,
    space: ColorSpace,
    getter: fn(&SassColor) -> SassResult<f64>,
    unit: &str,
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    new_built_in_channel_function(name, space, getter, unit, true, arena)
}

// Like `BuiltInCallable::function`, but always sets the URL to `sass:color`
// (Dart's `_function` helper, color.dart:2055), inlined into each constructor
// above via the shared `new_built_in_channel_function` body below.
fn new_built_in_channel_function<'compile, 'parse>(
    name: &str,
    space: ColorSpace,
    getter: fn(&SassColor) -> SassResult<f64>,
    unit: &str,
    global: bool,
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    let n = name.to_string();
    let u = unit.to_string();
    let space_name = space.name().to_string();
    BuiltInCallable::function(
        name,
        "$color",
        "sass:color",
        arena,
        Rc::new(
            move |config: &EvalConfig<'compile, 'parse>,
                  state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump| {
                let c = value::assert_color(&args[0], Some("color"))?;
                let prefix = if global { "" } else { "color." };
                let v = getter(c)?;
                warn_deprecation(
                    config,
                    state,
                    &format!(
                        "{prefix}{n}() is deprecated. Suggestion:\n\
                     \n\
                     color.channel($color, {n:?}, $space: {space_name})\n\
                     \n\
                     More info: https://sass-lang.com/d/color-functions",
                    ),
                    &COLOR_FUNCTIONS,
                )?;
                let num = if u.is_empty() {
                    SassNumber::new(v, None)
                } else {
                    SassNumber::new(v, Some(&u))
                };
                Ok(Value::new_with_arena(arena, ValueKind::Number(num)))
            },
        ) as SyncBuiltInCallback<'compile, 'parse>,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_percentage_or_unitless() {
        // Unitless and % pass through.
        let v = percentage_or_unitless(&SassNumber::new(0.5, None), 255.0, "alpha").unwrap();
        assert_eq!(v, 0.5);
        let v = percentage_or_unitless(&SassNumber::new(50.0, Some("%")), 255.0, "alpha").unwrap();
        assert_eq!(v, 127.5);
    }

    #[test]
    fn test_percentage_or_unitless_error_message() {
        let err =
            percentage_or_unitless(&SassNumber::new(0.5, Some("px")), 255.0, "alpha").unwrap_err();
        match *err {
            SassError::Script {
                message,
                argument_name,
            } => {
                assert_eq!(message, "Expected 0.5px to have unit \"%\" or no units.");
                assert_eq!(argument_name.as_deref(), Some("alpha"));
            }
            other => panic!("expected Script error, got {other:?}"),
        }
    }
}
