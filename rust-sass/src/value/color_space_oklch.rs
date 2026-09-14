// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/oklch.dart
// go-source: go/value/color_space_oklch.go

// The OKLCH space: polar form of OKLab (lightness, chroma, hue). (Dart:
// `OklchColorSpace`, internal.)

use crate::common::exception::SassResult;
use crate::value::color::{ColorSpace, SassColor};
use crate::value::color_conversions::OklabConvertOpts;
use crate::value::color_space_oklab::oklab_convert_internal;
use std::f64::consts::PI;

// Converts an OKLCH color to `dest`: resolves chroma/hue to rectangular
// `a`/`b` (hue in degrees) and delegates to OKLab, forwarding which polar
// channels were missing so powerless-channel rules survive.
pub(crate) fn oklch_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    let chv = c1.unwrap_or(0.0);
    let hv = c2.unwrap_or(0.0);
    let hue_radians = hv * PI / 180.0;
    let av = chv * hue_radians.cos();
    let bv = chv * hue_radians.sin();

    oklab_convert_internal(
        dest,
        c0,
        Some(av),
        Some(bv),
        alpha,
        Some(&OklabConvertOpts {
            missing_chroma: c1.is_none(),
            missing_hue: c2.is_none(),
        }),
    )
}

pub(crate) fn oklch_transformation_matrix(_dest: ColorSpace) -> Option<&'static [f64; 9]> {
    // Matches Dart: `OklchColorSpace` defines no matrix of its own. Every
    // conversion runs through OKLab instead.
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
        assert_eq!(ColorSpace::Oklch.name(), "oklch");
        assert!(!ColorSpace::Oklch.is_bounded());
        assert!(!ColorSpace::Oklch.is_legacy());
        assert!(ColorSpace::Oklch.is_polar());
    }

    #[test]
    fn test_oklch_convert() {
        let c = convert_color(
            ColorSpace::Oklch,
            ColorSpace::Srgb,
            0.5,
            0.1,
            45.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "0.569700204");
        assert_eq!(ws(c.channel1), "0.3088484941");
        assert_eq!(ws(c.channel2), "0.1856030772");
        let c2 = convert_color(
            ColorSpace::Oklch,
            ColorSpace::Lab,
            0.5,
            0.1,
            45.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "41.3278676545");
        assert_eq!(ws(c2.channel1), "26.5144151584");
        assert_eq!(ws(c2.channel2), "31.0977049205");
    }
}
