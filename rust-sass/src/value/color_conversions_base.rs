// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color.dart (isInGamut, toGamut, channel,
//   isChannelMissing, toSpace, changeRgb/Hsl/Hwb/Alpha/Channels)
// go-source: go/value/color_conversions_base.go

use crate::common::exception::{SassError, SassResult};
use crate::util::number;
use crate::value::color::{ColorSpace, SassColor};
use crate::value::color_channel_types::{ColorChannel, LinearChannel, ALPHA_CHANNEL};
use crate::value::color_gamut_clip::clip_gamut_map;
use crate::value::color_gamut_local_minde::local_minde_gamut_map;
use std::collections::HashMap;

// Returns the channel descriptor at `channel`, or the shared alpha channel
// for out-of-range indices (3, the alpha position).
#[allow(dead_code)]
pub(crate) fn channel_info(space: ColorSpace, channel: i32) -> ColorChannel {
    let chs = space.channels_linear();
    if (0..3).contains(&channel) {
        chs[channel as usize].channel
    } else {
        ALPHA_CHANNEL.channel
    }
}

// Returns the three channel descriptors for `space`.
pub(crate) fn space_channels(space: ColorSpace) -> &'static [LinearChannel; 3] {
    space.channels_linear()
}

// Returns the three channel names for `space`.
#[allow(dead_code)]
pub(crate) fn space_channel_names(space: ColorSpace) -> [&'static str; 3] {
    let chs = space.channels_linear();
    [
        chs[0].channel.name,
        chs[1].channel.name,
        chs[2].channel.name,
    ]
}

impl SassColor {
    /// Whether this color is in-gamut for its color space.
    ///
    /// Unbounded spaces are always in gamut; polar-angle channels never count
    /// as out of gamut (no bounded space currently pairs the two).
    pub fn is_in_gamut(&self) -> bool {
        if !self.space.is_bounded() {
            return true;
        }
        let chs = self.space.channels_linear();
        is_channel_in_gamut(self.channel0, &chs[0])
            && is_channel_in_gamut(self.channel1, &chs[1])
            && is_channel_in_gamut(self.channel2, &chs[2])
    }

    /// Returns a copy of this color that's in-gamut in its current space.
    ///
    /// In-gamut colors return unchanged; out-of-gamut colors are mapped with
    /// the given method.
    pub fn to_gamut(&self, method: GamutMapMethod) -> SassResult<SassColor> {
        if self.is_in_gamut() {
            return Ok(self.clone());
        }
        match method {
            GamutMapMethod::Clip => Ok(clip_gamut_map(self)),
            GamutMapMethod::LocalMinde => local_minde_gamut_map(self),
        }
    }

    /// Returns a copy of this color with the alpha channel set to `alpha`.
    pub fn change_alpha(&self, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            self.space,
            Some(self.channel0),
            Some(self.channel1),
            Some(self.channel2),
            Some(alpha),
        )
    }

    /// Changes one or more RGB channels and returns the result.
    ///
    /// Legacy colors only; kept for the deprecated global-function path.
    /// Unspecified channels keep their current values.
    pub fn change_rgb(
        &self,
        red: Option<i32>,
        green: Option<i32>,
        blue: Option<i32>,
        alpha: Option<f64>,
    ) -> SassResult<SassColor> {
        if !self.is_legacy() {
            return Err(Box::new(SassError::Script {
                message: "color.changeRgb() is only supported for legacy colors. Please use color.changeChannels() instead with an explicit $space argument.".into(),
                argument_name: None,
            }));
        }
        let a = alpha.unwrap_or(self.alpha);
        let r = match red {
            Some(v) => v as f64,
            None => self.channel_by_name("red")?,
        };
        let g = match green {
            Some(v) => v as f64,
            None => self.channel_by_name("green")?,
        };
        let b = match blue {
            Some(v) => v as f64,
            None => self.channel_by_name("blue")?,
        };
        SassColor::new_color_for_space_internal(ColorSpace::Rgb, Some(r), Some(g), Some(b), Some(a))
    }

    /// Changes one or more HSL channels and returns the result, converted
    /// back into this color's original space.
    ///
    /// Legacy colors only; unspecified channels keep their current values.
    pub fn change_hsl(
        &self,
        hue: Option<f64>,
        saturation: Option<f64>,
        lightness: Option<f64>,
        alpha: Option<f64>,
    ) -> SassResult<SassColor> {
        if !self.is_legacy() {
            return Err(Box::new(SassError::Script {
                message: "color.changeHsl() is only supported for legacy colors. Please use color.changeChannels() instead with an explicit $space argument.".into(),
                argument_name: None,
            }));
        }
        let h = match hue {
            Some(v) => v,
            None => self.hue()?,
        };
        let s = match saturation {
            Some(v) => v,
            None => self.saturation()?,
        };
        let l = match lightness {
            Some(v) => v,
            None => self.lightness()?,
        };
        let a = alpha.unwrap_or(self.alpha);

        let result = SassColor::new_color_for_space_internal(
            ColorSpace::Hsl,
            Some(h),
            Some(s),
            Some(l),
            Some(a),
        )?;
        result.to_space(self.space, None)
    }

    /// Changes one or more HWB channels and returns the result, converted
    /// back into this color's original space.
    ///
    /// Legacy colors only; unspecified channels keep their current values.
    pub fn change_hwb(
        &self,
        hue: Option<f64>,
        whiteness: Option<f64>,
        blackness: Option<f64>,
        alpha: Option<f64>,
    ) -> SassResult<SassColor> {
        if !self.is_legacy() {
            return Err(Box::new(SassError::Script {
                // Matches Dart verbatim, typo included: changeHwb throws
                // "color.changeHsl() is only supported...".
                message: "color.changeHsl() is only supported for legacy colors. Please use color.changeChannels() instead with an explicit $space argument.".into(),
                argument_name: None,
            }));
        }
        let h = match hue {
            Some(v) => v,
            None => self.hue()?,
        };
        let w = match whiteness {
            Some(v) => v,
            None => self.whiteness()?,
        };
        let b = match blackness {
            Some(v) => v,
            None => self.blackness()?,
        };
        let a = alpha.unwrap_or(self.alpha) + 0.0;

        let result = SassColor::new_color_for_space_internal(
            ColorSpace::Hwb,
            Some(h),
            Some(w),
            Some(b),
            Some(a),
        )?;
        result.to_space(self.space, None)
    }

    /// Changes one or more channels by name and returns the result.
    ///
    /// With `space`, converts there first, applies the changes, then converts
    /// back. Errors on unknown channel names and on channels set twice.
    pub fn change_channels(
        &self,
        new_values: &HashMap<String, f64>,
        space: Option<&ColorSpace>,
    ) -> SassResult<SassColor> {
        change_channels_impl(self, new_values, None, space)
    }

    /// Returns the value of `channel`, or an error if this color has no
    /// channel by that name.
    ///
    /// Matches Dart: `SassColor.channel`, including the trailing-dot message
    /// and the channel-name error attribution (callers needing a different
    /// message build their own error instead of reaching this fallback).
    pub fn channel_by_name(&self, channel: &str) -> SassResult<f64> {
        let chs = self.space.channels_linear();
        if channel == chs[0].channel.name {
            return Ok(self.channel0);
        }
        if channel == chs[1].channel.name {
            return Ok(self.channel1);
        }
        if channel == chs[2].channel.name {
            return Ok(self.channel2);
        }
        if channel == "alpha" {
            return Ok(self.alpha);
        }
        let s = self.to_display_string()?;
        Err(Box::new(SassError::Script {
            // Matches Dart: channel() throws `doesn't have a channel named
            // "$channel".` with the channelName attribution (the callers that
            // surface a different message, e.g. color.channel(), build their
            // own error and never reach this fallback).
            message: format!("Color {s} doesn't have a channel named {channel:?}."),
            argument_name: None,
        }))
    }

    /// Whether `channel` is missing in this color (`none` in CSS terms).
    pub fn is_channel_missing_by_name(&self, channel: &str) -> SassResult<bool> {
        let chs = self.space.channels_linear();
        if channel == chs[0].channel.name {
            return Ok(self.missing & 1 != 0);
        }
        if channel == chs[1].channel.name {
            return Ok(self.missing & 2 != 0);
        }
        if channel == chs[2].channel.name {
            return Ok(self.missing & 4 != 0);
        }
        if channel == "alpha" {
            return Ok(self.missing & 8 != 0);
        }
        let s = self.to_display_string()?;
        Err(Box::new(SassError::Script {
            message: format!("Color {s} doesn't have a channel named {channel:?}."),
            argument_name: Some("channel".into()),
        }))
    }
}

// Whether `value` is in-gamut for `ch`.
//
// Matches Dart: `SassColor._isChannelInGamut` — polar-angle channels are
// always in gamut; linear channels compare with fuzzy bounds.
fn is_channel_in_gamut(value: f64, ch: &LinearChannel) -> bool {
    if ch.channel.is_polar_angle {
        return true;
    }
    number::fuzzy_less_than_or_equals(value, ch.max)
        && number::fuzzy_greater_than_or_equals(value, ch.min)
}

// Shared `change_channels` implementation.
//
// Matches Dart: `SassColor.changeChannels` — an empty map returns the color
// unchanged; with `space`, converts there, applies, and converts back.
// Explicitly set channels win; the rest carry over (missing stays missing).
// Dart's `colorName` duplicate-channel attribution is not threaded here; the
// errors carry no argument name.
fn change_channels_impl(
    c: &SassColor,
    new_values: &HashMap<String, f64>,
    _color_name: Option<&str>,
    space: Option<&ColorSpace>,
) -> SassResult<SassColor> {
    if new_values.is_empty() {
        return Ok(c.clone());
    }

    if let Some(sp) = space {
        if *sp != c.space {
            let space_converted = c.to_space(*sp, None)?;
            let converted = change_channels_impl(&space_converted, new_values, None, space)?;
            return converted.to_space(c.space, None);
        }
    }

    let chs = c.space.channels_linear();
    let mut new0: Option<f64> = None;
    let mut new1: Option<f64> = None;
    let mut new2: Option<f64> = None;
    let mut new_alpha: Option<f64> = None;

    for (name, val) in new_values {
        if name == chs[0].channel.name {
            if new0.is_some() {
                return Err(Box::new(SassError::Script {
                    message: format!(
                        "Multiple values supplied for {:?}: {:?} and {:?}",
                        chs[0].channel.name, new0, val
                    ),
                    argument_name: None,
                }));
            }
            new0 = Some(*val);
        } else if name == chs[1].channel.name {
            if new1.is_some() {
                return Err(Box::new(SassError::Script {
                    message: format!(
                        "Multiple values supplied for {:?}: {:?} and {:?}",
                        chs[1].channel.name, new1, val
                    ),
                    argument_name: None,
                }));
            }
            new1 = Some(*val);
        } else if name == chs[2].channel.name {
            if new2.is_some() {
                return Err(Box::new(SassError::Script {
                    message: format!(
                        "Multiple values supplied for {:?}: {:?} and {:?}",
                        chs[2].channel.name, new2, val
                    ),
                    argument_name: None,
                }));
            }
            new2 = Some(*val);
        } else if name == "alpha" {
            if new_alpha.is_some() {
                return Err(Box::new(SassError::Script {
                    message: format!(
                        "Multiple values supplied for alpha: {:?} and {:?}",
                        new_alpha, val
                    ),
                    argument_name: None,
                }));
            }
            new_alpha = Some(*val);
        } else {
            let s = c.to_display_string()?;
            return Err(Box::new(SassError::Script {
                message: format!("Color {s} doesn't have a channel named {name:?}"),
                argument_name: None,
            }));
        }
    }

    let c0 = new0.or(if c.missing & 1 == 0 {
        Some(c.channel0)
    } else {
        None
    });
    let c1 = new1.or(if c.missing & 2 == 0 {
        Some(c.channel1)
    } else {
        None
    });
    let c2 = new2.or(if c.missing & 4 == 0 {
        Some(c.channel2)
    } else {
        None
    });
    let a = new_alpha.or(if c.missing & 8 == 0 {
        Some(c.alpha)
    } else {
        None
    });

    SassColor::new_color_for_space_internal(c.space, c0, c1, c2, a)
}

/// Algorithms that map an out-of-gamut color into its space's gamut.
///
/// Matches Dart: `GamutMapMethod` (`gamut_map_method.dart`); the re-export in
/// `color_gamut.rs` is the module-level entry point. Unlike Dart's class
/// hierarchy, the two methods live here as enum variants because the Rust
/// call sites (`to_gamut`, `local_minde_gamut_map`) only need dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GamutMapMethod {
    /// Clamps each out-of-gamut channel to its channel bounds.
    ///
    /// Fast but visually crude; useful where clipping is the expected
    /// behavior.
    Clip,
    /// Maps in Oklch using the deltaEOK difference formula with the
    /// local-MINDE refinement, per the original Color Level 4 candidate
    /// recommendation.
    LocalMinde,
}

impl GamutMapMethod {
    pub fn from_name(name: &str) -> SassResult<GamutMapMethod> {
        Self::from_name_with_arg(name, None)
    }

    /// Parses a method from its Sass name (`"clip"`, `"local-minde"`).
    ///
    /// Matches Dart: `GamutMapMethod.fromName`, whose optional `argumentName`
    /// attributes unknown-method errors to the caller's argument.
    pub fn from_name_with_arg(
        name: &str,
        argument_name: Option<&str>,
    ) -> SassResult<GamutMapMethod> {
        match name {
            "clip" => Ok(GamutMapMethod::Clip),
            "local-minde" => Ok(GamutMapMethod::LocalMinde),
            _ => Err(Box::new(SassError::Script {
                message: format!("Unknown gamut map method {name:?}."),
                argument_name: argument_name.map(str::to_string),
            })),
        }
    }

    /// The Sass name of the method.
    pub fn as_str(&self) -> &'static str {
        match self {
            GamutMapMethod::Clip => "clip",
            GamutMapMethod::LocalMinde => "local-minde",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_is_legacy() {
        let c = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        assert!(c.is_legacy());
        let srgb = SassColor::for_space(ColorSpace::Srgb, [0.5, 0.5, 0.5], 1.0, [false; 4]);
        assert!(!srgb.is_legacy());
    }

    #[test]
    fn test_is_in_gamut() {
        let c = SassColor::for_space(ColorSpace::Srgb, [0.5, 0.5, 0.5], 1.0, [false; 4]);
        assert!(c.is_in_gamut());
        let lab = SassColor::for_space(ColorSpace::Lab, [50.0, 0.0, 0.0], 1.0, [false; 4]);
        assert!(lab.is_in_gamut());
    }

    #[test]
    fn test_has_missing_channel() {
        let c = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        assert!(!c.has_missing_channel());
        let miss = SassColor::for_space(
            ColorSpace::Rgb,
            [0.0, 0.0, 0.0],
            1.0,
            [true, false, false, false],
        );
        assert!(miss.has_missing_channel());
    }

    #[test]
    fn test_channel_by_name_trailing_dot() {
        // Matches Dart: channel() throws `doesn't have a channel named
        // "$channel".` (trailing dot, channelName attribution).
        let c = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        let err = c.channel_by_name("bogus").unwrap_err();
        match *err {
            SassError::Script { message, .. } => {
                assert!(message.ends_with('.'), "{message}");
                assert!(message.contains("\"bogus\""), "{message}");
            }
            other => panic!("expected Script, got {other:?}"),
        }
    }

    #[test]
    fn test_change_hwb_keeps_dart_typo() {
        // Matches Dart verbatim, typo included: changeHwb throws
        // "color.changeHsl() is only supported...".
        let c = SassColor::for_space(ColorSpace::Oklch, [0.5, 0.1, 10.0], 1.0, [false; 4]);
        let err = c.change_hwb(None, None, None, None).unwrap_err();
        match *err {
            SassError::Script { message, .. } => {
                assert!(message.contains("color.changeHsl()"), "{message}");
            }
            other => panic!("expected Script, got {other:?}"),
        }
    }

    #[test]
    fn test_change_alpha() {
        let c = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        let c2 = c.change_alpha(0.5).unwrap();
        assert!((c2.alpha - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_change_channels() {
        let c = SassColor::for_space(ColorSpace::Lab, [50.0, 25.0, -25.0], 1.0, [false; 4]);
        let mut vals = HashMap::new();
        vals.insert("lightness".to_string(), 75.0);
        let result = c.change_channels(&vals, None).unwrap();
        assert!((result.channel0 - 75.0).abs() < 1e-9);
    }

    #[test]
    fn test_channel_info() {
        let ch = channel_info(ColorSpace::Srgb, 0);
        assert_eq!(ch.name, "red");
        let alpha_ch = channel_info(ColorSpace::Srgb, 3);
        assert_eq!(alpha_ch.name, "alpha");
    }
}
