// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/logger.dart
// go-source: go/sasslogger/logger.go

//! Loggers that surface warnings, debug output, and deprecation warnings.
//!
//! Matches Dart: `logger.dart` (the [`Logger`] interface,
//! `LoggerWithDeprecationType`, the `WarnForDeprecation` extension,
//! `_QuietLogger`) plus the `logger/*.dart` implementations re-exported here.
//!
//! The Rust [`Logger`] trait folds Dart's three pieces into one surface:
//! `warn`/`debug` plus a first-class `warn_deprecation` taking a
//! `&'static Deprecation` (Dart threads the `Deprecation` object through the
//! `@internal` `LoggerWithDeprecationType.internalWarn` instead; the
//! `WarnForDeprecation` extension's future-deprecation gate and
//! user-logger fallback live in [`DeprecationProcessingLogger`] and the
//! embedded/wasm loggers). `{@category Compile}` is dropped per rule 1.
//! `Logger.quiet`/`Logger.stderr`/`Logger.defaultLogger` factories map to
//! [`QuietLogger`], [`StderrLogger`], and [`new_default_logger`].

pub mod default;
mod deprecation_processing;
pub mod quiet;
pub mod stderr;
#[cfg(test)]
pub mod test_utils;
mod tracking;
pub mod warn_logger;

use std::fmt;
use std::rc::Rc;

use crate::common::exception::{SassResult, Trace};
use crate::common::span::Span;
use crate::deprecation::Deprecation;

pub use default::new_default_logger;
pub use deprecation_processing::DeprecationProcessingLogger;
pub use quiet::QuietLogger;
pub use stderr::StderrLogger;
pub use tracking::TrackingLogger;
pub use warn_logger::{BufferedWarnLogger, NoOpWarnLogger, WarnLogger};

/// An interface for loggers that print messages produced by Sass stylesheets.
///
/// This may be implemented by user code. Single-threaded: no `Send`/`Sync`,
/// shared via `Rc<dyn Logger>`. Span-free methods take a generic `<'a>` span
/// lifetime (not `'parse`) so the trait stays usable in `EvalConfig` without
/// threading the arena through every call site.
///
/// Matches Dart: `Logger` in `logger.dart`. The `deprecation: bool` flag on
/// Dart's `warn` is replaced by the first-class [`Logger::warn_deprecation`];
/// `debug` takes `Option<&Span>` (Dart requires a span, passing is the
/// caller's job).
pub trait Logger: fmt::Debug {
    /// Emits a warning with `message`.
    ///
    /// If `span` is passed, it is the Sass source location that generated the
    /// warning; `trace` is the Sass stack trace at issue time, carried
    /// structurally and rendered by the logger only when it needs a string.
    /// Matches Dart's `Logger.warn` (minus the `deprecation` flag, which is
    /// [`Logger::warn_deprecation`]).
    fn warn<'a>(&self, message: &str, span: Option<&Span<'a>>, trace: Option<&Trace>);

    /// Emits a debugging message associated with `span`.
    fn debug<'a>(&self, message: &str, span: Option<&Span<'a>>);

    /// Emits a deprecation warning of type `deprecation`.
    ///
    /// `trace` is the Sass stack trace at issue time. Returns `Err` when the
    /// deprecation is configured fatal (see
    /// [`DeprecationProcessingLogger`]). Matches Dart's
    /// `LoggerWithDeprecationType.internalWarn` / `warnForDeprecation`.
    fn warn_deprecation<'a>(
        &self,
        message: &str,
        span: Option<&Span<'a>>,
        deprecation: &'static Deprecation,
        trace: Option<&Trace>,
    ) -> SassResult<()>;
}

/// Forwards [`Logger`] calls to the wrapped logger.
///
/// This lets `DeprecationProcessingLogger<Rc<dyn Logger>>` wrap an arbitrary
/// user-supplied logger (e.g. the embedded `EmbeddedLogger`), matching Dart,
/// which always wraps the provided logger in a `DeprecationProcessingLogger`.
/// Rust-only: Dart needs no such impl (interface dispatch is implicit).
impl Logger for Rc<dyn Logger> {
    fn warn<'a>(&self, message: &str, span: Option<&Span<'a>>, trace: Option<&Trace>) {
        (**self).warn(message, span, trace)
    }

    fn debug<'a>(&self, message: &str, span: Option<&Span<'a>>) {
        (**self).debug(message, span)
    }

    fn warn_deprecation<'a>(
        &self,
        message: &str,
        span: Option<&Span<'a>>,
        deprecation: &'static Deprecation,
        trace: Option<&Trace>,
    ) -> SassResult<()> {
        (**self).warn_deprecation(message, span, deprecation, trace)
    }
}
