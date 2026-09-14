# Module: `value/`

The Sass value type and its eleven concrete types. See also `callable.md` for
`SassFunction`/`SassMixin`'s callable machinery.

## The `Value` handle

```rust
pub enum ValueKind<'parse> {
    Boolean(SassBoolean), Null,
    String(SassString<'parse>), Number(SassNumber), Color(SassColor),
    List(SassList<'parse>), ArgumentList(SassArgumentList<'parse>),
    Map(SassMap<'parse>), Calculation(Box<SassCalculation>),
    Function(SassFunction<'parse>), Mixin(SassMixin<'parse>),
}

pub struct Value<'parse>(&'parse ValueInner<'parse>);   // Copy, arena reference
```

`Value` is `&'parse ValueInner` — an arena reference, not an owning `Rc`. The
value is built with `Value::new_with_arena(&arena, ValueKind::...)` into the
compile arena, so cloning a `Value` is a pointer copy and every value returned
to Sass lives for the whole compile. The type set is closed (sealed in Dart),
which is why it is an enum with static dispatch rather than a trait object.

### Equality and hashing

`PartialEq`/`Hash` perform **deep value equality** — not pointer identity. The only
identity-compared values are `SassFunction` and `SassMixin` (whose equality is
their `Callable`'s address identity). A `Cell<Option<i32>>` hash cache lives in
`ValueInner` so repeated hashing (e.g. in an `IndexMap<Value, Value>` lookup)
does not re-walk a deep list or map; `SassMap` keys work through the reflexive
`Borrow<Value>`.

### Value invariance

`Value<'parse>` is **invariant** in `'parse` because `SassFunction`/`SassMixin`
hold a `Callable`, whose callback mentions `'parse` in argument position. The
value API therefore decouples the reference lifetime from the content lifetime:
assertion functions are `fn assert_*<'v, 'parse>(v: &'v Value<'parse>)`, and
`ListValue<'v, 'parse>` carries both. Never write `fn f(v: &'parse Value<'parse>)`.

## Concrete types

### `SassNumber`

A **single struct** — not Go's three-way split (unitless / single-unit /
complex):

```rust
pub struct SassNumber { value: f64, numerator_units: Vec<String>, denominator_units: Vec<String>, as_slash: Option<Box<(SassNumber, SassNumber)>> }
```

Arithmetic dispatches at runtime on whether the operands have units. Why one
struct: the Dart class hierarchy is one class, and Go's three-type split exists
only because Go can't express Dart's optional-unit constructor polymorphism; Rust
can, so a single struct is the faithful port.

The canonical multiplier for a unit is a **hardcoded `match`**, not a
`HashMap` lookup: Go relies on `orderedmap.LinkedMap` insertion order (the first
entry is canonical), but Rust's `HashMap` iteration order is non-deterministic.
`CONVERSION_FACTORS` remains a `HashMap` for `conversion_factor()` lookups, but
`canonical_multiplier_for_unit` is static. It mirrors Dart's
`canonicalMultiplierForUnit` (`1 / innerMap.values.first`, i.e. the reciprocal
of the canonical row's first entry): `kHz → 1000.0`, `dpcm → 2.54`,
`dppx → 96.0` — so `1kHz == 1000Hz`, `1dppx == 96dpi`, `1dpcm == 2.54dpi` hold
for `equals`/`hash_code` alike (coercion itself uses `conversion_factor`
directly and is unaffected).

The convertible unit families are: lengths (`in`, `cm`, `pc`, `mm`, `q`, `pt`,
`px`), angles (`deg`, `grad`, `rad`, `turn`), time (`s`, `ms`), frequency
(`Hz`, `kHz`), and pixel density (`dpi`, `dpcm`, `dppx`). Font-relative units
(`em`, `rem`, `vw`, …) are _compatible_ but **not convertible**. Math-function
unit semantics: `sqrt` is unitless; `sin`/`cos`/`tan` coerce to radians → unitless;
`atan`/`asin`/`acos` take unitless → `deg`; `abs` preserves units.

### `SassColor`

```rust
pub struct SassColor { space: ColorSpace, channel0: f64, channel1: f64, channel2: f64, alpha: f64, missing: u8, format: Option<ColorFormat> }
```

- `ColorSpace` is a 17-variant enum (three legacy `rgb`/`hsl`/`hwb`, four Lab-like
  `lab`/`lch`/`oklab`/`oklch`, and ten modern spaces).
- Missing channels are `0.0` with a `missing: u8` bitmask; the bitmask
  propagates through every conversion.
- `ColorFormat` is `RgbFunction | Preserved(String)` — `Preserved` stores the
  original source text (Dart's `SpanColorFormat`), which is why a literal
  `#f00` round-trips while `RgbFunction` forces `rgb()` output.
- Named colors (148) use a reverse-alphabetical `LazyLock<HashMap>` so
  **last-write-wins** ("gray" overwrites "grey").
- Two Dart-replicated behaviors: negative saturation/chroma shifts the hue 180°
  and negates the value; and the LCH/OKLCH hue `a`/`b` channels are computed
  non-null even when both chroma and hue are missing.
- Interpolation (`interpolate` → `forSpaceInternal`): the mixed LCH/OKLCH hue
  is normalized to `[0, 360)` like every other hue write (Dart's
  `_normalizeHue` inside `forSpaceInternal`), so a 350/10 shorter-mix yields
  `0deg`, not `360deg`. The formula is verbatim `(h % 360 + 360) % 360` —
  the intermediate `+360` loses ~1 ULP for already-positive hues and must be
  preserved (see `ref/math.md` condition 4). `ie-hex-str` rounds each component with `fuzzy_round`
  (Dart's `fuzzyRound` in `_ieHexStr`), not plain `round()`.
- `channel_by_name`'s unknown-channel fallback ends with `.` (Dart's
  `doesn't have a channel named "$channel".`); `change_hwb` keeps Dart's
  `changeHsl` typo verbatim in its legacy-only error. The legacy `hash_code`
  hashes the missing-aware getters (missing → `0.0`, Dart's `?? 0`), not the
  stored channel values. `InterpolationMethod::from_value` mirrors Dart's
  `assertCommonListStyle(name, allowSlash: false)` separator shapes, names the
  space-element unquoted check, and leaves hue-element checks bare (per
  observed attribution); `GamutMapMethod::from_name_with_arg` threads the
  argument name for unknown methods.

### `SassCalculation`

`{ name: String, arguments: Vec<CalcArgument> }` with `CalcArgument =
Number | Calculation | String(text, quoted) | Interpolation | Operation`. Thirty-plus
constructors (`new_calc`, `new_min`, `new_max`, `new_clamp`, …) and a
construct-time `simplify()` engine; `operate` folds sign bits. A `Cell<Option<usize>>`
caches the hash.

Error layering mirrors Dart's throw-site discipline: `verify_compatible_numbers`
throws the **unspanned** `Script` error (like Dart's `SassScriptException` from
`_verifyCompatibleNumbers`), and the eval caller attaches spans —
`expression_to_calc_argument`'s `BinaryOperation` arm wraps `operate_internal`
in `add_exception_span` (the whole-operation span, mirroring Dart's
`_addExceptionSpan` in `_visitCalculationExpression`), while `_visitCalculation`
re-verifies against the original argument nodes for per-operand `MultiSpan`
labels. Both message and label interpolation go through
`SassNumber.to_display_string()` (Dart `SassNumber.toString` — precision, fuzzy
integers, `-0`), never Rust `{}` on `f64`. `_verifyLength` exempts any string
flavor (`String | Interpolation`, mirroring Dart's `arg is SassString`), and
`hypot` threads `numbers[i+1]`/`numbers[1]` attribution into
`convert_value_to_match`.

### `SassString`, `SassList`, `SassArgumentList`, `SassMap`

- `SassString { text: &'parse str, has_quotes }` — `text` is arena-backed (a
  slice of the source file or an arena-built temporary). `quote_inner_text`
  iterates `chars` (Dart iterates UTF-16 units), so non-ASCII text survives
  the plain-CSS `@import url()` re-quoting path unmangled.
- `SassList { contents: Vec<Value>, separator: ListSeparator, has_brackets }`;
  `ListSeparator` is `Space | Comma | Slash | Undecided`.
- `SassArgumentList` embeds a list plus `keywords: IndexMap<String, Value>` and
  a `were_keywords_accessed` flag (an arena `&'parse Cell<bool>`, so `keywords()`
  marks it through shared references).
- `SassMap` is `IndexMap<Value, Value>`. `hash_code` combines via the
  unordered `hash::map_hash` (Dart `package:collection` `MapEquality.hash`:
  per-entry `3*key + 7*value` summed, then the Jenkins finalizer), so equal
  maps with different insertion orders hash equally.

The empty value `()` parses to an empty unbracketed `SassList` (never
`Value::Null` — `Null` and `List` are distinct variants); `map.get`/`try_map`
traverse an empty list as an empty map.

### `SassFunction` / `SassMixin`

```rust
pub struct SassFunction<'parse> { pub callable: Callable<'parse, 'parse>, compile_context: Option<CompileContext> }
```

Dart-exact: equality/hash are the callable's address identity, the serializer prints
`callable.name()`, and `compile_context` (an `Rc<()>`, compared by `Rc::ptr_eq`)
guards against using a function value across compilations. Lifetimes unify to
`<'parse, 'parse>` (same for `SassMixin`).

### Display vs CSS forms

`to_display_string()` (Dart `toString`, inspect serialization) never fails;
`to_css_string(quote)` (Dart `toCssString`) routes through the shared
serializer and raises `SassError::Script` for non-CSS values (maps,
functions, mixins, empty unbracketed lists). Callers must not conflate the
two (see `patterns.md` §6).

## Operators, assertions, and helpers

Fourteen default binary operators (`plus`, `minus`, `times`, `divided_by`,
`modulo`, `single_equals`, `greater_than`, …) and four unary operators dispatch
via `match (self, other)`. Dart-exact edges: `Number + Color` (and
`Color + Number`, `Number - Color`, `Calculation ± anything`) raise
`Undefined operation "…"`, while non-number/non-color operands concatenate
(`1 + true → 1true`, `"a" - "b" → "a"-"b"`) — `default_plus`/`default_minus`
only stringify, mirroring Dart's base `Value.plus`/`minus`.

Assertion functions return references that **borrow from the input**
(`&'v SassBoolean`, `&'v SassList`, …). The exception is `assert_map`, which
returns an **owned** `SassMap`: when the input is an empty list there is no
inner map to borrow, so an empty map is constructed. An empty
`SassArgumentList` counts as an empty map too (Dart's `SassList.assertMap` is
inherited by `SassArgumentList`) — this is what lets `load-css` treat
`$with: ()` as an explicitly empty configuration.

`sprint_any()` (Go's duck-typing helper for a `visitCalculationExpression` that
returns `any`) is **not ported**: the Rust return type is the
`EvalResult::Value | EvalResult::CalcOp` enum, so both variants already implement
their own display/serialization.

## Golden-value testing

Color and math values are pinned with **golden tests**: print the real output at
`%.16g`, hardcode it, assert `abs(got - want) < 1e-12`, and delete the printer.
Never compute the expected value from the same formula as the code under test.
See `patterns.md` §8 for the weak-assertion taxonomy this guards against.

## File mapping

| Dart                             | Go                                                  | Rust                                                 |
| -------------------------------- | --------------------------------------------------- | ---------------------------------------------------- |
| `lib/src/value/*.dart`           | `go/value/value.go`, `number.go`, `color*.go`, …    | `src/value/*.rs`                                     |
| `lib/src/value/calculation.dart` | `go/value/calculation.go`                           | `src/value/calculation.rs`                           |
| `lib/src/value/color/` (spaces)  | `go/value/color_space_*.go`, `color_conversions.go` | `src/value/color_space_*.rs`, `color_conversions.rs` |
