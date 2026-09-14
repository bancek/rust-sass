// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/channel.dart
// go-source: go/value/color_channel_types.go

/// Metadata about a single channel in a known color space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorChannel {
    /// The channel's name.
    pub name: &'static str,
    /// Whether this is a polar-angle channel: an angle in degrees around a
    /// circle. True exactly for non-linear channels such as hue.
    pub is_polar_angle: bool,
    /// The unit used when the value is serialized or returned from a Sass
    /// function. Any compatible unit works for input, except where
    /// [`LinearChannel::requires_percent`] forbids unitless values.
    pub associated_unit: &'static str,
}

impl ColorChannel {
    // Internal constructor; matches Dart's `@internal` const constructor.
    pub fn new(name: &'static str, is_polar_angle: bool, associated_unit: &'static str) -> Self {
        ColorChannel {
            name,
            is_polar_angle,
            associated_unit,
        }
    }

    /// Whether this channel is analogous to `other` in the CSS Color 4
    /// interpolation-missing sense: `red`/`x`, `green`/`y`, `blue`/`z`,
    /// `chroma`/`saturation` pairs plus the self-analogous `lightness` and
    /// `hue`.
    ///
    /// Matches Dart: ColorChannel.isAnalogous
    pub fn is_analogous(&self, other: ColorChannel) -> bool {
        analogous_names(self.name, other.name)
    }
}

fn analogous_names(a: &str, b: &str) -> bool {
    (a == "red" || a == "x") && (b == "red" || b == "x")
        || (a == "green" || a == "y") && (b == "green" || b == "y")
        || (a == "blue" || a == "z") && (b == "blue" || b == "z")
        || (a == "chroma" || a == "saturation") && (b == "chroma" || b == "saturation")
        || a == "lightness" && b == "lightness"
        || a == "hue" && b == "hue"
}

/// Metadata about a color channel with a linear (as opposed to polar) value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearChannel {
    pub channel: ColorChannel,
    /// The channel's minimum value: the percentage reference point and the
    /// in-gamut boundary, not a hard clamp unless the space is bounded.
    pub min: f64,
    /// The channel's maximum value: the percentage reference point and the
    /// in-gamut boundary, not a hard clamp unless the space is bounded.
    pub max: f64,
    /// Whether values must carry `%`, forbidding unitless input.
    pub requires_percent: bool,
    /// Whether the lower bound is clamped for colors built with the global
    /// function syntax.
    pub lower_clamped: bool,
    /// Whether the upper bound is clamped for colors built with the global
    /// function syntax.
    pub upper_clamped: bool,
}

// Defaults `associated_unit` to `%` exactly for a 0-100 range; the
// `ConventionallyPercent`/`NoPercent` options override either way.
//
// Matches Dart: the `conventionallyPercent` parameter of `LinearChannel`.
fn percent_if(min: f64, max: f64) -> &'static str {
    if min == 0.0 && max == 100.0 {
        "%"
    } else {
        ""
    }
}

impl LinearChannel {
    // Internal constructor; matches Dart's `@internal` const constructor.
    pub fn new(
        name: &'static str,
        min: f64,
        max: f64,
        opts: &[LinearChannelOption],
    ) -> LinearChannel {
        let mut ch = LinearChannel {
            channel: ColorChannel {
                name,
                is_polar_angle: false,
                associated_unit: percent_if(min, max),
            },
            min,
            max,
            requires_percent: false,
            lower_clamped: false,
            upper_clamped: false,
        };
        for opt in opts {
            opt.apply(&mut ch);
        }
        ch
    }
}

pub enum LinearChannelOption {
    /// Values must carry `%`; unitless input is an error.
    RequiresPercent,
    /// Clamp the lower bound for the global function syntax.
    LowerClamped,
    /// Clamp the upper bound for the global function syntax.
    UpperClamped,
    /// Force `associated_unit` to `%` regardless of range.
    ConventionallyPercent,
    /// Force no associated unit regardless of range.
    NoPercent,
}

impl LinearChannelOption {
    fn apply(&self, ch: &mut LinearChannel) {
        match self {
            LinearChannelOption::RequiresPercent => ch.requires_percent = true,
            LinearChannelOption::LowerClamped => ch.lower_clamped = true,
            LinearChannelOption::UpperClamped => ch.upper_clamped = true,
            LinearChannelOption::ConventionallyPercent => ch.channel.associated_unit = "%",
            LinearChannelOption::NoPercent => ch.channel.associated_unit = "",
        }
    }
}

/// The alpha channel shared across all colors.
pub const ALPHA_CHANNEL: LinearChannel = LinearChannel {
    channel: ColorChannel {
        name: "alpha",
        is_polar_angle: false,
        associated_unit: "",
    },
    min: 0.0,
    max: 1.0,
    requires_percent: false,
    lower_clamped: false,
    upper_clamped: false,
};

/// The hue channel shared across all polar color spaces.
///
/// Matches Dart: `hueChannel` in `space/utils.dart`.
pub fn hue_channel_info() -> LinearChannel {
    LinearChannel {
        channel: ColorChannel {
            name: "hue",
            is_polar_angle: true,
            associated_unit: "deg",
        },
        min: 0.0,
        max: 0.0,
        requires_percent: false,
        lower_clamped: false,
        upper_clamped: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_color_channel() {
        let ch = ColorChannel::new("lightness", false, "%");
        assert_eq!(ch.name, "lightness");
        assert!(!ch.is_polar_angle);
        assert_eq!(ch.associated_unit, "%");
    }

    #[test]
    fn test_new_color_channel_polar_angle() {
        let ch = ColorChannel::new("hue", true, "deg");
        assert!(ch.is_polar_angle);
        assert_eq!(ch.associated_unit, "deg");
    }

    #[test]
    fn test_color_channel_is_analogous_red() {
        let red = ColorChannel::new("red", false, "");
        let x = ColorChannel::new("x", false, "");
        assert!(red.is_analogous(x));
        assert!(x.is_analogous(red));
        assert!(red.is_analogous(red));
        assert!(x.is_analogous(x));
    }

    #[test]
    fn test_color_channel_is_analogous_green() {
        let green = ColorChannel::new("green", false, "");
        let y = ColorChannel::new("y", false, "");
        assert!(green.is_analogous(y));
        assert!(y.is_analogous(green));
    }

    #[test]
    fn test_color_channel_is_analogous_blue() {
        let blue = ColorChannel::new("blue", false, "");
        let z = ColorChannel::new("z", false, "");
        assert!(blue.is_analogous(z));
        assert!(z.is_analogous(blue));
    }

    #[test]
    fn test_color_channel_is_analogous_chroma_saturation() {
        let chroma = ColorChannel::new("chroma", false, "");
        let saturation = ColorChannel::new("saturation", false, "");
        assert!(chroma.is_analogous(saturation));
        assert!(saturation.is_analogous(chroma));
        assert!(chroma.is_analogous(chroma));
    }

    #[test]
    fn test_color_channel_is_analogous_lightness() {
        let lightness = ColorChannel::new("lightness", false, "%");
        let other = ColorChannel::new("lightness", false, "");
        assert!(lightness.is_analogous(other));
    }

    #[test]
    fn test_color_channel_is_analogous_hue() {
        let hue = ColorChannel::new("hue", true, "deg");
        let other = ColorChannel::new("hue", true, "");
        assert!(hue.is_analogous(other));
    }

    #[test]
    fn test_color_channel_is_analogous_not_analogous() {
        let red = ColorChannel::new("red", false, "");
        let green = ColorChannel::new("green", false, "");
        assert!(!red.is_analogous(green));
        let lightness = ColorChannel::new("lightness", false, "%");
        let hue = ColorChannel::new("hue", true, "deg");
        assert!(!lightness.is_analogous(hue));
        let chroma = ColorChannel::new("chroma", false, "");
        let red2 = ColorChannel::new("red", false, "");
        assert!(!chroma.is_analogous(red2));
    }

    #[test]
    fn test_new_linear_channel() {
        let ch = LinearChannel::new("red", 0.0, 1.0, &[]);
        assert_eq!(ch.channel.name, "red");
        assert_eq!(ch.min, 0.0);
        assert_eq!(ch.max, 1.0);
        assert!(!ch.channel.is_polar_angle);
        assert!(!ch.requires_percent);
        assert!(!ch.lower_clamped);
        assert!(!ch.upper_clamped);
    }

    #[test]
    fn test_new_linear_channel_percent_auto() {
        let ch = LinearChannel::new("lightness", 0.0, 100.0, &[]);
        assert_eq!(ch.channel.associated_unit, "%");
    }

    #[test]
    fn test_new_linear_channel_no_percent_auto() {
        let ch = LinearChannel::new("red", 0.0, 1.0, &[]);
        assert_eq!(ch.channel.associated_unit, "");
    }

    #[test]
    fn test_new_linear_channel_with_options() {
        let ch = LinearChannel::new(
            "lightness",
            0.0,
            100.0,
            &[
                LinearChannelOption::RequiresPercent,
                LinearChannelOption::LowerClamped,
                LinearChannelOption::UpperClamped,
            ],
        );
        assert!(ch.requires_percent);
        assert!(ch.lower_clamped);
        assert!(ch.upper_clamped);
    }

    #[test]
    fn test_new_linear_channel_with_conventionally_percent() {
        let ch = LinearChannel::new("red", 0.0, 1.0, &[]);
        assert_eq!(ch.channel.associated_unit, "");
        let ch2 = LinearChannel::new(
            "red",
            0.0,
            1.0,
            &[LinearChannelOption::ConventionallyPercent],
        );
        assert_eq!(ch2.channel.associated_unit, "%");
    }

    #[test]
    fn test_new_linear_channel_with_no_percent() {
        let ch = LinearChannel::new("lightness", 0.0, 100.0, &[LinearChannelOption::NoPercent]);
        assert_eq!(ch.channel.associated_unit, "");
    }

    // Locks the `ALPHA_CHANNEL` constant definition (fails if the const
    // body is edited), not a runtime value — hence a constant assertion.
    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn test_alpha_channel() {
        assert_eq!(ALPHA_CHANNEL.channel.name, "alpha");
        assert_eq!(ALPHA_CHANNEL.min, 0.0);
        assert_eq!(ALPHA_CHANNEL.max, 1.0);
        assert!(!ALPHA_CHANNEL.channel.is_polar_angle);
    }

    #[test]
    fn test_hue_channel_info() {
        let ch = hue_channel_info();
        assert_eq!(ch.channel.name, "hue");
        assert!(ch.channel.is_polar_angle);
        assert_eq!(ch.channel.associated_unit, "deg");
        assert!(!ch.requires_percent);
        assert!(!ch.lower_clamped);
        assert!(!ch.upper_clamped);
    }

    #[test]
    fn test_linear_channel_requires_percent_option() {
        let ch = LinearChannel::new("red", 0.0, 1.0, &[LinearChannelOption::RequiresPercent]);
        assert!(ch.requires_percent);
        assert!(!ch.lower_clamped);
        assert!(!ch.upper_clamped);
    }

    #[test]
    fn test_linear_channel_lower_clamped_option() {
        let ch = LinearChannel::new("red", 0.0, 1.0, &[LinearChannelOption::LowerClamped]);
        assert!(ch.lower_clamped);
    }

    #[test]
    fn test_linear_channel_upper_clamped_option() {
        let ch = LinearChannel::new("red", 0.0, 1.0, &[LinearChannelOption::UpperClamped]);
        assert!(ch.upper_clamped);
    }

    #[test]
    fn test_linear_channel_all_option_combo() {
        let ch = LinearChannel::new(
            "lightness",
            0.0,
            100.0,
            &[
                LinearChannelOption::RequiresPercent,
                LinearChannelOption::LowerClamped,
                LinearChannelOption::UpperClamped,
                LinearChannelOption::NoPercent,
            ],
        );
        assert!(ch.requires_percent);
        assert!(ch.lower_clamped);
        assert!(ch.upper_clamped);
        assert_eq!(ch.channel.associated_unit, "");
    }
}
