// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/logger/deprecation_processing.dart
// go-source: go/sasslogger/deprecation_processing.go

//! Silence/fatal/future/repetition handling for deprecation warnings.
//!
//! Matches Dart: `DeprecationProcessingLogger` in
//! `logger/deprecation_processing.dart`. The compile pipeline always wraps the
//! user logger in this decorator; `validate` runs before evaluation and
//! `summarize` after (see `ref/pipeline.md`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::common::exception::{SassError, SassResult, Trace};
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::common::span::Span;
use crate::deprecation::Deprecation;

use crate::logger::Logger;

/// Maximum repetitions of the same warning emitted before hiding the rest.
/// Matches Dart's `_maxRepetitions`.
const MAX_REPETITIONS: usize = 5;

/// A logger that wraps an inner logger to have special handling for
/// deprecation warnings, silencing, making fatal, enabling future, and/or
/// limiting repetition based on its inputs.
///
/// Warning counts live in a `RefCell` map so `warn_deprecation` stays `&self`
/// under `Rc<dyn Logger>` sharing. Set fields are `HashSet`s of canonical
/// `&'static Deprecation` identities.
pub struct DeprecationProcessingLogger<I> {
    inner: I,
    warning_counts: RefCell<HashMap<&'static Deprecation, usize>>,
    silence_deprecations: HashSet<&'static Deprecation>,
    fatal_deprecations: HashSet<&'static Deprecation>,
    future_deprecations: HashSet<&'static Deprecation>,
    limit_repetition: bool,
}

impl<I: fmt::Debug> fmt::Debug for DeprecationProcessingLogger<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeprecationProcessingLogger")
            .field("inner", &self.inner)
            .field("silence_deprecations", &self.silence_deprecations)
            .field("fatal_deprecations", &self.fatal_deprecations)
            .field("future_deprecations", &self.future_deprecations)
            .field("limit_repetition", &self.limit_repetition)
            .finish()
    }
}

impl<I: Logger> DeprecationProcessingLogger<I> {
    /// Wraps `inner` with the silence/fatal/future sets. Future deprecations
    /// in `fatal_deprecations` still error even when absent from
    /// `future_deprecations`; `limit_repetition` caps repeats at
    /// [`MAX_REPETITIONS`]. Matches Dart's constructor.
    pub fn new(
        inner: I,
        silence_deprecations: &[&'static Deprecation],
        fatal_deprecations: &[&'static Deprecation],
        future_deprecations: &[&'static Deprecation],
        limit_repetition: bool,
    ) -> Self {
        DeprecationProcessingLogger {
            inner,
            warning_counts: RefCell::new(HashMap::new()),
            silence_deprecations: silence_deprecations.iter().copied().collect(),
            fatal_deprecations: fatal_deprecations.iter().copied().collect(),
            future_deprecations: future_deprecations.iter().copied().collect(),
            limit_repetition,
        }
    }

    /// Warns if any of the deprecation options are incompatible or
    /// unnecessary (fatal future-not-enabled, obsolete, silence+fatal,
    /// silenced `user-authored`, conflicting future silence, enabled
    /// non-future). Runs before evaluation. Matches Dart's `validate`.
    pub fn validate(&self) {
        for dep in &self.fatal_deprecations {
            let in_future = self.future_deprecations.contains(dep);
            let in_silence = self.silence_deprecations.contains(dep);

            if dep.is_future && !in_future {
                self.inner.warn(
                    &format!(
                        "Future {} deprecation must be enabled before it can be made fatal.",
                        dep.id
                    ),
                    None,
                    None,
                );
            } else if dep.obsolete_in.is_some() {
                self.inner.warn(
                    &format!(
                        "{} deprecation is obsolete, so does not need to be made fatal.",
                        dep.id
                    ),
                    None,
                    None,
                );
            } else if in_silence {
                self.inner.warn(
                    &format!(
                        "Ignoring setting to silence {} deprecation, since it has also been made fatal.",
                        dep.id
                    ),
                    None,
                    None,
                );
            }
        }

        for dep in &self.silence_deprecations {
            let in_future = self.future_deprecations.contains(dep);

            if dep.id == "user-authored" {
                self.inner.warn(
                    "User-authored deprecations should not be silenced.",
                    None,
                    None,
                );
            } else if dep.obsolete_in.is_some() {
                self.inner.warn(
                    &format!(
                        "{} deprecation is obsolete. If you were previously silencing it, \
                         your code may now behave in unexpected ways.",
                        dep.id
                    ),
                    None,
                    None,
                );
            } else if dep.is_future && in_future {
                self.inner.warn(
                    &format!(
                        "Conflicting options for future {} deprecation cancel each other out.",
                        dep.id
                    ),
                    None,
                    None,
                );
            } else if dep.is_future {
                self.inner.warn(
                    &format!(
                        "Future {} deprecation is not yet active, so silencing it is unnecessary.",
                        dep.id
                    ),
                    None,
                    None,
                );
            }
        }

        for dep in &self.future_deprecations {
            if !dep.is_future {
                self.inner.warn(
                    &format!(
                        "{} is not a future deprecation, so it does not need to be explicitly enabled.",
                        dep.id
                    ),
                    None,
                    None,
                );
            }
        }
    }

    /// Prints a warning indicating the number of deprecation warnings that
    /// were omitted due to repetition. Runs after evaluation, on success and
    /// failure.
    ///
    /// The `js` flag indicates whether this is running in JS mode, in which
    /// case it doesn't mention "verbose mode" because the JS API doesn't
    /// support that. Matches Dart's `summarize({required bool js})`.
    pub fn summarize(&self, js: bool) {
        let counts = self.warning_counts.borrow();
        let mut total = 0usize;
        for count in counts.values() {
            if *count > MAX_REPETITIONS {
                total += count - MAX_REPETITIONS;
            }
        }
        if total > 0 {
            let mut msg = format!("{total} repetitive deprecation warnings omitted.");
            if !js {
                msg.push_str("\nRun in verbose mode to see all warnings.");
            }
            self.inner.warn(&msg, None, None);
        }
    }
}

impl<I: Logger> Logger for DeprecationProcessingLogger<I> {
    /// Forwards non-deprecation warnings untouched. Matches Dart's
    /// `internalWarn` `else` branch.
    fn warn<'a>(&self, message: &str, span: Option<&Span<'a>>, trace: Option<&Trace>) {
        self.inner.warn(message, span, trace)
    }

    /// Forwards debug messages untouched. Matches Dart's `debug`.
    fn debug<'a>(&self, message: &str, span: Option<&Span<'a>>) {
        self.inner.debug(message, span)
    }

    /// Processes a deprecation warning (Dart's `_handleDeprecation`, folded
    /// into `warn_deprecation` here since Rust has no `internalWarn` split).
    ///
    /// If `deprecation` is in `fatal_deprecations`, this shows an error
    /// (`Runtime` with span and stack, `Sass` with span alone, `Script`
    /// otherwise — matching Dart's `(span, trace)` switch). If it's a future
    /// deprecation that hasn't been opted into, a silenced deprecation, or a
    /// warning already emitted [`MAX_REPETITIONS`] times with
    /// `limit_repetition`, the warning is dropped. Otherwise it is passed on
    /// to `warn` — forwarding through `internalWarn` for
    /// `LoggerWithDeprecationType` inners, or with `deprecation: true` for
    /// plain user loggers in Dart; here `warn_deprecation` is already
    /// first-class so the call forwards directly.
    fn warn_deprecation<'a>(
        &self,
        message: &str,
        span: Option<&Span<'a>>,
        deprecation: &'static Deprecation,
        trace: Option<&Trace>,
    ) -> SassResult<()> {
        if deprecation.is_future && !self.future_deprecations.contains(deprecation) {
            return Ok(());
        }

        if self.fatal_deprecations.contains(deprecation) {
            let full_msg = format!(
                "{message}\n\nThis is only an error because you've set the {} \
                 deprecation to be fatal.\n\
                 Remove this setting if you need to keep using this feature.",
                deprecation.id
            );
            if let Some(span) = span {
                if let Some(trace) = trace {
                    return Err(Box::new(SassError::Runtime {
                        message: full_msg,
                        span: SourceSpanWithContext::from_span(span)?,
                        trace: trace.clone(),
                        cause: None,
                        loaded_urls: vec![],
                    }));
                }
                return Err(Box::new(SassError::Sass {
                    message: full_msg,
                    span: SourceSpanWithContext::from_span(span)?,
                    cause: None,
                    loaded_urls: vec![],
                }));
            }
            return Err(Box::new(SassError::Script {
                message: full_msg,
                argument_name: None,
            }));
        }

        if self.silence_deprecations.contains(deprecation) {
            return Ok(());
        }

        if self.limit_repetition {
            let mut counts = self.warning_counts.borrow_mut();
            let count = counts.entry(deprecation).or_insert(0);
            *count += 1;
            if *count > MAX_REPETITIONS {
                return Ok(());
            }
        }

        self.inner
            .warn_deprecation(message, span, deprecation, trace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::exception::Frame;
    use crate::common::exception::SassError;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::deprecation::TYPE_FUNCTION;
    use crate::deprecation::{CALL_STRING, CSS_FUNCTION_MIXIN, SLASH_DIV, USER_AUTHORED};
    use crate::io::VirtualIo;
    use crate::logger::{QuietLogger, StderrLogger, TrackingLogger};
    use bumpalo::Bump;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    fn capture_inner() -> (StderrLogger, Arc<Mutex<String>>) {
        let output = Arc::new(Mutex::new(String::new()));
        let out = output.clone();
        let l =
            StderrLogger::new_with_write_fn(false, false, Rc::new(VirtualIo::new()), move |s| {
                out.lock().unwrap().push_str(&s)
            });
        (l, output)
    }

    fn captured(out: &Arc<Mutex<String>>) -> String {
        out.lock().unwrap().clone()
    }

    #[test]
    fn test_dpl_silence() {
        let tl = TrackingLogger::new(QuietLogger);
        let dp = DeprecationProcessingLogger::new(tl, &[&SLASH_DIV], &[], &[], false);
        assert!(dp.warn_deprecation("test", None, &SLASH_DIV, None).is_ok());
        assert!(dp
            .warn_deprecation("test", None, &CALL_STRING, None)
            .is_ok());
    }

    #[test]
    fn test_dpl_fatal() {
        let dp = DeprecationProcessingLogger::new(QuietLogger, &[], &[&CALL_STRING], &[], false);
        assert!(dp
            .warn_deprecation("test", None, &CALL_STRING, None)
            .is_err());
    }

    #[test]
    fn test_dpl_fatal_error_message() {
        let dp = DeprecationProcessingLogger::new(QuietLogger, &[], &[&CALL_STRING], &[], false);
        let err = dp
            .warn_deprecation("test", None, &CALL_STRING, None)
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(
                "This is only an error because you've set the call-string deprecation to be fatal."
            ),
            "got {msg:?}"
        );
    }

    #[test]
    fn test_dpl_fatal_with_span() {
        let dp = DeprecationProcessingLogger::new(QuietLogger, &[], &[&CALL_STRING], &[], false);
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "body { }\n", None);
        let span = Span::File(FileSpan::new(Some(fs), 0, 8));
        let err = dp
            .warn_deprecation("test", Some(&span), &CALL_STRING, None)
            .unwrap_err();
        assert!(
            matches!(*err, SassError::Sass { .. }),
            "expected SassError::Sass, got {err:?}"
        );
    }

    #[test]
    fn test_dpl_fatal_with_span_and_stack() {
        let dp = DeprecationProcessingLogger::new(QuietLogger, &[], &[&CALL_STRING], &[], false);
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "body { }\n", None);
        let span = Span::File(FileSpan::new(Some(fs), 0, 8));
        let trace = Trace::new(vec![Frame {
            uri: None,
            line: 1,
            column: 1,
            member: "stack line".to_string(),
        }]);
        let err = dp
            .warn_deprecation("test", Some(&span), &CALL_STRING, Some(&trace))
            .unwrap_err();
        match *err {
            SassError::Runtime { trace, .. } => {
                assert_eq!(trace.len(), 1);
                assert_eq!(trace[0].member, "stack line");
            }
            _ => panic!("expected SassError::Runtime, got {err:?}"),
        }
    }

    #[test]
    fn test_dpl_fatal_preserves_structured_trace() {
        let dp = DeprecationProcessingLogger::new(QuietLogger, &[], &[&CALL_STRING], &[], false);
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "body { }\n", None);
        let span = Span::File(FileSpan::new(Some(fs), 0, 8));
        // A member containing a `file:line:col`-shaped substring would be
        // mangled by the deleted `trace_from_string` round-trip; the structured
        // trace must survive verbatim.
        let trace = Trace::new(vec![Frame {
            uri: None,
            line: 7,
            column: 9,
            member: "foo:12  bar".to_string(),
        }]);
        let err = dp
            .warn_deprecation("test", Some(&span), &CALL_STRING, Some(&trace))
            .unwrap_err();
        match *err {
            SassError::Runtime { trace, .. } => {
                assert_eq!(trace.len(), 1);
                assert_eq!(trace[0].member, "foo:12  bar");
                assert_eq!(trace[0].line, 7);
                assert_eq!(trace[0].column, 9);
            }
            _ => panic!("expected SassError::Runtime, got {err:?}"),
        }
    }

    #[test]
    fn test_dpl_future() {
        let dp = DeprecationProcessingLogger::new(QuietLogger, &[], &[], &[], false);
        assert!(dp
            .warn_deprecation("test", None, &CSS_FUNCTION_MIXIN, None)
            .is_ok());
        assert!(dp
            .warn_deprecation("test", None, &TYPE_FUNCTION, None)
            .is_ok());
    }

    #[test]
    fn test_dpl_limit_repetition() {
        let tl = TrackingLogger::new(QuietLogger);
        let dp = DeprecationProcessingLogger::new(tl, &[], &[], &[], true);
        for _ in 0..5 {
            assert!(dp
                .warn_deprecation("test", None, &CALL_STRING, None)
                .is_ok());
        }
        assert!(dp
            .warn_deprecation("test", None, &CALL_STRING, None)
            .is_ok());
    }

    #[test]
    fn test_dpl_pass_through() {
        let tl = TrackingLogger::new(QuietLogger);
        let dp = DeprecationProcessingLogger::new(tl, &[], &[], &[], false);
        dp.warn("test", None, None);
        dp.debug("test", None);
    }

    #[test]
    fn test_dpl_summarize() {
        let tl = TrackingLogger::new(QuietLogger);
        let dp = DeprecationProcessingLogger::new(tl, &[], &[], &[], true);
        for _ in 0..7 {
            let _ = dp.warn_deprecation("test", None, &CALL_STRING, None);
        }
        dp.summarize(false);
    }

    #[test]
    fn test_dpl_summarize_exact() {
        let (inner, out) = capture_inner();
        let dp = DeprecationProcessingLogger::new(inner, &[], &[], &[], true);
        for _ in 0..7 {
            let _ = dp.warn_deprecation("test", None, &CALL_STRING, None);
        }
        dp.summarize(false);
        let s = captured(&out);
        assert!(
            s.contains("2 repetitive deprecation warnings omitted."),
            "got {s:?}"
        );
        assert!(
            s.contains("Run in verbose mode to see all warnings."),
            "got {s:?}"
        );
    }

    #[test]
    fn test_dpl_summarize_js() {
        let (inner, out) = capture_inner();
        let dp = DeprecationProcessingLogger::new(inner, &[], &[], &[], true);
        for _ in 0..7 {
            let _ = dp.warn_deprecation("test", None, &CALL_STRING, None);
        }
        dp.summarize(true);
        let s = captured(&out);
        assert!(
            s.contains("2 repetitive deprecation warnings omitted."),
            "got {s:?}"
        );
        assert!(!s.contains("Run in verbose mode"), "got {s:?}");
    }

    #[test]
    fn test_dpl_validate_fatal_obsolete() {
        let (inner, out) = capture_inner();
        let dp = DeprecationProcessingLogger::new(inner, &[], &[&CSS_FUNCTION_MIXIN], &[], false);
        dp.validate();
        let s = captured(&out);
        assert!(
            s.contains(
                "css-function-mixin deprecation is obsolete, so does not need to be made fatal."
            ),
            "got {s:?}"
        );
    }

    #[test]
    fn test_dpl_validate_fatal_and_silenced() {
        let (inner, out) = capture_inner();
        let dp =
            DeprecationProcessingLogger::new(inner, &[&CALL_STRING], &[&CALL_STRING], &[], false);
        dp.validate();
        let s = captured(&out);
        assert!(
            s.contains("Ignoring setting to silence call-string deprecation, since it has also been made fatal."),
            "got {s:?}"
        );
    }

    #[test]
    fn test_dpl_validate_silence_user_authored() {
        let (inner, out) = capture_inner();
        let dp = DeprecationProcessingLogger::new(inner, &[&USER_AUTHORED], &[], &[], false);
        dp.validate();
        let s = captured(&out);
        assert!(
            s.contains("User-authored deprecations should not be silenced."),
            "got {s:?}"
        );
    }

    #[test]
    fn test_dpl_validate_silence_obsolete() {
        let (inner, out) = capture_inner();
        let dp = DeprecationProcessingLogger::new(inner, &[&CSS_FUNCTION_MIXIN], &[], &[], false);
        dp.validate();
        let s = captured(&out);
        assert!(
            s.contains("css-function-mixin deprecation is obsolete. If you were previously silencing it, your code may now behave in unexpected ways."),
            "got {s:?}"
        );
    }

    #[test]
    fn test_dpl_validate_future_not_future() {
        let (inner, out) = capture_inner();
        let dp = DeprecationProcessingLogger::new(inner, &[], &[], &[&CALL_STRING], false);
        dp.validate();
        let s = captured(&out);
        assert!(
            s.contains("call-string is not a future deprecation, so it does not need to be explicitly enabled."),
            "got {s:?}"
        );
    }

    #[test]
    fn test_dpl_validate() {
        let tl = TrackingLogger::new(QuietLogger);
        let dp = DeprecationProcessingLogger::new(
            tl,
            &[&CALL_STRING],
            &[&CALL_STRING],
            &[&CALL_STRING],
            false,
        );
        dp.validate();
    }
}
