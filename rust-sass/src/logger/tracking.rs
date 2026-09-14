// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/logger/tracking.dart
// go-source: go/sasslogger/logger_tracking.go

//! Usage-tracking decorator. Matches Dart: `TrackingLogger` in
//! `logger/tracking.dart`.

use std::cell::Cell;

use crate::common::exception::{SassResult, Trace};
use crate::common::span::Span;
use crate::deprecation::Deprecation;

use crate::logger::Logger;

/// A logger that wraps another logger and keeps track of when it is used.
///
/// Used to detect whether any warning/debug output was emitted (e.g. exit-code
/// decisions). Flags are `Cell<bool>` for `&self` mutation under
/// `Rc<dyn Logger>` sharing. Note: like Dart, `warn_deprecation` counts as a
/// warning, not as debug.
#[derive(Debug)]
pub struct TrackingLogger<I> {
    inner: I,
    emitted_warning: Cell<bool>,
    emitted_debug: Cell<bool>,
}

impl<I> TrackingLogger<I> {
    /// Wraps `inner`. Both emitted flags start `false`.
    pub fn new(inner: I) -> Self {
        TrackingLogger {
            inner,
            emitted_warning: Cell::new(false),
            emitted_debug: Cell::new(false),
        }
    }

    /// Whether [`Logger::warn`] (or `warn_deprecation`) has been called.
    /// Matches Dart's `emittedWarning`.
    pub fn emitted_warning(&self) -> bool {
        self.emitted_warning.get()
    }

    /// Whether [`Logger::debug`] has been called. Matches Dart's
    /// `emittedDebug`.
    pub fn emitted_debug(&self) -> bool {
        self.emitted_debug.get()
    }
}

impl<I: Logger> Logger for TrackingLogger<I> {
    /// Sets the warning flag, then forwards. Matches Dart's `warn`.
    fn warn<'a>(&self, message: &str, span: Option<&Span<'a>>, trace: Option<&Trace>) {
        self.emitted_warning.set(true);
        self.inner.warn(message, span, trace)
    }

    /// Sets the debug flag, then forwards. Matches Dart's `debug`.
    fn debug<'a>(&self, message: &str, span: Option<&Span<'a>>) {
        self.emitted_debug.set(true);
        self.inner.debug(message, span)
    }

    /// Sets the warning flag, then forwards. (Dart's `TrackingLogger` does not
    /// override deprecation handling — it predates `internalWarn`; counting
    /// deprecations as warnings is the Rust adaptation.)
    fn warn_deprecation<'a>(
        &self,
        message: &str,
        span: Option<&Span<'a>>,
        deprecation: &'static Deprecation,
        trace: Option<&Trace>,
    ) -> SassResult<()> {
        self.emitted_warning.set(true);
        self.inner
            .warn_deprecation(message, span, deprecation, trace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deprecation::CALL_STRING;
    use crate::logger::QuietLogger;

    #[test]
    fn test_tracking_logger_initial_state() {
        let tl = TrackingLogger::new(QuietLogger);
        assert!(!tl.emitted_warning());
        assert!(!tl.emitted_debug());
    }

    #[test]
    fn test_tracking_logger_warn() {
        let tl = TrackingLogger::new(QuietLogger);
        tl.warn("test", None, None);
        assert!(tl.emitted_warning());
        assert!(!tl.emitted_debug());
    }

    #[test]
    fn test_tracking_logger_debug() {
        let tl = TrackingLogger::new(QuietLogger);
        tl.debug("test", None);
        assert!(tl.emitted_debug());
        assert!(!tl.emitted_warning());
    }

    #[test]
    fn test_tracking_logger_warn_deprecation() {
        let tl = TrackingLogger::new(QuietLogger);
        assert!(tl.warn_deprecation("dep", None, &CALL_STRING, None).is_ok());
        assert!(tl.emitted_warning());
    }
}
