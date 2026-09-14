// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/rec2020.dart
// go-source: go/value/color_space_rec2020.go

// The rec2020 space: bounded RGB channels with the plain 2.4-gamma power-law
// transfer function (dart-sass 1.102; Dart: `Rec2020ColorSpace`, internal).

use crate::common::exception::SassResult;
use crate::value::color::{ColorSpace, SassColor};
use crate::value::color_conversions::convert_linear;

// Converts a rec2020 color to `dest` through the generic linear path: the
// transfer functions live on [`ColorSpace::to_linear`]/`from_linear` and the
// matrix below, so there is nothing space-specific to do here.
pub(crate) fn rec2020_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    convert_linear(ColorSpace::Rec2020, dest, c0, c1, c2, alpha, None)
}

// Returns the matrix for a linear rec2020-to-`dest` conversion. Spaces
// handled by the generic `convert_linear` routing need none here.
pub(crate) fn rec2020_transformation_matrix(dest: ColorSpace) -> Option<&'static [f64; 9]> {
    use super::color_conversions::*;
    match dest {
        ColorSpace::Srgb | ColorSpace::SrgbLinear | ColorSpace::Rgb => {
            Some(&LINEAR_REC2020_TO_LINEAR_SRGB)
        }
        ColorSpace::A98Rgb => Some(&LINEAR_REC2020_TO_LINEAR_A98_RGB),
        ColorSpace::DisplayP3 | ColorSpace::DisplayP3Linear => {
            Some(&LINEAR_REC2020_TO_LINEAR_DISPLAY_P3)
        }
        ColorSpace::ProphotoRgb => Some(&LINEAR_REC2020_TO_LINEAR_PROPHOTO_RGB),
        ColorSpace::XyzD65 => Some(&LINEAR_REC2020_TO_XYZ_D65),
        ColorSpace::XyzD50 => Some(&LINEAR_REC2020_TO_XYZ_D50),
        ColorSpace::Lms => Some(&LINEAR_REC2020_TO_LMS),
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
        assert_eq!(ColorSpace::Rec2020.name(), "rec2020");
        assert!(ColorSpace::Rec2020.is_bounded());
        assert!(!ColorSpace::Rec2020.is_legacy());
        assert!(!ColorSpace::Rec2020.is_polar());
    }

    #[test]
    fn test_to_from_linear() {
        let space = ColorSpace::Rec2020;
        for v in [0.0, 0.5, 1.0] {
            let lin = space.to_linear(v);
            let round = space.from_linear(lin);
            assert!((v - round).abs() < 1e-10, "roundtrip({v}) = {round}");
        }
    }

    #[test]
    fn test_rec2020_convert() {
        let c = convert_color(
            ColorSpace::Rec2020,
            ColorSpace::Lab,
            1.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "59.8036299926");
        assert_eq!(ws(c.channel1), "116.8849865694");
        assert_eq!(ws(c.channel2), "106.7572157253");
        let c2 = convert_color(
            ColorSpace::Rec2020,
            ColorSpace::Srgb,
            1.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "1.2482198282");
        assert_eq!(ws(c2.channel1), "-0.3879075029");
        assert_eq!(ws(c2.channel2), "-0.1435143925");
    }
}
