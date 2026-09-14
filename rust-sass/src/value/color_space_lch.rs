// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/lch.dart
// go-source: go/value/color_space_lch.go

// The LCH space: polar form of Lab (lightness, chroma, hue). (Dart:
// `LchColorSpace`, internal.)

use crate::common::exception::SassResult;
use crate::value::color::{ColorSpace, SassColor};
use crate::value::color_conversions::LabConvertOpts;
use crate::value::color_space_lab::lab_convert_internal;
use std::f64::consts::PI;

// Converts an LCH color to `dest`: resolves chroma/hue to rectangular `a`/`b`
// (hue in degrees) and delegates to Lab, forwarding which polar channels
// were missing so powerless-channel rules survive.
pub(crate) fn lch_convert(
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

    lab_convert_internal(
        dest,
        c0,
        Some(av),
        Some(bv),
        alpha,
        Some(&LabConvertOpts {
            missing_chroma: c1.is_none(),
            missing_hue: c2.is_none(),
        }),
    )
}

pub(crate) fn lch_transformation_matrix(_dest: ColorSpace) -> Option<&'static [f64; 9]> {
    // Matches Dart: `LchColorSpace` defines no matrix of its own. Every
    // conversion runs through Lab instead.
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
        assert_eq!(ColorSpace::Lch.name(), "lch");
        assert!(!ColorSpace::Lch.is_bounded());
        assert!(!ColorSpace::Lch.is_legacy());
        assert!(ColorSpace::Lch.is_polar());
    }

    #[test]
    fn test_lch_convert() {
        let c = convert_color(
            ColorSpace::Lch,
            ColorSpace::Lab,
            50.0,
            10.0,
            45.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "50");
        assert_eq!(ws(c.channel1), "7.0710678119");
        assert_eq!(ws(c.channel2), "7.0710678119");
        let c2 = convert_color(
            ColorSpace::Lch,
            ColorSpace::Srgb,
            50.0,
            10.0,
            45.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "0.5269006242");
        assert_eq!(ws(c2.channel1), "0.4492146127");
        assert_eq!(ws(c2.channel2), "0.4206062974");
    }
}
