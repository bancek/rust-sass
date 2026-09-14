// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/evaluate.dart (_warn, _EvaluationContext.warn)
// go-source: go/eval/evaluate.go (warn method) + go/evalcontext/evaluation_context.go (WarnDeprecation)

use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::common::span::{MultiSpan, Span};
use crate::deprecation::Deprecation;
use crate::eval::helpers::stack_trace;
use crate::eval::{EvalConfig, EvalState, WarnKey};
use crate::logger::BufferedWarnLogger;
use crate::selector::WarnLogger;

// ===========================================================================
// warn_deprecation_impl — shared inner function for all deprecation variants
// ===========================================================================

fn warn_deprecation_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    message: &str,
    deprecation: &'static Deprecation,
    span: FileSpan<'parse>,
    dedup: bool,
) -> SassResult<()>
where
    'compile: 'parse,
{
    if config.quiet_deps && state.in_dependency {
        return Ok(());
    }

    if dedup {
        let key = WarnKey::from_span(message, span);
        if !state.warnings_emitted.insert(key) {
            return Ok(());
        }
    }

    let trace = stack_trace(state, Some(span));

    let s = Span::File(span);
    config
        .logger
        .warn_deprecation(message, Some(&s), deprecation, Some(&trace))
}

// ===========================================================================
// warn — non-deprecation warning with explicit span
// Dart: _warn with null deprecation (evaluate.dart:4683)
// Go: v.warn with nil deprecation (evaluate.go:326)
//
// Emits a warning with the given `message` about the given `span`.
// ===========================================================================

pub(crate) fn warn<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    message: &str,
    span: FileSpan<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    if config.quiet_deps && state.in_dependency {
        return Ok(());
    }
    let key = WarnKey::from_span(message, span);
    if !state.warnings_emitted.insert(key) {
        return Ok(());
    }
    let trace = stack_trace(state, Some(span));
    let span_s = Span::File(span);
    config.logger.warn(message, Some(&span_s), Some(&trace));
    Ok(())
}

// ===========================================================================
// warn_deprecation_span — deprecation warning with explicit span
// Dart: _EvaluateVisitor._warn with non-null deprecation (evaluate.dart:4683)
// Go: v.warn with non-nil deprecation (evaluate.go:326)
// ===========================================================================

pub(crate) fn warn_deprecation_span<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    message: &str,
    deprecation: &'static Deprecation,
    span: FileSpan<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    warn_deprecation_impl(config, state, message, deprecation, span, true)
}

// ===========================================================================
// warn_deprecation — convenience wrapper, derives span from state
// Dart: _EvaluationContext.warn() resolves span then calls _warn (evaluate.dart:4889)
// Go: ec.WarnDeprecation() resolves CurrentCallableSpan then calls warnFn→warn
// ===========================================================================

pub(crate) fn warn_deprecation<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    message: &str,
    deprecation: &'static Deprecation,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let span = state
        .import_span
        .or(state.callable_span)
        .unwrap_or(state.default_warn_span);

    warn_deprecation_impl(config, state, message, deprecation, span, true)
}

// ===========================================================================
// warn_deprecation_multi_span — deprecation with MultiSpan (no dedup)
// Dart: evaluate.dart:4683 (_warn with MultiSpan)
// Go: evaluate.go:326 (warn with multiSpanFileSpan)
// Note: MultiSpan uses identity equality in Dart (Object.==), so dedup
// never fires. We match this by skipping the WarnKey check entirely.
// ===========================================================================

pub(crate) fn warn_deprecation_multi_span<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    message: &str,
    primary_span: FileSpan<'parse>,
    primary_label: &str,
    secondary: Vec<(FileSpan<'parse>, String)>,
    deprecation: &'static Deprecation,
) -> SassResult<()>
where
    'compile: 'parse,
{
    if config.quiet_deps && state.in_dependency {
        return Ok(());
    }
    let trace = stack_trace(state, Some(primary_span));
    let span = Span::Multi(MultiSpan::new(
        Span::File(primary_span),
        primary_label.to_string(),
        secondary,
    ));
    config
        .logger
        .warn_deprecation(message, Some(&span), deprecation, Some(&trace))
}

// ===========================================================================
// WarnLoggerAdapter — bridges config+state for selector::WarnLogger trait
// ===========================================================================

pub(crate) struct WarnLoggerAdapter<'r, 'compile: 'parse, 'parse> {
    pub config: &'r EvalConfig<'compile, 'parse>,
    pub state: &'r mut EvalState<'compile, 'parse>,
}

impl<'r, 'compile: 'parse, 'parse> WarnLogger for WarnLoggerAdapter<'r, 'compile, 'parse> {
    fn warn_deprecation(
        &mut self,
        message: &str,
        deprecation: &'static Deprecation,
    ) -> SassResult<()> {
        warn_deprecation(self.config, self.state, message, deprecation)
    }
}

// ===========================================================================
// flush_buffered_warnings — drains a BufferedWarnLogger through the eval warn pipeline
// ===========================================================================

pub(crate) fn flush_buffered_warnings<'compile, 'parse>(
    warn_logger: &BufferedWarnLogger<'parse>,
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    for (msg, dep, span) in warn_logger.drain() {
        if let Some(span) = span {
            warn_deprecation_span(config, state, &msg, dep, span)?;
        } else {
            warn_deprecation(config, state, &msg, dep)?;
        }
    }
    Ok(())
}
