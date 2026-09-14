// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color.dart (SassColor.toSpace) — the to_rgb,
//   to_hsl, to_hwb helpers have no named Dart counterpart; they convert to
//   the legacy rgb/hsl/hwb space and unpack the channels.
// go-source: go/value/color_channel.go

use crate::common::exception::SassResult;
use crate::value::color::{ColorSpace, SassColor};
use crate::value::color_conversions::convert_color;

impl SassColor {
    /// Converts this color to `space`.
    ///
    /// Identity conversions return the color unchanged. With
    /// `legacy_missing: Some(false)`, missing channels in a legacy result are
    /// filled with zeros instead of staying missing.
    pub fn to_space(
        &self,
        space: ColorSpace,
        legacy_missing: Option<bool>,
    ) -> SassResult<SassColor> {
        let lm = legacy_missing.unwrap_or(true);
        if self.space == space {
            return Ok(self.clone());
        }

        let converted = convert_color(
            self.space,
            space,
            self.channel0,
            self.channel1,
            self.channel2,
            self.alpha,
            [
                self.is_channel0_missing(),
                self.is_channel1_missing(),
                self.is_channel2_missing(),
                self.is_alpha_missing(),
            ],
            None,
        )?;

        if !lm && converted.is_legacy() && converted.has_missing_channel() {
            let c0 = converted.channel0;
            let c1 = converted.channel1;
            let c2 = converted.channel2;
            let a = converted.alpha;
            let result = SassColor::new_color_for_space_internal(
                converted.space,
                Some(c0),
                Some(c1),
                Some(c2),
                Some(a),
            );
            if let Ok(r) = result {
                return Ok(r);
            }
        }
        Ok(converted)
    }

    /// Returns the color's channels in the legacy RGB space (0-255).
    pub fn to_rgb(&self) -> SassResult<(f64, f64, f64, f64)> {
        let cc = self.to_space(ColorSpace::Rgb, None)?;
        Ok((cc.channel0, cc.channel1, cc.channel2, cc.alpha))
    }

    /// Returns the color's channels in the legacy HSL space.
    pub fn to_hsl(&self) -> SassResult<(f64, f64, f64, f64)> {
        let cc = self.to_space(ColorSpace::Hsl, None)?;
        Ok((cc.channel0, cc.channel1, cc.channel2, cc.alpha))
    }

    /// Returns the color's channels in the legacy HWB space.
    pub fn to_hwb(&self) -> SassResult<(f64, f64, f64, f64)> {
        let cc = self.to_space(ColorSpace::Hwb, None)?;
        Ok((cc.channel0, cc.channel1, cc.channel2, cc.alpha))
    }

    // Whether any channel (or alpha) is missing.
    //
    // Matches Dart: `SassColor.hasMissingChannel` (@internal).
    pub(crate) fn has_missing_channel(&self) -> bool {
        self.missing != 0
    }
}

#[cfg(test)]
mod tests {
    use super::super::color::{ColorSpace, SassColor};
    use crate::util::number;

    fn ws(v: f64) -> String {
        number::write_number_to_string(v)
    }

    #[test]
    fn test_to_space_identity() {
        let c = SassColor::rgb(128.0, 64.0, 0.0, 1.0);
        let result = c.to_space(ColorSpace::Rgb, None).unwrap();
        assert_eq!(c.channel0, result.channel0);
    }

    #[test]
    fn test_to_space_conversion() {
        let c = SassColor::for_space(ColorSpace::Srgb, [0.5, 0.5, 0.5], 1.0, [false; 4]);
        let result = c.to_space(ColorSpace::Lab, None).unwrap();
        assert_eq!(result.space, ColorSpace::Lab);
        assert_eq!(ws(result.channel0), "53.3889647411");
        assert_eq!(ws(result.channel1), "0");
        assert_eq!(ws(result.channel2), "0");
        assert_eq!(ws(result.alpha), "1");
    }

    #[test]
    fn test_to_space_missing_channels() {
        let mut c = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        c.missing |= 2;
        let result = c.to_space(ColorSpace::Lab, None).unwrap();
        assert_eq!(result.space, ColorSpace::Lab);
        assert_eq!(ws(result.channel0), "54.2905414047");
        assert_eq!(ws(result.channel1), "80.8049281704");
        assert_eq!(ws(result.channel2), "69.8909647686");
    }

    #[test]
    fn test_to_space_legacy_missing() {
        let mut c = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        c.missing |= 2;
        let result = c.to_space(ColorSpace::Hsl, Some(false)).unwrap();
        assert_eq!(result.space, ColorSpace::Hsl);
        assert_eq!(ws(result.channel0), "0");
        assert_eq!(ws(result.channel1), "100");
        assert_eq!(ws(result.channel2), "50");
    }

    #[test]
    fn test_to_rgb() {
        let c = SassColor::rgb(255.0, 128.0, 0.0, 1.0);
        let (r, g, b, a) = c.to_rgb().unwrap();
        assert_eq!(ws(r), "255");
        assert_eq!(ws(g), "128");
        assert_eq!(ws(b), "0");
        assert_eq!(ws(a), "1");
    }

    #[test]
    fn test_to_hsl() {
        let c = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        let (h, s, l, a) = c.to_hsl().unwrap();
        assert_eq!(ws(h), "0");
        assert_eq!(ws(s), "100");
        assert_eq!(ws(l), "50");
        assert_eq!(ws(a), "1");
    }

    #[test]
    fn test_to_hwb() {
        let c = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        let (h, w, bl, a) = c.to_hwb().unwrap();
        assert_eq!(ws(h), "0");
        assert_eq!(ws(w), "0");
        assert_eq!(ws(bl), "0");
        assert_eq!(ws(a), "1");
    }
}
