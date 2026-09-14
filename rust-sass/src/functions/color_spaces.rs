// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/functions/color.dart (constructors: _rgb/_rgbTwoArg,
//   _hsl, _parseChannels/_parseSlashChannels/_parseNumberOrString,
//   _specialCommaSpaces, _colorFromChannels RGB/HSL arms via color_spaces_shared)
// go-source: go/functions/color_spaces.go

use crate::value::assert_common_list_style;
use crate::value::ListSeparator;
use crate::value::SassList;
use std::rc::Rc;

use bumpalo::Bump;

use crate::callable::BuiltInCallable;
use crate::common::exception::{SassError, SassResult};
use crate::common::source_span_file_source::FileSource;
use crate::common::span_scanner::SpanScanner;
use crate::eval::{EvalConfig, EvalState};
use crate::parse::expression::number_impl;
use crate::parse::parser::ParserState;
use crate::parse::stylesheet::Syntax;
use crate::value::Value;
use crate::value::{self, color::ColorSpace, SassColor, SassNumber, SassString, ValueKind};

use crate::functions::color_helpers::percentage_or_unitless;
use crate::functions::color_spaces_shared::{color_from_channels, color_space_chs};
use crate::functions::helpers::{clamp_like_css, function_string, is_none};

// `isSpecialVariable` / `isColor` predicates (Dart `Value` getters) used by
// the `rgb` two-arg and `hsl` two-arg overloads to detect `var()`-ish inputs
// that must echo as plain CSS.
fn is_special_variable(v: &Value<'_>) -> bool {
    v.is_special_variable()
}
// `isColor` predicate: the two-arg `rgb` overload needs it alongside
// `isSpecialVariable` to decide the plain-CSS fallback.
fn is_color(v: &Value<'_>) -> bool {
    matches!(v.kind(), ValueKind::Color(_))
}

/// Creates the overloaded `rgb()` function.
///
/// Signatures: `$red, $green, $blue, $alpha` / `$red, $green, $blue` /
/// `$color, $alpha` / `$channels`.
///
/// Ports Dart's `BuiltInCallable.overloadedFunction("rgb", ...)`
/// (color.dart:53); each overload delegates to [`rgb_impl`], [`rgb_two_arg`],
/// or [`parse_channels`] with `space: rgb`.
pub fn rgb_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    let space = ColorSpace::Rgb;
    BuiltInCallable::overloaded_function(
        "rgb",
        "",
        vec![
            (
                "$red, $green, $blue, $alpha",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        rgb_impl(arena, config, state, "rgb", &args)
                    },
                ),
            ),
            (
                "$red, $green, $blue",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        rgb_impl(arena, config, state, "rgb", &args)
                    },
                ),
            ),
            (
                "$color, $alpha",
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        rgb_two_arg(arena, "rgb", &args)
                    },
                ),
            ),
            (
                "$channels",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        parse_channels(
                            config,
                            state,
                            arena,
                            "rgb",
                            "channels",
                            &args[0],
                            Some(space),
                        )
                    },
                ),
            ),
        ],
        arena,
    )
}

/// Creates the overloaded `rgba()` function.
///
/// Same four overloads as [`rgb_callable`], with `"rgba"` as the plain-CSS
/// fallback name.
///
/// Ports Dart's `BuiltInCallable.overloadedFunction("rgba", ...)`
/// (color.dart:65).
pub fn rgba_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    let space = ColorSpace::Rgb;
    BuiltInCallable::overloaded_function(
        "rgba",
        "",
        vec![
            (
                "$red, $green, $blue, $alpha",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        rgb_impl(arena, config, state, "rgba", &args)
                    },
                ),
            ),
            (
                "$red, $green, $blue",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        rgb_impl(arena, config, state, "rgba", &args)
                    },
                ),
            ),
            (
                "$color, $alpha",
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        rgb_two_arg(arena, "rgba", &args)
                    },
                ),
            ),
            (
                "$channels",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        parse_channels(
                            config,
                            state,
                            arena,
                            "rgba",
                            "channels",
                            &args[0],
                            Some(space),
                        )
                    },
                ),
            ),
        ],
        arena,
    )
}

/// Implements the three- and four-argument `rgb()`/`rgba()` functions.
///
/// Special numbers pass through as plain CSS ([`function_string`]); otherwise
/// asserts `$red`/`$green`/`$blue`, clamps the optional `$alpha` via
/// `%`-or-unitless normalization, and builds an `RgbFunction`-tagged color.
///
/// Ports Dart's `_rgb` (color.dart:1361), including `fromRgbFunction: true`.
fn rgb_impl<'compile, 'parse>(
    arena: &'compile Bump,
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    name: &str,
    args: &[Value<'parse>],
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let alpha = args.get(3);
    if args[0].is_special_number()
        || args[1].is_special_number()
        || args[2].is_special_number()
        || alpha.is_some_and(|a| a.is_special_number())
    {
        return function_string(arena, name, args);
    }
    let r = value::assert_number(&args[0], Some("red"))?;
    let g = value::assert_number(&args[1], Some("green"))?;
    let b = value::assert_number(&args[2], Some("blue"))?;
    let mut a = 1.0f64;
    if let Some(av) = alpha {
        let an = value::assert_number(av, Some("alpha"))?;
        let p = percentage_or_unitless(an, 1.0, "alpha")?;
        a = clamp_like_css(p, 0.0, 1.0);
    }
    color_from_channels(
        arena,
        config,
        state,
        ColorSpace::Rgb,
        Some(Value::new_with_arena(arena, ValueKind::Number(r.clone()))),
        Some(Value::new_with_arena(arena, ValueKind::Number(g.clone()))),
        Some(Value::new_with_arena(arena, ValueKind::Number(b.clone()))),
        Some(a),
        true,
        true,
    )
}

/// Implements the two-argument `rgb()`/`rgba()` functions.
///
/// `var()`-ish inputs (or a special alpha with a non-color first arg) fall
/// back to plain CSS, since `--foo` may expand to `123, 456, 789` after
/// substitution. Non-legacy colors throw with a `color.change()` suggestion;
/// a special-number alpha re-emits numeric channels plus the raw alpha.
///
/// Ports Dart's `_rgbTwoArg` (color.dart:1388).
fn rgb_two_arg<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    name: &str,
    args: &[Value<'parse>],
) -> SassResult<Value<'parse>> {
    let first = &args[0];
    let second = &args[1];
    if is_special_variable(first) || (!is_color(first) && is_special_variable(second)) {
        return function_string(arena, name, args);
    }
    let color = value::assert_color(first, Some("color"))?;
    if !color.is_legacy() {
        let cs = color.to_display_string()?;
        let a1s = second.to_display_string()?;
        return Err(Box::new(SassError::Script { message: format!("Expected {cs} to be in the legacy RGB, HSL, or HWB color space.\n\nRecommendation: color.change({cs}, $alpha: {a1s})"), argument_name: Some(name.to_string()) }));
    }
    let rgb = color.to_space(ColorSpace::Rgb, None)?;
    let r = rgb.channel0;
    let g = rgb.channel1;
    let b = rgb.channel2;
    if second.is_special_number() {
        let mut v = vec![
            Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(r, None))),
            Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(g, None))),
            Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(b, None))),
        ];
        v.push(*second);
        return function_string(arena, name, &v);
    }
    let s = value::assert_number(second, Some("alpha"))?;
    let p = percentage_or_unitless(s, 1.0, "alpha")?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Color(SassColor::rgb(r, g, b, clamp_like_css(p, 0.0, 1.0))),
    ))
}

/// Creates the overloaded `hsl()` function.
///
/// Signatures: `$hue, $saturation, $lightness, $alpha` /
/// `$hue, $saturation, $lightness` / `$hue, $saturation` / `$channels`. The
/// two-arg overload only survives for special variables (`hsl(123,
/// var(--foo))` may become `10%, 20%` post-substitution) and otherwise throws
/// `Missing argument $lightness.`.
///
/// Ports Dart's `BuiltInCallable.overloadedFunction("hsl", ...)`
/// (color.dart:107).
pub fn hsl_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    let space = ColorSpace::Hsl;
    BuiltInCallable::overloaded_function(
        "hsl",
        "",
        vec![
            (
                "$hue, $saturation, $lightness, $alpha",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        hsl_impl(arena, config, state, "hsl", &args)
                    },
                ),
            ),
            (
                "$hue, $saturation, $lightness",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        hsl_impl(arena, config, state, "hsl", &args)
                    },
                ),
            ),
            (
                "$hue, $saturation",
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump| {
                        if is_special_variable(&args[0]) || is_special_variable(&args[1]) {
                            function_string(arena, "hsl", &args)
                        } else {
                            Err(Box::new(SassError::Script {
                                message: "Missing argument $lightness.".into(),
                                argument_name: None,
                            }))
                        }
                    },
                ),
            ),
            (
                "$channels",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        parse_channels(
                            config,
                            state,
                            arena,
                            "hsl",
                            "channels",
                            &args[0],
                            Some(space),
                        )
                    },
                ),
            ),
        ],
        arena,
    )
}

/// Creates the overloaded `hsla()` function.
///
/// Same four overloads as [`hsl_callable`], with `"hsla"` as the plain-CSS
/// fallback name.
///
/// Ports Dart's `BuiltInCallable.overloadedFunction("hsla", ...)`
/// (color.dart:128).
pub fn hsla_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    let space = ColorSpace::Hsl;
    BuiltInCallable::overloaded_function(
        "hsla",
        "",
        vec![
            (
                "$hue, $saturation, $lightness, $alpha",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        hsl_impl(arena, config, state, "hsla", &args)
                    },
                ),
            ),
            (
                "$hue, $saturation, $lightness",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        hsl_impl(arena, config, state, "hsla", &args)
                    },
                ),
            ),
            (
                "$hue, $saturation",
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump| {
                        if is_special_variable(&args[0]) || is_special_variable(&args[1]) {
                            function_string(arena, "hsla", &args)
                        } else {
                            Err(Box::new(SassError::Script {
                                message: "Missing argument $lightness.".into(),
                                argument_name: None,
                            }))
                        }
                    },
                ),
            ),
            (
                "$channels",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        parse_channels(
                            config,
                            state,
                            arena,
                            "hsla",
                            "channels",
                            &args[0],
                            Some(space),
                        )
                    },
                ),
            ),
        ],
        arena,
    )
}

/// Implements the three- and four-argument `hsl()`/`hsla()` functions.
///
/// Mirrors [`rgb_impl`]: special numbers fall back to plain CSS, `$alpha`
/// clamps via `%`-or-unitless normalization, and the HSL arm of
/// [`color_from_channels`] warns for unitless saturation/lightness.
///
/// Ports Dart's `_hsl` (color.dart:1427).
fn hsl_impl<'compile, 'parse>(
    arena: &'compile Bump,
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    name: &str,
    args: &[Value<'parse>],
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let alpha = args.get(3);
    if args[0].is_special_number()
        || args[1].is_special_number()
        || args[2].is_special_number()
        || alpha.is_some_and(|a| a.is_special_number())
    {
        return function_string(arena, name, args);
    }
    let hue = value::assert_number(&args[0], Some("hue"))?;
    let sat = value::assert_number(&args[1], Some("saturation"))?;
    let lit = value::assert_number(&args[2], Some("lightness"))?;
    let mut a = 1.0;
    if let Some(av) = alpha {
        let an = value::assert_number(av, Some("alpha"))?;
        a = clamp_like_css(percentage_or_unitless(an, 1.0, "alpha")?, 0.0, 1.0);
    }
    color_from_channels(
        arena,
        config,
        state,
        ColorSpace::Hsl,
        Some(Value::new_with_arena(arena, ValueKind::Number(hue.clone()))),
        Some(Value::new_with_arena(arena, ValueKind::Number(sat.clone()))),
        Some(Value::new_with_arena(arena, ValueKind::Number(lit.clone()))),
        Some(a),
        false,
        true,
    )
}

/// Returns whether `space` uses the comma-separated plain-CSS fallback.
///
/// Only `rgb` and `hsl` do: when a special number sneaks into a three-channel
/// call in these spaces, the input re-emits as `name(c0, c1, c2[, alpha])` for
/// browser compatibility instead of `name(input)`.
///
/// Ports Dart's `_specialCommaSpaces` constant (color.dart:28).
fn special_comma_spaces(space: ColorSpace) -> bool {
    // Matches Dart: const _specialCommaSpaces = {ColorSpace.rgb, ColorSpace.hsl}
    // Matches Go:  var specialCommaSpaces = map[...]{Rgb: {}, Hsl: {}}
    matches!(space, ColorSpace::Rgb | ColorSpace::Hsl)
}

/// Parses channel-list `input` into a color, or echoes plain CSS when unresolvable.
///
/// With `space: Some`, parses three channels in that space; with `None`
/// (the `color()` function), the first channel names the space — except the
/// `rgb`/`hsl`/`hwb`/`lab`/`lch`/`oklab`/`oklch` spaces, which `color()`
/// rejects with a "use the `<space>()` function" error. `var()`/`from`-led /
/// special-number / slash-alpha inputs fall back to
/// [`function_string`](super::helpers::function_string); `name` attributes
/// errors to the source argument (`$channels`, `$description`, or empty for
/// the synthetic HWB call).
///
/// Ports Dart's `_parseChannels` (color.dart:1666).
pub fn parse_channels<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    function_name: &str,
    name: &str,
    input: &Value<'parse>,
    space: Option<ColorSpace>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    if is_special_variable(input) {
        return function_string(arena, function_name, &[*input]);
    }
    let (components, alpha_value) = match parse_slash_channels(arena, input, name)? {
        Some(SlashResult { components, alpha }) => (components, alpha),
        None => {
            return function_string(arena, function_name, &[*input]);
        }
    };
    let ch_list = assert_common_list_style(arena, &components, name, false)?;
    if ch_list.is_empty() {
        return Err(Box::new(SassError::Script {
            message: "Color component list may not be empty.".into(),
            argument_name: if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            },
        }));
    }
    if let ValueKind::String(s) = &*ch_list[0] {
        if !s.has_quotes && s.text.to_lowercase() == "from" {
            return function_string(arena, function_name, &[*input]);
        }
    }
    let channels: Vec<Value<'parse>>;
    let sp;
    if components.is_special_variable() {
        channels = ch_list;
        sp = space;
    } else if space.is_none() {
        let sn = value::assert_string(&ch_list[0], Some(name))?;
        sn.assert_unquoted().map_err(|e| SassError::Script {
            message: e.to_string(),
            argument_name: Some(name.to_string()),
        })?;
        if is_special_variable(&Value::new_with_arena(arena, ValueKind::String(sn.clone()))) {
            return function_string(arena, function_name, &[*input]);
        }
        let ps = ColorSpace::from_name(sn.text, Some(name))?;
        match ps { ColorSpace::Rgb|ColorSpace::Hsl|ColorSpace::Hwb|ColorSpace::Lab|ColorSpace::Lch|ColorSpace::Oklab|ColorSpace::Oklch =>
            return Err(Box::new(SassError::Script { message: format!("The color() function doesn't support the color space {}. Use the {}() function instead.", ps.name(), ps.name()), argument_name: if name.is_empty() { None } else { Some(name.to_string()) } })), _ => {} }
        channels = ch_list[1..].to_vec();
        sp = Some(ps);
    } else {
        channels = ch_list;
        sp = space;
    }

    for (i, ch) in channels.iter().enumerate() {
        if !ch.is_special_number() && !matches!(&**ch, ValueKind::Number(_)) && !is_none(ch) {
            let cn = if let Some(s) = sp {
                let chs = color_space_chs(s);
                if i < 3 {
                    chs[i].channel.name.to_string() + " channel"
                } else {
                    format!("channel {}", i + 1)
                }
            } else {
                format!("channel {}", i + 1)
            };
            let cs = ch.to_display_string()?;
            return Err(Box::new(SassError::Script {
                message: format!("Expected {cn} to be a number, was {cs}."),
                argument_name: if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                },
            }));
        }
    }

    let mut alpha = None;
    if let Some(ref av) = alpha_value {
        if av.is_special_number() {
            if let Some(s) = sp {
                if channels.len() == 3 && special_comma_spaces(s) {
                    let mut av2 = channels.clone();
                    av2.push(*av);
                    return function_string(arena, function_name, &av2);
                }
            }
            return function_string(arena, function_name, &[*input]);
        }
        if is_none(av) {
        } else if let ValueKind::Number(an) = &**av {
            alpha = Some(clamp_like_css(
                percentage_or_unitless(an, 1.0, "alpha")?,
                0.0,
                1.0,
            ));
        } else {
            value::assert_number(av, Some(name))?;
        }
    } else {
        alpha = Some(1.0);
    }
    let sp = match sp {
        Some(s) => s,
        None => return function_string(arena, function_name, &[*input]),
    };

    for ch in &channels {
        if ch.is_special_number() {
            if channels.len() == 3 && special_comma_spaces(sp) {
                let mut av2 = channels.clone();
                if let Some(ref av) = alpha_value {
                    av2.push(*av);
                }
                return function_string(arena, function_name, &av2);
            }
            return function_string(arena, function_name, &[*input]);
        }
    }
    if channels.len() != 3 {
        let is = input.to_display_string()?;
        return Err(Box::new(SassError::Script {
            message: format!(
                "The {} color space has 3 channels but {is} has {}.",
                sp.name(),
                channels.len()
            ),
            argument_name: if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            },
        }));
    }
    color_from_channels(
        arena,
        config,
        state,
        sp,
        Some(channels[0]),
        Some(channels[1]),
        Some(channels[2]),
        alpha,
        sp == ColorSpace::Rgb,
        true,
    )
}

// The `(components, alpha)` record returned by `_parseSlashChannels`
// (color.dart:1797): the space-separated component list plus the `/`-alpha,
// or `None` from the caller when the input should echo as plain CSS.
struct SlashResult<'parse> {
    components: Value<'parse>,
    alpha: Option<Value<'parse>>,
}

/// Splits `input` into space-separated components plus an optional `/`-alpha.
///
/// Handles slash-separated lists, trailing `channel/alpha` strings, and
/// `as-slash` numbers. Returns `None` when the shape can't be parsed and the
/// caller should echo the input as plain CSS (multi-`/` strings).
///
/// Ports Dart's `_parseSlashChannels` (color.dart:1797).
fn parse_slash_channels<'compile, 'parse>(
    arena: &'compile Bump,
    input: &Value<'parse>,
    name: &str,
) -> SassResult<Option<SlashResult<'parse>>>
where
    'compile: 'parse,
{
    let list = assert_common_list_style(arena, input, name, true)?;
    match input.separator() {
        ListSeparator::Slash => {
            if list.len() == 2 {
                Ok(Some(SlashResult {
                    components: list[0],
                    alpha: Some(list[1]),
                }))
            } else {
                Err(Box::new(SassError::Script {
                    message: format!(
                        "Only 2 slash-separated elements allowed, but {} {} passed.",
                        list.len(),
                        if list.len() == 1 { "was" } else { "were" }
                    ),
                    argument_name: if name.is_empty() {
                        None
                    } else {
                        Some(name.to_string())
                    },
                }))
            }
        }
        _ => {
            if !list.is_empty() {
                let li = list.len() - 1;
                if let ValueKind::String(s) = &*list[li] {
                    if !s.has_quotes {
                        let parts: Vec<&str> = s.text.split('/').collect();
                        return match parts.len() {
                            1 => Ok(Some(SlashResult {
                                components: *input,
                                alpha: None,
                            })),
                            2 => {
                                let ch = value_or_string(parts[0].trim(), arena)?;
                                let al = value_or_string(parts[1].trim(), arena)?;
                                let mut nl: Vec<Value<'parse>> = list[..li].to_vec();
                                nl.push(ch);
                                Ok(Some(SlashResult {
                                    components: Value::new_with_arena(
                                        arena,
                                        ValueKind::List(SassList::new(
                                            nl,
                                            ListSeparator::Space,
                                            false,
                                        )),
                                    ),
                                    alpha: Some(al),
                                }))
                            }
                            _ => Ok(None),
                        };
                    }
                }
                if let ValueKind::Number(n) = &*list[li] {
                    if n.has_slash() {
                        if let Some((before, after)) = n.slash_pair() {
                            let mut nl: Vec<Value<'parse>> = list[..li].to_vec();
                            nl.push(Value::new_with_arena(
                                arena,
                                ValueKind::Number(before.clone()),
                            ));
                            let sl = SassList::new(nl, ListSeparator::Space, false);
                            return Ok(Some(SlashResult {
                                components: Value::new_with_arena(arena, ValueKind::List(sl)),
                                alpha: Some(Value::new_with_arena(
                                    arena,
                                    ValueKind::Number(after.clone()),
                                )),
                            }));
                        }
                    }
                }
            }
            Ok(Some(SlashResult {
                components: *input,
                alpha: None,
            }))
        }
    }
}

/// Parses `s` as a number, falling back to an unquoted string.
///
/// Ports Dart's `_parseNumberOrString` (color.dart:1831): `ScssParser`
/// number parse, `SassString` on `SassFormatException`.
fn value_or_string<'compile, 'parse>(s: &'_ str, arena: &'compile Bump) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let source = FileSource::new_in(arena, s, None);
    let mut scanner = SpanScanner::new(source);
    let state = ParserState {
        syntax: Syntax::Scss,
        interpolation_map: None,
        in_expression: false,
    };

    if let Ok(num_expr) = number_impl(&mut scanner, &state) {
        if scanner.is_done() {
            return Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(SassNumber::new(num_expr.value, num_expr.unit.as_deref())),
            ));
        }
    }

    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(s), false)),
    ))
}

/// Creates the module `hwb()` overloads.
///
/// The 4-arg form synthesizes a slash-separated channel list and re-enters
/// [`parse_channels`] with an empty attribution name (matching Dart's unnamed
/// `_parseChannels('hwb', ...)` call); the 1-arg form forwards `$channels`.
///
/// Ports Dart's `BuiltInCallable.overloadedFunction("hwb", ...)`
/// (color.dart:511).
pub fn hwb_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::overloaded_function(
        "hwb",
        "",
        vec![
            (
                "$hue, $whiteness, $blackness, $alpha: 1",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        let inner = SassList::new(
                            vec![args[0], args[1], args[2]],
                            ListSeparator::Space,
                            false,
                        );
                        let outer = SassList::new(
                            vec![
                                Value::new_with_arena(arena, ValueKind::List(inner)),
                                args[3],
                            ],
                            ListSeparator::Slash,
                            false,
                        );
                        parse_channels(
                            config,
                            state,
                            arena,
                            "hwb",
                            "",
                            &Value::new_with_arena(arena, ValueKind::List(outer)),
                            Some(ColorSpace::Hwb),
                        )
                    },
                ),
            ),
            (
                "$channels",
                Rc::new(
                    move |config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          arena: &'compile Bump| {
                        parse_channels(
                            config,
                            state,
                            arena,
                            "hwb",
                            "channels",
                            &args[0],
                            Some(ColorSpace::Hwb),
                        )
                    },
                ),
            ),
        ],
        arena,
    )
}

/// Creates the deprecated-global `hwb()` function (`$channels`).
///
/// Same body as the 1-arg module overload; the global registry adds the
/// `color` deprecation warning.
///
/// Ports Dart's `_function("hwb", r"$channels", ...)` global entry
/// (color.dart:386).
pub fn hwb_global_callable<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "hwb",
        "$channels",
        "sass:color",
        arena,
        Rc::new(
            move |config: &EvalConfig<'compile, 'parse>,
                  state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump| {
                parse_channels(
                    config,
                    state,
                    arena,
                    "hwb",
                    "channels",
                    &args[0],
                    Some(ColorSpace::Hwb),
                )
            },
        ),
    )
}

/// Creates the `lab()` function (`$channels`).
///
/// Forwards to [`parse_channels`] in `lab`; a `null` `$channels` throws
/// `Missing argument $channels.` (the test harness fills omitted args with
/// null — see ref/functions.md).
///
/// Ports Dart's `_function("lab", r"$channels", ...)` entries
/// (color.dart:397 global, module list).
pub fn lab_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "lab",
        "$channels",
        "sass:color",
        arena,
        Rc::new(
            move |config: &EvalConfig<'compile, 'parse>,
                  state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump| {
                if matches!(&*args[0], ValueKind::Null) {
                    return Err(Box::new(SassError::Script {
                        message: "Missing argument $channels.".into(),
                        argument_name: None,
                    }));
                }
                parse_channels(
                    config,
                    state,
                    arena,
                    "lab",
                    "channels",
                    &args[0],
                    Some(ColorSpace::Lab),
                )
            },
        ),
    )
}
/// Creates the `lch()` function (`$channels`).
///
/// Same shape as [`lab_callable`], in `lch`.
///
/// Ports Dart's `_function("lch", r"$channels", ...)` entries (color.dart:408).
pub fn lch_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "lch",
        "$channels",
        "sass:color",
        arena,
        Rc::new(
            move |config: &EvalConfig<'compile, 'parse>,
                  state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump| {
                if matches!(&*args[0], ValueKind::Null) {
                    return Err(Box::new(SassError::Script {
                        message: "Missing argument $channels.".into(),
                        argument_name: None,
                    }));
                }
                parse_channels(
                    config,
                    state,
                    arena,
                    "lch",
                    "channels",
                    &args[0],
                    Some(ColorSpace::Lch),
                )
            },
        ),
    )
}
/// Creates the `oklab()` function (`$channels`).
///
/// Same shape as [`lab_callable`], in `oklab`.
///
/// Ports Dart's `_function("oklab", r"$channels", ...)` entries (color.dart:420).
pub fn oklab_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "oklab",
        "$channels",
        "sass:color",
        arena,
        Rc::new(
            move |config: &EvalConfig<'compile, 'parse>,
                  state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump| {
                if matches!(&*args[0], ValueKind::Null) {
                    return Err(Box::new(SassError::Script {
                        message: "Missing argument $channels.".into(),
                        argument_name: None,
                    }));
                }
                parse_channels(
                    config,
                    state,
                    arena,
                    "oklab",
                    "channels",
                    &args[0],
                    Some(ColorSpace::Oklab),
                )
            },
        ),
    )
}
/// Creates the `oklch()` function (`$channels`).
///
/// Same shape as [`lab_callable`], in `oklch`.
///
/// Ports Dart's `_function("oklch", r"$channels", ...)` entries (color.dart:431).
pub fn oklch_callable<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "oklch",
        "$channels",
        "sass:color",
        arena,
        Rc::new(
            move |config: &EvalConfig<'compile, 'parse>,
                  state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump| {
                if matches!(&*args[0], ValueKind::Null) {
                    return Err(Box::new(SassError::Script {
                        message: "Missing argument $channels.".into(),
                        argument_name: None,
                    }));
                }
                parse_channels(
                    config,
                    state,
                    arena,
                    "oklch",
                    "channels",
                    &args[0],
                    Some(ColorSpace::Oklch),
                )
            },
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::functions::test_utils::*;

    use crate::value::ValueKind;

    #[rust_sass_macros::maybe_test]
    async fn test_rgb() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &rgb_callable(&arena),
            &[num(&arena, 255.0), num(&arena, 0.0), num(&arena, 0.0)],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => {
                assert_eq!(c.space, ColorSpace::Rgb);
                assert!((c.channel0 - 255.0).abs() < 0.01);
            }
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_rgba() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &rgba_callable(&arena),
            &[
                num(&arena, 255.0),
                num(&arena, 0.0),
                num(&arena, 0.0),
                num(&arena, 0.5),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert!((c.alpha - 0.5).abs() < 0.01),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_hsl() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &hsl_callable(&arena),
            &[
                num(&arena, 0.0),
                percent(&arena, 100.0),
                percent(&arena, 50.0),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert_eq!(c.space, ColorSpace::Hsl),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_hsl_missing_lightness() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &hsl_callable(&arena),
            &[num(&arena, 0.0), num(&arena, 100.0)],
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Script { message, .. } => {
                assert_eq!(message, "Missing argument $lightness.")
            }
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_hwb() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &hwb_callable(&arena),
            &[
                num(&arena, 0.0),
                percent(&arena, 0.0),
                percent(&arena, 0.0),
                num(&arena, 1.0),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert_eq!(c.space, ColorSpace::Hwb),
            _ => panic!(),
        }
    }
    #[rust_sass_macros::maybe_test]
    async fn test_lab_missing() {
        let arena = Bump::new();
        let err = eval(&arena, &lab_callable(&arena), &[]).await.unwrap_err();
        match *err {
            SassError::Script { message, .. } => assert_eq!(message, "Missing argument $channels."),
            _ => panic!(),
        }
    }
}
