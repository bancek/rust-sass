// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color.dart (interpolate, _interpolateHues)
// go-source: go/value/color_interpolation.go

use crate::common::exception::{SassError, SassResult};
use crate::util::number;
use crate::value::color::{new_color_for_space_internal_no_check, ColorSpace, SassColor};
use crate::value::interpolation_method::{HueInterpolationMethod, InterpolationMethod};

impl SassColor {
    /// Returns a color partway between `self` and `other` following the CSS
    /// Color 4 [color interpolation] procedure.
    ///
    /// [color interpolation]: https://www.w3.org/TR/css-color-4/#interpolation
    ///
    /// `weight` (default `0.5`) is how much of `self` the result holds and
    /// must be in `0..=1`. A channel missing in one color (or analogous to a
    /// missing channel in the pre-conversion color) takes the other color's
    /// value; when `legacy_missing` is `false`, missing legacy channels that
    /// survive conversion become zero.
    pub fn interpolate(
        &self,
        other: &SassColor,
        method: &InterpolationMethod,
        legacy_missing: bool,
        weight: Option<f64>,
    ) -> SassResult<SassColor> {
        interpolate_colors(self, other, method, legacy_missing, weight)
    }
}

fn interpolate_colors(
    c: &SassColor,
    other: &SassColor,
    method: &InterpolationMethod,
    legacy_missing: bool,
    weight: Option<f64>,
) -> SassResult<SassColor> {
    let w = weight.unwrap_or(0.5);

    if number::fuzzy_equals(w, 0.0) {
        return Ok(other.clone());
    }
    if number::fuzzy_equals(w, 1.0) {
        return Ok(c.clone());
    }

    if !(0.0..=1.0).contains(&w) {
        return Err(Box::new(SassError::Script {
            message: format!("Invalid value: Not in inclusive range 0..1: {w}"),
            argument_name: Some("weight".into()),
        }));
    }

    let color1 = c.to_space(method.space, None)?;
    let color2 = other.to_space(method.space, None)?;

    // A channel missing in one color (or analogous to a missing channel —
    // conversions now propagate analogous missingness themselves, #2810)
    // takes the other color's value.
    let ch1 = [
        color1.channel0_or_nil().or(color2.channel0_or_nil()),
        color1.channel1_or_nil().or(color2.channel1_or_nil()),
        color1.channel2_or_nil().or(color2.channel2_or_nil()),
    ];
    let ch2 = [
        color2.channel0_or_nil().or(color1.channel0_or_nil()),
        color2.channel1_or_nil().or(color1.channel1_or_nil()),
        color2.channel2_or_nil().or(color1.channel2_or_nil()),
    ];

    let alpha1 = if c.is_alpha_missing() {
        other.alpha
    } else {
        c.alpha
    };
    let alpha2 = if other.is_alpha_missing() {
        c.alpha
    } else {
        other.alpha
    };

    let this_mult = if !c.is_alpha_missing() {
        c.alpha * w
    } else {
        w
    };
    let other_mult = if !other.is_alpha_missing() {
        other.alpha * (1.0 - w)
    } else {
        1.0 - w
    };

    let mixed_alpha: Option<f64> = if c.is_alpha_missing() && other.is_alpha_missing() {
        None
    } else {
        Some(alpha1 * w + alpha2 * (1.0 - w))
    };

    let ma_val = mixed_alpha.unwrap_or(1.0);

    // `ch2[i]` is always `Some` when `ch1[i]` is (both fall back to the
    // same present value), so a missing `ch1[i]` means both sides miss.
    let mixed = [0, 1, 2].map(|i| match (ch1[i], ch2[i]) {
        (Some(a), Some(b)) => Some((a * this_mult + b * other_mult) / ma_val),
        _ => None,
    });

    let result = match method.space {
        ColorSpace::Hsl | ColorSpace::Hwb => {
            let h_val = match (ch1[0], ch2[0]) {
                (Some(h1), Some(h2)) => Some(interpolate_hues(
                    h1,
                    h2,
                    method.hue.unwrap_or(HueInterpolationMethod::Shorter),
                    w,
                )),
                _ => None,
            };
            new_color_for_space_internal_no_check(
                method.space,
                h_val,
                mixed[1],
                mixed[2],
                mixed_alpha,
            )
        }
        ColorSpace::Lch | ColorSpace::Oklch => {
            // Matches Dart: the interpolated hue goes through
            // `forSpaceInternal`, which normalizes it to [0, 360).
            let h_val = match (ch1[2], ch2[2]) {
                (Some(h1), Some(h2)) => Some(interpolate_hues(
                    h1,
                    h2,
                    method.hue.unwrap_or(HueInterpolationMethod::Shorter),
                    w,
                )),
                _ => None,
            };
            new_color_for_space_internal_no_check(
                method.space,
                mixed[0],
                mixed[1],
                h_val,
                mixed_alpha,
            )
        }
        _ => new_color_for_space_internal_no_check(
            method.space,
            mixed[0],
            mixed[1],
            mixed[2],
            mixed_alpha,
        ),
    };

    result.to_space(c.space, Some(legacy_missing))
}

// Returns a hue partway between `hue1` and `hue2` per `method`
// (algorithms from https://www.w3.org/TR/css-color-4/#hue-interpolation);
// `weight` is how much of `hue1` the result holds.
//
// Matches Dart: `SassColor._interpolateHues` (value/color.dart).
fn interpolate_hues(hue1: f64, hue2: f64, method: HueInterpolationMethod, weight: f64) -> f64 {
    let mut h1 = hue1;
    let mut h2 = hue2;
    match method {
        HueInterpolationMethod::Shorter => {
            let diff = h2 - h1;
            if diff > 180.0 {
                h1 += 360.0;
            } else if diff < -180.0 {
                h2 += 360.0;
            }
        }
        HueInterpolationMethod::Longer => {
            let diff = h2 - h1;
            if diff > 0.0 && diff < 180.0 {
                h2 += 360.0;
            } else if diff > -180.0 && diff <= 0.0 {
                h1 += 360.0;
            }
        }
        HueInterpolationMethod::Increasing => {
            if h2 < h1 {
                h2 += 360.0;
            }
        }
        HueInterpolationMethod::Decreasing => {
            if h1 < h2 {
                h1 += 360.0;
            }
        }
    }
    h1 * weight + h2 * (1.0 - weight)
}

#[cfg(test)]
mod tests {
    use super::super::interpolation_method::InterpolationMethod;
    use super::*;
    use crate::util::number;

    fn ws(v: f64) -> String {
        number::write_number_to_string(v)
    }

    #[test]
    fn test_interpolate_identity() {
        let c1 = SassColor::for_space(ColorSpace::Srgb, [0.5, 0.5, 0.5], 1.0, [false; 4]);
        let c2 = c1.clone();
        let method = InterpolationMethod::new(ColorSpace::Srgb, None).unwrap();
        let result = c1.interpolate(&c2, &method, false, Some(0.5)).unwrap();
        assert_eq!(result.space, ColorSpace::Srgb);
        assert_eq!(ws(result.channel0), "0.5");
        assert_eq!(ws(result.channel1), "0.5");
        assert_eq!(ws(result.channel2), "0.5");
        assert_eq!(ws(result.alpha), "1");
    }

    #[test]
    fn test_interpolate_weight_zero() {
        let c1 = SassColor::for_space(ColorSpace::Srgb, [1.0, 0.0, 0.0], 1.0, [false; 4]);
        let c2 = SassColor::for_space(ColorSpace::Srgb, [0.0, 0.0, 1.0], 1.0, [false; 4]);
        let method = InterpolationMethod::new(ColorSpace::Srgb, None).unwrap();
        let result = c1.interpolate(&c2, &method, false, Some(0.0)).unwrap();
        assert_eq!(ws(result.channel0), "0");
        assert_eq!(ws(result.channel1), "0");
        assert_eq!(ws(result.channel2), "1");
    }

    #[test]
    fn test_interpolate_weight_one() {
        let c1 = SassColor::for_space(ColorSpace::Srgb, [1.0, 0.0, 0.0], 1.0, [false; 4]);
        let c2 = SassColor::for_space(ColorSpace::Srgb, [0.0, 0.0, 1.0], 1.0, [false; 4]);
        let method = InterpolationMethod::new(ColorSpace::Srgb, None).unwrap();
        let result = c1.interpolate(&c2, &method, false, Some(1.0)).unwrap();
        assert_eq!(ws(result.channel0), "1");
        assert_eq!(ws(result.channel1), "0");
        assert_eq!(ws(result.channel2), "0");
    }

    // Missing channels take the other color's value; missing on both sides
    // stays missing (#2810: conversions propagate analogous missingness,
    // so interpolation only checks direct missingness).
    #[test]
    fn test_interpolate_missing_channels() {
        let present = SassColor::for_space(ColorSpace::Lab, [60.0, 10.0, 20.0], 1.0, [false; 4]);
        let missing = SassColor::for_space(
            ColorSpace::Lab,
            [50.0, 0.0, 0.0],
            1.0,
            [false, true, true, false],
        );
        let method = InterpolationMethod::new(ColorSpace::Lab, None).unwrap();
        let result = missing
            .interpolate(&present, &method, false, Some(0.5))
            .unwrap();
        assert!(!result.is_channel1_missing());
        assert!(!result.is_channel2_missing());

        let both = missing
            .interpolate(&missing, &method, false, Some(0.5))
            .unwrap();
        assert!(both.is_channel1_missing());
        assert!(both.is_channel2_missing());
    }

    #[test]
    fn test_lch_oklch_hue_normalized() {
        // Dart routes the interpolated hue through `forSpaceInternal`,
        // normalizing to [0, 360): shorter-mix of 350+10 gives 0, not 360.
        for space in [ColorSpace::Oklch, ColorSpace::Lch] {
            let a = SassColor::for_space(space, [0.5, 0.1, 350.0], 1.0, [false; 4]);
            let b = SassColor::for_space(space, [0.5, 0.1, 10.0], 1.0, [false; 4]);
            let method = InterpolationMethod::new(space, None).unwrap();
            let result = a.interpolate(&b, &method, false, Some(0.5)).unwrap();
            assert_eq!(
                ws(result.channel2),
                "0",
                "hue in {space:?} must normalize 360 to 0"
            );
        }
    }
}
