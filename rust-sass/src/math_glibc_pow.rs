//! Faithful Rust port of glibc's `sysdeps/ieee754/dbl-64/e_pow.c`
//! (`__pow`), non-FMA path only (`#ifndef __FP_FAST_FMA`), `TOINT_INTRINSICS=0`,
//! `WANT_ROUNDING=1`, `WANT_ERRNO=0`. Table data generated from
//! `e_pow_log_data.c` / `e_exp_data.c`.
//!
//! Reproduces the sass-spec expected pow values bit-for-bit, including
//! last-ULP cases fdlibm/`compiler_builtins`/`fpmath`/V8 round the wrong way.
//!
//! See `docs/ref/math.md` (module reference + parity model) and
//! `docs/ref/wasm.md` ("wasm-specific seams", `glibc-math` record).
//!
//! LGPL-2.1-or-later (glibc).

// dart-source: N/A (true-original port of glibc C — no Dart counterpart; Dart calls through to C libm)
// go-source: N/A (no Go counterpart)

use crate::math_glibc_tables as tables;

#[inline(always)]
fn from_bits(b: u64) -> f64 {
    f64::from_bits(b)
}

#[inline(always)]
fn top12(x: f64) -> u32 {
    (x.to_bits() >> 52) as u32
}

// ---- log_inline config ----
const OFF: u64 = 0x3fe6_9555_0000_0000;
const TOP12_1PM54: u32 = 0x3c9; // top12(2^-54)
const TOP12_512: u32 = 0x408; // top12(512.0) = top12(2^9)
const TOP12_1024: u32 = 0x409; // top12(1024.0) = top12(2^10)
const ONE_1P1009: f64 = f64::from_bits(0x7f00_0000_0000_0000); // 2^1009
const ONE_1PM1022: f64 = f64::from_bits(0x0010_0000_0000_0000); // 2^-1022

/// Compute (hi, tail) with hi + tail = log(x). `ix` is x's bit pattern,
/// normalized so the exponent may be negative for subnormals.
#[inline]
fn log_inline(ix: u64) -> (f64, f64) {
    let tmp = ix.wrapping_sub(OFF);
    let i = ((tmp >> (52 - 7)) % 128) as usize;
    let kd = ((tmp as i64) >> 52) as f64; // arithmetic shift
    let iz = ix.wrapping_sub(tmp & (0xfff_u64 << 52));
    let z = from_bits(iz);

    let invc = from_bits(tables::POW_TAB[i][0]);
    let logc = from_bits(tables::POW_TAB[i][1]);
    let logctail = from_bits(tables::POW_TAB[i][2]);

    // Non-FMA split of z: rhi, rlo, rhi*rhi exact, |rlo| <= |r|.
    let zhi = from_bits((iz + (1_u64 << 31)) & 0xffff_ffff_0000_0000);
    let zlo = z - zhi;
    let rhi = zhi * invc - 1.0;
    let rlo = zlo * invc;
    let r = rhi + rlo;

    let ln2hi = from_bits(tables::POW_LN2HI);
    let ln2lo = from_bits(tables::POW_LN2LO);
    // k*Ln2 + log(c) + r.
    let t1 = kd * ln2hi + logc;
    let t2 = t1 + r;
    let lo1 = kd * ln2lo + logctail;
    let lo2 = t1 - t2 + r;

    // Non-FMA polynomial split.
    let a0 = from_bits(tables::POW_POLY[0]); // -0.5
    let ar = a0 * r;
    let ar2 = r * ar;
    let ar3 = r * ar2;
    let arhi = a0 * rhi;
    let arhi2 = rhi * arhi;
    let hi = t2 + arhi2;
    let lo3 = rlo * (ar + arhi);
    let lo4 = t2 - hi + arhi2;
    let p = ar3
        * (from_bits(tables::POW_POLY[1])
            + r * from_bits(tables::POW_POLY[2])
            + ar2
                * (from_bits(tables::POW_POLY[3])
                    + r * from_bits(tables::POW_POLY[4])
                    + ar2 * (from_bits(tables::POW_POLY[5]) + r * from_bits(tables::POW_POLY[6]))));
    let lo = lo1 + lo2 + lo3 + lo4 + p;
    let y = hi + lo;
    (y, hi - y + lo)
}

const EXP_SHIFT: u64 = 0x4338_0000_0000_0000; // 0x1.8p52
const SIGN_BIAS: u64 = 0x800 << 7; // 0x40000

/// Handle cases that may overflow/underflow when computing scale*(1+TMP).
#[inline]
fn specialcase(tmp: f64, mut sbits: u64, ki: u64) -> f64 {
    if ki & 0x8000_0000 == 0 {
        // k > 0: exponent of scale might have overflowed by <= 460.
        sbits = sbits.wrapping_sub(1009_u64 << 52);
        let scale = from_bits(sbits);
        let y = scale + scale * tmp;
        return y * ONE_1P1009; // check_oflow (WANT_ERRNO=0: identity)
    }
    // k < 0: special care in the subnormal range.
    sbits = sbits.wrapping_add(1022_u64 << 52);
    let scale = from_bits(sbits); // signed scale
    let mut y = scale + scale * tmp;
    if y.abs() < 1.0 {
        let mut one = 1.0;
        if y < 0.0 {
            one = -1.0;
        }
        let lo = scale - y + scale * tmp;
        let hi = one + y;
        let lo = one - hi + y + lo;
        y = (hi + lo) - one; // math_narrow_eval (no extra precision)
        if y == 0.0 {
            y = from_bits(sbits & 0x8000_0000_0000_0000); // fix sign of 0
        }
    }
    y * ONE_1PM1022 // check_uflow (WANT_ERRNO=0: identity)
}

/// exp(x + xtail), sign via sign_bias (0 or SIGN_BIAS).
#[inline]
fn exp_inline(x: f64, xtail: f64, sign_bias: u64) -> f64 {
    let mut abstop = top12(x) & 0x7ff;
    if abstop.wrapping_sub(TOP12_1PM54) >= TOP12_512.wrapping_sub(TOP12_1PM54) {
        if abstop.wrapping_sub(TOP12_1PM54) >= 0x8000_0000 {
            // Avoid spurious underflow for tiny x. 0 is a common input.
            let one = 1.0 + x; // WANT_ROUNDING
            return if sign_bias != 0 { -one } else { one };
        }
        if abstop >= TOP12_1024 {
            // inf and nan already handled; x finite but large.
            if x.to_bits() >> 63 != 0 {
                return if sign_bias != 0 { -0.0 } else { 0.0 }; // __math_uflow
            } else {
                return if sign_bias != 0 {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                }; // __math_oflow
            }
        }
        abstop = 0; // large x: specialcased below
    }
    // exp(x) = 2^(k/N) * exp(r), r in [-ln2/2N, ln2/2N].
    let z = from_bits(tables::EXP_INVLN2N) * x;
    // TOINT_INTRINSICS=0: shift trick.
    let kd = z + from_bits(EXP_SHIFT);
    let ki = kd.to_bits();
    let kd = kd - from_bits(EXP_SHIFT);
    let mut r = x + kd * from_bits(tables::EXP_NEGLN2HIN) + kd * from_bits(tables::EXP_NEGLN2LON);
    r += xtail; // 2^-200 < |xtail| < 2^-8/N
    let idx = (2 * (ki % 128)) as usize;
    let top = (ki + sign_bias) << (52 - 7);
    let tail = from_bits(tables::EXP_TAB[idx]);
    let sbits = tables::EXP_TAB[idx + 1].wrapping_add(top);
    let r2 = r * r;
    let tmp = tail
        + r
        + r2 * (from_bits(tables::EXP_POLY[0]) + r * from_bits(tables::EXP_POLY[1]))
        + r2 * r2 * (from_bits(tables::EXP_POLY[2]) + r * from_bits(tables::EXP_POLY[3]));
    if abstop == 0 {
        return specialcase(tmp, sbits, ki);
    }
    let scale = from_bits(sbits);
    scale + scale * tmp
}

/// 0 if not int, 1 if odd int, 2 if even int. `iy` = bits of nonzero finite f64.
#[inline]
fn checkint(iy: u64) -> i32 {
    let e = (iy >> 52) & 0x7ff;
    if e < 0x3ff {
        return 0;
    }
    if e > 0x3ff + 52 {
        return 2;
    }
    let rem_mask = (1_u64 << (0x3ff + 52 - e)) - 1;
    if iy & rem_mask != 0 {
        return 0;
    }
    if iy & (1_u64 << (0x3ff + 52 - e)) != 0 {
        1
    } else {
        2
    }
}

/// 1 if `i` is the bit pattern of 0, infinity or nan.
#[inline]
fn zeroinfnan(i: u64) -> bool {
    let inf = f64::INFINITY.to_bits();
    (2_u64.wrapping_mul(i)).wrapping_sub(1) >= (2_u64.wrapping_mul(inf)).wrapping_sub(1)
}

/// Port of glibc `__pow`, non-FMA.
pub fn pow(x: f64, y: f64) -> f64 {
    let mut sign_bias = 0u64;
    let mut ix = x.to_bits();
    let iy = y.to_bits();
    let mut topx = top12(x);
    let topy = top12(y);
    let one = 1.0f64.to_bits();
    let inf = f64::INFINITY.to_bits();

    if topx.wrapping_sub(0x001) >= 0x7ff - 0x001
        || (topy & 0x7ff).wrapping_sub(0x3be) >= 0x43e - 0x3be
    {
        if zeroinfnan(iy) {
            if 2_u64.wrapping_mul(iy) == 0 {
                return 1.0; // (anything)^0 = 1
            }
            if ix == one {
                return 1.0; // 1^anything = 1
            }
            if 2_u64.wrapping_mul(ix) > 2_u64.wrapping_mul(inf)
                || 2_u64.wrapping_mul(iy) > 2_u64.wrapping_mul(inf)
            {
                return x + y; // nan
            }
            if 2_u64.wrapping_mul(ix) == 2_u64.wrapping_mul(one) {
                return 1.0; // (+-1)^+-inf
            }
            // Matches glibc e_pow.c: `== !(iy >> 63)`, i.e. |x|<1 && y==inf or
            // |x|>1 && y==-inf.
            if (2_u64.wrapping_mul(ix) < 2_u64.wrapping_mul(one)) == (iy >> 63 == 0) {
                return 0.0;
            }
            return y * y;
        }
        if zeroinfnan(ix) {
            let mut x2 = x * x;
            if ix >> 63 != 0 && checkint(iy) == 1 {
                x2 = -x2;
                sign_bias = 1;
            }
            if 2_u64.wrapping_mul(ix) == 0 && iy >> 63 != 0 {
                return if sign_bias != 0 {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                }; // __math_divzero
            }
            return if iy >> 63 != 0 { 1.0 / x2 } else { x2 };
        }
        // x and y are nonzero finite.
        if ix >> 63 != 0 {
            // finite x < 0
            let yint = checkint(iy);
            if yint == 0 {
                return f64::NAN; // __math_invalid
            }
            if yint == 1 {
                sign_bias = SIGN_BIAS;
            }
            ix &= 0x7fff_ffff_ffff_ffff;
            topx &= 0x7ff;
        }
        if (topy & 0x7ff).wrapping_sub(0x3be) >= 0x43e - 0x3be {
            if ix == one {
                return 1.0; // |x|==1, y extreme (sign cleared above)
            }
            if (topy & 0x7ff) < 0x3be {
                // |y| < 2^-65: x^y ~= 1 + y*log(x) (WANT_ROUNDING)
                return if ix > one { 1.0 + y } else { 1.0 - y };
            }
            return if (ix > one) == (topy < 0x800) {
                f64::INFINITY // __math_oflow(0)
            } else {
                0.0 // __math_uflow(0)
            };
        }
        if topx == 0 {
            // Normalize subnormal x so the exponent becomes negative.
            ix = (x * from_bits(0x4330_0000_0000_0000)).to_bits(); // x * 2^52
            ix &= 0x7fff_ffff_ffff_ffff;
            ix = ix.wrapping_sub(52_u64 << 52);
        }
    }

    let (hi, lo) = log_inline(ix);
    // y * (hi + lo) as a double-double (non-FMA).
    let yhi = from_bits(iy & 0xffff_ffff_f800_0000);
    let ylo = y - yhi;
    let lhi = from_bits(hi.to_bits() & 0xffff_ffff_f800_0000);
    let llo = hi - lhi + lo;
    let ehi = yhi * lhi;
    let elo = ylo * lhi + y * llo;
    exp_inline(ehi, elo, sign_bias)
}
