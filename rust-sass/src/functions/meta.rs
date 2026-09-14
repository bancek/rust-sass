// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/functions/meta.dart
// go-source: go/functions/meta.go

use crate::eval::warn::warn_deprecation;
use crate::serialize::serialize_value_inspect;
use std::rc::Rc;

use bumpalo::Bump;

use crate::ast::sass::statement::CallableDeclaration;
use crate::callable::{BuiltInCallable, Callable, CallableKind, SyncBuiltInCallback};
use crate::common::exception::{SassError, SassResult};
use crate::deprecation::FEATURE_EXISTS;
use crate::eval::{EvalConfig, EvalState};
use crate::module::BuiltInModule;
use crate::value::{
    assert_calculation, assert_mixin, assert_string, CalcArgument, ListSeparator, SassList,
    SassMap, SassString, Value, ValueKind, SASS_FALSE, SASS_TRUE,
};

/// Like [`BuiltInCallable::function`], but pins the module URL to
/// `"sass:meta"`.
///
/// Matches Dart's private `_function()` helper in `meta.dart`.
fn meta_function<'compile, 'parse>(
    name: &str,
    parameters: &str,
    arena: &'compile Bump,
    callback: SyncBuiltInCallback<'compile, 'parse>,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(name, parameters, "sass:meta", arena, callback)
}

/// Sass introspection functions that exist both as globals and in the
/// `sass:meta` module, and that need no evaluator state.
///
/// This is only a partial list of the `sass:meta` members: the rest
/// (`variable-exists`, `function-exists`, `get-function`, `load-css`, …)
/// need access to the environment or other runtime state, so the evaluator
/// registers them itself (see `eval/meta.rs`).
///
/// The returned globals carry a `meta` deprecation warning, matching Dart's
/// `meta.global` (`_shared` + `withDeprecationWarning('meta')`).
pub fn shared_meta_functions<'compile, 'parse>(
    arena: &'compile Bump,
) -> Vec<Callable<'compile, 'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    macro_rules! c {
        ($arena:expr, $f:expr) => {
            Callable::new($arena, CallableKind::BuiltIn($f))
        };
    }
    vec![
        c!(
            arena,
            feature_exists_function(arena).with_deprecation_warning("meta", None)
        ),
        c!(
            arena,
            inspect_function(arena).with_deprecation_warning("meta", None)
        ),
        c!(
            arena,
            type_of_function(arena).with_deprecation_warning("meta", None)
        ),
        c!(
            arena,
            keywords_function(arena).with_deprecation_warning("meta", None)
        ),
    ]
}

/// The `sass:meta` module's non-evaluator-owned members: the shared
/// introspection functions plus the module-only `calc-name`, `calc-args` and
/// `accepts-content`.
///
/// The evaluators's `meta` module additionally includes the runtime-state
/// functions (see [`shared_meta_functions`]); those are assembled in
/// `eval/meta.rs`, so they are not listed here.
///
/// Matches Dart: `meta.moduleFunctions` (`..._shared` + the three
/// module-only `_function()` entries).
pub fn meta_module<'compile, 'parse>(arena: &'compile Bump) -> BuiltInModule<'compile, 'parse>
where
    'compile: 'parse,
{
    macro_rules! c {
        ($arena:expr, $f:expr) => {
            Callable::new($arena, CallableKind::BuiltIn($f))
        };
    }
    let fns: Vec<Callable<'compile, 'parse>> = vec![
        c!(arena, feature_exists_function(arena)),
        c!(arena, inspect_function(arena)),
        c!(arena, type_of_function(arena)),
        c!(arena, keywords_function(arena)),
        c!(arena, calc_name_function(arena)),
        c!(arena, calc_args_function(arena)),
        c!(arena, accepts_content_function(arena)),
    ];
    BuiltInModule::new(arena, "meta".into(), &fns, &[], indexmap::IndexMap::new())
}

/// Feature names `feature-exists()` recognizes.
///
/// Matches Dart's private `_features` set in `meta.dart`.
const SASS_FEATURES: &[&str] = &[
    "global-variable-shadowing",
    "extend-selector-pseudoclass",
    "units-level-3",
    "at-error",
    "custom-property",
];

fn feature_exists_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    warn_deprecation(config, state,
        "The feature-exists() function is deprecated.\n\nMore info: https://sass-lang.com/d/feature-exists",
        &FEATURE_EXISTS,
    )?;
    let feature = assert_string(&args[0], Some("feature"))?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Boolean(if SASS_FEATURES.contains(&feature.text) {
            SASS_TRUE
        } else {
            SASS_FALSE
        }),
    ))
}

/// Creates the `meta.feature-exists` callable (`$feature`).
///
/// Warns `feature-exists` deprecation first, then reports whether `$feature`
/// names a supported feature ([`SASS_FEATURES`]).
fn feature_exists_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    meta_function(
        "feature-exists",
        "$feature",
        arena,
        Rc::new(feature_exists_impl),
    )
}

fn inspect_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let text = serialize_value_inspect(&args[0])?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(&text), false)),
    ))
}

/// Creates the `meta.inspect` callable (`$value`).
///
/// Returns `$value` serialized with `inspect: true` as an unquoted string.
fn inspect_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    meta_function("inspect", "$value", arena, Rc::new(inspect_impl))
}

fn type_of_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let type_name = match &*args[0] {
        ValueKind::ArgumentList(_) => "arglist",
        ValueKind::Boolean(_) => "bool",
        ValueKind::Color(_) => "color",
        ValueKind::List(_) => "list",
        ValueKind::Map(_) => "map",
        ValueKind::Null => "null",
        ValueKind::Number(_) => "number",
        ValueKind::Function(_) => "function",
        ValueKind::Mixin(_) => "mixin",
        ValueKind::Calculation(_) => "calculation",
        ValueKind::String(_) => "string",
    };
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(type_name), false)),
    ))
}

/// Creates the `meta.type-of` callable (`$value`).
///
/// Returns the value's type name (`"arglist"`, `"bool"`, `"color"`,
/// `"calculation"`, …) as an unquoted string.
fn type_of_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    meta_function("type-of", "$value", arena, Rc::new(type_of_impl))
}

fn keywords_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    match &*args[0] {
        ValueKind::ArgumentList(arg_list) => {
            let mut m = SassMap::empty();
            for (key, val) in arg_list.keywords() {
                m.set(
                    Value::new_with_arena(
                        arena,
                        ValueKind::String(SassString::new(arena.alloc_str(key), false)),
                    ),
                    *val,
                );
            }
            Ok(Value::new_with_arena(arena, ValueKind::Map(m)))
        }
        other => {
            let arg_str = other.to_display_string()?;
            Err(Box::new(SassError::Script {
                message: format!("{} is not an argument list.", arg_str),
                argument_name: Some("args".into()),
            }))
        }
    }
}

/// Creates the `meta.keywords` callable (`$args`).
///
/// Collects the keyword arguments of an argument list into a map with
/// unquoted-string keys; anything else is a `$args:` script error.
fn keywords_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    meta_function("keywords", "$args", arena, Rc::new(keywords_impl))
}

// ---- Module-only functions ----

fn calc_name_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let calc = assert_calculation(&args[0], Some("calc"))?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(&calc.name), true)),
    ))
}

/// Creates the module-only `meta.calc-name` callable (`$calc`).
///
/// Returns the calculation's name as a quoted string.
fn calc_name_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    meta_function("calc-name", "$calc", arena, Rc::new(calc_name_impl))
}

fn calc_args_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let calc = assert_calculation(&args[0], Some("calc"))?;
    let mut result: Vec<Value<'parse>> = Vec::with_capacity(calc.arguments.len());
    for arg in &calc.arguments {
        // Value-typed arguments pass through unchanged; other arguments are
        // stringified (Go: fmt "%v" via String(), Dart: argument.toString()).
        result.push(match arg {
            CalcArgument::Number(n) => Value::new_with_arena(arena, ValueKind::Number(n.clone())),
            CalcArgument::Calculation(c) => {
                Value::new_with_arena(arena, ValueKind::Calculation(c.clone()))
            }
            CalcArgument::String(s, has_quotes) => Value::new_with_arena(
                arena,
                ValueKind::String(SassString::new(arena.alloc_str(s), *has_quotes)),
            ),
            CalcArgument::Operation(op) => Value::new_with_arena(
                arena,
                ValueKind::String(SassString::new(
                    arena.alloc_str(&op.to_display_string()?),
                    false,
                )),
            ),
            CalcArgument::Interpolation(v) => Value::new_with_arena(
                arena,
                ValueKind::String(SassString::new(arena.alloc_str(v), false)),
            ),
        });
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::List(SassList::new(result, ListSeparator::Comma, false)),
    ))
}

/// Creates the module-only `meta.calc-args` callable (`$calc`).
///
/// Returns the calculation's arguments as a comma-separated list: [`Value`]
/// arguments pass through unchanged, anything else is stringified into an
/// unquoted string (Dart's `argument.toString()`).
fn calc_args_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    meta_function("calc-args", "$calc", arena, Rc::new(calc_args_impl))
}

fn accepts_content_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let mixin = assert_mixin(&args[0], Some("mixin"))?;
    // Matches Dart: switch (mixin.callable) — BuiltInCallable answers via
    // acceptsContent, UserDefinedCallable via `declaration is MixinRule` +
    // hasContent, anything else throws UnsupportedError. (Go diverges for a
    // non-mixin UserDefinedCallable — it returns false via its isMixin flag;
    // see go-discrepancies.md #13. Both arms are unreachable via real Sass
    // code because assert_mixin guarantees a mixin value.)
    let accepts = match mixin.callable.kind() {
        CallableKind::BuiltIn(c) => c.accepts_content(),
        CallableKind::UserDefined(c) => match &c.declaration {
            CallableDeclaration::Mixin(m) => m.has_content(),
            _ => panic!("Unknown callable type UserDefinedCallable."),
        },
        CallableKind::PlainCss(_) => panic!("Unknown callable type PlainCssCallable."),
    };
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Boolean(if accepts { SASS_TRUE } else { SASS_FALSE }),
    ))
}

/// Creates the module-only `meta.accepts-content` callable (`$mixin`).
///
/// Reports whether the mixin accepts an `@content` block: built-ins answer
/// via `accepts_content`, user-defined mixins via their declaration's
/// `has_content`. (A non-mixin user-defined declaration is unreachable
/// through real Sass code — `assert_mixin` rejects it first — and panics,
/// matching Dart's `UnsupportedError` arm.)
fn accepts_content_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    meta_function(
        "accepts-content",
        "$mixin",
        arena,
        Rc::new(accepts_content_impl),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::serialize_value_inspect;

    use indexmap::IndexMap;

    use crate::functions::test_utils::{
        assert_is_false, assert_is_true, eval, eval_recorded, test_built_in_mixin, test_callable,
        test_user_defined_function, test_user_defined_mixin,
    };
    use crate::logger::test_utils::RecordLogger;
    use crate::value::{
        CalculationOperation, CalculationOperator, SassArgumentList, SassCalculation, SassColor,
        SassFunction, SassMixin, SassNumber,
    };

    // --- helpers ---

    /// Mirrors Go: value.SerializeValueInspect (assertInspect in meta_test.go).
    fn inspect(v: &Value<'_>) -> String {
        serialize_value_inspect(v).unwrap()
    }

    fn assert_inspect(got: &Value<'_>, want: &str) {
        assert_eq!(inspect(got), want);
    }

    fn str_val<'compile: 'parse, 'parse>(arena: &'compile Bump, s: &str) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(s), false)),
        )
    }

    fn quoted_val<'compile: 'parse, 'parse>(arena: &'compile Bump, s: &str) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(s), true)),
        )
    }

    fn num_val<'compile: 'parse, 'parse>(arena: &'compile Bump, v: f64) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(v, None)))
    }

    fn comma_list<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        items: &[Value<'parse>],
    ) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::List(SassList::new(items.to_vec(), ListSeparator::Comma, false)),
        )
    }

    fn map_val<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        pairs: &[(&str, f64)],
    ) -> Value<'parse> {
        let mut m = SassMap::empty();
        for (k, v) in pairs {
            m.set(
                Value::new_with_arena(
                    arena,
                    ValueKind::String(SassString::new(arena.alloc_str(k), false)),
                ),
                Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(*v, None))),
            );
        }
        Value::new_with_arena(arena, ValueKind::Map(m))
    }

    fn mk_calc<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        name: &str,
        args: Vec<CalcArgument>,
    ) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(name, args))),
        )
    }

    fn arg_list_with_keywords<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        pairs: &[(&str, f64)],
    ) -> Value<'parse> {
        let mut keywords = IndexMap::new();
        for (k, v) in pairs {
            keywords.insert(
                k.to_string(),
                Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(*v, None))),
            );
        }
        Value::new_with_arena(
            arena,
            ValueKind::ArgumentList(SassArgumentList::new(
                arena,
                vec![],
                keywords,
                ListSeparator::Comma,
            )),
        )
    }

    /// Asserts err is a Script error with the exact message and argument name.
    fn assert_script_err(err: Box<SassError>, want_msg: &str, want_arg: Option<&str>) {
        match *err {
            SassError::Script {
                message,
                argument_name,
            } => {
                assert_eq!(message, want_msg);
                assert_eq!(argument_name.as_deref(), want_arg);
            }
            other => panic!("expected Script error, got {other:?}"),
        }
    }

    fn assert_unquoted_string(got: &Value<'_>, want: &str) {
        match &**got {
            ValueKind::String(s) => {
                assert_eq!(s.text, want);
                assert!(!s.has_quotes, "expected unquoted string");
            }
            other => panic!("want String, got {other:?}"),
        }
    }

    fn assert_quoted_string(got: &Value<'_>, want: &str) {
        match &**got {
            ValueKind::String(s) => {
                assert_eq!(s.text, want);
                assert!(s.has_quotes, "expected quoted string");
            }
            other => panic!("want String, got {other:?}"),
        }
    }

    fn meta_global_builtin_warning_msg(name: &str) -> String {
        [
            "Global built-in functions are deprecated and will be removed in Dart Sass 3.0.0.",
            &format!("Use meta.{name} instead."),
            "",
            "More info and automated migrator: https://sass-lang.com/d/import",
        ]
        .join("\n")
    }

    fn feature_exists_warning_msg() -> String {
        [
            "The feature-exists() function is deprecated.",
            "",
            "More info: https://sass-lang.com/d/feature-exists",
        ]
        .join("\n")
    }

    fn assert_single_warning(logger: &RecordLogger, want: &str, dep_id: &str) {
        let messages = logger.messages();
        assert_eq!(messages.len(), 1, "warnings = {messages:?}, want 1");
        assert_eq!(messages[0], want);
        assert_eq!(logger.deprecation_ids()[0].as_deref(), Some(dep_id));
    }

    fn shared_bic<'compile, 'parse>(
        arena: &'compile Bump,
        index: usize,
    ) -> BuiltInCallable<'compile, 'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fns = shared_meta_functions(arena);
        match fns[index].kind() {
            CallableKind::BuiltIn(b) => b.clone(),
            _ => panic!("expected BuiltIn callable"),
        }
    }

    #[rust_sass_macros::maybe_async]
    async fn eval_ok<'compile, 'parse>(
        arena: &'compile Bump,
        fn_: &BuiltInCallable<'compile, 'parse>,
        args: &[Value<'parse>],
    ) -> Value<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        eval(arena, fn_, args).await.unwrap()
    }

    // --- shared_meta_functions ---

    #[rust_sass_macros::maybe_test]
    async fn test_shared_meta_functions_names() {
        let arena = Bump::new();
        let fns = shared_meta_functions(&arena);
        let want = ["feature-exists", "inspect", "type-of", "keywords"];
        assert_eq!(fns.len(), want.len());
        for (i, f) in fns.iter().enumerate() {
            assert_eq!(f.name(), want[i], "fns[{i}]");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_shared_meta_functions_deprecation_metadata() {
        let arena = Bump::new();
        let fns = shared_meta_functions(&arena);
        let want = ["feature-exists", "inspect", "type-of", "keywords"];
        for (i, f) in fns.iter().enumerate() {
            match f.kind() {
                CallableKind::BuiltIn(b) => {
                    assert_eq!(b.name(), want[i]);
                    let dw = b
                        .deprecation_warning()
                        .expect("deprecation warning should be set");
                    assert_eq!(dw.0, "meta");
                    assert_eq!(dw.1, want[i]);
                }
                _ => panic!("expected BuiltIn callable"),
            }
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_shared_meta_inspect_emits_deprecation_warning() {
        let arena = Bump::new();
        let inspect_fn = shared_bic(&arena, 1);
        let (result, logger) = eval_recorded(&arena, &inspect_fn, &[num_val(&arena, 1.0)]).await;
        assert_unquoted_string(&result.unwrap(), "1");
        assert_single_warning(
            &logger,
            &meta_global_builtin_warning_msg("inspect"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_shared_meta_type_of_emits_deprecation_warning() {
        let arena = Bump::new();
        let type_of = shared_bic(&arena, 2);
        let (result, logger) = eval_recorded(&arena, &type_of, &[num_val(&arena, 1.0)]).await;
        assert_unquoted_string(&result.unwrap(), "number");
        assert_single_warning(
            &logger,
            &meta_global_builtin_warning_msg("type-of"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_shared_meta_keywords_emits_deprecation_warning() {
        let arena = Bump::new();
        let keywords = shared_bic(&arena, 3);
        let (result, logger) = eval_recorded(
            &arena,
            &keywords,
            &[arg_list_with_keywords(&arena, &[("a", 1.0)])],
        )
        .await;
        assert_inspect(&result.unwrap(), "(a: 1)");
        assert_single_warning(
            &logger,
            &meta_global_builtin_warning_msg("keywords"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_shared_meta_feature_exists_emits_both_warnings() {
        // The wrapper emits GlobalBuiltin first, then the callback emits
        // FeatureExists.
        let arena = Bump::new();
        let feature_exists = shared_bic(&arena, 0);
        let (result, logger) =
            eval_recorded(&arena, &feature_exists, &[str_val(&arena, "at-error")]).await;
        assert_is_true(&result.unwrap());
        let messages = logger.messages();
        assert_eq!(messages.len(), 2, "warnings = {messages:?}, want 2");
        assert_eq!(
            messages[0],
            meta_global_builtin_warning_msg("feature-exists")
        );
        assert_eq!(messages[1], feature_exists_warning_msg());
        let deps = logger.deprecation_ids();
        assert_eq!(deps[0].as_deref(), Some("global-builtin"));
        assert_eq!(deps[1].as_deref(), Some("feature-exists"));
    }

    // --- meta_module ---

    #[rust_sass_macros::maybe_test]
    async fn test_meta_module_url() {
        let arena = Bump::new();
        let m = meta_module(&arena);
        assert_eq!(m.url, "sass:meta");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_meta_module_functions() {
        let arena = Bump::new();
        let m = meta_module(&arena);
        let want = [
            "feature-exists",
            "inspect",
            "type-of",
            "keywords",
            "calc-name",
            "calc-args",
            "accepts-content",
        ];
        assert_eq!(m.functions.len(), want.len());
        for (i, name) in m.functions.keys().enumerate() {
            assert_eq!(name, want[i], "functions[{i}]");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_meta_module_no_mixins_no_variables() {
        let arena = Bump::new();
        let m = meta_module(&arena);
        assert_eq!(m.mixins.len(), 0);
        assert_eq!(m.variables.len(), 0);
    }

    // --- feature-exists ---

    #[rust_sass_macros::maybe_test]
    async fn test_feature_exists_supported_features() {
        let arena = Bump::new();
        for feature in [
            "global-variable-shadowing",
            "extend-selector-pseudoclass",
            "units-level-3",
            "at-error",
            "custom-property",
        ] {
            let (result, logger) = eval_recorded(
                &arena,
                &feature_exists_function(&arena),
                &[str_val(&arena, feature)],
            )
            .await;
            assert_is_true(&result.unwrap());
            assert_single_warning(&logger, &feature_exists_warning_msg(), "feature-exists");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_feature_exists_unknown_feature() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &feature_exists_function(&arena),
            &[str_val(&arena, "unknown-feature")],
        )
        .await;
        assert_is_false(&result.unwrap());
        assert_single_warning(&logger, &feature_exists_warning_msg(), "feature-exists");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_feature_exists_quoted_string() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &feature_exists_function(&arena),
            &[quoted_val(&arena, "at-error")],
        )
        .await;
        assert_is_true(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_feature_exists_type_error() {
        let arena = Bump::new();
        // The deprecation warning is emitted before the type check.
        let (result, logger) = eval_recorded(
            &arena,
            &feature_exists_function(&arena),
            &[num_val(&arena, 1.0)],
        )
        .await;
        assert_script_err(result.unwrap_err(), "1 is not a string.", Some("feature"));
        assert_single_warning(&logger, &feature_exists_warning_msg(), "feature-exists");
    }

    // --- inspect ---

    #[rust_sass_macros::maybe_test]
    async fn test_inspect_number() {
        let arena = Bump::new();
        let got = eval_ok(&arena, &inspect_function(&arena), &[num_val(&arena, 1.0)]).await;
        assert_unquoted_string(&got, "1");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_inspect_quoted_string() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &inspect_function(&arena),
            &[quoted_val(&arena, "foo")],
        )
        .await;
        assert_unquoted_string(&got, "\"foo\"");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_inspect_unquoted_string() {
        let arena = Bump::new();
        let got = eval_ok(&arena, &inspect_function(&arena), &[str_val(&arena, "foo")]).await;
        assert_unquoted_string(&got, "foo");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_inspect_comma_list() {
        let arena = Bump::new();
        // serialize_value_inspect — no Dart toString parens.
        let got = eval_ok(
            &arena,
            &inspect_function(&arena),
            &[comma_list(
                &arena,
                &[
                    num_val(&arena, 1.0),
                    num_val(&arena, 2.0),
                    num_val(&arena, 3.0),
                ],
            )],
        )
        .await;
        assert_unquoted_string(&got, "1, 2, 3");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_inspect_single_comma_list() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &inspect_function(&arena),
            &[comma_list(&arena, &[num_val(&arena, 1.0)])],
        )
        .await;
        assert_unquoted_string(&got, "(1,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_inspect_empty_list() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &inspect_function(&arena),
            &[comma_list(&arena, &[])],
        )
        .await;
        assert_unquoted_string(&got, "()");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_inspect_null() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &inspect_function(&arena),
            &[Value::new_with_arena(&arena, ValueKind::Null)],
        )
        .await;
        assert_unquoted_string(&got, "null");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_inspect_boolean() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &inspect_function(&arena),
            &[Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE))],
        )
        .await;
        assert_unquoted_string(&got, "true");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_inspect_map() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &inspect_function(&arena),
            &[map_val(&arena, &[("a", 1.0), ("b", 2.0)])],
        )
        .await;
        assert_unquoted_string(&got, "(a: 1, b: 2)");
    }

    // --- type-of ---

    #[rust_sass_macros::maybe_test]
    async fn test_type_of() {
        let arena = Bump::new();
        let arglist = Value::new_with_arena(
            &arena,
            ValueKind::ArgumentList(SassArgumentList::new(
                &arena,
                vec![],
                IndexMap::new(),
                ListSeparator::Comma,
            )),
        );
        let cases = vec![
            (arglist, "arglist"),
            (
                Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE)),
                "bool",
            ),
            (
                Value::new_with_arena(
                    &arena,
                    ValueKind::Color(SassColor::rgb(255.0, 0.0, 0.0, 1.0)),
                ),
                "color",
            ),
            (
                comma_list(&arena, &[num_val(&arena, 1.0), num_val(&arena, 2.0)]),
                "list",
            ),
            (map_val(&arena, &[("a", 1.0)]), "map"),
            (Value::new_with_arena(&arena, ValueKind::Null), "null"),
            (num_val(&arena, 1.0), "number"),
            (
                Value::new_with_arena(
                    &arena,
                    ValueKind::Function(SassFunction::new(test_callable(&arena, "f"))),
                ),
                "function",
            ),
            (
                Value::new_with_arena(
                    &arena,
                    ValueKind::Mixin(SassMixin::new(test_callable(&arena, "m"))),
                ),
                "mixin",
            ),
            (
                mk_calc(
                    &arena,
                    "calc",
                    vec![CalcArgument::Number(SassNumber::new(1.0, None))],
                ),
                "calculation",
            ),
            (str_val(&arena, "foo"), "string"),
            (quoted_val(&arena, "foo"), "string"),
        ];
        for (v, want) in cases {
            let got = eval_ok(&arena, &type_of_function(&arena), &[v]).await;
            assert_unquoted_string(&got, want);
        }
    }

    // --- keywords ---

    #[rust_sass_macros::maybe_test]
    async fn test_keywords() {
        let arena = Bump::new();
        let al = arg_list_with_keywords(&arena, &[("a", 1.0), ("b", 2.0)]);
        let got = eval_ok(&arena, &keywords_function(&arena), &[al]).await;
        assert_inspect(&got, "(a: 1, b: 2)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_keywords_keys_are_unquoted_strings() {
        let arena = Bump::new();
        let al = arg_list_with_keywords(&arena, &[("key", 1.0)]);
        let got = eval_ok(&arena, &keywords_function(&arena), &[al]).await;
        let ValueKind::Map(m) = &*got else {
            panic!("expected Map, got {got:?}");
        };
        assert_eq!(m.len(), 1);
        for (k, _) in &m.entries {
            assert_unquoted_string(k, "key");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_keywords_empty() {
        let arena = Bump::new();
        let al = arg_list_with_keywords(&arena, &[]);
        let got = eval_ok(&arena, &keywords_function(&arena), &[al]).await;
        assert_inspect(&got, "()");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_keywords_marks_accessed() {
        let arena = Bump::new();
        let al = arg_list_with_keywords(&arena, &[("a", 1.0)]);
        {
            let ValueKind::ArgumentList(inner) = &*al else {
                panic!("expected ArgumentList");
            };
            assert!(!inner.were_keywords_accessed.get());
        }
        eval_ok(&arena, &keywords_function(&arena), &[al]).await;
        let ValueKind::ArgumentList(inner) = &*al else {
            panic!("expected ArgumentList");
        };
        assert!(
            inner.were_keywords_accessed.get(),
            "keywords() should mark keywords as accessed"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_keywords_list_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &keywords_function(&arena),
            &[comma_list(
                &arena,
                &[num_val(&arena, 1.0), num_val(&arena, 2.0)],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "(1, 2) is not an argument list.", Some("args"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_keywords_string_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &keywords_function(&arena),
            &[str_val(&arena, "foo")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "foo is not an argument list.", Some("args"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_keywords_number_error() {
        let arena = Bump::new();
        let err = eval(&arena, &keywords_function(&arena), &[num_val(&arena, 1.0)])
            .await
            .unwrap_err();
        assert_script_err(err, "1 is not an argument list.", Some("args"));
    }

    // --- calc-name ---

    #[rust_sass_macros::maybe_test]
    async fn test_calc_name() {
        let arena = Bump::new();
        let calc = mk_calc(
            &arena,
            "calc",
            vec![CalcArgument::Number(SassNumber::new(1.0, None))],
        );
        let got = eval_ok(&arena, &calc_name_function(&arena), &[calc]).await;
        assert_quoted_string(&got, "calc");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_calc_name_max() {
        let arena = Bump::new();
        let calc = mk_calc(
            &arena,
            "max",
            vec![
                CalcArgument::Number(SassNumber::new(1.0, None)),
                CalcArgument::Number(SassNumber::new(2.0, None)),
            ],
        );
        let got = eval_ok(&arena, &calc_name_function(&arena), &[calc]).await;
        assert_quoted_string(&got, "max");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_calc_name_number_error() {
        let arena = Bump::new();
        let err = eval(&arena, &calc_name_function(&arena), &[num_val(&arena, 1.0)])
            .await
            .unwrap_err();
        assert_script_err(err, "1 is not a calculation.", Some("calc"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_calc_name_string_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &calc_name_function(&arena),
            &[str_val(&arena, "calc(1px)")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "calc(1px) is not a calculation.", Some("calc"));
    }

    // --- calc-args ---

    #[rust_sass_macros::maybe_test]
    async fn test_calc_args_numbers() {
        let arena = Bump::new();
        let calc = mk_calc(
            &arena,
            "max",
            vec![
                CalcArgument::Number(SassNumber::new(1.0, None)),
                CalcArgument::Number(SassNumber::new(2.0, Some("px"))),
            ],
        );
        let got = eval_ok(&arena, &calc_args_function(&arena), &[calc]).await;
        assert_inspect(&got, "1, 2px");
        assert_eq!(got.separator(), ListSeparator::Comma);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_calc_args_single() {
        let arena = Bump::new();
        let calc = mk_calc(
            &arena,
            "calc",
            vec![CalcArgument::Number(SassNumber::new(1.0, Some("px")))],
        );
        let got = eval_ok(&arena, &calc_args_function(&arena), &[calc]).await;
        assert_inspect(&got, "(1px,)");
        assert_eq!(got.length_as_list(), 1);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_calc_args_operation() {
        let arena = Bump::new();
        // Non-Value arguments are stringified via to_display_string
        // (Dart: argument.toString()).
        let op = CalculationOperation::new(
            CalculationOperator::Plus,
            CalcArgument::Number(SassNumber::new(1.0, Some("px"))),
            CalcArgument::Number(SassNumber::new(2.0, Some("px"))),
        );
        let calc = mk_calc(&arena, "calc", vec![CalcArgument::Operation(Box::new(op))]);
        let got = eval_ok(&arena, &calc_args_function(&arena), &[calc]).await;
        assert_eq!(got.length_as_list(), 1);
        let items = got.as_list(&arena).unwrap();
        assert_unquoted_string(&items[0], "1px + 2px");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_calc_args_interpolation() {
        let arena = Bump::new();
        // Matches Dart: CalculationInterpolation.toString() => value.
        let calc = mk_calc(
            &arena,
            "calc",
            vec![CalcArgument::Interpolation("var(--x)".to_string())],
        );
        let got = eval_ok(&arena, &calc_args_function(&arena), &[calc]).await;
        let items = got.as_list(&arena).unwrap();
        assert_unquoted_string(&items[0], "var(--x)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_calc_args_string_argument() {
        let arena = Bump::new();
        // String arguments are Values and pass through unchanged.
        let calc = mk_calc(
            &arena,
            "calc",
            vec![CalcArgument::String("100% - 10px".to_string(), false)],
        );
        let got = eval_ok(&arena, &calc_args_function(&arena), &[calc]).await;
        let items = got.as_list(&arena).unwrap();
        assert_unquoted_string(&items[0], "100% - 10px");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_calc_args_nested_calculation() {
        let arena = Bump::new();
        let inner = SassCalculation::new_unsimplified(
            "calc",
            vec![CalcArgument::Number(SassNumber::new(1.0, Some("px")))],
        );
        let calc = mk_calc(
            &arena,
            "max",
            vec![
                CalcArgument::Calculation(Box::new(inner)),
                CalcArgument::Number(SassNumber::new(2.0, Some("px"))),
            ],
        );
        let got = eval_ok(&arena, &calc_args_function(&arena), &[calc]).await;
        let items = got.as_list(&arena).unwrap();
        assert_eq!(items.len(), 2);
        assert!(
            matches!(&*items[0], ValueKind::Calculation(_)),
            "items[0] should be a Calculation, got {:?}",
            items[0]
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_calc_args_type_error() {
        let arena = Bump::new();
        let err = eval(&arena, &calc_args_function(&arena), &[num_val(&arena, 1.0)])
            .await
            .unwrap_err();
        assert_script_err(err, "1 is not a calculation.", Some("calc"));
    }

    // --- accepts-content ---

    #[rust_sass_macros::maybe_test]
    async fn test_accepts_content_built_in_true() {
        let arena = Bump::new();
        // Mirrors Go: TestAcceptsContentBuiltInTrue — a BuiltInCallable mixin
        // answers via accepts_content().
        let mixin = Value::new_with_arena(
            &arena,
            ValueKind::Mixin(SassMixin::new(test_built_in_mixin(&arena, true))),
        );
        let got = eval_ok(&arena, &accepts_content_function(&arena), &[mixin]).await;
        assert_is_true(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_accepts_content_built_in_false() {
        let arena = Bump::new();
        // Mirrors Go: TestAcceptsContentBuiltInFalse.
        let mixin = Value::new_with_arena(
            &arena,
            ValueKind::Mixin(SassMixin::new(test_built_in_mixin(&arena, false))),
        );
        let got = eval_ok(&arena, &accepts_content_function(&arena), &[mixin]).await;
        assert_is_false(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_accepts_content_user_defined_with_content() {
        let arena = Bump::new();
        // Mirrors Go: TestAcceptsContentUserDefinedWithContent — the mixin
        // declaration contains an @content rule.
        let mixin = Value::new_with_arena(
            &arena,
            ValueKind::Mixin(SassMixin::new(test_user_defined_mixin(&arena, true))),
        );
        let got = eval_ok(&arena, &accepts_content_function(&arena), &[mixin]).await;
        assert_is_true(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_accepts_content_user_defined_without_content() {
        let arena = Bump::new();
        // Mirrors Go: TestAcceptsContentUserDefinedWithoutContent.
        let mixin = Value::new_with_arena(
            &arena,
            ValueKind::Mixin(SassMixin::new(test_user_defined_mixin(&arena, false))),
        );
        let got = eval_ok(&arena, &accepts_content_function(&arena), &[mixin]).await;
        assert_is_false(&got);
    }

    #[rust_sass_macros::maybe_test]
    #[should_panic(expected = "Unknown callable type")]
    async fn test_accepts_content_non_mixin_declaration_panics() {
        let arena = Bump::new();
        // Matches Dart: a UserDefinedCallable whose declaration is not a
        // MixinRule falls through to `throw UnsupportedError`. (Go diverges
        // and returns false — go-discrepancies.md #13.) Unreachable via real
        // Sass code.
        let mixin = Value::new_with_arena(
            &arena,
            ValueKind::Mixin(SassMixin::new(test_user_defined_function(&arena))),
        );
        let _ = eval(&arena, &accepts_content_function(&arena), &[mixin]).await;
    }

    #[rust_sass_macros::maybe_test]
    async fn test_accepts_content_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &accepts_content_function(&arena),
            &[num_val(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a mixin reference.", Some("mixin"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_accepts_content_string_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &accepts_content_function(&arena),
            &[str_val(&arena, "m")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "m is not a mixin reference.", Some("mixin"));
    }
}
