// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/rgb.dart
// go-source: go/value/color_space_rgb.go

// The legacy RGB space: 0–255 channels that convert by rescaling to unit
// sRGB and delegating. (Dart: `RgbColorSpace`, internal.)

use crate::common::exception::SassResult;
use crate::value::color::{new_color_for_space_internal_no_check, ColorSpace, SassColor};
use crate::value::color_space_srgb::srgb_convert_internal;

// Converts an RGB color to `dest`. Rescales 0–255 channels to unit sRGB and
// delegates to sRGB conversion; an RGB destination is stored directly
// without rescaling. Missing (`None`) channels propagate through the
// rescale untouched.
pub(crate) fn rgb_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    if dest == ColorSpace::Rgb {
        return Ok(new_color_for_space_internal_no_check(
            dest, c0, c1, c2, alpha,
        ));
    }
    let r = c0.map(|v| v / 255.0);
    let g = c1.map(|v| v / 255.0);
    let b = c2.map(|v| v / 255.0);
    srgb_convert_internal(dest, r, g, b, alpha, None)
}

pub(crate) fn rgb_transformation_matrix(_dest: ColorSpace) -> Option<&'static [f64; 9]> {
    // Matches Dart: `RgbColorSpace` defines no matrix of its own. Every
    // conversion runs through the sRGB path above instead.
    None
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
        assert_eq!(ColorSpace::Rgb.name(), "rgb");
        assert!(ColorSpace::Rgb.is_bounded());
        assert!(ColorSpace::Rgb.is_legacy());
        assert!(!ColorSpace::Rgb.is_polar());
    }

    #[test]
    fn test_rgb_convert() {
        let c = convert_color(
            ColorSpace::Rgb,
            ColorSpace::Hsl,
            1.0,
            0.5,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "30");
        assert_eq!(ws(c.channel1), "100");
        assert_eq!(ws(c.channel2), "0.1960784314");
        let c2 = convert_color(
            ColorSpace::Rgb,
            ColorSpace::Hwb,
            1.0,
            0.5,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "30");
        assert_eq!(ws(c2.channel1), "0");
        assert_eq!(ws(c2.channel2), "99.6078431373");
    }
}
