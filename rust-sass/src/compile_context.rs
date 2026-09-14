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

use std::rc::Rc;

/// Identity token for a single compilation.
///
/// `SassMixin`/`SassFunction` values capture it at creation;
/// `meta.call`/`meta.apply` assert it matches the current compilation before
/// invoking the wrapped callable. Compared by `Rc::ptr_eq` — matching Go's
/// `any` holding `&struct{}{}` (interface identity) and Dart's `Object()`
/// (object identity).
pub type CompileContext = Rc<()>;

/// Creates a fresh compilation identity token.
/// Matches Go: `compileContext: &struct{}{}` / Dart: `Object()`.
pub fn new_compile_context() -> CompileContext {
    Rc::new(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_same_context_is_ptr_eq() {
        let ctx = new_compile_context();
        let clone = ctx.clone();
        assert!(Rc::ptr_eq(&ctx, &clone));
    }

    #[test]
    fn test_different_contexts_are_not_ptr_eq() {
        let ctx1 = new_compile_context();
        let ctx2 = new_compile_context();
        assert!(!Rc::ptr_eq(&ctx1, &ctx2));
    }
}
