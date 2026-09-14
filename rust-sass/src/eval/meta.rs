// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/evaluate.dart (meta functions section, lines 378-707)
// go-source: go/eval/evaluate_meta.go

//! Evaluator-context `sass:meta` members.
//!
//! The context-free `sass:meta` members live in [`crate::functions::meta`];
//! this module holds the members that need evaluator state ([`EvalConfig`]
//! / [`EvalState`]): the 11 functions built in `create_meta_functions` and
//! the 2 mixins built in `create_meta_mixins`, assembled into the
//! `sass:meta` module by [`register_meta_functions`].
//!
//! Matches Dart: `_EvaluateVisitor` constructor meta-functions section
//! (`evaluate.dart:386-706`) — "defined in the context of the evaluator
//! because they need access to the environment or other local state".

use crate::eval::css::evaluate_css_stylesheet;
use crate::eval::expression::invoke_callable;
use crate::eval::helpers::add_exception_trace;
use crate::eval::helpers::with_stack_frame;
use crate::eval::statement::apply_mixin;
use crate::eval::statement::combine_css;
use crate::eval::statement::load_module;
use crate::eval::warn::warn_deprecation;
use crate::functions::meta::meta_module;
use std::rc::Rc;

use bumpalo::Bump;
#[cfg(feature = "async")]
use futures::future::LocalBoxFuture;
use indexmap::IndexMap;

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression_value::ValueExpression;
#[cfg(feature = "async")]
use crate::callable::AsyncBuiltInCallback;
use crate::callable::{
    BuiltInCallable, Callable, CallableKind, PlainCssCallable, SyncBuiltInCallback,
};
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::BOGUS_SPAN;
use crate::compile_context::CompileContext;
use crate::configuration::{Configuration, ConfiguredValue};
use crate::deprecation::{CALL_STRING, WITH_PRIVATE};
use crate::eval::{EvalConfig, EvalState, EvaluateVisitor};
use crate::module::{Module, ModuleKind};
use crate::url::SassUrl;
use crate::value::{
    assert_mixin, assert_string, SassFunction, SassMap, SassMixin, SassString, Value, ValueKind,
    SASS_FALSE, SASS_TRUE,
};

/// Creates one evaluator-context `sass:meta` function.
///
/// Matches Dart: `BuiltInCallable.function(..., url: "sass:meta")`
/// (`evaluate.dart:389-599`). Every entry in `create_meta_functions` goes
/// through here so the `sass:meta` URL is set in one place.
fn meta_fn<'compile, 'parse>(
    name: &str,
    parameters: &str,
    arena: &'compile Bump,
    callback: SyncBuiltInCallback<'compile, 'parse>,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    BuiltInCallable::function(name, parameters, "sass:meta", arena, callback)
}

/// Async twin of [`meta_fn`] for the reentrant `call` entry.
///
/// Reentrant callbacks must return futures in the `async` build, so this takes
/// an `AsyncBuiltInCallback`; in the sync build the twin below takes a plain
/// `SyncBuiltInCallback` instead (one source tree, two builds — see
/// `architecture.md` §6).
#[rust_sass_macros::async_impl]
fn meta_fn_async<'compile, 'parse>(
    name: &str,
    parameters: &str,
    arena: &'compile Bump,
    callback: AsyncBuiltInCallback<'compile, 'parse>,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    BuiltInCallable::function_async(name, parameters, "sass:meta", arena, callback)
}

#[rust_sass_macros::sync_impl]
fn meta_fn_async<'compile, 'parse>(
    name: &str,
    parameters: &str,
    arena: &'compile Bump,
    callback: SyncBuiltInCallback<'compile, 'parse>,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    BuiltInCallable::function(name, parameters, "sass:meta", arena, callback)
}

/// Reentrant body of `meta.call($function, $args...)`.
///
/// Calls the given function reference with the rest-args as its arguments.
/// `SassFunction` values run through
/// [`invoke_callable`](crate::eval::expression::invoke_callable) after an
/// `assert_compile_context` check; plain strings take the deprecated path
/// (emits [`CALL_STRING`] with a `call(get-function(...))` recommendation,
/// then resolves by normalized name against the environment plus
/// `built_in_functions`, falling back to a plain-CSS callable). Anything else
/// is a `Script` error naming `$function`. Keywords travel separately: when
/// the rest arg is a [`SassArgumentList`](crate::value::SassArgumentList) with
/// keywords, they are repacked as a map `keywordRest` alongside the positional
/// `rest` (Dart: `evaluate.dart:540-562`).
///
/// Extracted from the callback closure so the body is plain syntax visible to
/// the maybe_async visitor (macro tokens like `box_rec!(...)` are opaque to
/// it); the closure only forwards arguments.
#[rust_sass_macros::maybe_async]
async fn call_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    args: Vec<Value<'parse>>,
    cctx: CompileContext,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let function = &args[0];

    // SassFunction path: assert function, assert compile context,
    // then dispatch through the evaluator's callable runner.
    match &**function {
        ValueKind::Function(f) => {
            f.assert_compile_context(&cctx)?;
            let span = state.callable_span.unwrap_or(BOGUS_SPAN);
            let rest_val = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| Value::new_with_arena(arena, ValueKind::Null));
            let rest_expr = Expression::Value(ValueExpression::new(rest_val, span));
            let arg_list = ArgumentList::new(
                vec![],
                IndexMap::new(),
                IndexMap::new(),
                span,
                Some(rest_expr),
                None,
            );
            invoke_callable(config, state, arena, &f.callable, &arg_list, span).await
        }
        ValueKind::String(s) => {
            let serialized = s.to_css_string(true)?;
            warn_deprecation(
                config,
                state,
                &format!(
                    "Passing a string to call() is deprecated and will be \
             illegal in Dart Sass 2.0.0.\n\n\
             Recommendation: call(get-function({serialized}))",
                ),
                &CALL_STRING,
            )?;
            let normalized = s.text.replace("_", "-");
            let fn_callable = state
                .env
                .get_function(&normalized, None)?
                .or_else(|| config.built_in_functions.borrow().get(&normalized).cloned())
                .unwrap_or_else(|| {
                    Callable::new(
                        arena,
                        CallableKind::PlainCss(PlainCssCallable {
                            name: s.text.to_string(),
                        }),
                    )
                });
            let span = state.callable_span.unwrap_or(BOGUS_SPAN);
            let rest_val = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| Value::new_with_arena(arena, ValueKind::Null));

            // Extract keywords from SassArgumentList (matching Go evaluate_meta.go:339-346)
            let keyword_rest = match &*rest_val {
                ValueKind::ArgumentList(al) if !al.keywords.is_empty() => {
                    let mut kw_entries: Vec<(Value<'parse>, Value<'parse>)> = Vec::new();
                    for (name, val) in &al.keywords {
                        kw_entries.push((
                            Value::new_with_arena(
                                arena,
                                ValueKind::String(SassString::new(arena.alloc_str(name), false)),
                            ),
                            *val,
                        ));
                    }
                    let kw_map = SassMap::from_entries(kw_entries);
                    Some(Expression::Value(ValueExpression::new(
                        Value::new_with_arena(arena, ValueKind::Map(kw_map)),
                        span,
                    )))
                }
                _ => None,
            };

            let rest_expr = Expression::Value(ValueExpression::new(rest_val, span));
            let arg_list = ArgumentList::new(
                vec![],
                IndexMap::new(),
                IndexMap::new(),
                span,
                Some(rest_expr),
                keyword_rest,
            );
            invoke_callable(config, state, arena, &fn_callable, &arg_list, span).await
        }
        _ => Err(Box::new(SassError::Script {
            message: format!(
                "{} is not a function reference.",
                function.to_display_string()?
            ),
            argument_name: Some("function".into()),
        })),
    }
}

// ===========================================================================
// register_meta_functions
// ===========================================================================

/// Registers the evaluator-context `sass:meta` members and assembles the module.
///
/// Matches Dart: `evaluate.dart:689-706` — builds the `meta` module from the
/// shared `meta.moduleFunctions` plus the evaluator-context functions and
/// mixins here, stores it in `built_in_modules` so `@use 'sass:meta'`
/// resolves, wraps each function with `with_deprecation_warning("meta")` into
/// `built_in_functions` (underscore-→-hyphen key), and appends the functions
/// (not the mixins — Dart keeps `metaMixins` module-only, so
/// `meta.function-exists("apply")` is `false`) to `global_functions` for the
/// global lookup path.
///
/// Marked `#[maybe_async]`: in the sync build the `Async` callback closures
/// convert to plain `Fn` closures (return types rewrite via the
/// LocalBoxFuture patch; `box_rec!` edges become direct calls), so they
/// register as `Sync` callbacks.
#[rust_sass_macros::maybe_async]
pub fn register_meta_functions<'compile, 'parse>(
    v: &mut EvaluateVisitor<'compile, 'parse>,
    arena: &'compile Bump,
) -> SassResult<()>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let mut meta_mod = meta_module(arena);
    let fns = create_meta_functions(v, arena);
    let mixins = create_meta_mixins(v, arena);

    for f in &fns {
        meta_mod.add_function(*f);
    }
    for m in &mixins {
        meta_mod.add_mixin(*m);
    }

    let module = Module::new(arena, ModuleKind::BuiltIn(meta_mod));
    v.config
        .built_in_modules
        .insert("sass:meta".to_string(), module);

    // Register globally with deprecation warning (Dart:
    // metaFunctions.wrapWithDeprecationWarning('meta')).
    for fn_ in &fns {
        match fn_.kind() {
            CallableKind::BuiltIn(b) => {
                let wrapped = b.with_deprecation_warning("meta", None);
                let key = wrapped.name().replace("_", "-");
                v.config
                    .built_in_functions
                    .borrow_mut()
                    .insert(key, Callable::new(arena, CallableKind::BuiltIn(wrapped)));
            }
            _ => panic!("expected BuiltIn callable"),
        }
    }
    // Append evaluator functions (not mixins) for test harness lookup
    // (eval_fn uses unwrapped names from this list). Dart keeps `metaMixins`
    // module-only: `meta.function-exists("apply")` is false.
    let mut gf = v.config.global_functions.borrow_mut();
    for f in &fns {
        gf.push(*f);
    }

    Ok(())
}

// ===========================================================================
// create_meta_functions
// ===========================================================================

/// Builds the 11 evaluator-context `sass:meta` functions.
///
/// Matches Dart: `metaFunctions` list (`evaluate.dart:386-600`). Each entry is
/// a `BuiltInCallable::function` with URL `sass:meta` made via [`meta_fn`]
/// (`call` uses [`meta_fn_async`] since it re-enters the evaluator). `$name`
/// arguments normalize `_` → `-` before environment lookup; an omitted
/// `$module` is `realNull` (null counts as absent, anything else must assert
/// as a string).
///
/// Behavior per function, adapted to the free-function shape (`config` is
/// unused — lookups go through `state.env` live, or through the `env` /
/// `built_in_functions` / `compile_context` handles cloned up front so
/// closures capture `Copy` arena/`Rc` handles, never `&mut state`):
///
/// * `global-variable-exists($name, $module: null)` — `state.env`
///   `global_variable_exists` on the normalized name with the optional
///   namespace.
/// * `variable-exists($name)` — `state.env` `variable_exists` on the
///   normalized name.
/// * `function-exists($name, $module: null)` — `env.function_exists`
///   (normalized) **or** `built_in_functions` containing the **raw**
///   (unnormalized) key — Dart checks `_builtInFunctions.containsKey
///   (variable.text)` (`evaluate.dart:422`), so `my_fn` does not match a
///   `my-fn` global.
/// * `mixin-exists($name, $module: null)` — `env.mixin_exists` on the
///   normalized name with the optional namespace.
/// * `content-exists()` — `Script` error unless `state.env.in_mixin()`; else
///   whether `state.env.content()` is present.
/// * `module-variables($module)` — map of the namespaced module's variables
///   (`SassString` → value); `Script` error when the namespace is unknown.
/// * `module-functions($module)` — same, values wrapped as `SassFunction`
///   with the captured compile context.
/// * `module-mixins($module)` — same, values wrapped as `SassMixin` with the
///   captured compile context.
/// * `get-function($name, $css: false, $module: null)` — with `$css`, rejects
///   a simultaneous `$module` and returns a `PlainCssCallable` under the
///   caller's name; otherwise resolves the normalized name against `env` (a
///   given namespace short-circuits the built-in fallback) and then
///   `built_in_functions`, throwing `Function not found: <name>`; the result
///   carries the captured compile context.
/// * `get-mixin($name, $module: null)` — resolves the normalized name against
///   `env`, throwing `Mixin not found: <name>`; the result carries the
///   captured compile context.
/// * `call($function, $args...)` — reentrant; forwards to [`call_impl`]:
///   `SassFunction` values run via `invoke_callable` after an
///   `assert_compile_context` check, strings take the deprecated
///   [`CALL_STRING`] path, anything else is a `$function`-named `Script`
///   error.
#[rust_sass_macros::maybe_async]
fn create_meta_functions<'compile, 'parse>(
    v: &EvaluateVisitor<'compile, 'parse>,
    arena: &'compile Bump,
) -> Vec<Callable<'compile, 'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    // Capture cloned handles for late-binding semantics: the closures below
    // outlive `&mut state`, so they capture `Copy` arena refs / `Rc` host
    // handles — never `&mut state` (see `ref/eval.md` borrow patterns).
    let env = v.state.env;
    let built_in_functions = v.config.built_in_functions;
    let compile_context = v.config.compile_context.clone();

    [
        // --- global-variable-exists ---
        Callable::new(
            arena,
            CallableKind::BuiltIn(meta_fn(
                "global-variable-exists",
                "$name, $module: null",
                arena,
                {
                    Rc::new(
                        move |_config: &EvalConfig<'compile, 'parse>,
                              state: &mut EvalState<'compile, 'parse>,
                              args: Vec<Value<'parse>>,
                              _arena: &'compile Bump|
                              -> SassResult<Value<'parse>> {
                            let name = assert_string(&args[0], Some("name"))?;
                            let normalized = name.text.replace("_", "-");
                            let namespace = if let Some(m) = args.get(1) {
                                if matches!(&**m, ValueKind::Null) {
                                    None
                                } else {
                                    Some(assert_string(m, Some("module"))?.text.to_string())
                                }
                            } else {
                                None
                            };
                            let exists = state
                                .env
                                .global_variable_exists(&normalized, namespace.as_deref())?;
                            Ok(Value::new_with_arena(
                                arena,
                                ValueKind::Boolean(if exists { SASS_TRUE } else { SASS_FALSE }),
                            ))
                        },
                    )
                },
            )),
        ),
        // --- variable-exists ---
        Callable::new(
            arena,
            CallableKind::BuiltIn(meta_fn("variable-exists", "$name", arena, {
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        let name = assert_string(&args[0], Some("name"))?;
                        let normalized = name.text.replace("_", "-");
                        let exists = state.env.variable_exists(&normalized)?;
                        Ok(Value::new_with_arena(
                            arena,
                            ValueKind::Boolean(if exists { SASS_TRUE } else { SASS_FALSE }),
                        ))
                    },
                )
            })),
        ),
        // --- function-exists ---
        Callable::new(
            arena,
            CallableKind::BuiltIn(meta_fn("function-exists", "$name, $module: null", arena, {
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        let name = assert_string(&args[0], Some("name"))?;
                        let normalized = name.text.replace("_", "-");
                        let namespace = if let Some(m) = args.get(1) {
                            if matches!(&**m, ValueKind::Null) {
                                None
                            } else {
                                Some(assert_string(m, Some("module"))?.text.to_string())
                            }
                        } else {
                            None
                        };
                        let env_exists = env.function_exists(&normalized, namespace.as_deref())?;
                        if env_exists {
                            return Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_TRUE)));
                        }
                        // Dart: _builtInFunctions.containsKey(variable.text) — RAW name
                        if built_in_functions.borrow().contains_key(name.text) {
                            return Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_TRUE)));
                        }
                        Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)))
                    },
                )
            })),
        ),
        // --- mixin-exists ---
        Callable::new(
            arena,
            CallableKind::BuiltIn(meta_fn("mixin-exists", "$name, $module: null", arena, {
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        let name = assert_string(&args[0], Some("name"))?;
                        let normalized = name.text.replace("_", "-");
                        let namespace = if let Some(m) = args.get(1) {
                            if matches!(&**m, ValueKind::Null) {
                                None
                            } else {
                                Some(assert_string(m, Some("module"))?.text.to_string())
                            }
                        } else {
                            None
                        };
                        let exists = env.mixin_exists(&normalized, namespace.as_deref())?;
                        Ok(Value::new_with_arena(
                            arena,
                            ValueKind::Boolean(if exists { SASS_TRUE } else { SASS_FALSE }),
                        ))
                    },
                )
            })),
        ),
        // --- content-exists ---
        Callable::new(
            arena,
            CallableKind::BuiltIn(meta_fn("content-exists", "", arena, {
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          state: &mut EvalState<'compile, 'parse>,
                          _args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        if !state.env.in_mixin() {
                            return Err(Box::new(SassError::Script {
                                message: "content-exists() may only be called within a mixin."
                                    .into(),
                                argument_name: None,
                            }));
                        }
                        let has_content = state.env.content().is_some();
                        Ok(Value::new_with_arena(
                            arena,
                            ValueKind::Boolean(if has_content { SASS_TRUE } else { SASS_FALSE }),
                        ))
                    },
                )
            })),
        ),
        // --- module-variables ---
        Callable::new(
            arena,
            CallableKind::BuiltIn(meta_fn("module-variables", "$module", arena, {
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        let ns = assert_string(&args[0], Some("module"))?;
                        let modules = env.modules();
                        let module = modules.get(ns.text).ok_or_else(|| SassError::Script {
                            message: format!("There is no module with namespace {:?}.", ns.text),
                            argument_name: None,
                        })?;
                        let mut m = SassMap::empty();
                        for (name, val) in module.variables().entries() {
                            m.set(
                                Value::new_with_arena(
                                    arena,
                                    ValueKind::String(SassString::new(
                                        arena.alloc_str(&name),
                                        true,
                                    )),
                                ),
                                val,
                            );
                        }
                        Ok(Value::new_with_arena(arena, ValueKind::Map(m)))
                    },
                )
            })),
        ),
        // --- module-functions ---
        Callable::new(
            arena,
            CallableKind::BuiltIn(meta_fn("module-functions", "$module", arena, {
                let cctx = compile_context.clone();
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        let ns = assert_string(&args[0], Some("module"))?;
                        let modules = env.modules();
                        let module = modules.get(ns.text).ok_or_else(|| SassError::Script {
                            message: format!("There is no module with namespace {:?}.", ns.text),
                            argument_name: None,
                        })?;
                        let mut m = SassMap::empty();
                        for (name, fn_) in module.functions().entries() {
                            m.set(
                                Value::new_with_arena(
                                    arena,
                                    ValueKind::String(SassString::new(
                                        arena.alloc_str(&name),
                                        true,
                                    )),
                                ),
                                Value::new_with_arena(
                                    arena,
                                    ValueKind::Function(SassFunction::with_compile_context(
                                        fn_,
                                        Some(cctx.clone()),
                                    )),
                                ),
                            );
                        }
                        Ok(Value::new_with_arena(arena, ValueKind::Map(m)))
                    },
                )
            })),
        ),
        // --- module-mixins ---
        Callable::new(
            arena,
            CallableKind::BuiltIn(meta_fn("module-mixins", "$module", arena, {
                let cctx = compile_context.clone();
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        let ns = assert_string(&args[0], Some("module"))?;
                        let modules = env.modules();
                        let module = modules.get(ns.text).ok_or_else(|| SassError::Script {
                            message: format!("There is no module with namespace {:?}.", ns.text),
                            argument_name: None,
                        })?;
                        let mut m = SassMap::empty();
                        for (name, mx) in module.mixins().entries() {
                            m.set(
                                Value::new_with_arena(
                                    arena,
                                    ValueKind::String(SassString::new(
                                        arena.alloc_str(&name),
                                        true,
                                    )),
                                ),
                                Value::new_with_arena(
                                    arena,
                                    ValueKind::Mixin(SassMixin::with_compile_context(
                                        mx,
                                        Some(cctx.clone()),
                                    )),
                                ),
                            );
                        }
                        Ok(Value::new_with_arena(arena, ValueKind::Map(m)))
                    },
                )
            })),
        ),
        // --- get-function ---
        Callable::new(
            arena,
            CallableKind::BuiltIn(meta_fn(
                "get-function",
                "$name, $css: false, $module: null",
                arena,
                {
                    let cctx = compile_context.clone();
                    Rc::new(
                        move |_config: &EvalConfig<'compile, 'parse>,
                              _state: &mut EvalState<'compile, 'parse>,
                              args: Vec<Value<'parse>>,
                              _arena: &'compile Bump|
                              -> SassResult<Value<'parse>> {
                            let name = assert_string(&args[0], Some("name"))?;
                            let css = args.get(1).map(|v| v.is_truthy()).unwrap_or(false);
                            let module_val = args.get(2);

                            if css {
                                let module_str = module_val.and_then(|v| {
                                    if matches!(&**v, ValueKind::Null) {
                                        None
                                    } else {
                                        Some(v)
                                    }
                                });
                                if module_str.is_some() {
                                    return Err(Box::new(SassError::Script {
                                        message: "$css and $module may not both be passed at once."
                                            .into(),
                                        argument_name: None,
                                    }));
                                }
                                let pc = PlainCssCallable {
                                    name: name.text.to_string(),
                                };
                                return Ok(Value::new_with_arena(
                                    arena,
                                    ValueKind::Function(SassFunction::with_compile_context(
                                        Callable::new(arena, CallableKind::PlainCss(pc)),
                                        Some(cctx.clone()),
                                    )),
                                ));
                            }

                            let namespace = module_val.and_then(|v| {
                                if matches!(&**v, ValueKind::Null) {
                                    None
                                } else {
                                    Some(v)
                                }
                            });
                            let namespace_str = if let Some(v) = namespace {
                                Some(assert_string(v, Some("module"))?.text.to_string())
                            } else {
                                None
                            };
                            let normalized = name.text.replace("_", "-");
                            let local = env.get_function(&normalized, namespace_str.as_deref())?;
                            if let Some(c) = local {
                                return Ok(Value::new_with_arena(
                                    arena,
                                    ValueKind::Function(SassFunction::with_compile_context(
                                        c,
                                        Some(cctx.clone()),
                                    )),
                                ));
                            }
                            // Namespace was specified → no built-in fallback (Dart:
                            // local != nil || namespace != nil → return local).
                            if namespace_str.is_some() {
                                return Err(function_not_found(name)?);
                            }
                            // Built-in fallback.
                            if let Some(c) = built_in_functions.borrow().get(&normalized).cloned() {
                                return Ok(Value::new_with_arena(
                                    arena,
                                    ValueKind::Function(SassFunction::with_compile_context(
                                        c,
                                        Some(cctx.clone()),
                                    )),
                                ));
                            }
                            Err(function_not_found(name)?)
                        },
                    )
                },
            )),
        ),
        // --- get-mixin ---
        Callable::new(
            arena,
            CallableKind::BuiltIn(meta_fn("get-mixin", "$name, $module: null", arena, {
                let cctx = compile_context.clone();
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        let name = assert_string(&args[0], Some("name"))?;
                        let normalized = name.text.replace("_", "-");
                        let namespace = args.get(1).and_then(|v| {
                            if matches!(&**v, ValueKind::Null) {
                                None
                            } else {
                                Some(v)
                            }
                        });
                        let namespace_str = if let Some(v) = namespace {
                            Some(assert_string(v, Some("module"))?.text.to_string())
                        } else {
                            None
                        };
                        let local = env.get_mixin(&normalized, namespace_str.as_deref())?;
                        match local {
                            Some(c) => Ok(Value::new_with_arena(
                                arena,
                                ValueKind::Mixin(SassMixin::with_compile_context(
                                    c,
                                    Some(cctx.clone()),
                                )),
                            )),
                            None => Err(mixin_not_found(name)?),
                        }
                    },
                )
            })),
        ),
        // --- call ---
        Callable::new(
            arena,
            CallableKind::BuiltIn(meta_fn_async("call", r"$function, $args...", arena, {
                let cctx = compile_context.clone();
                Rc::new(
                    move |config,
                          state,
                          args,
                          _arena|
                          -> LocalBoxFuture<'_, SassResult<Value<'parse>>> {
                        let cctx = cctx.clone();
                        box_rec!(call_impl(config, state, arena, args, cctx))
                    },
                )
            })),
        ),
    ]
    .into()
}

// ===========================================================================
// load_css_impl / apply_impl — extracted free fns to sidestep HRTB lifetime
// issues in the create_meta_mixins closures (both-bounds required for
// invariance — see docs/architecture.md).
// ===========================================================================

/// Reentrant body of the `meta.load-css($url, $with: null)` mixin.
///
/// Parses `$url`, builds the `$with` [`Configuration`], loads the module via
/// `load_module` (which returns the `(Module, bool)` tuple rather than taking
/// a callback — a callback would need to capture `&mut EvalState` while
/// `load_module` already borrows it), then combines and evaluates its CSS
/// inside a `load-css()` stack frame with an exception trace (so CSS-eval
/// errors report the `load-css()` frame), and finally asserts the
/// configuration is empty.
///
/// `$with` mapping (Dart: `evaluate.dart:606-633`): null → empty
/// configuration (never the caller's ambient one); an empty list counts as an
/// explicitly empty map (per `SassList.assertMap`); otherwise each key must
/// assert as a string (normalized `_` → `-`), configuring the same variable
/// twice is an error, and `-private` keys emit the [`WITH_PRIVATE`]
/// deprecation once.
#[rust_sass_macros::maybe_async]
async fn load_css_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let url_val = assert_string(&args[0], Some("url"))?;
    let parsed_url = SassUrl::parse(url_val.text)
        .unwrap_or_else(|_| SassUrl::parse(&format!("file:///{}", &url_val.text)).unwrap());
    let with_val = &args[1];

    // Dart `load-css`: `configuration = const Configuration.empty()` for a
    // null `$with`, and `SassList.assertMap` (which counts empty lists as
    // empty maps) otherwise — never the caller's ambient configuration.
    let with_config = if matches!(&**with_val, ValueKind::Null) {
        Some(Configuration::empty(arena))
    } else if let ValueKind::List(l) = &**with_val {
        if l.contents.is_empty() {
            let span = state.callable_span.unwrap_or(BOGUS_SPAN);
            Some(Configuration::new_explicit(arena, IndexMap::new(), span))
        } else {
            return Err(Box::new(SassError::Script {
                message: format!("{} is not a map.", with_val.to_display_string()?),
                argument_name: Some("with".into()),
            }));
        }
    } else {
        let with_map = match &**with_val {
            ValueKind::Map(m) => m,
            _ => {
                return Err(Box::new(SassError::Script {
                    message: format!("{} is not a map.", with_val.to_display_string()?),
                    argument_name: Some("with".into()),
                }))
            }
        };
        let span = state.callable_span.unwrap_or(BOGUS_SPAN);
        let mut values: IndexMap<String, ConfiguredValue> = IndexMap::new();
        let mut private_deprecation = false;
        for (variable, val) in &with_map.entries {
            let name_str = match &**variable {
                ValueKind::String(s) => s.text.replace("_", "-"),
                _ => {
                    return Err(Box::new(SassError::Script {
                        message: format!("{} is not a string.", variable.to_display_string()?),
                        argument_name: Some("with key".into()),
                    }))
                }
            };
            if values.contains_key(&name_str) {
                return Err(Box::new(SassError::Script {
                    message: format!("The variable ${} was configured twice.", name_str),
                    argument_name: None,
                }));
            }
            if name_str.starts_with('-') && !private_deprecation {
                private_deprecation = true;
                warn_deprecation(
                    config,
                    state,
                    &format!(
                        "Configuring private variables (such as ${}) \
                     is deprecated.\nThis will be an error in \
                     Dart Sass 2.0.0.",
                        name_str
                    ),
                    &WITH_PRIVATE,
                )?;
            }
            values.insert(name_str, ConfiguredValue::explicit(*val, span, span));
        }
        Some(Configuration::new_explicit(arena, values, span))
    };

    let callable_node_span = state.callable_span.unwrap_or(BOGUS_SPAN);
    let base_url = callable_node_span.source_url();
    let (module, _first_load) = load_module(
        config,
        state,
        arena,
        &parsed_url,
        "load-css()",
        callable_node_span,
        with_config.as_ref(),
        true,
        base_url,
    )
    .await?;
    // Dart runs the load-css callback (`_combineCss(module, clone: true)`)
    // inside `_loadModule`'s `_withStackFrame("load-css()")`, so CSS-eval
    // errors (e.g. unsatisfied @extend) get the `load-css()` frame.
    with_stack_frame(
        config,
        state,
        arena,
        "load-css()",
        callable_node_span,
        None,
        async |config, state| {
            add_exception_trace(state, async |s2| {
                let combined = combine_css(arena, &module, true)?;
                evaluate_css_stylesheet(config, s2, arena, &combined).await?;
                Ok(())
            })
            .await
        },
    )
    .await?;

    if let Some(ref c) = with_config {
        if c.is_explicit() && !c.is_empty() {
            if let Some((name, _cv)) = c.values().into_iter().next() {
                return Err(Box::new(SassError::Script {
                    message: format!(
                        "${} was not declared with !default in the @used module.",
                        name
                    ),
                    argument_name: None,
                }));
            }
        }
    }

    Ok(Value::new_with_arena(arena, ValueKind::Null))
}

/// Reentrant body of the `meta.apply($mixin, $args...)` mixin.
///
/// Asserts the mixin reference (plus its compile context), repacks the rest
/// args as the invocation, captures the ambient `@content` block from
/// `state.env`, and forwards to `apply_mixin` (extracted from
/// `evaluate_include_rule`). Returns null. Dart: `evaluate.dart:646-686`;
/// `accepts_content` is true so `@include meta.apply(...) { ... }` passes its
/// content block through.
#[rust_sass_macros::maybe_async]
async fn apply_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    cctx: CompileContext,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let mixin = assert_mixin(&args[0], Some("mixin"))?;
    mixin.assert_compile_context(&cctx)?;
    let span = state.callable_span.unwrap_or(BOGUS_SPAN);
    let rest_val = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| Value::new_with_arena(arena, ValueKind::Null));
    let rest_expr = Expression::Value(ValueExpression::new(rest_val, span));
    let arg_list = ArgumentList::new(
        vec![],
        IndexMap::new(),
        IndexMap::new(),
        span,
        Some(rest_expr),
        None,
    );
    let current_content = state.env.content();
    apply_mixin(
        config,
        state,
        arena,
        &mixin.callable,
        &arg_list,
        current_content,
        span,
        span,
    )
    .await
    .map(|_| Value::new_with_arena(arena, ValueKind::Null))
}

// ===========================================================================
// create_meta_mixins
// ===========================================================================

/// Builds the 2 evaluator-context `sass:meta` mixins.
///
/// Matches Dart: `metaMixins` list (`evaluate.dart:602-687`). Both are
/// `BuiltInCallable::mixin` with URL `sass:meta` made via
/// [`meta_mixin_fn_async`]; both re-enter the evaluator through the
/// `pub(crate)` entry points above (`load-css` → `load_module` +
/// `evaluate_css_stylesheet` via [`load_css_impl`], `apply` →
/// `apply_mixin` via [`apply_impl`]). Only `apply` sets `accepts_content`.
#[rust_sass_macros::maybe_async]
fn create_meta_mixins<'compile, 'parse>(
    v: &EvaluateVisitor<'compile, 'parse>,
    arena: &'compile Bump,
) -> Vec<Callable<'compile, 'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let compile_context = v.config.compile_context.clone();

    [
        // --- load-css ---
        Callable::new(
            arena,
            CallableKind::BuiltIn(meta_mixin_fn_async(
                "load-css",
                "$url, $with: null",
                false,
                arena,
                {
                    Rc::new(
                    move |config,
                          state,
                          args,
                          __arena|
                          -> LocalBoxFuture<'_, SassResult<Value<'parse>>> {
                        box_rec!(load_css_impl(config, state, args, arena))
                    },
                )
                },
            )),
        ),
        // --- apply ---
        Callable::new(
            arena,
            CallableKind::BuiltIn(meta_mixin_fn_async(
                "apply",
                "$mixin, $args...",
                true,
                arena,
                {
                    let cctx = compile_context.clone();
                    Rc::new(
                    move |config,
                          state,
                          args,
                          __arena|
                          -> LocalBoxFuture<'_, SassResult<Value<'parse>>> {
                        let cctx = cctx.clone();
                        box_rec!(apply_impl(config, state, args, cctx, arena))
                    },
                )
                },
            )),
        ),
    ]
    .into()
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Creates one evaluator-context `sass:meta` mixin (async/sync twin pair).
///
/// Matches Dart: `BuiltInCallable.mixin(..., acceptsContent: ...)`
/// (`evaluate.dart:603-686`). `load-css` passes `false`, `apply` passes
/// `true`.
#[rust_sass_macros::async_impl]
fn meta_mixin_fn_async<'compile, 'parse>(
    name: &str,
    params: &str,
    accepts_content: bool,
    arena: &'compile Bump,
    callback: AsyncBuiltInCallback<'compile, 'parse>,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let mut bic = BuiltInCallable::mixin_async(name, params, "sass:meta", arena, callback);
    bic.set_accepts_content(accepts_content);
    bic
}

#[rust_sass_macros::sync_impl]
fn meta_mixin_fn_async<'compile, 'parse>(
    name: &str,
    params: &str,
    accepts_content: bool,
    arena: &'compile Bump,
    callback: SyncBuiltInCallback<'compile, 'parse>,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let mut bic = BuiltInCallable::mixin(name, params, "sass:meta", arena, callback);
    bic.set_accepts_content(accepts_content);
    bic
}

fn function_not_found(name: &SassString<'_>) -> SassResult<Box<SassError>> {
    // Matches Dart: `throw "Function not found: $name"`
    // (`evaluate.dart:515`) — note the display (quoted vs unquoted) rendering
    // of the name is preserved via `to_display_string`.
    Ok(Box::new(SassError::Script {
        message: format!("Function not found: {}", name.to_display_string()?),
        argument_name: None,
    }))
}

fn mixin_not_found(name: &SassString<'_>) -> SassResult<Box<SassError>> {
    // Matches Dart: `throw "Mixin not found: $name"` (`evaluate.dart:535`).
    Ok(Box::new(SassError::Script {
        message: format!("Mixin not found: {}", name.to_display_string()?),
        argument_name: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::parameter_list::ParameterList;
    use crate::compile::compile;
    use crate::compile::compile_string;
    use crate::compile::CompileOptions;
    use crate::functions::test_utils::invoke_callback;
    use crate::io::Io;
    use crate::logger::test_utils::LogCall;
    use crate::serialize::serialize_value_inspect;
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::rc::Rc;

    use indexmap::IndexMap;

    use crate::common::file_span::{FileSpan, BOGUS_SPAN};
    use crate::compile_context::new_compile_context;
    use crate::io::VirtualIo;

    use crate::logger::test_utils::RecordLogger;
    use crate::module::BuiltInModule;
    use crate::value::{
        ListSeparator, SassArgumentList, SassBoolean, SassMap, SassNumber, SassString, Value,
        ValueKind,
    };

    // --- helpers ---

    fn meta_span() -> FileSpan<'static> {
        BOGUS_SPAN
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

    fn noop_callable<'compile, 'parse>(
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

    fn new_visitor<'compile, 'parse>(arena: &'compile Bump) -> EvaluateVisitor<'compile, 'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let mut v = EvaluateVisitor::new(
            Rc::new(RecordLogger::new()),
            new_compile_context(),
            arena,
            Rc::new(VirtualIo::new()),
        );
        register_meta_functions(&mut v, arena).unwrap();
        v
    }

    #[rust_sass_macros::maybe_async]
    // Test-only helper: the `borrow()` guard is held across the
    // `invoke_callback(...).await` below. Sound under the no-reentry contract
    // (critical-invariants.md): the async test runs single-threaded and the
    // borrow cannot conflict while suspended.
    #[allow(clippy::await_holding_refcell_ref)]
    async fn eval_fn<'compile, 'parse>(
        v: &mut EvaluateVisitor<'compile, 'parse>,
        name: &str,
        args: &[Value<'parse>],
        arena: &'compile Bump,
    ) -> SassResult<Value<'parse>>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let f = &v.config.global_functions.borrow();
        let c = f
            .iter()
            .find(|x| x.name() == name)
            .unwrap_or_else(|| panic!("meta fn {name:?} not found"));
        let CallableKind::BuiltIn(bic) = c.kind() else {
            panic!("expected BuiltIn callable")
        };
        let overload = bic.callback_for(args.len(), &HashSet::new()).unwrap();
        let mut padded = args.to_vec();
        while padded.len() < overload.params.parameters.len() {
            padded.push(Value::new_with_arena(arena, ValueKind::Null));
        }
        invoke_callback(&overload.callback, &v.config, &mut v.state, padded, arena).await
    }

    #[rust_sass_macros::maybe_async]
    async fn eval_ok<'compile, 'parse>(
        v: &mut EvaluateVisitor<'compile, 'parse>,
        name: &str,
        args: &[Value<'parse>],
        arena: &'compile Bump,
    ) -> Value<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        eval_fn(v, name, args, arena).await.unwrap()
    }

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

    fn assert_bool(got: &Value<'_>, want: bool) {
        match &**got {
            ValueKind::Boolean(SassBoolean { value }) => assert_eq!(*value, want),
            other => panic!("want Boolean, got {other:?}"),
        }
    }

    fn assert_inspect(got: &Value<'_>, want: &str) {
        assert_eq!(serialize_value_inspect(got).unwrap(), want);
    }

    fn module_with<'compile, 'parse>(
        v: &mut EvaluateVisitor<'compile, 'parse>,
        arena: &'compile Bump,
        namespace: &str,
        fns: Vec<Callable<'compile, 'parse>>,
        mixins: Vec<Callable<'compile, 'parse>>,
        vars: &[(&str, f64)],
    ) where
        'compile: 'parse,
    {
        let mut var_map = IndexMap::new();
        for (k, v_) in vars {
            var_map.insert(
                k.to_string(),
                Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(*v_, None))),
            );
        }
        let mod_ = BuiltInModule::new(arena, namespace.to_string(), &fns, &mixins, var_map);
        v.state
            .env
            .add_module(
                Module::new(arena, ModuleKind::BuiltIn(mod_)),
                meta_span(),
                Some(namespace),
            )
            .unwrap();
    }

    #[rust_sass_macros::maybe_async]
    async fn mixin_callback<'compile, 'parse>(
        v: &mut EvaluateVisitor<'compile, 'parse>,
        name: &str,
        args: Vec<Value<'parse>>,
        arena: &'compile Bump,
    ) -> SassResult<Value<'parse>>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        // Meta mixins live module-only (not in `global_functions`): resolve
        // through `sass:meta` like a namespaced `@include` would.
        let modules = &v.config.built_in_modules;
        let module = modules
            .get("sass:meta")
            .expect("sass:meta module registered");
        let c = module
            .mixins()
            .get(name)
            .unwrap_or_else(|| panic!("meta mixin {name:?} not found"));
        let CallableKind::BuiltIn(bic) = c.kind() else {
            panic!("expected BuiltIn callable")
        };
        let overload = bic.callback_for(args.len(), &HashSet::new()).unwrap();
        invoke_callback(&overload.callback, &v.config, &mut v.state, args, arena).await
    }

    // --- tests ---

    #[rust_sass_macros::maybe_test]
    async fn test_global_variable_exists() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        v.state
            .env
            .set_variable("a-b", num_val(&arena, 1.0), meta_span(), None, true)
            .unwrap();
        assert_bool(
            &eval_ok(
                &mut v,
                "global-variable-exists",
                &[quoted_val(&arena, "a-b")],
                &arena,
            )
            .await,
            true,
        );
        assert_bool(
            &eval_ok(
                &mut v,
                "global-variable-exists",
                &[quoted_val(&arena, "a_b")],
                &arena,
            )
            .await,
            true,
        );
        assert_bool(
            &eval_ok(
                &mut v,
                "global-variable-exists",
                &[quoted_val(&arena, "nope")],
                &arena,
            )
            .await,
            false,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_variable_exists_module() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        module_with(&mut v, &arena, "m", vec![], vec![], &[("x", 1.0)]);
        assert_bool(
            &eval_ok(
                &mut v,
                "global-variable-exists",
                &[quoted_val(&arena, "x"), quoted_val(&arena, "m")],
                &arena,
            )
            .await,
            true,
        );
        assert_bool(
            &eval_ok(
                &mut v,
                "global-variable-exists",
                &[quoted_val(&arena, "y"), quoted_val(&arena, "m")],
                &arena,
            )
            .await,
            false,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_variable_exists_missing_module() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        assert_script_err(
            eval_fn(
                &mut v,
                "global-variable-exists",
                &[quoted_val(&arena, "x"), quoted_val(&arena, "nope")],
                &arena,
            )
            .await
            .unwrap_err(),
            "There is no module with the namespace \"nope\".",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_variable_exists_name_type_error() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        assert_script_err(
            eval_fn(
                &mut v,
                "global-variable-exists",
                &[num_val(&arena, 1.0)],
                &arena,
            )
            .await
            .unwrap_err(),
            "1 is not a string.",
            Some("name"),
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_variable_exists() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        v.state
            .env
            .set_variable("a-b", num_val(&arena, 1.0), meta_span(), None, true)
            .unwrap();
        assert_bool(
            &eval_ok(
                &mut v,
                "variable-exists",
                &[quoted_val(&arena, "a-b")],
                &arena,
            )
            .await,
            true,
        );
        assert_bool(
            &eval_ok(
                &mut v,
                "variable-exists",
                &[quoted_val(&arena, "a_b")],
                &arena,
            )
            .await,
            true,
        );
        assert_bool(
            &eval_ok(
                &mut v,
                "variable-exists",
                &[quoted_val(&arena, "nope")],
                &arena,
            )
            .await,
            false,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_function_exists_environment() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        v.state.env.set_function(noop_callable(&arena, "my-fn"));
        assert_bool(
            &eval_ok(
                &mut v,
                "function-exists",
                &[quoted_val(&arena, "my-fn")],
                &arena,
            )
            .await,
            true,
        );
        assert_bool(
            &eval_ok(
                &mut v,
                "function-exists",
                &[quoted_val(&arena, "my_fn")],
                &arena,
            )
            .await,
            true,
        );
        assert_bool(
            &eval_ok(
                &mut v,
                "function-exists",
                &[quoted_val(&arena, "nope")],
                &arena,
            )
            .await,
            false,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_function_exists_global() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        // Host globals live in `_builtInFunctions` in Dart, so register here
        // (normalized key) rather than in the `global_functions` side list.
        v.config
            .built_in_functions
            .borrow_mut()
            .insert("global-fn".to_string(), noop_callable(&arena, "global-fn"));
        assert_bool(
            &eval_ok(
                &mut v,
                "function-exists",
                &[quoted_val(&arena, "global-fn")],
                &arena,
            )
            .await,
            true,
        );
        assert_bool(
            &eval_ok(
                &mut v,
                "function-exists",
                &[quoted_val(&arena, "global_fn")],
                &arena,
            )
            .await,
            false,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_function_exists_surface() {
        // Dart `function-exists` is `env.exists ||
        // _builtInFunctions.containsKey(raw)`: mixins are module-only
        // (B5) and host globals match by raw map key (B1, no side-list scan).
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        v.config
            .built_in_functions
            .borrow_mut()
            .insert("my-fn".to_string(), noop_callable(&arena, "my_fn"));
        for (name, want) in [
            ("apply", false),
            ("load-css", false),
            ("get-function", true),
            ("my_fn", false),
            ("my-fn", true),
        ] {
            assert_bool(
                &eval_ok(
                    &mut v,
                    "function-exists",
                    &[quoted_val(&arena, name)],
                    &arena,
                )
                .await,
                want,
            );
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_load_css_empty_with() {
        // Dart `SassList.assertMap` counts empty lists as empty maps, so
        // `$with: ()` is an explicitly empty configuration — configuring a
        // built-in module with it errors like any explicit config.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@use \"sass:meta\";\n@include meta.load-css(\"sass:math\", $with: ());\na { b: 1; }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, .. } => {
                assert_eq!(message, "Built-in module sass:math can't be configured.")
            }
            other => panic!("expected Runtime error, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_load_css_null_with_isolated() {
        // Dart `load-css` uses `const Configuration.empty()` for a null
        // `$with`: the caller's configuration never leaks into the loaded
        // module — even when the outer module hasn't consumed it yet because
        // its own `!default` comes later (pre-fix this yielded `c: 1`,
        // starving the outer declaration to `o: 0` via the shared inner).
        let arena = Bump::new();
        let mut files = HashMap::new();
        files.insert(
            "/main.scss".to_string(),
            "@use \"outer3.scss\" with ($x: 1);".to_string(),
        );
        files.insert(
            "/outer3.scss".to_string(),
            "@use \"sass:meta\";\n@include meta.load-css(\"b3.scss\");\n$x: 0 !default;\nouter { o: $x; }"
                .to_string(),
        );
        files.insert(
            "/b3.scss".to_string(),
            "$x: 0 !default;\nb { c: $x; }\n".to_string(),
        );
        let io: Rc<dyn Io> = Rc::new(VirtualIo::with_files(files));
        let result = compile("/main.scss", io, CompileOptions::new(&arena), &arena)
            .await
            .unwrap();
        assert!(result.css().contains("c: 0;"), "got: {}", result.css());
        assert!(result.css().contains("o: 1;"), "got: {}", result.css());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_function_exists_module() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        module_with(
            &mut v,
            &arena,
            "m",
            vec![noop_callable(&arena, "mod-fn")],
            vec![],
            &[],
        );
        assert_bool(
            &eval_ok(
                &mut v,
                "function-exists",
                &[quoted_val(&arena, "mod-fn"), quoted_val(&arena, "m")],
                &arena,
            )
            .await,
            true,
        );
        assert_bool(
            &eval_ok(
                &mut v,
                "function-exists",
                &[quoted_val(&arena, "nope"), quoted_val(&arena, "m")],
                &arena,
            )
            .await,
            false,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_mixin_exists() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        v.state.env.set_mixin(noop_callable(&arena, "my-mixin"));
        assert_bool(
            &eval_ok(
                &mut v,
                "mixin-exists",
                &[quoted_val(&arena, "my-mixin")],
                &arena,
            )
            .await,
            true,
        );
        assert_bool(
            &eval_ok(
                &mut v,
                "mixin-exists",
                &[quoted_val(&arena, "my_mixin")],
                &arena,
            )
            .await,
            true,
        );
        assert_bool(
            &eval_ok(
                &mut v,
                "mixin-exists",
                &[quoted_val(&arena, "nope")],
                &arena,
            )
            .await,
            false,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_mixin_exists_module() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        module_with(
            &mut v,
            &arena,
            "m",
            vec![],
            vec![noop_callable(&arena, "mod-mix")],
            &[],
        );
        assert_bool(
            &eval_ok(
                &mut v,
                "mixin-exists",
                &[quoted_val(&arena, "mod-mix"), quoted_val(&arena, "m")],
                &arena,
            )
            .await,
            true,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_content_exists_outside_mixin() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        assert_script_err(
            eval_fn(&mut v, "content-exists", &[], &arena)
                .await
                .unwrap_err(),
            "content-exists() may only be called within a mixin.",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_content_exists_in_mixin() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        v.state.env.set_in_mixin(true);
        assert_bool(&eval_ok(&mut v, "content-exists", &[], &arena).await, false);
        v.state
            .env
            .set_content(Some(noop_callable(&arena, "content_c")));
        assert_bool(&eval_ok(&mut v, "content-exists", &[], &arena).await, true);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_module_variables() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        module_with(
            &mut v,
            &arena,
            "m",
            vec![],
            vec![],
            &[("a", 1.0), ("b", 2.0)],
        );
        assert_inspect(
            &eval_ok(
                &mut v,
                "module-variables",
                &[quoted_val(&arena, "m")],
                &arena,
            )
            .await,
            "(\"a\": 1, \"b\": 2)",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_module_variables_missing_module() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        assert_script_err(
            eval_fn(
                &mut v,
                "module-variables",
                &[quoted_val(&arena, "nope")],
                &arena,
            )
            .await
            .unwrap_err(),
            "There is no module with namespace \"nope\".",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_module_functions() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        let fn_ = noop_callable(&arena, "mod-fn");
        module_with(&mut v, &arena, "m", vec![fn_], vec![], &[]);
        let got = eval_ok(
            &mut v,
            "module-functions",
            &[quoted_val(&arena, "m")],
            &arena,
        )
        .await;
        let ValueKind::Map(m) = got.kind() else {
            panic!("expected Map")
        };
        assert_eq!(m.len(), 1);
        for (k, val) in &m.entries {
            let ValueKind::String(ks) = &**k else {
                panic!("key not string")
            };
            assert!(ks.has_quotes);
            let ValueKind::Function(sf) = &**val else {
                panic!("value not SassFunction")
            };
            assert!(sf.callable.identity_eq(&fn_));
            assert!(sf.assert_compile_context(&v.config.compile_context).is_ok());
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_module_mixins() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        let mx = noop_callable(&arena, "mod-mix");
        module_with(&mut v, &arena, "m", vec![], vec![mx], &[]);
        let got = eval_ok(&mut v, "module-mixins", &[quoted_val(&arena, "m")], &arena).await;
        let ValueKind::Map(m) = got.kind() else {
            panic!("expected Map")
        };
        for (k, val) in &m.entries {
            let ValueKind::String(ks) = &**k else {
                panic!("key not string")
            };
            assert!(ks.has_quotes);
            let ValueKind::Mixin(sm) = &**val else {
                panic!("value not SassMixin")
            };
            assert!(sm.callable.identity_eq(&mx));
            assert!(sm.assert_compile_context(&v.config.compile_context).is_ok());
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_get_function_environment() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        let fn_ = noop_callable(&arena, "my-fn");
        v.state.env.set_function(fn_);
        let got = eval_ok(
            &mut v,
            "get-function",
            &[quoted_val(&arena, "my-fn")],
            &arena,
        )
        .await;
        let ValueKind::Function(sf) = got.kind() else {
            panic!("expected SassFunction")
        };
        assert!(sf.callable.identity_eq(&fn_));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_get_function_built_in_fallback() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        let fn_ = noop_callable(&arena, "builtin-fn");
        v.config
            .built_in_functions
            .borrow_mut()
            .insert("builtin-fn".into(), fn_);
        let got = eval_ok(
            &mut v,
            "get-function",
            &[quoted_val(&arena, "builtin_fn")],
            &arena,
        )
        .await;
        let ValueKind::Function(sf) = got.kind() else {
            panic!("expected SassFunction")
        };
        assert!(sf.callable.identity_eq(&fn_));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_get_function_namespace_short_circuit() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        v.config
            .built_in_functions
            .borrow_mut()
            .insert("builtin-fn".into(), noop_callable(&arena, "builtin-fn"));
        module_with(&mut v, &arena, "m", vec![], vec![], &[]);
        assert_script_err(
            eval_fn(
                &mut v,
                "get-function",
                &[
                    quoted_val(&arena, "builtin-fn"),
                    Value::new_with_arena(&arena, ValueKind::Boolean(SASS_FALSE)),
                    quoted_val(&arena, "m"),
                ],
                &arena,
            )
            .await
            .unwrap_err(),
            "Function not found: \"builtin-fn\"",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_get_function_not_found() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        assert_script_err(
            eval_fn(
                &mut v,
                "get-function",
                &[quoted_val(&arena, "nope")],
                &arena,
            )
            .await
            .unwrap_err(),
            "Function not found: \"nope\"",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_get_function_not_found_unquoted() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        assert_script_err(
            eval_fn(&mut v, "get-function", &[str_val(&arena, "nope")], &arena)
                .await
                .unwrap_err(),
            "Function not found: nope",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_get_function_css() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        let got = eval_ok(
            &mut v,
            "get-function",
            &[
                quoted_val(&arena, "foo"),
                Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE)),
            ],
            &arena,
        )
        .await;
        let ValueKind::Function(sf) = got.kind() else {
            panic!("expected SassFunction")
        };
        assert_eq!(sf.callable.name(), "foo");
        assert!(matches!(sf.callable.kind(), CallableKind::PlainCss(_)));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_get_function_css_and_module_error() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        assert_script_err(
            eval_fn(
                &mut v,
                "get-function",
                &[
                    quoted_val(&arena, "foo"),
                    Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE)),
                    quoted_val(&arena, "m"),
                ],
                &arena,
            )
            .await
            .unwrap_err(),
            "$css and $module may not both be passed at once.",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_get_mixin() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        let mx = noop_callable(&arena, "my-mixin");
        v.state.env.set_mixin(mx);
        let got = eval_ok(
            &mut v,
            "get-mixin",
            &[quoted_val(&arena, "my_mixin")],
            &arena,
        )
        .await;
        let ValueKind::Mixin(sm) = got.kind() else {
            panic!("expected SassMixin")
        };
        assert!(sm.callable.identity_eq(&mx));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_get_mixin_not_found() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        assert_script_err(
            eval_fn(&mut v, "get-mixin", &[quoted_val(&arena, "nope")], &arena)
                .await
                .unwrap_err(),
            "Mixin not found: \"nope\"",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_call_type_error() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        assert_script_err(
            eval_fn(
                &mut v,
                "call",
                &[num_val(&arena, 1.0), arg_list(&arena, &[])],
                &arena,
            )
            .await
            .unwrap_err(),
            "1 is not a function reference.",
            Some("function"),
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_call_compile_context_mismatch() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        let other = new_compile_context();
        let sf = Value::new_with_arena(
            &arena,
            ValueKind::Function(SassFunction::with_compile_context(
                noop_callable(&arena, "f"),
                Some(other),
            )),
        );
        assert_script_err(
            eval_fn(&mut v, "call", &[sf, arg_list(&arena, &[])], &arena)
                .await
                .unwrap_err(),
            "get-function(\"f\") does not belong to current compilation.",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_apply_type_error() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        assert_script_err(
            mixin_callback(
                &mut v,
                "apply",
                vec![num_val(&arena, 1.0), arg_list(&arena, &[])],
                &arena,
            )
            .await
            .unwrap_err(),
            "1 is not a mixin reference.",
            Some("mixin"),
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_apply_compile_context_mismatch() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        let other = new_compile_context();
        let sm = Value::new_with_arena(
            &arena,
            ValueKind::Mixin(SassMixin::with_compile_context(
                noop_callable(&arena, "m"),
                Some(other),
            )),
        );
        assert_script_err(
            mixin_callback(&mut v, "apply", vec![sm, arg_list(&arena, &[])], &arena)
                .await
                .unwrap_err(),
            "get-mixin(\"m\") does not belong to current compilation.",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_apply_accepts_content() {
        // `load-css`/`apply` live module-only (Dart keeps `metaMixins` out of
        // the global function list), so look them up on `sass:meta`.
        let arena = Bump::new();
        let v = new_visitor(&arena);
        let module = v
            .config
            .built_in_modules
            .get("sass:meta")
            .expect("sass:meta module registered");
        for name in ["apply", "load-css"] {
            let m = module
                .mixins()
                .get(name)
                .unwrap_or_else(|| panic!("meta mixin {name:?} missing"));
            let CallableKind::BuiltIn(ref bic) = m.kind() else {
                panic!("expected BuiltIn mixin");
            };
            assert_eq!(bic.accepts_content(), name == "apply");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_load_css_url_type_error() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        assert_script_err(
            mixin_callback(
                &mut v,
                "load-css",
                vec![
                    num_val(&arena, 1.0),
                    Value::new_with_arena(&arena, ValueKind::Null),
                ],
                &arena,
            )
            .await
            .unwrap_err(),
            "1 is not a string.",
            Some("url"),
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_load_css_with_type_error() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        assert_script_err(
            mixin_callback(
                &mut v,
                "load-css",
                vec![quoted_val(&arena, "x"), num_val(&arena, 1.0)],
                &arena,
            )
            .await
            .unwrap_err(),
            "1 is not a map.",
            Some("with"),
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_load_css_configured_twice() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        let mut with = SassMap::empty();
        with.set(
            Value::new_with_arena(
                &arena,
                ValueKind::String(SassString::new(arena.alloc_str("a_b"), true)),
            ),
            num_val(&arena, 1.0),
        );
        with.set(
            Value::new_with_arena(
                &arena,
                ValueKind::String(SassString::new(arena.alloc_str("a-b"), true)),
            ),
            num_val(&arena, 2.0),
        );
        assert_script_err(
            mixin_callback(
                &mut v,
                "load-css",
                vec![
                    quoted_val(&arena, "x"),
                    Value::new_with_arena(&arena, ValueKind::Map(with)),
                ],
                &arena,
            )
            .await
            .unwrap_err(),
            "The variable $a-b was configured twice.",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_load_css_private_variable_deprecation() {
        let arena = Bump::new();
        let mut v = new_visitor(&arena);
        let logger = Rc::new(RecordLogger::new());
        v.config.logger = logger.clone();
        let mut with = SassMap::empty();
        with.set(
            Value::new_with_arena(
                &arena,
                ValueKind::String(SassString::new(arena.alloc_str("-priv"), true)),
            ),
            num_val(&arena, 1.0),
        );
        let _ = mixin_callback(
            &mut v,
            "load-css",
            vec![
                quoted_val(&arena, "nonexistent-file"),
                Value::new_with_arena(&arena, ValueKind::Map(with)),
            ],
            &arena,
        )
        .await;
        let calls = logger.calls();
        let has_deprecation = calls.iter().any(|call| match call {
            LogCall::WarnDeprecation { msg, .. } => msg.contains("Configuring private variables"),
            _ => false,
        });
        assert!(
            has_deprecation,
            "expected deprecation warning about private variables"
        );
    }

    fn arg_list<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        items: &[Value<'parse>],
    ) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::ArgumentList(SassArgumentList::new(
                arena,
                items.to_vec(),
                IndexMap::new(),
                ListSeparator::Comma,
            )),
        )
    }

    #[rust_sass_macros::maybe_test]
    async fn test_register_meta_functions() {
        let arena = Bump::new();
        let v = new_visitor(&arena);
        let mod_ = v
            .config
            .built_in_modules
            .get("sass:meta")
            .expect("sass:meta module not registered");
        let ModuleKind::BuiltIn(ref meta_mod) = *mod_.kind() else {
            panic!("expected BuiltIn")
        };
        assert_eq!(meta_mod.functions.len(), 18);
        assert_eq!(meta_mod.mixins.len(), 2);
        for name in [
            "global-variable-exists",
            "variable-exists",
            "function-exists",
            "mixin-exists",
            "content-exists",
            "module-variables",
            "module-functions",
            "module-mixins",
            "get-function",
            "get-mixin",
            "call",
        ] {
            assert!(meta_mod.functions.contains_key(name), "missing {name}");
        }
        for name in ["load-css", "apply"] {
            assert!(meta_mod.mixins.contains_key(name), "missing mixin {name}");
        }
        assert_eq!(v.config.global_functions.borrow().len(), 11); // fns only; mixins are module-only
    }
}
