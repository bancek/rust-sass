// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/functions/color.dart (legacy adjusters: grayscale/
//   saturate/desaturate/adjust-hue/lighten/darken, alpha/opacity, _invert/
//   _invertChannel, _complement, _mix/_mixLegacy, _opacify/_transparentize,
//   _ieHexStr, _suggestScaleAndAdjust, _missingChannelError, _adjustChannel
//   polar arm via adjust_polar_channel)
// go-source: go/functions/color_manipulation.go

use crate::eval::warn::warn_deprecation;
use crate::functions::color_helpers::wrap_unquoted_err;
use crate::value::color_channel_types::LinearChannel;
use std::rc::Rc;

use bumpalo::Bump;

use crate::callable::BuiltInCallable;
use crate::common::exception::{SassError, SassResult};
use crate::deprecation as de;
use crate::eval::{EvalConfig, EvalState};
use crate::util::number as nu;
use crate::value::Value;
use crate::value::{
    self, color::ColorSpace, color_conversions_base::GamutMapMethod,
    interpolation_method::InterpolationMethod, SassColor, SassNumber, SassString, ValueKind,
};

use crate::functions::color_helpers::{angle_value, check_percent, is_microsoft_filter};
use crate::functions::color_spaces_shared::color_space_chs;
use crate::functions::helpers::{clamp_like_css, function_string, warn_for_global_builtin};

// True modulo shared by `_invertChannel`'s hue arm and `adjust_polar_channel`:
// Rust `%` keeps the sign of the left operand, Dart's `%` on doubles used here
// always wraps into `[0, m)`.
fn math_mod(x: f64, m: f64) -> f64 {
    let r = x % m;
    if r < 0.0 {
        r + m
    } else {
        r
    }
}

// Display (`to_display_string`, Dart `toString`) / CSS (`to_css_string`,
// Dart `toCssString`) shorthands for error and deprecation messages below.
fn num_str(n: &SassNumber) -> SassResult<String> {
    n.to_display_string()
}
// CSS (`to_css_string`) shorthands for error and suggestion messages.
fn num_css(n: &SassNumber) -> SassResult<String> {
    n.to_css_string(true)
}
// Display shorthands for legacy-only error messages (`$color` interpolation).
fn color_str(c: &SassColor) -> SassResult<String> {
    c.to_display_string()
}
// CSS shorthand for the missing-channel error's `(color: ...)` payload.
fn color_css(c: &SassColor) -> SassResult<String> {
    c.to_css_string(true)
}

/// Wraps a raw channel value for `new_color_for_space_internal`.
///
/// Rust-only constructor shim (no Dart counterpart): forwards the
/// `Option<f64>` channels to `SassColor::new_color_for_space_internal`.
fn new_color_for_space_internal(
    space: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    SassColor::new_color_for_space_internal(space, c0, c1, c2, alpha)
}

/// Throws when an `invert`/`complement` write targets a missing channel.
///
/// Ports Dart's `_missingChannelError` (color.dart:2039); same body as the
/// `color_adjust.rs` copy, which serves `change`/`adjust`/`scale`.
fn missing_channel_error(color: &SassColor, channel_name: &str) -> SassError {
    let css = color_css(color).unwrap_or_else(|_| format!("{:?}", color));
    SassError::Script { message: format!("Because the CSS working group is still deciding on the best behavior, Sass doesn't currently support modifying missing channels (color: {css})."), argument_name: Some(channel_name.to_string()) }
}

/// Returns the inverse of `val` in linear channel `ch`.
///
/// Missing channels throw [`missing_channel_error`]: hue rotates 180°,
/// channels with negative minima negate, otherwise subtract from `max`.
///
/// Ports Dart's `_invertChannel` (color.dart:951).
fn invert_channel(color: &SassColor, ch: &LinearChannel, val: Option<f64>) -> SassResult<f64> {
    let v = match val {
        Some(v) => v,
        None => return Err(Box::new(missing_channel_error(color, ch.channel.name))),
    };
    Ok(if ch.channel.is_polar_angle {
        math_mod(v + 180.0, 360.0)
    } else if ch.min < 0.0 {
        -v
    } else {
        ch.max - v
    })
}

/// Suggests `color.scale()` + `color.adjust()` translations for a legacy tweak.
///
/// `adjustment` is the requested change to `channel_name` on `color`
/// (negative for `darken`/`desaturate`/`transparentize`); the scale factor is
/// `±100%` when the result overshoots, else the proportional distance. Alpha
/// suggestions are unitless, others carry `%`.
///
/// Ports Dart's `_suggestScaleAndAdjust` (color.dart:1992).
fn suggest_scale_and_adjust(color: &SassColor, adjustment: f64, channel_name: &str) -> String {
    let (ch_min, ch_max) = if channel_name == "alpha" {
        (0.0, 1.0)
    } else {
        let chs = color_space_chs(ColorSpace::Hsl);
        match chs.iter().find(|c| c.channel.name == channel_name) {
            Some(c) => (c.min, c.max),
            None => return String::new(),
        }
    };
    let old_value = if channel_name == "alpha" {
        color.alpha
    } else {
        match color.to_space(ColorSpace::Hsl, None) {
            Ok(c) => match c.channel_by_name(channel_name) {
                Ok(v) => v,
                Err(_) => return String::new(),
            },
            Err(_) => return String::new(),
        }
    };
    let new_value = old_value + adjustment;
    let mut s = String::from("Suggestion");
    if adjustment != 0.0 {
        let factor = if new_value > ch_max {
            1.0
        } else if new_value < ch_min {
            -1.0
        } else if adjustment > 0.0 {
            adjustment / (ch_max - old_value)
        } else {
            (new_value - old_value) / (old_value - ch_min)
        };
        let fc = num_css(&SassNumber::new(factor * 100.0, Some("%"))).unwrap_or_default();
        s.push_str(&format!(
            "s:\n\ncolor.scale($color, ${channel_name}: {fc})\n"
        ));
    } else {
        s.push_str(":\n\n");
    }
    let diff = if channel_name != "alpha" {
        SassNumber::new(adjustment, Some("%"))
    } else {
        SassNumber::new(adjustment, None)
    };
    let dc = num_css(&diff).unwrap_or_default();
    s.push_str(&format!("color.adjust($color, ${channel_name}: {dc})"));
    s
}

/// Creates the global `grayscale()` function (`$color`).
///
/// Numbers/special numbers echo as plain CSS; otherwise warns
/// `warn_for_global_builtin` and delegates to [`grayscale_impl`].
///
/// Ports Dart's `_function("grayscale", ...)` global entry (color.dart:147).
pub fn grayscale_callable<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "grayscale",
        "$color",
        "sass:color",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            if matches!(&*args[0], ValueKind::Number(_)) || args[0].is_special_number() {
                return function_string(arena, "grayscale", &args[..1]);
            }
            warn_for_global_builtin(config, state, "color", "grayscale")?;
            grayscale_impl(arena, value::assert_color(&args[0], Some("color"))?)
        }),
    )
}
/// Creates the module `color.grayscale()` function (`$color`).
///
/// A number arg echoes as plain CSS **plus** a `COLOR_MODULE_COMPAT`
/// deprecation; otherwise delegates to [`grayscale_impl`] with no global
/// warning.
///
/// Ports Dart's module `_function("grayscale", ...)` (color.dart:494).
pub fn grayscale_module_callable<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "grayscale",
        "$color",
        "sass:color",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            if let ValueKind::Number(_) = &*args[0] {
                let result = function_string(arena, "grayscale", &args[..1])?;
                let arg_str = args[0].to_display_string()?;
                warn_deprecation(config, state, &format!("Passing a number ({arg_str}) to color.grayscale() is deprecated.\n\nRecommendation: {}", result.to_display_string()?), &de::COLOR_MODULE_COMPAT)?;
                return Ok(result);
            }
            grayscale_impl(arena, value::assert_color(&args[0], Some("color"))?)
        }),
    )
}
/// Grayscales `color` with no plain-CSS handling.
///
/// Legacy colors zero the HSL saturation; modern colors zero the OKLCH
/// chroma; either way converts back to the original space.
///
/// Ports Dart's `_grayscale` (color.dart:963).
fn grayscale_impl<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    color: &SassColor,
) -> SassResult<Value<'parse>> {
    if color.is_legacy() {
        let hsl = color.to_space(ColorSpace::Hsl, None)?;
        let c = new_color_for_space_internal(
            ColorSpace::Hsl,
            hsl.channel0_or_nil(),
            Some(0.0),
            hsl.channel2_or_nil(),
            Some(hsl.alpha),
        )?;
        Ok(Value::new_with_arena(
            arena,
            ValueKind::Color(c.to_space(color.space, Some(false))?),
        ))
    } else {
        let oklch = color.to_space(ColorSpace::Oklch, None)?;
        let c = new_color_for_space_internal(
            ColorSpace::Oklch,
            oklch.channel0_or_nil(),
            Some(0.0),
            oklch.channel2_or_nil(),
            oklch.alpha_or_nil(),
        )?;
        Ok(Value::new_with_arena(
            arena,
            ValueKind::Color(c.to_space(color.space, None)?),
        ))
    }
}

/// Creates the overloaded global `saturate()` function.
///
/// The `$amount` overload echoes numbers/special numbers as plain CSS (a
/// non-number asserts `$amount` and re-emits `saturate(<css>)`); the
/// `$color, $amount` overload requires legacy colors, clamps
/// `saturation + amount` to `[0, 100]`, and warns `COLOR_FUNCTIONS` with a
/// scale/adjust suggestion.
///
/// Ports Dart's `BuiltInCallable.overloadedFunction("saturate", ...)`
/// (color.dart:238).
pub fn saturate_callable<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::overloaded_function(
        "saturate",
        "",
        vec![
            (
                "$amount",
                Rc::new(move |_config, _state, args, arena: &'compile Bump| {
                    if matches!(&*args[0], ValueKind::Number(_)) || args[0].is_special_number() {
                        return function_string(arena, "saturate", &args);
                    }
                    Ok(Value::new_with_arena(
                        arena,
                        ValueKind::String(SassString::new(
                            arena.alloc_str(&format!(
                                "saturate({})",
                                num_str(value::assert_number(&args[0], Some("amount"))?)?
                            )),
                            false,
                        )),
                    ))
                }),
            ),
            (
                "$color, $amount",
                Rc::new(move |config, state, args, arena: &'compile Bump| {
                    warn_for_global_builtin(config, state, "color", "adjust")?;
                    let color = value::assert_color(&args[0], Some("color"))?;
                    let amount = value::assert_number(&args[1], Some("amount"))?;
                    if !color.is_legacy() {
                        return Err(Box::new(SassError::Script { message: "saturate() is only supported for legacy colors. Please use color.adjust() instead with an explicit $space argument.".into(), argument_name: None }));
                    }
                    amount.value_in_range(0.0, 100.0, Some("amount"))?;
                    let v = clamp_like_css(color.saturation()? + amount.value, 0.0, 100.0);
                    let result = color.change_hsl(None, Some(v), None, None)?;
                    warn_deprecation(config, state, &format!("saturate() is deprecated. {}\n\nMore info: https://sass-lang.com/d/color-functions", suggest_scale_and_adjust(color, amount.value, "saturation")), &de::COLOR_FUNCTIONS)?;
                    Ok(Value::new_with_arena(arena, ValueKind::Color(result)))
                }),
            ),
        ],
        arena,
    )
}

/// Creates the global `desaturate()` function (`$color, $amount`).
///
/// Mirrors the two-arg `saturate` arm, subtracting the amount and suggesting
/// with the negated adjustment.
///
/// Ports Dart's `_function("desaturate", ...)` (color.dart:277).
pub fn desaturate_callable<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "desaturate",
        "$color, $amount",
        "sass:color",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            let color = value::assert_color(&args[0], Some("color"))?;
            let amount = value::assert_number(&args[1], Some("amount"))?;
            if !color.is_legacy() {
                return Err(Box::new(SassError::Script { message: "desaturate() is only supported for legacy colors. Please use color.adjust() instead with an explicit $space argument.".into(), argument_name: None }));
            }
            amount.value_in_range(0.0, 100.0, Some("amount"))?;
            let v = clamp_like_css(color.saturation()? - amount.value, 0.0, 100.0);
            let result = color.change_hsl(None, Some(v), None, None)?;
            warn_deprecation(config, state, &format!("desaturate() is deprecated. {}\n\nMore info: https://sass-lang.com/d/color-functions", suggest_scale_and_adjust(color, -amount.value, "saturation")), &de::COLOR_FUNCTIONS)?;
            Ok(Value::new_with_arena(arena, ValueKind::Color(result)))
        }),
    )
}

/// Creates the global `adjust-hue()` function (`$color, $degrees`).
///
/// Parses `$degrees` as an angle, requires a legacy color, and shifts the HSL
/// hue with a `color.adjust($color, $hue: ...)` suggestion.
///
/// Ports Dart's `_function("adjust-hue", ...)` (color.dart:158).
pub fn adjust_hue_callable<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "adjust-hue",
        "$color, $degrees",
        "sass:color",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            let color = value::assert_color(&args[0], Some("color"))?;
            let degrees = angle_value(config, state, &args[1], "degrees")?;
            if !color.is_legacy() {
                return Err(Box::new(SassError::Script { message: "adjust-hue() is only supported for legacy colors. Please use color.adjust() instead with an explicit $space argument.".into(), argument_name: None }));
            }
            let deg_css = num_css(&SassNumber::new(degrees, Some("deg"))).unwrap_or_default();
            warn_deprecation(config, state, &format!("adjust-hue() is deprecated. Suggestion:\n\ncolor.adjust($color, $hue: {deg_css})\n\nMore info: https://sass-lang.com/d/color-functions"), &de::COLOR_FUNCTIONS)?;
            let result = color.change_hsl(Some(color.hue()? + degrees), None, None, None)?;
            Ok(Value::new_with_arena(arena, ValueKind::Color(result)))
        }),
    )
}

/// Creates the global `lighten()` function (`$color, $amount`).
///
/// Requires a legacy color; clamps `lightness + amount` to `[0, 100]` and
/// warns `COLOR_FUNCTIONS` with a scale/adjust suggestion.
///
/// Ports Dart's `_function("lighten", ...)` (color.dart:182).
pub fn lighten_callable<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "lighten",
        "$color, $amount",
        "sass:color",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            let color = value::assert_color(&args[0], Some("color"))?;
            let amount = value::assert_number(&args[1], Some("amount"))?;
            if !color.is_legacy() {
                return Err(Box::new(SassError::Script { message: "lighten() is only supported for legacy colors. Please use color.adjust() instead with an explicit $space argument.".into(), argument_name: None }));
            }
            amount.value_in_range(0.0, 100.0, Some("amount"))?;
            let v = clamp_like_css(color.lightness()? + amount.value, 0.0, 100.0);
            let result = color.change_hsl(None, None, Some(v), None)?;
            warn_deprecation(config, state, &format!("lighten() is deprecated. {}\n\nMore info: https://sass-lang.com/d/color-functions", suggest_scale_and_adjust(color, amount.value, "lightness")), &de::COLOR_FUNCTIONS)?;
            Ok(Value::new_with_arena(arena, ValueKind::Color(result)))
        }),
    )
}
/// Creates the global `darken()` function (`$color, $amount`).
///
/// Mirrors [`lighten_callable`], subtracting the amount.
///
/// Ports Dart's `_function("darken", ...)` (color.dart:210).
pub fn darken_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "darken",
        "$color, $amount",
        "sass:color",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            let color = value::assert_color(&args[0], Some("color"))?;
            let amount = value::assert_number(&args[1], Some("amount"))?;
            if !color.is_legacy() {
                return Err(Box::new(SassError::Script { message: "darken() is only supported for legacy colors. Please use color.adjust() instead with an explicit $space argument.".into(), argument_name: None }));
            }
            amount.value_in_range(0.0, 100.0, Some("amount"))?;
            let v = clamp_like_css(color.lightness()? - amount.value, 0.0, 100.0);
            let result = color.change_hsl(None, None, Some(v), None)?;
            warn_deprecation(config, state, &format!("darken() is deprecated. {}\n\nMore info: https://sass-lang.com/d/color-functions", suggest_scale_and_adjust(color, -amount.value, "lightness")), &de::COLOR_FUNCTIONS)?;
            Ok(Value::new_with_arena(arena, ValueKind::Color(result)))
        }),
    )
}

/// Creates the overloaded global `alpha()` function.
///
/// The `$color` arm echoes Microsoft filters as plain CSS, rejects non-legacy
/// colors (suggesting `color.channel()`), and otherwise returns the alpha
/// after `warn_for_global_builtin`. The `$args...` arm only echoes when every
/// arg is a Microsoft filter; empty throws `Missing argument $color.`, longer
/// lists throw `Only 1 argument allowed, but N were passed.`.
///
/// Ports Dart's `BuiltInCallable.overloadedFunction("alpha", ...)` global
/// entry (color.dart:327).
pub fn alpha_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::overloaded_function(
        "alpha",
        "sass:color",
        vec![
            (
                "$color",
                Rc::new(move |config, state, args, arena: &'compile Bump| {
                    if is_microsoft_filter(&args[0]) {
                        return function_string(arena, "alpha", &args);
                    }
                    if let ValueKind::Color(c) = &*args[0] {
                        if !c.is_legacy() {
                            return Err(Box::new(SassError::Script { message: "alpha() is only supported for legacy colors. Please use color.channel() instead.".into(), argument_name: None }));
                        }
                    }
                    warn_for_global_builtin(config, state, "color", "alpha")?;
                    Ok(Value::new_with_arena(
                        arena,
                        ValueKind::Number(SassNumber::new(
                            value::assert_color(&args[0], Some("color"))?.alpha,
                            None,
                        )),
                    ))
                }),
            ),
            (
                "$args...",
                Rc::new(move |_config, _state, args, _arena: &'compile Bump| {
                    let rest = args[0].as_list(arena)?;
                    if !rest.is_empty() && rest.iter().all(|v| is_microsoft_filter(v)) {
                        return function_string(arena, "alpha", &args);
                    }
                    if rest.is_empty() {
                        return Err(Box::new(SassError::Script {
                            message: "Missing argument $color.".into(),
                            argument_name: None,
                        }));
                    }
                    Err(Box::new(SassError::Script {
                        message: format!(
                            "Only 1 argument allowed, but {} were passed.",
                            rest.len()
                        ),
                        argument_name: None,
                    }))
                }),
            ),
        ],
        arena,
    )
}
/// Creates the overloaded module `color.alpha()` function.
///
/// Same Microsoft-filter passthrough as [`alpha_callable`], but each echo
/// additionally warns `COLOR_MODULE_COMPAT` with a `Recommendation:`, and the
/// color path returns alpha with no global warning.
///
/// Ports Dart's module `BuiltInCallable.overloadedFunction("alpha", ...)`
/// (color.dart:551).
pub fn alpha_module_callable<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::overloaded_function(
        "alpha",
        "sass:color",
        vec![
            (
                "$color",
                Rc::new(move |config, state, args, arena: &'compile Bump| {
                    if is_microsoft_filter(&args[0]) {
                        let r = function_string(arena, "alpha", &args)?;
                        warn_deprecation(config, state, &format!("Using color.alpha() for a Microsoft filter is deprecated.\n\nRecommendation: {}", r.to_display_string()?), &de::COLOR_MODULE_COMPAT)?;
                        return Ok(r);
                    }
                    if let ValueKind::Color(c) = &*args[0] {
                        if !c.is_legacy() {
                            return Err(Box::new(SassError::Script { message: "color.alpha() is only supported for legacy colors. Please use color.channel() instead.".into(), argument_name: None }));
                        }
                    }
                    Ok(Value::new_with_arena(
                        arena,
                        ValueKind::Number(SassNumber::new(
                            value::assert_color(&args[0], Some("color"))?.alpha,
                            None,
                        )),
                    ))
                }),
            ),
            (
                "$args...",
                Rc::new(move |config, state, args, _arena: &'compile Bump| {
                    let rest = args[0].as_list(arena)?;
                    if rest.iter().all(|v| is_microsoft_filter(v)) {
                        let r = function_string(arena, "alpha", &args)?;
                        warn_deprecation(config, state, &format!("Using color.alpha() for a Microsoft filter is deprecated.\n\nRecommendation: {}", r.to_display_string()?), &de::COLOR_MODULE_COMPAT)?;
                        return Ok(r);
                    }
                    Err(Box::new(SassError::Script {
                        message: format!(
                            "Only 1 argument allowed, but {} were passed.",
                            rest.len()
                        ),
                        argument_name: None,
                    }))
                }),
            ),
        ],
        arena,
    )
}

/// Creates the global `opacity()` function (`$color`).
///
/// Numbers/special numbers echo as plain CSS; otherwise warns
/// `warn_for_global_builtin` and returns the alpha.
///
/// Ports Dart's `_function("opacity", ...)` global entry (color.dart:368).
pub fn opacity_callable<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "opacity",
        "$color",
        "sass:color",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            if matches!(&*args[0], ValueKind::Number(_)) || args[0].is_special_number() {
                return function_string(arena, "opacity", &args);
            }
            warn_for_global_builtin(config, state, "color", "opacity")?;
            Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(SassNumber::new(
                    value::assert_color(&args[0], Some("color"))?.alpha,
                    None,
                )),
            ))
        }),
    )
}
/// Creates the module `color.opacity()` function (`$color`).
///
/// A number arg echoes as plain CSS plus a `COLOR_MODULE_COMPAT` deprecation;
/// otherwise returns the alpha.
///
/// Ports Dart's module `_function("opacity", ...)` (color.dart:601).
pub fn opacity_module_callable<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "opacity",
        "$color",
        "sass:color",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            if let ValueKind::Number(_) = &*args[0] {
                let r = function_string(arena, "opacity", &args)?;
                let a = args[0].to_display_string()?;
                warn_deprecation(config, state, &format!("Passing a number ({a} to color.opacity() is deprecated.\n\nRecommendation: {}", r.to_display_string()?), &de::COLOR_MODULE_COMPAT)?;
                return Ok(r);
            }
            Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(SassNumber::new(
                    value::assert_color(&args[0], Some("color"))?.alpha,
                    None,
                )),
            ))
        }),
    )
}

/// Creates the global `invert()` function (`$color, $weight: 100%, $space: null`).
///
/// Warns `warn_for_global_builtin` for non-number first args, then delegates
/// to [`invert_impl`] in global mode (special numbers keep the plain-CSS
/// path).
///
/// Ports Dart's global `_function("invert", ...)` entry (color.dart:77).
pub fn invert_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "invert",
        "$color, $weight: 100%, $space: null",
        "sass:color",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            let is_n = matches!(&*args[0], ValueKind::Number(_)) || args[0].is_special_number();
            if !is_n {
                warn_for_global_builtin(config, state, "color", "invert")?;
            }
            invert_impl(config, state, arena, &args, true)
        }),
    )
}
/// Creates the module `color.invert()` function (`$color, $weight: 100%, $space: null`).
///
/// Delegates to [`invert_impl`] in module mode; a plain-CSS string result
/// additionally warns `COLOR_MODULE_COMPAT` (number passed to `color.invert()`).
///
/// Ports Dart's module `_function("invert", ...)` entry (color.dart:460).
pub fn invert_module_callable<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "invert",
        "$color, $weight: 100%, $space: null",
        "sass:color",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            let r = invert_impl(config, state, arena, &args, false)?;
            if let ValueKind::String(s) = &*r {
                let a = args[0].to_display_string()?;
                warn_deprecation(config, state, &format!("Passing a number ({a}) to color.invert() is deprecated.\n\nRecommendation: {}", s.text), &de::COLOR_MODULE_COMPAT)?;
            }
            Ok(r)
        }),
    )
}
/// Implements `invert()` for both global and module callables.
///
/// When `global` is set, special-number first args take the plain-CSS path.
/// Otherwise: number first args echo when `$weight` is exactly `100%`;
/// `$space: null` requires a legacy color and mixes the RGB inversion by
/// weight; an explicit space inverts per-space channels (HWB swaps
/// whiteness/blackness positions; HSL/LCH/OKLCH invert hue + last channel;
/// other spaces invert all three) and interpolates for partial weights.
///
/// Ports Dart's `_invert` (color.dart:867), including the `global` flag doc.
fn invert_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    args: &[Value<'parse>],
    global: bool,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let weight = value::assert_number(&args[1], Some("weight"))?;
    let is_n = matches!(&*args[0], ValueKind::Number(_));
    if is_n || (global && args[0].is_special_number()) {
        if !nu::fuzzy_equals(weight.value, 100.0) || !weight.has_unit("%") {
            return Err(Box::new(SassError::Script {
                message: "Only one argument may be passed to the plain-CSS invert() function."
                    .into(),
                argument_name: None,
            }));
        }
        return function_string(arena, "invert", &args[..1]);
    }
    let color = value::assert_color(&args[0], Some("color"))?;
    if matches!(&*args[2], ValueKind::Null) {
        if !color.is_legacy() {
            let cs = color_str(color)?;
            return Err(Box::new(SassError::Script {
                message: format!(
                    "To use color.invert() with non-legacy color {cs}, you must provide a $space."
                ),
                argument_name: Some("color".into()),
            }));
        }
        check_percent(config, state, weight, "weight")?;
        let rgb = color.to_space(ColorSpace::Rgb, None)?;
        let chs = color_space_chs(ColorSpace::Rgb);
        let inv = SassColor::rgb_internal(
            Some(invert_channel(&rgb, &chs[0], rgb.channel0_or_nil())?),
            Some(invert_channel(&rgb, &chs[1], rgb.channel1_or_nil())?),
            Some(invert_channel(&rgb, &chs[2], rgb.channel2_or_nil())?),
            color.alpha_or_nil(),
            None,
        )?;
        let mixed = mix_legacy(arena, &inv, color, weight)?;
        if let ValueKind::Color(mc) = &*mixed {
            return Ok(Value::new_with_arena(
                arena,
                ValueKind::Color(mc.to_space(color.space, None)?),
            ));
        }
        return Ok(mixed);
    }
    let ss = value::assert_string(&args[2], Some("space"))?;
    // Match Go: sasscommon.NewSassScriptException(err.Error(), new("space"))
    if let Err(e) = ss.assert_unquoted() {
        return Err(Box::new(SassError::Script {
            message: e.to_string(),
            argument_name: Some("space".into()),
        }));
    }
    let space = ColorSpace::from_name(ss.text, Some("space"))?;
    weight.assert_unit("%", Some("weight"))?;
    weight.value_in_range(0.0, 100.0, Some("weight"))?;
    let w = weight.value / 100.0;
    if nu::fuzzy_equals(w, 0.0) {
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::Color(color.clone()),
        ));
    }
    let in_space = color.to_space(space, None)?;
    let chs = color_space_chs(space);
    let inverted = match space {
        ColorSpace::Hwb => new_color_for_space_internal(
            space,
            Some(invert_channel(
                &in_space,
                &chs[0],
                in_space.channel0_or_nil(),
            )?),
            in_space.channel2_or_nil(),
            in_space.channel1_or_nil(),
            in_space.alpha_or_nil(),
        )?,
        ColorSpace::Hsl | ColorSpace::Lch | ColorSpace::Oklch => {
            let c0 = invert_channel(&in_space, &chs[0], in_space.channel0_or_nil())?;
            let c2 = invert_channel(&in_space, &chs[2], in_space.channel2_or_nil())?;
            new_color_for_space_internal(
                space,
                Some(c0),
                in_space.channel1_or_nil(),
                Some(c2),
                Some(in_space.alpha),
            )?
        }
        _ => new_color_for_space_internal(
            space,
            Some(invert_channel(
                &in_space,
                &chs[0],
                in_space.channel0_or_nil(),
            )?),
            Some(invert_channel(
                &in_space,
                &chs[1],
                in_space.channel1_or_nil(),
            )?),
            Some(invert_channel(
                &in_space,
                &chs[2],
                in_space.channel2_or_nil(),
            )?),
            Some(in_space.alpha),
        )?,
    };
    if nu::fuzzy_equals(w, 1.0) {
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::Color(inverted.to_space(color.space, Some(false))?),
        ));
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Color(color.interpolate(
            &inverted,
            &InterpolationMethod::new(space, None)?,
            false,
            Some(1.0 - w),
        )?),
    ))
}

/// Creates the `complement()` function (`$color, $space: null`).
///
/// Legacy colors default to HSL; otherwise `$space` must name a polar space.
/// Rotates the hue channel 180° (channel 0 for legacy spaces, channel 2 for
/// modern ones) and converts back with `legacyMissing: false`.
///
/// Ports Dart's `_complement` (color.dart:813).
pub fn complement_callable<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "complement",
        "$color, $space: null",
        "sass:color",
        arena,
        Rc::new(move |_config, _state, args, arena: &'compile Bump| {
            let color = value::assert_color(&args[0], Some("color"))?;
            let space = if color.is_legacy() && matches!(&*args[1], ValueKind::Null) {
                ColorSpace::Hsl
            } else {
                let s = value::assert_string(&args[1], Some("space"))?;
                // Matches Dart: (..assertUnquoted("space")) carries the
                // argument name.
                s.assert_unquoted()
                    .map_err(|e| wrap_unquoted_err(e, "space"))?;
                ColorSpace::from_name(s.text, Some("space"))?
            };
            if !space.is_polar() {
                return Err(Box::new(SassError::Script {
                    message: format!("Color space {} doesn't have a hue channel.", space.name()),
                    argument_name: Some("space".into()),
                }));
            }
            let in_space = color.to_space(space, Some(!matches!(&*args[1], ValueKind::Null)))?;
            let result = if space.is_legacy() {
                // Legacy: adjust hue (channel 0) by 180
                let c0 = adjust_polar_channel(in_space.channel0_or_nil(), &in_space, 180.0)?;
                new_color_for_space_internal(
                    space,
                    c0,
                    in_space.channel1_or_nil(),
                    in_space.channel2_or_nil(),
                    in_space.alpha_or_nil(),
                )?
            } else {
                // Non-legacy: adjust hue (channel 2) by 180
                let c2 = adjust_polar_channel(in_space.channel2_or_nil(), &in_space, 180.0)?;
                new_color_for_space_internal(
                    space,
                    in_space.channel0_or_nil(),
                    in_space.channel1_or_nil(),
                    c2,
                    in_space.alpha_or_nil(),
                )?
            };
            Ok(Value::new_with_arena(
                arena,
                ValueKind::Color(result.to_space(color.space, Some(false))?),
            ))
        }),
    )
}
/// Rotates a hue channel value by `adjustment` degrees, wrapped to `[0, 360)`.
///
/// `None` (missing) throws [`missing_channel_error`] for `"hue"`.
///
/// Ports the `_adjustChannel(colorInSpace, ..., SassNumber(180))` calls in
/// Dart's `_complement` (color.dart:838/852).
fn adjust_polar_channel(
    channel_val: Option<f64>,
    color: &SassColor,
    adjustment: f64,
) -> SassResult<Option<f64>> {
    match channel_val {
        None => Err(Box::new(missing_channel_error(color, "hue"))),
        Some(v) => {
            let mut nv = math_mod(v + adjustment, 360.0);
            if nv < 0.0 {
                nv += 360.0;
            }
            Ok(Some(nv))
        }
    }
}

/// Creates the `mix()` function (`$color1, $color2, $weight: 50%, $method: null`).
///
/// With `$method`, interpolates in that space (weight coerced to `%`
/// `[0, 100]`). Without, warns `FUNCTION_UNITS` for unitless weights and
/// requires both colors to be legacy, then runs the legacy algorithm.
///
/// Ports Dart's `_mix` (color.dart:775).
pub fn mix_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "mix",
        "$color1, $color2, $weight: 50%, $method: null",
        "sass:color",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            let c1 = value::assert_color(&args[0], Some("color1"))?;
            let c2 = value::assert_color(&args[1], Some("color2"))?;
            let weight = value::assert_number(&args[2], Some("weight"))?;
            if !matches!(&*args[3], ValueKind::Null) {
                let method = InterpolationMethod::from_value(arena, &args[3], Some("method"))?;
                weight.assert_unit("%", Some("weight"))?;
                weight.value_in_range(0.0, 100.0, Some("weight"))?;
                return Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Color(c1.interpolate(
                        c2,
                        &method,
                        false,
                        Some(weight.value / 100.0),
                    )?),
                ));
            }
            check_percent(config, state, weight, "weight")?;
            if !c1.is_legacy() {
                let s = color_str(c1)?;
                return Err(Box::new(SassError::Script {
                    message: format!(
                        "To use color.mix() with non-legacy color {s}, you must provide a $method."
                    ),
                    argument_name: Some("color1".into()),
                }));
            }
            if !c2.is_legacy() {
                let s = color_str(c2)?;
                return Err(Box::new(SassError::Script {
                    message: format!(
                        "To use color.mix() with non-legacy color {s}, you must provide a $method."
                    ),
                    argument_name: Some("color2".into()),
                }));
            }
            mix_legacy(arena, c1, c2, weight)
        }),
    )
}
/// Mixes two legacy colors by `weight` with Sass's alpha-aware algorithm.
///
/// Normalizes weight and alpha distance to `[-1, 1]`, combines as
/// `(w + a)/(1 + w*a)` (with the `w*a == -1` special case), then averages RGB
/// channels by the combined weight and alpha by the raw weight, clamped.
///
/// Ports Dart's `_mixLegacy` (color.dart:1510), including the normalization
/// rationale in its comment block.
fn mix_legacy<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    c1: &SassColor,
    c2: &SassColor,
    weight: &SassNumber,
) -> SassResult<Value<'parse>> {
    weight.value_in_range(0.0, 100.0, Some("weight"))?;
    let ws = weight.value / 100.0;
    let nw = ws * 2.0 - 1.0;
    let ad = c1.alpha - c2.alpha;
    let cw1 = if nw * ad == -1.0 {
        nw
    } else {
        (nw + ad) / (1.0 + nw * ad)
    };
    let w1 = (cw1 + 1.0) / 2.0;
    let w2 = 1.0 - w1;
    let r1 = &c1.to_space(ColorSpace::Rgb, None)?;
    let r2 = &c2.to_space(ColorSpace::Rgb, None)?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Color(SassColor::rgb(
            r1.channel0 * w1 + r2.channel0 * w2,
            r1.channel1 * w1 + r2.channel1 * w2,
            r1.channel2 * w1 + r2.channel2 * w2,
            clamp_like_css(r1.alpha * ws + r2.alpha * (1.0 - ws), 0.0, 1.0),
        )),
    ))
}

/// Creates the `opacify()`/`fade-in()` function (`$color, $amount`).
///
/// `name` selects the entry (both share the body). Requires a legacy color,
/// adds the unitless `[0, 1]` amount to alpha (clamped), and warns
/// `COLOR_FUNCTIONS` with a scale/adjust suggestion.
///
/// Ports Dart's `_opacify` (color.dart:1557).
pub fn opacify_callable<'compile, 'parse>(
    name: &str,
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    let n = name.to_string();
    let fn_name = n.clone();
    BuiltInCallable::function(
        &fn_name,
        "$color, $amount",
        "sass:color",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            let c = value::assert_color(&args[0], Some("color"))?;
            let a = value::assert_number(&args[1], Some("amount"))?;
            if !c.is_legacy() {
                return Err(Box::new(SassError::Script { message: format!("{n}() is only supported for legacy colors. Please use color.adjust() instead with an explicit $space argument."), argument_name: None }));
            }
            a.value_in_range_with_unit(0.0, 1.0, "amount", "")?;
            let na = clamp_like_css(c.alpha + a.value, 0.0, 1.0);
            warn_deprecation(
                config,
                state,
                &format!(
                    "{n}() is deprecated. {}\n\nMore info: https://sass-lang.com/d/color-functions",
                    suggest_scale_and_adjust(c, a.value, "alpha")
                ),
                &de::COLOR_FUNCTIONS,
            )?;
            Ok(Value::new_with_arena(
                arena,
                ValueKind::Color(c.change_alpha(na)?),
            ))
        }),
    )
}
/// Creates the `transparentize()`/`fade-out()` function (`$color, $amount`).
///
/// Mirrors [`opacify_callable`], subtracting the amount and suggesting with
/// the negated adjustment.
///
/// Ports Dart's `_transparentize` (color.dart:1586).
pub fn transparentize_callable<'compile, 'parse>(
    name: &str,
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    let n = name.to_string();
    let fn_name = n.clone();
    BuiltInCallable::function(
        &fn_name,
        "$color, $amount",
        "sass:color",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            let c = value::assert_color(&args[0], Some("color"))?;
            let a = value::assert_number(&args[1], Some("amount"))?;
            if !c.is_legacy() {
                return Err(Box::new(SassError::Script { message: format!("{n}() is only supported for legacy colors. Please use color.adjust() instead with an explicit $space argument."), argument_name: None }));
            }
            a.value_in_range_with_unit(0.0, 1.0, "amount", "")?;
            let na = clamp_like_css(c.alpha - a.value, 0.0, 1.0);
            warn_deprecation(
                config,
                state,
                &format!(
                    "{n}() is deprecated. {}\n\nMore info: https://sass-lang.com/d/color-functions",
                    suggest_scale_and_adjust(c, -a.value, "alpha")
                ),
                &de::COLOR_FUNCTIONS,
            )?;
            Ok(Value::new_with_arena(
                arena,
                ValueKind::Color(c.change_alpha(na)?),
            ))
        }),
    )
}

/// Creates the `ie-hex-str()` function (`$color`).
///
/// Converts to RGB, gamut-maps (`local-minde`), then emits `#AARRGGBB` with
/// per-component `fuzzy_round` (not plain `round`).
///
/// Ports Dart's `_ieHexStr` (color.dart:1005).
pub fn ie_hex_str_callable<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "ie-hex-str",
        "$color",
        "sass:color",
        arena,
        Rc::new(move |_config, _state, args, arena: &'compile Bump| {
            let color = value::assert_color(&args[0], Some("color"))?;
            let rgb = color.to_space(ColorSpace::Rgb, None)?;
            let gamut = rgb.to_gamut(GamutMapMethod::LocalMinde)?;
            // Matches Dart: fuzzyRound per component (_ieHexStr hexString).
            let a = nu::fuzzy_round(gamut.alpha * 255.0).map_err(|m| SassError::Script {
                message: m,
                argument_name: None,
            })?;
            let r = nu::fuzzy_round(gamut.channel0).map_err(|m| SassError::Script {
                message: m,
                argument_name: None,
            })?;
            let g = nu::fuzzy_round(gamut.channel1).map_err(|m| SassError::Script {
                message: m,
                argument_name: None,
            })?;
            let b = nu::fuzzy_round(gamut.channel2).map_err(|m| SassError::Script {
                message: m,
                argument_name: None,
            })?;
            Ok(Value::new_with_arena(
                arena,
                ValueKind::String(SassString::new(
                    arena.alloc_str(&format!("#{a:02X}{r:02X}{g:02X}{b:02X}")),
                    false,
                )),
            ))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::functions::test_utils::*;

    use crate::value::{SassColor, ValueKind};

    fn col_rgb<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        r: f64,
        g: f64,
        b: f64,
        a: f64,
    ) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::Color(SassColor::rgb(r, g, b, a)))
    }
    fn col_srgb<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        r: f64,
        g: f64,
        b: f64,
        a: f64,
    ) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::Color(SassColor::srgb(r, g, b, a).unwrap()),
        )
    }
    fn col_hsl<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        h: f64,
        s: f64,
        l: f64,
        a: f64,
    ) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::Color(SassColor::hsl(h, s, l, a).unwrap()))
    }

    #[rust_sass_macros::maybe_test]
    async fn test_grayscale_red() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &grayscale_callable(&arena),
            &[col_rgb(&arena, 255.0, 0.0, 0.0, 1.0)],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => {
                assert!((c.channel0 - 127.5).abs() < 1.0);
            }
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_grayscale_alpha() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &grayscale_callable(&arena),
            &[col_rgb(&arena, 255.0, 0.0, 0.0, 0.5)],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.alpha - 0.5).abs() < 0.01),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_grayscale_module() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &grayscale_module_callable(&arena),
            &[col_rgb(&arena, 0.0, 255.0, 0.0, 1.0)],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.channel0 - 127.5).abs() < 1.0),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_grayscale_number() {
        let arena = Bump::new();
        let v = eval(&arena, &grayscale_callable(&arena), &[num(&arena, 42.0)])
            .await
            .unwrap();
        assert_str(&v, "grayscale(42)");
    }
    #[rust_sass_macros::maybe_test]
    async fn test_saturate() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &saturate_callable(&arena),
            &[col_hsl(&arena, 0.0, 50.0, 50.0, 1.0), num(&arena, 50.0)],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.saturation().unwrap() - 100.0).abs() < 1.0),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_saturate_non_legacy() {
        let arena = Bump::new();
        let e = eval(
            &arena,
            &saturate_callable(&arena),
            &[col_srgb(&arena, 0.5, 0.5, 0.5, 1.0), num(&arena, 50.0)],
        )
        .await
        .unwrap_err();
        match *e {
            SassError::Script { message, .. } => {
                assert!(message.contains("only supported for legacy"))
            }
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_desaturate() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &desaturate_callable(&arena),
            &[col_hsl(&arena, 0.0, 100.0, 50.0, 1.0), num(&arena, 50.0)],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.saturation().unwrap() - 50.0).abs() < 1.0),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_adjust_hue() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &adjust_hue_callable(&arena),
            &[col_hsl(&arena, 0.0, 100.0, 50.0, 1.0), num(&arena, 90.0)],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.hue().unwrap() - 90.0).abs() < 1.0),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_lighten() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &lighten_callable(&arena),
            &[col_hsl(&arena, 0.0, 100.0, 50.0, 1.0), num(&arena, 20.0)],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.lightness().unwrap() - 70.0).abs() < 1.0),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_darken() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &darken_callable(&arena),
            &[col_hsl(&arena, 0.0, 100.0, 50.0, 1.0), num(&arena, 20.0)],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.lightness().unwrap() - 30.0).abs() < 1.0),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_opacify() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &opacify_callable("opacify", &arena),
            &[col_rgb(&arena, 255.0, 0.0, 0.0, 0.5), num(&arena, 0.3)],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.alpha - 0.8).abs() < 0.01),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_transparentize() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &transparentize_callable("transparentize", &arena),
            &[col_rgb(&arena, 255.0, 0.0, 0.0, 0.5), num(&arena, 0.3)],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.alpha - 0.2).abs() < 0.01),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_invert_white() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &invert_callable(&arena),
            &[
                col_rgb(&arena, 255.0, 255.0, 255.0, 1.0),
                percent(&arena, 100.0),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => {
                assert!((c.channel0).abs() < 1.0);
            }
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_complement_red() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &complement_callable(&arena),
            &[
                col_rgb(&arena, 255.0, 0.0, 0.0, 1.0),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => {
                assert!((c.channel0).abs() < 1.0);
                assert!((c.channel1 - 255.0).abs() < 5.0);
            }
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_mix() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &mix_callable(&arena),
            &[
                col_rgb(&arena, 255.0, 0.0, 0.0, 1.0),
                col_rgb(&arena, 0.0, 0.0, 255.0, 1.0),
                percent(&arena, 50.0),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.channel0 - 127.5).abs() < 2.0),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_ie_hex_str() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &ie_hex_str_callable(&arena),
            &[col_rgb(&arena, 0.0, 255.0, 0.0, 0.5)],
        )
        .await
        .unwrap();
        assert_str(&v, "#8000FF00");
    }
    #[rust_sass_macros::maybe_test]
    async fn test_ie_hex_str_fuzzy_round() {
        // Dart uses fuzzyRound per component; alpha*255 = 0.4999… (epsilon
        // below X.5) rounds up to 1, while plain round() gives 0.
        let arena = Bump::new();
        let v = eval(
            &arena,
            &ie_hex_str_callable(&arena),
            &[col_rgb(&arena, 0.0, 255.0, 0.0, 0.00196078431370589)],
        )
        .await
        .unwrap();
        assert_str(&v, "#0100FF00");
    }
    #[rust_sass_macros::maybe_test]
    async fn test_alpha_get() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &alpha_callable(&arena),
            &[col_rgb(&arena, 255.0, 0.0, 0.0, 0.75)],
        )
        .await
        .unwrap();
        assert_num(&v, 0.75);
    }
    #[rust_sass_macros::maybe_test]
    async fn test_opacity() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &opacity_callable(&arena),
            &[col_rgb(&arena, 255.0, 0.0, 0.0, 0.75)],
        )
        .await
        .unwrap();
        assert_num(&v, 0.75);
    }
}
