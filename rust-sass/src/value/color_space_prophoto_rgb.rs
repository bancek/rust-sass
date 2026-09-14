// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/prophoto_rgb.dart
// go-source: go/value/color_space_prophoto_rgb.go

// The prophoto-rgb space: bounded RGB channels with a piecewise transfer
// function (linear near black, exponent 1.8 above). (Dart:
// `ProphotoRgbColorSpace`, internal.)

use crate::common::exception::SassResult;
use crate::value::color::{ColorSpace, SassColor};
use crate::value::color_conversions::convert_linear;

// Converts a prophoto-rgb color to `dest` through the generic linear path:
// the transfer functions live on [`ColorSpace::to_linear`]/`from_linear` and
// the matrix below, so there is nothing space-specific to do here.
pub(crate) fn prophoto_rgb_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    convert_linear(ColorSpace::ProphotoRgb, dest, c0, c1, c2, alpha, None)
}

// Returns the matrix for a linear prophoto-rgb-to-`dest` conversion. Spaces
// handled by the generic `convert_linear` routing need none here.
pub(crate) fn prophoto_rgb_transformation_matrix(dest: ColorSpace) -> Option<&'static [f64; 9]> {
    use super::color_conversions::*;
    match dest {
        ColorSpace::Srgb | ColorSpace::SrgbLinear | ColorSpace::Rgb => {
            Some(&LINEAR_PROPHOTO_RGB_TO_LINEAR_SRGB)
        }
        ColorSpace::A98Rgb => Some(&LINEAR_PROPHOTO_RGB_TO_LINEAR_A98_RGB),
        ColorSpace::DisplayP3 | ColorSpace::DisplayP3Linear => {
            Some(&LINEAR_PROPHOTO_RGB_TO_LINEAR_DISPLAY_P3)
        }
        ColorSpace::Rec2020 => Some(&LINEAR_PROPHOTO_RGB_TO_LINEAR_REC2020),
        ColorSpace::XyzD65 => Some(&LINEAR_PROPHOTO_RGB_TO_XYZ_D65),
        ColorSpace::XyzD50 => Some(&LINEAR_PROPHOTO_RGB_TO_XYZ_D50),
        ColorSpace::Lms => Some(&LINEAR_PROPHOTO_RGB_TO_LMS),
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
        assert_eq!(ColorSpace::ProphotoRgb.name(), "prophoto-rgb");
        assert!(ColorSpace::ProphotoRgb.is_bounded());
        assert!(!ColorSpace::ProphotoRgb.is_legacy());
        assert!(!ColorSpace::ProphotoRgb.is_polar());
    }

    #[test]
    fn test_to_from_linear() {
        let space = ColorSpace::ProphotoRgb;
        for v in [0.0, 0.5, 1.0] {
            let lin = space.to_linear(v);
            let round = space.from_linear(lin);
            assert!((v - round).abs() < 1e-10, "roundtrip({v}) = {round}");
        }
    }

    #[test]
    fn test_prophoto_rgb_convert() {
        let c = convert_color(
            ColorSpace::ProphotoRgb,
            ColorSpace::Lab,
            1.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "60.6113461371");
        assert_eq!(ws(c.channel1), "139.1593742457");
        assert_eq!(ws(c.channel2), "104.502320926");
        let c2 = convert_color(
            ColorSpace::ProphotoRgb,
            ColorSpace::Srgb,
            1.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "1.3632928088");
        assert_eq!(ws(c2.channel1), "-0.5156626969");
        assert_eq!(ws(c2.channel2), "-0.090130414");
    }
}
