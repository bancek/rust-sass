// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/srgb_linear.dart
// go-source: go/value/color_space_srgb_linear.go

use crate::common::exception::SassResult;
use crate::value::color::{ColorSpace, SassColor};
use crate::value::color_conversions::convert_linear;
use crate::value::color_space_srgb::srgb_convert;
use crate::value::color_utils;

// The linear-light sRGB space: the transfer function is the identity, so
// channels are already linear. (Dart: `SrgbLinearColorSpace`, internal.)

// Converts a linear-sRGB color to `dest`. Legacy (RGB/HSL/HWB) and plain
// sRGB destinations go through the sRGB gamma curve first (`from_linear`
// then `srgb_convert`); every other destination uses the generic linear
// conversion (`convert_linear`).
pub(crate) fn srgb_linear_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    if matches!(
        dest,
        ColorSpace::Srgb | ColorSpace::Rgb | ColorSpace::Hsl | ColorSpace::Hwb
    ) {
        let r_lin = c0.map(color_utils::srgb_and_display_p3_from_linear);
        let g_lin = c1.map(color_utils::srgb_and_display_p3_from_linear);
        let b_lin = c2.map(color_utils::srgb_and_display_p3_from_linear);
        return srgb_convert(dest, r_lin, g_lin, b_lin, alpha);
    }
    convert_linear(ColorSpace::SrgbLinear, dest, c0, c1, c2, alpha, None)
}

pub(crate) fn srgb_linear_transformation_matrix(dest: ColorSpace) -> Option<&'static [f64; 9]> {
    // Same matrices as sRGB: both spaces share linear-light primaries, so the
    // linear destination matrices coincide.
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
        assert_eq!(ColorSpace::SrgbLinear.name(), "srgb-linear");
        assert!(ColorSpace::SrgbLinear.is_bounded());
        assert!(!ColorSpace::SrgbLinear.is_legacy());
        assert!(!ColorSpace::SrgbLinear.is_polar());
    }

    #[test]
    fn test_to_from_linear() {
        let space = ColorSpace::SrgbLinear;
        for v in [0.0, 0.5, 1.0] {
            let lin = space.to_linear(v);
            let round = space.from_linear(lin);
            assert!((v - round).abs() < 1e-10, "roundtrip({v}) = {round}");
        }
    }

    #[test]
    fn test_srgb_linear_convert() {
        let c = convert_color(
            ColorSpace::SrgbLinear,
            ColorSpace::Srgb,
            1.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "1");
        assert_eq!(ws(c.channel1), "0");
        assert_eq!(ws(c.channel2), "0");
        let c2 = convert_color(
            ColorSpace::SrgbLinear,
            ColorSpace::Lab,
            1.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "54.2905414047");
        assert_eq!(ws(c2.channel1), "80.8049281704");
        assert_eq!(ws(c2.channel2), "69.8909647686");
    }
}
