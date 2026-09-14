# Module: `util/`

Small shared helpers with minimal dependencies.

| File                | Contents                                                                                                                                                                                                                                            |
| ------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `string.rs`         | Surrogate-pair helpers (`is_high_surrogate`, `combine_surrogates`, …) — `i32`-typed to match Go's code points; never trigger on valid UTF-8.                                                                                                        |
| `utils.rs`          | `is_public(name)`, `is_private_member(name)`, `pluralize(name, n, plural)`.                                                                                                                                                                         |
| `trim_ascii.rs`     | `trim_ascii`, `trim_ascii_left`/`right`, and `a(word)` (a/an). Trims only ASCII `0x20`; `exclude_escape` preserves a trailing space after a backslash.                                                                                              |
| `character.rs`      | 19 character-classification functions (`is_name`, `is_name_start`, `is_hex`, `is_whitespace`, …) plus `as_hex`, `decimal_char_for`, `hex_char_for` (panics at `n >= 16`), `opposite`, and case-folding helpers. `MAX_ALLOWED_CHARACTER = 0x10FFFF`. |
| `number.rs`         | 16 fuzzy-math functions (`fuzzy_equals`, `fuzzy_as_int`, `fuzzy_round`, `modulo_like_sass`, `clamp_like_css`, …) with `PRECISION = 10`.                                                                                                             |
| `fuzzy_equality.rs` | `FuzzyEquality` (equality/hash for floats).                                                                                                                                                                                                         |
| `css_identifier.rs` | `to_css_identifier(text)` — CSS identifier escaping via `SpanScanner`.                                                                                                                                                                              |

Edge cases: `fuzzy_as_int` returns `None` for non-finite/overflowing values —
the low-side bound is `<` (exactly `i64::MIN` is representable, mirroring
Dart's 64-bit `round()`), the high-side bound is `>=` (no f64 holds exactly
`i64::MAX`, so anything reaching it is out of range);
`fuzzy_round` rounds `.5` up; `modulo_like_sass` uses floored division (unlike
Rust's `%`); `clamp_like_css` prefers the lower bound on NaN.
`strip_leading_zero` (compressed short path) strips only a leading `0` (Dart's
`codeUnitAt(0) == $0`), so `0.5 → .5` but `-0.5` keeps its sign.

## File mapping

| Dart                  | Go             | Rust            |
| --------------------- | -------------- | --------------- |
| `lib/src/util/*.dart` | `go/util/*.go` | `src/util/*.rs` |
