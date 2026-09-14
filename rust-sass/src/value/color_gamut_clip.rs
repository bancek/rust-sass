// Copyright 2024 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/gamut_map_method/clip.dart
// go-source: go/value/color_gamut_clip.go

use crate::util::number;
use crate::value::color::SassColor;
use crate::value::color_conversions_base::space_channels;

// Maps `c` into gamut by clamping each channel to its valid range.
//
// Matches Dart: `ClipGamutMap.map`; the per-channel helper
// `ClipGamutMap._clampChannel` is inlined in the loop below. Missing and
// polar-angle channels pass through untouched, as do colors in unbounded
// spaces.
pub(crate) fn clip_gamut_map(c: &SassColor) -> SassColor {
    let chs = space_channels(c.space);
    let mut new_ch = [c.channel0, c.channel1, c.channel2];
    let new_missing = c.missing;

    for i in 0..3 {
        let ch = &chs[i];
        if (c.missing & (1 << i)) != 0 || ch.channel.is_polar_angle {
            continue;
        }
        if c.space.is_bounded() {
            new_ch[i] = number::clamp_like_css(new_ch[i], ch.min, ch.max);
        }
    }

    SassColor {
        space: c.space,
        channel0: new_ch[0],
        channel1: new_ch[1],
        channel2: new_ch[2],
        alpha: c.alpha,
        missing: new_missing,
        format: c.format.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::color::ColorSpace;
    use super::*;
    use crate::util::number;
    fn ws(v: f64) -> String {
        number::write_number_to_string(v)
    }

    #[test]
    fn test_clip_gamut() {
        let c = SassColor::for_space(ColorSpace::Srgb, [1.5, -0.5, 0.5], 1.0, [false; 4]);
        let result = clip_gamut_map(&c);
        assert_eq!(ws(result.channel0), "1");
        assert_eq!(ws(result.channel1), "0");
        assert_eq!(ws(result.channel2), "0.5");
        let c = SassColor::for_space(ColorSpace::Srgb, [0.5, 0.5, 0.5], 1.0, [false; 4]);
        let result = clip_gamut_map(&c);
        assert_eq!(ws(result.channel0), "0.5");
        assert_eq!(ws(result.channel1), "0.5");
        assert_eq!(ws(result.channel2), "0.5");
    }
}
