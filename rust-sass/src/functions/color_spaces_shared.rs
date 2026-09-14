// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/functions/color.dart (shared helpers _channelFromValue,
//   _colorFromChannels; _forcePercent lives in color_helpers.rs)
// go-source: go/functions/color_spaces.go (shared helpers)

use crate::value::color::ColorFormat;
use bumpalo::Bump;

use crate::common::exception::{SassError, SassResult};
use crate::eval::{EvalConfig, EvalState};
use crate::value::Value;
use crate::value::{
    color::ColorSpace, color_channel_types::LinearChannel, SassColor, SassNumber, ValueKind,
};

use crate::functions::color_helpers::{
    angle_value, check_percent, force_percent, percentage_or_unitless,
};
use crate::functions::helpers::clamp_like_css;

/// Returns the three linear channel descriptors for `space`.
///
/// Ports the `space.channels[...]` / `ColorSpace.channels` lookups shared by
/// `_colorFromChannels`, `_adjustColor`, `_scaleColor`, and friends
/// (color.dart throughout); the `&'static` table is the Rust form of Dart's
/// per-space channel lists.
pub(crate) fn color_space_chs(space: ColorSpace) -> &'static [LinearChannel; 3] {
    space.channels_linear()
}

// Dart's `_invertChannel` hue arm (`(value + 180) % 360`, color.dart:956) and
// `_adjustChannel`'s legacy hue normalization share this helper: true modulo
// for negative inputs (Rust `%` keeps the sign, Dart's doesn't for this use).
fn math_mod(x: f64, m: f64) -> f64 {
    let r = x % m;
    if r < 0.0 {
        r + m
    } else {
        r
    }
}

// Display shorthand for error messages (Dart interpolates `$n` via `toString`).
fn num_str(n: &SassNumber) -> SassResult<String> {
    n.to_display_string()
}

/// Converts one channel number to a raw channel value per `ch`.
///
/// Polar-angle channels coerce to `deg` and wrap to `[0, 360)`; channels that
/// require `%` throw without it; otherwise normalizes via
/// [`percentage_or_unitless`](super::color_helpers::percentage_or_unitless)
/// and, when `clamp` is set, clamps to the channel's clamped bounds
/// (`clamp_like_css` with unclamped sides as infinities).
///
/// Ports Dart's `_channelFromValue` (color.dart:1924): the three
/// `LinearChannel` switch arms plus the polar fallback.
pub(crate) fn channel_from_value(
    ch: &LinearChannel,
    n: Option<&SassNumber>,
    clamp: bool,
) -> SassResult<Option<f64>> {
    let n = match n {
        Some(n) => n,
        None => return Ok(None),
    };
    if ch.channel.is_polar_angle {
        let v = n.coerce_value_to_unit("deg", Some(ch.channel.name))?;
        return Ok(Some(math_mod(v, 360.0)));
    }
    if ch.requires_percent && !n.has_unit("%") {
        let n_str = num_str(n)?;
        return Err(Box::new(SassError::Script {
            message: format!("Expected {n_str} to have unit \"%\"."),
            argument_name: Some(ch.channel.name.to_string()),
        }));
    }
    let mut v = percentage_or_unitless(n, ch.max, ch.channel.name)?;
    if clamp && (ch.lower_clamped || ch.upper_clamped) {
        let min = if ch.lower_clamped {
            ch.min
        } else {
            f64::NEG_INFINITY
        };
        let max = if ch.upper_clamped {
            ch.max
        } else {
            f64::INFINITY
        };
        v = clamp_like_css(v, min, max);
    }
    Ok(Some(v))
}

/// Creates a color in `space` from per-channel values, or throws when invalid.
///
/// `None` channels stay missing; `alpha: None` likewise means missing. When
/// `clamp` is set, clamped channels are clamped. The HSL arm warns
/// `FUNCTION_UNITS` for unitless saturation/lightness and forces `%`; the HWB
/// arm requires `%` whiteness/blackness and rescales pairs summing over 100;
/// the RGB arm tags `ColorFormat::RgbFunction` when built by `rgb()`/`rgba()`.
///
/// Ports Dart's `_colorFromChannels` (color.dart:1843) with its
/// HSL/HWB/RGB/`default` switch arms.
// Arity mirrors Dart's `_colorFromChannels`; a params struct would diverge
// from the port.
#[allow(clippy::too_many_arguments)]
pub(crate) fn color_from_channels<'compile, 'parse>(
    arena: &'compile Bump,
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    space: ColorSpace,
    channel0: Option<Value<'_>>,
    channel1: Option<Value<'_>>,
    channel2: Option<Value<'_>>,
    alpha: Option<f64>,
    from_rgb_function: bool,
    clamp: bool,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let to_num = |v: &Option<Value<'_>>| -> Option<SassNumber> {
        v.as_deref().and_then(|v| match v {
            ValueKind::Number(n) => Some(n.clone()),
            _ => None,
        })
    };
    let c0 = to_num(&channel0);
    let c1 = to_num(&channel1);
    let c2 = to_num(&channel2);

    match space {
        ColorSpace::Hsl => {
            if let Some(ref n) = c1 {
                check_percent(config, state, n, "saturation")?;
            }
            if let Some(ref n) = c2 {
                check_percent(config, state, n, "lightness")?;
            }
            let c0v = if let Some(ref n) = c0 {
                Some(angle_value(
                    config,
                    state,
                    &Value::new_with_arena(arena, ValueKind::Number(n.clone())),
                    "hue",
                )?)
            } else {
                None
            };
            let chs = color_space_chs(ColorSpace::Hsl);
            let c1v = channel_from_value(&chs[1], c1.as_ref().map(force_percent).as_ref(), clamp)?;
            let c2v = channel_from_value(&chs[2], c2.as_ref().map(force_percent).as_ref(), clamp)?;
            Ok(Value::new_with_arena(
                arena,
                ValueKind::Color(SassColor::new_color_for_space_internal(
                    ColorSpace::Hsl,
                    c0v,
                    c1v,
                    c2v,
                    alpha,
                )?),
            ))
        }
        ColorSpace::Hwb => {
            if let Some(ref n) = c1 {
                n.assert_unit("%", Some("whiteness"))?;
            }
            if let Some(ref n) = c2 {
                n.assert_unit("%", Some("blackness"))?;
            }
            let mut whiteness = c1.as_ref().map(|n| n.value);
            let mut blackness = c2.as_ref().map(|n| n.value);
            if let (Some(w), Some(b)) = (&mut whiteness, &mut blackness) {
                if *w + *b > 100.0 {
                    let ow = *w;
                    *w = *w / (*w + *b) * 100.0;
                    *b = *b / (ow + *b) * 100.0;
                }
            }
            let c0v = if let Some(ref n) = c0 {
                Some(angle_value(
                    config,
                    state,
                    &Value::new_with_arena(arena, ValueKind::Number(n.clone())),
                    "hue",
                )?)
            } else {
                None
            };
            Ok(Value::new_with_arena(
                arena,
                ValueKind::Color(SassColor::new_color_for_space_internal(
                    ColorSpace::Hwb,
                    c0v,
                    whiteness,
                    blackness,
                    alpha,
                )?),
            ))
        }
        ColorSpace::Rgb => {
            let chs = color_space_chs(ColorSpace::Rgb);
            let c0v = channel_from_value(&chs[0], c0.as_ref(), clamp)?;
            let c1v = channel_from_value(&chs[1], c1.as_ref(), clamp)?;
            let c2v = channel_from_value(&chs[2], c2.as_ref(), clamp)?;
            let fmt = if from_rgb_function {
                Some(ColorFormat::RgbFunction)
            } else {
                None
            };
            Ok(Value::new_with_arena(
                arena,
                ValueKind::Color(SassColor::rgb_internal(c0v, c1v, c2v, alpha, fmt)?),
            ))
        }
        _ => {
            let chs = color_space_chs(space);
            let c0v = channel_from_value(&chs[0], c0.as_ref(), clamp)?;
            let c1v = channel_from_value(&chs[1], c1.as_ref(), clamp)?;
            let c2v = channel_from_value(&chs[2], c2.as_ref(), clamp)?;
            Ok(Value::new_with_arena(
                arena,
                ValueKind::Color(SassColor::new_color_for_space_internal(
                    space, c0v, c1v, c2v, alpha,
                )?),
            ))
        }
    }
}
