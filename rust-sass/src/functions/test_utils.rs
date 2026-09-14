// Shared test utilities for functions/ tests.
// dart-source: (test helpers, not present in Dart)
// go-source: (test helpers, not present in Go)

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::statement::CallableDeclaration;
use crate::ast::sass::statement::ContentRule;
use crate::ast::sass::statement::FunctionRule;
use crate::ast::sass::statement::MixinRule;
use crate::ast::sass::statement::Statement;
use crate::callable::BuiltInCallback;
use crate::callable::UserDefinedCallable;
use crate::common::file_span::FileSpan;
use crate::compile_context::new_compile_context;
use crate::environment::Environment;
use crate::io::VirtualIo;
use crate::value::SassBoolean;
use std::collections::HashSet;
use std::rc::Rc;

use bumpalo::Bump;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::parameter_list::ParameterList;
use crate::callable::{BuiltInCallable, Callable, CallableKind};
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::BOGUS_SPAN;
use crate::eval::{EvalConfig, EvalState};
use crate::logger::test_utils::RecordLogger;
use crate::value::Value;
use crate::value::{color::ColorSpace, SassColor, SassNumber, SassString, ValueKind};

/// Builds a no-op built-in `Callable` with the given name. The `Rc` inside
/// `Callable` provides identity — two calls with the same name produce
/// distinct callables (Go: distinct `testMixinRef` pointers).
pub fn test_callable<'compile, 'parse>(
    arena: &'compile Bump,
    name: &str,
) -> Callable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    Callable::new(
        arena,
        CallableKind::BuiltIn(BuiltInCallable::new(
            name.to_string(),
            ParameterList::empty(BOGUS_SPAN),
            Rc::new(
                |_config: &EvalConfig<'compile, 'parse>,
                 _state: &mut EvalState<'compile, 'parse>,
                 _args: Vec<Value<'parse>>,
                 arena: &'compile Bump|
                 -> SassResult<Value<'parse>> {
                    Ok(Value::new_with_arena(arena, ValueKind::Null))
                },
            ),
        )),
    )
}

/// Builds a built-in mixin callable with the given accepts-content flag.
/// Mirrors Go: metaFunction(...) + SetAcceptsContent in meta_test.go.
pub fn test_built_in_mixin<'compile, 'parse>(
    arena: &'compile Bump,
    accepts_content: bool,
) -> Callable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let mut bic = BuiltInCallable::new(
        "m".to_string(),
        ParameterList::empty(BOGUS_SPAN),
        Rc::new(
            |_config: &EvalConfig<'compile, 'parse>,
             _state: &mut EvalState<'compile, 'parse>,
             _args: Vec<Value<'parse>>,
             arena: &'compile Bump|
             -> SassResult<Value<'parse>> {
                Ok(Value::new_with_arena(arena, ValueKind::Null))
            },
        ),
    );
    bic.set_accepts_content(accepts_content);
    Callable::new(arena, CallableKind::BuiltIn(bic))
}

/// Builds a user-defined mixin callable whose declaration optionally
/// contains an `@content` rule. Mirrors Go: NewUserDefinedCallable with a
/// MixinRule declaration in meta_test.go.
pub fn test_user_defined_mixin<'compile, 'parse>(
    arena: &'compile Bump,
    has_content: bool,
) -> Callable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let children = if has_content {
        vec![Statement::ContentRule(ContentRule::new(
            ArgumentList::empty(BOGUS_SPAN),
            BOGUS_SPAN,
        ))]
    } else {
        vec![]
    };
    Callable::new(
        arena,
        CallableKind::UserDefined(UserDefinedCallable::new(
            CallableDeclaration::Mixin(MixinRule::new(
                "m".to_string(),
                ParameterList::empty(BOGUS_SPAN),
                children,
                BOGUS_SPAN,
                None,
            )),
            Environment::new(arena),
            false,
        )),
    )
}

/// Builds a user-defined callable whose declaration is a FunctionRule —
/// only constructible in tests (real Sass can't wrap a function callable in
/// a mixin value).
pub fn test_user_defined_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> Callable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    Callable::new(
        arena,
        CallableKind::UserDefined(UserDefinedCallable::new(
            CallableDeclaration::Function(FunctionRule::new(
                "f".to_string(),
                ParameterList::empty(BOGUS_SPAN),
                vec![],
                BOGUS_SPAN,
                None,
            )),
            Environment::new(arena),
            false,
        )),
    )
}

/// Build a unitless number value.
pub fn num<'compile: 'parse, 'parse>(arena: &'compile Bump, v: f64) -> Value<'parse> {
    Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(v, None)))
}

/// Build a percentage number value.
pub fn percent<'compile: 'parse, 'parse>(arena: &'compile Bump, v: f64) -> Value<'parse> {
    Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(v, Some("%"))))
}

/// Build an unquoted string value.
pub fn sass_string<'compile: 'parse, 'parse>(arena: &'compile Bump, v: &str) -> Value<'parse> {
    Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(v), false)),
    )
}

/// Build a quoted string value.
pub fn quoted<'compile: 'parse, 'parse>(arena: &'compile Bump, v: &str) -> Value<'parse> {
    Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(v), true)),
    )
}

/// Build an RGB color.
pub fn color_rgb<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    r: f64,
    g: f64,
    b: f64,
    a: f64,
) -> Value<'parse> {
    Value::new_with_arena(arena, ValueKind::Color(SassColor::rgb(r, g, b, a)))
}

/// Build an sRGB color (0-1 channel range).
pub fn color_srgb<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    r: f64,
    g: f64,
    b: f64,
    a: f64,
) -> Value<'parse> {
    Value::new_with_arena(
        arena,
        ValueKind::Color(SassColor::srgb(r, g, b, a).unwrap()),
    )
}

/// Build an HSL color.
pub fn color_hsl<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    h: f64,
    s: f64,
    l: f64,
    a: f64,
) -> Value<'parse> {
    Value::new_with_arena(arena, ValueKind::Color(SassColor::hsl(h, s, l, a).unwrap()))
}

/// Build an HWB color.
pub fn color_hwb<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    h: f64,
    w: f64,
    bl: f64,
    a: f64,
) -> Value<'parse> {
    Value::new_with_arena(
        arena,
        ValueKind::Color(SassColor::hwb(h, w, bl, a).unwrap()),
    )
}

/// Build a Lab color.
pub fn color_lab<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    l: f64,
    a_val: f64,
    b_val: f64,
    alpha: f64,
) -> Value<'parse> {
    Value::new_with_arena(
        arena,
        ValueKind::Color(SassColor::lab(l, a_val, b_val, alpha).unwrap()),
    )
}

/// Build an LCH color.
pub fn color_lch<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    l: f64,
    c: f64,
    h: f64,
    alpha: f64,
) -> Value<'parse> {
    Value::new_with_arena(
        arena,
        ValueKind::Color(SassColor::lch(l, c, h, alpha).unwrap()),
    )
}

/// Build an XYZ D65 color.
pub fn color_xyz<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    x: f64,
    y: f64,
    z: f64,
    a: f64,
) -> Value<'parse> {
    Value::new_with_arena(
        arena,
        ValueKind::Color(SassColor::xyz_d65(x, y, z, a).unwrap()),
    )
}

/// Build a color in a specific space with optional missing channels.
pub fn color_for_space<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    space: ColorSpace,
    c0: f64,
    c1: f64,
    c2: f64,
    a: f64,
    missing: [bool; 4],
) -> Value<'parse> {
    Value::new_with_arena(
        arena,
        ValueKind::Color(SassColor::for_space(space, [c0, c1, c2], a, missing)),
    )
}

/// Shared eval helper for calling built-in functions in tests.
/// Warnings are recorded (and discarded); use `eval_recorded` to assert them.
#[rust_sass_macros::maybe_async]
pub async fn eval<'compile, 'parse>(
    arena: &'compile Bump,
    bic: &BuiltInCallable<'compile, 'parse>,
    args: &[Value<'parse>],
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    eval_recorded(arena, bic, args).await.0
}

/// Converts a value-shaped default-value Expression (plain string/number/
/// boolean/null, as used in builtin signatures like `$separator: auto`) to
/// a Value. Returns None for computed defaults (variables, calls, ...),
/// which have no meaning outside a real evaluation.
fn expression_default_value<'parse>(
    arena: &'parse bumpalo::Bump,
    expr: &Expression<'parse>,
) -> Option<Value<'parse>> {
    let kind = match expr {
        Expression::String(s) if s.text.is_plain() => ValueKind::String(SassString::new(
            arena.alloc_str(s.text.initial_plain()),
            s.has_quotes,
        )),
        Expression::Number(n) if n.unit.is_none() => {
            ValueKind::Number(SassNumber::new(n.value, None))
        }
        Expression::Boolean(b) => ValueKind::Boolean(SassBoolean::new(b.value)),
        Expression::Null(_) => ValueKind::Null,
        _ => return None,
    };
    Some(Value::new_with_arena(arena, kind))
}

/// Like `eval`, but returns the RecordLogger so tests can assert emitted
/// warnings. Mirrors Go: mathEvalWarn.
#[rust_sass_macros::maybe_async]
pub async fn eval_recorded<'compile, 'parse>(
    arena: &'compile Bump,
    bic: &BuiltInCallable<'compile, 'parse>,
    args: &[Value<'parse>],
) -> (SassResult<Value<'parse>>, Rc<RecordLogger>)
where
    'compile: 'parse,
    'parse: 'compile,
{
    let logger = Rc::new(RecordLogger::new());
    let names = HashSet::new();
    let overload = match bic.callback_for(args.len(), &names) {
        Ok(o) => o,
        Err(e) => return (Err(e), logger),
    };
    let mut padded = args.to_vec();
    while padded.len() < overload.params.parameters.len() {
        // Fill omitted args from declared defaults (like the real call path
        // in eval/expression.rs + eval/helpers.rs) so callbacks observe the
        // same values as production — notably `$separator: auto`, not Null.
        // NOTE: defaults are Expressions needing evaluation; only
        // value-shaped defaults (plain strings/bools/numbers/null) are
        // supported here — complex defaults fall back to Null, matching the
        // old behavior for those cases.
        let param = &overload.params.parameters[padded.len()];
        padded.push(match &param.default_value {
            None => Value::new_with_arena(arena, ValueKind::Null),
            Some(d) => expression_default_value(arena, d)
                .unwrap_or_else(|| Value::new_with_arena(arena, ValueKind::Null)),
        });
    }
    let mut state = EvalState::new(arena, FileSpan::new(None, 0, 0));
    let config = EvalConfig::new(
        arena,
        logger.clone(),
        new_compile_context(),
        Rc::new(VirtualIo::new()),
    );
    let result = invoke_callback(&overload.callback, &config, &mut state, padded, arena).await;
    (result, logger)
}

/// Assert that no warnings were recorded.
pub fn assert_no_warnings(logger: &RecordLogger) {
    assert!(
        logger.messages().is_empty(),
        "warnings = {:?}, want none",
        logger.messages()
    );
}

/// Assert exactly one warning with the given message and deprecation ID
/// (None for a plain warning).
pub fn assert_single_warning(logger: &RecordLogger, want: &str, dep_id: Option<&str>) {
    let messages = logger.messages();
    assert_eq!(messages.len(), 1, "warnings = {messages:?}, want 1");
    assert_eq!(messages[0], want);
    let deps = logger.deprecation_ids();
    assert_eq!(deps[0].as_deref(), dep_id);
}

/// Assert that an error is a Script error with the given message and argument name.
pub fn assert_script_err(err: Box<SassError>, want_msg: &str, want_arg: Option<&str>) {
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

/// Assert a Boolean value is true.
pub fn assert_is_true(got: &Value<'_>) {
    match &**got {
        ValueKind::Boolean(b) => assert!(b.value, "want true, got false"),
        _ => panic!("want Boolean true, got {got:?}"),
    }
}

/// Assert a Boolean value is false.
pub fn assert_is_false(got: &Value<'_>) {
    match &**got {
        ValueKind::Boolean(b) => assert!(!b.value, "want false, got true"),
        _ => panic!("want Boolean false, got {got:?}"),
    }
}

/// Assert an unquoted string value matches the expected text.
pub fn assert_str(got: &Value<'_>, want: &str) {
    match &**got {
        ValueKind::String(s) => {
            assert_eq!(s.text, want, "string text mismatch");
            assert!(!s.has_quotes, "expected unquoted string");
        }
        _ => panic!("want String, got {got:?}"),
    }
}

/// Assert a number value is approximately the expected value.
pub fn assert_num(got: &Value<'_>, want: f64) {
    match &**got {
        ValueKind::Number(n) => assert!(
            (n.value - want).abs() < f64::EPSILON * 1000.0,
            "num = {}, want {}",
            n.value,
            want,
        ),
        _ => panic!("want Number, got {got:?}"),
    }
}

/// Mode-agnostic built-in callback invocation for tests: matches the
/// `Sync | Async` enum in async builds, calls directly in sync builds.
#[rust_sass_macros::maybe_async]
pub(crate) async fn invoke_callback<'compile, 'parse>(
    callback: &BuiltInCallback<'compile, 'parse>,
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile bumpalo::Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    #[cfg(feature = "async")]
    match callback {
        BuiltInCallback::Sync(cb) => cb(config, state, args, arena),
        BuiltInCallback::Async(cb) => cb(config, state, args, arena).await,
    }
    #[cfg(not(feature = "async"))]
    {
        callback(config, state, args, arena)
    }
}
