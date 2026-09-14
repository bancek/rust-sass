// dart-source: lib/src/exception.dart (error translation at span-utility boundaries) + (standard) dart:core (ArgumentError/RangeError conversions)
// go-source: go/sasscommon/core_errors.go (errors) + go/sasscommon/exception.go (SassError wrapping)

use crate::common::core_errors::{ArgumentError, RangeError};
use crate::common::exception::SassError;
use std::fmt::Display;
use std::fmt::Formatter;

/// The internal error from span utility methods.
///
/// Matches Dart: the `ArgumentError`/`RangeError` that span algebra
/// (`span.dart`, `span_with_context.dart`) throws, plus the `SassError` that
/// lazy-span builders produce. `Argument` is a URL mismatch, containment
/// failure, or wrong order; `Range` is a subspan out of bounds; `Sass` comes
/// from a lazy builder or a scan error. It is `Debug`-only (matched on and
/// converted, never displayed). At evaluator/parser boundaries
/// `Argument`/`Range` become `SassError::Runtime` with the current stack
/// span and `Sass` passes through; `From<SpanError> for SassError` maps them
/// to `SassError::Script` or the pass-through.
#[derive(Debug)]
pub enum SpanError {
    Argument(String),
    Range(String),
    Sass(Box<SassError>),
}

impl From<ArgumentError> for SpanError {
    fn from(e: ArgumentError) -> Self {
        SpanError::Argument(e.message)
    }
}

impl From<RangeError> for SpanError {
    fn from(e: RangeError) -> Self {
        SpanError::Range(e.message)
    }
}

impl From<SassError> for SpanError {
    fn from(e: SassError) -> Self {
        SpanError::Sass(Box::new(e))
    }
}

impl From<Box<SassError>> for SpanError {
    fn from(e: Box<SassError>) -> Self {
        SpanError::Sass(e)
    }
}

impl Display for SpanError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SpanError::Argument(msg) => write!(f, "{msg}"),
            SpanError::Range(msg) => write!(f, "{msg}"),
            SpanError::Sass(e) => write!(f, "{e}"),
        }
    }
}
