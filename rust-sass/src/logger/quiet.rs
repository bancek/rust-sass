// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/logger.dart (_QuietLogger)
// go-source: go/sasslogger/quiet_logger.go

//! The silent logger. Matches Dart: `_QuietLogger` in `logger.dart` (the
//! `Logger.quiet` factory target).

use crate::common::exception::{SassResult, Trace};
use crate::common::span::Span;
use crate::deprecation::Deprecation;

use crate::logger::Logger;

/// A logger that emits no messages.
///
/// Used by `--quiet` and by tests that must swallow output. Matches Dart's
/// `_QuietLogger`: every method is a no-op, and `warn_deprecation` always
/// succeeds (never fatal).
#[derive(Debug, Clone, Copy)]
pub struct QuietLogger;

impl Logger for QuietLogger {
    /// Discards the warning. Matches Dart's `_QuietLogger.warn` (empty body).
    fn warn<'a>(&self, _message: &str, _span: Option<&Span<'a>>, _trace: Option<&Trace>) {}

    /// Discards the debug message.
    fn debug<'a>(&self, _message: &str, _span: Option<&Span<'a>>) {}

    /// Discards the deprecation warning and reports success.
    fn warn_deprecation<'a>(
        &self,
        _message: &str,
        _span: Option<&Span<'a>>,
        _deprecation: &'static Deprecation,
        _trace: Option<&Trace>,
    ) -> SassResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deprecation::CALL_STRING;
    use crate::deprecation::SLASH_DIV;

    #[test]
    fn test_quiet_all_methods() {
        QuietLogger.warn("test", None, None);
        QuietLogger.debug("test", None);
        assert!(QuietLogger
            .warn_deprecation("test", None, &CALL_STRING, None)
            .is_ok());
    }

    #[test]
    fn test_quiet_warn_deprecation_no_error() {
        assert!(QuietLogger
            .warn_deprecation("msg", None, &SLASH_DIV, None)
            .is_ok());
    }
}
