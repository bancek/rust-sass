// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/functions/color.dart (misc: _adjust/_scale/_change/_ieHexStr
//   closures, _updateComponents, _changeColor/_channelForChange, _scaleColor/
//   _scaleChannel, _adjustColor/_adjustChannel, _sniffLegacyColorSpace)
// go-source: go/functions/color_adjust.go

use crate::eval::warn::warn_deprecation;
use crate::value::color_channel_types::ALPHA_CHANNEL;
use std::rc::Rc;

use bumpalo::Bump;
use indexmap::IndexMap;

use crate::callable::BuiltInCallable;
use crate::common::exception::{SassError, SassResult};
use crate::deprecation as de;
use crate::eval::{EvalConfig, EvalState};
use crate::value::Value;
use crate::value::{
    self, color::ColorSpace, color_channel_types::LinearChannel, SassColor, SassNumber, ValueKind,
};

use crate::functions::color_helpers::{angle_value, color_in_space};
use crate::functions::color_spaces_shared::{channel_from_value, color_from_channels};
use crate::functions::helpers::{clamp_like_css, is_none};

// Display/CSS shorthands for error messages (Dart interpolates `$color`/`$n`
// directly, which routes through `toCssString()`/`toString()`; here the two
// forms are explicit per patterns.md §6).
fn color_css(c: &SassColor) -> SassResult<String> {
    c.to_css_string(true)
}

// Display shorthand for "not a number or none" errors (Dart interpolates
// the offending `$channelArg`/`$alphaArg` via `toString()`).
fn num_str(n: &SassNumber) -> SassResult<String> {
    n.to_display_string()
}

/// Throws when a `change`/`adjust`/`scale` write targets a missing channel.
///
/// Message carries the color's CSS form; the channel name goes in
/// `$channel`-style attribution (`argument_name`).
///
/// Ports Dart's `_missingChannelError` (color.dart:2039).
fn missing_channel_error(color: &SassColor, channel_name: &str) -> SassError {
    let css = color_css(color).unwrap_or_else(|_| format!("{:?}", color));
    SassError::Script {
        message: format!("Because the CSS working group is still deciding on the best behavior, Sass doesn't currently support modifying missing channels (color: {css})."),
        argument_name: Some(channel_name.to_string()),
    }
}

/// Warns `FUNCTION_UNITS` for a unitless HSL saturation/lightness adjustment.
///
/// No-op for `%` inputs; otherwise suggests the `%` form. Only used on the
/// deprecation-period path in [`adjust_channel_impl`] where the adjustment is
/// re-tagged as `%` before conversion.
///
/// Ports the `_checkPercent(adjustmentArg, channel.name)` call inside Dart's
/// `_adjustChannel` HSL arm (color.dart:1275).
fn check_percent_deprecation<'compile, 'parse>(
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
    let n_str = num_str(n)?;
    let suggestion = n.unit_suggestion(name, Some("%"));
    warn_deprecation(config, state,
        &format!("${name}: Passing a number without unit % ({n_str}) is deprecated.\n\nTo preserve current behavior: {suggestion}\n\nMore info: https://sass-lang.com/d/function-units"),
        &de::FUNCTION_UNITS,
    )
}

/// Returns `old_value` shifted by `adjustment_arg` per `channel`'s definition.
///
/// `None` adjustment returns the old value untouched; a missing old channel
/// throws [`missing_channel_error`]. Deprecation-period coercions run first:
/// HSL/HWB hue adjustments parse as angles, HSL saturation/lightness warn and
/// re-tag as `%`, unitful alpha warns and strips units. Clamped channels pin
/// overshoots at the bound the old value was already beyond.
///
/// Ports Dart's `_adjustChannel` (color.dart:1254), including the
/// `(space, channel)` switch arms and the lower/upper-clamped result guards.
fn adjust_channel_impl<'compile, 'parse>(
    arena: &'compile Bump,
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    color: &SassColor,
    channel: &LinearChannel,
    old_value: Option<f64>,
    adjustment_arg: Option<&SassNumber>,
) -> SassResult<Option<f64>>
where
    'compile: 'parse,
{
    let mut adjustment = match adjustment_arg {
        Some(a) => a,
        None => return Ok(old_value),
    };
    let old = match old_value {
        Some(v) => v,
        None => return Err(Box::new(missing_channel_error(color, channel.channel.name))),
    };
    let cloned: Option<SassNumber>;
    if (color.space == ColorSpace::Hsl || color.space == ColorSpace::Hwb)
        && channel.channel.is_polar_angle
    {
        let deg = angle_value(
            config,
            state,
            &Value::new_with_arena(arena, ValueKind::Number(adjustment.clone())),
            "hue",
        )?;
        cloned = Some(SassNumber::new(deg, None));
        adjustment = cloned.as_ref().unwrap();
    } else if color.space == ColorSpace::Hsl
        && (channel.channel.name == "saturation" || channel.channel.name == "lightness")
    {
        check_percent_deprecation(config, state, adjustment, channel.channel.name)?;
        cloned = Some(SassNumber::new(adjustment.value, Some("%")));
        adjustment = cloned.as_ref().unwrap();
    } else if channel.channel.name == "alpha" && adjustment.has_units() {
        warn_deprecation(config, state,
            &format!("$alpha: Passing a number with unit {} is deprecated.\n\nTo preserve current behavior: {}\n\nMore info: https://sass-lang.com/d/function-units",
                adjustment.unit_string(), adjustment.unit_suggestion("alpha", None)),
            &de::FUNCTION_UNITS,
        )?;
        cloned = Some(SassNumber::new(adjustment.value, None));
        adjustment = cloned.as_ref().unwrap();
    }
    let adj_ptr = channel_from_value(channel, Some(adjustment), false)?;
    let adj = match adj_ptr {
        Some(v) => v,
        None => return Ok(Some(old)),
    };
    let mut result = old + adj;
    if channel.lower_clamped && result < channel.min {
        result = if old < channel.min {
            old.max(result)
        } else {
            channel.min
        };
    }
    if channel.upper_clamped && result > channel.max {
        result = if old > channel.max {
            old.min(result)
        } else {
            channel.max
        };
    }
    Ok(Some(result))
}

/// Returns `old_value` scaled by `factor_arg` per `channel`'s definition.
///
/// `None` factor returns the old value; polar channels throw
/// `Channel isn't scalable.`; missing old channels throw
/// [`missing_channel_error`]. The `%` factor interpolates toward `max`
/// (positive) or `min` (negative); a zero factor or an already-saturated old
/// value returns the old value.
///
/// Ports Dart's `_scaleChannel` (color.dart:1184), including the
/// non-`LinearChannel` guard and the `factor == 0 / >= max / <= min` arms.
fn scale_channel_impl(
    _color: &SassColor,
    channel: &LinearChannel,
    old_value: Option<f64>,
    factor_arg: Option<&SassNumber>,
) -> SassResult<Option<f64>> {
    let factor = match factor_arg {
        Some(f) => f,
        None => return Ok(old_value),
    };
    if channel.channel.is_polar_angle {
        return Err(Box::new(SassError::Script {
            message: "Channel isn't scalable.".into(),
            argument_name: Some(channel.channel.name.to_string()),
        }));
    }
    let old = match old_value {
        Some(v) => v,
        None => {
            return Err(Box::new(missing_channel_error(
                _color,
                channel.channel.name,
            )))
        }
    };
    factor.assert_unit("%", Some(channel.channel.name))?;
    factor.value_in_range(-100.0, 100.0, Some(channel.channel.name))?;
    let f = factor.value / 100.0;
    if f == 0.0 {
        return Ok(Some(old));
    }
    if f > 0.0 {
        if old >= channel.max {
            return Ok(Some(old));
        }
        Ok(Some(old + (channel.max - old) * f))
    } else {
        if old <= channel.min {
            return Ok(Some(old));
        }
        Ok(Some(old + (old - channel.min) * f))
    }
}

/// Returns a copy of `color` with channels shifted by `channel_args`/`alpha_arg`.
///
/// Alpha clamps to `[0, 1]` after adjusting; each channel goes through
/// [`adjust_channel_impl`].
///
/// Ports Dart's `_adjustColor` (color.dart:1217), including the
/// "color space doesn't matter for alpha" note (any non-strictly-bounded
/// channel descriptor works — here `ALPHA_CHANNEL`).
fn adjust_color_impl<'compile, 'parse>(
    arena: &'compile Bump,
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    color: &SassColor,
    channel_args: &[Option<&SassNumber>],
    alpha_arg: Option<&SassNumber>,
) -> SassResult<SassColor>
where
    'compile: 'parse,
{
    let chs = color.space.channels_linear();
    let adjusted_alpha = if let Some(a) = alpha_arg {
        adjust_channel_impl(
            arena,
            config,
            state,
            color,
            &ALPHA_CHANNEL,
            color.alpha_or_nil(),
            Some(a),
        )?
        .map(|v| clamp_like_css(v, 0.0, 1.0))
    } else {
        color.alpha_or_nil()
    };
    let a0 = adjust_channel_impl(
        arena,
        config,
        state,
        color,
        &chs[0],
        color.channel0_or_nil(),
        channel_args[0],
    )?;
    let a1 = adjust_channel_impl(
        arena,
        config,
        state,
        color,
        &chs[1],
        color.channel1_or_nil(),
        channel_args[1],
    )?;
    let a2 = adjust_channel_impl(
        arena,
        config,
        state,
        color,
        &chs[2],
        color.channel2_or_nil(),
        channel_args[2],
    )?;
    SassColor::new_color_for_space_internal(color.space, a0, a1, a2, adjusted_alpha)
}

/// Returns a copy of `color` with channels scaled by `channel_args`/`alpha_arg`.
///
/// Each channel (and alpha) goes through [`scale_channel_impl`].
///
/// Ports Dart's `_scaleColor` (color.dart:1154).
fn scale_color_impl(
    color: &SassColor,
    channel_args: &[Option<&SassNumber>],
    alpha_arg: Option<&SassNumber>,
) -> SassResult<SassColor> {
    let chs = color.space.channels_linear();
    let scaled_alpha = if let Some(a) = alpha_arg {
        scale_channel_impl(color, &ALPHA_CHANNEL, color.alpha_or_nil(), Some(a))?
    } else {
        color.alpha_or_nil()
    };
    let s0 = scale_channel_impl(color, &chs[0], color.channel0_or_nil(), channel_args[0])?;
    let s1 = scale_channel_impl(color, &chs[1], color.channel1_or_nil(), channel_args[1])?;
    let s2 = scale_channel_impl(color, &chs[2], color.channel2_or_nil(), channel_args[2])?;
    SassColor::new_color_for_space_internal(color.space, s0, s1, s2, scaled_alpha)
}

/// Returns a copy of `color` with channels replaced by `channel_args`/`alpha_val`.
///
/// Missing (`None`) entries keep the old channel; unquoted `none` clears it to
/// missing. Non-number, non-`none` entries throw `<value> is not a number or
/// unquoted "none".` with the channel-name attribution; alpha additionally
/// accepts unitless/`%` ranges and warns `FUNCTION_UNITS` for other units.
///
/// Ports Dart's `_changeColor` (color.dart:1090) with the `_channelForChange`
/// closure (color.dart:1131) inlined as `channel_for_change`: untouched
/// HSL/HWB saturation-like channels re-tag as `%` (the `channel > 0` rule),
/// matching Dart's `SassNumber(value, '%'-or-null)` reconstruction.
fn change_color_impl<'compile, 'parse>(
    arena: &'compile Bump,
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    color: &SassColor,
    channel_args: &[Option<Value<'parse>>],
    alpha_val: Option<&Value<'parse>>,
) -> SassResult<SassColor>
where
    'compile: 'parse,
{
    let channel_for_change =
        |channel_arg: &Option<Value<'parse>>, ch_idx: usize| -> SassResult<Option<SassNumber>> {
            let arg = match channel_arg {
                Some(a) => a,
                None => {
                    let or_nil = [
                        color.channel0_or_nil(),
                        color.channel1_or_nil(),
                        color.channel2_or_nil(),
                    ];
                    return if let Some(v) = or_nil[ch_idx] {
                        if (color.space == ColorSpace::Hsl || color.space == ColorSpace::Hwb)
                            && ch_idx > 0
                        {
                            Ok(Some(SassNumber::new(v, Some("%"))))
                        } else {
                            Ok(Some(SassNumber::new(v, None)))
                        }
                    } else {
                        Ok(None)
                    };
                }
            };
            if is_none(arg) {
                return Ok(None);
            }
            if let ValueKind::Number(n) = &**arg {
                return Ok(Some(n.clone()));
            }
            let s = num_str(&SassNumber::new(0.0, None))
                .map(|_| arg.to_display_string().unwrap_or_default())?;
            let chs = color.space.channels_linear();
            Err(Box::new(SassError::Script {
                message: format!("{s} is not a number or unquoted \"none\"."),
                argument_name: Some(chs[ch_idx].channel.name.to_string()),
            }))
        };
    let c0 = channel_for_change(&channel_args[0], 0)?;
    let c1 = channel_for_change(&channel_args[1], 1)?;
    let c2 = channel_for_change(&channel_args[2], 2)?;
    let alpha = match alpha_val {
        None => Some(color.alpha),
        Some(_) if is_none(alpha_val.unwrap()) => None,
        Some(v) => {
            if let ValueKind::Number(n) = &**v {
                if !n.has_units() {
                    Some(n.value_in_range(0.0, 1.0, Some("alpha"))?)
                } else if n.has_unit("%") {
                    Some(n.value_in_range_with_unit(0.0, 100.0, "alpha", "%")? / 100.0)
                } else {
                    let n_str = num_str(n)?;
                    warn_deprecation(config, state,&format!("$alpha: Passing a unit other than % ({n_str}) is deprecated.\n\nTo preserve current behavior: {}\n\nSee https://sass-lang.com/d/function-units", n.unit_suggestion("alpha", None)), &de::FUNCTION_UNITS)?;
                    Some(n.value_in_range(0.0, 1.0, Some("alpha"))?)
                }
            } else {
                let s = v.to_display_string()?;
                return Err(Box::new(SassError::Script {
                    message: format!("{s} is not a number or unquoted \"none\"."),
                    argument_name: Some("alpha".into()),
                }));
            }
        }
    };
    let result = color_from_channels(
        arena,
        config,
        state,
        color.space,
        c0.map(|n| Value::new_with_arena(arena, ValueKind::Number(n))),
        c1.map(|n| Value::new_with_arena(arena, ValueKind::Number(n))),
        c2.map(|n| Value::new_with_arena(arena, ValueKind::Number(n))),
        alpha,
        false,
        false,
    )?;
    match &*result {
        ValueKind::Color(c) => Ok(c.clone()),
        _ => panic!("expected Color"),
    }
}

/// Guesses the legacy space (RGB/HSL/HWB) a keyword update targets.
///
/// Scans keyword names in order: `red`/`green`/`blue` imply RGB,
/// `saturation`/`lightness` imply HSL, `whiteness`/`blackness` imply HWB;
/// otherwise a lone `hue` implies HSL. Returns `None` when no legacy keyword
/// is present.
///
/// Ports Dart's `_sniffLegacyColorSpace` (color.dart:1311), used for the
/// backwards-compat path that converts legacy colors before keyword updates.
fn sniff_legacy_color_space(keywords: &IndexMap<String, Value<'_>>) -> Option<ColorSpace> {
    for key in keywords.keys() {
        match key.as_str() {
            "red" | "green" | "blue" => return Some(ColorSpace::Rgb),
            "saturation" | "lightness" => return Some(ColorSpace::Hsl),
            "whiteness" | "blackness" => return Some(ColorSpace::Hwb),
            _ => {}
        }
    }
    if keywords.contains_key("hue") {
        Some(ColorSpace::Hsl)
    } else {
        None
    }
}

/// Creates the `color.adjust()` function (`$color, $kwargs...`).
///
/// Runs [`update_components`] in adjust mode.
///
/// Ports Dart's `_adjust` closure (color.dart:987).
pub fn adjust_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "adjust",
        "$color, $kwargs...",
        "sass:color",
        arena,
        Rc::new(
            move |config: &EvalConfig<'compile, 'parse>,
                  state: &mut EvalState<'compile, 'parse>,
                  args,
                  arena: &'compile Bump| {
                update_components(arena, config, state, &args, false, true, false)
            },
        ),
    )
}
/// Creates the `color.scale()` function (`$color, $kwargs...`).
///
/// Runs [`update_components`] in scale mode.
///
/// Ports Dart's `_scale` closure (color.dart:993).
pub fn scale_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "scale",
        "$color, $kwargs...",
        "sass:color",
        arena,
        Rc::new(
            move |config: &EvalConfig<'compile, 'parse>,
                  state: &mut EvalState<'compile, 'parse>,
                  args,
                  arena: &'compile Bump| {
                update_components(arena, config, state, &args, false, false, true)
            },
        ),
    )
}
/// Creates the `color.change()` function (`$color, $kwargs...`).
///
/// Runs [`update_components`] in change mode.
///
/// Ports Dart's `_change` closure (color.dart:999).
pub fn change_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "change",
        "$color, $kwargs...",
        "sass:color",
        arena,
        Rc::new(
            move |config: &EvalConfig<'compile, 'parse>,
                  state: &mut EvalState<'compile, 'parse>,
                  args,
                  arena: &'compile Bump| {
                update_components(arena, config, state, &args, true, false, false)
            },
        ),
    )
}

/// Implements `color.change`/`color.adjust`/`color.scale` for `update_components`.
///
/// Exactly one of `change`/`_adjust`/`scale` selects the mode (`_adjust` is the
/// default when neither `change` nor `scale` is set — Dart asserts exactly one
/// flag is true at the three call sites). Rejects positional kwargs, splits
/// off `$space`/`$alpha`, converts legacy colors via
/// [`sniff_legacy_color_space`] (or `_colorInSpace` otherwise), maps keyword
/// names to channel indices, then converts the result back to the original
/// space with `legacyMissing: false`.
///
/// Ports Dart's `_updateComponents` (color.dart:1023).
fn update_components<'compile, 'parse>(
    arena: &'compile Bump,
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    args: &[Value<'parse>],
    change: bool,
    _adjust: bool,
    scale: bool,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let argument_list = match &*args[1] {
        ValueKind::ArgumentList(al) => al,
        _ => panic!("expected ArgumentList"),
    };
    if !argument_list.as_list().is_empty() {
        return Err(Box::new(SassError::Script { message: "Only one positional argument is allowed. All other arguments must be passed by name.".into(), argument_name: None }));
    }
    let all_keywords: IndexMap<String, Value<'parse>> = argument_list.keywords().clone();
    let mut keywords = all_keywords;
    let original_color = value::assert_color(&args[0], Some("color"))?;
    let space_keyword = keywords.swap_remove("space");
    let alpha_val = keywords.swap_remove("alpha");
    if let Some(ref sv) = space_keyword {
        let s = value::assert_string(sv, Some("space"))?;
        s.assert_unquoted().map_err(|e| SassError::Script {
            message: e.to_string(),
            argument_name: Some("space".into()),
        })?;
    }
    let color = if space_keyword.is_none() && original_color.is_legacy() && !keywords.is_empty() {
        if let Some(s) = sniff_legacy_color_space(&keywords) {
            original_color.to_space(s, Some(false))?
        } else {
            original_color.clone()
        }
    } else {
        color_in_space(
            original_color,
            space_keyword
                .as_ref()
                .unwrap_or(&Value::new_with_arena(arena, ValueKind::Null)),
            None,
        )?
    };
    let chs = color.space.channels_linear();
    let mut channel_args: Vec<Option<Value<'parse>>> = vec![None, None, None];
    for (key, val) in keywords.iter() {
        let mut found = false;
        for (i, ch) in chs.iter().enumerate() {
            if ch.channel.name == key.as_str() {
                channel_args[i] = Some(*val);
                found = true;
                break;
            }
        }
        if !found {
            return Err(Box::new(SassError::Script {
                message: format!(
                    "Color space {} doesn't have a channel with this name.",
                    color.space.name()
                ),
                argument_name: Some(key.clone()),
            }));
        }
    }
    let result = if change {
        change_color_impl(
            arena,
            config,
            state,
            &color,
            &channel_args,
            alpha_val.as_ref(),
        )?
    } else {
        let cn: Vec<Option<&SassNumber>> = channel_args
            .iter()
            .enumerate()
            .map(|(i, a)| {
                a.as_ref()
                    .map(|v| {
                        let name: &str = chs[i].channel.name;
                        value::assert_number(v, Some(name))
                    })
                    .transpose()
            })
            .collect::<SassResult<Vec<Option<&SassNumber>>>>()?;
        let an: Option<&SassNumber> = alpha_val
            .as_ref()
            .map(|v| value::assert_number(v, Some("alpha")))
            .transpose()?;
        if scale {
            scale_color_impl(&color, &cn, an)?
        } else {
            adjust_color_impl(arena, config, state, &color, &cn, an)?
        }
    };
    let back = result.to_space(original_color.space, Some(false))?;
    Ok(Value::new_with_arena(arena, ValueKind::Color(back)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::functions::test_utils::*;
    use crate::value::ListSeparator;
    use crate::value::SassArgumentList;
    use crate::value::{SassNumber, ValueKind};

    fn n<'compile: 'parse, 'parse>(arena: &'compile Bump, v: f64) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(v, None)))
    }
    fn pc<'compile: 'parse, 'parse>(arena: &'compile Bump, v: f64) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(v, Some("%"))))
    }

    #[rust_sass_macros::maybe_test]
    async fn test_adjust_lightness() {
        let arena = Bump::new();
        let color = color_rgb(&arena, 255.0, 0.0, 0.0, 1.0);
        let kwargs = SassArgumentList::new(
            &arena,
            vec![],
            indexmap::IndexMap::from([("lightness".to_string(), pc(&arena, 20.0))]),
            ListSeparator::Comma,
        );
        let v = eval(
            &arena,
            &adjust_callable(&arena),
            &[
                color,
                Value::new_with_arena(&arena, ValueKind::ArgumentList(kwargs)),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.lightness().unwrap() - 70.0).abs() < 1.0),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_scale_lightness() {
        let arena = Bump::new();
        let color = color_rgb(&arena, 255.0, 0.0, 0.0, 1.0);
        let kwargs = SassArgumentList::new(
            &arena,
            vec![],
            indexmap::IndexMap::from([("lightness".to_string(), pc(&arena, 50.0))]),
            ListSeparator::Comma,
        );
        let v = eval(
            &arena,
            &scale_callable(&arena),
            &[
                color,
                Value::new_with_arena(&arena, ValueKind::ArgumentList(kwargs)),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.lightness().unwrap() - 75.0).abs() < 1.0),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_change_red() {
        let arena = Bump::new();
        let color = color_rgb(&arena, 255.0, 0.0, 0.0, 1.0);
        let kwargs = SassArgumentList::new(
            &arena,
            vec![],
            indexmap::IndexMap::from([("red".to_string(), n(&arena, 100.0))]),
            ListSeparator::Comma,
        );
        let v = eval(
            &arena,
            &change_callable(&arena),
            &[
                color,
                Value::new_with_arena(&arena, ValueKind::ArgumentList(kwargs)),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.channel0 - 100.0).abs() < 1.0),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_change_positional_error() {
        let arena = Bump::new();
        let color = color_rgb(&arena, 255.0, 0.0, 0.0, 1.0);
        let kwargs = SassArgumentList::new(
            &arena,
            vec![n(&arena, 1.0)],
            indexmap::IndexMap::new(),
            ListSeparator::Comma,
        );
        let err = eval(
            &arena,
            &change_callable(&arena),
            &[
                color,
                Value::new_with_arena(&arena, ValueKind::ArgumentList(kwargs)),
            ],
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Script { message, .. } => {
                assert!(message.contains("Only one positional argument"))
            }
            _ => panic!(),
        }
    }
}
