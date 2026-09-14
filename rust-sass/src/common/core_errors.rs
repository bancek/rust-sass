// Copyright (c) 2012, the Dart project authors.  Please see the AUTHORS file
// for details. All rights reserved. Use of this source code is governed by a
// BSD-style license that can be found in the LICENSE file.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: (standard) dart:core
// go-source: go/sasscommon/core_errors.go

use thiserror::Error;

/// An invalid argument was passed to a function.
///
/// Matches Dart: `ArgumentError` (`dart:core`) — `name` is the argument name
/// when known. Surfaced through [`SpanError`](super::span_error::SpanError)
/// at span-utility boundaries rather than raised directly.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct ArgumentError {
    pub name: Option<String>,
    pub message: String,
}

/// A value is outside a valid range.
///
/// Matches Dart: `RangeError` (`dart:core`) — `name` is the parameter name.
/// Surfaced through [`SpanError`](super::span_error::SpanError) at
/// span-utility boundaries rather than raised directly.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct RangeError {
    pub name: String,
    pub message: String,
}

/// An operation invalid for the current state.
///
/// Matches Dart: `StateError` (`dart:core`).
#[derive(Debug, Error)]
#[error("{message}")]
pub struct StateError {
    pub message: String,
}

/// An unsupported operation was attempted.
///
/// Matches Dart: `UnsupportedError` (`dart:core`).
#[derive(Debug, Error)]
#[error("{message}")]
pub struct UnsupportedError {
    pub message: String,
}
