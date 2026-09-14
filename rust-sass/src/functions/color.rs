// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/functions/color.dart (registries: global + module lists;
//   module-only space/to-space/is-legacy/is-missing/is-in-gamut/to-gamut/channel/
//   same/is-powerless/color closures below)
// go-source: go/functions/color.go

use crate::functions::color_helpers::wrap_unquoted_err;
use crate::functions::color_spaces::parse_channels;
use crate::util::number::fuzzy_equals;
use crate::value::color_conversions_base::space_channels;
use crate::value::color_conversions_base::GamutMapMethod;
use crate::value::SassString;
use std::rc::Rc;

use bumpalo::Bump;

use crate::callable::{BuiltInCallable, Callable, CallableKind};
use crate::common::exception::{SassError, SassResult};
use crate::eval::{EvalConfig, EvalState};
use crate::module::BuiltInModule;
use crate::value::Value;
use crate::value::{
    self, color::ColorSpace, SassColor, SassNumber, ValueKind, SASS_FALSE, SASS_TRUE,
};

use crate::functions::color_adjust::{adjust_callable, change_callable, scale_callable};
use crate::functions::color_helpers::{
    channel_function, channel_function_global, channel_function_global_with_unit,
    channel_function_with_unit, channel_name_from_arg, color_in_space, removed_color_function,
    removed_color_function_negative, to_xyz_no_missing,
};
use crate::functions::color_manipulation::{
    adjust_hue_callable, alpha_callable, alpha_module_callable, complement_callable,
    darken_callable, desaturate_callable, grayscale_callable, grayscale_module_callable,
    ie_hex_str_callable, invert_callable, invert_module_callable, lighten_callable, mix_callable,
    opacify_callable, opacity_callable, opacity_module_callable, saturate_callable,
    transparentize_callable,
};
use crate::functions::color_spaces::{
    hsl_callable, hsla_callable, hwb_callable, hwb_global_callable, lab_callable, lch_callable,
    oklab_callable, oklch_callable, rgb_callable, rgba_callable,
};

// ============================================================================
// GlobalColorFunctions
// ============================================================================
// These are all deprecated globals. Each wraps a module function with
// deprecation warnings. The underlying callables come from color_helpers.rs,
// color_spaces.rs, color_manipulation.rs, and color_adjust.rs.
//
// Ports Dart's `global` list (color.dart:31): channel getters with
// `.withDeprecationWarning("color")`, `rgb`/`rgba`/`hsl`/`hsla` overload sets
// shared with the module, `adjust-color`/`scale-color`/`change-color` renames,
// and plain-CSS fallthrough arms (`grayscale`/`saturate`/`alpha`/`opacity`,
// two-arg `rgb`, two-arg `hsl` special-variable checks).
/// Returns all globally-available color functions.
///
/// Ports Dart's `global` list (color.dart:31): the module callables wrapped
/// with a `color` deprecation warning (plus the `adjust` second-arg renames
/// for `adjust-hue`/`lighten`/`darken`/`desaturate`/`opacify`/…), except the
/// shared `rgb`/`rgba`/`hsl`/`hsla` overload sets which warn per-call instead.
pub fn global_color_functions<'compile, 'parse>(
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
        c!(
            arena,
            channel_function_global("red", ColorSpace::Rgb, SassColor::red, arena,)
                .with_deprecation_warning("color", None)
        ),
        c!(
            arena,
            channel_function_global("green", ColorSpace::Rgb, SassColor::green, arena,)
                .with_deprecation_warning("color", None)
        ),
        c!(
            arena,
            channel_function_global("blue", ColorSpace::Rgb, SassColor::blue, arena,)
                .with_deprecation_warning("color", None)
        ),
        c!(
            arena,
            mix_callable(arena).with_deprecation_warning("color", None)
        ),
        c!(arena, rgb_callable(arena)),
        c!(arena, rgba_callable(arena)),
        c!(arena, invert_callable(arena)),
        c!(arena,
            channel_function_global_with_unit("hue", ColorSpace::Hsl, SassColor::hue, "deg", arena,)
                .with_deprecation_warning("color", None)
        ),
        c!(
            arena,
            channel_function_global_with_unit(
                "saturation",
                ColorSpace::Hsl,
                SassColor::saturation,
                "%",
                arena,
            )
            .with_deprecation_warning("color", None)
        ),
        c!(
            arena,
            channel_function_global_with_unit(
                "lightness",
                ColorSpace::Hsl,
                SassColor::lightness,
                "%",
                arena,
            )
            .with_deprecation_warning("color", None)
        ),
        c!(arena, hsl_callable(arena)),
        c!(arena, hsla_callable(arena)),
        c!(arena, grayscale_callable(arena)),
        c!(
            arena,
            adjust_hue_callable(arena).with_deprecation_warning("color", Some("adjust"))
        ),
        c!(
            arena,
            lighten_callable(arena).with_deprecation_warning("color", Some("adjust"))
        ),
        c!(
            arena,
            darken_callable(arena).with_deprecation_warning("color", Some("adjust"))
        ),
        c!(arena, saturate_callable(arena)),
        c!(
            arena,
            desaturate_callable(arena).with_deprecation_warning("color", Some("adjust"))
        ),
        c!(
            arena,
            opacify_callable("opacify", arena).with_deprecation_warning("color", Some("adjust"))
        ),
        c!(
            arena,
            opacify_callable("fade-in", arena).with_deprecation_warning("color", Some("adjust"))
        ),
        c!(
            arena,
            transparentize_callable("transparentize", arena)
                .with_deprecation_warning("color", Some("adjust"))
        ),
        c!(
            arena,
            transparentize_callable("fade-out", arena)
                .with_deprecation_warning("color", Some("adjust"))
        ),
        c!(arena, alpha_callable(arena)),
        c!(arena, opacity_callable(arena)),
        c!(arena, color_function_gl(arena)),
        c!(arena, hwb_global_callable(arena)),
        c!(arena, lab_callable(arena)),
        c!(arena, lch_callable(arena)),
        c!(arena, oklab_callable(arena)),
        c!(arena, oklch_callable(arena)),
        c!(
            arena,
            complement_callable(arena).with_deprecation_warning("color", None)
        ),
        c!(arena, ie_hex_str_callable(arena)),
        c!(
            arena,
            adjust_callable(arena)
                .with_deprecation_warning("color", None)
                .with_name("adjust-color".into())
        ),
        c!(
            arena,
            scale_callable(arena)
                .with_deprecation_warning("color", None)
                .with_name("scale-color".into())
        ),
        c!(
            arena,
            change_callable(arena)
                .with_deprecation_warning("color", None)
                .with_name("change-color".into())
        ),
    ]
}

// ============================================================================
// ColorModule
// ============================================================================

/// Returns the sass:color built-in module.
///
/// Ports Dart's `module` list (color.dart:451): channel getters, `mix`,
/// `invert`, removed-function stubs (`adjust-hue`/`lighten`/`darken`/…),
/// `grayscale`, HWB/Lab-family constructors, `alpha`/`opacity`,
/// `space`/`to-space`/`is-legacy`/`is-missing`/`is-in-gamut`/`to-gamut`/
/// `channel`/`same`/`is-powerless`/`complement`, and `adjust`/`scale`/`change`/
/// `ie-hex-str`.
pub fn color_module<'compile, 'parse>(arena: &'compile Bump) -> BuiltInModule<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    macro_rules! c {
        ($arena:expr, $f:expr) => {
            Callable::new($arena, CallableKind::BuiltIn($f))
        };
    }
    let fns: Vec<Callable<'compile, 'parse>> = vec![
        c!(
            arena,
            channel_function("red", ColorSpace::Rgb, SassColor::red, arena)
        ),
        c!(
            arena,
            channel_function("green", ColorSpace::Rgb, SassColor::green, arena,)
        ),
        c!(
            arena,
            channel_function("blue", ColorSpace::Rgb, SassColor::blue, arena,)
        ),
        c!(arena, mix_callable(arena)),
        c!(arena, invert_module_callable(arena)),
        c!(
            arena,
            channel_function_with_unit("hue", ColorSpace::Hsl, SassColor::hue, "deg", arena,)
        ),
        c!(
            arena,
            channel_function_with_unit(
                "saturation",
                ColorSpace::Hsl,
                SassColor::saturation,
                "%",
                arena,
            )
        ),
        c!(
            arena,
            channel_function_with_unit(
                "lightness",
                ColorSpace::Hsl,
                SassColor::lightness,
                "%",
                arena,
            )
        ),
        c!(arena, removed_color_function("adjust-hue", "hue", arena)),
        c!(arena, removed_color_function("lighten", "lightness", arena)),
        c!(
            arena,
            removed_color_function_negative("darken", "lightness", arena)
        ),
        c!(
            arena,
            removed_color_function("saturate", "saturation", arena)
        ),
        c!(
            arena,
            removed_color_function_negative("desaturate", "saturation", arena,)
        ),
        c!(arena, grayscale_module_callable(arena)),
        c!(arena, hwb_callable(arena)),
        c!(
            arena,
            channel_function_with_unit(
                "whiteness",
                ColorSpace::Hwb,
                SassColor::whiteness,
                "%",
                arena,
            )
        ),
        c!(
            arena,
            channel_function_with_unit(
                "blackness",
                ColorSpace::Hwb,
                SassColor::blackness,
                "%",
                arena,
            )
        ),
        c!(arena, removed_color_function("opacify", "alpha", arena)),
        c!(arena, removed_color_function("fade-in", "alpha", arena)),
        c!(
            arena,
            removed_color_function_negative("transparentize", "alpha", arena,)
        ),
        c!(
            arena,
            removed_color_function_negative("fade-out", "alpha", arena)
        ),
        c!(arena, alpha_module_callable(arena)),
        c!(arena, opacity_module_callable(arena)),
        c!(arena, space_function(arena)),
        c!(arena, to_space_function(arena)),
        c!(arena, is_legacy_function(arena)),
        c!(arena, is_missing_function(arena)),
        c!(arena, is_in_gamut_function(arena)),
        c!(arena, to_gamut_function(arena)),
        c!(arena, channel_function_sass(arena)),
        c!(arena, same_function(arena)),
        c!(arena, is_powerless_function(arena)),
        c!(arena, complement_callable(arena)),
        c!(arena, adjust_callable(arena)),
        c!(arena, scale_callable(arena)),
        c!(arena, change_callable(arena)),
        c!(arena, ie_hex_str_callable(arena)),
    ];
    BuiltInModule::new(arena, "color".into(), &fns, &[], indexmap::IndexMap::new())
}

// ============================================================================
// Module-only inline functions (defined in color.go)
// ============================================================================

// The `space`, `to-space`, `is-legacy`, `is-missing`, `is-in-gamut`,
// `to-gamut`, `channel`, `same`, `is-powerless`, and `color` closures have no
// Go/Dart helper of their own: each is an inline `_function(...)` entry in
// Dart's `module` list (color.dart:451), ported here one `*_function` each
// (split rule: the `module` registry lives here while shared helpers live in
// color_helpers.rs / color_spaces_shared.rs).

/// Creates the `color()` function (`$description`).
///
/// Parses `$description` as a `color()`-style channel list with no preset
/// space (`None` space arg to [`parse_channels`](super::color_spaces::parse_channels)).
///
/// Ports Dart's inline `_function("color", r"$description", ...)`
/// (color.dart:380).
fn color_function_gl<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    BuiltInCallable::function(
        "color",
        "$description",
        "sass:color",
        arena,
        Rc::new(
            move |config: &EvalConfig<'compile, 'parse>,
                  state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                parse_channels(config, state, arena, "color", "description", &args[0], None)
            },
        ),
    )
}

/// Creates the `color.space()` function (`$color`).
///
/// Returns the color's space name as an unquoted string.
///
/// Ports Dart's inline `_function("space", r"$color", ...)` (color.dart:619).
fn space_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    BuiltInCallable::function(
        "space",
        "$color",
        "sass:color",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let col = value::assert_color(&args[0], Some("color"))?;
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::String(SassString::new(arena.alloc_str(col.space.name()), false)),
                ))
            },
        ),
    )
}

/// Creates the `color.to-space()` function (`$color, $space`).
///
/// Converts via `_colorInSpace` with `legacyMissing: false`, so legacy spaces
/// never return missing channels (callers pick a legacy space for maximum
/// compatibility). `$space: null` returns the input unchanged.
///
/// Ports Dart's inline `_function("to-space", ...)` (color.dart:631) plus its
/// `// color.to-space() never returns missing channels` rationale above it.
fn to_space_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    BuiltInCallable::function(
        "to-space",
        "$color, $space",
        "sass:color",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let col = value::assert_color(&args[0], Some("color"))?;
                // Matches Dart: to-space routes through _colorInSpace, which
                // returns the color unchanged for $space: null.
                if matches!(&*args[1], ValueKind::Null) {
                    return Ok(Value::new_with_arena(arena, ValueKind::Color(col.clone())));
                }
                let s = value::assert_string(&args[1], Some("space"))?;
                s.assert_unquoted()
                    .map_err(|e| wrap_unquoted_err(e, "space"))?;
                let space = ColorSpace::from_name(s.text, Some("space"))?;
                let result = col.to_space(space, Some(false))?;
                Ok(Value::new_with_arena(arena, ValueKind::Color(result)))
            },
        ),
    )
}

/// Creates the `color.is-legacy()` function (`$color`).
///
/// Ports Dart's inline `_function("is-legacy", ...)` (color.dart:638).
fn is_legacy_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    BuiltInCallable::function(
        "is-legacy",
        "$color",
        "sass:color",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let col = value::assert_color(&args[0], Some("color"))?;
                if col.is_legacy() {
                    Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_TRUE)))
                } else {
                    Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)))
                }
            },
        ),
    )
}

/// Creates the `color.is-missing()` function (`$color, $channel`).
///
/// Asserts `$channel` is quoted (via `channel_name_from_arg`) and reports
/// whether that channel is missing, with `color`/`channel` attribution.
///
/// Ports Dart's inline `_function("is-missing", ...)` (color.dart:644).
fn is_missing_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    BuiltInCallable::function(
        "is-missing",
        "$color, $channel",
        "sass:color",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let col = value::assert_color(&args[0], Some("color"))?;
                let channel = channel_name_from_arg(&args[1])?;
                let is_missing = col.is_channel_missing_by_name(&channel)?;
                if is_missing {
                    Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_TRUE)))
                } else {
                    Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)))
                }
            },
        ),
    )
}

/// Creates the `color.is-in-gamut()` function (`$color, $space: null`).
///
/// Converts with `_colorInSpace` defaults (`legacyMissing: true`) before the
/// gamut check.
///
/// Ports Dart's inline `_function("is-in-gamut", ...)` (color.dart:656).
fn is_in_gamut_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    BuiltInCallable::function(
        "is-in-gamut",
        "$color, $space: null",
        "sass:color",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let col = value::assert_color(&args[0], Some("color"))?;
                let working_color = color_in_space(col, &args[1], None)?;
                if working_color.is_in_gamut() {
                    Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_TRUE)))
                } else {
                    Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)))
                }
            },
        ),
    )
}

/// Creates the `color.to-gamut()` function (`$color, $space: null, $method: null`).
///
/// `$method` is required (forwards-compat with CSS spec changes); it is parsed
/// before the unbounded-space early return so invalid method names always
/// error. Unbounded spaces return the color unchanged, otherwise maps
/// `toSpace(space).toGamut(method)` back into the original space with
/// `legacyMissing: false`.
///
/// Ports Dart's inline `_function("to-gamut", ...)` (color.dart:663).
fn to_gamut_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    BuiltInCallable::function(
        "to-gamut",
        "$color, $space: null, $method: null",
        "sass:color",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let col = value::assert_color(&args[0], Some("color"))?;
                let space = if !matches!(&*args[1], ValueKind::Null) {
                    let s = value::assert_string(&args[1], Some("space"))?;
                    s.assert_unquoted()
                        .map_err(|e| wrap_unquoted_err(e, "space"))?;
                    ColorSpace::from_name(s.text, Some("space"))?
                } else {
                    col.space
                };
                if matches!(&*args[2], ValueKind::Null) {
                    return Err(Box::new(SassError::Script {
                        message: "color.to-gamut() requires a $method argument for forwards-\
                                  compatibility with changes in the CSS spec. Suggestion:\n\
                                  \n\
                                  $method: local-minde"
                            .into(),
                        argument_name: Some("method".into()),
                    }));
                }
                let method_s = value::assert_string(&args[2], Some("method"))?;
                method_s
                    .assert_unquoted()
                    .map_err(|e| wrap_unquoted_err(e, "method"))?;
                let method = GamutMapMethod::from_name_with_arg(method_s.text, None)?;
                if !space.is_bounded() {
                    return Ok(Value::new_with_arena(arena, ValueKind::Color(col.clone())));
                }
                let space_converted = col.to_space(space, None)?;
                let gamut_color = space_converted.to_gamut(method)?;
                let result = gamut_color.to_space(col.space, Some(false))?;
                Ok(Value::new_with_arena(arena, ValueKind::Color(result)))
            },
        ),
    )
}

/// Creates the `color.channel()` function (`$color, $channel, $space: null`).
///
/// Converts with `_colorInSpace` defaults, returns `alpha` directly, and
/// rescales `%`-unit channels (`value * 100 / max`) for the returned number.
/// Unknown channels throw `Color <css> has no channel named <name>.` with
/// `$channel` attribution.
///
/// Ports Dart's inline `_function("channel", ...)` (color.dart:689).
fn channel_function_sass<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    BuiltInCallable::function(
        "channel",
        "$color, $channel, $space: null",
        "sass:color",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let col = value::assert_color(&args[0], Some("color"))?;
                let working_color = color_in_space(col, &args[2], None)?;
                let channel_name = channel_name_from_arg(&args[1])?;
                if channel_name == "alpha" {
                    return Ok(Value::new_with_arena(
                        arena,
                        ValueKind::Number(SassNumber::new(working_color.alpha, None)),
                    ));
                }
                let chs = space_channels(working_color.space);
                let mut found = false;
                let mut channel_value = 0.0f64;
                let mut unit = "";
                for (i, ch) in chs.iter().enumerate() {
                    if ch.channel.name == channel_name {
                        found = true;
                        channel_value = match i {
                            0 => working_color.channel0,
                            1 => working_color.channel1,
                            2 => working_color.channel2,
                            _ => unreachable!(),
                        };
                        unit = ch.channel.associated_unit;
                        if unit == "%" {
                            channel_value = channel_value * 100.0 / ch.max;
                        }
                        break;
                    }
                }
                if !found {
                    let col_str = working_color.to_display_string()?;
                    return Err(Box::new(SassError::Script {
                        message: format!("Color {col_str} has no channel named {channel_name}."),
                        argument_name: Some("channel".into()),
                    }));
                }
                let num = if unit.is_empty() {
                    SassNumber::new(channel_value, None)
                } else {
                    SassNumber::new(channel_value, Some(unit))
                };
                Ok(Value::new_with_arena(arena, ValueKind::Number(num)))
            },
        ),
    )
}

/// Creates the `color.same()` function (`$color1, $color2`).
///
/// Same-space colors compare channel-wise with fuzzy equality; different
/// spaces compare after `toXyzNoMissing` conversion (missing channels as zero)
/// with exact equality.
///
/// Ports Dart's inline `_function("same", ...)` (color.dart:714) including the
/// local `toXyzNoMissing` helper (now [`to_xyz_no_missing`](super::color_helpers::to_xyz_no_missing)).
fn same_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    BuiltInCallable::function(
        "same",
        "$color1, $color2",
        "sass:color",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let col1 = value::assert_color(&args[0], Some("color1"))?;
                let col2 = value::assert_color(&args[1], Some("color2"))?;
                let same = if col1.space == col2.space {
                    fuzzy_equals(col1.channel0, col2.channel0)
                        && fuzzy_equals(col1.channel1, col2.channel1)
                        && fuzzy_equals(col1.channel2, col2.channel2)
                        && fuzzy_equals(col1.alpha, col2.alpha)
                } else {
                    let x1 = to_xyz_no_missing(col1)?;
                    let x2 = to_xyz_no_missing(col2)?;
                    x1.equals(&x2)
                };
                if same {
                    Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_TRUE)))
                } else {
                    Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)))
                }
            },
        ),
    )
}

/// Creates the `color.is-powerless()` function (`$color, $channel, $space: null`).
///
/// Converts with `_colorInSpace` defaults, then delegates to the color's
/// powerless-channel check with `color`/`channel` attribution.
///
/// Ports Dart's inline `_function("is-powerless", ...)` (color.dart:754).
fn is_powerless_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    BuiltInCallable::function(
        "is-powerless",
        "$color, $channel, $space: null",
        "sass:color",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let col = value::assert_color(&args[0], Some("color"))?;
                let working_color = color_in_space(col, &args[2], None)?;
                let channel = channel_name_from_arg(&args[1])?;
                let is_powerless = working_color.is_channel_powerless_by_name(&channel)?;
                if is_powerless {
                    Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_TRUE)))
                } else {
                    Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)))
                }
            },
        ),
    )
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[allow(clippy::approx_constant)]
mod tests {
    use super::*;
    use crate::value::ListSeparator;
    use crate::value::SassList;
    use crate::value::SassString;

    use crate::value::{color::ColorSpace, SassColor, ValueKind};
    use std::collections::HashSet;

    use crate::functions::test_utils::*;

    // ========================================================================
    // GlobalColorFunctions
    // ========================================================================

    #[rust_sass_macros::maybe_test]
    async fn test_global_color_functions() {
        let arena = Bump::new();
        let fns = global_color_functions(&arena);
        let names: HashSet<&str> = fns.iter().map(|c| c.name()).collect();
        let expected = [
            "red",
            "green",
            "blue",
            "mix",
            "rgb",
            "rgba",
            "invert",
            "hue",
            "saturation",
            "lightness",
            "hsl",
            "hsla",
            "grayscale",
            "adjust-hue",
            "lighten",
            "darken",
            "saturate",
            "desaturate",
            "opacify",
            "fade-in",
            "transparentize",
            "fade-out",
            "alpha",
            "opacity",
            "color",
            "hwb",
            "lab",
            "lch",
            "oklab",
            "oklch",
            "complement",
            "ie-hex-str",
            "adjust-color",
            "scale-color",
            "change-color",
        ];
        for name in expected {
            assert!(names.contains(name), "missing global: {name}");
        }
    }

    // ========================================================================
    // ColorModule
    // ========================================================================
    // space
    // ========================================================================

    #[rust_sass_macros::maybe_test]
    async fn test_space_rgb() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &space_function(&arena),
            &[color_rgb(&arena, 255.0, 0.0, 0.0, 1.0)],
        )
        .await
        .unwrap();
        assert_str(&v, "rgb");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_space_hsl() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &space_function(&arena),
            &[color_hsl(&arena, 120.0, 100.0, 50.0, 1.0)],
        )
        .await
        .unwrap();
        assert_str(&v, "hsl");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_space_srgb() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &space_function(&arena),
            &[color_srgb(&arena, 0.5, 0.5, 0.5, 1.0)],
        )
        .await
        .unwrap();
        assert_str(&v, "srgb");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_space_lab() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &space_function(&arena),
            &[color_lab(&arena, 50.0, 0.0, 0.0, 1.0)],
        )
        .await
        .unwrap();
        assert_str(&v, "lab");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_space_error_not_color() {
        let arena = Bump::new();
        let err = eval(&arena, &space_function(&arena), &[num(&arena, 42.0)])
            .await
            .unwrap_err();
        assert_script_err(err, "42 is not a color.", Some("color"));
    }

    // ========================================================================
    // is-legacy
    // ========================================================================

    #[rust_sass_macros::maybe_test]
    async fn test_is_legacy_rgb() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_legacy_function(&arena),
            &[color_rgb(&arena, 255.0, 0.0, 0.0, 1.0)],
        )
        .await
        .unwrap();
        assert_is_true(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_legacy_hsl() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_legacy_function(&arena),
            &[color_hsl(&arena, 120.0, 100.0, 50.0, 1.0)],
        )
        .await
        .unwrap();
        assert_is_true(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_legacy_hwb() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_legacy_function(&arena),
            &[color_hwb(&arena, 0.0, 0.0, 0.0, 1.0)],
        )
        .await
        .unwrap();
        assert_is_true(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_legacy_srgb_false() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_legacy_function(&arena),
            &[color_srgb(&arena, 0.5, 0.5, 0.5, 1.0)],
        )
        .await
        .unwrap();
        assert_is_false(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_legacy_lab_false() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_legacy_function(&arena),
            &[color_lab(&arena, 50.0, 0.0, 0.0, 1.0)],
        )
        .await
        .unwrap();
        assert_is_false(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_legacy_error_not_color() {
        let arena = Bump::new();
        let err = eval(&arena, &is_legacy_function(&arena), &[num(&arena, 42.0)])
            .await
            .unwrap_err();
        assert_script_err(err, "42 is not a color.", Some("color"));
    }

    // ========================================================================
    // is-missing
    // ========================================================================

    #[rust_sass_macros::maybe_test]
    async fn test_is_missing_none() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_missing_function(&arena),
            &[
                color_rgb(&arena, 255.0, 0.0, 0.0, 1.0),
                quoted(&arena, "red"),
            ],
        )
        .await
        .unwrap();
        assert_is_false(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_missing_red() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_missing_function(&arena),
            &[
                color_for_space(
                    &arena,
                    ColorSpace::Rgb,
                    0.0,
                    0.0,
                    0.0,
                    1.0,
                    [true, false, false, false],
                ),
                quoted(&arena, "red"),
            ],
        )
        .await
        .unwrap();
        assert_is_true(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_missing_alpha() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_missing_function(&arena),
            &[
                color_for_space(
                    &arena,
                    ColorSpace::Rgb,
                    255.0,
                    0.0,
                    0.0,
                    0.5,
                    [false, false, false, true],
                ),
                quoted(&arena, "alpha"),
            ],
        )
        .await
        .unwrap();
        assert_is_true(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_missing_different_channel() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_missing_function(&arena),
            &[
                color_for_space(
                    &arena,
                    ColorSpace::Rgb,
                    0.0,
                    0.0,
                    0.0,
                    1.0,
                    [true, false, false, false],
                ),
                quoted(&arena, "green"),
            ],
        )
        .await
        .unwrap();
        assert_is_false(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_missing_error_not_color() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &is_missing_function(&arena),
            &[num(&arena, 42.0), quoted(&arena, "red")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "42 is not a color.", Some("color"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_missing_error_channel_not_string() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &is_missing_function(&arena),
            &[color_rgb(&arena, 255.0, 0.0, 0.0, 1.0), num(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("channel"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_missing_error_channel_unquoted() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &is_missing_function(&arena),
            &[
                color_rgb(&arena, 255.0, 0.0, 0.0, 1.0),
                sass_string(&arena, "red"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Expected red to be a quoted string.", Some("channel"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_missing_error_invalid_channel() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &is_missing_function(&arena),
            &[
                color_rgb(&arena, 255.0, 0.0, 0.0, 1.0),
                quoted(&arena, "nonexistent"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "Color red doesn't have a channel named \"nonexistent\".",
            Some("channel"),
        );
    }

    // ========================================================================
    // to-gamut
    // ========================================================================

    #[rust_sass_macros::maybe_test]
    async fn test_to_gamut_error_not_color() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_gamut_function(&arena),
            &[
                num(&arena, 42.0),
                sass_string(&arena, "srgb"),
                quoted(&arena, "clip"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "42 is not a color.", Some("color"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_gamut_error_method_null() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_gamut_function(&arena),
            &[
                color_srgb(&arena, 0.5, 0.5, 0.5, 1.0),
                sass_string(&arena, "srgb"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "color.to-gamut() requires a $method argument for forwards-\
             compatibility with changes in the CSS spec. Suggestion:\n\
             \n\
             $method: local-minde",
            Some("method"),
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_gamut_error_method_not_string() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_gamut_function(&arena),
            &[
                color_srgb(&arena, 0.5, 0.5, 0.5, 1.0),
                sass_string(&arena, "srgb"),
                num(&arena, 1.0),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("method"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_gamut_error_method_quoted() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_gamut_function(&arena),
            &[
                color_srgb(&arena, 0.5, 0.5, 0.5, 1.0),
                sass_string(&arena, "srgb"),
                quoted(&arena, "clip"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "Expected \"clip\" to be an unquoted string.",
            Some("method"),
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_gamut_error_space_quoted() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_gamut_function(&arena),
            &[
                color_srgb(&arena, 0.5, 0.5, 0.5, 1.0),
                quoted(&arena, "srgb"),
                quoted(&arena, "clip"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "Expected \"srgb\" to be an unquoted string.",
            Some("space"),
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_gamut_error_space_not_string() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_gamut_function(&arena),
            &[
                color_srgb(&arena, 0.5, 0.5, 0.5, 1.0),
                num(&arena, 1.0),
                quoted(&arena, "clip"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("space"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_gamut_error_unknown_space() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_gamut_function(&arena),
            &[
                color_srgb(&arena, 0.5, 0.5, 0.5, 1.0),
                sass_string(&arena, "not-a-space"),
                quoted(&arena, "clip"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Unknown color space \"not-a-space\".", Some("space"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_gamut_error_unknown_method() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_gamut_function(&arena),
            &[
                color_srgb(&arena, 0.5, 0.5, 0.5, 1.0),
                sass_string(&arena, "srgb"),
                sass_string(&arena, "not-a-method"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Unknown gamut map method \"not-a-method\".", None);
    }

    // ========================================================================
    // to-space
    // ========================================================================

    #[rust_sass_macros::maybe_test]
    async fn test_to_space_error_not_color() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_space_function(&arena),
            &[num(&arena, 42.0), sass_string(&arena, "srgb")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "42 is not a color.", Some("color"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_space_error_space_not_string() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_space_function(&arena),
            &[color_srgb(&arena, 0.5, 0.5, 0.5, 1.0), num(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("space"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_space_error_space_quoted() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_space_function(&arena),
            &[
                color_srgb(&arena, 0.5, 0.5, 0.5, 1.0),
                quoted(&arena, "srgb"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "Expected \"srgb\" to be an unquoted string.",
            Some("space"),
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_space_error_unknown_space() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_space_function(&arena),
            &[
                color_srgb(&arena, 0.5, 0.5, 0.5, 1.0),
                sass_string(&arena, "not-a-space"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Unknown color space \"not-a-space\".", Some("space"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_space_to_lab() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &to_space_function(&arena),
            &[
                color_srgb(&arena, 0.5, 0.0, 0.0, 1.0),
                sass_string(&arena, "lab"),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert_eq!(c.space, ColorSpace::Lab),
            _ => panic!("expected Color"),
        }
    }

    // ========================================================================
    // channel
    // ========================================================================

    #[rust_sass_macros::maybe_test]
    async fn test_channel_error_not_color() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &channel_function_sass(&arena),
            &[num(&arena, 42.0), quoted(&arena, "red")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "42 is not a color.", Some("color"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_channel_error_channel_not_string() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &channel_function_sass(&arena),
            &[color_rgb(&arena, 255.0, 0.0, 0.0, 1.0), num(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("channel"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_channel_error_channel_unquoted() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &channel_function_sass(&arena),
            &[
                color_rgb(&arena, 255.0, 0.0, 0.0, 1.0),
                sass_string(&arena, "red"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Expected red to be a quoted string.", Some("channel"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_channel_red() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &channel_function_sass(&arena),
            &[
                color_rgb(&arena, 128.0, 64.0, 32.0, 1.0),
                quoted(&arena, "red"),
            ],
        )
        .await
        .unwrap();
        assert_num(&v, 128.0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_channel_alpha() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &channel_function_sass(&arena),
            &[
                color_rgb(&arena, 128.0, 64.0, 32.0, 1.0),
                quoted(&arena, "alpha"),
            ],
        )
        .await
        .unwrap();
        assert_num(&v, 1.0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_channel_hue_from_hsl() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &channel_function_sass(&arena),
            &[
                color_hsl(&arena, 90.0, 100.0, 50.0, 1.0),
                quoted(&arena, "hue"),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Number(n) => assert_eq!(n.value, 90.0),
            _ => panic!("expected Number"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_channel_error_not_found() {
        let arena = Bump::new();
        let c = color_hsl(&arena, 90.0, 100.0, 50.0, 1.0);
        let err = eval(
            &arena,
            &channel_function_sass(&arena),
            &[c, quoted(&arena, "red")],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "Color hsl(90, 100%, 50%) has no channel named red.",
            Some("channel"),
        );
    }

    // ========================================================================
    // same
    // ========================================================================

    #[rust_sass_macros::maybe_test]
    async fn test_same_error_first_not_color() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &same_function(&arena),
            &[num(&arena, 42.0), color_rgb(&arena, 255.0, 0.0, 0.0, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "42 is not a color.", Some("color1"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_same_error_second_not_color() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &same_function(&arena),
            &[color_rgb(&arena, 255.0, 0.0, 0.0, 1.0), num(&arena, 42.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "42 is not a color.", Some("color2"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_same_true() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &same_function(&arena),
            &[
                color_rgb(&arena, 255.0, 0.0, 0.0, 1.0),
                color_rgb(&arena, 255.0, 0.0, 0.0, 1.0),
            ],
        )
        .await
        .unwrap();
        assert_is_true(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_same_false() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &same_function(&arena),
            &[
                color_rgb(&arena, 255.0, 0.0, 0.0, 1.0),
                color_rgb(&arena, 0.0, 255.0, 0.0, 1.0),
            ],
        )
        .await
        .unwrap();
        assert_is_false(&v);
    }

    // ========================================================================
    // is-powerless
    // ========================================================================

    #[rust_sass_macros::maybe_test]
    async fn test_is_powerless_error_not_color() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &is_powerless_function(&arena),
            &[num(&arena, 42.0), quoted(&arena, "hue")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "42 is not a color.", Some("color"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_powerless_error_channel_not_string() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &is_powerless_function(&arena),
            &[color_rgb(&arena, 255.0, 0.0, 0.0, 1.0), num(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("channel"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_powerless_error_channel_unquoted() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &is_powerless_function(&arena),
            &[
                color_rgb(&arena, 255.0, 0.0, 0.0, 1.0),
                sass_string(&arena, "hue"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Expected hue to be a quoted string.", Some("channel"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_powerless_hue_zero_saturation() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_powerless_function(&arena),
            &[
                color_hsl(&arena, 120.0, 0.0, 50.0, 1.0),
                quoted(&arena, "hue"),
            ],
        )
        .await
        .unwrap();
        assert_is_true(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_powerless_hue_nonzero_saturation() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_powerless_function(&arena),
            &[
                color_hsl(&arena, 120.0, 100.0, 50.0, 1.0),
                quoted(&arena, "hue"),
            ],
        )
        .await
        .unwrap();
        assert_is_false(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_powerless_saturation_never() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_powerless_function(&arena),
            &[
                color_hsl(&arena, 120.0, 0.0, 50.0, 1.0),
                quoted(&arena, "saturation"),
            ],
        )
        .await
        .unwrap();
        assert_is_false(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_powerless_lch_hue_zero_chroma() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_powerless_function(&arena),
            &[
                color_lch(&arena, 50.0, 0.0, 200.0, 1.0),
                quoted(&arena, "hue"),
            ],
        )
        .await
        .unwrap();
        assert_is_true(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_powerless_alpha_never() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_powerless_function(&arena),
            &[
                color_hsl(&arena, 120.0, 0.0, 50.0, 1.0),
                quoted(&arena, "alpha"),
            ],
        )
        .await
        .unwrap();
        assert_is_false(&v);
    }

    // ========================================================================
    // is-in-gamut
    // ========================================================================

    #[rust_sass_macros::maybe_test]
    async fn test_is_in_gamut_error_not_color() {
        let arena = Bump::new();
        let err = eval(&arena, &is_in_gamut_function(&arena), &[num(&arena, 42.0)])
            .await
            .unwrap_err();
        assert_script_err(err, "42 is not a color.", Some("color"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_in_gamut_srgb_in() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_in_gamut_function(&arena),
            &[color_srgb(&arena, 0.5, 0.5, 0.5, 1.0)],
        )
        .await
        .unwrap();
        assert_is_true(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_in_gamut_xyz_unbounded() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_in_gamut_function(&arena),
            &[color_xyz(&arena, 0.5, 0.5, 0.5, 1.0)],
        )
        .await
        .unwrap();
        assert_is_true(&v);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_in_gamut_srgb_out() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_in_gamut_function(&arena),
            &[color_srgb(&arena, 1.5, 0.5, 0.5, 1.0)],
        )
        .await
        .unwrap();
        assert_is_false(&v);
    }

    // ========================================================================
    // color (global function)
    // ========================================================================

    #[rust_sass_macros::maybe_test]
    async fn test_global_color_red() {
        let arena = Bump::new();
        let list = SassList::new(
            vec![
                Value::new_with_arena(
                    &arena,
                    ValueKind::String(SassString::new(arena.alloc_str("srgb"), false)),
                ),
                ValueKind::unitless_number(&arena, 1.0),
                ValueKind::unitless_number(&arena, 0.0),
                ValueKind::unitless_number(&arena, 0.0),
            ],
            ListSeparator::Space,
            false,
        );
        let v = eval(
            &arena,
            &color_function_gl(&arena),
            &[Value::new_with_arena(&arena, ValueKind::List(list))],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Color(c) => assert_eq!(c.space, ColorSpace::Srgb),
            _ => panic!("expected Color"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_color_error_invalid() {
        let arena = Bump::new();
        let list = SassList::new(
            vec![Value::new_with_arena(
                &arena,
                ValueKind::String(SassString::new(arena.alloc_str("not-a-space"), false)),
            )],
            ListSeparator::Space,
            false,
        );
        let err = eval(
            &arena,
            &color_function_gl(&arena),
            &[Value::new_with_arena(&arena, ValueKind::List(list))],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "Unknown color space \"not-a-space\".",
            Some("description"),
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_color_error_quoted() {
        let arena = Bump::new();
        let err = eval(&arena, &color_function_gl(&arena), &[quoted(&arena, "red")])
            .await
            .unwrap_err();
        assert_script_err(
            err,
            "Expected \"red\" to be an unquoted string.",
            Some("description"),
        );
    }

    // ========================================================================
    // Channel functions (module: red, green, blue, hue, saturation, lightness)
    // ========================================================================

    #[rust_sass_macros::maybe_test]
    async fn test_module_channel_red() {
        let arena = Bump::new();
        let fn_ = channel_function("red", ColorSpace::Rgb, SassColor::red, &arena);
        let v = eval(&arena, &fn_, &[color_rgb(&arena, 255.0, 128.0, 64.0, 1.0)])
            .await
            .unwrap();
        assert_num(&v, 255.0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_module_channel_green() {
        let arena = Bump::new();
        let fn_ = channel_function("green", ColorSpace::Rgb, SassColor::green, &arena);
        let v = eval(&arena, &fn_, &[color_rgb(&arena, 255.0, 128.0, 64.0, 1.0)])
            .await
            .unwrap();
        assert_num(&v, 128.0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_module_channel_blue() {
        let arena = Bump::new();
        let fn_ = channel_function("blue", ColorSpace::Rgb, SassColor::blue, &arena);
        let v = eval(&arena, &fn_, &[color_rgb(&arena, 255.0, 128.0, 64.0, 1.0)])
            .await
            .unwrap();
        assert_num(&v, 64.0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_module_channel_hue() {
        let arena = Bump::new();
        let fn_ = channel_function_with_unit("hue", ColorSpace::Hsl, SassColor::hue, "deg", &arena);
        let v = eval(&arena, &fn_, &[color_hsl(&arena, 120.0, 50.0, 50.0, 1.0)])
            .await
            .unwrap();
        assert_num(&v, 120.0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_module_channel_error_not_color() {
        let arena = Bump::new();
        let fn_ = channel_function("red", ColorSpace::Rgb, SassColor::red, &arena);
        let err = eval(&arena, &fn_, &[num(&arena, 42.0)]).await.unwrap_err();
        assert_script_err(err, "42 is not a color.", Some("color"));
    }

    // ========================================================================
    // Removed functions
    // ========================================================================

    #[rust_sass_macros::maybe_test]
    async fn test_removed_adjust_hue() {
        let arena = Bump::new();
        let fn_ = removed_color_function("adjust-hue", "hue", &arena);
        let err = eval(&arena, &fn_, &[num(&arena, 42.0), num(&arena, 10.0)])
            .await
            .unwrap_err();
        assert_script_err(
            err,
            "The function adjust-hue() isn't in the sass:color module.\n\n\
             Recommendation: color.adjust(42, $hue: 10)\n\n\
             More info: https://sass-lang.com/documentation/functions/color#adjust-hue",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_removed_darken() {
        let arena = Bump::new();
        let fn_ = removed_color_function_negative("darken", "lightness", &arena);
        let err = eval(&arena, &fn_, &[num(&arena, 42.0), num(&arena, 10.0)])
            .await
            .unwrap_err();
        assert_script_err(
            err,
            "The function darken() isn't in the sass:color module.\n\n\
             Recommendation: color.adjust(42, $lightness: -10)\n\n\
             More info: https://sass-lang.com/documentation/functions/color#darken",
            None,
        );
    }
}
