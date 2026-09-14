// Copyright 2017 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/importer/filesystem.dart (warnForDeprecation via evaluation context)
//   + lib/src/evaluation_context.dart (warnForDeprecation, EvaluationContextLogger rationale)
// go-source: go/eval/import_cache.go (warnFn on FilesystemImporter)

//! Deferred deprecation warnings for code that cannot reach the evaluator.
//!
//! Matches Dart: the zone-based ambient `EvaluationContext.current` letting
//! utility code (e.g. `FilesystemImporter.canonicalize`'s `FS_IMPORTER_CWD`
//! warning) call `warnForDeprecation` without an eval handle. Rust has no
//! ambient zone (see `ref/warn-logger.md` why stored closures and borrowed
//! `&mut EvalState` parameters both fail), so importers and `value/` helpers
//! take a `&dyn WarnLogger`, buffer into a [`BufferedWarnLogger`], and the
//! caller flushes each entry through the eval warn pipeline (dedup,
//! `quiet_deps`, span resolution, stack trace) afterward.

use std::cell::RefCell;

use bumpalo::Bump;

use crate::common::file_span::FileSpan;
use crate::deprecation::Deprecation;

/// A logger for deprecation warnings emitted during import canonicalization.
///
/// Importers record warnings during `canonicalize()`. The caller drains them
/// afterward and routes each through the evaluator's warn pipeline (dedup,
/// `quiet_deps`, span resolution, stack trace). `&self` (with interior
/// mutability in implementors) replaces Dart's ambient zone reachability.
pub trait WarnLogger<'parse> {
    /// Records a deprecation warning of type `deprecation` with `message`.
    /// `span` is the importer's local span, resolved by the caller at flush
    /// time. Returns `()` — no error propagation, no eval dependency.
    fn warn_deprecation(
        &self,
        message: &str,
        deprecation: &'static Deprecation,
        span: Option<FileSpan<'parse>>,
    );
}

/// Collects deprecation warnings in a buffer for deferred processing.
///
/// [`BufferedWarnLogger::drain`] returns all recorded warnings so the
/// caller can route them through the evaluator's deprecation warn pipeline.
/// The buffer is arena-allocated (`&'parse RefCell<..>`, `Copy`-shareable)
/// so importers behind `&self` can record without owning storage.
#[derive(Clone)]
pub struct BufferedWarnLogger<'parse> {
    pending: &'parse RefCell<Vec<(String, &'static Deprecation, Option<FileSpan<'parse>>)>>,
}

impl<'parse> BufferedWarnLogger<'parse> {
    /// Creates an empty buffer in `arena`. The `'compile: 'parse` bound ties
    /// the buffer to the compile arena's lifetime.
    pub fn new<'compile: 'parse>(arena: &'compile Bump) -> Self {
        Self {
            pending: arena.alloc(RefCell::new(Vec::new())),
        }
    }

    /// Drains and returns all recorded warnings in order, leaving the buffer
    /// empty. The caller routes each entry through `eval::warn`.
    pub fn drain(&self) -> Vec<(String, &'static Deprecation, Option<FileSpan<'parse>>)> {
        self.pending.borrow_mut().drain(..).collect()
    }
}

impl<'parse> WarnLogger<'parse> for BufferedWarnLogger<'parse> {
    fn warn_deprecation(
        &self,
        message: &str,
        deprecation: &'static Deprecation,
        span: Option<FileSpan<'parse>>,
    ) {
        self.pending
            .borrow_mut()
            .push((message.to_string(), deprecation, span));
    }
}

/// A no-op [`WarnLogger`] that silently discards all warnings.
///
/// Used when no eval context is available to flush into (e.g. speculative
/// canonicalization probes). Has no Dart counterpart as a named type —
/// Dart's `warnForDeprecation` outside a zone falls back to
/// `Logger.defaultLogger` instead of discarding.
pub struct NoOpWarnLogger;

impl<'parse> WarnLogger<'parse> for NoOpWarnLogger {
    fn warn_deprecation(
        &self,
        _message: &str,
        _deprecation: &'static Deprecation,
        _span: Option<FileSpan<'parse>>,
    ) {
    }
}
