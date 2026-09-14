// Copyright 2024 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/gamut_map_method/local_minde.dart
// go-source: go/value/color_gamut_local_minde.go

use crate::common::exception::SassResult;
use crate::util::number;
use crate::value::color::{new_color_for_space_internal_no_check, ColorSpace, SassColor};
use crate::value::color_conversions::convert_color;
use crate::value::color_conversions_base::GamutMapMethod;

// Missing channels default to zero inside the search; matches Dart's
// `?? 0` on the Oklch lightness/hue/alpha.
fn deref_or_zero(p: Option<f64>) -> f64 {
    p.unwrap_or(0.0)
}

// The deltaEOK color-difference measure: Euclidean distance in Oklab.
//
// Matches Dart: `LocalMindeGamutMap._deltaEOK`, from the Color Level 4
// color-difference definition.
fn delta_eok(c1: &SassColor, c2: &SassColor) -> SassResult<f64> {
    let lab1 = c1.to_space(ColorSpace::Oklab, None)?;
    let lab2 = c2.to_space(ColorSpace::Oklab, None)?;
    let d0 = lab1.channel0 - lab2.channel0;
    let d1 = lab1.channel1 - lab2.channel1;
    let d2 = lab1.channel2 - lab2.channel2;
    Ok((d0 * d0 + d1 * d1 + d2 * d2).sqrt())
}

// Maps `color` into gamut with the deltaEOK difference formula and the
// local-MINDE improvement.
//
// Matches Dart: `LocalMindeGamutMap.map`, following the CSS Color 4
// gamut-mapping algorithm: white/black fast paths at the lightness extremes
// (legacy colors go through RGB white so they keep legacy semantics),
// an early return when clipping is already close enough, then a binary search
// on Oklch chroma. `current` is built directly in the target space since every
// use but the chroma step converts it there first. The `min_in_gamut` guard
// intentionally skips the in-gamut check once a below-threshold clip has been
// seen, per the csswg discussion Dart links — `min_in_gamut = false` implies
// the candidate is out of gamut.
pub(crate) fn local_minde_gamut_map(color: &SassColor) -> SassResult<SassColor> {
    // Thresholds from the gamut-mapping algorithm.
    const JND: f64 = 0.02;
    const EPSILON: f64 = 0.0001;

    let origin_oklch = color.to_space(ColorSpace::Oklch, None)?;

    let lightness = if (origin_oklch.missing & 1) == 0 {
        Some(origin_oklch.channel0)
    } else {
        None
    };
    let hue = if (origin_oklch.missing & 4) == 0 {
        Some(origin_oklch.channel2)
    } else {
        None
    };
    let alpha = if (origin_oklch.missing & 8) == 0 {
        Some(origin_oklch.alpha)
    } else {
        None
    };
    let alpha_or_null = if (color.missing & 8) == 0 {
        Some(color.alpha)
    } else {
        None
    };

    if number::fuzzy_greater_than_or_equals(deref_or_zero(lightness), 1.0) {
        if color.is_legacy() {
            let c = new_color_for_space_internal_no_check(
                ColorSpace::Rgb,
                Some(255.0),
                Some(255.0),
                Some(255.0),
                alpha_or_null,
            );
            return c.to_space(color.space, None);
        }
        let one = Some(1.0);
        return Ok(new_color_for_space_internal_no_check(
            color.space,
            one,
            one,
            one,
            alpha_or_null,
        ));
    } else if number::fuzzy_less_than_or_equals(deref_or_zero(lightness), 0.0) {
        let c = new_color_for_space_internal_no_check(
            ColorSpace::Rgb,
            Some(0.0),
            Some(0.0),
            Some(0.0),
            alpha_or_null,
        );
        return c.to_space(color.space, None);
    }

    let mut clipped = color.to_gamut(GamutMapMethod::Clip)?;
    if delta_eok(&clipped, color)? < JND {
        return Ok(clipped);
    }

    let mut min = 0.0;
    let mut max = origin_oklch.channel1;
    let mut min_in_gamut = true;

    while max - min > EPSILON {
        let chroma = (min + max) / 2.0;

        let current = convert_color(
            ColorSpace::Oklch,
            color.space,
            deref_or_zero(lightness),
            chroma,
            deref_or_zero(hue),
            deref_or_zero(alpha),
            [
                (origin_oklch.missing & 1) != 0,
                false,
                (origin_oklch.missing & 4) != 0,
                (origin_oklch.missing & 8) != 0,
            ],
            None,
        )?;

        if min_in_gamut && current.is_in_gamut() {
            min = chroma;
            continue;
        }

        clipped = current.to_gamut(GamutMapMethod::Clip)?;
        let e = delta_eok(&clipped, &current)?;
        if e < JND {
            if JND - e < EPSILON {
                return Ok(clipped);
            }
            min_in_gamut = false;
            min = chroma;
        } else {
            max = chroma;
        }
    }
    Ok(clipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::number;
    fn ws(v: f64) -> String {
        number::write_number_to_string(v)
    }

    #[test]
    fn test_delta_eok_zero() {
        let c = SassColor::rgb(50.0, 60.0, 70.0, 1.0);
        let de = delta_eok(&c, &c).unwrap();
        assert!((de - 0.0).abs() < 1e-12);
    }

    #[test]
    fn test_local_minde() {
        let c = SassColor::for_space(ColorSpace::Srgb, [1.5, 0.1, 0.1], 1.0, [false; 4]);
        let result = local_minde_gamut_map(&c).unwrap();
        assert_eq!(ws(result.channel0), "1");
        assert_eq!(ws(result.channel1), "0.7237814013");
        assert_eq!(ws(result.channel2), "0.6738285316");

        let c = SassColor::for_space(ColorSpace::Srgb, [2.0, -0.5, 0.0], 1.0, [false; 4]);
        let result = local_minde_gamut_map(&c).unwrap();
        assert_eq!(ws(result.channel0), "1");
        assert_eq!(ws(result.channel1), "1");
        assert_eq!(ws(result.channel2), "1");
    }

    #[test]
    fn test_delta_eok_golden() {
        let c1 = SassColor::for_space(ColorSpace::Srgb, [1.0, 0.0, 0.0], 1.0, [false; 4]);
        let c2 = SassColor::for_space(ColorSpace::Srgb, [0.9, 0.0, 0.0], 1.0, [false; 4]);
        let de = delta_eok(&c1, &c2).unwrap();
        assert_eq!(ws(de), "0.0519781045");
    }
}
