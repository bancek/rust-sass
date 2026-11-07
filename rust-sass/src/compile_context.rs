// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/evaluate.dart (final Object _compileContext = Object())
// go-source: go/eval/evaluate.go (compileContext: &struct{}{})

//! The identity token for a single compilation (Dart's `EvaluateVisitor._compileContext`).
//!
//! A fresh token is minted per `evaluate()` call and captured by every
//! `SassFunction`/`SassMixin` value created during it, so `meta.call`/
//! `meta.apply` can reject values leaking across compilations.

use std::sync::atomic::{AtomicU64, Ordering};

/// Identity token for a single compilation.
///
/// `SassMixin`/`SassFunction`/`SassModule` values capture it at creation;
/// `meta.call`/`meta.apply` assert it matches the current compilation before
/// invoking the wrapped callable. A monotonically increasing id gives the same
/// identity semantics as Go's `&struct{}{}` / Dart's `Object()` while staying
/// a plain `Copy` value: the token may be stored in the arena, which never
/// runs destructors, so an `Rc` would leak its allocation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CompileContext(u64);

/// Process-global sequence; ids are never reused, unlike heap addresses.
static NEXT_COMPILE_CONTEXT: AtomicU64 = AtomicU64::new(1);

/// Creates a fresh compilation identity token.
/// Matches Go: `compileContext: &struct{}{}` / Dart: `Object()`.
pub fn new_compile_context() -> CompileContext {
    CompileContext(NEXT_COMPILE_CONTEXT.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_same_context_is_equal() {
        let ctx = new_compile_context();
        let copy = ctx;
        assert_eq!(ctx, copy);
    }

    #[test]
    fn test_different_contexts_are_not_equal() {
        let ctx1 = new_compile_context();
        let ctx2 = new_compile_context();
        assert_ne!(ctx1, ctx2);
    }
}
