// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color.dart, lib/src/value/color/space.dart (ColorSpace)
// go-source: go/value/color.go

use crate::common::exception::{SassError, SassResult};
use crate::util::number;
use crate::value::color_channel_types::ColorChannel;
use crate::value::color_channel_types::LinearChannel;
use crate::value::color_space_a98_rgb::a98_rgb_transformation_matrix;
use crate::value::color_space_display_p3::display_p3_transformation_matrix;
use crate::value::color_space_display_p3_linear::display_p3_linear_transformation_matrix;
use crate::value::color_space_hsl::hsl_transformation_matrix;
use crate::value::color_space_hwb::hwb_transformation_matrix;
use crate::value::color_space_lab::lab_transformation_matrix;
use crate::value::color_space_lch::lch_transformation_matrix;
use crate::value::color_space_lms::lms_transformation_matrix;
use crate::value::color_space_oklab::oklab_transformation_matrix;
use crate::value::color_space_oklch::oklch_transformation_matrix;
use crate::value::color_space_prophoto_rgb::prophoto_rgb_transformation_matrix;
use crate::value::color_space_rec2020::rec2020_transformation_matrix;
use crate::value::color_space_rgb::rgb_transformation_matrix;
use crate::value::color_space_srgb::srgb_transformation_matrix;
use crate::value::color_space_srgb_linear::srgb_linear_transformation_matrix;
use crate::value::color_space_xyz_d50::xyz_d50_transformation_matrix;
use crate::value::color_space_xyz_d65::xyz_d65_transformation_matrix;
use crate::value::color_utils::a98_from_linear;
use crate::value::color_utils::a98_to_linear;
use crate::value::color_utils::prophoto_from_linear;
use crate::value::color_utils::prophoto_to_linear;
use crate::value::color_utils::rec2020_from_linear;
use crate::value::color_utils::rec2020_to_linear;
use crate::value::color_utils::srgb_and_display_p3_from_linear;
use crate::value::color_utils::srgb_and_display_p3_to_linear;
use crate::value::color_utils::XYZ_CHANNELS;

use crate::serialize::SerializeVisitor;
use crate::value::ValueVisitor;

/// A SassScript color value.
///
/// The meaning of each channel depends on [`space`](Self::space); see
/// [`ColorSpace::channels`] for the channel names in each space.
//
// Matches Dart: `SassColor` (value/color.dart). Dart avoids public fields
// because the JS API overrides same-named getters; Rust uses public fields
// directly. Dart's `channels`/`channelsOrNull` list getters have no
// counterpart here — callers read the fields (plus the `*_or_nil`/`*_missing`
// accessors) directly.
#[derive(Clone, Debug)]
pub struct SassColor {
    /// This color's space.
    pub space: ColorSpace,
    /// This color's first channel (0.0 when missing).
    ///
    /// Use [`channel0_or_nil`](Self::channel0_or_nil) to distinguish a
    /// genuinely missing channel from zero.
    pub channel0: f64,
    /// This color's second channel (0.0 when missing).
    ///
    /// Use [`channel1_or_nil`](Self::channel1_or_nil) to distinguish a
    /// genuinely missing channel from zero.
    pub channel1: f64,
    /// This color's third channel (0.0 when missing).
    ///
    /// Use [`channel2_or_nil`](Self::channel2_or_nil) to distinguish a
    /// genuinely missing channel from zero.
    pub channel2: f64,
    /// This color's alpha channel, between `0` and `1` (0.0 when missing).
    pub alpha: f64,
    /// Bitmask of missing channels: bit 0–2 for the color channels, bit 3
    /// for alpha. A set bit means the stored `0.0` stands in for a
    /// [missing component](https://www.w3.org/TR/css-color-4/#missing).
    pub missing: u8,
    /// The format this color was originally written in, if one is preserved
    /// for expanded-mode serialization.
    pub format: Option<ColorFormat>,
}

/// The source format a color was defined in.
///
/// When a color is serialized in expanded mode it keeps its original format.
#[derive(Clone, Debug, PartialEq)]
pub enum ColorFormat {
    /// A color defined with the `rgb()` or `rgba()` functions.
    RgbFunction,
    /// A color serialized as the exact text it was written with.
    ///
    /// Dart tracks this as a source span to avoid a substring allocation;
    /// the Rust port stores the text itself.
    Preserved(String),
}

/// A SassScript color space.
///
/// Three spaces (`Rgb`, `Hsl`, `Hwb`) are legacy spaces with pre-color-spaces
/// behavior; the rest are modern spaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorSpace {
    Rgb,
    Hsl,
    Hwb,
    Srgb,
    SrgbLinear,
    DisplayP3,
    DisplayP3Linear,
    A98Rgb,
    ProphotoRgb,
    Rec2020,
    XyzD65,
    XyzD50,
    Lab,
    Lch,
    Oklab,
    Oklch,
    Lms,
}

impl ColorSpace {
    /// Whether this is a legacy space (pre-color-spaces syntax with legacy
    /// compatibility behavior).
    pub fn is_legacy(&self) -> bool {
        matches!(self, ColorSpace::Rgb | ColorSpace::Hsl | ColorSpace::Hwb)
    }

    /// Whether colors in this space are bounded (out-of-range channels make
    /// a color out of gamut).
    pub fn is_bounded(&self) -> bool {
        match self {
            ColorSpace::Rgb | ColorSpace::Hsl | ColorSpace::Hwb => true,
            ColorSpace::Srgb | ColorSpace::SrgbLinear => true,
            ColorSpace::DisplayP3 | ColorSpace::DisplayP3Linear => true,
            ColorSpace::A98Rgb => true,
            ColorSpace::ProphotoRgb => true,
            ColorSpace::Rec2020 => true,
            ColorSpace::XyzD65 | ColorSpace::XyzD50 => false,
            ColorSpace::Lab | ColorSpace::Lch => false,
            ColorSpace::Oklab | ColorSpace::Oklch => false,
            ColorSpace::Lms => false,
        }
    }

    /// Whether this space has a polar hue channel (`Hsl`, `Hwb`, `Lch`,
    /// `Oklch`).
    pub fn is_polar(&self) -> bool {
        matches!(
            self,
            ColorSpace::Hsl | ColorSpace::Hwb | ColorSpace::Lch | ColorSpace::Oklch
        )
    }

    /// Converts a channel from this space's transfer curve to linear light.
    ///
    /// Only spaces with a transfer curve support this; polar/lab-like spaces
    /// panic.
    pub fn to_linear(&self, channel: f64) -> f64 {
        match self {
            ColorSpace::Rgb => srgb_and_display_p3_to_linear(channel / 255.0),
            ColorSpace::Hsl
            | ColorSpace::Hwb
            | ColorSpace::Lab
            | ColorSpace::Lch
            | ColorSpace::Oklab
            | ColorSpace::Oklch => panic!("BUG: Color space doesn't support linear conversions"),
            ColorSpace::Srgb | ColorSpace::DisplayP3 => srgb_and_display_p3_to_linear(channel),
            ColorSpace::SrgbLinear
            | ColorSpace::DisplayP3Linear
            | ColorSpace::XyzD65
            | ColorSpace::XyzD50
            | ColorSpace::Lms => channel,
            ColorSpace::A98Rgb => a98_to_linear(channel),
            ColorSpace::ProphotoRgb => prophoto_to_linear(channel),
            ColorSpace::Rec2020 => rec2020_to_linear(channel),
        }
    }

    /// Converts a linear-light channel back to this space's transfer curve.
    pub fn from_linear(&self, channel: f64) -> f64 {
        match self {
            ColorSpace::Rgb => srgb_and_display_p3_from_linear(channel) * 255.0,
            ColorSpace::Hsl
            | ColorSpace::Hwb
            | ColorSpace::Lab
            | ColorSpace::Lch
            | ColorSpace::Oklab
            | ColorSpace::Oklch => panic!("BUG: Color space doesn't support linear conversions"),
            ColorSpace::Srgb | ColorSpace::DisplayP3 => srgb_and_display_p3_from_linear(channel),
            ColorSpace::SrgbLinear
            | ColorSpace::DisplayP3Linear
            | ColorSpace::XyzD65
            | ColorSpace::XyzD50
            | ColorSpace::Lms => channel,
            ColorSpace::A98Rgb => a98_from_linear(channel),
            ColorSpace::ProphotoRgb => prophoto_from_linear(channel),
            ColorSpace::Rec2020 => rec2020_from_linear(channel),
        }
    }

    /// The matrix converting linear-light channels from `self` to `dest`,
    /// or `None` when the pair converts through another space instead.
    pub fn transformation_matrix(&self, dest: ColorSpace) -> Option<&'static [f64; 9]> {
        match self {
            ColorSpace::Rgb => rgb_transformation_matrix(dest),
            ColorSpace::Hsl => hsl_transformation_matrix(dest),
            ColorSpace::Hwb => hwb_transformation_matrix(dest),
            ColorSpace::Srgb => srgb_transformation_matrix(dest),
            ColorSpace::SrgbLinear => srgb_linear_transformation_matrix(dest),
            ColorSpace::DisplayP3 => display_p3_transformation_matrix(dest),
            ColorSpace::DisplayP3Linear => display_p3_linear_transformation_matrix(dest),
            ColorSpace::A98Rgb => a98_rgb_transformation_matrix(dest),
            ColorSpace::ProphotoRgb => prophoto_rgb_transformation_matrix(dest),
            ColorSpace::Rec2020 => rec2020_transformation_matrix(dest),
            ColorSpace::XyzD65 => xyz_d65_transformation_matrix(dest),
            ColorSpace::XyzD50 => xyz_d50_transformation_matrix(dest),
            ColorSpace::Lab => lab_transformation_matrix(dest),
            ColorSpace::Lch => lch_transformation_matrix(dest),
            ColorSpace::Oklab => oklab_transformation_matrix(dest),
            ColorSpace::Oklch => oklch_transformation_matrix(dest),
            ColorSpace::Lms => lms_transformation_matrix(dest),
        }
    }

    /// The full channel descriptors for this space (names, ranges, clamping).
    ///
    /// Legacy `Rgb` keeps the 0–255 clamped descriptors; every other RGB-like
    /// space shares the unclamped 0–1 set.
    pub fn channels_linear(&self) -> &'static [LinearChannel; 3] {
        const LEGACY_RGB_CHANNELS: [LinearChannel; 3] = [
            LinearChannel {
                channel: ColorChannel {
                    name: "red",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: 0.0,
                max: 255.0,
                requires_percent: false,
                lower_clamped: true,
                upper_clamped: true,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "green",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: 0.0,
                max: 255.0,
                requires_percent: false,
                lower_clamped: true,
                upper_clamped: true,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "blue",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: 0.0,
                max: 255.0,
                requires_percent: false,
                lower_clamped: true,
                upper_clamped: true,
            },
        ];
        const RGB_CHANNELS: [LinearChannel; 3] = [
            LinearChannel {
                channel: ColorChannel {
                    name: "red",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: 0.0,
                max: 1.0,
                requires_percent: false,
                lower_clamped: false,
                upper_clamped: false,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "green",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: 0.0,
                max: 1.0,
                requires_percent: false,
                lower_clamped: false,
                upper_clamped: false,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "blue",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: 0.0,
                max: 1.0,
                requires_percent: false,
                lower_clamped: false,
                upper_clamped: false,
            },
        ];
        const HSL_CHANNELS: [LinearChannel; 3] = [
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
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "saturation",
                    is_polar_angle: false,
                    associated_unit: "%",
                },
                min: 0.0,
                max: 100.0,
                requires_percent: true,
                lower_clamped: true,
                upper_clamped: false,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "lightness",
                    is_polar_angle: false,
                    associated_unit: "%",
                },
                min: 0.0,
                max: 100.0,
                requires_percent: true,
                lower_clamped: false,
                upper_clamped: false,
            },
        ];
        const HWB_CHANNELS: [LinearChannel; 3] = [
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
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "whiteness",
                    is_polar_angle: false,
                    associated_unit: "%",
                },
                min: 0.0,
                max: 100.0,
                requires_percent: true,
                lower_clamped: false,
                upper_clamped: false,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "blackness",
                    is_polar_angle: false,
                    associated_unit: "%",
                },
                min: 0.0,
                max: 100.0,
                requires_percent: true,
                lower_clamped: false,
                upper_clamped: false,
            },
        ];
        const LAB_CHANNELS: [LinearChannel; 3] = [
            LinearChannel {
                channel: ColorChannel {
                    name: "lightness",
                    is_polar_angle: false,
                    associated_unit: "%",
                },
                min: 0.0,
                max: 100.0,
                requires_percent: false,
                lower_clamped: true,
                upper_clamped: true,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "a",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: -125.0,
                max: 125.0,
                requires_percent: false,
                lower_clamped: false,
                upper_clamped: false,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "b",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: -125.0,
                max: 125.0,
                requires_percent: false,
                lower_clamped: false,
                upper_clamped: false,
            },
        ];
        const LCH_CHANNELS: [LinearChannel; 3] = [
            LinearChannel {
                channel: ColorChannel {
                    name: "lightness",
                    is_polar_angle: false,
                    associated_unit: "%",
                },
                min: 0.0,
                max: 100.0,
                requires_percent: false,
                lower_clamped: true,
                upper_clamped: true,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "chroma",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: 0.0,
                max: 150.0,
                requires_percent: false,
                lower_clamped: true,
                upper_clamped: false,
            },
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
            },
        ];
        const OKLAB_CHANNELS: [LinearChannel; 3] = [
            LinearChannel {
                channel: ColorChannel {
                    name: "lightness",
                    is_polar_angle: false,
                    associated_unit: "%",
                },
                min: 0.0,
                max: 1.0,
                requires_percent: false,
                lower_clamped: true,
                upper_clamped: true,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "a",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: -0.4,
                max: 0.4,
                requires_percent: false,
                lower_clamped: false,
                upper_clamped: false,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "b",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: -0.4,
                max: 0.4,
                requires_percent: false,
                lower_clamped: false,
                upper_clamped: false,
            },
        ];
        const OKLCH_CHANNELS: [LinearChannel; 3] = [
            LinearChannel {
                channel: ColorChannel {
                    name: "lightness",
                    is_polar_angle: false,
                    associated_unit: "%",
                },
                min: 0.0,
                max: 1.0,
                requires_percent: false,
                lower_clamped: true,
                upper_clamped: true,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "chroma",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: 0.0,
                max: 0.4,
                requires_percent: false,
                lower_clamped: true,
                upper_clamped: false,
            },
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
            },
        ];
        const LMS_CHANNELS: [LinearChannel; 3] = [
            LinearChannel {
                channel: ColorChannel {
                    name: "long",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: 0.0,
                max: 1.0,
                requires_percent: false,
                lower_clamped: false,
                upper_clamped: false,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "medium",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: 0.0,
                max: 1.0,
                requires_percent: false,
                lower_clamped: false,
                upper_clamped: false,
            },
            LinearChannel {
                channel: ColorChannel {
                    name: "short",
                    is_polar_angle: false,
                    associated_unit: "",
                },
                min: 0.0,
                max: 1.0,
                requires_percent: false,
                lower_clamped: false,
                upper_clamped: false,
            },
        ];

        match self {
            ColorSpace::Rgb => &LEGACY_RGB_CHANNELS,
            ColorSpace::Hsl => &HSL_CHANNELS,
            ColorSpace::Hwb => &HWB_CHANNELS,
            ColorSpace::Srgb
            | ColorSpace::SrgbLinear
            | ColorSpace::DisplayP3
            | ColorSpace::DisplayP3Linear
            | ColorSpace::A98Rgb
            | ColorSpace::ProphotoRgb
            | ColorSpace::Rec2020 => &RGB_CHANNELS,
            ColorSpace::XyzD65 | ColorSpace::XyzD50 => &XYZ_CHANNELS,
            ColorSpace::Lab => &LAB_CHANNELS,
            ColorSpace::Lch => &LCH_CHANNELS,
            ColorSpace::Oklab => &OKLAB_CHANNELS,
            ColorSpace::Oklch => &OKLCH_CHANNELS,
            ColorSpace::Lms => &LMS_CHANNELS,
        }
    }

    /// The CSS name of this space (`"srgb-linear"`, `"xyz"` for `XyzD65`, …).
    pub fn name(&self) -> &'static str {
        match self {
            ColorSpace::Rgb => "rgb",
            ColorSpace::Hsl => "hsl",
            ColorSpace::Hwb => "hwb",
            ColorSpace::Srgb => "srgb",
            ColorSpace::SrgbLinear => "srgb-linear",
            ColorSpace::DisplayP3 => "display-p3",
            ColorSpace::DisplayP3Linear => "display-p3-linear",
            ColorSpace::A98Rgb => "a98-rgb",
            ColorSpace::ProphotoRgb => "prophoto-rgb",
            ColorSpace::Rec2020 => "rec2020",
            ColorSpace::XyzD65 => "xyz",
            ColorSpace::XyzD50 => "xyz-d50",
            ColorSpace::Lab => "lab",
            ColorSpace::Lch => "lch",
            ColorSpace::Oklab => "oklab",
            ColorSpace::Oklch => "oklch",
            ColorSpace::Lms => "lms",
        }
    }

    /// The channel names for this space.
    ///
    /// Only the legacy/polar spaces have real names; every other space
    /// reports generic `channel0`–`channel2` here (full descriptors live on
    /// [`channels_linear`](Self::channels_linear)).
    pub fn channels(&self) -> &[&'static str] {
        match self {
            ColorSpace::Rgb => &["red", "green", "blue"],
            ColorSpace::Hsl => &["hue", "saturation", "lightness"],
            ColorSpace::Hwb => &["hue", "whiteness", "blackness"],
            ColorSpace::Lch | ColorSpace::Oklch => &["lightness", "chroma", "hue"],
            _ => &["channel0", "channel1", "channel2"],
        }
    }

    /// Looks up a color space by CSS name (`"xyz"` and `"xyz-d65"` both map
    /// to `XyzD65`).
    ///
    /// `argument_name` names the function argument the name came from, for
    /// error reporting.
    pub fn from_name(name: &str, argument_name: Option<&str>) -> SassResult<ColorSpace> {
        match name.to_lowercase().as_str() {
            "rgb" => Ok(ColorSpace::Rgb),
            "hsl" => Ok(ColorSpace::Hsl),
            "hwb" => Ok(ColorSpace::Hwb),
            "srgb" => Ok(ColorSpace::Srgb),
            "srgb-linear" => Ok(ColorSpace::SrgbLinear),
            "display-p3" => Ok(ColorSpace::DisplayP3),
            "display-p3-linear" => Ok(ColorSpace::DisplayP3Linear),
            "a98-rgb" => Ok(ColorSpace::A98Rgb),
            "prophoto-rgb" => Ok(ColorSpace::ProphotoRgb),
            "rec2020" => Ok(ColorSpace::Rec2020),
            "xyz" | "xyz-d65" => Ok(ColorSpace::XyzD65),
            "xyz-d50" => Ok(ColorSpace::XyzD50),
            "lab" => Ok(ColorSpace::Lab),
            "lch" => Ok(ColorSpace::Lch),
            "oklab" => Ok(ColorSpace::Oklab),
            "oklch" => Ok(ColorSpace::Oklch),
            _ => Err(Box::new(SassError::Script {
                message: format!("Unknown color space \"{name}\"."),
                argument_name: argument_name.map(|s| s.to_string()),
            })),
        }
    }
}

impl SassColor {
    /// Creates a color in the `rgb` space.
    pub fn rgb(red: f64, green: f64, blue: f64, alpha: f64) -> SassColor {
        SassColor {
            space: ColorSpace::Rgb,
            channel0: red,
            channel1: green,
            channel2: blue,
            alpha,
            missing: 0,
            format: None,
        }
    }

    /// Creates a color in the `hsl` space.
    ///
    /// A missing alpha is a [missing component](https://www.w3.org/TR/css-color-4/#missing)
    /// (usually equivalent to transparent); alpha must be in `0..=1`.
    /// A negative saturation shifts the hue 180° and is negated.
    pub fn hsl(hue: f64, saturation: f64, lightness: f64, alpha: f64) -> SassResult<SassColor> {
        let a = number::fuzzy_assert_range(alpha, 0, 1, Some("alpha")).map_err(|e| {
            SassError::Script {
                message: format!("{}: {}", e.name, e.message),
                argument_name: Some("alpha".to_string()),
            }
        })?;
        let invert = number::fuzzy_less_than(saturation, 0.0);
        let h = normalize_hue(hue);
        let h = if invert {
            normalize_hue(hue + 180.0)
        } else {
            h
        };
        let sat = if invert { -saturation } else { saturation };
        SassColor::new_color_for_space_internal(
            ColorSpace::Hsl,
            Some(h),
            Some(sat),
            Some(lightness),
            Some(a),
        )
    }

    /// Creates a color in the `hwb` space.
    ///
    /// A missing alpha is a [missing component](https://www.w3.org/TR/css-color-4/#missing)
    /// (usually equivalent to transparent); alpha must be in `0..=1`.
    pub fn hwb(hue: f64, whiteness: f64, blackness: f64, alpha: f64) -> SassResult<SassColor> {
        let a = number::fuzzy_assert_range(alpha, 0, 1, Some("alpha")).map_err(|e| {
            SassError::Script {
                message: format!("{}: {}", e.name, e.message),
                argument_name: Some("alpha".to_string()),
            }
        })?;
        let h = normalize_hue(hue);
        SassColor::new_color_for_space_internal(
            ColorSpace::Hwb,
            Some(h),
            Some(whiteness),
            Some(blackness),
            Some(a),
        )
    }

    /// Creates a color in the `srgb` space.
    ///
    /// Alpha must be in `0..=1`; `None`-style missing channels are handled
    /// by [`for_space`](Self::for_space)/
    /// [`new_color_for_space_internal`](Self::new_color_for_space_internal).
    pub fn srgb(red: f64, green: f64, blue: f64, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            ColorSpace::Srgb,
            Some(red),
            Some(green),
            Some(blue),
            Some(alpha),
        )
    }

    /// Creates a color in the `srgb-linear` space (alpha must be in `0..=1`).
    pub fn srgb_linear(red: f64, green: f64, blue: f64, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            ColorSpace::SrgbLinear,
            Some(red),
            Some(green),
            Some(blue),
            Some(alpha),
        )
    }

    /// Creates a color in the `display-p3` space (alpha must be in `0..=1`).
    pub fn display_p3(red: f64, green: f64, blue: f64, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            ColorSpace::DisplayP3,
            Some(red),
            Some(green),
            Some(blue),
            Some(alpha),
        )
    }

    /// Creates a color in the `display-p3-linear` space (alpha in `0..=1`).
    pub fn display_p3_linear(red: f64, green: f64, blue: f64, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            ColorSpace::DisplayP3Linear,
            Some(red),
            Some(green),
            Some(blue),
            Some(alpha),
        )
    }

    /// Creates a color in the `a98-rgb` space (alpha must be in `0..=1`).
    pub fn a98_rgb(red: f64, green: f64, blue: f64, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            ColorSpace::A98Rgb,
            Some(red),
            Some(green),
            Some(blue),
            Some(alpha),
        )
    }

    /// Creates a color in the `prophoto-rgb` space (alpha in `0..=1`).
    pub fn prophoto_rgb(red: f64, green: f64, blue: f64, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            ColorSpace::ProphotoRgb,
            Some(red),
            Some(green),
            Some(blue),
            Some(alpha),
        )
    }

    /// Creates a color in the `rec2020` space (alpha must be in `0..=1`).
    pub fn rec2020(red: f64, green: f64, blue: f64, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            ColorSpace::Rec2020,
            Some(red),
            Some(green),
            Some(blue),
            Some(alpha),
        )
    }

    /// Creates a color in the `xyz` (`XyzD65`) space (alpha in `0..=1`).
    pub fn xyz_d65(x: f64, y: f64, z: f64, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            ColorSpace::XyzD65,
            Some(x),
            Some(y),
            Some(z),
            Some(alpha),
        )
    }

    /// Creates a color in the `xyz-d50` space (alpha must be in `0..=1`).
    pub fn xyz_d50(x: f64, y: f64, z: f64, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            ColorSpace::XyzD50,
            Some(x),
            Some(y),
            Some(z),
            Some(alpha),
        )
    }

    /// Creates a color in the `lab` space (alpha must be in `0..=1`).
    pub fn lab(lightness: f64, a: f64, b: f64, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            ColorSpace::Lab,
            Some(lightness),
            Some(a),
            Some(b),
            Some(alpha),
        )
    }

    /// Creates a color in the `lch` space.
    ///
    /// A negative chroma is negated and shifts the hue 180°; alpha must be
    /// in `0..=1`.
    pub fn lch(lightness: f64, chroma: f64, hue: f64, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            ColorSpace::Lch,
            Some(lightness),
            Some(chroma),
            Some(hue),
            Some(alpha),
        )
    }

    /// Creates a color in the `oklab` space (alpha must be in `0..=1`).
    pub fn oklab(lightness: f64, a: f64, b: f64, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            ColorSpace::Oklab,
            Some(lightness),
            Some(a),
            Some(b),
            Some(alpha),
        )
    }

    /// Creates a color in the `oklch` space.
    ///
    /// A negative chroma is negated and shifts the hue 180°; alpha must be
    /// in `0..=1`.
    pub fn oklch(lightness: f64, chroma: f64, hue: f64, alpha: f64) -> SassResult<SassColor> {
        SassColor::new_color_for_space_internal(
            ColorSpace::Oklch,
            Some(lightness),
            Some(chroma),
            Some(hue),
            Some(alpha),
        )
    }

    /// Like [`rgb`](Self::rgb), but also records the source [`ColorFormat`].
    //
    // Matches Dart: `SassColor.rgbInternal` (value/color.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`).
    pub fn rgb_internal(
        red: Option<f64>,
        green: Option<f64>,
        blue: Option<f64>,
        alpha: Option<f64>,
        format: Option<ColorFormat>,
    ) -> SassResult<SassColor> {
        if let Some(a) = alpha {
            number::fuzzy_assert_range(a, 0, 1, Some("alpha")).map_err(|e| SassError::Script {
                message: format!("{}: {}", e.name, e.message),
                argument_name: Some("alpha".to_string()),
            })?;
        }
        Ok(new_color_for_space_no_check_fmt(
            ColorSpace::Rgb,
            red,
            green,
            blue,
            alpha,
            format,
        ))
    }

    /// Creates a color in `space` from raw channels, with explicit per-channel
    /// missing flags (`missing[3]` is alpha).
    ///
    /// Unlike [`new_color_for_space_internal`](Self::new_color_for_space_internal)
    /// this applies no hue/chroma preprocessing.
    pub fn for_space(
        space: ColorSpace,
        channels: [f64; 3],
        alpha: f64,
        missing: [bool; 4],
    ) -> SassColor {
        let mut m = 0u8;
        if missing[0] {
            m |= 1;
        }
        if missing[1] {
            m |= 2;
        }
        if missing[2] {
            m |= 4;
        }
        if missing[3] {
            m |= 8;
        }
        SassColor {
            space,
            channel0: channels[0],
            channel1: channels[1],
            channel2: channels[2],
            alpha,
            missing: m,
            format: None,
        }
    }

    /// Whether this is a legacy color (one in `rgb`, `hsl`, or `hwb`).
    pub fn is_legacy(&self) -> bool {
        self.space.is_legacy()
    }

    /// This color's first channel, or `None` for a missing channel.
    ///
    /// The semantics depend on the color space.
    pub fn channel0_or_nil(&self) -> Option<f64> {
        if self.missing & 1 != 0 {
            None
        } else {
            Some(self.channel0)
        }
    }

    /// This color's second channel, or `None` for a missing channel.
    ///
    /// The semantics depend on the color space.
    pub fn channel1_or_nil(&self) -> Option<f64> {
        if self.missing & 2 != 0 {
            None
        } else {
            Some(self.channel1)
        }
    }

    /// This color's third channel, or `None` for a missing channel.
    ///
    /// The semantics depend on the color space.
    pub fn channel2_or_nil(&self) -> Option<f64> {
        if self.missing & 4 != 0 {
            None
        } else {
            Some(self.channel2)
        }
    }

    /// This color's alpha channel, or `None` for a missing channel.
    pub fn alpha_or_nil(&self) -> Option<f64> {
        if self.missing & 8 != 0 {
            None
        } else {
            Some(self.alpha)
        }
    }

    /// Whether this color's first channel is missing.
    pub fn is_channel0_missing(&self) -> bool {
        self.missing & 1 != 0
    }
    /// Whether this color's second channel is missing.
    pub fn is_channel1_missing(&self) -> bool {
        self.missing & 2 != 0
    }
    /// Whether this color's third channel is missing.
    pub fn is_channel2_missing(&self) -> bool {
        self.missing & 4 != 0
    }
    /// Whether this color's alpha channel is missing.
    pub fn is_alpha_missing(&self) -> bool {
        self.missing & 8 != 0
    }

    /// The hash code for this color.
    ///
    /// Legacy colors hash their `rgb` conversion (missing-aware, so missing
    /// reads as `0.0`); modern colors hash the space name plus the stored
    /// channels.
    pub fn hash_code(&self) -> i32 {
        if self.space.is_legacy() {
            let rgb = self
                .to_space(ColorSpace::Rgb, None)
                .expect("BUG: failed to convert legacy color to RGB for hashing");
            // Matches Dart: hashCode hashes the getters (missing → 0 via
            // `?? 0`), not the stored channel values. RGB conversion with
            // legacyMissing:false already zeroes missing legacy channels, but
            // read through the missing-aware getters for exactness.
            let r = if rgb.is_channel0_missing() {
                0.0
            } else {
                rgb.channel0
            };
            let g = if rgb.is_channel1_missing() {
                0.0
            } else {
                rgb.channel1
            };
            let b = if rgb.is_channel2_missing() {
                0.0
            } else {
                rgb.channel2
            };
            let a = if rgb.is_alpha_missing() {
                0.0
            } else {
                rgb.alpha
            };
            return number::fuzzy_hash_code(r)
                ^ number::fuzzy_hash_code(g)
                ^ number::fuzzy_hash_code(b)
                ^ number::fuzzy_hash_code(a);
        }
        let space_name = self.space.name();
        let mut h: i32 = 0;
        for c in space_name.chars() {
            h = h.wrapping_mul(31).wrapping_add(c as i32);
        }
        h ^ number::fuzzy_hash_code(self.channel0)
            ^ number::fuzzy_hash_code(self.channel1)
            ^ number::fuzzy_hash_code(self.channel2)
            ^ number::fuzzy_hash_code(self.alpha)
    }

    /// Whether `self` equals `other` (fuzzy channel comparison).
    ///
    /// Legacy colors only equal other legacy colors: same-space colors
    /// compare channel-wise (missing-aware), cross-space colors compare via
    /// their `rgb` conversions. Modern colors must share the space.
    pub fn equals(&self, other: &SassColor) -> bool {
        if self.space.is_legacy() {
            if !other.space.is_legacy() {
                return false;
            }
            if !number::fuzzy_equals_nullable(
                self.alpha,
                other.alpha,
                self.is_alpha_missing(),
                other.is_alpha_missing(),
            ) {
                return false;
            }
            if self.space == other.space {
                return number::fuzzy_equals_nullable(
                    self.channel0,
                    other.channel0,
                    self.is_channel0_missing(),
                    other.is_channel0_missing(),
                ) && number::fuzzy_equals_nullable(
                    self.channel1,
                    other.channel1,
                    self.is_channel1_missing(),
                    other.is_channel1_missing(),
                ) && number::fuzzy_equals_nullable(
                    self.channel2,
                    other.channel2,
                    self.is_channel2_missing(),
                    other.is_channel2_missing(),
                );
            }
            if let (Ok(self_rgb), Ok(other_rgb)) = (
                self.to_space(ColorSpace::Rgb, None),
                other.to_space(ColorSpace::Rgb, None),
            ) {
                return number::fuzzy_equals(self_rgb.channel0, other_rgb.channel0)
                    && number::fuzzy_equals(self_rgb.channel1, other_rgb.channel1)
                    && number::fuzzy_equals(self_rgb.channel2, other_rgb.channel2)
                    && number::fuzzy_equals(self_rgb.alpha, other_rgb.alpha);
            }
            return false;
        }
        self.space == other.space
            && number::fuzzy_equals_nullable(
                self.channel0,
                other.channel0,
                self.is_channel0_missing(),
                other.is_channel0_missing(),
            )
            && number::fuzzy_equals_nullable(
                self.channel1,
                other.channel1,
                self.is_channel1_missing(),
                other.is_channel1_missing(),
            )
            && number::fuzzy_equals_nullable(
                self.channel2,
                other.channel2,
                self.is_channel2_missing(),
                other.is_channel2_missing(),
            )
            && number::fuzzy_equals_nullable(
                self.alpha,
                other.alpha,
                self.is_alpha_missing(),
                other.is_alpha_missing(),
            )
    }

    // Legacy getters using to_space + channel_by_name

    /// This color's red channel, between `0` and `255`, rounded.
    ///
    /// Only supported for legacy colors; rounding may be lossy, so prefer
    /// channel access for the true value.
    pub fn red(&self) -> SassResult<f64> {
        let val = self.legacy_channel(ColorSpace::Rgb, "red")?;
        Ok(val.round())
    }
    /// This color's green channel, between `0` and `255`, rounded.
    ///
    /// Only supported for legacy colors; rounding may be lossy.
    pub fn green(&self) -> SassResult<f64> {
        let val = self.legacy_channel(ColorSpace::Rgb, "green")?;
        Ok(val.round())
    }
    /// This color's blue channel, between `0` and `255`, rounded.
    ///
    /// Only supported for legacy colors; rounding may be lossy.
    pub fn blue(&self) -> SassResult<f64> {
        let val = self.legacy_channel(ColorSpace::Rgb, "blue")?;
        Ok(val.round())
    }
    /// This color's hue, between `0` and `360` (legacy colors only).
    pub fn hue(&self) -> SassResult<f64> {
        self.legacy_channel(ColorSpace::Hsl, "hue")
    }
    /// This color's saturation, a percentage between `0` and `100`
    /// (legacy colors only).
    pub fn saturation(&self) -> SassResult<f64> {
        self.legacy_channel(ColorSpace::Hsl, "saturation")
    }
    /// This color's lightness, a percentage between `0` and `100`
    /// (legacy colors only).
    pub fn lightness(&self) -> SassResult<f64> {
        self.legacy_channel(ColorSpace::Hsl, "lightness")
    }
    /// This color's whiteness, a percentage between `0` and `100`
    /// (legacy colors only).
    pub fn whiteness(&self) -> SassResult<f64> {
        self.legacy_channel(ColorSpace::Hwb, "whiteness")
    }
    /// This color's blackness, a percentage between `0` and `100`
    /// (legacy colors only).
    pub fn blackness(&self) -> SassResult<f64> {
        self.legacy_channel(ColorSpace::Hwb, "blackness")
    }

    /// Whether this color's first channel is
    /// [powerless](https://www.w3.org/TR/css-color-4/#powerless).
    ///
    /// Hue is powerless when saturation is zero (`Hsl`) or whiteness plus
    /// blackness reach 100 (`Hwb`); other spaces always return `false`.
    pub fn is_channel0_powerless(&self) -> bool {
        match self.space {
            ColorSpace::Hsl => number::fuzzy_equals(self.channel1, 0.0),
            ColorSpace::Hwb => {
                number::fuzzy_greater_than_or_equals(self.channel1 + self.channel2, 100.0)
            }
            _ => false,
        }
    }
    /// Whether this color's second channel is powerless (always `false`).
    pub fn is_channel1_powerless(&self) -> bool {
        false
    }
    /// Whether this color's third channel is powerless.
    ///
    /// The `Lch`/`Oklch` hue is powerless when chroma is zero; other spaces
    /// always return `false`.
    pub fn is_channel2_powerless(&self) -> bool {
        match self.space {
            ColorSpace::Lch | ColorSpace::Oklch => number::fuzzy_equals(self.channel1, 0.0),
            _ => false,
        }
    }

    /// Whether the named channel in this color is
    /// [powerless](https://www.w3.org/TR/css-color-4/#powerless).
    ///
    /// Errors when the color has no channel with that name; `alpha` is never
    /// powerless.
    pub fn is_channel_powerless_by_name(&self, channel: &str) -> SassResult<bool> {
        let chs = self.space.channels_linear();
        if channel == chs[0].channel.name {
            return Ok(self.is_channel0_powerless());
        }
        if channel == chs[1].channel.name {
            return Ok(self.is_channel1_powerless());
        }
        if channel == chs[2].channel.name {
            return Ok(self.is_channel2_powerless());
        }
        if channel == "alpha" {
            return Ok(false);
        }
        let s = self.to_display_string()?;
        Err(Box::new(SassError::Script {
            message: format!("Color {s} doesn't have a channel named {channel:?}."),
            argument_name: Some("channel".into()),
        }))
    }

    // Converts legacy colors to `space` and reads `channel`; errors for
    // non-legacy colors.
    //
    // Matches Dart: `SassColor._legacyChannel` (value/color.dart).
    fn legacy_channel(&self, space: ColorSpace, channel: &str) -> SassResult<f64> {
        if !self.is_legacy() {
            return Err(Box::new(SassError::Script {
                message: format!("color.{channel}() is only supported for legacy colors. Please use color.channel() instead with an explicit $space argument."),
                argument_name: None,
            }));
        }
        let converted = self.to_space(space, None)?;
        converted.channel_by_name(channel)
    }

    /// Returns a valid CSS representation of `self`.
    ///
    /// Use [`to_display_string`](Self::to_display_string) instead to get a
    /// string representation even if this isn't valid CSS.
    pub fn to_css_string(&self, quote: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(quote, false);
        visitor.visit_color(self)?;
        Ok(visitor.into_string())
    }

    /// Returns a string representation of `self` (equivalent to `inspect()`).
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(true, true);
        visitor.visit_color(self)?;
        Ok(visitor.into_string())
    }

    /// Records the source [`ColorFormat`] used for expanded-mode
    /// serialization.
    pub fn set_format(&mut self, f: Option<ColorFormat>) {
        self.format = f;
    }

    /// Creates a color with preprocessing for specific color spaces.
    ///
    /// `None` indicates a missing channel. Alpha is validated to be in [0, 1].
    ///
    /// Matches Dart: SassColor.forSpaceInternal
    /// Matches Go: NewColorForSpaceInternal
    pub(crate) fn new_color_for_space_internal(
        space: ColorSpace,
        c0: Option<f64>,
        c1: Option<f64>,
        c2: Option<f64>,
        alpha: Option<f64>,
    ) -> SassResult<SassColor> {
        if let Some(a) = alpha {
            number::fuzzy_assert_range(a, 0, 1, Some("alpha")).map_err(|e| SassError::Script {
                message: format!("{}: {}", e.name, e.message),
                argument_name: Some("alpha".to_string()),
            })?;
        }
        Ok(new_color_for_space_internal_no_check(
            space, c0, c1, c2, alpha,
        ))
    }
}

// Builds a color without alpha validation or hue/chroma preprocessing.
//
// Matches Dart: `SassColor._forSpace` (value/color.dart) — the private
// constructor backing `forSpaceInternal`; asserts there (format implies
// `rgb`, never `lms`) are upheld by the callers.
pub(crate) fn new_color_for_space_internal_no_check(
    space: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassColor {
    match space {
        ColorSpace::Hsl => {
            let s = c1.map(|s| {
                if number::fuzzy_less_than(s, 0.0) {
                    -s
                } else {
                    s
                }
            });
            let h = normalize_hue_optional(c0, c1);
            let h_ptr = c0.map(|_| h);
            let s_ptr = c1.map(|_| s.map(number::normalize_linear).unwrap_or(0.0));
            new_color_for_space_no_check(
                space,
                h_ptr,
                s_ptr,
                c2.map(number::normalize_linear),
                alpha.map(number::normalize_linear),
            )
        }
        ColorSpace::Hwb => {
            let h_ptr = c0.map(normalize_hue);
            new_color_for_space_no_check(
                space,
                h_ptr,
                c1.map(number::normalize_linear),
                c2.map(number::normalize_linear),
                alpha.map(number::normalize_linear),
            )
        }
        ColorSpace::Lch | ColorSpace::Oklch => {
            let ch = c1.map(|ch| {
                if number::fuzzy_less_than(ch, 0.0) {
                    -ch
                } else {
                    ch
                }
            });
            let h = normalize_hue_optional(c2, c1);
            let ch_ptr = c1.map(|_| ch.map(number::normalize_linear).unwrap_or(0.0));
            let h_ptr = c2.map(|_| h);
            new_color_for_space_no_check(
                space,
                c0.map(number::normalize_linear),
                ch_ptr,
                h_ptr,
                alpha.map(number::normalize_linear),
            )
        }
        // Dart's `forSpaceInternal` `_` branch normalizes every space except
        // `rgb`, whose `rgbInternal` constructor bypasses preprocessing
        // entirely (raw `_forSpace`).
        ColorSpace::Rgb => new_color_for_space_no_check(space, c0, c1, c2, alpha),
        _ => new_color_for_space_no_check(
            space,
            c0.map(number::normalize_linear),
            c1.map(number::normalize_linear),
            c2.map(number::normalize_linear),
            alpha.map(number::normalize_linear),
        ),
    }
}

// Raw channel packing without hue/chroma preprocessing or alpha validation.
//
// Matches Dart: the tail of `SassColor._forSpace` after `forSpaceInternal`
// preprocessing (value/color.dart).
fn new_color_for_space_no_check(
    space: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassColor {
    new_color_for_space_no_check_fmt(space, c0, c1, c2, alpha, None)
}

fn new_color_for_space_no_check_fmt(
    space: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
    format: Option<ColorFormat>,
) -> SassColor {
    let c0v = c0.unwrap_or(0.0);
    let c1v = c1.unwrap_or(0.0);
    let c2v = c2.unwrap_or(0.0);
    let av = alpha.unwrap_or(0.0);
    let mut missing = 0u8;
    if c0.is_none() {
        missing |= 1;
    }
    if c1.is_none() {
        missing |= 2;
    }
    if c2.is_none() {
        missing |= 4;
    }
    if alpha.is_none() {
        missing |= 8;
    }
    SassColor {
        space,
        channel0: c0v,
        channel1: c1v,
        channel2: c2v,
        alpha: av,
        missing,
        format,
    }
}

// Normalizes a hue to `[0, 360)`.
//
// Matches Dart: `SassColor._normalizeHue` (value/color.dart) — zero and
// non-finite hues (including NaN) normalize to `0`.
fn normalize_hue(h: f64) -> f64 {
    if h == 0.0 || !h.is_finite() {
        0.0
    } else {
        ((h % 360.0) + 360.0) % 360.0
    }
}

// Like [`normalize_hue`], but offsets the hue 180° when a negative
// saturation/chroma made it wrap, and maps a missing hue to `0.0` (the
// caller records the missing bit from the original `Option`). Zero and
// non-finite hues normalize to `0.0`, as in [`normalize_hue`].
//
// Matches Dart: `SassColor._normalizeHue(..., invert: ...)` (value/color.dart).
fn normalize_hue_optional(hue: Option<f64>, other: Option<f64>) -> f64 {
    match hue {
        None => 0.0,
        Some(h) if h == 0.0 || !h.is_finite() => 0.0,
        Some(h) => {
            let offset = match other {
                Some(o) if number::fuzzy_less_than(o, 0.0) => 180.0,
                _ => 0.0,
            };
            ((h % 360.0) + 360.0 + offset) % 360.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{Value, ValueKind};
    use bumpalo::Bump;

    #[test]
    fn test_color_rgb() {
        let c = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        assert_eq!(c.space, ColorSpace::Rgb);
        assert_eq!(c.channel0, 255.0);
        assert!(c.is_legacy());
    }

    #[test]
    fn test_color_srgb() {
        let c = SassColor::for_space(ColorSpace::Srgb, [0.5, 0.5, 0.5], 1.0, [false; 4]);
        assert_eq!(c.space, ColorSpace::Srgb);
        assert!(!c.is_legacy());
    }

    #[test]
    fn test_color_equals_same_space() {
        let c1 = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        let c2 = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        assert!(c1.equals(&c2));
        let c3 = SassColor::rgb(0.0, 0.0, 255.0, 1.0);
        assert!(!c1.equals(&c3));
    }

    #[test]
    fn test_color_equals_different_space() {
        let legacy = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        let modern = SassColor::for_space(ColorSpace::Srgb, [1.0, 0.0, 0.0], 1.0, [false; 4]);
        assert!(
            !legacy.equals(&modern),
            "different spaces should not be equal"
        );
    }

    #[test]
    fn test_color_equals_missing() {
        let c1 = SassColor::for_space(
            ColorSpace::Rgb,
            [0.0, 0.0, 0.0],
            1.0,
            [true, false, false, false],
        );
        let c2 = SassColor::for_space(
            ColorSpace::Rgb,
            [99.0, 0.0, 0.0],
            1.0,
            [true, false, false, false],
        );
        assert!(
            c1.equals(&c2),
            "both missing channel0 should be equal regardless of stored value"
        );
    }

    #[test]
    fn test_color_missing_channels() {
        let c = SassColor::for_space(
            ColorSpace::Rgb,
            [0.0, 0.0, 0.0],
            1.0,
            [true, false, false, false],
        );
        assert!(c.is_channel0_missing());
        assert_eq!(c.channel0_or_nil(), None);
        assert!(!c.is_channel1_missing());
        assert!(c.channel1_or_nil().is_some());
    }

    #[test]
    fn test_color_alpha_missing() {
        let c = SassColor::for_space(
            ColorSpace::Rgb,
            [1.0, 1.0, 1.0],
            0.0,
            [false, false, false, true],
        );
        assert!(c.is_alpha_missing());
        assert_eq!(c.alpha_or_nil(), None);
    }

    #[test]
    fn test_color_hash_code() {
        let c1 = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        let c2 = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        assert_eq!(c1.hash_code(), c2.hash_code());
    }

    #[test]
    fn test_color_space_is_legacy() {
        assert!(ColorSpace::Rgb.is_legacy());
        assert!(ColorSpace::Hsl.is_legacy());
        assert!(ColorSpace::Hwb.is_legacy());
        assert!(!ColorSpace::Srgb.is_legacy());
        assert!(!ColorSpace::Lab.is_legacy());
    }

    #[test]
    fn test_color_to_css_string_legacy() {
        let arena = Bump::new();
        let c = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        let v = Value::new_with_arena(&arena, ValueKind::Color(c));
        assert_eq!(v.to_css_string(true).unwrap(), "red");
    }

    #[test]
    fn test_color_to_css_string_modern() {
        let arena = Bump::new();
        let c = SassColor::for_space(ColorSpace::Srgb, [0.5, 0.5, 0.5], 1.0, [false; 4]);
        let v = Value::new_with_arena(&arena, ValueKind::Color(c));
        assert_eq!(v.to_css_string(true).unwrap(), "color(srgb 0.5 0.5 0.5)");
    }

    #[test]
    fn test_color_to_string() {
        let arena = Bump::new();
        let c = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        let v = Value::new_with_arena(&arena, ValueKind::Color(c));
        assert_eq!(v.to_display_string().unwrap(), "red");
    }

    // Degenerate colors (#2840): `forSpaceInternal` normalizes `NaN` and
    // negative zero linear channels (and alpha) to `0`, and non-finite or
    // zero hues to `0`.
    #[test]
    fn test_for_space_internal_normalizes_degenerate() {
        let c = SassColor::new_color_for_space_internal(
            ColorSpace::Srgb,
            Some(f64::NAN),
            Some(-0.0),
            Some(0.5),
            Some(-0.0),
        )
        .unwrap();
        assert_eq!(c.channel0.to_bits(), 0.0f64.to_bits());
        assert_eq!(c.channel1.to_bits(), 0.0f64.to_bits());
        assert_eq!(c.channel2, 0.5);
        assert_eq!(c.alpha.to_bits(), 0.0f64.to_bits());

        let h = SassColor::new_color_for_space_internal(
            ColorSpace::Hsl,
            Some(f64::INFINITY),
            Some(50.0),
            Some(50.0),
            Some(1.0),
        )
        .unwrap();
        assert_eq!(h.channel0, 0.0);

        // The `rgb` space bypasses preprocessing (Dart `rgbInternal`).
        let r =
            SassColor::rgb_internal(Some(f64::NAN), Some(0.0), Some(0.0), Some(1.0), None).unwrap();
        assert!(r.channel0.is_nan());
    }
}
