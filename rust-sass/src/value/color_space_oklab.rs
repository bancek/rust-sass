// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/oklab.dart
// go-source: go/value/color_space_oklab.go

// The OKLab space: perceptual lightness plus rectangular `a`/`b` channels.
// (Dart: `OklabColorSpace`, internal.)

use crate::common::exception::SassResult;
use crate::value::color::{ColorSpace, SassColor};
use crate::value::color_conversions::{LmsConvertOpts, OklabConvertOpts, OKLAB_TO_LMS};
use crate::value::color_space_lms::lms_convert_internal;
use crate::value::color_utils;

// Converts an OKLab color to `dest` (entry point; no missing-channel flags).
pub(crate) fn oklab_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    oklab_convert_internal(dest, c0, c1, c2, alpha, None)
}

// Carries the missing-channel flags (`opts`) that OKLCH callers forward. An
// OKLCH destination goes through `lab_to_lch` so powerless-channel rules
// apply; every other destination cubes the `OKLAB_TO_LMS` matrix product
// into LMS (CSS Color 4 algorithm) and continues through LMS conversion.
pub(crate) fn oklab_convert_internal(
    dest: ColorSpace,
    lightness: Option<f64>,
    a: Option<f64>,
    b: Option<f64>,
    alpha: Option<f64>,
    opts: Option<&OklabConvertOpts>,
) -> SassResult<SassColor> {
    let lv = lightness.unwrap_or(0.0);
    let av = a.unwrap_or(0.0);
    let bv = b.unwrap_or(0.0);

    if dest == ColorSpace::Oklch {
        let missing_chroma = opts.is_some_and(|o| o.missing_chroma);
        let missing_hue = opts.is_some_and(|o| o.missing_hue);
        return color_utils::lab_to_lch(dest, lightness, a, b, alpha, missing_chroma, missing_hue);
    }

    // Analogous missingness: null `a`+`b` imply missing chroma+hue and vice
    // versa (#2810). Note the order (nulls first) differs from Lab.
    let (mut a, mut b) = (a, b);
    let mut missing_chroma = opts.is_some_and(|o| o.missing_chroma);
    let mut missing_hue = opts.is_some_and(|o| o.missing_hue);
    if a.is_none() && b.is_none() {
        missing_chroma = true;
        missing_hue = true;
    } else if missing_chroma && missing_hue {
        a = None;
        b = None;
    }

    let missing_lightness = lightness.is_none();
    let missing_a = a.is_none();
    let missing_b = b.is_none();

    let m = &OKLAB_TO_LMS;
    let long_lms = (m[0] * lv) + (m[1] * av) + (m[2] * bv);
    let medium_lms = (m[3] * lv) + (m[4] * av) + (m[5] * bv);
    let short_lms = (m[6] * lv) + (m[7] * av) + (m[8] * bv);
    // Algorithm from https://www.w3.org/TR/css-color-4/#color-conversion-code
    let long = long_lms * long_lms * long_lms + 0.0;
    let medium = medium_lms * medium_lms * medium_lms + 0.0;
    let short = short_lms * short_lms * short_lms + 0.0;

    lms_convert_internal(
        dest,
        Some(long),
        Some(medium),
        Some(short),
        alpha,
        Some(&LmsConvertOpts {
            missing_lightness,
            missing_chroma,
            missing_hue,
            missing_a,
            missing_b,
        }),
    )
}

pub(crate) fn oklab_transformation_matrix(_dest: ColorSpace) -> Option<&'static [f64; 9]> {
    // Matches Dart: `OklabColorSpace` defines no matrix of its own. Every
    // conversion runs through LMS or the dedicated branch above instead.
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
        assert_eq!(ColorSpace::Oklab.name(), "oklab");
        assert!(!ColorSpace::Oklab.is_bounded());
        assert!(!ColorSpace::Oklab.is_legacy());
        assert!(!ColorSpace::Oklab.is_polar());
    }

    #[test]
    fn test_oklab_convert() {
        let c = convert_color(
            ColorSpace::Oklab,
            ColorSpace::Srgb,
            0.5,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "0.388572859");
        assert_eq!(ws(c.channel1), "0.388572859");
        assert_eq!(ws(c.channel2), "0.388572859");
        let c2 = convert_color(
            ColorSpace::Oklab,
            ColorSpace::Lab,
            0.5,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "42");
        assert_eq!(ws(c2.channel1), "0");
        assert_eq!(ws(c2.channel2), "0");
    }
}
