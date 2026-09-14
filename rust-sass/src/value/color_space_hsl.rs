// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/space/hsl.dart
// go-source: go/value/color_space_hsl.go

use crate::common::exception::SassResult;
use crate::value::color::{ColorSpace, SassColor};
use crate::value::color_conversions::SrgbConvertOpts;
use crate::value::color_space_srgb::srgb_convert_internal;
use crate::value::color_utils::hue_to_rgb;

// The legacy HSL space: a polar encoding (`hue`, `saturation`, `lightness`)
// over sRGB primaries. (Dart: `HslColorSpace`, internal.)
//
// The channel layout (bounded, legacy, polar; saturation clamped at the low
// end, lightness unclamped) is declared once on [`ColorSpace`]; see
// `channels_linear` in `value/color.rs`.

// Converts an HSL color to `dest` via the CSS3 `hue_to_rgb` algorithm,
// scaling hue to turns and saturation/lightness to unit, then delegating to
// sRGB conversion with missing-channel flags forwarded so achromatic results
// keep a powerless (missing) hue.
pub(crate) fn hsl_convert(
    dest: ColorSpace,
    c0: Option<f64>,
    c1: Option<f64>,
    c2: Option<f64>,
    alpha: Option<f64>,
) -> SassResult<SassColor> {
    // Algorithm from the CSS3 spec: https://www.w3.org/TR/css3-color/#hsl-color.
    let scaled_hue = c0.map(|v| (v / 360.0) % 1.0).unwrap_or(0.0);
    let scaled_saturation = c1.map(|v| v / 100.0).unwrap_or(0.0);
    let scaled_lightness = c2.map(|v| v / 100.0).unwrap_or(0.0);

    let m2 = if scaled_lightness <= 0.5 {
        scaled_lightness * (scaled_saturation + 1.0)
    } else {
        // FMA prevention: `as f64` forces rounding before the addition
        // (see critical-invariants.md); clippy sees a no-op cast.
        #[allow(clippy::unnecessary_cast)]
        let m2 =
            scaled_lightness + scaled_saturation - (scaled_lightness * scaled_saturation) as f64;
        m2
    };
    // FMA prevention (see above).
    #[allow(clippy::unnecessary_cast)]
    let m1 = ((scaled_lightness * 2.0) as f64) - m2;

    let rv = hue_to_rgb(m1, m2, scaled_hue + 1.0 / 3.0);
    let gv = hue_to_rgb(m1, m2, scaled_hue);
    let bv = hue_to_rgb(m1, m2, scaled_hue - 1.0 / 3.0);

    srgb_convert_internal(
        dest,
        Some(rv),
        Some(gv),
        Some(bv),
        alpha,
        Some(&SrgbConvertOpts {
            missing_lightness: c2.is_none(),
            missing_chroma: c1.is_none(),
            missing_hue: c0.is_none(),
        }),
    )
}

pub(crate) fn hsl_transformation_matrix(_dest: ColorSpace) -> Option<&'static [f64; 9]> {
    // Matches Dart: `HslColorSpace` defines no matrix. HSL is not linearly
    // transformable; every conversion runs through sRGB via `hsl_convert`.
    None
}

#[cfg(test)]
mod tests {
    use super::super::color::ColorSpace;
    use super::super::color_conversions::convert_color;
    use crate::math;
    use crate::util::number;
    use crate::value::color_conversions::matrix_mul;
    use crate::value::color_space_srgb::srgb_transformation_matrix;
    use crate::value::color_utils::hue_to_rgb;
    fn ws(v: f64) -> String {
        number::write_number_to_string(v)
    }

    #[test]
    fn test_metadata() {
        assert_eq!(ColorSpace::Hsl.name(), "hsl");
        assert!(ColorSpace::Hsl.is_bounded());
        assert!(ColorSpace::Hsl.is_legacy());
        assert!(ColorSpace::Hsl.is_polar());
    }

    #[test]
    fn test_hsl_convert() {
        let c = convert_color(
            ColorSpace::Hsl,
            ColorSpace::Rgb,
            120.0,
            100.0,
            50.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c.channel0), "0");
        assert_eq!(ws(c.channel1), "255");
        assert_eq!(ws(c.channel2), "0");
        let c2 = convert_color(
            ColorSpace::Hsl,
            ColorSpace::Srgb,
            120.0,
            100.0,
            50.0,
            1.0,
            [false; 4],
            None,
        )
        .unwrap();
        assert_eq!(ws(c2.channel0), "0");
        assert_eq!(ws(c2.channel1), "1");
        assert_eq!(ws(c2.channel2), "0");
    }

    /// Pins the last-ULP `pow` behavior behind
    /// `spec/core_functions/color/to_space/hsl/xyz/out_of_range/far`
    /// (`color.to-space(hsl(20deg 999999% 50%), xyz)`).
    ///
    /// Expected channels (macOS/glibc libm, i.e. the spec goldens):
    /// x = 136956388.39988744   (0x41a05393c8ccbe0c)
    /// y = 59264689.52803937    (0x418c42758c396cb5)
    /// z = -623200798.6169885   (0xc1c292a50f4ef97b)
    ///
    /// fdlibm-style pows (`compiler_builtins` on wasm, `libm`, `fpmath`) round
    /// x up to 136956388.3998875 (0x41a05393c8ccbe0e). Running both providers
    /// checks that the platform libm (`f64::powf`) and — with the `glibc-math`
    /// feature — the glibc e_pow.c port behind `math::pow` match those goldens.
    ///
    /// The full computation, written out so it can be pasted straight into
    /// Python (`**` is the C-libm `pow`): the H/S/L channels are scaled
    /// (`H = (20/360) % 1`, `S = 999999/100`, `L = 50/100`), the HSL value is
    /// turned into RGB via the CSS3 `hue_to_rgb` helper, each channel is
    /// linearized (`srgb_and_display_p3_to_linear`), and the result is the
    /// sRGB → XYZ (D65) matrix product (`LINEAR_SRGB_TO_XYZ_D65`):
    ///
    /// ```python
    /// H = (20.0 / 360.0) % 1.0
    /// S = 999999.0 / 100.0
    /// L = 50.0 / 100.0
    /// m2 = L * (S + 1.0)            # L <= 0.5
    /// m1 = 2.0 * L - m2
    /// def hue_to_rgb(m1, m2, hue):
    ///     if hue < 0.0: hue += 1.0
    ///     if hue > 1.0: hue -= 1.0
    ///     if hue < 1.0 / 6.0:    return m1 + ((m2 - m1) * hue) * 6.0
    ///     elif hue <= 1.0 / 2.0: return m2
    ///     elif hue < 2.0 / 3.0:  return m1 + ((m2 - m1) * (2.0 / 3.0 - hue)) * 6.0
    ///     else:                  return m1
    /// r = hue_to_rgb(m1, m2, H + 1.0 / 3.0)
    /// g = hue_to_rgb(m1, m2, H)
    /// b = hue_to_rgb(m1, m2, H - 1.0 / 3.0)
    /// def lin(c):
    ///     a = abs(c)
    ///     if a <= 0.04045: return c / 12.92
    ///     return (-1.0 if c < 0.0 else 1.0) * (((a + 0.055) / 1.055) ** 2.4)
    /// lr, lg, lb = lin(r), lin(g), lin(b)
    /// M = (0.4123907992659595, 0.35758433938387796, 0.1804807884018343,
    ///      0.21263900587151036, 0.7151686787677559, 0.07219231536073371,
    ///      0.01933081871559185, 0.11919477979462598, 0.9505321522496606)
    /// x = M[0]*lr + M[1]*lg + M[2]*lb
    /// y = M[3]*lr + M[4]*lg + M[5]*lb
    /// z = M[6]*lr + M[7]*lg + M[8]*lb
    /// # -> x=136956388.39988744, y=59264689.52803937, z=-623200798.6169885
    /// ```
    #[test]
    fn hsl_to_xyz_out_of_range_far_matches_expected_ulp() {
        let scaled_hue = (20.0 / 360.0) % 1.0;
        let scaled_saturation = 999999.0 / 100.0;
        let scaled_lightness = 50.0 / 100.0;
        let m2 = if scaled_lightness <= 0.5 {
            scaled_lightness * (scaled_saturation + 1.0)
        } else {
            scaled_lightness + scaled_saturation - scaled_lightness * scaled_saturation
        };
        let m1 = scaled_lightness * 2.0 - m2;
        let rv = hue_to_rgb(m1, m2, scaled_hue + 1.0 / 3.0);
        let gv = hue_to_rgb(m1, m2, scaled_hue);
        let bv = hue_to_rgb(m1, m2, scaled_hue - 1.0 / 3.0);

        let mat = srgb_transformation_matrix(ColorSpace::XyzD65).unwrap();
        let providers = [
            ("powf", f64::powf as fn(f64, f64) -> f64),
            ("math::pow", math::pow as fn(f64, f64) -> f64),
        ];
        for (name, pow) in providers {
            let lin = |c: f64| {
                let abs = c.abs();
                if abs <= 0.04045 {
                    c / 12.92
                } else {
                    c.signum() * pow((abs + 0.055) / 1.055, 2.4)
                }
            };
            let (x, y, z) = matrix_mul(mat, lin(rv), lin(gv), lin(bv));
            assert_eq!(
                (x.to_bits(), y.to_bits(), z.to_bits()),
                (0x41a05393c8ccbe0c, 0x418c42758c396cb5, 0xc1c292a50f4ef97b),
                "{name} produced x={x} y={y} z={z}"
            );
        }
    }
}
