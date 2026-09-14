// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/xyz_d50.dart
// go-source: go/value/color_space_xyz_d50.go

use crate::common::exception::SassResult;
use crate::value::color::{new_color_for_space_internal_no_check, ColorSpace, SassColor};
use crate::value::color_conversions::{
    convert_linear, lab_convert_component_to_f, ConvertLinearOpts, XyzD50ConvertOpts, D50,
};
use crate::value::color_utils;

// The XYZ D50 space: unbounded tristimulus values under the D50 white point,
// the profile-connection hub for the Lab family. (Dart: `XyzD50ColorSpace`,
// internal.)

// Converts an XYZ D50 color to `dest` (entry point; no missing-channel
// flags).
pub(crate) fn xyz_d50_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    xyz_d50_convert_internal(dest, c0, c1, c2, alpha, None)
}

// Carries the missing-channel flags (`opts`) forwarded from the generic
// linear path. Lab/LCH destinations use the Bruce Lindbloom / CSS Color 4
// algorithm (normalize by the D50 white point, apply the epsilon/kappa
// piecewise cube-root from `_convertComponentToLabF`, scale to L/a/b); LCH
// goes through `lab_to_lch` so powerless-channel rules apply. Every other
// destination delegates to the generic linear conversion (`convert_linear`).
pub(crate) fn xyz_d50_convert_internal(
    dest: ColorSpace,
    x: Option<f64>,
    y: Option<f64>,
    z: Option<f64>,
    alpha: Option<f64>,
    opts: Option<&XyzD50ConvertOpts>,
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
        || (x.is_none() && y.is_none() && z.is_none())
    {
        return Ok(new_color_for_space_internal_no_check(
            dest, None, None, None, alpha,
        ));
    }

    let xv = x.unwrap_or(0.0);
    let yv = y.unwrap_or(0.0);
    let zv = z.unwrap_or(0.0);

    match dest {
        ColorSpace::Lab | ColorSpace::Lch => {
            // Algorithm from https://www.w3.org/TR/css-color-4/#color-conversion-code
            // and http://www.brucelindbloom.com/index.html?Eqn_RGB_XYZ_Matrix.html
            let d50 = &*D50;
            let f0 = lab_convert_component_to_f(xv / d50[0]);
            let f1 = lab_convert_component_to_f(yv / d50[1]);
            let f2 = lab_convert_component_to_f(zv / d50[2]);

            let lightness = (116.0 * f1) - 16.0;
            let av = 500.0 * (f0 - f1);
            let bv = 200.0 * (f1 - f2);

            if dest == ColorSpace::Lab {
                let l_p = if !missing_lightness {
                    Some(lightness)
                } else {
                    None
                };
                let a_p = if !missing_a { Some(av) } else { None };
                let b_p = if !missing_b { Some(bv) } else { None };
                return SassColor::new_color_for_space_internal(dest, l_p, a_p, b_p, alpha);
            }

            let l_ptr = if !missing_lightness {
                Some(lightness)
            } else {
                None
            };
            color_utils::lab_to_lch(
                ColorSpace::Lch,
                l_ptr,
                Some(av),
                Some(bv),
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
            convert_linear(ColorSpace::XyzD50, dest, x, y, z, alpha, Some(&linear_opts))
        }
    }
}

pub(crate) fn xyz_d50_transformation_matrix(dest: ColorSpace) -> Option<&'static [f64; 9]> {
    // Linear matrices from XYZ D50 to each linear destination. Lab/LCH need
    // no matrix: they convert through the dedicated branch above.
    use super::color_conversions::*;
    match dest {
        ColorSpace::Srgb | ColorSpace::SrgbLinear | ColorSpace::Rgb => {
            Some(&XYZ_D50_TO_LINEAR_SRGB)
        }
        ColorSpace::A98Rgb => Some(&XYZ_D50_TO_LINEAR_A98_RGB),
        ColorSpace::ProphotoRgb => Some(&XYZ_D50_TO_LINEAR_PROPHOTO_RGB),
        ColorSpace::DisplayP3 | ColorSpace::DisplayP3Linear => Some(&XYZ_D50_TO_LINEAR_DISPLAY_P3),
        ColorSpace::Rec2020 => Some(&XYZ_D50_TO_LINEAR_REC2020),
        ColorSpace::XyzD65 => Some(&XYZ_D50_TO_XYZ_D65),
        ColorSpace::Lms => Some(&XYZ_D50_TO_LMS),
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
        assert_eq!(ColorSpace::XyzD50.name(), "xyz-d50");
        assert!(!ColorSpace::XyzD50.is_bounded());
        assert!(!ColorSpace::XyzD50.is_legacy());
        assert!(!ColorSpace::XyzD50.is_polar());
    }

    #[test]
    fn test_xyz_d50_convert() {
        let c = convert_color(
            ColorSpace::XyzD50,
            ColorSpace::Lab,
            0.5,
            0.5,
            0.5,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "76.0692610142");
        assert_eq!(ws(c.channel1), "4.8387310772");
        assert_eq!(ws(c.channel2), "-10.505341671");
        let c2 = convert_color(
            ColorSpace::XyzD50,
            ColorSpace::Srgb,
            0.5,
            0.5,
            0.5,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "0.7438835606");
        assert_eq!(ws(c2.channel1), "0.7256918895");
        assert_eq!(ws(c2.channel2), "0.811893154");
    }
}
