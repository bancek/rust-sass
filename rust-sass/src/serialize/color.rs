// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

//! Legacy + modern color serialization (Dart's `visitColor` family).
//!
//! Decision tree over [`ColorSpace`]: complete legacy colors take the
//! shortest name/hex/function spelling; out-of-gamut Lab/Lch/Oklab/Oklch
//! fall back to `color-mix()` (or `from red/black` relative syntax when
//! channels are missing); other modern spaces use `color(space ...)`.

// dart-source: lib/src/visitor/serialize.dart (visitColor, _writeChannel, _writeLegacyColor, _tryHexOrNamedRgb, _tryIntegerRgbChannels, _canUseHex, _writeRgb, _writeHsl, _writeHwb, _writeColorFunction, _maybeWriteSlashAlpha, _capture, _asInt)
// go-source: go/value/visitor_color.go

use crate::serialize::value::visit_number_impl;
use std::fmt::Write;

use crate::common::SassResult;
use crate::serialize::{OutputStyle, SerializeState};
use crate::source_map_buffer::SourceMapBuffer;
use crate::util::character;
use crate::util::number;
use crate::value::color::{ColorFormat, ColorSpace, SassColor};
use crate::value::color_names;
use crate::value::SassNumber;

// ─── PUBLIC ENTRY POINT ───

/// Writes `color` per the decision tree in the module docs (Dart's `visitColor`).
pub(crate) fn visit_color_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    color: &SassColor,
) -> SassResult<()> {
    match color.space {
        ColorSpace::Rgb | ColorSpace::Hsl | ColorSpace::Hwb
            if !color.is_channel0_missing()
                && !color.is_channel1_missing()
                && !color.is_channel2_missing()
                && !color.is_alpha_missing() =>
        {
            write_legacy_color(buf, state, color)
        }

        ColorSpace::Rgb => {
            write!(buf, "rgb(").unwrap();
            write_channel(buf, state, color.channel0_or_nil(), "")?;
            buf.write_char(' ')?;
            write_channel(buf, state, color.channel1_or_nil(), "")?;
            buf.write_char(' ')?;
            write_channel(buf, state, color.channel2_or_nil(), "")?;
            maybe_write_slash_alpha(buf, state, color);
            buf.write_char(')')?;
            Ok(())
        }

        ColorSpace::Hsl | ColorSpace::Hwb => {
            write!(buf, "{}", color.space.name()).unwrap();
            buf.write_char('(')?;
            let deg_unit = match state.style {
                OutputStyle::Compressed => "",
                OutputStyle::Expanded | OutputStyle::Nested | OutputStyle::Compact => "deg",
            };
            write_channel(buf, state, color.channel0_or_nil(), deg_unit)?;
            buf.write_char(' ')?;
            write_channel(buf, state, color.channel1_or_nil(), "%")?;
            buf.write_char(' ')?;
            write_channel(buf, state, color.channel2_or_nil(), "%")?;
            maybe_write_slash_alpha(buf, state, color);
            buf.write_char(')')?;
            Ok(())
        }

        ColorSpace::Lab | ColorSpace::Lch
            if !state.inspect
                && !number::fuzzy_in_range(color.channel0, 0.0, 100.0)
                && !color.is_channel1_missing()
                && !color.is_channel2_missing() =>
        {
            write_color_mix(buf, state, color)
        }

        ColorSpace::Oklab | ColorSpace::Oklch
            if !state.inspect
                && !number::fuzzy_in_range(color.channel0, 0.0, 1.0)
                && !color.is_channel1_missing()
                && !color.is_channel2_missing() =>
        {
            write_color_mix(buf, state, color)
        }

        ColorSpace::Lch | ColorSpace::Oklch
            if !state.inspect
                && number::fuzzy_less_than(color.channel1, 0.0)
                && !color.is_channel0_missing()
                && !color.is_channel1_missing() =>
        {
            write_color_mix(buf, state, color)
        }

        ColorSpace::Lab | ColorSpace::Oklab | ColorSpace::Lch | ColorSpace::Oklch => {
            write_lab_lch_color(buf, state, color)
        }

        _ => write_color_function(buf, state, color),
    }
}

// ─── CHANNEL WRITER ───

/// Writes one possibly-missing channel plus unit; non-finite channels go
/// through `calc()` via a synthetic single-unit number (Dart's `_writeChannel`).
fn write_channel(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    channel: Option<f64>,
    unit: &str,
) -> SassResult<()> {
    match channel {
        None => {
            write!(buf, "none").unwrap();
            Ok(())
        }
        Some(v) => {
            if v.is_finite() {
                state.write_number(buf, v);
                if !unit.is_empty() {
                    write!(buf, "{}", unit).unwrap();
                }
                Ok(())
            } else {
                // Matches Dart: visitNumber(SassNumber(channel, unit)) → wraps in calc()
                // Matches Go:  VisitNumber(NewSingleUnitNumber(v, unit)) → wraps in calc()
                let number = SassNumber::new(v, if unit.is_empty() { None } else { Some(unit) });
                visit_number_impl(buf, state, &number)
            }
        }
    }
}

// ─── ALPHA ───

/// Appends ` / alpha` unless opaque (Dart's `_maybeWriteSlashAlpha`).
fn maybe_write_slash_alpha(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    color: &SassColor,
) {
    if number::fuzzy_equals(color.alpha, 1.0) {
        return;
    }
    state.write_optional_space(buf);
    buf.write_char('/').unwrap();
    state.write_optional_space(buf);
    write_channel(buf, state, color.alpha_or_nil(), "").unwrap();
}

// ─── COLOR-MIX ───

// Out-of-gamut Lab-family colors serialize via `color-mix()` — more widely
// supported than relative-color syntax — with the color routed through XYZ
// (which has no gamut limits) so values survive unclamped per spec. `red` vs
// `black` is just the compressed/expanded spelling of the 0% endpoint.
fn write_color_mix(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    color: &SassColor,
) -> SassResult<()> {
    write!(buf, "color-mix(in ").unwrap();
    write!(buf, "{}", color.space.name()).unwrap();
    write!(buf, "{}", state.comma_sep()).unwrap();

    let xyz = color.to_space(ColorSpace::XyzD65, None)?;
    write_color_function(buf, state, &xyz)?;

    state.write_optional_space(buf);
    write!(buf, "100%").unwrap();
    write!(buf, "{}", state.comma_sep()).unwrap();

    match state.style {
        OutputStyle::Compressed => write!(buf, "red").unwrap(),
        OutputStyle::Expanded | OutputStyle::Nested | OutputStyle::Compact => {
            write!(buf, "black").unwrap()
        }
    }
    buf.write_char(')')?;
    Ok(())
}

// ─── LAB/LCH/OKLAB/OKLCH ───

// `color-mix()` cannot carry missing channels, so those fall through here to
// the more expressive `from red/black` relative syntax, which never clamps.
fn write_lab_lch_color(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    color: &SassColor,
) -> SassResult<()> {
    write!(buf, "{}", color.space.name()).unwrap();
    buf.write_char('(')?;

    let chs = color.space.channels_linear();
    let polar = chs[2].channel.is_polar_angle;

    if !state.inspect
        && (!number::fuzzy_in_range(color.channel0, 0.0, 100.0)
            || (polar && number::fuzzy_less_than(color.channel1, 0.0)))
    {
        write!(buf, "from ").unwrap();
        match state.style {
            OutputStyle::Compressed => write!(buf, "red").unwrap(),
            OutputStyle::Expanded | OutputStyle::Nested | OutputStyle::Compact => {
                write!(buf, "black").unwrap()
            }
        }
        buf.write_char(' ')?;
    }

    if !matches!(state.style, OutputStyle::Compressed) && !color.is_channel0_missing() {
        let max = chs[0].max;
        state.write_number(buf, color.channel0 * 100.0 / max);
        buf.write_char('%')?;
    } else {
        write_channel(buf, state, color.channel0_or_nil(), "")?;
    }
    buf.write_char(' ')?;
    write_channel(buf, state, color.channel1_or_nil(), "")?;
    buf.write_char(' ')?;

    let ch2_unit = if polar && !matches!(state.style, OutputStyle::Compressed) {
        "deg"
    } else {
        ""
    };
    write_channel(buf, state, color.channel2_or_nil(), ch2_unit)?;

    maybe_write_slash_alpha(buf, state, color);
    buf.write_char(')')?;
    Ok(())
}

// ─── COLOR() FUNCTION ───

/// Writes `color(space c0 c1 c2 / alpha)` for modern non-Lab spaces
/// (Dart's `_writeColorFunction`).
fn write_color_function(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    color: &SassColor,
) -> SassResult<()> {
    write!(buf, "color({}", color.space.name()).unwrap();

    let channels: [Option<f64>; 3] = [
        color.channel0_or_nil(),
        color.channel1_or_nil(),
        color.channel2_or_nil(),
    ];
    for ch_opt in &channels {
        buf.write_char(' ')?;
        write_channel(buf, state, *ch_opt, "")?;
    }

    maybe_write_slash_alpha(buf, state, color);
    buf.write_char(')')?;
    Ok(())
}

// ─── LEGACY COLOR (THE BIG ONE) ───

// Unlike newer spaces, the three legacy spaces are interchangeable, so this
// picks the shortest spelling all supported browsers accept. Out-of-gamut
// colors can only round-trip through HSL (never clamped at parse time);
// generated transparent colors stay in `rgba()` for an old IE bug
// (sass/sass#1782); unhexable HWB degrades to HSL to preserve author intent.
fn write_legacy_color(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    color: &SassColor,
) -> SassResult<()> {
    let opaque = number::fuzzy_equals(color.alpha, 1.0);

    if !color.is_in_gamut() && !state.inspect {
        return write_hsl(buf, state, color);
    }

    if matches!(state.style, OutputStyle::Compressed) {
        let rgb = color.to_space(ColorSpace::Rgb, None)?;
        if opaque && try_hex_or_named_rgb(buf, state, &rgb)? {
            return Ok(());
        }

        // Render both candidates fully and emit the shorter spelling (plus
        // two characters of HSL handicap for its `%` signs), exactly like
        // Dart `_capture(() => _writeRgb/_writeHsl)`.
        let rgb_string = capture(|buf| write_rgb(buf, state, &rgb))?;
        let hsl = rgb.to_space(ColorSpace::Hsl, None)?;
        let hsl_string = capture(|buf| write_hsl(buf, state, &hsl))?;

        if rgb_string.len() <= hsl_string.len() + 2 {
            write!(buf, "{}", rgb_string).unwrap();
        } else {
            write!(buf, "{}", hsl_string).unwrap();
        }
        return Ok(());
    }

    if color.space == ColorSpace::Hsl {
        return write_hsl(buf, state, color);
    } else if state.inspect && color.space == ColorSpace::Hwb {
        return write_hwb(buf, state, color);
    }

    match &color.format {
        Some(ColorFormat::RgbFunction) => {
            return write_rgb(buf, state, color);
        }
        Some(ColorFormat::Preserved(original)) => {
            write!(buf, "{}", original).unwrap();
            return Ok(());
        }
        None => {}
    }

    if opaque {
        let rgb = color.to_space(ColorSpace::Rgb, None)?;
        let name = color_names::color_name_for_sass_color(&rgb);
        if !name.is_empty() {
            write!(buf, "{}", name).unwrap();
            return Ok(());
        }
        if can_use_hex(&rgb) {
            buf.write_char('#')?;
            write_hex_component(buf, rgb.channel0.round() as i32)?;
            write_hex_component(buf, rgb.channel1.round() as i32)?;
            write_hex_component(buf, rgb.channel2.round() as i32)?;
            return Ok(());
        }
    }

    if color.space == ColorSpace::Hwb {
        write_hsl(buf, state, color)
    } else {
        write_rgb(buf, state, color)
    }
}

// ─── COMPRESSED HEX/NAME SHORTCUT ───

/// Runs `emit` into a throwaway plain buffer and returns the text it would
/// have emitted (Dart's `_capture`; source-map segments are dropped, as in
/// Dart's `NoSourceMapBuffer` swap).
fn capture(emit: impl FnOnce(&mut SourceMapBuffer<'_>) -> SassResult<()>) -> SassResult<String> {
    let mut tmp = SourceMapBuffer::new_plain();
    emit(&mut tmp)?;
    Ok(tmp.into_string())
}

/// Writes `rgb` as a name or hex when it fits, shortest first, reporting
/// whether anything was written (Dart's `_tryHexOrNamedRgb`; caller must pass
/// an RGB-space color).
fn try_hex_or_named_rgb(
    buf: &mut SourceMapBuffer<'_>,
    _state: &SerializeState,
    rgb: &SassColor,
) -> SassResult<bool> {
    if !can_use_hex(rgb) {
        return Ok(false);
    }

    let red_int = rgb.channel0.round() as i32;
    let green_int = rgb.channel1.round() as i32;
    let blue_int = rgb.channel2.round() as i32;

    let short_hex = is_symmetrical_hex(red_int)
        && is_symmetrical_hex(green_int)
        && is_symmetrical_hex(blue_int);

    let max_len = if short_hex { 4 } else { 7 };

    let name = color_names::color_name_for_sass_color(rgb);
    if !name.is_empty() && name.len() <= max_len {
        write!(buf, "{}", name).unwrap();
    } else if short_hex {
        buf.write_char('#')?;
        buf.write_char(character::hex_char_for(red_int & 0xF))?;
        buf.write_char(character::hex_char_for(green_int & 0xF))?;
        buf.write_char(character::hex_char_for(blue_int & 0xF))?;
    } else {
        buf.write_char('#')?;
        write_hex_component(buf, red_int)?;
        write_hex_component(buf, green_int)?;
        write_hex_component(buf, blue_int)?;
    }
    Ok(true)
}

// ─── HEX UTILS ───

/// Whether `rgb` (in RGB space) fits a hex spelling: every channel a fuzzy
/// integer in `[0, 256)` (Dart's `_canUseHex`).
fn can_use_hex(rgb: &SassColor) -> bool {
    can_use_hex_for_channel(rgb.channel0)
        && can_use_hex_for_channel(rgb.channel1)
        && can_use_hex_for_channel(rgb.channel2)
}

/// Whether one channel fits a two-digit hex value (Dart's `_canUseHexForChannel`).
fn can_use_hex_for_channel(channel: f64) -> bool {
    number::fuzzy_is_int(channel)
        && number::fuzzy_greater_than_or_equals(channel, 0.0)
        && number::fuzzy_less_than(channel, 256.0)
}

/// Whether a channel byte is symmetric (`0xAA`) and fits one hex digit
/// (Dart's `_isSymmetricalHex` / `_canUseShortHex`).
fn is_symmetrical_hex(color: i32) -> bool {
    (color & 0xF) == (color >> 4)
}

/// Emits one channel byte as two hex digits (Dart's `_writeHexComponent`).
fn write_hex_component(buf: &mut SourceMapBuffer<'_>, color: i32) -> SassResult<()> {
    buf.write_char(character::hex_char_for(color >> 4))?;
    buf.write_char(character::hex_char_for(color & 0xF))?;
    Ok(())
}

// ─── LEGACY FUNCTION WRITERS ───

/// Writes `rgb()/rgba()` via the RGB space (Dart's `_writeRgb`).
///
/// Integer channels take the plain spelling; otherwise every channel is
/// emitted as a percentage (older browsers only accept integers or
/// percentages in legacy `rgb()`), via the *original* color's channels
/// exactly like Dart (`color.channelN * 100 / 255`).
fn write_rgb(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    color: &SassColor,
) -> SassResult<()> {
    let opaque = number::fuzzy_equals(color.alpha, 1.0);
    let rgb = color.to_space(ColorSpace::Rgb, None)?;

    if opaque {
        write!(buf, "rgb(").unwrap();
    } else {
        write!(buf, "rgba(").unwrap();
    }

    if !try_integer_rgb_channels(buf, state, &rgb)? {
        write_channel(buf, state, Some(color.channel0 * 100.0 / 255.0), "%")?;
        write!(buf, "{}", state.comma_sep()).unwrap();
        write_channel(buf, state, Some(color.channel1 * 100.0 / 255.0), "%")?;
        write!(buf, "{}", state.comma_sep()).unwrap();
        write_channel(buf, state, Some(color.channel2 * 100.0 / 255.0), "%")?;
    }

    if !opaque {
        write!(buf, "{}", state.comma_sep()).unwrap();
        state.write_number(buf, color.alpha);
    }

    buf.write_char(')')?;
    Ok(())
}

/// If `rgb`'s channels are all integers, writes them plainly and returns
/// `true`; otherwise writes nothing and returns `false` (Dart's
/// `_tryIntegerRgbChannels`; caller must pass an RGB-space color).
fn try_integer_rgb_channels(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    rgb: &SassColor,
) -> SassResult<bool> {
    let channels = [
        number::as_int_for_serialize(rgb.channel0, state.inspect),
        number::as_int_for_serialize(rgb.channel1, state.inspect),
        number::as_int_for_serialize(rgb.channel2, state.inspect),
    ];
    let mut ints = [0i64; 3];
    for (i, ch) in channels.iter().enumerate() {
        match ch {
            Some(v) => ints[i] = *v,
            None => return Ok(false),
        }
    }

    // i64 `Display` never uses exponent notation (Dart `_removeExponent`).
    write!(buf, "{}", ints[0]).unwrap();
    write!(buf, "{}", state.comma_sep()).unwrap();
    write!(buf, "{}", ints[1]).unwrap();
    write!(buf, "{}", state.comma_sep()).unwrap();
    write!(buf, "{}", ints[2]).unwrap();
    Ok(true)
}

/// Writes `hsl()/hsla()` via the HSL space (Dart's `_writeHsl`).
fn write_hsl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    color: &SassColor,
) -> SassResult<()> {
    let opaque = number::fuzzy_equals(color.alpha, 1.0);
    let hsl = color.to_space(ColorSpace::Hsl, None)?;

    if opaque {
        write!(buf, "hsl(").unwrap();
    } else {
        write!(buf, "hsla(").unwrap();
    }

    write_channel(buf, state, Some(hsl.channel_by_name("hue")?), "")?;
    write!(buf, "{}", state.comma_sep()).unwrap();
    write_channel(buf, state, Some(hsl.channel_by_name("saturation")?), "%")?;
    write!(buf, "{}", state.comma_sep()).unwrap();
    write_channel(buf, state, Some(hsl.channel_by_name("lightness")?), "%")?;

    if !opaque {
        write!(buf, "{}", state.comma_sep()).unwrap();
        state.write_number(buf, color.alpha);
    }

    buf.write_char(')')?;
    Ok(())
}

/// Writes modern-space `hwb()`; inspect-only, so always the new space
/// syntax (Dart's `_writeHwb`).
fn write_hwb(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    color: &SassColor,
) -> SassResult<()> {
    let hwb = color.to_space(ColorSpace::Hwb, None)?;

    write!(buf, "hwb(").unwrap();
    state.write_number(buf, hwb.channel_by_name("hue")?);
    buf.write_char(' ')?;
    state.write_number(buf, hwb.channel_by_name("whiteness")?);
    buf.write_char('%')?;
    buf.write_char(' ')?;
    state.write_number(buf, hwb.channel_by_name("blackness")?);
    buf.write_char('%')?;

    if !number::fuzzy_equals(color.alpha, 1.0) {
        write!(buf, " / ").unwrap();
        state.write_number(buf, color.alpha);
    }

    buf.write_char(')')?;
    Ok(())
}

// ─── TESTS ───

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::LINE_FEED_LF;
    use crate::serialize::{OutputStyle, SerializeState};
    use crate::source_map_buffer::SourceMapBuffer;
    use crate::value::color::{ColorFormat, ColorSpace, SassColor};

    fn serialize_color(color: &SassColor, inspect: bool, compressed: bool) -> String {
        let state = SerializeState {
            indentation: 0,
            style: if compressed {
                OutputStyle::Compressed
            } else {
                OutputStyle::Expanded
            },
            source_comments: false,
            inspect,
            quote: true,
            line_feed: LINE_FEED_LF,
            indent_char: ' ',
            indent_width: 2,
        };
        let mut buf = SourceMapBuffer::new_plain();
        visit_color_impl(&mut buf, &state, color).unwrap();
        buf.into_string()
    }

    fn cs(color: &SassColor) -> String {
        serialize_color(color, false, false)
    }
    fn cs_inspect(color: &SassColor) -> String {
        serialize_color(color, true, false)
    }
    fn cs_compressed(color: &SassColor) -> String {
        serialize_color(color, false, true)
    }

    #[test]
    fn test_legacy_rgb_named() {
        assert_eq!(cs(&SassColor::rgb(255.0, 0.0, 0.0, 1.0)), "red");
        assert_eq!(cs(&SassColor::rgb(0.0, 255.0, 0.0, 1.0)), "lime");
        assert_eq!(cs(&SassColor::rgb(0.0, 0.0, 255.0, 1.0)), "blue");
        assert_eq!(cs(&SassColor::rgb(0.0, 0.0, 0.0, 1.0)), "black");
        assert_eq!(cs(&SassColor::rgb(255.0, 255.0, 255.0, 1.0)), "white");
    }

    #[test]
    fn test_legacy_rgba() {
        let c = SassColor::rgb(255.0, 0.0, 0.0, 0.5);
        assert_eq!(cs(&c), "rgba(255, 0, 0, 0.5)");
    }

    #[test]
    fn test_legacy_rgb_hex() {
        assert_eq!(cs(&SassColor::rgb(18.0, 52.0, 86.0, 1.0)), "#123456");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compressed_tiebreak_uses_compressed_spellings() {
        // Non-integer legacy channels serialize as percentages (#2800), so
        // the compressed rgb-vs-hsl tiebreak compares full captured
        // function strings (Dart `_capture`). rgb(0.1, 0.1, 0.1) can no
        // longer win as `rgb(.1,.1,.1)`; HSL wins instead — byte-identical
        // to Dart.
        let c = SassColor::rgb(0.1, 0.1, 0.1, 1.0);
        assert_eq!(cs_compressed(&c), "hsl(0,0%,.0392156863%)");
    }

    #[test]
    fn test_fractional_channels_use_percents() {
        // Legacy `rgb()` with a non-integer channel uses percentages for
        // all channels (#2800) — older browsers only accept integers or
        // percentages here.
        let c = SassColor::rgb(0.0, 255.0, 127.5, 1.0);
        assert_eq!(cs(&c), "rgb(0%, 100%, 50%)");
    }

    #[test]
    fn test_fractional_channels_use_percents_rgba() {
        let c = SassColor::rgb(0.0, 255.0, 127.5, 0.5);
        assert_eq!(cs(&c), "rgba(0%, 100%, 50%, 0.5)");
    }

    #[test]
    fn test_legacy_hsl() {
        let c = SassColor::hsl(120.0, 50.0, 50.0, 1.0).unwrap();
        assert_eq!(cs(&c), "hsl(120, 50%, 50%)");
    }

    #[test]
    fn test_legacy_hsla() {
        let c = SassColor::hsl(120.0, 50.0, 50.0, 0.5).unwrap();
        assert_eq!(cs(&c), "hsla(120, 50%, 50%, 0.5)");
    }

    #[test]
    fn test_legacy_hwb_inspect() {
        let c = SassColor::hwb(0.0, 0.0, 0.0, 1.0).unwrap();
        let result = cs_inspect(&c);
        assert!(result.starts_with("hwb("));
    }

    #[test]
    fn test_preserved_format() {
        let c = SassColor::rgb_internal(
            Some(255.0),
            Some(0.0),
            Some(0.0),
            Some(1.0),
            Some(ColorFormat::Preserved("#f00".into())),
        )
        .unwrap();
        assert_eq!(cs(&c), "#f00");
    }

    #[test]
    fn test_rgb_function_format() {
        let c = SassColor::rgb_internal(
            Some(255.0),
            Some(0.0),
            Some(0.0),
            Some(1.0),
            Some(ColorFormat::RgbFunction),
        )
        .unwrap();
        assert_eq!(cs(&c), "rgb(255, 0, 0)");
    }

    #[test]
    fn test_compressed_named_vs_hex() {
        assert_eq!(cs_compressed(&SassColor::rgb(255.0, 0.0, 0.0, 1.0)), "red");
    }

    #[test]
    fn test_compressed_hex() {
        assert_eq!(
            cs_compressed(&SassColor::rgb(18.0, 52.0, 86.0, 1.0)),
            "#123456"
        );
    }

    #[test]
    fn test_compressed_short_hex() {
        let c = SassColor::rgb(0xAA as f64, 0xBB as f64, 0xCC as f64, 1.0);
        assert_eq!(cs_compressed(&c), "#abc");
    }

    #[test]
    fn test_rgb_missing() {
        let c = SassColor::for_space(
            ColorSpace::Rgb,
            [255.0, 0.0, 0.0],
            1.0,
            [true, false, false, false],
        );
        assert_eq!(cs(&c), "rgb(none 0 0)");
    }

    #[test]
    fn test_hsl_missing() {
        let c = SassColor::for_space(
            ColorSpace::Hsl,
            [120.0, 50.0, 50.0],
            1.0,
            [false, true, false, false],
        );
        assert_eq!(cs(&c), "hsl(120deg none 50%)");
    }

    #[test]
    fn test_hwb_missing() {
        let c = SassColor::for_space(
            ColorSpace::Hwb,
            [0.0, 50.0, 0.0],
            1.0,
            [false, true, false, false],
        );
        assert_eq!(cs(&c), "hwb(0deg none 0%)");
    }

    #[test]
    fn test_lab() {
        let c = SassColor::for_space(ColorSpace::Lab, [50.0, 25.0, -25.0], 1.0, [false; 4]);
        assert_eq!(cs(&c), "lab(50% 25 -25)");
    }

    #[test]
    fn test_lch() {
        let c = SassColor::for_space(ColorSpace::Lch, [50.0, 30.0, 200.0], 1.0, [false; 4]);
        assert_eq!(cs(&c), "lch(50% 30 200deg)");
    }

    #[test]
    fn test_oklab() {
        let c = SassColor::for_space(ColorSpace::Oklab, [0.5, 0.1, -0.1], 1.0, [false; 4]);
        assert_eq!(cs(&c), "oklab(50% 0.1 -0.1)");
    }

    #[test]
    fn test_oklch() {
        let c = SassColor::for_space(ColorSpace::Oklch, [0.5, 0.15, 180.0], 1.0, [false; 4]);
        assert_eq!(cs(&c), "oklch(50% 0.15 180deg)");
    }

    #[test]
    fn test_srgb() {
        let c = SassColor::for_space(ColorSpace::Srgb, [0.5, 0.5, 0.5], 1.0, [false; 4]);
        assert_eq!(cs(&c), "color(srgb 0.5 0.5 0.5)");
    }

    #[test]
    fn test_display_p3() {
        let c = SassColor::for_space(ColorSpace::DisplayP3, [0.5, 0.5, 0.5], 1.0, [false; 4]);
        assert_eq!(cs(&c), "color(display-p3 0.5 0.5 0.5)");
    }

    #[test]
    fn test_xyz() {
        let c = SassColor::for_space(ColorSpace::XyzD65, [0.5, 0.5, 0.5], 1.0, [false; 4]);
        assert_eq!(cs(&c), "color(xyz 0.5 0.5 0.5)");
    }

    #[test]
    fn test_alpha_slash() {
        let c = SassColor::for_space(ColorSpace::Srgb, [0.5, 0.5, 0.5], 0.5, [false; 4]);
        let result = cs(&c);
        assert!(result.contains(" / 0.5"));
    }

    #[test]
    fn test_alpha_opaque_no_slash() {
        let c = SassColor::for_space(ColorSpace::Srgb, [0.5, 0.5, 0.5], 1.0, [false; 4]);
        let result = cs(&c);
        assert!(!result.contains(" / "));
    }

    #[test]
    fn test_alpha_none() {
        let c = SassColor::for_space(
            ColorSpace::Srgb,
            [0.5, 0.5, 0.5],
            0.5,
            [false, false, false, true],
        );
        let result = cs(&c);
        assert!(result.contains("none"));
    }

    #[test]
    fn test_lab_oog_color_mix() {
        let c = SassColor::for_space(ColorSpace::Lab, [150.0, 10.0, -10.0], 1.0, [false; 4]);
        assert!(cs(&c).starts_with("color-mix(in lab"));
    }

    #[test]
    fn test_lch_negative_chroma_color_mix() {
        let c = SassColor::for_space(ColorSpace::Lch, [50.0, -10.0, 180.0], 1.0, [false; 4]);
        assert!(cs(&c).starts_with("color-mix(in lch"));
    }

    #[test]
    fn test_oklab_oog_color_mix() {
        let c = SassColor::for_space(ColorSpace::Oklab, [1.5, 0.1, -0.1], 1.0, [false; 4]);
        assert!(cs(&c).starts_with("color-mix(in oklab"));
    }

    #[test]
    fn test_compressed_lab_no_percent() {
        let c = SassColor::for_space(ColorSpace::Lab, [50.0, 10.0, -10.0], 1.0, [false; 4]);
        assert_eq!(cs_compressed(&c), "lab(50 10 -10)");
    }

    #[test]
    fn test_compressed_oklch_no_deg() {
        let c = SassColor::for_space(ColorSpace::Oklch, [0.5, 0.15, 180.0], 1.0, [false; 4]);
        assert_eq!(cs_compressed(&c), "oklch(.5 .15 180)");
    }
}
