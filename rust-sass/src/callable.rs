// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/callable.dart + lib/src/callable/async.dart + lib/src/callable/built_in.dart + lib/src/callable/async_built_in.dart + lib/src/callable/user_defined.dart + lib/src/callable/plain_css.dart
// go-source: go/sasscallable/callable.go + go/functions/callable_built_in.go + go/functions/callable_user_defined.go + go/functions/callable_plain_css.go

//! Functions and mixins invokable from Sass by passing arguments.
//!
//! The runtime representation has three variants ([`CallableKind`]:
//! user-defined, built-in, plain-CSS). Custom-function authors should follow
//! the core-function conventions from Dart's `Callable` docs: use the
//! `Value::assert_*` family (with the argument name) so users get good type
//! errors; treat every value as a list via `as_list` rather than casting to
//! `SassList`; preserve input metadata (separators, brackets, quotes, units)
//! on outputs; default to comma-separated lists, quoted strings, and unitless
//! numbers; use one-based Sass indexing from the end for negatives; and count
//! string indices in Unicode code points.

use crate::functions::helpers::warn_for_global_builtin_message;
use crate::parse::stylesheet_parse::parse_parameter_list;
use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::ast::sass::parameter_list::ParameterList;
use crate::ast::sass::statement::CallableDeclaration;
use crate::common::exception::{SassError, SassResult};
use crate::environment::Environment;
use crate::eval::{EvalConfig, EvalState};
use crate::value::Value;
use bumpalo::Bump;
#[cfg(feature = "async")]
use futures::future::LocalBoxFuture;

/// The synchronous built-in callback: evaluated arguments in, return value
/// out. Callables using this form work in both the sync and async builds, so
/// prefer it whenever the callback needs no asynchronous work (Dart's
/// `Callable` extends `AsyncCallable` for the same reason).
pub type SyncBuiltInCallback<'compile, 'parse> = Rc<
    dyn Fn(
            &EvalConfig<'compile, 'parse>,
            &mut EvalState<'compile, 'parse>,
            Vec<Value<'parse>>,
            &'compile Bump,
        ) -> SassResult<Value<'parse>>
        + 'parse,
>;

/// A built-in function callback for callables that *need* to do asynchronous
/// work (`meta.load-css`/`call`/`apply`, host-provided async functions). Only
/// compatible with the async build; anything that can work synchronously
/// should be [`SyncBuiltInCallback`] instead.
#[rust_sass_macros::async_impl]
pub type AsyncBuiltInCallback<'compile, 'parse> = Rc<
    dyn for<'a> Fn(
            &'a EvalConfig<'compile, 'parse>,
            &'a mut EvalState<'compile, 'parse>,
            Vec<Value<'parse>>,
            &'a Bump,
        ) -> LocalBoxFuture<'a, SassResult<Value<'parse>>>
        + 'parse,
>;

/// Selects how a built-in runs: `Sync` boxes no future (the fast path for
/// the ~380 pure built-ins) while `Async` awaits a boxed future. In the sync
/// build the enum collapses to the `Sync` alias.
#[rust_sass_macros::async_impl]
#[derive(Clone)]
pub enum BuiltInCallback<'compile, 'parse> {
    Sync(SyncBuiltInCallback<'compile, 'parse>),
    Async(AsyncBuiltInCallback<'compile, 'parse>),
}

#[rust_sass_macros::sync_impl]
pub type BuiltInCallback<'compile, 'parse> = SyncBuiltInCallback<'compile, 'parse>;

/// One overload of a [`BuiltInCallable`]: the parameter declaration to match
/// against plus the callback to run when it matches.
pub struct BuiltInOverload<'compile, 'parse> {
    /// The parameter declaration this overload matches against.
    pub params: ParameterList<'parse>,
    /// The callback to run when this overload matches.
    pub callback: BuiltInCallback<'compile, 'parse>,
}

impl<'compile: 'parse, 'parse> Clone for BuiltInOverload<'compile, 'parse> {
    fn clone(&self) -> Self {
        BuiltInOverload {
            params: self.params.clone(),
            callback: self.callback.clone(),
        }
    }
}

impl<'compile: 'parse, 'parse> fmt::Debug for BuiltInOverload<'compile, 'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BuiltInOverload")
            .field("params", &self.params)
            .finish()
    }
}

// === Production types ===

/// A callable defined in Rust code.
///
/// Unlike user-defined callables, built-in callables support overloads: they
/// may declare multiple callbacks with multiple parameter sets, and when the
/// callable is invoked the first callback with matching parameters runs
/// (see [`callback_for`](Self::callback_for)).
#[derive(Clone)]
pub struct BuiltInCallable<'compile, 'parse> {
    name: String,
    /// The overloads declared for this callable.
    overloads: Rc<Vec<BuiltInOverload<'compile, 'parse>>>,
    /// Whether this callable could potentially accept an `@content` block.
    /// This can only be true for mixins.
    accepts_content: bool,
    deprecation_warning: Option<(String, String)>, // (module, name) for global built-in deprecation
}

/// A callable defined in the user's Sass stylesheet.
///
/// Matches Dart: `UserDefinedCallable<E>` — `{declaration, environment,
/// inDependency}` with `name` derived from the declaration. (Go additionally
/// stores flattened `name`/`arguments`/`isMixin` copies — see
/// go-discrepancies.md #14.)
#[derive(Clone)]
pub struct UserDefinedCallable<'compile, 'parse> {
    /// The declaration.
    pub declaration: CallableDeclaration<'parse>,
    /// The environment in which this callable was declared.
    pub environment: Environment<'compile, 'parse>,
    /// Whether this callable was defined in a dependency — that is, whether
    /// it was (transitively) loaded through a load path or importer rather
    /// than relative to the entrypoint.
    pub in_dependency: bool,
}

impl<'compile: 'parse, 'parse> UserDefinedCallable<'compile, 'parse> {
    /// Matches Dart: `UserDefinedCallable(this.declaration, this.environment,
    /// {required this.inDependency})`.
    pub fn new(
        declaration: CallableDeclaration<'parse>,
        environment: Environment<'compile, 'parse>,
        in_dependency: bool,
    ) -> Self {
        UserDefinedCallable {
            declaration,
            environment,
            in_dependency,
        }
    }

    /// Matches Dart: `String get name => declaration.name`.
    pub fn name(&self) -> &str {
        self.declaration.name()
    }

    /// The declared parameters this callable accepts (Dart reads
    /// `declaration.parameters` directly).
    pub fn parameters(&self) -> &ParameterList<'parse> {
        self.declaration.parameters()
    }
}

/// A callable that emits a plain CSS function (e.g. an unknown function in a
/// plain-CSS context, serialized as `name(args...)`).
///
/// This can't be used for mixins.
pub struct PlainCssCallable {
    /// The function name, emitted verbatim as `name(args...)`.
    pub name: String,
}

/// The runtime representation of a Sass function or mixin: user-defined
/// (from the stylesheet), built-in (from Rust code), or a plain-CSS
/// fallback constructed on the fly when nothing else matches.
pub enum CallableKind<'compile, 'parse> {
    /// A callback defined in the user's Sass stylesheet.
    UserDefined(UserDefinedCallable<'compile, 'parse>),
    /// A callable defined in Rust code, with overloads.
    BuiltIn(BuiltInCallable<'compile, 'parse>),
    /// A plain-CSS function fallback. Never stored persistently.
    PlainCss(PlainCssCallable),
}

/// A function or mixin that can be invoked from Sass by passing arguments.
///
/// The handle is an arena-allocated `Copy` reference; identity is address
/// equality (see [`identity_eq`](Self::identity_eq)). Usable with both the
/// synchronous and asynchronous `compile` entry points.
#[derive(Clone, Copy)]
pub struct Callable<'compile, 'parse>(&'parse CallableKind<'compile, 'parse>);

impl<'compile: 'parse, 'parse> fmt::Debug for Callable<'compile, 'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Callable({})", self.name())
    }
}

impl<'compile: 'parse, 'parse> Callable<'compile, 'parse> {
    /// Allocates a callable in the compile arena and returns its handle.
    pub fn new(arena: &'compile Bump, kind: CallableKind<'compile, 'parse>) -> Self {
        Callable(arena.alloc(kind))
    }

    pub fn kind(&self) -> &CallableKind<'compile, 'parse> {
        self.0
    }

    /// The callable's name.
    ///
    /// Matches Dart: `String get name => declaration.name`.
    pub fn name(&self) -> &str {
        match self.0 {
            CallableKind::UserDefined(u) => u.name(),
            CallableKind::BuiltIn(b) => b.name(),
            CallableKind::PlainCss(p) => &p.name,
        }
    }

    /// Identity hash — the address of the shared `CallableKind` allocation.
    /// Matches Go: `reflect.ValueOf(ref).Pointer()` / Dart: `callable.hashCode`
    /// (identity hash).
    pub fn identity_hash(&self) -> usize {
        self.0 as *const CallableKind<'compile, 'parse> as usize
    }

    /// Identity comparison across value lifetimes (address equality of the
    /// shared `CallableKind` allocation). Same semantics as `PartialEq`, but
    /// usable when the two callables have different `'parse` parameters —
    /// `Value<'parse>` is invariant, so cross-lifetime `Value::equals` needs it.
    pub fn identity_eq(&self, other: &Callable<'compile, 'parse>) -> bool {
        self.identity_hash() == other.identity_hash()
    }
}

impl PartialEq for Callable<'_, '_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl Eq for Callable<'_, '_> {}

impl Hash for Callable<'_, '_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.0 as *const CallableKind<'_, '_>).hash(state)
    }
}

// === BuiltInCallable impl ===

impl<'compile: 'parse, 'parse> BuiltInCallable<'compile, 'parse> {
    /// Creates a function with a single parameter declaration and callback.
    ///
    /// The declaration is parsed from `params_str`, which uses the same
    /// syntax as an argument list written in Sass (not including
    /// parentheses); it may be empty for a function of no arguments.
    /// `url_str` is the URL of the module the function is defined in.
    pub fn new(
        name: String,
        params: ParameterList<'parse>,
        callback: SyncBuiltInCallback<'compile, 'parse>,
    ) -> Self {
        #[cfg(feature = "async")]
        let callback = BuiltInCallback::Sync(callback);
        BuiltInCallable {
            name,
            overloads: Rc::new(vec![BuiltInOverload { params, callback }]),
            accepts_content: false,
            deprecation_warning: None,
        }
    }

    #[rust_sass_macros::async_impl]
    pub fn new_async(
        name: String,
        params: ParameterList<'parse>,
        callback: AsyncBuiltInCallback<'compile, 'parse>,
    ) -> Self {
        BuiltInCallable {
            name,
            overloads: Rc::new(vec![BuiltInOverload {
                params,
                callback: BuiltInCallback::Async(callback),
            }]),
            accepts_content: false,
            deprecation_warning: None,
        }
    }

    pub fn new_overloaded(name: String, overloads: Vec<BuiltInOverload<'compile, 'parse>>) -> Self {
        BuiltInCallable {
            name,
            overloads: Rc::new(overloads),
            accepts_content: false,
            deprecation_warning: None,
        }
    }

    /// The callable's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this callable could potentially accept an `@content` block.
    /// This can only be true for mixins.
    pub fn accepts_content(&self) -> bool {
        self.accepts_content
    }

    /// Sets whether this callable accepts an `@content` block (used when
    /// registering built-in mixins that forward content).
    pub fn set_accepts_content(&mut self, v: bool) {
        self.accepts_content = v;
    }

    /// Returns a copy of this callable with the given name (the overloads
    /// are shared, not re-registered).
    pub fn with_name(&self, name: String) -> Self {
        BuiltInCallable {
            name,
            overloads: Rc::clone(&self.overloads),
            accepts_content: self.accepts_content,
            deprecation_warning: self.deprecation_warning.clone(),
        }
    }

    /// The `(module, name)` metadata recorded by
    /// [`with_deprecation_warning`](Self::with_deprecation_warning), used for
    /// introspection of global built-in deprecations.
    pub fn deprecation_warning(&self) -> Option<&(String, String)> {
        self.deprecation_warning.as_ref()
    }

    /// Returns the best-matching overload for the given positional count and named arguments.
    ///
    /// Matches Dart: `BuiltInCallable.callbackFor` (lines 108-134)
    /// - Exact match: `params.matches(positional, names)` returns true → return immediately
    /// - Fuzzy match: pick smallest `|paramCount - positional|`
    /// - Tiebreak: same abs distance → prefer more parameters (positive distance)
    fn find_overload(
        &self,
        positional: usize,
        names: &HashSet<&str>,
    ) -> SassResult<&BuiltInOverload<'compile, 'parse>> {
        let mut fuzzy_match: Option<&BuiltInOverload<'compile, 'parse>> = None;
        let mut min_mismatch_distance: i32 = 0;

        for overload in self.overloads.iter() {
            if overload.params.matches(positional, names) {
                return Ok(overload);
            }

            let mismatch_distance = overload.params.parameters.len() as i32 - positional as i32;

            if let Some(_fm) = fuzzy_match {
                if mismatch_distance.abs() > min_mismatch_distance.abs() {
                    continue;
                }
                if mismatch_distance.abs() == min_mismatch_distance.abs() && mismatch_distance < 0 {
                    continue;
                }
            }

            min_mismatch_distance = mismatch_distance;
            fuzzy_match = Some(overload);
        }

        match fuzzy_match {
            Some(matched) => Ok(matched),
            None => Err(Box::new(SassError::Script {
                message: format!("BuiltInCallable {} may not have empty overloads", self.name),
                argument_name: None,
            })),
        }
    }

    /// Returns the parameter declaration and callback for the given
    /// positional and named arguments.
    ///
    /// If no exact match is found, finds the closest approximation. Note that
    /// this doesn't guarantee that `positional` and `names` are valid for the
    /// returned parameters.
    ///
    /// Matches Dart: `BuiltInCallable.callbackFor` (lines 108-134)
    /// - Exact match: `params.matches(positional, names)` returns true → return immediately
    /// - Fuzzy match: pick smallest `|paramCount - positional|`
    /// - Tiebreak: same abs distance → prefer more parameters (positive distance)
    pub fn callback_for(
        &self,
        positional: usize,
        names: &HashSet<&str>,
    ) -> SassResult<&BuiltInOverload<'compile, 'parse>> {
        self.find_overload(positional, names)
    }

    /// Creates a built-in function from a signature string.
    /// Panics if the parameter string fails to parse (hardcoded strings only).
    /// Matches Dart: `BuiltInCallable.function` — parses
    /// `@function name(params)` so the declaration span renders correctly.
    /// Matches Go: `MustNewBuiltInCallableFunction`
    pub fn function(
        name: &str,
        params_str: &str,
        url_str: &str,
        arena: &'compile Bump,
        callback: SyncBuiltInCallback<'compile, 'parse>,
    ) -> Self {
        let contents = format!("@function {name}({params_str}) {{");
        let params = parse_parameter_list(&contents, url_str, arena).unwrap_or_else(|e| {
            panic!("BUG: ParseParameterList for {name}({params_str}) in {url_str}: {e}")
        });
        let mut bic = BuiltInCallable::new(name.to_string(), params, callback);
        bic.deprecation_warning = None;
        bic
    }

    /// Like `function`, but wraps an `AsyncBuiltInCallback` in
    /// `BuiltInCallback::Async`.
    #[rust_sass_macros::async_impl]
    pub fn function_async(
        name: &str,
        params_str: &str,
        url_str: &str,
        arena: &'compile Bump,
        callback: AsyncBuiltInCallback<'compile, 'parse>,
    ) -> Self {
        let contents = format!("@function {name}({params_str}) {{");
        let params = parse_parameter_list(&contents, url_str, arena).unwrap_or_else(|e| {
            panic!("BUG: ParseParameterList for {name}({params_str}) in {url_str}: {e}")
        });
        let mut bic = BuiltInCallable::new_async(name.to_string(), params, callback);
        bic.deprecation_warning = None;
        bic
    }

    /// Creates a built-in mixin from a signature string.
    /// Panics if the parameter string fails to parse (hardcoded strings only).
    /// Matches Dart: `BuiltInCallable.mixin` — parses `@mixin {name}(...)` so
    /// the declaration span renders `@mixin apply($mixin, $args...)`.
    pub fn mixin(
        name: &str,
        params_str: &str,
        url_str: &str,
        arena: &'compile Bump,
        callback: SyncBuiltInCallback<'compile, 'parse>,
    ) -> Self {
        let contents = format!("@mixin {name}({params_str}) {{");
        let params = parse_parameter_list(&contents, url_str, arena).unwrap_or_else(|e| {
            panic!("BUG: ParseParameterList for {name}({params_str}) in {url_str}: {e}")
        });
        let mut bic = BuiltInCallable::new(name.to_string(), params, callback);
        bic.deprecation_warning = None;
        bic
    }

    /// Creates a built-in mixin from a signature string.
    /// Panics if the parameter string fails to parse (hardcoded strings only).
    /// Matches Dart: `BuiltInCallable.mixin` — parses `@mixin {name}(...)` so
    /// the declaration span renders `@mixin apply($mixin, $args...)`.
    #[rust_sass_macros::async_impl]
    pub fn mixin_async(
        name: &str,
        params_str: &str,
        url_str: &str,
        arena: &'compile Bump,
        callback: AsyncBuiltInCallback<'compile, 'parse>,
    ) -> Self {
        let contents = format!("@mixin {name}({params_str}) {{");
        let params = parse_parameter_list(&contents, url_str, arena).unwrap_or_else(|e| {
            panic!("BUG: ParseParameterList for {name}({params_str}) in {url_str}: {e}")
        });
        let mut bic = BuiltInCallable::new_async(name.to_string(), params, callback);
        bic.deprecation_warning = None;
        bic
    }

    /// Creates a function with multiple implementations. Each entry pairs a
    /// parameter declaration (same syntax as [`function`](Self::function))
    /// with the callback to run when that declaration matches.
    /// Panics if any parameter string fails to parse (hardcoded strings only).
    /// Matches Go: `MustNewBuiltInCallableOverloadedFunction`
    pub fn overloaded_function(
        name: &str,
        url_str: &str,
        overload_defs: Vec<(&str, SyncBuiltInCallback<'compile, 'parse>)>,
        arena: &'compile Bump,
    ) -> Self {
        let overloads: Vec<BuiltInOverload<'compile, 'parse>> = overload_defs
            .into_iter()
            .map(|(params_str, callback)| {
                let contents = format!("@function {name}({params_str}) {{");
                let params = parse_parameter_list(&contents, url_str, arena).unwrap_or_else(|e| {
                    panic!("BUG: ParseParameterList for {name}({params_str}): {e}")
                });
                #[cfg(feature = "async")]
                let callback = BuiltInCallback::Sync(callback);
                BuiltInOverload { params, callback }
            })
            .collect();
        BuiltInCallable {
            name: name.to_string(),
            overloads: Rc::new(overloads),
            accepts_content: false,
            deprecation_warning: None,
        }
    }

    /// Like `overloaded_function`, but wraps `AsyncBuiltInCallback`s in
    /// `BuiltInCallback::Async`.
    #[rust_sass_macros::async_impl]
    pub fn overloaded_function_async(
        name: &str,
        url_str: &str,
        overload_defs: Vec<(&str, AsyncBuiltInCallback<'compile, 'parse>)>,
        arena: &'compile Bump,
    ) -> Self {
        let overloads: Vec<BuiltInOverload<'compile, 'parse>> = overload_defs
            .into_iter()
            .map(|(params_str, callback)| {
                let contents = format!("@function {name}({params_str}) {{");
                let params = parse_parameter_list(&contents, url_str, arena).unwrap_or_else(|e| {
                    panic!("BUG: ParseParameterList for {name}({params_str}): {e}")
                });
                BuiltInOverload {
                    params,
                    callback: BuiltInCallback::Async(callback),
                }
            })
            .collect();
        BuiltInCallable {
            name: name.to_string(),
            overloads: Rc::new(overloads),
            accepts_content: false,
            deprecation_warning: None,
        }
    }

    // --- sync-build twins: same names, `Sync` callbacks (the closures
    // convert in place under #[maybe_async]) ---

    #[rust_sass_macros::sync_impl]
    pub fn new_async(
        name: String,
        params: ParameterList<'parse>,
        callback: SyncBuiltInCallback<'compile, 'parse>,
    ) -> Self {
        Self::new(name, params, callback)
    }

    #[rust_sass_macros::sync_impl]
    pub fn function_async(
        name: &str,
        params_str: &str,
        url_str: &str,
        arena: &'compile Bump,
        callback: SyncBuiltInCallback<'compile, 'parse>,
    ) -> Self {
        Self::function(name, params_str, url_str, arena, callback)
    }

    #[rust_sass_macros::sync_impl]
    pub fn mixin_async(
        name: &str,
        params_str: &str,
        url_str: &str,
        arena: &'compile Bump,
        callback: SyncBuiltInCallback<'compile, 'parse>,
    ) -> Self {
        let contents = format!("@mixin {name}({params_str}) {{");
        let params = parse_parameter_list(&contents, url_str, arena).unwrap_or_else(|e| {
            panic!("BUG: ParseParameterList for {name}({params_str}) in {url_str}: {e}")
        });
        let mut bic = BuiltInCallable::new(name.to_string(), params, callback);
        bic.deprecation_warning = None;
        bic
    }

    #[rust_sass_macros::sync_impl]
    pub fn overloaded_function_async(
        name: &str,
        url_str: &str,
        overload_defs: Vec<(&str, SyncBuiltInCallback<'compile, 'parse>)>,
        arena: &'compile Bump,
    ) -> Self {
        Self::overloaded_function(name, url_str, overload_defs, arena)
    }
}

impl<'compile: 'parse, 'parse: 'compile> BuiltInCallable<'compile, 'parse> {
    /// Returns a copy of this callable that emits a GlobalBuiltin deprecation
    /// warning before invoking the original callback. If `new_name` is set,
    /// it is used in the deprecation message instead of the callable's own
    /// name.
    ///
    /// Matches Go: BuiltInCallable.WithDeprecationWarning / Dart:
    /// BuiltInCallable.withDeprecationWarning — every overload callback is
    /// wrapped. The (module, name) metadata is also retained for
    /// introspection.
    #[rust_sass_macros::async_impl]
    pub fn with_deprecation_warning(&self, module: &str, new_name: Option<&str>) -> Self {
        let n = new_name.unwrap_or(&self.name).to_string();
        let new_overloads: Vec<BuiltInOverload<'compile, 'parse>> = self
            .overloads
            .iter()
            .map(|ov| {
                let params = ov.params.clone();
                let module = module.to_string();
                let name = n.clone();
                match &ov.callback {
                    BuiltInCallback::Sync(original) => BuiltInOverload {
                        params,
                        callback: BuiltInCallback::Sync(with_deprecation_sync_wrapper(
                            Rc::clone(original),
                            module,
                            name,
                        )),
                    },
                    BuiltInCallback::Async(original) => BuiltInOverload {
                        params,
                        callback: BuiltInCallback::Async(with_deprecation_async_wrapper(
                            Rc::clone(original),
                            module,
                            name,
                        )),
                    },
                }
            })
            .collect();
        BuiltInCallable {
            name: self.name.clone(),
            overloads: Rc::new(new_overloads),
            accepts_content: self.accepts_content,
            deprecation_warning: Some((module.to_string(), n)),
        }
    }

    /// Sync-build variant: `BuiltInCallback` is the `Sync` alias here, so
    /// every overload is wrapped directly.
    #[rust_sass_macros::sync_impl]
    pub fn with_deprecation_warning(&self, module: &str, new_name: Option<&str>) -> Self {
        let n = new_name.unwrap_or(&self.name).to_string();
        let new_overloads: Vec<BuiltInOverload<'compile, 'parse>> = self
            .overloads
            .iter()
            .map(|ov| {
                let params = ov.params.clone();
                let module = module.to_string();
                let name = n.clone();
                BuiltInOverload {
                    params,
                    callback: with_deprecation_sync_wrapper(Rc::clone(&ov.callback), module, name),
                }
            })
            .collect();
        BuiltInCallable {
            name: self.name.clone(),
            overloads: Rc::new(new_overloads),
            accepts_content: self.accepts_content,
            deprecation_warning: Some((module.to_string(), n)),
        }
    }
}

/// Wraps a `SyncBuiltInCallback` with a deprecation warning, returning a new
/// `SyncBuiltInCallback` that emits the warning before invoking the original.
fn with_deprecation_sync_wrapper<'compile, 'parse>(
    original: SyncBuiltInCallback<'compile, 'parse>,
    module: String,
    name: String,
) -> SyncBuiltInCallback<'compile, 'parse>
where
    'compile: 'parse,
{
    // The message is constant per wrapper — format it once at registration
    // instead of on every built-in invocation.
    let message = global_builtin_deprecation_message(&module, &name);
    Rc::new(
        move |config: &EvalConfig<'compile, 'parse>,
              state: &mut EvalState<'compile, 'parse>,
              args: Vec<Value<'parse>>,
              arena: &'compile Bump|
              -> SassResult<Value<'parse>> {
            warn_for_global_builtin_message(config, state, &message)?;
            original(config, state, args, arena)
        },
    )
}

/// Emits a deprecation warning for a global built-in function that is now
/// available as function `name` in built-in module `module` (Dart's
/// `warnForGlobalBuiltIn` in `callable/async_built_in.dart`).
fn global_builtin_deprecation_message(module: &str, name: &str) -> String {
    format!(
        "Global built-in functions are deprecated and will be removed in \
         Dart Sass 3.0.0.\nUse {module}.{name} instead.\n\n\
         More info and automated migrator: https://sass-lang.com/d/import"
    )
}

/// Wraps an `AsyncBuiltInCallback` with a deprecation warning, returning a new
/// `AsyncBuiltInCallback` that emits the warning before invoking the original.
/// The `'parse: 'compile` bound is required for the closure's higher-ranked
/// trait coercion (see docs/ref/macros.md, "Lifetime invariance").
#[rust_sass_macros::async_impl]
fn with_deprecation_async_wrapper<'compile, 'parse>(
    original: AsyncBuiltInCallback<'compile, 'parse>,
    module: String,
    name: String,
) -> AsyncBuiltInCallback<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let message = global_builtin_deprecation_message(&module, &name);
    Rc::new(
        move |config, state, args, arena| -> LocalBoxFuture<'_, SassResult<Value<'parse>>> {
            let message = message.clone();
            let original = original.clone();
            Box::pin(async move {
                warn_for_global_builtin_message(config, state, &message)?;
                original(config, state, args, arena).await
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use crate::ast::sass::statement::FunctionRule;
    use crate::ast::sass::statement::MixinRule;
    use bumpalo::Bump;

    use super::*;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_null::NullExpression;
    use crate::ast::sass::parameter::Parameter;
    use crate::common::file_span::{FileSpan, BOGUS_SPAN};
    use crate::common::source_span_file_source::FileSource;
    use crate::value::{Value, ValueKind};

    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    fn noop_callback<'compile, 'parse>() -> SyncBuiltInCallback<'compile, 'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        Rc::new(
            |_config: &EvalConfig<'compile, 'parse>,
             _state: &mut EvalState<'compile, 'parse>,
             _args: Vec<Value<'parse>>,
             arena: &'compile Bump|
             -> SassResult<Value<'parse>> {
                Ok(Value::new_with_arena(arena, ValueKind::Null))
            },
        )
    }

    #[cfg(feature = "async")]
    fn noop_callback_cfg<'compile, 'parse>() -> BuiltInCallback<'compile, 'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        BuiltInCallback::Sync(noop_callback())
    }

    #[cfg(not(feature = "async"))]
    fn noop_callback_cfg<'compile, 'parse>() -> SyncBuiltInCallback<'compile, 'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        noop_callback()
    }

    fn empty_params<'compile, 'parse>(arena: &'compile Bump) -> ParameterList<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        ParameterList::empty(make_span(arena, ""))
    }

    fn required_param<'compile, 'parse>(arena: &'compile Bump, name: &str) -> Parameter<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let span = make_span(arena, name);
        Parameter::new(name.to_string(), span, None)
    }

    fn optional_param<'compile, 'parse>(arena: &'compile Bump, name: &str) -> Parameter<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let span = make_span(arena, name);
        Parameter::new(
            name.to_string(),
            span,
            Some(Expression::Null(NullExpression::new(make_span(
                arena, "null",
            )))),
        )
    }

    fn params_with<'compile, 'parse>(
        arena: &'compile Bump,
        required: &[&str],
        optional: &[&str],
    ) -> ParameterList<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let mut pars: Vec<Parameter<'parse>> = Vec::new();
        for name in required {
            pars.push(required_param(arena, name));
        }
        for name in optional {
            pars.push(optional_param(arena, name));
        }
        ParameterList::new(pars, make_span(arena, "(...)"), None)
    }

    // --- BuiltInCallable ---

    #[test]
    fn test_built_in_callable_new() {
        let arena = Bump::new();
        let pl = empty_params(&arena);
        let fn_ = BuiltInCallable::new("test-fn".into(), pl, noop_callback());
        assert_eq!(fn_.name(), "test-fn");
    }

    #[test]
    fn test_built_in_callable_new_overloaded() {
        let arena = Bump::new();
        let pl1 = empty_params(&arena);
        let pl2 = empty_params(&arena);
        let overloads = vec![
            BuiltInOverload {
                params: pl1,
                callback: noop_callback_cfg(),
            },
            BuiltInOverload {
                params: pl2,
                callback: noop_callback_cfg(),
            },
        ];
        let fn_ = BuiltInCallable::new_overloaded("overloaded".into(), overloads);
        assert_eq!(fn_.name(), "overloaded");
    }

    #[test]
    fn test_built_in_callable_accepts_content_default() {
        let arena = Bump::new();
        let pl = empty_params(&arena);
        let fn_ = BuiltInCallable::new("test".into(), pl, noop_callback());
        assert!(!fn_.accepts_content());
    }

    #[test]
    fn test_built_in_callable_set_accepts_content() {
        let arena = Bump::new();
        let pl = empty_params(&arena);
        let mut fn_ = BuiltInCallable::new("test".into(), pl, noop_callback());
        fn_.set_accepts_content(true);
        assert!(fn_.accepts_content());
    }

    #[test]
    fn test_built_in_callable_with_name() {
        let arena = Bump::new();
        let pl = empty_params(&arena);
        let fn_ = BuiltInCallable::new("original".into(), pl, noop_callback());
        let renamed = fn_.with_name("renamed".into());
        assert_eq!(fn_.name(), "original");
        assert_eq!(renamed.name(), "renamed");
    }

    // --- callback_for ---

    #[test]
    fn test_callback_for_exact_match_single_overload() {
        let arena = Bump::new();
        let pl = empty_params(&arena);
        let fn_ = BuiltInCallable::new("test".into(), pl.clone(), noop_callback());

        let result = fn_.callback_for(0, &HashSet::new()).unwrap();
        assert_eq!(result.params.span, pl.span);
    }

    #[test]
    fn test_callback_for_exact_match_first_wins() {
        let arena = Bump::new();
        let pl1 = empty_params(&arena);
        let pl2 = params_with(&arena, &[], &["a"]);
        let overloads = vec![
            BuiltInOverload {
                params: pl1.clone(),
                callback: noop_callback_cfg(),
            },
            BuiltInOverload {
                params: pl2,
                callback: noop_callback_cfg(),
            },
        ];
        let fn_ = BuiltInCallable::new_overloaded("test".into(), overloads);

        let result = fn_.callback_for(0, &HashSet::new()).unwrap();
        assert_eq!(result.params.span, pl1.span);
    }

    #[test]
    fn test_callback_for_exact_match_second_overload() {
        let arena = Bump::new();
        let pl1 = params_with(&arena, &["a"], &[]);
        let pl2 = empty_params(&arena);
        let overloads = vec![
            BuiltInOverload {
                params: pl1,
                callback: noop_callback_cfg(),
            },
            BuiltInOverload {
                params: pl2.clone(),
                callback: noop_callback_cfg(),
            },
        ];
        let fn_ = BuiltInCallable::new_overloaded("test".into(), overloads);

        let result = fn_.callback_for(0, &HashSet::new()).unwrap();
        assert_eq!(result.params.span, pl2.span);
    }

    #[test]
    fn test_callback_for_fuzzy_match_closer_distance() {
        let arena = Bump::new();
        let pl3 = params_with(&arena, &["a", "b", "c"], &[]);
        let pl2 = params_with(&arena, &["x", "y"], &[]);
        let overloads = vec![
            BuiltInOverload {
                params: pl3,
                callback: noop_callback_cfg(),
            },
            BuiltInOverload {
                params: pl2.clone(),
                callback: noop_callback_cfg(),
            },
        ];
        let fn_ = BuiltInCallable::new_overloaded("test".into(), overloads);

        let result = fn_.callback_for(1, &HashSet::new()).unwrap();
        assert_eq!(result.params.span, pl2.span);
    }

    #[test]
    fn test_callback_for_fuzzy_match_tiebreak() {
        let arena = Bump::new();
        let pl_fewer = params_with(&arena, &[], &["a"]);
        let pl_more = params_with(&arena, &[], &["a", "b", "c"]);
        let overloads = vec![
            BuiltInOverload {
                params: pl_fewer,
                callback: noop_callback_cfg(),
            },
            BuiltInOverload {
                params: pl_more.clone(),
                callback: noop_callback_cfg(),
            },
        ];
        let fn_ = BuiltInCallable::new_overloaded("test".into(), overloads);

        // positional=2: pl_fewer distance = 1-2 = -1 (abs=1), pl_more distance = 3-2 = 1 (abs=1)
        let result = fn_.callback_for(2, &HashSet::new()).unwrap();
        assert_eq!(result.params.span, pl_more.span);
    }

    #[test]
    fn test_callback_for_no_match_empty_overloads() {
        let fn_ = BuiltInCallable::new_overloaded("test".into(), vec![]);

        match *fn_.callback_for(0, &HashSet::new()).unwrap_err() {
            SassError::Script { message, .. } => {
                assert_eq!(message, "BuiltInCallable test may not have empty overloads");
            }
            _ => panic!("expected Script error"),
        }
    }

    // --- PlainCssCallable ---

    #[test]
    fn test_plain_css_callable_name() {
        let pc = PlainCssCallable {
            name: "rotate".into(),
        };
        assert_eq!(pc.name, "rotate");
    }

    #[test]
    fn test_plain_css_callable_new() {
        let pc = PlainCssCallable {
            name: "calc".into(),
        };
        assert_eq!(pc.name, "calc");
    }

    // --- UserDefinedCallable ---

    fn mixin_declaration<'parse>(name: &str) -> CallableDeclaration<'parse> {
        CallableDeclaration::Mixin(MixinRule::new(
            name.to_string(),
            ParameterList::empty(BOGUS_SPAN),
            vec![],
            BOGUS_SPAN,
            None,
        ))
    }

    fn function_declaration<'parse>(name: &str) -> CallableDeclaration<'parse> {
        CallableDeclaration::Function(FunctionRule::new(
            name.to_string(),
            ParameterList::empty(BOGUS_SPAN),
            vec![],
            BOGUS_SPAN,
            None,
        ))
    }

    #[test]
    fn test_user_defined_callable_new() {
        let arena = Bump::new();
        let ud = UserDefinedCallable::new(
            function_declaration("my_fn"),
            Environment::new(&arena),
            false,
        );
        // Matches Dart: name is derived from the declaration (normalized).
        assert_eq!(ud.name(), "my-fn");
        assert!(ud.parameters().is_empty());
        assert!(!ud.in_dependency);
    }

    #[test]
    fn test_user_defined_callable_mixin_declaration() {
        let arena = Bump::new();
        let ud = UserDefinedCallable::new(
            mixin_declaration("my-mixin"),
            Environment::new(&arena),
            false,
        );
        assert!(matches!(ud.declaration, CallableDeclaration::Mixin(_)));
        assert_eq!(ud.name(), "my-mixin");
    }

    #[test]
    fn test_user_defined_callable_in_dependency() {
        let arena = Bump::new();
        let ud = UserDefinedCallable::new(
            function_declaration("dep-fn"),
            Environment::new(&arena),
            true,
        );
        assert!(ud.in_dependency);
    }

    // --- Callable identity ---

    #[test]
    fn test_callable_identity_same_rc() {
        let arena = Bump::new();
        let pl = empty_params(&arena);
        let builtin = BuiltInCallable::new("test".into(), pl, noop_callback());
        let c1 = Callable::new(&arena, CallableKind::BuiltIn(builtin));
        let c2 = c1;

        assert!(c1.eq(&c2));
    }

    #[test]
    fn test_callable_identity_different_rc() {
        let arena = Bump::new();
        let pl1 = empty_params(&arena);
        let pl2 = empty_params(&arena);
        let builtin1 = BuiltInCallable::new("a".into(), pl1, noop_callback());
        let builtin2 = BuiltInCallable::new("a".into(), pl2, noop_callback());
        let c1 = Callable::new(&arena, CallableKind::BuiltIn(builtin1));
        let c2 = Callable::new(&arena, CallableKind::BuiltIn(builtin2));

        assert!(!c1.eq(&c2));
    }

    #[test]
    fn test_callable_identity_hash() {
        let arena = Bump::new();
        let pl = empty_params(&arena);
        let builtin = BuiltInCallable::new("test".into(), pl, noop_callback());
        let c1 = Callable::new(&arena, CallableKind::BuiltIn(builtin));
        let c2 = c1;

        assert_eq!(c1.identity_hash(), c2.identity_hash());
        assert_ne!(c1.identity_hash(), 0);
    }

    #[test]
    fn test_callable_builtin_via_kind() {
        let arena = Bump::new();
        let pl = empty_params(&arena);
        let builtin = BuiltInCallable::new("test".into(), pl, noop_callback());
        let c = Callable::new(&arena, CallableKind::BuiltIn(builtin));

        match c.kind() {
            CallableKind::BuiltIn(b) => assert_eq!(b.name(), "test"),
            _ => panic!("expected BuiltIn variant"),
        }
    }

    #[test]
    fn test_callable_plain_css_via_kind() {
        let arena = Bump::new();
        let pc = PlainCssCallable {
            name: "css-fn".into(),
        };
        let c = Callable::new(&arena, CallableKind::PlainCss(pc));

        match c.kind() {
            CallableKind::PlainCss(p) => assert_eq!(p.name, "css-fn"),
            _ => panic!("expected PlainCss variant"),
        }
    }

    #[test]
    fn test_callable_user_defined_via_kind() {
        let arena = Bump::new();
        let ud = UserDefinedCallable::new(mixin_declaration("ud"), Environment::new(&arena), false);
        let c = Callable::new(&arena, CallableKind::UserDefined(ud));

        assert_eq!(c.name(), "ud");
        match c.kind() {
            CallableKind::UserDefined(u) => {
                assert!(matches!(u.declaration, CallableDeclaration::Mixin(_)));
                assert!(!u.in_dependency);
            }
            _ => panic!("expected UserDefined variant"),
        }
    }
}
