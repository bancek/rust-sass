// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/xyz_d65.dart
// go-source: go/value/color_space_xyz_d65.go

use crate::common::exception::SassResult;
use crate::value::color::{ColorSpace, SassColor};
use crate::value::color_conversions::convert_linear;

// The XYZ D65 space (serialized as `xyz`): unbounded tristimulus values under
// the D65 white point. Unlike XYZ D50 it defines no conversion of its own —
// every destination runs through the generic linear path. (Dart:
// `XyzD65ColorSpace`, internal.)

// Converts an XYZ D65 color to `dest` via the generic linear conversion
// (`convert_linear`).
pub(crate) fn xyz_d65_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    convert_linear(ColorSpace::XyzD65, dest, c0, c1, c2, alpha, None)
}

pub(crate) fn xyz_d65_transformation_matrix(dest: ColorSpace) -> Option<&'static [f64; 9]> {
    // Linear matrices from XYZ D65 to each linear destination. Lab/LCH route
    // through XYZ D50 inside the generic path, so they need no matrix here.
    use super::color_conversions::*;
    match dest {
        ColorSpace::Srgb | ColorSpace::SrgbLinear | ColorSpace::Rgb => {
            Some(&XYZ_D65_TO_LINEAR_SRGB)
        }
        ColorSpace::A98Rgb => Some(&XYZ_D65_TO_LINEAR_A98_RGB),
        ColorSpace::ProphotoRgb => Some(&XYZ_D65_TO_LINEAR_PROPHOTO_RGB),
        ColorSpace::DisplayP3 | ColorSpace::DisplayP3Linear => Some(&XYZ_D65_TO_LINEAR_DISPLAY_P3),
        ColorSpace::Rec2020 => Some(&XYZ_D65_TO_LINEAR_REC2020),
        ColorSpace::XyzD50 => Some(&XYZ_D65_TO_XYZ_D50),
        ColorSpace::Lms => Some(&XYZ_D65_TO_LMS),
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
        assert_eq!(ColorSpace::XyzD65.name(), "xyz");
        assert!(!ColorSpace::XyzD65.is_bounded());
        assert!(!ColorSpace::XyzD65.is_legacy());
        assert!(!ColorSpace::XyzD65.is_polar());
    }

    #[test]
    fn test_to_from_linear() {
        let space = ColorSpace::XyzD65;
        for v in [0.0, 0.5, 1.0] {
            let lin = space.to_linear(v);
            let round = space.from_linear(lin);
            assert!((v - round).abs() < 1e-10, "roundtrip({v}) = {round}");
        }
    }

    #[test]
    fn test_xyz_d65_convert() {
        let c = convert_color(
            ColorSpace::XyzD65,
            ColorSpace::Lab,
            0.5,
            0.5,
            0.5,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "76.1608841835");
        assert_eq!(ws(c.channel1), "7.1944893389");
        assert_eq!(ws(c.channel2), "4.6048603909");
        let c2 = convert_color(
            ColorSpace::XyzD65,
            ColorSpace::Srgb,
            0.5,
            0.5,
            0.5,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "0.7992092975");
        assert_eq!(ws(c2.channel1), "0.7180602368");
        assert_eq!(ws(c2.channel2), "0.7044225805");
    }
}
