// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/hwb.dart
// go-source: go/value/color_space_hwb.go

use crate::common::exception::SassResult;
use crate::value::color::new_color_for_space_internal_no_check;
use crate::value::color::{ColorSpace, SassColor};
use crate::value::color_conversions::SrgbConvertOpts;
use crate::value::color_space_srgb::srgb_convert_internal;
use crate::value::color_utils::hue_to_rgb;

// The legacy HWB space: `hue` plus `whiteness`/`blackness` amounts that mix
// the pure-hue sRGB color toward white or black. (Dart: `HwbColorSpace`,
// internal.)
//
// The channel layout (bounded, legacy, polar) is declared once on
// [`ColorSpace`]; see `channels_linear` in `value/color.rs`.

// Converts an HWB color to `dest` via the CSS Color 4 HWB-to-RGB algorithm:
// normalizes whiteness/blackness when their sum exceeds 1, mixes each
// `hue_to_rgb` primary toward white by `factor`, and delegates to sRGB
// conversion with only the hue-missing flag forwarded.
pub(crate) fn hwb_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    // Analogous missingness (#2810): whiteness/blackness are not analogous
    // to any channels, so handle a whiteness+blackness-missing source
    // manually rather than piping those flags everywhere.
    if c1.is_none() && c2.is_none() {
        if c0.is_none() {
            return Ok(new_color_for_space_internal_no_check(
                dest, None, None, None, alpha,
            ));
        }
        let converted = hwb_convert(dest, c0, Some(0.0), Some(0.0), alpha)?;
        return Ok(match dest {
            ColorSpace::Hsl => new_color_for_space_internal_no_check(
                dest,
                Some(converted.channel0),
                None,
                None,
                Some(converted.alpha),
            ),
            ColorSpace::Lch | ColorSpace::Oklch => new_color_for_space_internal_no_check(
                dest,
                None,
                None,
                Some(converted.channel2),
                Some(converted.alpha),
            ),
            _ => converted,
        });
    }

    // From https://www.w3.org/TR/css-color-4/#hwb-to-rgb
    let scaled_hue = c0.map(|v| (v % 360.0) / 360.0).unwrap_or(0.0);
    let mut scaled_whiteness = c1.map(|v| v / 100.0).unwrap_or(0.0);
    let mut scaled_blackness = c2.map(|v| v / 100.0).unwrap_or(0.0);

    let sum = scaled_whiteness + scaled_blackness;
    if sum > 1.0 {
        scaled_whiteness /= sum;
        scaled_blackness /= sum;
    }

    let factor = 1.0 - scaled_whiteness - scaled_blackness;
    // FMA prevention: `as f64` forces rounding before the addition
    // (see critical-invariants.md); clippy sees a no-op cast.
    #[allow(clippy::unnecessary_cast)]
    let to_rgb =
        |hue: f64| -> f64 { ((hue_to_rgb(0.0, 1.0, hue) * factor) as f64) + scaled_whiteness };

    let rv = to_rgb(scaled_hue + 1.0 / 3.0);
    let gv = to_rgb(scaled_hue);
    let bv = to_rgb(scaled_hue - 1.0 / 3.0);

    srgb_convert_internal(
        dest,
        Some(rv),
        Some(gv),
        Some(bv),
        alpha,
        Some(&SrgbConvertOpts {
            missing_lightness: false,
            missing_chroma: false,
            missing_hue: c0.is_none(),
        }),
    )
}

pub(crate) fn hwb_transformation_matrix(_dest: ColorSpace) -> Option<&'static [f64; 9]> {
    // Matches Dart: `HwbColorSpace` defines no matrix. Like HSL, HWB
    // converts via sRGB (`hwb_convert`) rather than a linear transform.
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
        assert_eq!(ColorSpace::Hwb.name(), "hwb");
        assert!(ColorSpace::Hwb.is_bounded());
        assert!(ColorSpace::Hwb.is_legacy());
        assert!(ColorSpace::Hwb.is_polar());
    }

    #[test]
    fn test_hwb_convert() {
        let c = convert_color(
            ColorSpace::Hwb,
            ColorSpace::Rgb,
            240.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "0");
        assert_eq!(ws(c.channel1), "0");
        assert_eq!(ws(c.channel2), "255");
        let c2 = convert_color(
            ColorSpace::Hwb,
            ColorSpace::Srgb,
            240.0,
            0.0,
            0.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "0");
        assert_eq!(ws(c2.channel1), "0");
        assert_eq!(ws(c2.channel2), "1");
    }
}
