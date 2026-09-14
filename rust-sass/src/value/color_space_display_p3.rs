// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/display_p3.dart
// go-source: go/value/color_space_display_p3.go

// The display-p3 space: bounded RGB channels sharing the sRGB transfer
// function. (Dart: `DisplayP3ColorSpace`, internal.)

use crate::common::exception::SassResult;
use crate::value::color::{new_color_for_space_internal_no_check, ColorSpace, SassColor};
use crate::value::color_conversions::convert_linear;
use crate::value::color_utils;

// Converts a display-p3 color to `dest`. Display-p3-linear is reached by
// applying the shared sRGB/display-p3 transfer function directly (matching
// Dart's per-channel nullable map, so missing channels stay missing);
// everything else goes through the generic linear path.
pub(crate) fn display_p3_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    if dest == ColorSpace::DisplayP3Linear {
        let r_lin = c0.map(color_utils::srgb_and_display_p3_to_linear);
        let g_lin = c1.map(color_utils::srgb_and_display_p3_to_linear);
        let b_lin = c2.map(color_utils::srgb_and_display_p3_to_linear);
        return Ok(new_color_for_space_internal_no_check(
            dest, r_lin, g_lin, b_lin, alpha,
        ));
    }
    convert_linear(ColorSpace::DisplayP3, dest, c0, c1, c2, alpha, None)
}

// Returns the matrix for a linear display-p3-to-`dest` conversion. Spaces
// handled by the generic `convert_linear` routing need none here.
pub(crate) fn display_p3_transformation_matrix(dest: ColorSpace) -> Option<&'static [f64; 9]> {
    use super::color_conversions::*;
    match dest {
        ColorSpace::Srgb | ColorSpace::SrgbLinear | ColorSpace::Rgb => {
            Some(&LINEAR_DISPLAY_P3_TO_LINEAR_SRGB)
        }
        ColorSpace::A98Rgb => Some(&LINEAR_DISPLAY_P3_TO_LINEAR_A98_RGB),
        ColorSpace::ProphotoRgb => Some(&LINEAR_DISPLAY_P3_TO_LINEAR_PROPHOTO_RGB),
        ColorSpace::Rec2020 => Some(&LINEAR_DISPLAY_P3_TO_LINEAR_REC2020),
        ColorSpace::XyzD65 => Some(&LINEAR_DISPLAY_P3_TO_XYZ_D65),
        ColorSpace::XyzD50 => Some(&LINEAR_DISPLAY_P3_TO_XYZ_D50),
        ColorSpace::Lms => Some(&LINEAR_DISPLAY_P3_TO_LMS),
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
        assert_eq!(ColorSpace::DisplayP3.name(), "display-p3");
        assert!(ColorSpace::DisplayP3.is_bounded());
        assert!(!ColorSpace::DisplayP3.is_legacy());
        assert!(!ColorSpace::DisplayP3.is_polar());
    }

    #[test]
    fn test_to_from_linear() {
        let space = ColorSpace::DisplayP3;
        for v in [0.0, 0.5, 1.0] {
            let lin = space.to_linear(v);
            let round = space.from_linear(lin);
            assert!((v - round).abs() < 1e-10);
        }
    }

    #[test]
    fn test_display_p3_convert() {
        let c = convert_color(
            ColorSpace::DisplayP3,
            ColorSpace::Lab,
            1.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "56.2077729169");
        assert_eq!(ws(c.channel1), "94.464418467");
        assert_eq!(ws(c.channel2), "98.8921195438");
        let c2 = convert_color(
            ColorSpace::DisplayP3,
            ColorSpace::Srgb,
            1.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "1.0930663624");
        assert_eq!(ws(c2.channel1), "-0.2267419736");
        assert_eq!(ws(c2.channel2), "-0.1501345809");
    }
}
