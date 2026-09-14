// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/srgb.dart
// go-source: go/value/color_space_srgb.go

use crate::common::exception::SassResult;
use crate::util::number;
use crate::value::color::{new_color_for_space_internal_no_check, ColorSpace, SassColor};
use crate::value::color_conversions::{convert_linear, ConvertLinearOpts, SrgbConvertOpts};
use crate::value::color_utils::{self};

// The sRGB space: unit-range channels defined by CSS Color 4. (Dart:
// `SrgbColorSpace`, internal.)
//
// The conversion hub for the legacy spaces: RGB rescales into sRGB, and
// HSL/HWB/Lab-family destinations funnel back through here.

// Converts an sRGB color to `dest` (entry point; no missing-channel flags).
pub(crate) fn srgb_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    srgb_convert_internal(dest, c0, c1, c2, alpha, None)
}

// Carries the missing-channel flags (`opts`) that HSL/HWB callers forward so
// achromatic results keep a powerless (missing) hue. HSL/HWB destinations use
// the CSS Color 4 RGB-to-HSL algorithm (max/min/delta, with the negative-
// saturation fixup that rotates the hue 180 degrees); RGB rescales to 0-255,
// sRGB-linear applies the transfer function, and everything else delegates to
// the generic linear conversion (`convert_linear`).
pub(crate) fn srgb_convert_internal(
    dest: ColorSpace,
    red: Option<f64>,
    green: Option<f64>,
    blue: Option<f64>,
    alpha: Option<f64>,
    opts: Option<&SrgbConvertOpts>,
) -> SassResult<SassColor> {
    // Analogous missingness (#2810): an all-missing source converts to an
    // all-missing destination, as does a source missing a whole analogous
    // lightness+chroma+hue set.
    let missing_lightness = opts.is_some_and(|o| o.missing_lightness);
    let missing_chroma = opts.is_some_and(|o| o.missing_chroma);
    let missing_hue = opts.is_some_and(|o| o.missing_hue);
    if (red.is_none() && green.is_none() && blue.is_none())
        || (missing_lightness && missing_chroma && missing_hue)
    {
        return Ok(new_color_for_space_internal_no_check(
            dest, None, None, None, alpha,
        ));
    }

    let rv = red.unwrap_or(0.0);
    let gv = green.unwrap_or(0.0);
    let bv = blue.unwrap_or(0.0);

    match dest {
        ColorSpace::Hsl | ColorSpace::Hwb => {
            // Algorithm from https://drafts.csswg.org/css-color-4/#rgb-to-hsl
            // NaN-propagating max/min: matches Go's math.Max/math.Min semantics
            // (Go propagates NaN, Rust's f64::max/min uses maxNum which ignores NaN)
            let max = if rv.is_nan() || gv.is_nan() || bv.is_nan() {
                f64::NAN
            } else {
                rv.max(gv).max(bv)
            };
            let min = if rv.is_nan() || gv.is_nan() || bv.is_nan() {
                f64::NAN
            } else {
                rv.min(gv).min(bv)
            };
            let delta = max - min;

            let mut hue = if max == min {
                0.0
            } else if max == rv {
                60.0 * (gv - bv) / delta + 360.0
            } else if max == gv {
                60.0 * (bv - rv) / delta + 120.0
            } else {
                60.0 * (rv - gv) / delta + 240.0
            };

            if dest == ColorSpace::Hsl {
                let lightness = (min + max) / 2.0;
                let mut saturation = if lightness == 0.0 || lightness == 1.0 {
                    0.0
                } else {
                    // FMA prevention: intermediate bindings as f64 to avoid LLVM fusion.
                    // See Go: float64(100*(max-lightness)) / math.Min(lightness, float64(1-lightness))
                    let num: f64 = 100.0 * (max - lightness);
                    let den: f64 = lightness.min(1.0 - lightness);
                    num / den
                };
                if saturation < 0.0 {
                    hue += 180.0;
                    saturation = saturation.abs();
                }
                hue %= 360.0;

                let h_ptr = if !(missing_hue || number::fuzzy_equals(saturation, 0.0)) {
                    Some(hue)
                } else {
                    None
                };
                let s_ptr = if !missing_chroma {
                    Some(saturation)
                } else {
                    None
                };
                let l_ptr = if !missing_lightness {
                    Some(lightness * 100.0)
                } else {
                    None
                };
                return Ok(new_color_for_space_internal_no_check(
                    ColorSpace::Hsl,
                    h_ptr,
                    s_ptr,
                    l_ptr,
                    alpha,
                ));
            }

            let whiteness = min * 100.0;
            let blackness = 100.0 - (max * 100.0);
            hue %= 360.0;
            let h_missing =
                missing_hue || number::fuzzy_greater_than_or_equals(whiteness + blackness, 100.0);

            let h_ptr = if !h_missing { Some(hue) } else { None };
            // Analogous missingness (#2810): whiteness/blackness are not
            // analogous to any channels, so they go missing only with the
            // whole lightness+chroma set.
            let wb_ptr = |v: f64| {
                if missing_chroma && missing_lightness {
                    None
                } else {
                    Some(v)
                }
            };
            Ok(new_color_for_space_internal_no_check(
                ColorSpace::Hwb,
                h_ptr,
                wb_ptr(whiteness),
                wb_ptr(blackness),
                alpha,
            ))
        }
        ColorSpace::Rgb => {
            let r255 = red.map(|v| v * 255.0);
            let g255 = green.map(|v| v * 255.0);
            let b255 = blue.map(|v| v * 255.0);
            Ok(new_color_for_space_internal_no_check(
                dest, r255, g255, b255, alpha,
            ))
        }
        ColorSpace::SrgbLinear => {
            let r_lin = red.map(color_utils::srgb_and_display_p3_to_linear);
            let g_lin = green.map(color_utils::srgb_and_display_p3_to_linear);
            let b_lin = blue.map(color_utils::srgb_and_display_p3_to_linear);
            Ok(new_color_for_space_internal_no_check(
                dest, r_lin, g_lin, b_lin, alpha,
            ))
        }
        _ => {
            let linear_opts = opts.map(|o| ConvertLinearOpts {
                missing_lightness: o.missing_lightness,
                missing_chroma: o.missing_chroma,
                missing_hue: o.missing_hue,
                missing_a: false,
                missing_b: false,
            });
            convert_linear(
                ColorSpace::Srgb,
                dest,
                red,
                green,
                blue,
                alpha,
                linear_opts.as_ref(),
            )
        }
    }
}

pub(crate) fn srgb_transformation_matrix(dest: ColorSpace) -> Option<&'static [f64; 9]> {
    // Matrices from linear sRGB to each linear destination. HSL/HWB/RGB and
    // the Lab family need no matrix here: they convert through dedicated
    // branches above or via the generic `convert_linear` routing instead.
    use super::color_conversions::*;
    match dest {
        ColorSpace::DisplayP3 | ColorSpace::DisplayP3Linear => {
            Some(&LINEAR_SRGB_TO_LINEAR_DISPLAY_P3)
        }
        ColorSpace::A98Rgb => Some(&LINEAR_SRGB_TO_LINEAR_A98_RGB),
        ColorSpace::ProphotoRgb => Some(&LINEAR_SRGB_TO_LINEAR_PROPHOTO_RGB),
        ColorSpace::Rec2020 => Some(&LINEAR_SRGB_TO_LINEAR_REC2020),
        ColorSpace::XyzD65 => Some(&LINEAR_SRGB_TO_XYZ_D65),
        ColorSpace::XyzD50 => Some(&LINEAR_SRGB_TO_XYZ_D50),
        ColorSpace::Lms => Some(&LINEAR_SRGB_TO_LMS),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::color::ColorSpace;
    use super::super::color_conversions::convert_color;
    use crate::util::number;
    fn ws(v: f64) -> String {
        number::write_number_to_string(v)
    }

    #[test]
    fn test_metadata() {
        assert_eq!(ColorSpace::Srgb.name(), "srgb");
        assert!(ColorSpace::Srgb.is_bounded());
        assert!(!ColorSpace::Srgb.is_legacy());
        assert!(!ColorSpace::Srgb.is_polar());
    }

    #[test]
    fn test_srgb_convert() {
        let c = convert_color(
            ColorSpace::Srgb,
            ColorSpace::Lab,
            1.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "54.2905414047");
        assert_eq!(ws(c.channel1), "80.8049281704");
        assert_eq!(ws(c.channel2), "69.8909647686");
        let c2 = convert_color(
            ColorSpace::Srgb,
            ColorSpace::Hsl,
            1.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "0");
        assert_eq!(ws(c2.channel1), "100");
        assert_eq!(ws(c2.channel2), "50");
    }
}
