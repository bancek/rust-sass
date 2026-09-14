// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/utils.dart (folded: space/a98_rgb.dart, space/prophoto_rgb.dart, space/rec2020.dart, space/lms.dart transfer curves)
// go-source: go/value/color_utils.go

use crate::common::exception::SassResult;
use crate::math;
use crate::util::number;
use crate::value::color::ColorSpace;
use crate::value::color_channel_types::ColorChannel;
use std::f64::consts::PI;

use crate::value::color::SassColor;
use crate::value::color_channel_types::LinearChannel;

/// Channels shared across both XYZ color spaces (folded from the per-space
/// `xyzChannels` constant in Dart's `space/utils.dart`).
pub(crate) const XYZ_CHANNELS: [LinearChannel; 3] = [
    LinearChannel {
        channel: ColorChannel {
            name: "x",
            is_polar_angle: false,
            associated_unit: "",
        },
        min: 0.0,
        max: 1.0,
        requires_percent: false,
        lower_clamped: false,
        upper_clamped: false,
    },
    LinearChannel {
        channel: ColorChannel {
            name: "y",
            is_polar_angle: false,
            associated_unit: "",
        },
        min: 0.0,
        max: 1.0,
        requires_percent: false,
        lower_clamped: false,
        upper_clamped: false,
    },
    LinearChannel {
        channel: ColorChannel {
            name: "z",
            is_polar_angle: false,
            associated_unit: "",
        },
        min: 0.0,
        max: 1.0,
        requires_percent: false,
        lower_clamped: false,
        upper_clamped: false,
    },
];

/// Lab conversion constants (`29^3/3^3` and `6^3/29^3`, folded from
/// `space/utils.dart`).
pub(crate) const LAB_KAPPA: f64 = 24389.0 / 27.0;
/// Lab conversion constant paired with [`LAB_KAPPA`].
pub(crate) const LAB_EPSILON: f64 = 216.0 / 24389.0;

// The shared `hueChannel`/`rgbChannels` descriptors from Dart's
// `space/utils.dart` live on [`ColorSpace::channels_linear`](super::color::ColorSpace::channels_linear)
// instead (see `color.rs`); there is no Rust counterpart to import them from
// here — omitted per the split rule.

/// Converts a legacy HSL/HWB hue to an RGB channel.
///
/// Algorithm from the CSS3 spec: http://www.w3.org/TR/css3-color/#hsl-color.
pub(crate) fn hue_to_rgb(m1: f64, m2: f64, hue: f64) -> f64 {
    let mut hue = hue;
    if hue < 0.0 {
        hue += 1.0;
    }
    if hue > 1.0 {
        hue -= 1.0;
    }

    // Matches Dart (and the CSS spec: `hue*6 < 3`): strict `<` at every
    // boundary. A `<=` at `1/2` would return `m2` instead of the
    // arithmetically-equal-but-bit-distinct `m1 + ...` form, breaking
    // byte-identity (e.g. saturate(plum) blue: 255.0 vs 254.99999999999997 —
    // exposed by exact-integer serialization, #2800).
    if hue < 1.0 / 6.0 {
        m1 + (((m2 - m1) * hue) * 6.0)
    } else if hue < 1.0 / 2.0 {
        m2
    } else if hue < 2.0 / 3.0 {
        m1 + (((m2 - m1) * (2.0 / 3.0 - hue)) * 6.0)
    } else {
        m1
    }
}

/// Converts a single sRGB or Display-P3 channel to linear-light form.
///
/// Algorithm from https://www.w3.org/TR/css-color-4/#color-conversion-code
pub(crate) fn srgb_and_display_p3_to_linear(channel: f64) -> f64 {
    let abs = channel.abs();
    if abs <= 0.04045 {
        channel / 12.92
    } else {
        channel.signum() * math::pow((abs + 0.055) / 1.055, 2.4)
    }
}

/// Converts a single linear-light channel to sRGB or Display-P3 gamma form.
///
/// Algorithm from https://www.w3.org/TR/css-color-4/#color-conversion-code
pub(crate) fn srgb_and_display_p3_from_linear(channel: f64) -> f64 {
    let abs = channel.abs();
    if abs <= 0.0031308 {
        channel * 12.92
    } else {
        channel.signum() * ((1.055 * math::pow(abs, 1.0 / 2.4)) - 0.055)
    }
}

/// Converts a single A98-RGB channel to linear-light form.
///
/// Folded from `A98RgbColorSpace.toLinear` (`space/a98_rgb.dart`); algorithm
/// from <https://www.w3.org/TR/css-color-4/#color-conversion-code>.
pub(crate) fn a98_to_linear(channel: f64) -> f64 {
    channel.signum() * math::pow(channel.abs(), 563.0 / 256.0)
}

/// Converts a single linear-light channel to A98-RGB form.
///
/// Folded from `A98RgbColorSpace.fromLinear` (`space/a98_rgb.dart`).
pub(crate) fn a98_from_linear(channel: f64) -> f64 {
    channel.signum() * math::pow(channel.abs(), 256.0 / 563.0)
}

/// Converts a single ProPhoto RGB channel to linear-light form.
///
/// Folded from `ProphotoRgbColorSpace.toLinear` (`space/prophoto_rgb.dart`);
/// algorithm from <https://www.w3.org/TR/css-color-4/#color-conversion-code>.
pub(crate) fn prophoto_to_linear(channel: f64) -> f64 {
    let abs = channel.abs();
    if abs <= 16.0 / 512.0 {
        channel / 16.0
    } else {
        channel.signum() * math::pow(abs, 1.8)
    }
}

/// Converts a single linear-light channel to ProPhoto RGB form.
///
/// Folded from `ProphotoRgbColorSpace.fromLinear`
/// (`space/prophoto_rgb.dart`).
pub(crate) fn prophoto_from_linear(channel: f64) -> f64 {
    let abs = channel.abs();
    if abs >= 1.0 / 512.0 {
        channel.signum() * math::pow(abs, 1.0 / 1.8)
    } else {
        16.0 * channel
    }
}

/// Converts a single Rec2020 channel to linear-light form.
///
/// Folded from `Rec2020ColorSpace.toLinear` (`space/rec2020.dart`); algorithm
/// from <https://www.w3.org/TR/css-color-4/#color-conversion-code>. Since
/// dart-sass 1.102 (#2729) this is the plain 2.4-gamma power law (the old
/// alpha/beta piecewise form is gone).
pub(crate) fn rec2020_to_linear(channel: f64) -> f64 {
    let abs = channel.abs();
    channel.signum() * math::pow(abs, 2.40)
}

/// Converts a single linear-light channel to Rec2020 form.
///
/// Folded from `Rec2020ColorSpace.fromLinear` (`space/rec2020.dart`).
pub(crate) fn rec2020_from_linear(channel: f64) -> f64 {
    let abs = channel.abs();
    channel.signum() * math::pow(abs, 1.0 / 2.40)
}

/// Returns the cube root of the absolute value of `v` with the same sign as
/// `v` (folded from `LmsColorSpace._cubeRootPreservingSign` in
/// `space/lms.dart`).
pub(crate) fn cube_root_preserving_sign(v: f64) -> f64 {
    v.signum() * math::pow(v.abs(), 1.0 / 3.0)
}

/// Converts a Lab or OKLab color to LCH or OKLCH, respectively.
///
/// Algorithm from <https://www.w3.org/TR/css-color-4/#color-conversion-code>.
/// `missing_chroma`/`missing_hue` record whether the source color was missing
/// those channels; a zero chroma (or missing hue) leaves the hue missing, and
/// negative hues wrap into `[0, 360)`.
///
/// Dart: calls SassColor.forSpaceInternal which validates alpha via
/// fuzzyAssertRange.
pub(crate) fn lab_to_lch(
    dest: ColorSpace,
    lightness: Option<f64>,
    a: Option<f64>,
    b: Option<f64>,
    alpha: Option<f64>,
    missing_chroma: bool,
    missing_hue: bool,
) -> SassResult<SassColor> {
    // Analogous missingness: null `a`+`b` imply missing chroma+hue (#2810).
    let missing_chroma = missing_chroma || (a.is_none() && b.is_none());
    let missing_hue = missing_hue || (a.is_none() && b.is_none());
    let av = a.unwrap_or(0.0);
    let bv = b.unwrap_or(0.0);
    let chroma = ((av * av) + (bv * bv)).sqrt();
    let mut hue = None;
    if !missing_hue && !number::fuzzy_equals(chroma, 0.0) {
        let h = bv.atan2(av) * 180.0 / PI;
        hue = if h >= 0.0 { Some(h) } else { Some(h + 360.0) };
    }

    let ch_ptr = if !missing_chroma { Some(chroma) } else { None };

    SassColor::new_color_for_space_internal(dest, lightness, ch_ptr, hue, alpha)
}

#[cfg(test)]
mod tests {
    use super::super::color::ColorSpace;
    use super::*;
    use crate::util::number;
    use crate::value::color_conversions::convert_color;

    fn ws(v: f64) -> String {
        number::write_number_to_string(v)
    }

    #[test]
    fn test_hue_to_rgb() {
        let m1 = 0.2;
        let m2 = 0.7;
        let tests = [
            (0.0 / 6.0, "0.2"),
            (0.5 / 6.0, "0.45"),
            (1.0 / 6.0, "0.7"),
            (2.0 / 6.0, "0.7"),
            (3.0 / 6.0, "0.7"),
            (3.5 / 6.0, "0.45"),
            (4.0 / 6.0, "0.2"),
            (5.0 / 6.0, "0.2"),
            (1.0, "0.2"),
        ];
        for (hue, want) in tests {
            let got = ws(hue_to_rgb(m1, m2, hue));
            assert_eq!(
                got, want,
                "hue_to_rgb({m1}, {m2}, {hue}) = {got}, want {want}"
            );
        }
    }

    #[test]
    fn test_hue_to_rgb_wraps() {
        let m1 = 0.2;
        let m2 = 0.7;
        assert_eq!(ws(hue_to_rgb(m1, m2, 0.5 / 6.0)), "0.45");
        assert_eq!(ws(hue_to_rgb(m1, m2, -5.5 / 6.0)), "0.45");
        assert_eq!(ws(hue_to_rgb(m1, m2, 1.0 / 6.0)), "0.7");
        assert_eq!(ws(hue_to_rgb(m1, m2, 7.0 / 6.0)), "0.7");
    }

    // The `hue == 1/2` midpoint takes the `< 2/3` branch (Dart/CSS spec),
    // not `m2` directly: the results agree mathematically but differ in the
    // last ULP. Bit-pinned via saturate(plum): its blue channel converts
    // through exactly hue 0.5, and exact-integer serialization (#2800)
    // exposes the dust (255.0 vs 254.99999999999997).
    #[test]
    fn test_hue_to_rgb_midpoint_matches_dart_bits() {
        let m1 = 0.49411764705882355;
        let m2 = 1.0;
        let got = hue_to_rgb(m1, m2, 0.5);
        assert_eq!(got.to_bits(), 0x3fefffffffffffff);
        assert_eq!(got * 255.0, 254.99999999999997);
    }

    #[test]
    fn test_srgb_and_display_p3_roundtrip() {
        for v in [0.0, 0.05, 0.5, 1.0, -0.5] {
            let linear = srgb_and_display_p3_to_linear(v);
            let round = srgb_and_display_p3_from_linear(linear);
            assert_eq!(
                ws(v),
                ws(round),
                "sRGB roundtrip({}) = {}",
                ws(v),
                ws(round)
            );
        }
        for v in [0.0, 0.005, 0.5, 1.0, -0.5] {
            let round = srgb_and_display_p3_to_linear(srgb_and_display_p3_from_linear(v));
            assert_eq!(
                ws(v),
                ws(round),
                "sRGB reverse roundtrip({}) = {}",
                ws(v),
                ws(round)
            );
        }
    }

    #[test]
    fn test_a98_roundtrip() {
        for v in [0.0, 0.5, 1.0, -0.3] {
            let linear = a98_to_linear(v);
            let round = a98_from_linear(linear);
            assert_eq!(ws(v), ws(round), "A98 roundtrip({}) = {}", ws(v), ws(round));
        }
    }

    #[test]
    fn test_prophoto_roundtrip() {
        for v in [0.0, 0.001, 16.0 / 512.0, 0.5, 1.0] {
            let linear = prophoto_to_linear(v);
            let round = prophoto_from_linear(linear);
            assert_eq!(
                ws(v),
                ws(round),
                "ProPhoto roundtrip({}) = {}",
                ws(v),
                ws(round)
            );
        }
    }

    #[test]
    fn test_rec2020_roundtrip() {
        for v in [0.0, 0.001, 0.1, 0.5, 1.0, -0.3] {
            let linear = rec2020_to_linear(v);
            let round = rec2020_from_linear(linear);
            assert_eq!(
                ws(v),
                ws(round),
                "Rec2020 roundtrip({}) = {}",
                ws(v),
                ws(round)
            );
        }
    }

    #[test]
    fn test_cube_root_preserving_sign() {
        let tests = [
            (0.0, "0"),
            (1.0, "1"),
            (8.0, "2"),
            (-8.0, "-2"),
            (27.0, "3"),
            (-27.0, "-3"),
        ];
        for (v, want) in tests {
            assert_eq!(ws(cube_root_preserving_sign(v)), want);
        }
    }

    #[test]
    fn test_lab_constants() {
        assert_eq!(ws(LAB_KAPPA), "903.2962962963");
        assert_eq!(ws(LAB_EPSILON), "0.0088564517");
    }

    #[test]
    fn test_xyz_channels() {
        assert_eq!(XYZ_CHANNELS[0].channel.name, "x");
        assert_eq!(XYZ_CHANNELS[1].channel.name, "y");
        assert_eq!(XYZ_CHANNELS[2].channel.name, "z");
    }

    #[test]
    fn test_lab_to_lch() {
        let c = lab_to_lch(
            ColorSpace::Lch,
            Some(50.0),
            Some(1.0),
            Some(0.0),
            Some(1.0),
            false,
            false,
        )
        .unwrap();
        assert_eq!(c.space, ColorSpace::Lch);
        assert_eq!(ws(c.channel0), "50");
        assert_eq!(ws(c.channel1), "1");
        assert_eq!(ws(c.channel2), "0");
        assert_eq!(ws(c.alpha), "1");
    }

    #[test]
    fn test_lab_to_lch_hue90() {
        let c = lab_to_lch(
            ColorSpace::Lch,
            Some(50.0),
            Some(0.0),
            Some(1.0),
            Some(1.0),
            false,
            false,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "50");
        assert_eq!(ws(c.channel1), "1");
        assert_eq!(ws(c.channel2), "90");
    }

    #[test]
    fn test_lab_to_lch_hue180() {
        let c = lab_to_lch(
            ColorSpace::Lch,
            Some(50.0),
            Some(-1.0),
            Some(0.0),
            Some(1.0),
            false,
            false,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "50");
        assert_eq!(ws(c.channel1), "1");
        assert_eq!(ws(c.channel2), "180");
    }

    #[test]
    fn test_lab_to_lch_hue270() {
        let c = lab_to_lch(
            ColorSpace::Lch,
            Some(50.0),
            Some(0.0),
            Some(-1.0),
            Some(1.0),
            false,
            false,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "50");
        assert_eq!(ws(c.channel1), "1");
        assert_eq!(ws(c.channel2), "270");
    }

    #[test]
    fn test_lab_to_lch_missing_chroma() {
        let c = lab_to_lch(
            ColorSpace::Lch,
            Some(50.0),
            Some(1.0),
            Some(0.0),
            Some(1.0),
            true,
            false,
        )
        .unwrap();
        assert!(c.channel1_or_nil().is_none(), "chroma should be missing");
    }

    #[test]
    fn test_lab_to_lch_missing_hue() {
        let c = lab_to_lch(
            ColorSpace::Lch,
            Some(50.0),
            Some(1.0),
            Some(0.0),
            Some(1.0),
            false,
            true,
        )
        .unwrap();
        assert!(c.channel2_or_nil().is_none(), "hue should be missing");
    }

    #[test]
    fn test_srgb_to_linear_negative_handle() {
        assert_eq!(ws(srgb_and_display_p3_to_linear(-0.5)), "-0.2140411405");
    }

    #[test]
    fn test_prophoto_threshold() {
        let below = 16.0 / 512.0 / 2.0;
        let above = 16.0 / 512.0 * 2.0;
        assert_eq!(ws(prophoto_to_linear(below)), "0.0009765625");
        assert_eq!(ws(prophoto_to_linear(above)), "0.0068011763");
    }

    #[test]
    fn test_rec2020_gamma_240() {
        // Since dart-sass 1.102 (#2729) rec2020 uses the plain 2.4-gamma
        // power law. Goldens below are Dart CLI output for
        // color(rec2020 0.5 0.25 0.75) -> srgb.
        let c = convert_color(
            ColorSpace::Rec2020,
            ColorSpace::Srgb,
            0.5,
            0.25,
            0.75,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "0.5439373396");
        assert_eq!(ws(c.channel1), "0.1170946921");
        assert_eq!(ws(c.channel2), "0.7697591314");
    }
}
