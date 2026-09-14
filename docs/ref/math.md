# Module: `math.rs`

The thin `f64` wrapper layer used by the color pipeline, trig functions, and
`math.pow()`, plus the `glibc-math` port for wasm.

## Why the wrappers exist

1. **Feature-gating seam.** `pow` swaps implementations per target (see
   `glibc-math` below) without touching call sites.
2. **libm identity.** Go's `math.Sin`/`Cos`/`Atan2`/`Pow` are Go's own and
   diverge from C libm by up to several ULP (measured: `sin(pi)`, 7 ULP;
   `tan(45deg)`, 1 ULP); Rust's `f64::sin` _are_ C libm —
   the same library Dart uses — so no function-pointer indirection is needed.
   (Go closes the same gap differently: `sassmathcgo` hooks route its tests
   and spec runner through C libm unconditionally, with a `-tags cgomath`
   opt-in for its CLI.)
3. **Central FMA/rounding rules.** The module documents and tests the
   FMA-prevention rule that color matrix multiplications must obey.

All wrappers are `#[inline]` and delegate to `f64`; `pow` dispatches on the
feature:

```rust
pub fn pow(x: f64, y: f64) -> f64 {
    #[cfg(feature = "glibc-math")] { crate::math_glibc_pow::pow(x, y) }
    #[cfg(not(feature = "glibc-math"))] { x.powf(y) }
}
```

`math::pow` is the single `pow` entry point for the whole port (Sass
`math.pow()`, calc `pow`/`exp`, and every color-space conversion).

## Floating-point parity model

Spec golden CSS depends on the IEEE-754 double semantics of every operation.
Three conditions must hold:

1. **FMA prevention.** `-Ctarget-cpu=native` may fuse a multiply-add into one
   FMA, which rounds once instead of twice and diverges at the last ULP. Color
   matrix multiplications round each product explicitly:
   `(m[i] * v) as f64 + (m[j] * v) as f64 + ...`.
2. **libm target identity.** Native `f64::powf` on macOS/glibc libm is the
   algorithm the goldens were produced with; wasm has no libm (see below).
3. **Golden `to_bits` tests.** Tests assert raw `f64::to_bits()` values captured
   from Dart/C libm, so accidental libm/FMA drift fails loudly rather than as a
   1-ULP CSS diff.
4. **Verbatim lossy formulas.** Where Dart's own formula loses precision, the
   port replicates the loss instead of "fixing" it. Two instances, both load-
   bearing at the last ULP: cubing uses `x * x * x`, never `pow(x, 3.0)`
   (Dart's integer-`pow` path multiplies; C's `pow(x, 3.0)` takes the exp/log
   path — e.g. `color_conversions.rs` `labFToXZ` and the OKLab→LMS reverse
   path), and hue normalization uses `(h % 360 + 360) % 360` exactly (the
   intermediate `+360` loses ~1 ULP for already-positive hues via the
   larger-magnitude round-trip). Do not simplify either.

A related rule: the **D50 white point** must be computed at runtime
(`LazyLock`) as `[x/y, 1.0, (1-x-y)/y]`, not a const — const evaluation rounds
the division differently from Go's `init()`/Dart's lazy double division and
would diverge at the last ULP.

## Debugging a last-digit mismatch

When a test shows a last-digit CSS mismatch, build **standalone Rust and
Dart reproducers** that inline the full conversion pipeline with hardcoded
inputs, printing every intermediate value in full-precision hex (`{:x}`
of `to_bits()`); compare to find the exact divergence point, then minimize
to the single operation. This is how the cubing and `normalizeHue` rules
above were found (trace `srgb(-999999,0,0) → oklab → xyz` stage by stage
until one value differs by 1 ULP). Guessing from the output digit alone
almost never identifies the stage.

## Integer-valued builtins (`ceil`/`floor`/`round`)

Dart's `num.ceil()`/`floor()`/`round()` return 64-bit ints: out-of-range
finite values saturate at ±2⁶³∓1 and non-finite input throws
`Unsupported operation: Infinity or NaN toInt`. The `math.ceil`/`floor`/`round`
builtins (`functions/math.rs: dart_int_op`) replicate this — returning the
`i64` extreme as an f64 sentinel. No f64 holds 2⁶³−1 exactly (`i64::MAX as f64`
is 2⁶³), so the serializer (`util/number.rs: write_number_to`) special-cases
the sentinel f64s back to the exact digit strings `9223372036854775807` /
`-9223372036854775808`, matching Dart's `integer.toString()`.

## The `glibc-math` feature

**Problem.** On `wasm32-unknown-unknown` there is no libm: `f64::powf` lowers to
`compiler_builtins`' fdlibm-style `pow`, which rounds the wrong way on ~28
last-ULP `color/to_space/*/out_of_range/far` inputs. Neither `libm` nor V8's
`Math.pow` matches native either.

**Solution.** A faithful, non-FMA Rust port of glibc's `e_pow.c` (`__pow`),
gated behind `glibc-math`. It bit-matches native macOS/glibc libm on every color
input. Port configuration mirrors the C: non-FMA split paths,
`TOINT_INTRINSICS=0`, `WANT_ROUNDING=1`, `WANT_ERRNO=0`.

**Fidelity method.** The port was audited **line-by-line** against glibc rather
than black-box compared. That audit found two translation bugs bit-for-bit
testing would have masked: the `±inf`-exponent branch compared `|x| < 1` against
`y < 0` (swapping the `0`/`infinity` results), and the `2^1009` scale constant
was written as a 15-hex-digit literal (`2^-896`), underflowing `exp(1000.65)`.

**Gotcha.** `math::pow(x, 2.0)` changes semantics under the feature — native
`f64::powf` special-cases some integer exponents differently from the
exp/log-based glibc path. This is fine on wasm (where the feature is meant to be
on) but is why native builds keep `x.powf(y)` by default.

## File mapping

| Dart                              | Go                 | Rust                                                |
| --------------------------------- | ------------------ | --------------------------------------------------- |
| `lib/src/util/number.dart` (math) | `go/sassmath/*.go` | `src/math.rs`                                       |
| (glibc `e_pow.c`)                 | —                  | `src/math_glibc_pow.rs`, `src/math_glibc_tables.rs` |
