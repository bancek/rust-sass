// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/lab.dart
// go-source: go/value/color_space_lab.go

// The Lab space: perceptual lightness plus rectangular `a`/`b` channels, the
// source side of the XYZ D50 conversion hub. (Dart: `LabColorSpace`,
// internal.)

use crate::common::exception::SassResult;
use crate::value::color::{ColorSpace, SassColor};
use crate::value::color_conversions::{lab_f_to_xz, LabConvertOpts, XyzD50ConvertOpts, D50};
use crate::value::color_space_xyz_d50::xyz_d50_convert_internal;
use crate::value::color_utils;

// Converts a Lab color to `dest` (entry point; no missing-channel flags).
pub(crate) fn lab_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    lab_convert_internal(dest, c0, c1, c2, alpha, None)
}

// Carries the missing-channel flags (`opts`) that LCH callers forward. A
// Lab destination keeps missing `a`/`b` powerless below black
// (`lightness` missing or zero); LCH goes through `lab_to_lch` so
// powerless-channel rules apply. Every other destination routes through XYZ
// D50 with the CSS Color 4 / Lindbloom Lab-to-XYZ algorithm (`f1`, the
// epsilon/kappa piecewise via `lab_f_to_xz`, scaled by the D50 white point).
pub(crate) fn lab_convert_internal(
    dest: ColorSpace,
    lightness: Option<f64>,
    a: Option<f64>,
    b: Option<f64>,
    alpha: Option<f64>,
    opts: Option<&LabConvertOpts>,
) -> SassResult<SassColor> {
    // Analogous missingness: null `a`+`b` imply missing chroma+hue and vice
    // versa (#2810).
    let (mut a, mut b) = (a, b);
    let mut missing_chroma = opts.is_some_and(|o| o.missing_chroma);
    let mut missing_hue = opts.is_some_and(|o| o.missing_hue);
    if missing_chroma && missing_hue {
        a = None;
        b = None;
    } else if a.is_none() && b.is_none() {
        missing_chroma = true;
        missing_hue = true;
    }

    let lv = lightness.unwrap_or(0.0);
    let av = a.unwrap_or(0.0);
    let bv = b.unwrap_or(0.0);

    match dest {
        ColorSpace::Lab => {
            let powerless_ab = lightness.is_none() || number::fuzzy_equals(lv, 0.0);
            let (new_a, new_b) = if powerless_ab { (None, None) } else { (a, b) };
            SassColor::new_color_for_space_internal(dest, lightness, new_a, new_b, alpha)
        }
        ColorSpace::Lch => color_utils::lab_to_lch(dest, lightness, a, b, alpha, false, false),
        _ => {
            // Algorithm from https://www.w3.org/TR/css-color-4/#color-conversion-code
            // and http://www.brucelindbloom.com/index.html?Eqn_RGB_XYZ_Matrix.html
            let missing_lightness = lightness.is_none();
            let lv2 = if lightness.is_some() { lv } else { 0.0 };
            let f1 = (lv2 + 16.0) / 116.0;
            let xv = lab_f_to_xz(av / 500.0 + f1) * D50[0];
            let d50 = &*D50;
            let yv = if lv2 > color_utils::LAB_KAPPA * color_utils::LAB_EPSILON {
                let f1cube = (lv2 + 16.0) / 116.0;
                f1cube * f1cube * f1cube * d50[1]
            } else {
                (lv2 / color_utils::LAB_KAPPA) * d50[1]
            };
            let zv = lab_f_to_xz(f1 - bv / 200.0) * D50[2];

            xyz_d50_convert_internal(
                dest,
                Some(xv),
                Some(yv),
                Some(zv),
                alpha,
                Some(&XyzD50ConvertOpts {
                    missing_lightness,
                    missing_chroma,
                    missing_hue,
                    missing_a: a.is_none(),
                    missing_b: b.is_none(),
                }),
            )
        }
    }
}

use crate::util::number;

pub(crate) fn lab_transformation_matrix(_dest: ColorSpace) -> Option<&'static [f64; 9]> {
    // Matches Dart: `LabColorSpace` defines no matrix of its own. Every
    // conversion runs through the dedicated branches above instead.
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
        assert_eq!(ColorSpace::Lab.name(), "lab");
        assert!(!ColorSpace::Lab.is_bounded());
        assert!(!ColorSpace::Lab.is_legacy());
        assert!(!ColorSpace::Lab.is_polar());
    }

    #[test]
    fn test_lab_convert() {
        let c = convert_color(
            ColorSpace::Lab,
            ColorSpace::Srgb,
            50.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "0.4663266093");
        assert_eq!(ws(c.channel1), "0.4663266093");
        assert_eq!(ws(c.channel2), "0.4663266093");
        let c2 = convert_color(
            ColorSpace::Lab,
            ColorSpace::Lch,
            50.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "50");
        assert_eq!(ws(c2.channel1), "0");
        assert_eq!(ws(c2.channel2), "0");
    }

    // Analogous sets of missing channels survive conversion (#2810): the
    // changelog example `color.to-space(lch(50% none none), lab)` is
    // `lab(50% none none)`, not `lab(50% 0 0)`.
    #[test]
    fn test_analogous_missing_propagate_lch_to_lab() {
        let c = convert_color(
            ColorSpace::Lch,
            ColorSpace::Lab,
            50.0,
            0.0,
            0.0,
            1.0,
            [false, true, true, false],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "50");
        assert!(c.is_channel1_missing());
        assert!(c.is_channel2_missing());
    }

    #[test]
    fn test_analogous_missing_propagate_lab_to_lch() {
        let c = convert_color(
            ColorSpace::Lab,
            ColorSpace::Lch,
            50.0,
            0.0,
            0.0,
            1.0,
            [false, true, true, false],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "50");
        assert!(c.is_channel1_missing());
        assert!(c.is_channel2_missing());
    }
}
