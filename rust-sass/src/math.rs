// dart-source: lib/src/util/number.dart (sqrt/sin/cos/tan/atan/asin/acos/abs/log/pow/atan2 section)
// go-source: go/sassmath/math.go

/// Math functions for Sass color pipeline and trig operations.
///
/// Wraps `std::f64` methods to match the API of Go's `go/sassmath/` package.
/// The explicit function wrappers allow future feature-gating (e.g. WASM
/// needing a different `Pow` implementation) without changing call sites.
///
/// Unlike Go, Rust's `f64::sin` etc. are C's libm — the same as Dart — so
/// no `Custom*` function-pointer indirection is needed.
///
/// # FMA prevention
///
/// For matrix multiplication (used in color space conversions), calling sites
/// must use `(m[i] * v) as f64 + ...` to prevent FMA fusion when compiled
/// with `-Ctarget-cpu=native` (which enables FMA on supporting hardware).
/// This module's tests verify that behavior.
///
/// See `docs/ref/math.md` for the module reference (parity model,
/// FMA rule, and the `glibc-math` port).
///
/// `x` raised to `y`. This is also Sass's `math.pow()` (and calc `pow`/`exp`),
/// not just internal color conversions.
///
/// Defaults to the platform libm (`f64::powf`), which is what the spec goldens
/// were produced against. With the `glibc-math` feature it uses a Rust port of
/// glibc's `e_pow.c` instead: on wasm32 there is no libm and `f64::powf`
/// (compiler_builtins) rounds the wrong way on ~28 last-ULP specs, while the
/// glibc port bit-matches the native macOS/glibc libm used for those goldens.
#[inline]
pub fn pow(x: f64, y: f64) -> f64 {
    #[cfg(feature = "glibc-math")]
    {
        crate::math_glibc_pow::pow(x, y)
    }
    #[cfg(not(feature = "glibc-math"))]
    {
        x.powf(y)
    }
}

#[inline]
/// Arctangent of `y`/`x`, in radians.
pub fn atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

#[inline]
/// Sine of `x` radians (caller coerces units; see `value/number_math.rs`).
pub fn sin(x: f64) -> f64 {
    x.sin()
}

#[inline]
/// Cosine of `x` radians (caller coerces units; see `value/number_math.rs`).
pub fn cos(x: f64) -> f64 {
    x.cos()
}

#[inline]
/// Tangent of `x` radians.
pub fn tan(x: f64) -> f64 {
    x.tan()
}

#[inline]
/// Arctangent of `x`, in radians (callers convert to degrees).
pub fn atan(x: f64) -> f64 {
    x.atan()
}

#[inline]
/// Arcsine of `x`, in radians (callers convert to degrees).
pub fn asin(x: f64) -> f64 {
    x.asin()
}

#[inline]
/// Arccosine of `x`, in radians (callers convert to degrees).
pub fn acos(x: f64) -> f64 {
    x.acos()
}

#[inline]
/// Natural logarithm of `x` (callers divide for other bases).
pub fn ln(x: f64) -> f64 {
    x.ln()
}

#[inline]
/// Square root of `x`.
pub fn sqrt(x: f64) -> f64 {
    x.sqrt()
}

#[inline]
/// Absolute value of `x`.
pub fn abs(x: f64) -> f64 {
    x.abs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn bits(v: f64) -> u64 {
        v.to_bits()
    }

    // --- Golden values from Dart (C libm) — see /tmp/golden.dart ---

    #[test]
    fn matrix_mul_with_asm_f64_cast_matches_dart() {
        // Without `as f64` on each intermediate, LLVM may fuse into FMA
        // when compiled with `-Ctarget-cpu=native`. The cast forces
        // IEEE 754 rounding after each multiply, matching Dart/Go output.
        let m = [0.5, 0.3, 0.2, 0.1, 0.4, 0.5, 0.3, 0.3, 0.4];
        let (v0, v1, v2) = (1.0, 0.5, 0.2);

        // FMA-safe: `as f64` after each multiply
        #[allow(clippy::unnecessary_cast)]
        let r = (m[0] * v0) as f64 + (m[1] * v1) as f64 + (m[2] * v2) as f64;
        #[allow(clippy::unnecessary_cast)]
        let g = (m[3] * v0) as f64 + (m[4] * v1) as f64 + (m[5] * v2) as f64;
        #[allow(clippy::unnecessary_cast)]
        let b = (m[6] * v0) as f64 + (m[7] * v1) as f64 + (m[8] * v2) as f64;

        assert_eq!(bits(r), 0x3fe6147ae147ae15, "matmul_r");
        assert_eq!(bits(g), 0x3fd999999999999a, "matmul_g");
        assert_eq!(bits(b), 0x3fe0f5c28f5c28f6, "matmul_b");
    }

    #[test]
    fn m116_f1_minus_16_matches_dart() {
        let f1 = 1.1379310344827585;
        // FMA-safe: cast intermediate before subtract
        #[allow(clippy::unnecessary_cast)]
        let result = (116.0 * f1) as f64 - 16.0;
        assert_eq!(bits(result), 0x405cfffffffffffe);
    }

    #[test]
    fn trig_sin_pi() {
        let pi = PI;
        assert_eq!(bits(sin(pi)), 0x3ca1a62633145c07);
    }

    #[test]
    fn trig_cos_pi() {
        let pi = PI;
        assert_eq!(bits(cos(pi)), 0xbff0000000000000);
    }

    #[test]
    fn trig_cos_half_pi() {
        let pi = PI;
        assert_eq!(bits(cos(pi / 2.0)), 0x3c91a62633145c07);
    }

    #[test]
    fn trig_atan2_1_0() {
        assert_eq!(bits(atan2(1.0, 0.0)), 0x3ff921fb54442d18);
    }

    #[test]
    fn trig_atan2_0_neg1() {
        assert_eq!(bits(atan2(0.0, -1.0)), 0x400921fb54442d18);
    }

    #[test]
    fn basic_pow() {
        let v = pow(2.0, 3.0);
        assert!((v - 8.0).abs() < 1e-15, "pow(2,3) = {}", v);
    }

    #[test]
    fn basic_sqrt() {
        let v = sqrt(4.0);
        assert!((v - 2.0).abs() < 1e-15, "sqrt(4) = {}", v);
    }

    #[test]
    fn basic_abs() {
        assert!((abs(-3.5) - 3.5).abs() < 1e-15);
        assert!((abs(3.5) - 3.5).abs() < 1e-15);
    }

    #[test]
    fn all_functions_callable() {
        // Verify every function compiles and returns without panic.
        let _ = sin(0.0);
        let _ = cos(0.0);
        let _ = tan(0.0);
        let _ = atan(0.0);
        let _ = atan2(1.0, 1.0);
        let _ = asin(0.0);
        let _ = acos(1.0);
        let _ = ln(1.0);
        let _ = sqrt(4.0);
        let _ = abs(-1.0);
        let _ = pow(2.0, 3.0);
    }
}
