// Copyright 2017 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/callable/async_built_in.dart (warnForGlobalBuiltIn) + lib/src/callable/built_in.dart (BuiltInCallable.withDeprecationWarning) + lib/src/util/number.dart (clampLikeCss) + lib/src/functions/color.dart (_functionString, _isNone)
// go-source: go/functions/color_helpers.go (warnForGlobalBuiltIn, functionString, clampLikeCSS, isNone)

use bumpalo::Bump;

use crate::common::exception::SassResult;
use crate::deprecation::GLOBAL_BUILTIN;
use crate::eval::warn::warn_deprecation;
use crate::eval::{EvalConfig, EvalState};
use crate::util::number;
use crate::value::{SassString, Value, ValueKind};

// === Deprecation ===

/// Emits the `global-builtin` deprecation warning for a global built-in that
/// is now available as `module.name`.
///
/// Matches Dart's `warnForGlobalBuiltIn(module, name)` in
/// `callable/async_built_in.dart` (re-exported through `callable.dart`; the
/// sync `BuiltInCallable.withDeprecationWarning` wrapper in
/// `callable/built_in.dart` calls the same function).
pub fn warn_for_global_builtin<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    module: &str,
    name: &str,
) -> SassResult<()>
where
    'compile: 'parse,
{
    warn_for_global_builtin_message(
        config,
        state,
        &format!(
            "Global built-in functions are deprecated and will be removed in \
             Dart Sass 3.0.0.\nUse {module}.{name} instead.\n\n\
             More info and automated migrator: https://sass-lang.com/d/import"
        ),
    )
}
/// Same warning with a precomputed message.
///
/// Hot-path variant: the `with_deprecation_warning` wrapper formats the
/// message once at registration, so per-invocation work is just the warn
/// call below rather than re-formatting.
pub fn warn_for_global_builtin_message<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    message: &str,
) -> SassResult<()>
where
    'compile: 'parse,
{
    warn_deprecation(config, state, message, &GLOBAL_BUILTIN)
}

// === Simple helpers ===

/// Builds a CSS function-call string like `name(arg1, arg2, ...)`.
///
/// Returns an unquoted string, as though the call were plain CSS. Matches
/// Dart's private `_functionString()` in `functions/color.dart` (used for
/// legacy fallbacks when an argument can only resolve at browse time).
pub fn function_string<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    name: &str,
    args: &[Value<'_>],
) -> SassResult<Value<'parse>> {
    let parts: SassResult<Vec<String>> = args.iter().map(|v| v.to_css_string(true)).collect();
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(
            arena.alloc_str(&format!("{}({})", name, parts?.join(", "))),
            false,
        )),
    ))
}

/// Clamps `val` to `[min, max]`, with `NaN` preferring the lower bound.
///
/// Matches Dart's `clampLikeCss()` in `util/number.dart`: unlike Dart's
/// `num.clamp` (which prefers the upper bound for `NaN`), CSS clamping
/// semantics send `NaN` to the lower bound.
pub fn clamp_like_css(val: f64, min: f64, max: f64) -> f64 {
    number::clamp_like_css(val, min, max)
}

/// Rounds `v` half away from zero, as `i64`.
///
/// Rust-side convenience (no direct Dart counterpart; Dart call sites use
/// `double.round()` inline, e.g. hex serialization and `ie-hex-str`).
pub fn round_int(v: f64) -> i64 {
    v.round() as i64
}

/// Reports whether `val` is the unquoted string `"none"`
/// (case-insensitive).
///
/// Matches Dart's private `_isNone()` in `functions/color.dart` (used to
/// detect missing channel values while parsing modern color syntax).
pub fn is_none(val: &Value<'_>) -> bool {
    match &**val {
        ValueKind::String(s) => !s.has_quotes && s.text.eq_ignore_ascii_case("none"),
        _ => false,
    }
}

/// Returns a random integer in `[0, max)`.
///
/// Backs `math.random($limit)`; matches Dart's `_random.nextInt()` on the
/// shared `math.Random()` in `functions/math.dart`.
pub fn random_int(max: i64) -> i64 {
    rand::random_range(0..max)
}

/// Returns a random float in `[0, 1)`.
///
/// Backs bare `math.random()`; matches Dart's `_random.nextDouble()` on the
/// shared `math.Random()` in `functions/math.dart`.
pub fn random_float() -> f64 {
    rand::random_range(0.0..1.0)
}
