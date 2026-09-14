// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/interpolation_method.dart
// go-source: go/value/interpolation_method.go

use crate::value::ListSeparator;
use bumpalo::Bump;

use crate::common::exception::{SassError, SassResult};
use crate::value;
use crate::value::color::ColorSpace;
use crate::value::ValueKind;

/// The method by which two hues are adjusted when interpolating between colors.
///
/// Used by [`InterpolationMethod`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HueInterpolationMethod {
    /// Angles are adjusted so that `θ₂ - θ₁ ∈ [-180, 180]`.
    ///
    /// <https://www.w3.org/TR/css-color-4/#shorter>
    Shorter,
    /// Angles are adjusted so that `θ₂ - θ₁ ∈ {0, [180, 360)}`.
    ///
    /// <https://www.w3.org/TR/css-color-4/#hue-longer>
    Longer,
    /// Angles are adjusted so that `θ₂ - θ₁ ∈ [0, 360)`.
    ///
    /// <https://www.w3.org/TR/css-color-4/#hue-increasing>
    Increasing,
    /// Angles are adjusted so that `θ₂ - θ₁ ∈ (-360, 0]`.
    ///
    /// <https://www.w3.org/TR/css-color-4/#hue-decreasing>
    Decreasing,
}

impl HueInterpolationMethod {
    /// The CSS keyword for this hue method (`"shorter"`, `"longer"`, …).
    pub fn as_str(&self) -> &'static str {
        match self {
            HueInterpolationMethod::Shorter => "shorter",
            HueInterpolationMethod::Longer => "longer",
            HueInterpolationMethod::Increasing => "increasing",
            HueInterpolationMethod::Decreasing => "decreasing",
        }
    }

    // Matches Dart: `HueInterpolationMethod._fromValue`
    // (value/color/interpolation_method.dart) — case-insensitive keyword
    // match, failing on unknown methods.
    fn from_str(s: &str) -> Option<HueInterpolationMethod> {
        match s.to_lowercase().as_str() {
            "shorter" => Some(HueInterpolationMethod::Shorter),
            "longer" => Some(HueInterpolationMethod::Longer),
            "increasing" => Some(HueInterpolationMethod::Increasing),
            "decreasing" => Some(HueInterpolationMethod::Decreasing),
            _ => None,
        }
    }
}

/// The method by which two colors are interpolated to find a color in the
/// middle.
///
/// Used by `SassColor::interpolate`.
#[derive(Clone, Debug)]
pub struct InterpolationMethod {
    /// The color space in which to perform the interpolation.
    pub space: ColorSpace,
    /// How to interpolate the hues between two colors.
    ///
    /// This is `None` if and only if [`space`](Self::space) isn't a polar
    /// color space.
    pub hue: Option<HueInterpolationMethod>,
}

impl InterpolationMethod {
    /// Creates an interpolation method for `space`, defaulting the hue
    /// method to shorter interpolation for polar spaces.
    ///
    /// Fails when a hue method is given for a rectangular color space.
    pub fn new(
        space: ColorSpace,
        hue: Option<HueInterpolationMethod>,
    ) -> SassResult<InterpolationMethod> {
        if !space.is_polar() {
            if hue.is_some() {
                return Err(Box::new(SassError::Script {
                    message: format!(
                        "Hue interpolation method may not be set for rectangular color space {}.",
                        space.name()
                    ),
                    argument_name: None,
                }));
            }
            return Ok(InterpolationMethod { space, hue: None });
        }
        Ok(InterpolationMethod {
            space,
            hue: Some(hue.unwrap_or(HueInterpolationMethod::Shorter)),
        })
    }

    /// Parses a SassScript value representing an interpolation method, not
    /// beginning with "in".
    ///
    /// Fails unless `val` is a valid interpolation method. If `val` came
    /// from a function argument, `name` is the argument name (without the
    /// `$`), used for error reporting.
    /// Matches Dart: InterpolationMethod.fromValue routes through
    /// value.assertCommonListStyle(name, allowSlash: false).
    pub fn from_value<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        val: &ValueKind<'parse>,
        name: Option<&str>,
    ) -> SassResult<InterpolationMethod> {
        // NOTE: from_value takes ValueKind but assert_common_list_style needs
        // a Value handle; the mix() call site passes `&args[3]` (a Value) and
        // derefs to ValueKind. Route the whole Value through instead.
        Self::from_value_ref(arena, val, name)
    }

    fn from_value_ref<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        val: &ValueKind<'parse>,
        name: Option<&str>,
    ) -> SassResult<InterpolationMethod> {
        // Dart calls fromValue with name=null when the method comes through
        // the deprecated global mix() path... no — verified by probe: the
        // module path reports `$method:` attribution for list-shape errors
        // but bare `Expected "x" to be an unquoted string.` for quoted hue
        // elements. from_value receives name=Some("method") from mix(); the
        // per-element unquoted checks are bare (name=None) while the
        // list-shape and space-element checks carry the name.
        let arg_name = name.unwrap_or("value");
        let sep = val.separator();
        if sep == ListSeparator::Comma || sep == ListSeparator::Slash || val.has_brackets() {
            let v_str = val.to_display_string()?;
            // Mirror assert_common_list_style message shapes.
            let mut msg = String::from("Expected");
            if val.has_brackets() {
                msg.push_str(" an unbracketed");
            }
            let invalid_sep = true;
            if invalid_sep {
                if val.has_brackets() {
                    msg.push(',');
                } else {
                    msg.push_str(" a");
                }
                msg.push_str(" space-separated");
            }
            msg.push_str(&format!(" list, was {v_str}"));
            return Err(Box::new(SassError::Script {
                message: msg,
                argument_name: Some(arg_name.to_string()),
            }));
        }
        let list = val.as_list(arena)?;
        if list.is_empty() {
            return Err(Box::new(SassError::Script {
                message: "Expected a color interpolation method, got an empty list.".into(),
                argument_name: Some(arg_name.to_string()),
            }));
        }
        // Matches Dart: (list.first.assertString(name)..assertUnquoted(name))
        // — the unquoted check carries the argument name. EXCEPT the whole
        // single-string method value (e.g. `"hsl longer hue"` as one quoted
        // string): there list.first IS the whole value and Dart's
        // assertCommonListStyle path... verified by probe: quoted whole-value
        // reports `$method: Expected "hsl longer hue" to be an unquoted
        // string.` (with name), while a quoted hue ELEMENT reports bare
        // `Expected "longer" to be an unquoted string.` (no name).
        let space_str = value::assert_string(&list[0], Some(arg_name))?;
        space_str.assert_unquoted_with_name(Some(arg_name))?;
        let space = ColorSpace::from_name(space_str.text, Some(arg_name))?;

        if list.len() == 1 {
            return InterpolationMethod::new(space, None);
        }

        // Matches Dart: HueInterpolationMethod._fromValue(list[1], name) —
        // assertString + assertUnquoted both carry the name. Note: when the
        // hue element is quoted, assertUnquoted throws WITHOUT the name
        // (Dart's assertUnquoted() default), so the error is un-attributed
        // `Expected "longer" to be an unquoted string.` — verified by probe.
        let hue_str = value::assert_string(&list[1], Some(arg_name))?;
        hue_str.assert_unquoted().map_err(|e| SassError::Script {
            message: e.to_string(),
            argument_name: None,
        })?;
        let hue = match HueInterpolationMethod::from_str(hue_str.text) {
            Some(h) => h,
            None => {
                let s = list[1].to_display_string()?;
                return Err(Box::new(SassError::Script {
                    message: format!("Unknown hue interpolation method {s}."),
                    argument_name: Some(arg_name.to_string()),
                }));
            }
        };

        if list.len() == 2 {
            let s = val.to_display_string()?;
            return Err(Box::new(SassError::Script {
                message: format!("Expected unquoted string \"hue\" after {s}."),
                argument_name: Some(arg_name.to_string()),
            }));
        }

        let third = value::assert_string(&list[2], Some(arg_name))?;
        third.assert_unquoted()?;
        if third.text.to_lowercase() != "hue" {
            let s = val.to_display_string()?;
            let l2s = list[2].to_display_string()?;
            return Err(Box::new(SassError::Script {
                message: format!("Expected unquoted string \"hue\" at the end of {s}, was {l2s}."),
                argument_name: Some(arg_name.to_string()),
            }));
        }
        if list.len() > 3 {
            let s = val.to_display_string()?;
            return Err(Box::new(SassError::Script {
                message: format!("Expected nothing after \"hue\" in {s}."),
                argument_name: Some(arg_name.to_string()),
            }));
        }
        if !space.is_polar() {
            return Err(Box::new(SassError::Script {
                message: format!(
                    "Hue interpolation method \"HueInterpolationMethod.{} hue\" may not be set for rectangular color space {}.",
                    hue.as_str(), space.name()
                ),
                argument_name: Some(arg_name.to_string()),
            }));
        }
        InterpolationMethod::new(space, Some(hue))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hue_method_strings() {
        assert_eq!(HueInterpolationMethod::Shorter.as_str(), "shorter");
        assert_eq!(HueInterpolationMethod::Longer.as_str(), "longer");
        assert_eq!(HueInterpolationMethod::Increasing.as_str(), "increasing");
        assert_eq!(HueInterpolationMethod::Decreasing.as_str(), "decreasing");
    }

    #[test]
    fn test_new_polar_default() {
        let m = InterpolationMethod::new(ColorSpace::Hsl, None).unwrap();
        assert_eq!(m.space, ColorSpace::Hsl);
        assert_eq!(m.hue, Some(HueInterpolationMethod::Shorter));
    }

    #[test]
    fn test_new_rectangular() {
        let m = InterpolationMethod::new(ColorSpace::Srgb, None).unwrap();
        assert_eq!(m.space, ColorSpace::Srgb);
        assert_eq!(m.hue, None);
    }

    #[test]
    fn test_new_rectangular_with_hue_errors() {
        let result =
            InterpolationMethod::new(ColorSpace::Srgb, Some(HueInterpolationMethod::Longer));
        assert!(result.is_err());
    }
}
