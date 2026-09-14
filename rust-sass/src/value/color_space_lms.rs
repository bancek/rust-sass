// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/lms.dart
// go-source: go/value/color_space_lms.go

// The LMS space: intermediate only, never stored in a real color value and
// never returned by name lookup. Conversions to and from OKLab/OKLCH route
// through here. (Dart: `LmsColorSpace`, internal.)

use crate::common::exception::SassResult;
use crate::value::color::{new_color_for_space_internal_no_check, ColorSpace, SassColor};
use crate::value::color_conversions::{
    convert_linear, ConvertLinearOpts, LmsConvertOpts, LMS_TO_OKLAB,
};
use crate::value::color_utils::{self, cube_root_preserving_sign};

// Converts an LMS color to `dest` (entry point; no missing-channel flags).
pub(crate) fn lms_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    lms_convert_internal(dest, c0, c1, c2, alpha, None)
}

// Carries the missing-channel flags (`opts`) forwarded from the generic
// linear path. OKLab destinations apply the sign-preserving cube root and
// the `LMS_TO_OKLAB` matrix (CSS Color 4 algorithm); OKLCH does that same
// step inline through `lab_to_lch` to avoid an extra allocation, since
// OKLCH conversions are hot. Every other destination delegates to the
// generic linear conversion (`convert_linear`).
pub(crate) fn lms_convert_internal(
    dest: ColorSpace,
    long: Option<f64>,
    medium: Option<f64>,
    short: Option<f64>,
    alpha: Option<f64>,
    opts: Option<&LmsConvertOpts>,
) -> SassResult<SassColor> {
    // Analogous missingness normalization (#2810): missing `a`+`b` imply
    // missing chroma+hue and vice versa; an all-missing source converts to
    // an all-missing destination.
    let missing_lightness = opts.is_some_and(|o| o.missing_lightness);
    let mut missing_chroma = opts.is_some_and(|o| o.missing_chroma);
    let mut missing_hue = opts.is_some_and(|o| o.missing_hue);
    let mut missing_a = opts.is_some_and(|o| o.missing_a);
    let mut missing_b = opts.is_some_and(|o| o.missing_b);
    if missing_a && missing_b {
        missing_chroma = true;
        missing_hue = true;
    } else if missing_chroma && missing_hue {
        missing_a = true;
        missing_b = true;
    }
    if (missing_lightness && missing_chroma && missing_hue)
        || (long.is_none() && medium.is_none() && short.is_none())
    {
        return Ok(new_color_for_space_internal_no_check(
            dest, None, None, None, alpha,
        ));
    }

    let lv = long.unwrap_or(0.0);
    let mv = medium.unwrap_or(0.0);
    let sv = short.unwrap_or(0.0);

    match dest {
        ColorSpace::Oklab => {
            // Algorithm from https://drafts.csswg.org/css-color-4/#color-conversion-code
            let long_s = cube_root_preserving_sign(lv);
            let medium_s = cube_root_preserving_sign(mv);
            let short_s = cube_root_preserving_sign(sv);
            let m = &LMS_TO_OKLAB;
            let l = (m[0] * long_s) + (m[1] * medium_s) + (m[2] * short_s);
            let a = (m[3] * long_s) + (m[4] * medium_s) + (m[5] * short_s);
            let b = (m[6] * long_s) + (m[7] * medium_s) + (m[8] * short_s);

            let l_p = if !missing_lightness { Some(l) } else { None };
            let a_p = if !missing_a { Some(a) } else { None };
            let b_p = if !missing_b { Some(b) } else { None };
            SassColor::new_color_for_space_internal(ColorSpace::Oklab, l_p, a_p, b_p, alpha)
        }
        ColorSpace::Oklch => {
            // Equivalent to converting to OKLab and then to OKLCH, done
            // inline to avoid an extra allocation since OKLCH conversions
            // are expected to be very common.
            let long_s = cube_root_preserving_sign(lv);
            let medium_s = cube_root_preserving_sign(mv);
            let short_s = cube_root_preserving_sign(sv);
            let m = &LMS_TO_OKLAB;
            let l = (m[0] * long_s) + (m[1] * medium_s) + (m[2] * short_s);
            let a = (m[3] * long_s) + (m[4] * medium_s) + (m[5] * short_s);
            let b = (m[6] * long_s) + (m[7] * medium_s) + (m[8] * short_s);

            let l_ptr = if !missing_lightness { Some(l) } else { None };
            color_utils::lab_to_lch(
                ColorSpace::Oklch,
                l_ptr,
                Some(a),
                Some(b),
                alpha,
                missing_chroma,
                missing_hue,
            )
        }
        _ => {
            let linear_opts = ConvertLinearOpts {
                missing_lightness,
                missing_chroma,
                missing_hue,
                missing_a,
                missing_b,
            };
            convert_linear(
                ColorSpace::Lms,
                dest,
                long,
                medium,
                short,
                alpha,
                Some(&linear_opts),
            )
        }
    }
}

// Returns the matrix for a linear LMS-to-`dest` conversion. Spaces with
// dedicated branches above (OKLab/OKLCH) need none here.
pub(crate) fn lms_transformation_matrix(dest: ColorSpace) -> Option<&'static [f64; 9]> {
    use super::color_conversions::*;
    match dest {
        ColorSpace::Srgb | ColorSpace::SrgbLinear | ColorSpace::Rgb => Some(&LMS_TO_LINEAR_SRGB),
        ColorSpace::A98Rgb => Some(&LMS_TO_LINEAR_A98_RGB),
        ColorSpace::ProphotoRgb => Some(&LMS_TO_LINEAR_PROPHOTO_RGB),
        ColorSpace::DisplayP3 | ColorSpace::DisplayP3Linear => Some(&LMS_TO_LINEAR_DISPLAY_P3),
        ColorSpace::Rec2020 => Some(&LMS_TO_LINEAR_REC2020),
        ColorSpace::XyzD65 => Some(&LMS_TO_XYZ_D65),
        ColorSpace::XyzD50 => Some(&LMS_TO_XYZ_D50),
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
        assert_eq!(ColorSpace::Lms.name(), "lms");
        assert!(!ColorSpace::Lms.is_bounded());
        assert!(!ColorSpace::Lms.is_legacy());
        assert!(!ColorSpace::Lms.is_polar());
    }

    #[test]
    fn test_lms_convert() {
        let c = convert_color(
            ColorSpace::Lms,
            ColorSpace::Srgb,
            0.5,
            0.3,
            0.2,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "1.0395111833");
        assert_eq!(ws(c.channel1), "0.3141551416");
        assert_eq!(ws(c.channel2), "0.3935597021");
        let c2 = convert_color(
            ColorSpace::Lms,
            ColorSpace::Lab,
            0.5,
            0.3,
            0.2,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "62.3783913344");
        assert_eq!(ws(c2.channel1), "70.6051985009");
        assert_eq!(ws(c2.channel2), "31.527589847");
    }
}
