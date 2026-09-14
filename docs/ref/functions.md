# Module: `functions/`

The built-in Sass function library: the `sass:color`, `sass:math`,
`sass:list`, `sass:map`, `sass:selector`, `sass:string`, and `sass:meta`
modules, their deprecated global aliases, and the special `if()` function.

## Module inventory

| Module          | Highlights                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| --------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `sass:color`    | `rgb`/`hsl`/`hwb`/`lab`/`lch`/`oklab`/`oklch`, `mix` (`$method` parses via `InterpolationMethod::from_value`, mirroring Dart's `assertCommonListStyle` shapes), `adjust`/`scale`/`change`, `channel`, `to-space` (`$space: null` returns the input color, mirroring Dart's `_colorInSpace`), `is-missing`, `is-in-gamut`, `is-legacy`, `same`, `is-powerless`, `to-gamut` (`from_name_with_arg` for method attribution), `whiteness`, `blackness`, `space`; module-only `space`/`is-legacy`/… alongside the deprecated globals (`red`, `green`, `blue`, `rgba`, `invert`, `hue`, `saturation`, `lightness`, `hsla`, `grayscale`, `adjust-hue`, `lighten`, `darken`, `saturate`, `desaturate`, `opacify`/`fade-in`, `transparentize`/`fade-out`, `alpha`, `opacity`, `color`, `complement` (`$space` unquoted check carries `space` attribution), `ie-hex-str` (rounds each component with `fuzzy_round`, Dart's `fuzzyRound`), `adjust-color`/`scale-color`/`change-color`) |
| `sass:math`     | `abs`, `round`, `ceil`, `floor`, `min`, `max`, `clamp`, `pow`, `sqrt`, `hypot`, `log`, `percentage`, `random`, `compatible`, `is-unitless`, `unit`, trig (`sin`/`cos`/`tan`/`asin`/`acos`/`atan`/`atan2`), `div` (**last**, matching Dart's declaration order — observable via `meta.module-functions("math")`); `ceil`/`floor`/`round` saturate at int64 extremes like Dart's `num.ceil()` (see `ref/math.md`); `log` checks `$number` units before asserting `$base`; variables `$e`, `$pi`, `$epsilon`, `$max-safe-integer`, `$min-safe-integer`, `$max-number`, `$min-number`                                                                                                                                                                                                                                                                                                                                                                                           |
| `sass:list`     | `length`, `nth`, `set-nth`, `join`/`append` (explicit `$separator: null` throws via the shared `assert_string` — only an omitted separator falls back to auto; the test harness fills omitted args from declared defaults like the real call path), `zip`, `index`, `is-bracketed`, `list-separator`, `slash`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `sass:map`      | `get`, `set`, `merge`, `remove`, `keys`, `values`, `has-key`, `deep-merge`, `deep-remove`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `sass:selector` | `is-superselector`, `simple-selectors` (parses at the compound level, so non-compound input reports `$selector: expected selector.`), `selector-parse`, `selector-nest`, `selector-append` (parses lazily during the fold, so an early append failure fires before a later parse error), `selector-extend`, `selector-replace`, `selector-unify`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `sass:string`   | `unquote`, `quote`, `to-upper-case`, `to-lower-case`, `unique-id` (declared 9th, matching Dart), `str-length`, `str-insert`, `str-index`, `str-slice`, `split`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `sass:meta`     | `feature-exists`, `inspect`, `type-of`, `keywords`, `calc-name`, `calc-args`, `accepts-content` (the evaluator-registered ones live in `eval/meta.rs`; the `load-css`/`apply` mixins are module-only — not in `global_functions`, so `function-exists("apply")` is `false` — and `function-exists` itself is `env.exists ‖ builtInFunctions[raw]` with no extra scan)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |

`if($condition, $if-true, $if-false)` is hand-authored as `LegacyIfExpression` in
the parser — it is only reachable as a callable through `meta.call()`.

## Global vs module functions

Global functions (`rgb`, `lighten`, `str-length`, …) are deprecated in favor of
their module forms. A global is built by wrapping the module function with a
deprecation warning and a renamed signature:

```rust
with_deprecation_warning("map", "map-get").with_name("map-get")
```

The wrapping order matters: `with_deprecation_warning` runs **before**
`with_name`, so the deprecation message names the _original_ module function,
matching Dart's `.WithDeprecationWarning("map", nil).WithName("map-get")`.
Calling a wrapped global emits a `global-builtin` deprecation warning.

## Behavior notes

- **Error-message field split.** The message field carries the bare message
  (no `$name: ` prefix); the argument name goes in `argument_name`.
  `SassError::full_message()` re-joins them as `"$name: message"`, matching Go's
  `Error()` and Dart's `toString()`. Math tests assert the byte-identical
  `full_message()`; map/color tests assert the field split.
- **`$min-number`** is `5e-324` — the smallest _subnormal_ (Go's
  `math.SmallestNonzeroFloat64`, Dart's `double.minPositive`), **not**
  `f64::MIN_POSITIVE` (the smallest _normal_, 2.2e-308). Checked by a
  `to_bits() == 1` test.
- **`disallowed_function_names()`** is the set of global function names minus
  the 19 CSS-compatible ones; the count invariant `== global_functions().len() - 19`
  is enforced. It feeds the CSS parser's `disallowed_function_names` field, which
  decides which bare function calls are valid in plain CSS.

## File mapping

| Dart                       | Go                  | Rust                                         |
| -------------------------- | ------------------- | -------------------------------------------- |
| `lib/src/functions/*.dart` | `go/functions/*.go` | `src/functions/*.rs`, `src/functions/mod.rs` |
