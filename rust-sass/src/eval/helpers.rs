// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/evaluate.dart (helper sections: _exception,
//   _stackTrace, _withoutSlash, _withStackFrame, _withEnvironment, _withParent,
//   _copyParentAfterSibling, _addChild, _withStyleRule, _withMediaQueries,
//   _performInterpolation*, _serialize, _expressionNode, _evaluateArguments,
//   _evaluateMacroArguments, _addRestMap, _runUserDefinedCallable,
//   _verifyArguments, _mergeMediaQueries, _addExceptionSpan, _addExceptionTrace,
//   _addErrorSpan, _withFakeStylesheet, _hasCssNesting, _styleRule) +
//   lib/src/evaluation_context.dart (withEvaluationContext)
// go-source: go/eval/evaluate.go + go/eval/evaluate_helpers.go

//! Free-function evaluation helpers, split out of the visitor.
//!
//! Matches Dart: the `_`-prefixed helpers on `_EvaluateVisitor`
//! (`lib/src/visitor/evaluate.dart`). Dart threads them through `this`; here
//! each takes `config` (shared) + `state` (`&mut`) separately so nested
//! closures can reborrow `state` through the chain (see `ref/eval.md` borrow
//! patterns). All wrapping is callback-based: there is no helper that wraps
//! an already-built `SassResult`.

use crate::ast::css::media_query::CssMediaQuery;
use crate::ast::css::media_query::MediaQueryMergeResult;
use crate::ast::css::modifiable_node::ModifiableCssNode;
use crate::ast::css::modifiable_node::ModifiableCssNodeKind;
use crate::ast::css::style_rule::CssStyleRule;
use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::expression_value::ValueExpression;
use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation::InterpolationPart;
use crate::ast::sass::interpolation_map::InterpolationMap;
use crate::ast::sass::parameter_list::ParameterList;
use crate::ast::sass::statement::CallableDeclaration;
use crate::callable::UserDefinedCallable;
use crate::common::file_span::SourceLocation;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::eval::import_cache::ImportCache;
use crate::eval::statement::evaluate_block;
use crate::eval::warn::warn;
use crate::eval::warn::warn_deprecation_span;
use crate::value::color_names::color_name_for;
use crate::value::ListSeparator;
use crate::value::SassArgumentList;
use crate::value::SassMap;
use std::collections::HashSet;
#[cfg(feature = "async")]
use std::ops::AsyncFnOnce;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::statement::Stylesheet;
use crate::callable::{Callable, CallableKind, PlainCssCallable};
use crate::common::ast_node::AstNode;
use crate::common::exception::{Frame, SassError, SassResult, Trace};
use crate::common::file_span::FileSpan;
use crate::deprecation::SLASH_DIV;
use crate::environment::Environment;
use crate::eval::expression::evaluate_expression;
use crate::eval::importer::Importer;
use crate::eval::{EvalConfig, EvalState, StackFrame};
use crate::url::SassUrl;
use crate::value::ValueKind;
use crate::value::{SassNumber, Value};
use bumpalo::Bump;
use indexmap::IndexMap;

// ===========================================================================
// exception — creates a Runtime error with span + trace
// Go: evaluate_helpers.go:104
// ===========================================================================
// Matches Dart: `_exception(message, [span])` (evaluate.dart:4700).
// Uses the explicit span, else the top stack frame's span (Dart: `_stack.last`
// span). Always builds a full trace via [`stack_trace`]. Callers that raise
// type-check failures as spanless `Script` errors rely on [`add_exception_span`]
// at the call site to attach span + trace instead; `exception()` is for sites
// that already know the span (rest-arg dispatch, keyword-rest, "finished
// without @return", plain-CSS errors).

pub(crate) fn exception<'compile: 'parse, 'parse>(
    state: &mut EvalState<'compile, 'parse>,
    message: String,
    span: Option<FileSpan<'parse>>,
) -> SassError {
    let s = span
        .or_else(|| state.stack.as_ref().map(|f| f.span))
        .unwrap_or(FileSpan::new(None, 0, 0));
    let trace = stack_trace(state, Some(s));
    SassError::Runtime {
        message,
        span: SourceSpanWithContext::from_file_span(&s).unwrap_or_else(|_| bogus_span_ctx()),
        trace,
        cause: None,
        loaded_urls: vec![],
    }
}

// ===========================================================================
// stackTrace — builds a Trace from the state.stack linked list
// Go: evaluate_helpers.go:975
// ===========================================================================
// Matches Dart: `_stackTrace([span])` (evaluate.dart:4673). Stack frames pair
// the pre-push member name with the call-site span; the optional `span` becomes
// the innermost frame under the current member. Lines/columns are 1-based
// (Dart 0-based + 1) and URIs pass through [`humanize_frame`].
// Single-frame order quirk: one stored frame plus `Some(span)` yields
// [stored, span] rather than Dart's [span, stored]; multi-frame order matches.

pub(crate) fn stack_trace<'compile, 'parse>(
    state: &mut EvalState<'compile, 'parse>,
    span: Option<FileSpan<'parse>>,
) -> Trace
where
    'compile: 'parse,
{
    let mut frames: Vec<Frame> = Vec::new();
    let mut current = state.stack.as_ref();
    while let Some(frame) = current {
        let mut f = Frame {
            uri: frame.span.source_url().cloned(),
            line: frame.span.start_location().line + 1,
            column: frame.span.start_location().column + 1,
            member: frame.name.clone(),
        };
        humanize_frame(state.import_cache.as_ref(), &mut f);
        frames.push(f);
        current = frame.parent.as_ref();
    }
    // Step 2: Reverse to outermost→innermost
    frames.reverse();
    // Step 3: Append span frame at end
    if let Some(s) = span {
        let mut f = Frame {
            uri: s.source_url().cloned(),
            line: s.start_location().line + 1,
            column: s.start_location().column + 1,
            member: state.member.clone(),
        };
        humanize_frame(state.import_cache.as_ref(), &mut f);
        frames.push(f);
    }
    // Step 4: Reverse to [span_frame, innermost, ..., outermost]
    frames.reverse();
    Trace::new(frames)
}

// ===========================================================================
// humanizeFrame — replaces URI with human-readable form via import_cache
// Go: evaluate_helpers.go:1007
// ===========================================================================
// Matches Dart: `_stackFrame(member, span)` (evaluate.dart:4663), which maps
// the span URL through `importCache.humanize`. Here the humanization is split
// out so [`stack_trace`] can apply it per frame; unparseable humanized URLs
// leave the frame URI unchanged.

pub(crate) fn humanize_frame<'compile: 'parse, 'parse>(
    import_cache: Option<&ImportCache<'compile, 'parse>>,
    frame: &mut Frame,
) {
    if let Some(cache) = import_cache {
        if let Some(uri) = frame.uri.as_ref() {
            let humanized = cache.humanize(uri);
            if let Ok(h) = SassUrl::parse(&humanized) {
                frame.uri = Some(h);
            }
        }
    }
}

// ===========================================================================
// slashDivisionRecommendation — recursive math.div() recommendation
// Go: evaluate_statement.go:355
// Dart: evaluate.dart:4639 (local recommendation function in _withoutSlash)
// ===========================================================================
// Matches Dart: the local `recommendation()` closure inside `_withoutSlash`
// (evaluate.dart:4639). Recurses into slash pairs (`math.div(a, b)`); plain
// numbers render via display (NOT CSS) serialization.

fn slash_division_recommendation(num: &SassNumber) -> SassResult<String> {
    if num.has_slash() {
        let (n, d) = num.slash_pair().unwrap();
        let n_str = slash_division_recommendation(n)?;
        let d_str = slash_division_recommendation(d)?;
        Ok(format!("math.div({n_str}, {d_str})"))
    } else {
        num.to_display_string()
    }
}

// ===========================================================================
// withoutSlash — strips slash with deprecation warning
// Go: evaluate_statement.go:329
// Dart: evaluate.dart:4637
// ===========================================================================
// Matches Dart: `_withoutSlash(value, nodeForSpan)` (evaluate.dart:4637).
// A slash-separated number warns once per span (`SLASH_DIV`, deduped by
// `WarnKey`) with the recursive `math.div()` recommendation, then returns the
// number with the slash stripped (recursion into argument lists happens in
// `SassNumber::without_slash`, not here). Non-numbers pass through untouched.
// Callers apply this per destructured `@each` sub-item and per
// variable-declaration value, but never to modern `if()` branch values.

pub(crate) fn without_slash<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    val: Value<'parse>,
    span: FileSpan<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    if let ValueKind::Number(ref num) = &*val {
        if num.has_slash() {
            let rec = slash_division_recommendation(num)?;
            let message = format!(
                "Using / for division is deprecated and will be removed in Dart Sass 2.0.0.\n\n\
                 Recommendation: {rec}\n\n\
                 More info and automated migrator: https://sass-lang.com/d/slash-div"
            );
            warn_deprecation_span(config, state, &message, &SLASH_DIV, span)?;
            return Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(num.without_slash()),
            ));
        }
    }
    Ok(val)
}

// ===========================================================================
// withEvaluationContext — save/restore ec.default_warn_span
// Go: evaluate.go:256
// ===========================================================================
// Matches Dart: `withEvaluationContext(_EvaluationContext(this, node), …)`
// (evaluate.dart:712; context type in `evaluation_context.dart`). Dart installs
// a zone-scoped `EvaluationContext` carrying the current callable/import spans
// for host callbacks; Rust has no zones and no `EvaluationContext` type, so
// only the surviving observable piece — `default_warn_span`, the deprecation
// fallback span — is saved/restored around the callback (split rule). The
// callback runs even on error; restoration is unconditional, like Dart's
// `runZoned` teardown.

#[rust_sass_macros::async_impl]
pub(crate) async fn with_evaluation_context<'compile, 'parse, F, T>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    span: FileSpan<'parse>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> AsyncFnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<T>,
{
    let old = state.default_warn_span;
    state.default_warn_span = span;
    let result = f(config, state).await;
    state.default_warn_span = old;
    result
}

#[rust_sass_macros::sync_impl]
#[rust_sass_macros::must_be_sync]
pub(crate) async fn with_evaluation_context<'compile, 'parse, F, T>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    span: FileSpan<'parse>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> FnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<T>,
{
    let old = state.default_warn_span;
    state.default_warn_span = span;
    let result = f(config, state).await;
    state.default_warn_span = old;
    result
}

// ===========================================================================
// withFakeStylesheet — save/restore importer + stylesheet for temporary eval
// Go: evaluate.go:303
// ===========================================================================
// Matches Dart: `_withFakeStylesheet(importer, nodeWithSpan, callback)`
// (evaluate.dart:765). Used by the `run_expression`/`run_statement` REPL-style
// entries: swaps in the given importer plus an empty `Stylesheet` carrying the
// node's span, runs the callback, then restores both. Asserts in Dart that no
// stylesheet is active; here the old stylesheet is simply saved.

#[rust_sass_macros::async_impl]
pub(crate) async fn with_fake_stylesheet<'compile, 'parse, F, T>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    importer: Importer<'parse>,
    span: FileSpan<'parse>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> AsyncFnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<T>,
{
    let old_importer = std::mem::replace(&mut state.importer, importer);
    let old_stylesheet = state
        .stylesheet
        .replace(arena.alloc(Stylesheet::new(vec![], span)));
    let result = f(config, state).await;
    state.importer = old_importer;
    state.stylesheet = old_stylesheet;
    result
}

#[rust_sass_macros::sync_impl]
#[rust_sass_macros::must_be_sync]
pub(crate) async fn with_fake_stylesheet<'compile, 'parse, F, T>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    importer: Importer<'parse>,
    span: FileSpan<'parse>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> FnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<T>,
{
    let old_importer = std::mem::replace(&mut state.importer, importer);
    let old_stylesheet = state
        .stylesheet
        .replace(arena.alloc(Stylesheet::new(vec![], span)));
    let result = f(config, state).await;
    state.importer = old_importer;
    state.stylesheet = old_stylesheet;
    result
}

// ===========================================================================
// addExceptionSpan — runs callback, wraps Script → Runtime
// Go: evaluate_helpers.go:687
// ===========================================================================
// Matches Dart: `_addExceptionSpan(nodeWithSpan, callback, {addStackFrame})`
// (evaluate.dart:4736). Converts spanless `Script` failures into `Runtime`
// errors at the node's span, and `MultiSpanScript` into spanned `MultiSpan`
// via `with_member_use_span`; already-spanned errors pass through. `FileSpan`
// is passed eagerly here (Dart passes the `AstNode` to defer expensive span
// manufacture); `add_stack_frame: None` defaults to `true` (Dart's default).

#[rust_sass_macros::async_impl]
pub(crate) async fn add_exception_span<'compile: 'parse, 'parse, F, T>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    span: FileSpan<'parse>,
    add_stack_frame: Option<bool>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> AsyncFnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<T>,
{
    let stack_frame = add_stack_frame.unwrap_or(true);
    let result = f(config, state).await;
    match result {
        Ok(v) => Ok(v),
        Err(e) => match *e {
            SassError::Script { .. } => {
                let trace = stack_trace(state, if stack_frame { Some(span) } else { None });
                Err(Box::new(SassError::Runtime {
                    message: e.full_message(),
                    span: file_span_to_ctx(&span),
                    trace,
                    cause: None,
                    loaded_urls: vec![],
                }))
            }
            SassError::MultiSpanScript { .. } => {
                let trace = stack_trace(state, if stack_frame { Some(span) } else { None });
                Err(Box::new(
                    e.with_member_use_span(file_span_to_ctx(&span), trace),
                ))
            }
            other => Err(Box::new(other)),
        },
    }
}

#[rust_sass_macros::sync_impl]
#[rust_sass_macros::must_be_sync]
pub(crate) async fn add_exception_span<'compile: 'parse, 'parse, F, T>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    span: FileSpan<'parse>,
    add_stack_frame: Option<bool>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> FnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<T>,
{
    let stack_frame = add_stack_frame.unwrap_or(true);
    let result = f(config, state).await;
    match result {
        Ok(v) => Ok(v),
        Err(e) => match *e {
            SassError::Script { .. } => {
                let trace = stack_trace(state, if stack_frame { Some(span) } else { None });
                Err(Box::new(SassError::Runtime {
                    message: e.full_message(),
                    span: file_span_to_ctx(&span),
                    trace,
                    cause: None,
                    loaded_urls: vec![],
                }))
            }
            SassError::MultiSpanScript { .. } => {
                let trace = stack_trace(state, if stack_frame { Some(span) } else { None });
                Err(Box::new(
                    e.with_member_use_span(file_span_to_ctx(&span), trace),
                ))
            }
            other => Err(Box::new(other)),
        },
    }
}

// ===========================================================================
// addExceptionTrace — wraps SassException/MultiSpanSassException → Runtime
// Dart: _addExceptionTrace (evaluate.dart:4757) — catches SassException base
// class (Format, MultiSpan, Sass) and converts to SassRuntimeException
// Go: evaluate_helpers.go:789
// ===========================================================================
// Matches Dart: `_addExceptionTrace(callback)` (evaluate.dart:4757).
// Already-`Runtime` errors (and `MultiSpan` errors that already carry a trace)
// rethrow unchanged; other spanned failures gain a trace built from the
// error's own span as innermost frame plus the current stack, deduped by
// [`dedup_trace_frames`] so load-site `@use`/`@forward` errors report one
// frame. Callers wrap the load call while its frame is on the stack.

#[rust_sass_macros::async_impl]
pub(crate) async fn add_exception_trace<'compile: 'parse, 'parse, F, T>(
    state: &mut EvalState<'compile, 'parse>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> AsyncFnOnce(&'a mut EvalState<'compile, 'parse>) -> SassResult<T>,
{
    let result = f(state).await;
    match result {
        Ok(v) => Ok(v),
        // Dart `_addExceptionTrace`: `SassRuntimeException`s are rethrown
        // unchanged; other `SassException`s get a trace via `withTrace`, which
        // is `_stackTrace(error.span)` — the current stack frames plus the
        // error's span as the innermost frame (with the current member). This
        // runs at the point of the error (e.g. while a `@use` load frame is on
        // the stack), so callers must wrap the relevant call in
        // `add_exception_trace` (see the `@use`/`@forward` handlers).
        Err(e) if matches!(&*e, SassError::Format { .. } | SassError::Sass { .. }) => {
            let (message, span, cause, loaded_urls) = match *e {
                SassError::Format {
                    message,
                    span,
                    cause,
                    loaded_urls,
                    ..
                }
                | SassError::Sass {
                    message,
                    span,
                    cause,
                    loaded_urls,
                } => (message, span, cause, loaded_urls),
                _ => unreachable!(),
            };
            let mut trace = Trace::new(vec![Frame {
                uri: span.source_url.clone(),
                line: span.start.line + 1,
                column: span.start.column + 1,
                member: state.member.clone(),
            }]);
            trace.frames.extend(stack_trace(state, None).frames);
            let trace = dedup_trace_frames(trace);
            Err(Box::new(SassError::Runtime {
                message,
                span,
                trace,
                cause,
                loaded_urls,
            }))
        }
        Err(e) if matches!(&*e, SassError::MultiSpan { .. }) => {
            let SassError::MultiSpan {
                message,
                span,
                primary_label,
                secondary,
                original_source,
                cause,
                loaded_urls,
                trace,
            } = *e
            else {
                unreachable!()
            };
            // Mirrors Dart's `_addExceptionTrace`: a `MultiSpanSassRuntimeException`
            // (one that already carries a trace) is rethrown unchanged. Only a
            // trace-less `MultiSpanSassException` gets a trace attached here.
            if !trace.is_empty() {
                return Err(Box::new(SassError::MultiSpan {
                    message,
                    span,
                    primary_label,
                    secondary,
                    original_source,
                    cause,
                    loaded_urls,
                    trace,
                }));
            }
            let mut trace = Trace::new(vec![Frame {
                uri: span.source_url.clone(),
                line: span.start.line + 1,
                column: span.start.column + 1,
                member: state.member.clone(),
            }]);
            trace.frames.extend(stack_trace(state, None).frames);
            let trace = dedup_trace_frames(trace);
            Err(Box::new(SassError::MultiSpan {
                message,
                span,
                primary_label,
                secondary,
                original_source,
                cause,
                loaded_urls,
                trace,
            }))
        }
        Err(e) => Err(e),
    }
}

#[rust_sass_macros::sync_impl]
#[rust_sass_macros::must_be_sync]
pub(crate) async fn add_exception_trace<'compile: 'parse, 'parse, F, T>(
    state: &mut EvalState<'compile, 'parse>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> FnOnce(&'a mut EvalState<'compile, 'parse>) -> SassResult<T>,
{
    let result = f(state).await;
    match result {
        Ok(v) => Ok(v),
        // Dart `_addExceptionTrace`: `SassRuntimeException`s are rethrown
        // unchanged; other `SassException`s get a trace via `withTrace`, which
        // is `_stackTrace(error.span)` — the current stack frames plus the
        // error's span as the innermost frame (with the current member). This
        // runs at the point of the error (e.g. while a `@use` load frame is on
        // the stack), so callers must wrap the relevant call in
        // `add_exception_trace` (see the `@use`/`@forward` handlers).
        Err(e) if matches!(&*e, SassError::Format { .. } | SassError::Sass { .. }) => {
            let (message, span, cause, loaded_urls) = match *e {
                SassError::Format {
                    message,
                    span,
                    cause,
                    loaded_urls,
                    ..
                }
                | SassError::Sass {
                    message,
                    span,
                    cause,
                    loaded_urls,
                } => (message, span, cause, loaded_urls),
                _ => unreachable!(),
            };
            let mut trace = Trace::new(vec![Frame {
                uri: span.source_url.clone(),
                line: span.start.line + 1,
                column: span.start.column + 1,
                member: state.member.clone(),
            }]);
            trace.frames.extend(stack_trace(state, None).frames);
            let trace = dedup_trace_frames(trace);
            Err(Box::new(SassError::Runtime {
                message,
                span,
                trace,
                cause,
                loaded_urls,
            }))
        }
        Err(e) if matches!(&*e, SassError::MultiSpan { .. }) => {
            let SassError::MultiSpan {
                message,
                span,
                primary_label,
                secondary,
                original_source,
                cause,
                loaded_urls,
                trace,
            } = *e
            else {
                unreachable!()
            };
            // Mirrors Dart's `_addExceptionTrace`: a `MultiSpanSassRuntimeException`
            // (one that already carries a trace) is rethrown unchanged. Only a
            // trace-less `MultiSpanSassException` gets a trace attached here.
            if !trace.is_empty() {
                return Err(Box::new(SassError::MultiSpan {
                    message,
                    span,
                    primary_label,
                    secondary,
                    original_source,
                    cause,
                    loaded_urls,
                    trace,
                }));
            }
            let mut trace = Trace::new(vec![Frame {
                uri: span.source_url.clone(),
                line: span.start.line + 1,
                column: span.start.column + 1,
                member: state.member.clone(),
            }]);
            trace.frames.extend(stack_trace(state, None).frames);
            let trace = dedup_trace_frames(trace);
            Err(Box::new(SassError::MultiSpan {
                message,
                span,
                primary_label,
                secondary,
                original_source,
                cause,
                loaded_urls,
                trace,
            }))
        }
        Err(e) => Err(e),
    }
}

/// Removes frames whose (uri, line, column) equal a later frame's, keeping the
/// outermost. Matches Dart's trace for load-site errors: when the error's span
/// is the `@use`/`@forward` rule itself, the span frame ("@use") and the stack
/// frame ("root stylesheet") coincide at the same location, and only the
/// outermost ("root stylesheet") frame is reported.
fn dedup_trace_frames(trace: Trace) -> Trace {
    let frames = trace.frames;
    let mut result: Vec<Frame> = Vec::with_capacity(frames.len());
    for frame in frames {
        if let Some(last) = result.last() {
            let same = last.uri.as_ref().map(|u| u.as_str())
                == frame.uri.as_ref().map(|u| u.as_str())
                && last.line == frame.line
                && last.column == frame.column;
            if same {
                *result.last_mut().unwrap() = frame;
                continue;
            }
        }
        result.push(frame);
    }
    Trace::new(result)
}

// ===========================================================================
// addErrorSpan — re-spans @error errors at the call site
// Dart: _addErrorSpan (evaluate.dart:4774) — catches a
// SassRuntimeException whose span text starts with "@error" and re-throws it
// with [nodeWithSpan]'s span and the current stack trace. Applied at function
// call and mixin-include sites, so an `@error` deep inside a callable reports
// the call site, not the `@error` rule.
// ===========================================================================

#[rust_sass_macros::async_impl]
pub(crate) async fn add_error_span<'compile, 'parse, F, T>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    node_span: FileSpan<'parse>,
    trace_span: Option<FileSpan<'parse>>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> AsyncFnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<T>,
    'compile: 'parse,
{
    let result = f(config, state).await;
    match result {
        Ok(v) => Ok(v),
        Err(e) if matches!(*e, SassError::Runtime { .. }) => {
            let SassError::Runtime {
                message,
                span,
                trace,
                cause,
                loaded_urls,
            } = *e
            else {
                unreachable!()
            };
            if span.text.starts_with("@error") {
                // Dart `_addErrorSpan`: replace the span with the call site and
                // the trace with the current stack. `trace_span` is `Some` at
                // function-call sites (the callable frame has already been
                // popped, so the current-member frame is synthesized at the call
                // site) and `None` at mixin-include sites (the callable frame is
                // still on the stack).
                let new_trace = stack_trace(state, trace_span);
                Err(Box::new(SassError::Runtime {
                    message,
                    span: SourceSpanWithContext::from_file_span(&node_span).unwrap_or(span),
                    trace: new_trace,
                    cause,
                    loaded_urls,
                }))
            } else {
                Err(Box::new(SassError::Runtime {
                    message,
                    span,
                    trace,
                    cause,
                    loaded_urls,
                }))
            }
        }
        Err(e) => Err(e),
    }
}

#[rust_sass_macros::sync_impl]
#[rust_sass_macros::must_be_sync]
pub(crate) async fn add_error_span<'compile, 'parse, F, T>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    node_span: FileSpan<'parse>,
    trace_span: Option<FileSpan<'parse>>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> FnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<T>,
    'compile: 'parse,
{
    let result = f(config, state).await;
    match result {
        Ok(v) => Ok(v),
        Err(e) if matches!(*e, SassError::Runtime { .. }) => {
            let SassError::Runtime {
                message,
                span,
                trace,
                cause,
                loaded_urls,
            } = *e
            else {
                unreachable!()
            };
            if span.text.starts_with("@error") {
                // Dart `_addErrorSpan`: replace the span with the call site and
                // the trace with the current stack. `trace_span` is `Some` at
                // function-call sites (the callable frame has already been
                // popped, so the current-member frame is synthesized at the call
                // site) and `None` at mixin-include sites (the callable frame is
                // still on the stack).
                let new_trace = stack_trace(state, trace_span);
                Err(Box::new(SassError::Runtime {
                    message,
                    span: SourceSpanWithContext::from_file_span(&node_span).unwrap_or(span),
                    trace: new_trace,
                    cause,
                    loaded_urls,
                }))
            } else {
                Err(Box::new(SassError::Runtime {
                    message,
                    span,
                    trace,
                    cause,
                    loaded_urls,
                }))
            }
        }
        Err(e) => Err(e),
    }
}

// ===========================================================================
// loadedUrlsList — converts Vec<String> to Vec<Url>
// Go: evaluate_helpers.go:1019
// ===========================================================================
// Matches Dart: the `_loadedUrls` field (evaluate.dart:262), a `Set<Uri>` of
// every stylesheet canonical URL seen during compilation (entrypoint included,
// `stdin` excluded under node-sass compat). Rust stores `Vec<String>` on
// state; this parses each back to a URL, silently dropping unparseable ones,
// for error `loaded_urls` payloads.

pub(crate) fn loaded_urls_list(state: &EvalState<'_, '_>) -> Vec<SassUrl> {
    state
        .loaded_urls
        .iter()
        .filter_map(|u| SassUrl::parse(u).ok())
        .collect()
}

// ===========================================================================
// WithStackFrame — save/restore state.stack + member
// Go: evaluate_helpers.go:654
// ===========================================================================
// Matches Dart: `_withStackFrame(member, nodeWithSpan, callback)`
// (evaluate.dart:4621). Pushes the *previous* member name paired with the new
// call-site span, installs `member` as current, runs the callback, then pops.
// Takes the span eagerly here (Dart takes the `AstNode` to defer expensive
// span manufacture). Rust additionally threads a `Callable` for the frame
// (import/load machinery passes `None` → [`Callable::dummy`], a Rust-only
// placeholder with no Dart counterpart; user-defined invocations pass the
// real callable so host seams can classify mixin vs function frames);
// restoration is unconditional on return, like Dart's `try/finally`.

#[rust_sass_macros::async_impl]
pub(crate) async fn with_stack_frame<'compile, 'parse, F, T>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    member: &str,
    span: FileSpan<'parse>,
    callable: Option<Callable<'compile, 'parse>>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> AsyncFnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<T>,
    'compile: 'parse,
{
    let old_stack = state.stack.take();
    let old_member = std::mem::replace(&mut state.member, member.to_string());
    let new_frame = StackFrame {
        name: old_member.clone(),
        span,
        callable: callable.unwrap_or_else(|| Callable::dummy(arena)),
        parent: old_stack,
    };
    state.stack = Some(Box::new(new_frame));
    let result = f(config, state).await;
    if let Some(boxed) = state.stack.take() {
        state.stack = boxed.parent;
    }
    state.member = old_member;
    result
}

#[rust_sass_macros::sync_impl]
#[rust_sass_macros::must_be_sync]
pub(crate) async fn with_stack_frame<'compile, 'parse, F, T>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    member: &str,
    span: FileSpan<'parse>,
    callable: Option<Callable<'compile, 'parse>>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> FnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<T>,
    'compile: 'parse,
{
    let old_stack = state.stack.take();
    let old_member = std::mem::replace(&mut state.member, member.to_string());
    let new_frame = StackFrame {
        name: old_member.clone(),
        span,
        callable: callable.unwrap_or_else(|| Callable::dummy(arena)),
        parent: old_stack,
    };
    state.stack = Some(Box::new(new_frame));
    let result = f(config, state).await;
    if let Some(boxed) = state.stack.take() {
        state.stack = boxed.parent;
    }
    state.member = old_member;
    result
}

// ===========================================================================
// WithEnvironment — save/restore state.env
// Go: evaluate_helpers.go:675
// ===========================================================================
// Matches Dart: `_withEnvironment(environment, callback)` (evaluate.dart:4340).
// Swaps in the callable's closure environment so `@content`/mixin bodies see
// definition-site bindings, then restores the caller's environment. Plain
// save/restore combinator (borrow pattern): the `&mut state` borrow is held
// across the callback, so callers pass `config` separately.

#[rust_sass_macros::async_impl]
pub(crate) async fn with_environment<'compile, 'parse, F, T>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    env: Environment<'compile, 'parse>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> AsyncFnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<T>,
{
    let old_env = std::mem::replace(&mut state.env, env);
    let result = f(config, state).await;
    state.env = old_env;
    result
}

#[rust_sass_macros::sync_impl]
#[rust_sass_macros::must_be_sync]
pub(crate) async fn with_environment<'compile, 'parse, F, T>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    env: Environment<'compile, 'parse>,
    f: F,
) -> SassResult<T>
where
    F: for<'a> FnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<T>,
{
    let old_env = std::mem::replace(&mut state.env, env);
    let result = f(config, state).await;
    state.env = old_env;
    result
}

// ===========================================================================
// Helpers
// ===========================================================================

fn bogus_span_ctx() -> SourceSpanWithContext {
    SourceSpanWithContext::new(
        SourceLocation {
            offset: 0,
            line: 0,
            column: 0,
        },
        SourceLocation {
            offset: 0,
            line: 0,
            column: 0,
        },
        String::new(),
        String::new(),
        None,
    )
    .unwrap()
}

pub(crate) fn file_span_to_ctx(span: &FileSpan<'_>) -> SourceSpanWithContext {
    SourceSpanWithContext::from_file_span(span).unwrap_or_else(|_| bogus_span_ctx())
}

// ===========================================================================
// serialize — wraps value.to_css_string with error handling
// Go: evaluate_helpers.go:41
// ===========================================================================
// Matches Dart: `_serialize(value, nodeWithSpan, {quote})` (evaluate.dart:4473)
// — `value.toCssString(quote:)` with `Script` failures re-spanned at the node.
// (`_evaluateToCss`, evaluate.dart:4461, is the expression-taking twin; here
// callers evaluate first and pass the `Value`.) Takes the `FileSpan` eagerly;
// `add_stack_frame: None` keeps the existing stack as-is.

#[rust_sass_macros::maybe_async]
pub(crate) async fn serialize_value<'compile: 'parse, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    val: &Value<'parse>,
    span: FileSpan<'parse>,
    quote: bool,
) -> SassResult<String> {
    add_exception_span(config, state, span, None, async |_, _| {
        crate::serialize::serialize_value(val, quote)
    })
    .await
}

// ===========================================================================
// verifyParameterList — argument count + name validation
// Go: evaluate_helpers.go:863
// ===========================================================================
// Matches Dart: `_verifyArguments` (evaluate.dart:3963) delegating to
// `ParameterList.verify`. Reports source spellings (`original_name`, e.g.
// `$foo_bar` not `$foo-bar`) for missing/both-position errors, and includes
// `"positional "` in the over-arity message when named arguments are present.
// Emits spanless `Script`/`MultiSpanScript` errors — the call site must wrap
// with [`add_exception_span`] to attach span + trace, mirroring Dart's
// `_addExceptionSpan(nodeWithSpan, () => parameters.verify(...))`. A rest
// parameter accepts any arity; unknown names list `$`-prefixed candidates
// joined with "or".

pub(crate) fn verify_parameter_list<'compile, 'parse>(
    positional_count: usize,
    named: &HashSet<String>,
    params: Option<&ParameterList<'parse>>,
    _span: FileSpan<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let params = match params {
        Some(p) => p,
        None => {
            if positional_count > 0 || !named.is_empty() {
                // SassError::Script (spanless) — add_exception_span at the
                // call site converts this to SassError::Runtime with the
                // call-site span + trace, matching Go/Dart addExceptionSpan.
                return Err(Box::new(SassError::Script {
                    message: format!("Expected 0 arguments, got {}.", positional_count),
                    argument_name: None,
                }));
            }
            return Ok(());
        }
    };

    let params_span = params.span_with_name();
    let mut named_used = 0;

    for (i, param) in params.parameters.iter().enumerate() {
        if i < positional_count {
            if named.contains(&param.name) {
                return Err(Box::new(SassError::Script {
                    message: format!(
                        "Argument {} was passed both by position and by name.",
                        param.original_name()
                    ),
                    argument_name: None,
                }));
            }
        } else if named.contains(&param.name) {
            named_used += 1;
        } else if param.default_value.is_none() {
            // Dart `ParameterList.verify` reports `_originalParameterName`
            // (source spelling) rather than the normalized name.
            return Err(Box::new(SassError::MultiSpanScript {
                message: format!("Missing argument {}.", param.original_name()),
                primary_label: Some("invocation".into()),
                secondary: vec![(file_span_to_ctx(&params_span), "declaration".into())],
                cause: None,
                loaded_urls: vec![],
            }));
        }
    }

    if params.rest_parameter.is_some() {
        return Ok(());
    }

    if positional_count > params.parameters.len() {
        let plural = if params.parameters.len() != 1 {
            "arguments"
        } else {
            "argument"
        };
        let was_plural = if positional_count != 1 { "were" } else { "was" };
        // Dart `ParameterList.verify`: `"positional "` appears when named
        // arguments are present.
        let positional_prefix = if !named.is_empty() { "positional " } else { "" };
        return Err(Box::new(SassError::MultiSpanScript {
            message: format!(
                "Only {} {positional_prefix}{plural} allowed, but {} {was_plural} passed.",
                params.parameters.len(),
                positional_count,
            ),
            primary_label: Some("invocation".into()),
            secondary: vec![(file_span_to_ctx(&params_span), "declaration".into())],
            cause: None,
            loaded_urls: vec![],
        }));
    }

    if named_used < named.len() {
        let unknown_names: Vec<String> = named
            .iter()
            .filter(|n| !params.parameters.iter().any(|p| &p.name == *n))
            .cloned()
            .collect();

        if !unknown_names.is_empty() {
            let parameter_word = if unknown_names.len() == 1 {
                "parameter"
            } else {
                "parameters"
            };
            let dollar_names: Vec<String> = unknown_names.iter().map(|n| format!("${n}")).collect();
            let parameter_names = match dollar_names.len() {
                1 => dollar_names[0].clone(),
                2 => format!("{} or {}", dollar_names[0], dollar_names[1]),
                _ => {
                    let last = dollar_names.last().unwrap();
                    format!(
                        "{}, or {last}",
                        dollar_names[..dollar_names.len() - 1].join(", ")
                    )
                }
            };
            return Err(Box::new(SassError::MultiSpanScript {
                message: format!("No {parameter_word} named {parameter_names}."),
                primary_label: Some("invocation".into()),
                secondary: vec![(file_span_to_ctx(&params_span), "declaration".into())],
                cause: None,
                loaded_urls: vec![],
            }));
        }
    }

    Ok(())
}

// ===========================================================================
// expressionNode — resolves VariableExpression to its declaration node
// Go: evaluate_helpers.go:50
// ===========================================================================
// Matches Dart: `_expressionNode(expression)` (evaluate.dart:4485). A variable
// reference reports the span where the variable was declared (via
// `get_variable_node`), so slash-division warnings point at the declaration;
// anything else reports its own span. Falls back to the expression itself when
// the variable is undefined (the later lookup will raise). Returns a span
// eagerly — Dart returns the node to defer span manufacture, but Rust callers
// here always need the span.

pub(crate) fn expression_node<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    state: &EvalState<'compile, 'parse>,
    expression: &Expression<'parse>,
) -> SassResult<FileSpan<'parse>>
where
    'compile: 'parse,
{
    if let Expression::Variable(ve) = expression {
        if let Some(span) = state
            .env
            .get_variable_node(&ve.name, ve.namespace.as_deref())?
        {
            return Ok(span);
        }
    }
    expression.span()
}

// ===========================================================================
// styleRule — returns current style rule or nil when @at-root excludes
// Dart: _styleRule getter (evaluate.dart:246)
// Go: evaluate_helpers.go:1125
// ===========================================================================
// Matches Dart: `get _styleRule` (evaluate.dart:246) —
// `_atRootExcludingStyleRule ? null : _styleRuleIgnoringAtRoot`. The raw
// `_styleRuleIgnoringAtRoot` field deliberately ignores intermediate
// `@at-root` rules; this accessor is the one callers use in the common case
// where exclusion matters (selector resolution, `@extend`, declaration
// checks).

pub(crate) fn style_rule<'s, 'parse>(
    state: &'s EvalState<'_, 'parse>,
) -> Option<&'s CssStyleRule<'parse>> {
    if state.at_root_excluding_style_rule {
        None
    } else {
        state.style_rule_ignoring_at_root.as_ref()
    }
}

// ===========================================================================
// mergeMediaQueries — cross-joins two query sets
// Go: evaluate_helpers.go:1151
// Dart: _mergeMediaQueries returns List<CssMediaQuery>?
//   null = unrepresentable, [] = all merged away, [...] = merged
// ===========================================================================
// Matches Dart: `_mergeMediaQueries(queries1, queries2)` (evaluate.dart:2347).
// Tries every pair; an `Empty` pair is skipped, a single `Unrepresentable`
// pair poisons the whole result to `None` (the caller then emits the queries
// unmerged). An empty input yields `Some(vec![])` — everything merged away —
// which Dart's nullable return distinguishes from `None`.

pub(crate) fn merge_media_queries(
    queries1: &[CssMediaQuery],
    queries2: &[CssMediaQuery],
) -> Option<Vec<CssMediaQuery>> {
    if queries1.is_empty() || queries2.is_empty() {
        return Some(vec![]);
    }
    let mut result = Vec::new();
    for q1 in queries1 {
        for q2 in queries2 {
            match q1.merge(q2) {
                MediaQueryMergeResult::Successful(s) => {
                    result.push(s.query.clone());
                }
                MediaQueryMergeResult::Empty => {}
                MediaQueryMergeResult::Unrepresentable => {
                    return None;
                }
            }
        }
    }
    Some(result)
}

// ===========================================================================
// withMediaQueries — save/restore state.media_queries + state.media_query_sources
// Dart: _withMediaQueries(List<CssMediaQuery>? queries, Set<CssMediaQuery>? sources, callback)
// Go: evaluate_helpers.go:1175
// ===========================================================================
// Matches Dart: `_withMediaQueries(queries, sources, callback)`
// (evaluate.dart:4598). `sources` is the set of queries merged to produce
// `queries` (empty when not a merge result) — used to decide when one query
// may bubble through another. Both fields restore together; passing
// `(None, None)` clears the context (media exclusion in `@at-root`).

#[rust_sass_macros::async_impl]
pub(crate) async fn with_media_queries<'compile, 'parse, F>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    queries: Option<Vec<CssMediaQuery>>,
    sources: Option<Vec<CssMediaQuery>>,
    f: F,
) -> SassResult<()>
where
    F: for<'a> AsyncFnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<()>,
{
    let old_queries = state.media_queries.take();
    let old_sources = state.media_query_sources.take();
    state.media_queries = queries;
    state.media_query_sources = sources;
    let result = f(config, state).await;
    state.media_queries = old_queries;
    state.media_query_sources = old_sources;
    result
}

#[rust_sass_macros::sync_impl]
#[rust_sass_macros::must_be_sync]
pub(crate) async fn with_media_queries<'compile, 'parse, F>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    queries: Option<Vec<CssMediaQuery>>,
    sources: Option<Vec<CssMediaQuery>>,
    f: F,
) -> SassResult<()>
where
    F: for<'a> FnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<()>,
{
    let old_queries = state.media_queries.take();
    let old_sources = state.media_query_sources.take();
    state.media_queries = queries;
    state.media_query_sources = sources;
    let result = f(config, state).await;
    state.media_queries = old_queries;
    state.media_query_sources = old_sources;
    result
}

// ===========================================================================
// addChild — Dart-unified _addChild with optional through parameter
// Dart: _addChild(ModifiableCssNode node, {bool through(CssNode node)?})
// ===========================================================================
// Matches Dart: `_addChild(node, {through})` (evaluate.dart:4547). Without
// `through`, appends to the current parent. With it, walks up past parents
// matching `through`, then — if that parent has a following sibling — reuses
// an already-made childless copy or clones via `copy_without_children` so the
// sibling is not corrupted (the bubbling path). Dart throws `ArgumentError`
// when `through` never returns false; here that is a panic, same contract.

pub(crate) fn add_child<'compile, 'parse>(
    arena: &'compile Bump,
    state: &mut EvalState<'compile, 'parse>,
    node: &ModifiableCssNode<'parse>,
    through: Option<&dyn Fn(&ModifiableCssNode<'parse>) -> bool>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let parent = match state.parent.clone() {
        Some(p) => p,
        None => return Ok(()),
    };

    let mut target_parent = parent;

    if let Some(through_fn) = through {
        while through_fn(&target_parent) {
            let grandparent = match target_parent.parent() {
                Some(gp) => gp,
                None => panic!(
                    "through() must return false for at least one parent of {target_parent:?}."
                ),
            };
            target_parent = grandparent;
        }

        if target_parent.has_following_sibling() {
            let grandparent = target_parent.parent().unwrap();
            if let Some(last) = grandparent.last_child() {
                if last.equals_ignoring_children(&target_parent) {
                    target_parent = last;
                } else {
                    let new_parent = target_parent.copy_without_children(arena);
                    grandparent.add_child(&new_parent)?;
                    target_parent = new_parent;
                }
            }
        }
    }

    target_parent.add_child(node)?;
    register_tabs_pending(state, node);
    Ok(())
}

// ===========================================================================
// libsass NESTED tabs collection — registration + frame stamping
// No Dart counterpart (libsass `Cssize` tabs accumulation in
// `libsass/src/cssize.cpp`, rendered in `output.cpp`/`inspect.cpp`).
// ===========================================================================
// Matches libsass: nested style rules and bubbles inside a props-bearing
// rule block gain `+1` (`cssize.cpp:186-190`); bubble unwraps and at-root
// splices accumulate outward (`cssize.cpp:276,475`); media/supports/
// keyframes boundaries block propagation. Style rules and `@at-root` are
// transparent; only style-rule frames stamp, and only when the frame's own
// node has direct non-bubblable children (declarations, comments, other
// at-rules — libsass's `props` partition).

/// Registers `node` for NESTED `tabs` stamping when it is a style, media,
/// or supports rule. Called from [`add_child`] (attach-time scope), so
/// bubble copies created inside opaque scopes land in the inner generation
/// and are dropped un-stamped, while nodes hoisted outward collect.
fn register_tabs_pending<'compile, 'parse>(
    state: &mut EvalState<'compile, 'parse>,
    node: &ModifiableCssNode<'parse>,
) {
    let register = matches!(
        &*node.kind(),
        ModifiableCssNodeKind::StyleRule(_)
            | ModifiableCssNodeKind::MediaRule(_)
            | ModifiableCssNodeKind::SupportsRule(_)
    );
    if register {
        let gen = state.tabs_gen;
        state.tabs_pending.push((node.clone(), gen));
    }
}

/// Pushes a style-rule stamping frame for `own` (its `rule_node`).
pub(crate) fn tabs_frame_push<'compile, 'parse>(
    state: &mut EvalState<'compile, 'parse>,
    own: &ModifiableCssNode<'parse>,
) {
    let start = state.tabs_pending.len();
    state.tabs_frames.push((own.clone(), start));
}

/// Pops the innermost style-rule frame; when its node has direct
/// non-bubblable children, stamps `+1` on every pending node collected
/// during its scope in the current generation, except the frame's own node
/// and its subtree (content emitted in place needs no extra indent).
pub(crate) fn tabs_frame_pop_and_stamp<'compile, 'parse>(state: &mut EvalState<'compile, 'parse>) {
    let Some((own, start)) = state.tabs_frames.pop() else {
        return;
    };
    let has_props = own.kind().children_ref().is_some_and(|children| {
        children.iter().any(|c| {
            matches!(
                &*c.kind(),
                ModifiableCssNodeKind::Declaration(_)
                    | ModifiableCssNodeKind::Comment(_)
                    | ModifiableCssNodeKind::AtRule(_)
                    | ModifiableCssNodeKind::Import(_)
            )
        })
    });
    if !has_props {
        return;
    }
    let gen = state.tabs_gen;
    for (node, node_gen) in state.tabs_pending[start..].iter() {
        if *node_gen != gen || *node == own {
            continue;
        }
        // Skip content emitted in place (nested in the frame's own subtree).
        let mut current = node.parent();
        let mut in_subtree = false;
        while let Some(p) = current {
            if p == own {
                in_subtree = true;
                break;
            }
            current = p.parent();
        }
        if !in_subtree {
            node.add_tabs(1);
        }
    }
}

/// Enters an opaque (media/supports) scope: bumps the generation and
/// returns the pending start index. Nodes created inside belong to the
/// inner generation; see [`tabs_opaque_pop`].
pub(crate) fn tabs_opaque_push<'compile, 'parse>(state: &mut EvalState<'compile, 'parse>) -> usize {
    let start = state.tabs_pending.len();
    state.tabs_gen += 1;
    start
}

/// Leaves the opaque scope entered by [`tabs_opaque_push`]: drops inner
/// entries still nested under `opaque` (blocked propagation), rebases
/// entries that escaped outward (e.g. `@at-root` hoists past the boundary)
/// to the outer generation, and restores it.
///
/// Leak caveat: early `?` returns inside the scope skip the pop, leaving
/// the generation bumped — harmless because every `SassError` unwinds to
/// the compile boundary and discards the state.
pub(crate) fn tabs_opaque_pop<'compile, 'parse>(
    state: &mut EvalState<'compile, 'parse>,
    opaque: &ModifiableCssNode<'parse>,
    start: usize,
) {
    let outer_gen = state.tabs_gen.saturating_sub(1);
    let mut kept_end = start;
    for i in start..state.tabs_pending.len() {
        let (ref node, gen) = state.tabs_pending[i];
        if gen != state.tabs_gen {
            state.tabs_pending.swap(kept_end, i);
            kept_end += 1;
            continue;
        }
        // Still nested under the opaque node: blocked, drop.
        let mut current = node.parent();
        let mut inside = false;
        while let Some(p) = current {
            if p == *opaque {
                inside = true;
                break;
            }
            current = p.parent();
        }
        if inside {
            continue;
        }
        // Escaped outward (hoisted past the boundary): rebase so outer
        // frames can still reach it.
        state.tabs_pending[i].1 = outer_gen;
        state.tabs_pending.swap(kept_end, i);
        kept_end += 1;
    }
    state.tabs_pending.truncate(kept_end);
    state.tabs_gen = outer_gen;
}

// ===========================================================================
// hasCssNesting — returns true if current style rule has a parent that is also
// a style rule. Dart: _hasCssNesting
// ===========================================================================
// Matches Dart: `get _hasCssNesting` (evaluate.dart:353). Walks from
// [`style_rule`] — never the raw ignoring-at-root field — so it returns false
// while `@at-root` excludes style rules (Dart's `_styleRule` getter is null in
// that state). True means plain-CSS nesting is in effect and other nesting
// features are safe to use.

pub(crate) fn has_css_nesting(state: &EvalState<'_, '_>) -> bool {
    // Dart `_hasCssNesting` walks from `_styleRule`, which is null while
    // `_atRootExcludingStyleRule` is set — never directly from the style-rule
    // node.
    if state.at_root_excluding_style_rule {
        return false;
    }
    let sr_node = match state.css_style_rule_node.as_ref() {
        Some(n) => n,
        None => return false,
    };
    let mut current = sr_node.parent();
    while let Some(node) = current {
        if node.is_style_rule() {
            return true;
        }
        current = node.parent();
    }
    false
}

// ===========================================================================
// withCssStyleRule — save/restore styleRuleIgnoringAtRoot + css_style_rule_node
// Dart: _withStyleRule(ModifiableCssStyleRule rule, callback)
// ===========================================================================
// Matches Dart: `_withStyleRule(rule, callback)` (evaluate.dart:4582).
// Installs the rule defining the current parent selector for the callback's
// duration. Rust additionally threads the modifiable-tree node
// (`css_style_rule_node`) alongside the frozen `CssStyleRule` — a split forced
// by the wrapper/indirection CSS-tree design, with no Dart counterpart.

#[rust_sass_macros::async_impl]
pub(crate) async fn with_css_style_rule<'compile, 'parse, F>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    rule: &CssStyleRule<'parse>,
    rule_node: Option<ModifiableCssNode<'parse>>,
    callback: F,
) -> SassResult<()>
where
    F: for<'a> AsyncFnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<()>,
{
    let old_rule = state.style_rule_ignoring_at_root.clone();
    let old_node = state.css_style_rule_node.take();
    state.style_rule_ignoring_at_root = Some(rule.clone());
    state.css_style_rule_node = rule_node;
    let result = callback(config, state).await;
    state.style_rule_ignoring_at_root = old_rule;
    state.css_style_rule_node = old_node;
    result
}

#[rust_sass_macros::sync_impl]
#[rust_sass_macros::must_be_sync]
pub(crate) async fn with_css_style_rule<'compile, 'parse, F>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    rule: &CssStyleRule<'parse>,
    rule_node: Option<ModifiableCssNode<'parse>>,
    callback: F,
) -> SassResult<()>
where
    F: for<'a> FnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<()>,
{
    let old_rule = state.style_rule_ignoring_at_root.clone();
    let old_node = state.css_style_rule_node.take();
    state.style_rule_ignoring_at_root = Some(rule.clone());
    state.css_style_rule_node = rule_node;
    let result = callback(config, state).await;
    state.style_rule_ignoring_at_root = old_rule;
    state.css_style_rule_node = old_node;
    result
}

// ===========================================================================
// unusedKeywordsError — error for keywords not accepted by rest parameter
// Go: evaluate_helpers.go:1297
// ===========================================================================
// Matches Dart: the trailing check in `_runUserDefinedCallable`
// (evaluate.dart:3577) — after the body runs, leftover named args with an
// un-accessed rest argument (`wereKeywordsAccessed == false`) raise
// `No $parameter(s) named $…` with "invocation"/"declaration" labels. Eagerly
// builds the spanned `MultiSpan` error (Dart throws at the throw site); the
// trace includes the invocation span frame.

pub(crate) fn unused_keywords_error<'compile: 'parse, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    kwargs: &IndexMap<String, ()>,
    span: FileSpan<'parse>,
    params: &ParameterList<'parse>,
) -> SassError {
    let names: Vec<String> = kwargs.keys().map(|k| format!("${k}")).collect();
    let parameter_word = if names.len() == 1 {
        "parameter"
    } else {
        "parameters"
    };
    let parameter_names = match names.len() {
        1 => names[0].clone(),
        2 => format!("{} or {}", names[0], names[1]),
        _ => {
            let last = names.last().unwrap();
            format!("{}, or {last}", names[..names.len() - 1].join(", "))
        }
    };
    let params_span = params.span().unwrap_or(span);
    SassError::MultiSpan {
        message: format!("No {parameter_word} named {parameter_names}."),
        span: file_span_to_ctx(&span),
        primary_label: Some("invocation".into()),
        secondary: vec![(file_span_to_ctx(&params_span), "declaration".into())],
        original_source: None,
        cause: None,
        loaded_urls: vec![],
        trace: stack_trace(state, Some(span)),
    }
}

// ===========================================================================
// evaluateArguments — evaluates positional, named, rest, keywordRest
// Go: evaluate_helpers.go:243
// ===========================================================================
// Matches Dart: `_evaluateArguments(arguments)` (evaluate.dart:3763). Each
// positional/named value is evaluated, then passed through [`without_slash`]
// at its declaration-site span ([`expression_node`]) — unconditional tracking
// that Dart's TODO (evaluate.dart:3764) notes is kept for slash-division
// warnings. Rest-argument dispatch: `Map` → named via [`add_rest_map`],
// `ArgumentList` → positional + keywords (separator preserved), `List` →
// positional (separator preserved), anything else → a single positional value.
// A non-map keyword-rest raises at the use-site span (the raw keyword-rest
// argument span, not the resolved value node).

#[allow(dead_code)]
#[derive(Debug)]
/// Evaluated call arguments: positional/named values plus their declaration-site
// Matches Dart: the `_ArgumentResults` record returned by `_evaluateArguments`
// (evaluate.dart:3763). `positional_nodes`/`named_nodes` are the
// `_expressionNode`-resolved spans used for slash-division warnings; `separator`
// carries the rest list's separator (`Undecided` when no rest arg) for rest-arg
// `SassArgumentList` assembly.
pub(crate) struct ArgumentResults<'parse> {
    pub positional: Vec<Value<'parse>>,
    pub positional_nodes: Vec<FileSpan<'parse>>,
    pub named: IndexMap<String, Value<'parse>>,
    pub named_nodes: IndexMap<String, FileSpan<'parse>>,
    pub separator: ListSeparator,
}

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_arguments<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    args: &ArgumentList<'parse>,
) -> SassResult<ArgumentResults<'parse>>
where
    'compile: 'parse,
{
    let mut positional: Vec<Value<'parse>> = Vec::with_capacity(args.positional.len());
    let mut positional_nodes: Vec<FileSpan<'parse>> = Vec::with_capacity(args.positional.len());
    for expr in &args.positional {
        let val = evaluate_expression(config, state, arena, expr).await?;
        let span = expression_node(config, state, expr)?;
        let cleaned = without_slash(config, state, arena, val, span)?;
        positional.push(cleaned);
        positional_nodes.push(span);
    }

    let mut named: IndexMap<String, Value<'parse>> = IndexMap::new();
    let mut named_nodes: IndexMap<String, FileSpan<'parse>> = IndexMap::new();
    for (name, expr) in &args.named {
        let span = expression_node(config, state, expr)?;
        let val = evaluate_expression(config, state, arena, expr).await?;
        let cleaned = without_slash(config, state, arena, val, span)?;
        named.insert(name.clone(), cleaned);
        named_nodes.insert(name.clone(), span);
    }

    let mut separator = ListSeparator::Undecided;

    if let Some(ref rest) = args.rest {
        let rest_val = evaluate_expression(config, state, arena, rest).await?;
        let rest_span = expression_node(config, state, rest)?;
        match &*rest_val {
            ValueKind::Map(m) => {
                add_rest_map(config, state, arena, &mut named, m, rest_span, rest.span()?)?;
                for (key, _val) in &m.entries {
                    if let ValueKind::String(s) = &**key {
                        named_nodes.insert(s.text.to_string(), rest_span);
                    }
                }
            }
            ValueKind::ArgumentList(arg_list) => {
                for item in arg_list.as_list() {
                    let cleaned = without_slash(config, state, arena, *item, rest_span)?;
                    positional.push(cleaned);
                    positional_nodes.push(rest_span);
                }
                separator = arg_list.list.separator;
                for (key, val) in arg_list.keywords().iter() {
                    let cleaned = without_slash(config, state, arena, *val, rest_span)?;
                    named.insert(key.clone(), cleaned);
                    named_nodes.insert(key.clone(), rest_span);
                }
            }
            ValueKind::List(l) => {
                for item in l.as_list() {
                    let cleaned = without_slash(config, state, arena, *item, rest_span)?;
                    positional.push(cleaned);
                    positional_nodes.push(rest_span);
                }
                separator = l.separator;
            }
            _ => {
                let cleaned = without_slash(config, state, arena, rest_val, rest_span)?;
                positional.push(cleaned);
                positional_nodes.push(rest_span);
            }
        }
    }

    if let Some(ref kw_rest) = args.keyword_rest {
        let kw_val = evaluate_expression(config, state, arena, kw_rest).await?;
        let kw_span = expression_node(config, state, kw_rest)?;
        match &*kw_val {
            ValueKind::Map(m) => {
                add_rest_map(
                    config,
                    state,
                    arena,
                    &mut named,
                    m,
                    kw_span,
                    kw_rest.span()?,
                )?;
                for (key, _val) in &m.entries {
                    if let ValueKind::String(s) = &**key {
                        named_nodes.insert(s.text.to_string(), kw_span);
                    }
                }
            }
            // Dart `_evaluateArguments`: the error span is the use-site
            // `keywordRestArgs.span`, not the resolved value node.
            _ => {
                return Err(Box::new(exception(
                    state,
                    format!(
                        "Variable keyword arguments must be a map (was {}).",
                        kw_val.to_display_string()?
                    ),
                    Some(kw_rest.span()?),
                )));
            }
        }
    }

    Ok(ArgumentResults {
        positional,
        positional_nodes,
        named,
        named_nodes,
        separator,
    })
}

// ===========================================================================
// bindArguments — sets local variables for function/mixin parameters
// Go: evaluate_helpers.go:1043
// ===========================================================================
// Matches Dart: the binding half of `_runUserDefinedCallable`
// (evaluate.dart:3525: positional fill, default-value fallback, rest-arg
// `SassArgumentList` assembly). Positional values bind in order; gaps fall back
// to named, then to evaluated defaults (slash-stripped at the default's
// declaration span), then to a `Missing argument $…` error. Leftover
// positionals plus unconsumed named args pack into a comma-separated
// `SassArgumentList` under the rest parameter (returned as `Some` for the
// caller's unused-keyword check); without a rest parameter returns `None`.

// Arity mirrors Dart's `_runUserDefinedCallable` binding half; packing into
// a struct would diverge from the port.
#[allow(clippy::too_many_arguments)]
#[rust_sass_macros::maybe_async]
pub(crate) async fn bind_arguments<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    params: &ParameterList<'parse>,
    positional: &[Value<'parse>],
    positional_nodes: &[FileSpan<'parse>],
    named: &IndexMap<String, Value<'parse>>,
    named_nodes: &IndexMap<String, FileSpan<'parse>>,
    separator: ListSeparator,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let _ = config;
    let parameters = &params.parameters;

    let min_len = positional.len().min(parameters.len());
    for i in 0..min_len {
        let val = positional[i];
        let span = positional_nodes
            .get(i)
            .copied()
            .unwrap_or(parameters[i].span);
        state.env.set_local_variable(&parameters[i].name, val, span);
    }

    for param in parameters.iter().skip(positional.len()) {
        if let Some(val) = named.get(&param.name) {
            let span = named_nodes.get(&param.name).copied().unwrap_or(param.span);
            state.env.set_local_variable(&param.name, *val, span);
        } else if let Some(ref default_val) = param.default_value {
            let val = evaluate_expression(config, state, arena, default_val).await?;
            let span = expression_node(config, state, default_val)?;
            let cleaned = without_slash(config, state, arena, val, span)?;
            state.env.set_local_variable(&param.name, cleaned, span);
        } else {
            let params_span = params.span()?;
            return Err(Box::new(SassError::MultiSpan {
                message: format!("Missing argument ${}.", param.name),
                span: file_span_to_ctx(&params_span),
                primary_label: Some("invocation".into()),
                secondary: vec![(file_span_to_ctx(&params_span), "declaration".into())],
                original_source: None,
                cause: None,
                loaded_urls: vec![],
                trace: stack_trace(state, Some(params_span)),
            }));
        }
    }

    if let Some(ref rest_name) = params.rest_parameter {
        let rest_values: Vec<Value<'parse>> = if positional.len() > parameters.len() {
            positional[parameters.len()..].to_vec()
        } else {
            vec![]
        };
        let unused_named: IndexMap<String, Value<'parse>> = named
            .iter()
            .filter(|(k, _)| !parameters.iter().any(|p| &p.name == *k))
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let sep = if separator == ListSeparator::Undecided {
            ListSeparator::Comma
        } else {
            separator
        };
        let arg_list = SassArgumentList::new(arena, rest_values, unused_named, sep);
        let rc_val = Value::new_with_arena(arena, ValueKind::ArgumentList(arg_list));
        state
            .env
            .set_local_variable(rest_name, rc_val, params.span()?);
        return Ok(Some(rc_val));
    }

    Ok(None)
}

// ===========================================================================
// runUserDefinedCallable — evaluates args, binds, runs body
// Go: evaluate_helpers.go:1218
// ===========================================================================
// Matches Dart: `_runUserDefinedCallable(arguments, callable, nodeWithSpan,
// run)` (evaluate.dart:3498). Full nesting chain (five closure levels in
// Dart): evaluate args → stack frame (`name()`; `@content` keeps its bare
// name) → closure environment → scope → bind → body. `in_dependency` swaps to
// the callable's for the duration. Parameter verification runs inside the
// stack frame so the trace includes it. Only mixin invocations install
// content/`as_mixin` and wrap the body in [`add_error_span`]; functions return
// the block value or raise `Function finished without @return.` — a spanned
// `Runtime` at the declaration span (via [`exception`], not `Script`) so the
// span and trace survive outer wrappers. Unused rest keywords are checked
// after the body per [`unused_keywords_error`]. (`_runFunctionCallable`'s
// built-in/plain-CSS arms live in the expression evaluator, not here.)

// Arity mirrors Dart's `_runUserDefinedCallable`; packing into a struct
// would diverge from the port.
#[allow(clippy::too_many_arguments)]
#[rust_sass_macros::maybe_async]
pub(crate) async fn run_user_defined_callable<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    callable: &UserDefinedCallable<'compile, 'parse>,
    args: &ArgumentList<'parse>,
    content: Option<Callable<'compile, 'parse>>,
    is_mixin_invocation: bool,
    span: FileSpan<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let results = evaluate_arguments(config, state, arena, args).await?;

    let named_keys: HashSet<String> = results.named.keys().cloned().collect();

    let old_in_dependency = state.in_dependency;
    state.in_dependency = callable.in_dependency;

    let name = callable.name().to_string();
    let display_name = if name != "@content" {
        format!("{name}()")
    } else {
        name.clone()
    };

    let is_mixin = matches!(
        callable.declaration,
        CallableDeclaration::Mixin(_) | CallableDeclaration::ContentBlock(_)
    );
    let is_mixin_that_sets_in_mixin = matches!(callable.declaration, CallableDeclaration::Mixin(_));

    let result = with_stack_frame(
        config,
        state,
        arena,
        &display_name,
        span,
        // The real callable (not `dummy`): host seams (e.g. the libsass C
        // ABI) classify frames as mixin vs function from the declaration,
        // and skip `dummy` frames (import/load machinery) outright.
        // Production trace rendering only reads `name`/`span`, so this is
        // semantics-preserving (see `stack_trace`).
        Some(Callable::new(
            arena,
            CallableKind::UserDefined(callable.clone()),
        )),
        async |config, state| {
            let callable_env = callable.environment.closure(arena);
            with_environment(config, state, callable_env, async |config, state| {
                // Dart `_verifyArguments` runs inside `_withStackFrame`, so the
                // stack trace includes the `a()` frame.
                add_exception_span(config, state, span, Some(true), async |_, _| {
                    verify_parameter_list(
                        results.positional.len(),
                        &named_keys,
                        Some(callable.parameters()),
                        span,
                    )
                })
                .await?;
                let env = state.env;
                env.scope(
                    arena,
                    async || -> SassResult<Value<'_>> {
                        let bound_arg_list = bind_arguments(
                            config,
                            state,
                            arena,
                            callable.parameters(),
                            &results.positional,
                            &results.positional_nodes,
                            &results.named,
                            &results.named_nodes,
                            results.separator,
                        )
                        .await?;

                        // Save/restore content and in_mixin around body evaluation
                        // ONLY for mixin invocations (apply_mixin). For @content rules
                        // and function calls, the closure env already has the correct
                        // content and in_mixin state from when the callable was
                        // captured. Clearing content here (e.g. when content=None for
                        // a forwarded @content($arg) call) breaks content chains.
                        // Matches Dart: applyMixin wraps the body in
                        // withContent(content, () => asMixin(() => ...)), and each
                        // body statement in _addErrorSpan (async_evaluate.dart:2165).
                        let children = callable.declaration.children();
                        let body_result = if is_mixin_invocation {
                            env.with_content(content, async || {
                                add_error_span(config, state, span, None, async |config, state| {
                                    if is_mixin_that_sets_in_mixin {
                                        env.as_mixin(async || {
                                            evaluate_block(config, state, arena, children).await
                                        })
                                        .await
                                    } else {
                                        evaluate_block(config, state, arena, children).await
                                    }
                                })
                                .await
                            })
                            .await
                        } else {
                            evaluate_block(config, state, arena, children).await
                        };

                        let body_result = body_result?;

                        if let Some(val) = body_result {
                            return Ok(val);
                        }

                        // Dart `_runFunctionCallable`: `_exception("Function
                        // finished without @return.", callable.declaration.span)`
                        // — a spanned `Runtime` (not `Script`) so the
                        // declaration span and trace survive outer wrappers.
                        if !is_mixin {
                            return Err(Box::new(exception(
                                state,
                                "Function finished without @return.".to_string(),
                                Some(callable.declaration.span()),
                            )));
                        }

                        // Dart: check unused keyword arguments after body runs
                        // (evaluate.dart:3577-3595)
                        if let Some(ref rc_val) = bound_arg_list {
                            if !results.named.is_empty() {
                                if let ValueKind::ArgumentList(ref arg_list) = **rc_val {
                                    if !arg_list.were_keywords_accessed.get() {
                                        let unused: IndexMap<String, ()> =
                                            results.named.keys().map(|k| (k.clone(), ())).collect();
                                        return Err(Box::new(unused_keywords_error(
                                            config,
                                            state,
                                            &unused,
                                            span,
                                            callable.parameters(),
                                        )));
                                    }
                                }
                            }
                        }

                        Ok(Value::new_with_arena(arena, ValueKind::Null))
                    },
                    false,
                    true,
                )
                .await
            })
            .await
        },
    )
    .await;

    state.in_dependency = old_in_dependency;
    result
}

// ===========================================================================
// withParent — addChild + env scope + callback
// Dart: _withParent(S node, callback, {bool through(CssNode)?, bool scopeWhen = true})
// Go: evaluate_helpers.go:597
// ===========================================================================
// Matches Dart: `_withParent(node, callback, {through, scopeWhen})`
// (evaluate.dart:4513). Attaches the node via [`add_child`] (bubbling through
// matching parents), installs it as current parent, and runs the callback in
// a new environment scope unless `scope_when` is false. Save/restore is
// unconditional — the old parent returns even when the callback fails.

#[rust_sass_macros::async_impl]
pub(crate) async fn with_parent<'compile, 'parse, F>(
    arena: &'compile Bump,
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    node: ModifiableCssNode<'parse>,
    through: Option<&dyn Fn(&ModifiableCssNode<'parse>) -> bool>,
    scope_when: Option<bool>,
    callback: F,
) -> SassResult<()>
where
    'compile: 'parse,
    F: for<'a> AsyncFnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<()>,
{
    add_child(arena, state, &node, through)?;
    let old_parent = state.parent.replace(node);
    let scope = scope_when.unwrap_or(true);
    let result = if scope {
        let env = state.env;
        env.scope(arena, async || callback(config, state).await, false, true)
            .await
    } else {
        callback(config, state).await
    };
    state.parent = old_parent;
    result
}

#[rust_sass_macros::sync_impl]
#[rust_sass_macros::must_be_sync]
pub(crate) async fn with_parent<'compile, 'parse, F>(
    arena: &'compile Bump,
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    node: ModifiableCssNode<'parse>,
    through: Option<&dyn Fn(&ModifiableCssNode<'parse>) -> bool>,
    scope_when: Option<bool>,
    callback: F,
) -> SassResult<()>
where
    'compile: 'parse,
    F: for<'a> FnOnce(
        &'a EvalConfig<'compile, 'parse>,
        &'a mut EvalState<'compile, 'parse>,
    ) -> SassResult<()>,
{
    add_child(arena, state, &node, through)?;
    let old_parent = state.parent.replace(node);
    let scope = scope_when.unwrap_or(true);
    let result = if scope {
        let env = state.env;
        env.scope(arena, async || callback(config, state).await, false, true)
            .await
    } else {
        callback(config, state).await
    };
    state.parent = old_parent;
    result
}

// ===========================================================================
// performInterpolation — evaluates interpolation contents to a CSS string
// Go: evaluate_helpers.go:122
// ===========================================================================
// Matches Dart: `_performInterpolation(interpolation, {warnForColor})`
// (evaluate.dart:4375) — the string-only twin of
// [`perform_interpolation_with_map`]; both delegate to the shared helper
// below with source-map tracking off. `warn_for_color` triggers the named-color
// interpolation warning (with the `"" + <source>` alternative suggestion).

#[rust_sass_macros::maybe_async]
// Shared core of `perform_interpolation` and `perform_interpolation_with_map`.
// Matches Dart: `_performInterpolationHelper(interpolation, {required
// sourceMap, warnForColor})` (evaluate.dart:4404). Clears
// `in_supports_declaration` (calculations must not simplify inside
// interpolation), serializes each expression part unquoted, and — when
// `source_map` — records per-part target offsets for the `InterpolationMap`.
// The named-color warning fires per expression part before serialization.
async fn perform_interpolation_helper<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    interpolation: &Interpolation<'parse>,
    source_map: bool,
    warn_for_color: bool,
) -> SassResult<(String, Option<&'parse InterpolationMap<'parse>>)>
where
    'compile: 'parse,
{
    let old_in_supports = state.in_supports_declaration;
    state.in_supports_declaration = false;

    let mut result = String::new();
    let mut target_offsets: Vec<usize> = Vec::new();
    let mut first = true;

    for part in &interpolation.contents {
        if !first && source_map {
            target_offsets.push(result.len());
        }
        first = false;

        match part {
            InterpolationPart::Text(text) => {
                result.push_str(text);
            }
            InterpolationPart::Expression(expr) => {
                let val = evaluate_expression(config, state, arena, expr).await?;

                if warn_for_color {
                    if let ValueKind::Color(ref color) = &*val {
                        let name = color_name_for(color)?;
                        if !name.is_empty() {
                            let val_str =
                                serialize_value(config, state, &val, expr.span()?, false).await?;
                            let source = expr.span()?.text().to_string();
                            let alternative = format!("\"\" + {source}");
                            let msg = format!(
                                "You probably don't mean to use the color value \
                                 {name} in interpolation here.\n\
                                 It may end up represented as {val_str}, which will likely produce \
                                 invalid CSS.\n\
                                 Always quote color names when using them as strings or map keys \
                                 (for example, \"{name}\").\n\
                                 If you really want to use the color value here, use '{alternative}'."
                            );
                            warn(config, state, &msg, expr.span()?)?;
                        }
                    }
                }

                let css = serialize_value(config, state, &val, expr.span()?, false).await?;
                result.push_str(&css);
            }
        }
    }

    state.in_supports_declaration = old_in_supports;

    let map = if source_map {
        let map = InterpolationMap::new(interpolation.clone(), target_offsets).map_err(|e| {
            SassError::Script {
                message: e.message,
                argument_name: None,
            }
        })?;
        Some(&*arena.alloc(map))
    } else {
        None
    };

    Ok((result, map))
}

#[rust_sass_macros::maybe_async]
pub(crate) async fn perform_interpolation<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    interpolation: &Interpolation<'parse>,
    warn_for_color: bool,
) -> SassResult<String>
where
    'compile: 'parse,
{
    let (result, _) =
        perform_interpolation_helper(config, state, arena, interpolation, false, warn_for_color)
            .await?;
    Ok(result)
}

// ===========================================================================
// performInterpolationWithMap — interpolation with source map tracking
// Go: evaluate_helpers.go:131
// Dart: evaluate.dart:4387 (_performInterpolationWithMap)
// ===========================================================================
// Matches Dart: `_performInterpolationWithMap` (evaluate.dart:4390). Same as
// [`perform_interpolation`] but records per-part target offsets and returns the
// `InterpolationMap` mapping output spans back to the interpolation source
// (`@media`/`@at-root`/selector callers thread it into parsers so errors
// locate the interpolation site). Always returns a map — the helper's `None`
// case is unwrapped here.

#[rust_sass_macros::maybe_async]
pub(crate) async fn perform_interpolation_with_map<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    interpolation: &Interpolation<'parse>,
    warn_for_color: bool,
) -> SassResult<(String, &'parse InterpolationMap<'parse>)>
where
    'compile: 'parse,
{
    let (result, map) =
        perform_interpolation_helper(config, state, arena, interpolation, true, warn_for_color)
            .await?;
    Ok((result, map.unwrap()))
}

// ===========================================================================
// addRestMap — adds SassMap entries to an ordered map
// Go: evaluate_helpers.go:68
// ===========================================================================
// Matches Dart: `_addRestMap(values, map, nodeWithSpan, convert)`
// (evaluate.dart:3941). String keys insert (slash-stripped at the resolved
// declaration span); non-string keys raise `Variable keyword argument map must
// have string keys.` at `error_span` — the raw rest-argument span
// (`nodeWithSpan.span`), NOT the resolved span used for deprecation warnings.
// Dart's generic `convert` closure is monomorphized here: the value-evaluating
// variant is [`evaluate_macro_arguments`] (which wraps in `ValueExpression`),
// this one inserts values as-is.

pub(crate) fn add_rest_map<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    values: &mut IndexMap<String, Value<'parse>>,
    map: &SassMap<'parse>,
    span: FileSpan<'parse>,
    // Dart `_addRestMap` reports the error at `nodeWithSpan.span` — the raw
    // rest-argument span (async_evaluate.dart:3945), not the resolved
    // expression node used for the without-slash deprecation span.
    error_span: FileSpan<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    for (key, val) in &map.entries {
        if let ValueKind::String(s) = &**key {
            let cleaned = without_slash(config, state, arena, *val, span)?;
            values.insert(s.text.to_string(), cleaned);
        } else {
            let key_str = key.to_display_string()?;
            let map_str = map.to_display_string()?;
            let err = exception(
                state,
                format!("Variable keyword argument map must have string keys.\n{key_str} is not a string in {map_str}."),
                Some(error_span),
            );
            return Err(Box::new(err));
        }
    }
    Ok(())
}

// ===========================================================================
// evaluateMacroArguments — evaluates rest args for macro invocations
// Go: evaluate_helpers.go:395
// ===========================================================================
// Matches Dart: `_evaluateMacroArguments(invocation)` (evaluate.dart:3864).
// Lazily separates rest/keyword-rest for macros like `if()`: positional/named
// pass through as *expressions* (unevaluated), while the evaluated rest value
// is re-wrapped in `ValueExpression` at the raw rest-argument span. Map-rest
// errors report the whole-invocation `span` argument here (unlike
// [`evaluate_arguments`], whose rest errors use the rest-arg span); a non-map
// keyword-rest raises at its own argument span.

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_macro_arguments<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    args: &ArgumentList<'parse>,
    span: FileSpan<'parse>,
) -> SassResult<(
    Vec<Expression<'parse>>,
    IndexMap<String, Expression<'parse>>,
)>
where
    'compile: 'parse,
{
    let rest_args = match &args.rest {
        Some(rest_expr) => rest_expr,
        None => {
            return Ok((args.positional.clone(), args.named.clone()));
        }
    };

    let mut positional: Vec<Expression<'parse>> = args.positional.clone();
    let mut named: IndexMap<String, Expression<'parse>> = args
        .named
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let rest = evaluate_expression(config, state, arena, rest_args).await?;
    let rest_span = expression_node(config, state, rest_args)?;
    let rest_args_span = rest_args.span()?;

    match &*rest {
        ValueKind::Map(m) => {
            for (key, val) in &m.entries {
                if let ValueKind::String(s) = &**key {
                    let cleaned = without_slash(config, state, arena, *val, rest_span)?;
                    named.insert(
                        s.text.to_string(),
                        Expression::Value(ValueExpression {
                            value: Box::new(cleaned),
                            span: rest_args_span,
                        }),
                    );
                } else {
                    let key_str = key.to_display_string()?;
                    let map_str = rest.to_display_string()?;
                    return Err(Box::new(exception(
                        state,
                        format!("Variable keyword argument map must have string keys.\n{key_str} is not a string in {map_str}."),
                        Some(span),
                    )));
                }
            }
        }
        ValueKind::ArgumentList(arg_list) => {
            for item in arg_list.as_list() {
                let cleaned = without_slash(config, state, arena, *item, rest_span)?;
                positional.push(Expression::Value(ValueExpression {
                    value: Box::new(cleaned),
                    span: rest_args_span,
                }));
            }
            for (key, val) in arg_list.keywords().iter() {
                let cleaned = without_slash(config, state, arena, *val, rest_span)?;
                named.insert(
                    key.clone(),
                    Expression::Value(ValueExpression {
                        value: Box::new(cleaned),
                        span: rest_args_span,
                    }),
                );
            }
        }
        ValueKind::List(l) => {
            for item in l.as_list() {
                let cleaned = without_slash(config, state, arena, *item, rest_span)?;
                positional.push(Expression::Value(ValueExpression {
                    value: Box::new(cleaned),
                    span: rest_args_span,
                }));
            }
        }
        _ => {
            let cleaned = without_slash(config, state, arena, rest, rest_span)?;
            positional.push(Expression::Value(ValueExpression {
                value: Box::new(cleaned),
                span: rest_args_span,
            }));
        }
    }

    let kw_rest = match &args.keyword_rest {
        Some(kw_expr) => kw_expr,
        None => return Ok((positional, named)),
    };

    let keyword_rest = evaluate_expression(config, state, arena, kw_rest).await?;
    let kw_span = expression_node(config, state, kw_rest)?;
    let kw_args_span = kw_rest.span()?;

    match &*keyword_rest {
        ValueKind::Map(m) => {
            for (key, val) in &m.entries {
                if let ValueKind::String(s) = &**key {
                    let cleaned = without_slash(config, state, arena, *val, kw_span)?;
                    named.insert(
                        s.text.to_string(),
                        Expression::Value(ValueExpression {
                            value: Box::new(cleaned),
                            span: kw_args_span,
                        }),
                    );
                } else {
                    let key_str = key.to_display_string()?;
                    let map_str = keyword_rest.to_display_string()?;
                    return Err(Box::new(exception(
                        state,
                        format!("Variable keyword argument map must have string keys.\n{key_str} is not a string in {map_str}."),
                        Some(span),
                    )));
                }
            }
        }
        _ => {
            return Err(Box::new(exception(
                state,
                format!(
                    "Variable keyword arguments must be a map (was {}).",
                    keyword_rest.to_display_string()?
                ),
                Some(span),
            )));
        }
    }

    Ok((positional, named))
}

// ===========================================================================
// copyParentAfterSibling — duplicates parent if not last child
// Go: evaluate_helpers.go:625
// ===========================================================================
// Matches Dart: `_copyParentAfterSibling()` (evaluate.dart:4533). When the
// current parent is followed by a sibling (e.g. a declaration wrote CSS into a
// rule that a later bubbled rule already left), replaces it with a childless
// copy appended to the grandparent, so subsequent children land in a fresh
// node instead of corrupting the sibling's group. No-op without a parent or
// grandparent.

pub(crate) fn copy_parent_after_sibling<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    state: &mut EvalState<'compile, 'parse>,
) -> SassResult<()> {
    let parent = match state.parent.as_ref() {
        Some(p) => p,
        None => return Ok(()),
    };
    let grandparent = match parent.parent() {
        Some(gp) => gp,
        None => return Ok(()),
    };
    if grandparent.children_len() == 0 {
        return Ok(());
    }
    if let Some(last) = grandparent.last_child() {
        if last != *parent {
            let new_parent = parent.copy_without_children(arena);
            grandparent.add_child(&new_parent)?;
            state.parent = Some(new_parent);
        }
    }
    Ok(())
}

// ===========================================================================
// Callable::dummy — for StackFrame construction when no callable is available
// ===========================================================================

impl<'compile, 'parse> Callable<'compile, 'parse> {
    pub fn dummy(arena: &'compile Bump) -> Self
    where
        'compile: 'parse,
    {
        Callable::new(
            arena,
            CallableKind::PlainCss(PlainCssCallable {
                name: "dummy".to_string(),
            }),
        )
    }
}

// ===========================================================================
// TESTS
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::css::media_query::CssMediaQuery;
    use crate::ast::css::modifiable_node::ModifiableCssNode;
    use crate::ast::css::style_rule::ModifiableCssStyleRule;
    use crate::ast::css::stylesheet::ModifiableCssStylesheet;
    use crate::ast::sass::argument_list::ArgumentList;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_number::NumberExpression;
    use crate::ast::sass::expression_value::ValueExpression;
    use crate::ast::sass::expression_variable::VariableExpression;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::ast::sass::interpolation::InterpolationPart;
    use crate::ast::sass::parameter::Parameter;
    use crate::ast::sass::parameter_list::ParameterList;
    use crate::common::source_span_file_source::FileSource;
    use crate::common::span::Span;
    use crate::compile::compile_string;
    use crate::compile::CompileOptions;
    use crate::eval::warn::warn;
    use crate::io::Io;
    use crate::io::VirtualIo;
    use crate::logger::QuietLogger;
    use crate::selector::class::ClassSelector;
    use crate::selector::complex::ComplexSelector;
    use crate::selector::complex_component::ComplexSelectorComponent;
    use crate::selector::compound::CompoundSelector;
    use crate::selector::list::SelectorList;
    use crate::selector::SimpleSelector;
    use crate::value::ListSeparator;
    use crate::value::SassColor;
    use crate::value::SassList;
    use crate::value::SassMap;
    use crate::value::SassString;
    use std::cell::RefCell;

    use crate::compile_context::new_compile_context;
    use crate::eval::WarnKey;
    use crate::logger::Logger;
    use crate::value::SassNumber;
    use bumpalo::Bump;
    use std::collections::HashSet;
    use std::rc::Rc;

    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    fn test_visitor<'compile, 'parse>(
        arena: &'compile Bump,
    ) -> (EvalConfig<'compile, 'parse>, EvalState<'compile, 'parse>)
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let span = make_span(arena, "test { color: red; }");
        let logger: Rc<dyn Logger> = Rc::new(QuietLogger);
        let config = EvalConfig::new(
            arena,
            logger,
            new_compile_context(),
            Rc::new(VirtualIo::new()),
        );
        let mut state = EvalState::new(arena, span);
        state.member = "root stylesheet".to_string();
        (config, state)
    }

    // ── exception ──

    #[rust_sass_macros::maybe_test]
    async fn test_exception_with_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (_, mut state) = test_visitor(&arena);
        let err = exception(&mut state, "something went wrong".into(), Some(span));
        assert_eq!(err.message(), "something went wrong");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_exception_without_span_uses_stack() {
        let arena = Bump::new();
        let span = make_span(&arena, "frame span");
        let (_, mut state) = test_visitor(&arena);
        state.stack = Some(Box::new(StackFrame {
            name: "my-func()".into(),
            span,
            callable: Callable::dummy(&arena),
            parent: None,
        }));
        let err = exception(&mut state, "error".into(), None);
        assert_eq!(err.message(), "error");
    }

    // ── stackTrace ──

    #[rust_sass_macros::maybe_test]
    async fn test_stack_trace_empty() {
        let arena = Bump::new();
        let span = make_span(&arena, "root span");
        let (_, mut state) = test_visitor(&arena);
        let trace = stack_trace(&mut state, Some(span));
        assert_eq!(trace.len(), 1);
        assert_eq!(trace[0].member, "root stylesheet");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_stack_trace_single_frame() {
        let arena = Bump::new();
        let span = make_span(&arena, "call span");
        let span2 = make_span(&arena, "mixin span");
        let (_, mut state) = test_visitor(&arena);
        state.stack = Some(Box::new(StackFrame {
            name: "my-mixin()".into(),
            span: span2,
            callable: Callable::dummy(&arena),
            parent: None,
        }));
        let trace = stack_trace(&mut state, Some(span));
        assert_eq!(trace.len(), 2);
        assert_eq!(trace[0].member, "root stylesheet");
        assert_eq!(trace[1].member, "my-mixin()");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_stack_trace_multi_frame_ordering() {
        let arena = Bump::new();
        let span = make_span(&arena, "root");
        let span2 = make_span(&arena, "helper span");
        let span3 = make_span(&arena, "mixin span");
        let (_, mut state) = test_visitor(&arena);
        let innermost = StackFrame {
            name: "helper()".into(),
            span: span2,
            callable: Callable::dummy(&arena),
            parent: None,
        };
        let middle = StackFrame {
            name: "my-mixin()".into(),
            span: span3,
            callable: Callable::dummy(&arena),
            parent: Some(Box::new(innermost)),
        };
        state.stack = Some(Box::new(middle));
        let trace = stack_trace(&mut state, Some(span));
        assert_eq!(trace.len(), 3);
        assert_eq!(trace[0].member, "root stylesheet");
        assert_eq!(trace[1].member, "my-mixin()");
        assert_eq!(trace[2].member, "helper()");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_stack_trace_nil_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "mixin span");
        let (_config, mut state) = test_visitor(&arena);
        state.stack = Some(Box::new(StackFrame {
            name: "my-mixin()".into(),
            span,
            callable: Callable::dummy(&arena),
            parent: None,
        }));
        let trace = stack_trace(&mut state, None);
        assert_eq!(trace.len(), 1);
        assert_eq!(trace[0].member, "my-mixin()");
    }

    // ── warn ──

    #[rust_sass_macros::maybe_test]
    async fn test_warn_message() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        warn(&config, &mut state, "test warning", span).unwrap();
        let key = WarnKey::from_span("test warning", span);
        assert!(state.warnings_emitted.contains(&key));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_warn_dedup_same_message_and_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        warn(&config, &mut state, "test warning", span).unwrap();
        warn(&config, &mut state, "test warning", span).unwrap();
        assert_eq!(state.warnings_emitted.len(), 1);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_warn_different_messages_emit_both() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        warn(&config, &mut state, "first", span).unwrap();
        warn(&config, &mut state, "second", span).unwrap();
        assert_eq!(state.warnings_emitted.len(), 2);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_warn_different_spans_emit_both() {
        let arena = Bump::new();
        let span1 = make_span(&arena, "test1");
        let span2 = make_span(&arena, "test2");
        let (config, mut state) = test_visitor(&arena);
        warn(&config, &mut state, "same", span1).unwrap();
        warn(&config, &mut state, "same", span2).unwrap();
        assert_eq!(state.warnings_emitted.len(), 2);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_warn_quiet_deps_suppresses() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (mut config, mut state) = test_visitor(&arena);
        config.quiet_deps = true;
        state.in_dependency = true;
        warn(&config, &mut state, "suppressed", span).unwrap();
        assert_eq!(state.warnings_emitted.len(), 0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_warn_quiet_deps_but_not_in_dependency() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (mut config, mut state) = test_visitor(&arena);
        config.quiet_deps = true;
        state.in_dependency = false;
        warn(&config, &mut state, "should appear", span).unwrap();
        assert_eq!(state.warnings_emitted.len(), 1);
    }

    // ── withEvaluationContext ──

    #[rust_sass_macros::maybe_test]
    async fn test_with_evaluation_context_save_restore() {
        let arena = Bump::new();
        let span1 = make_span(&arena, "old span");
        let span2 = make_span(&arena, "new span");
        let (config, mut state) = test_visitor(&arena);
        state.default_warn_span = span1;
        let old = state.default_warn_span;

        let result = with_evaluation_context(&config, &mut state, span2, async |_, _| {
            Ok::<_, Box<SassError>>(42)
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(state.default_warn_span, old);
    }

    // ── with_stack_frame ──

    #[rust_sass_macros::maybe_test]
    async fn test_with_stack_frame_save_restore() {
        let arena = Bump::new();
        let span = make_span(&arena, "callable body");
        let (config, mut state) = test_visitor(&arena);
        let old_member = state.member.clone();

        let result = with_stack_frame(
            &config,
            &mut state,
            &arena,
            "my-function()",
            span,
            None,
            async |_, s| Ok::<_, Box<SassError>>(s.member.clone()),
        )
        .await;
        assert_eq!(result.unwrap(), "my-function()");
        assert_eq!(state.member, old_member);
        assert!(state.stack.is_none());
    }

    // ── with_environment ──

    #[rust_sass_macros::maybe_test]
    async fn test_with_environment_swap_restore() {
        let arena = Bump::new();
        let span = make_span(&arena, "env-test");
        let (config, mut state) = test_visitor(&arena);

        let new_env = Environment::new(&arena);
        new_env.set_local_variable("x", Value::new_with_arena(&arena, ValueKind::Null), span);

        with_environment(&config, &mut state, new_env, async |_, s| {
            let val = s.env.get_variable("x", None).unwrap();
            assert!(val.is_some());
            Ok::<_, Box<SassError>>(())
        })
        .await
        .unwrap();

        let val = state.env.get_variable("x", None).unwrap();
        assert!(val.is_none());
    }

    // ── verifyParameterList ──

    #[rust_sass_macros::maybe_test]
    async fn test_verify_parameter_list_exact_match() {
        let arena = Bump::new();
        let span = make_span(&arena, "$x, $y");
        let p = ParameterList::new(
            vec![
                Parameter::new("x".into(), span, None),
                Parameter::new("y".into(), span, None),
            ],
            span,
            None,
        );
        let result = verify_parameter_list(2, &HashSet::new(), Some(&p), span);
        assert!(result.is_ok());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_verify_parameter_list_too_many_positional() {
        let arena = Bump::new();
        let span = make_span(&arena, "$x");
        let p = ParameterList::new(vec![Parameter::new("x".into(), span, None)], span, None);
        let err = verify_parameter_list(3, &HashSet::new(), Some(&p), span).unwrap_err();
        assert!(err.message().contains("Only 1 argument allowed"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_verify_parameter_list_missing_argument() {
        let arena = Bump::new();
        let span = make_span(&arena, "$x, $y");
        let span_x = make_span(&arena, "$x");
        let span_y = make_span(&arena, "$y");
        let p = ParameterList::new(
            vec![
                Parameter::new("x".into(), span_x, None),
                Parameter::new("y".into(), span_y, None),
            ],
            span,
            None,
        );
        let err = verify_parameter_list(1, &HashSet::new(), Some(&p), span).unwrap_err();
        assert!(err.message().contains("Missing argument $y."));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_verify_parameter_list_both_positional_and_named() {
        let arena = Bump::new();
        let span = make_span(&arena, "$x");
        let p = ParameterList::new(vec![Parameter::new("x".into(), span, None)], span, None);
        let mut names = HashSet::new();
        names.insert("x".into());
        let err = verify_parameter_list(1, &names, Some(&p), span).unwrap_err();
        assert!(err
            .message()
            .contains("passed both by position and by name"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_verify_parameter_list_unknown_named() {
        let arena = Bump::new();
        let span = make_span(&arena, "$x");
        let p = ParameterList::new(vec![Parameter::new("x".into(), span, None)], span, None);
        let mut names = HashSet::new();
        names.insert("z".into());
        let err = verify_parameter_list(1, &names, Some(&p), span).unwrap_err();
        assert!(
            err.message().contains("No parameter named $z."),
            "got: {}",
            err.message()
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_verify_parameter_list_rest_accepts_all() {
        let arena = Bump::new();
        let span = make_span(&arena, "$x...");
        let p = ParameterList::new(
            vec![Parameter::new("x".into(), span, None)],
            span,
            Some("rest".into()),
        );
        let result = verify_parameter_list(10, &HashSet::new(), Some(&p), span);
        assert!(result.is_ok());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_verify_parameter_list_nil_params_no_args() {
        let arena = Bump::new();
        let span = make_span(&arena, "");
        let result = verify_parameter_list(0, &HashSet::new(), None, span);
        assert!(result.is_ok());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_verify_parameter_list_nil_params_with_args() {
        let arena = Bump::new();
        let span = make_span(&arena, "");
        let err = verify_parameter_list(1, &HashSet::new(), None, span).unwrap_err();
        assert!(err.message().contains("Expected 0 arguments"));
    }

    // ── mergeMediaQueries ──

    #[rust_sass_macros::maybe_test]
    async fn test_merge_media_queries_empty_first() {
        let q1: Vec<CssMediaQuery> = vec![];
        let q2 = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        let result = merge_media_queries(&q1, &q2);
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_merge_media_queries_matching() {
        let q1 = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        let q2 = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        let result = merge_media_queries(&q1, &q2);
        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 1);
    }

    // ── styleRule (test skipped: lifetime issue with invariant EvalState)
    // style_rule() borrows from state and returns a reference; lifetime
    // elision with invariant EvalState<'compile, 'parse> needs explicit handling.
    // Function compiles, verified via cargo check. Test deferred.

    // ── withMediaQueries ──

    #[rust_sass_macros::maybe_test]
    async fn test_with_media_queries_save_restore() {
        let arena = Bump::new();
        let (config, mut state) = test_visitor(&arena);
        let queries = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        assert!(state.media_queries.is_none());

        with_media_queries(
            &config,
            &mut state,
            Some(queries.clone()),
            None,
            async |_, s| {
                assert_eq!(s.media_queries.as_ref().unwrap().len(), 1);
                Ok(())
            },
        )
        .await
        .unwrap();

        assert!(state.media_queries.is_none());
    }

    // ── evaluateArguments ──

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_arguments_basic() {
        let arena = Bump::new();
        let span = make_span(&arena, "a, b");
        let (config, mut state) = test_visitor(&arena);
        let args = ArgumentList::new(vec![], IndexMap::new(), IndexMap::new(), span, None, None);
        let result = evaluate_arguments(&config, &mut state, &arena, &args)
            .await
            .unwrap();
        assert!(result.positional.is_empty());
        assert!(result.named.is_empty());
    }

    // ── bindArguments ──

    #[rust_sass_macros::maybe_test]
    async fn test_bind_arguments_sets_locals() {
        let arena = Bump::new();
        let span = make_span(&arena, "$x");
        let (config, mut state) = test_visitor(&arena);
        let params = ParameterList::new(vec![Parameter::new("x".into(), span, None)], span, None);
        let positional = vec![Value::new_with_arena(&arena, ValueKind::Null)];
        let named = IndexMap::new();
        let pos_nodes = vec![span];
        let named_nodes = IndexMap::new();

        bind_arguments(
            &config,
            &mut state,
            &arena,
            &params,
            &positional,
            &pos_nodes,
            &named,
            &named_nodes,
            ListSeparator::Undecided,
        )
        .await
        .unwrap();

        let val = state.env.get_variable("x", None).unwrap();
        assert!(val.is_some());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_bind_arguments_missing_required() {
        let arena = Bump::new();
        let span = make_span(&arena, "$x");
        let (config, mut state) = test_visitor(&arena);
        let params = ParameterList::new(vec![Parameter::new("x".into(), span, None)], span, None);
        let positional: Vec<Value> = vec![];
        let named = IndexMap::new();
        let pos_nodes: Vec<FileSpan> = vec![];
        let named_nodes = IndexMap::new();

        let err = bind_arguments(
            &config,
            &mut state,
            &arena,
            &params,
            &positional,
            &pos_nodes,
            &named,
            &named_nodes,
            ListSeparator::Undecided,
        )
        .await
        .unwrap_err();
        assert!(
            err.message().contains("Missing argument $x."),
            "got: {}",
            err.message()
        );
    }

    // ── evaluateArguments rest/kwRest ──

    fn make_args<'parse>(
        positional: Vec<Expression<'parse>>,
        named: IndexMap<String, Expression<'parse>>,
        rest: Option<Box<Expression<'parse>>>,
        kw_rest: Option<Box<Expression<'parse>>>,
        span: FileSpan<'parse>,
    ) -> ArgumentList<'parse> {
        ArgumentList {
            positional,
            named,
            named_spans: IndexMap::new(),
            span,
            rest,
            keyword_rest: kw_rest,
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_arguments_rest_map() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        let m = SassMap {
            entries: {
                let mut e = IndexMap::new();
                e.insert(
                    Value::new_with_arena(
                        &arena,
                        ValueKind::String(SassString {
                            text: "k",
                            has_quotes: false,
                        }),
                    ),
                    Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(1.0, None))),
                );
                e
            },
        };
        let args = make_args(
            vec![],
            IndexMap::new(),
            Some(Box::new(Expression::Value(ValueExpression {
                value: Box::new(Value::new_with_arena(&arena, ValueKind::Map(m))),
                span,
            }))),
            None,
            span,
        );
        let results = evaluate_arguments(&config, &mut state, &arena, &args)
            .await
            .unwrap();
        assert_eq!(results.named.len(), 1);
        assert!(results.named.contains_key("k"));
        assert_eq!(results.named_nodes.len(), 1);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_arguments_rest_list() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        let lst = SassList::new(
            vec![
                Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(1.0, None))),
                Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(2.0, None))),
            ],
            ListSeparator::Comma,
            false,
        );
        let args = make_args(
            vec![],
            IndexMap::new(),
            Some(Box::new(Expression::Value(ValueExpression {
                value: Box::new(Value::new_with_arena(&arena, ValueKind::List(lst))),
                span,
            }))),
            None,
            span,
        );
        let results = evaluate_arguments(&config, &mut state, &arena, &args)
            .await
            .unwrap();
        assert_eq!(results.positional.len(), 2);
        assert_eq!(results.positional_nodes.len(), 2);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_arguments_rest_single() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        let args = make_args(
            vec![],
            IndexMap::new(),
            Some(Box::new(Expression::Value(ValueExpression {
                value: Box::new(Value::new_with_arena(
                    &arena,
                    ValueKind::Number(SassNumber::new(42.0, None)),
                )),
                span,
            }))),
            None,
            span,
        );
        let results = evaluate_arguments(&config, &mut state, &arena, &args)
            .await
            .unwrap();
        assert_eq!(results.positional.len(), 1);
        assert_eq!(results.positional_nodes.len(), 1);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_arguments_kw_rest_not_map() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        let args = make_args(
            vec![],
            IndexMap::new(),
            None,
            Some(Box::new(Expression::Value(ValueExpression {
                value: Box::new(Value::new_with_arena(
                    &arena,
                    ValueKind::Number(SassNumber::new(1.0, None)),
                )),
                span,
            }))),
            span,
        );
        let err = evaluate_arguments(&config, &mut state, &arena, &args)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "Variable keyword arguments must be a map (was 1)."
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_arguments_positional_nodes() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        let args = make_args(
            vec![Expression::Value(ValueExpression {
                value: Box::new(Value::new_with_arena(
                    &arena,
                    ValueKind::Number(SassNumber::new(10.0, None)),
                )),
                span,
            })],
            IndexMap::new(),
            None,
            None,
            span,
        );
        let results = evaluate_arguments(&config, &mut state, &arena, &args)
            .await
            .unwrap();
        assert_eq!(results.positional.len(), 1);
        assert_eq!(results.positional_nodes.len(), 1);
        assert_eq!(results.positional_nodes[0], span);
    }

    // ── evaluateMacroArguments ──

    #[rust_sass_macros::maybe_test]
    async fn test_macro_arguments_no_rest() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        let pos_expr = Expression::Value(ValueExpression {
            value: Box::new(Value::new_with_arena(
                &arena,
                ValueKind::Number(SassNumber::new(1.0, None)),
            )),
            span,
        });
        let args = make_args(vec![pos_expr], IndexMap::new(), None, None, span);
        let (pos, named) = evaluate_macro_arguments(&config, &mut state, &arena, &args, span)
            .await
            .unwrap();
        assert_eq!(pos.len(), 1);
        assert!(named.is_empty());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_macro_arguments_rest_map() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        let m = SassMap {
            entries: {
                let mut e = IndexMap::new();
                e.insert(
                    Value::new_with_arena(
                        &arena,
                        ValueKind::String(SassString {
                            text: "k",
                            has_quotes: false,
                        }),
                    ),
                    Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(5.0, None))),
                );
                e
            },
        };
        let args = make_args(
            vec![],
            IndexMap::new(),
            Some(Box::new(Expression::Value(ValueExpression {
                value: Box::new(Value::new_with_arena(&arena, ValueKind::Map(m))),
                span,
            }))),
            None,
            span,
        );
        let (pos, named) = evaluate_macro_arguments(&config, &mut state, &arena, &args, span)
            .await
            .unwrap();
        assert_eq!(pos.len(), 0);
        assert_eq!(named.len(), 1);
        assert!(named.contains_key("k"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_macro_arguments_kw_rest_not_map() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        let args = make_args(
            vec![],
            IndexMap::new(),
            Some(Box::new(Expression::Value(ValueExpression {
                value: Box::new(Value::new_with_arena(
                    &arena,
                    ValueKind::Number(SassNumber::new(1.0, None)),
                )),
                span,
            }))),
            Some(Box::new(Expression::Value(ValueExpression {
                value: Box::new(Value::new_with_arena(
                    &arena,
                    ValueKind::Number(SassNumber::new(2.0, None)),
                )),
                span,
            }))),
            span,
        );
        let err = evaluate_macro_arguments(&config, &mut state, &arena, &args, span)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "Variable keyword arguments must be a map (was 2)."
        );
    }

    // ── performInterpolation ──

    #[rust_sass_macros::maybe_test]
    async fn test_perform_interpolation_plain_text() {
        let arena = Bump::new();
        let span = make_span(&arena, "");
        let (config, mut state) = test_visitor(&arena);
        let interp = Interpolation::plain("hello world".to_string(), Span::File(span));
        let result = perform_interpolation(&config, &mut state, &arena, &interp, false)
            .await
            .unwrap();
        assert_eq!(result, "hello world");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_perform_interpolation_with_map_plain_text() {
        let arena = Bump::new();
        let span = make_span(&arena, "");
        let (config, mut state) = test_visitor(&arena);
        let interp = Interpolation::plain("hello".to_string(), Span::File(span));
        let (result, map) =
            perform_interpolation_with_map(&config, &mut state, &arena, &interp, false)
                .await
                .unwrap();
        assert_eq!(result, "hello");
        // Plain text interpolation (1 component) produces a valid map with 0 offsets
        let _ = map;
    }

    #[rust_sass_macros::maybe_test]
    async fn test_perform_interpolation_with_map_expression() {
        let arena = Bump::new();
        let span = make_span(&arena, "a#{1+1}b");
        let (config, mut state) = test_visitor(&arena);
        let expr = Expression::Number(NumberExpression {
            value: 42.0,
            unit: None,
            span,
        });
        let interp = Interpolation::new(
            vec![
                InterpolationPart::Text("prefix:".into()),
                InterpolationPart::Expression(Box::new(expr)),
                InterpolationPart::Text("suffix".into()),
            ],
            vec![None, Some(span), None],
            Span::File(span),
        )
        .unwrap();
        let (result, map) =
            perform_interpolation_with_map(&config, &mut state, &arena, &interp, false)
                .await
                .unwrap();
        assert_eq!(result, "prefix:42suffix");
        let _ = map;
    }

    #[rust_sass_macros::maybe_test]
    async fn test_perform_interpolation_warn_for_color() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        let color = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        let expr = Expression::Value(ValueExpression {
            value: Box::new(Value::new_with_arena(&arena, ValueKind::Color(color))),
            span,
        });
        let interp = Interpolation::new(
            vec![InterpolationPart::Expression(Box::new(expr))],
            vec![Some(span)],
            Span::File(span),
        )
        .unwrap();
        let result = perform_interpolation(&config, &mut state, &arena, &interp, true)
            .await
            .unwrap();
        assert_eq!(result, "red");
        let warnings = &state.warnings_emitted;
        assert_eq!(warnings.len(), 1);
        let key = warnings.iter().next().unwrap();
        assert_eq!(
            key.message,
            format!(
                "You probably don't mean to use the color value \
                 red in interpolation here.\n\
                 It may end up represented as red, which will likely produce \
                 invalid CSS.\n\
                 Always quote color names when using them as strings or map keys \
                 (for example, \"red\").\n\
                 If you really want to use the color value here, use '\"\" + test'."
            )
        );
    }

    // ── without_slash ──

    #[rust_sass_macros::maybe_test]
    async fn test_without_slash_number_with_slash() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        let num = SassNumber::new(42.0, None).with_slash(
            SassNumber::new(1.0, Some("px")),
            SassNumber::new(2.0, Some("px")),
        );
        let val = Value::new_with_arena(&arena, ValueKind::Number(num));

        let result = without_slash(&config, &mut state, &arena, val, span).unwrap();
        match &*result {
            ValueKind::Number(n) => assert!(!n.has_slash()),
            other => panic!("expected Number, got {other:?}"),
        }
        let warnings = &state.warnings_emitted;
        assert_eq!(warnings.len(), 1);
        let key = warnings.iter().next().unwrap();
        assert_eq!(
            key.message,
            format!(
                "Using / for division is deprecated and will be removed in Dart Sass 2.0.0.\n\n\
                 Recommendation: math.div(1px, 2px)\n\n\
                 More info and automated migrator: https://sass-lang.com/d/slash-div"
            )
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_without_slash_number_without_slash() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        let num = SassNumber::new(42.0, None);
        let val = Value::new_with_arena(&arena, ValueKind::Number(num));

        let result = without_slash(&config, &mut state, &arena, val, span).unwrap();
        match &*result {
            ValueKind::Number(n) => assert!(!n.has_slash()),
            other => panic!("expected Number, got {other:?}"),
        }
        assert_eq!(state.warnings_emitted.len(), 0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_without_slash_non_number() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let (config, mut state) = test_visitor(&arena);
        let val = Value::new_with_arena(&arena, ValueKind::Null);

        let result = without_slash(&config, &mut state, &arena, val, span).unwrap();
        assert!(matches!(*result, ValueKind::Null));
        assert_eq!(state.warnings_emitted.len(), 0);
    }

    // ── slash_division_recommendation ──

    #[rust_sass_macros::maybe_test]
    async fn test_slash_division_recommendation_simple() {
        let num = SassNumber::new(1.0, Some("px"))
            .with_slash(SassNumber::new(1.0, None), SassNumber::new(2.0, None));
        let got = slash_division_recommendation(&num).unwrap();
        assert_eq!(got, "math.div(1, 2)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_slash_division_recommendation_nested() {
        let inner = SassNumber::new(1.0, Some("px")).with_slash(
            SassNumber::new(1.0, Some("px")),
            SassNumber::new(2.0, Some("px")),
        );
        let outer = SassNumber::new(42.0, None).with_slash(inner, SassNumber::new(3.0, Some("px")));
        let got = slash_division_recommendation(&outer).unwrap();
        assert_eq!(got, "math.div(math.div(1px, 2px), 3px)");
    }

    // ── expression_node ──

    #[rust_sass_macros::maybe_test]
    async fn test_expression_node_non_variable() {
        let arena = Bump::new();
        let span = make_span(&arena, "42");
        let (_config, state) = test_visitor(&arena);
        let expr = Expression::Number(NumberExpression {
            value: 42.0,
            unit: None,
            span,
        });
        let result = expression_node(&_config, &state, &expr).unwrap();
        assert_eq!(result, span);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_expression_node_variable_found() {
        let arena = Bump::new();
        let def_span = make_span(&arena, "definition");
        let ref_span = make_span(&arena, "reference");
        let (_config, state) = test_visitor(&arena);
        // Store a variable with a definition span in the environment
        state.env.set_local_variable(
            "x",
            Value::new_with_arena(&arena, ValueKind::Null),
            def_span,
        );
        let ve = Expression::Variable(VariableExpression {
            name: "x".into(),
            span: ref_span,
            namespace: None,
        });
        let result = expression_node(&_config, &state, &ve).unwrap();
        assert_eq!(result, def_span);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_function_without_return_span() {
        // Dart `_runFunctionCallable` throws `_exception("Function finished
        // without @return.", callable.declaration.span)`: a spanned `Runtime`
        // at the function declaration with the call trace.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@function f() { $x: 1; }\na { b: f(); }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime {
                message,
                span,
                trace,
                ..
            } => {
                assert_eq!(message, "Function finished without @return.");
                assert_eq!(span.line(), 1);
                assert_eq!(span.column(), 1);
                assert_eq!(trace.len(), 2);
                assert_eq!(trace[0].member, "f()");
                assert_eq!((trace[0].line, trace[0].column), (1, 1));
                assert_eq!(trace[1].member, "root stylesheet");
                assert_eq!((trace[1].line, trace[1].column), (2, 8));
            }
            other => panic!("expected spanned Runtime, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_keyword_rest_use_span() {
        // Dart `_evaluateArguments` reports a non-map keyword rest at the
        // use-site `keywordRestArgs.span`, not the resolved declaration span.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@function f($a) { @return $a; }\n$m: 1;\n$n: 2;\na { b: f($m..., $n...); }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, span, .. } => {
                assert_eq!(message, "Variable keyword arguments must be a map (was 2).");
                assert_eq!((span.line(), span.column()), (4, 17));
            }
            other => panic!("expected Runtime error, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_over_arity_positional_prefix() {
        // Dart `ParameterList.verify` includes "positional " in the over-arity
        // message when named arguments are present.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@function f($a, $b) { @return $a; }\na { b: f(1, 2, 3, $c: 4); }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::MultiSpan { message, .. } => assert_eq!(
                message,
                "Only 2 positional arguments allowed, but 3 were passed."
            ),
            other => panic!("expected MultiSpan error, got {other:?}"),
        }

        // Without named arguments the prefix is absent.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@function f($a, $b) { @return $a; }\na { b: f(1, 2, 3); }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::MultiSpan { message, .. } => {
                assert_eq!(message, "Only 2 arguments allowed, but 3 were passed.")
            }
            other => panic!("expected MultiSpan error, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_argument_source_spelling() {
        // Dart `ParameterList.verify` reports `_originalParameterName`
        // (source spelling, e.g. `$foo_bar`) rather than the normalized
        // `$foo-bar`.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@function f($foo_bar) { @return $foo_bar; }\na { b: f(); }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::MultiSpan { message, .. } => {
                assert_eq!(message, "Missing argument $foo_bar.")
            }
            other => panic!("expected MultiSpan error, got {other:?}"),
        }

        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@function f($foo_bar) { @return $foo_bar; }\na { b: f(1, $foo_bar: 2); }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, span, .. } => {
                assert_eq!(
                    message,
                    "Argument $foo_bar was passed both by position and by name."
                );
                assert_eq!((span.line(), span.column()), (2, 8));
            }
            other => panic!("expected Runtime error, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_expression_node_variable_not_found() {
        let arena = Bump::new();
        let span = make_span(&arena, "reference");
        let (_config, state) = test_visitor(&arena);
        let ve = Expression::Variable(VariableExpression {
            name: "missing".into(),
            span,
            namespace: None,
        });
        let result = expression_node(&_config, &state, &ve).unwrap();
        assert_eq!(result, span);
    }

    fn make_style_rule_node<'compile, 'parse>(
        arena: &'compile Bump,
        span: FileSpan<'parse>,
    ) -> ModifiableCssNode<'parse>
    where
        'compile: 'parse,
    {
        let class = ClassSelector::new(".foo".into(), span);
        let compound = CompoundSelector::new(vec![SimpleSelector::Class(class)], span).unwrap();
        let comp = ComplexSelectorComponent::new(Box::new(compound), vec![], span);
        let complex = ComplexSelector::new(vec![], vec![comp], span, false).unwrap();
        let selector = SelectorList::new(arena, vec![complex], span).unwrap();
        let rule = ModifiableCssStyleRule::new(Rc::new(RefCell::new(selector)), span, None, false);
        ModifiableCssNode::new(arena, ModifiableCssNodeKind::StyleRule(rule))
    }

    #[rust_sass_macros::maybe_test]
    async fn test_has_css_nesting_at_root() {
        // Dart `_hasCssNesting` walks from `_styleRule`, which is null while
        // `_atRootExcludingStyleRule` is set: with a style-rule ancestor in
        // the live chain, the flag suppresses the nesting shortcut.
        let arena = Bump::new();
        let (_config, mut state) = test_visitor(&arena);
        let span = make_span(&arena, "a { b { } }");
        let root = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Stylesheet(ModifiableCssStylesheet::new(span)),
        );
        let outer = make_style_rule_node(&arena, span);
        let inner = make_style_rule_node(&arena, span);
        root.add_child(&outer).unwrap();
        outer.add_child(&inner).unwrap();
        state.root = Some(root.clone());
        state.parent = Some(inner.clone());
        state.css_style_rule_node = Some(inner);

        state.at_root_excluding_style_rule = false;
        assert!(has_css_nesting(&state));
        state.at_root_excluding_style_rule = true;
        assert!(!has_css_nesting(&state));
    }
}
