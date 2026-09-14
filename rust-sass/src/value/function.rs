// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/function.dart
// go-source: go/value/function.go

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::callable::Callable;
use crate::common::exception::{SassError, SassResult};
use crate::compile_context::CompileContext;
use crate::serialize::SerializeVisitor;

use crate::value::ValueVisitor;

/// A SassScript function reference.
///
/// A function reference captures a function from the local environment so
/// that it may be passed between modules.
#[derive(Clone, Debug)]
pub struct SassFunction<'parse> {
    /// The callable that this function invokes.
    /// Matches Dart: `final AsyncCallable callable`.
    ///
    /// Typed as async-capable so the value works with both the sync and
    /// async evaluators; in practice the sync evaluator requires a sync
    /// callable.
    pub callable: Callable<'parse, 'parse>,

    /// Tracks whether this value belongs to the current compilation.
    /// Matches Dart: `final Object? _compileContext` (None for functions
    /// defined in plugins' host code).
    compile_context: Option<CompileContext>,
}

impl<'parse> SassFunction<'parse> {
    /// Matches Dart: `SassFunction(this.callable) : _compileContext = null`.
    pub fn new(callable: Callable<'parse, 'parse>) -> Self {
        SassFunction {
            callable,
            compile_context: None,
        }
    }

    /// Matches Dart: `SassFunction.withCompileContext`.
    pub fn with_compile_context(
        callable: Callable<'parse, 'parse>,
        compile_context: Option<CompileContext>,
    ) -> Self {
        SassFunction {
            callable,
            compile_context,
        }
    }

    /// Returns a valid CSS representation of `self`.
    ///
    /// Use [`to_display_string`](Self::to_display_string) instead to get a
    /// string representation even if this isn't valid CSS (function
    /// references never are).
    ///
    /// If `quote` is `false`, quoted strings are emitted without quotes
    /// (no effect on function references).
    pub fn to_css_string(&self, quote: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(quote, false);
        visitor.visit_function(self)?;
        Ok(visitor.into_string())
    }

    /// Returns a string representation of `self`.
    ///
    /// Note that this is equivalent to calling `inspect()` on the value, and
    /// thus won't reflect the user's output settings.
    /// [`to_css_string`](Self::to_css_string) should be used instead to
    /// convert `self` to CSS.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(true, true);
        visitor.visit_function(self)?;
        Ok(visitor.into_string())
    }

    /// Asserts that this function belongs to `compile_context` and returns it.
    /// Matches Dart: `SassFunction.assertCompileContext`.
    pub fn assert_compile_context(&self, compile_context: &CompileContext) -> SassResult<&Self> {
        if let Some(ref ctx) = self.compile_context {
            if !Rc::ptr_eq(ctx, compile_context) {
                let repr = self.to_display_string()?;
                return Err(Box::new(SassError::Script {
                    message: format!("{} does not belong to current compilation.", repr),
                    argument_name: None,
                }));
            }
        }
        Ok(self)
    }

    /// Whether the value counts as `true` in an `@if` statement and other
    /// contexts.
    pub fn is_truthy(&self) -> bool {
        true
    }

    /// Matches Dart: `hashCode => callable.hashCode` (identity hash) /
    /// Go: `reflect.ValueOf(ref).Pointer()`.
    pub fn hash_code(&self) -> i32 {
        self.callable.identity_hash() as i32
    }

    /// Matches Dart: `other is SassFunction && callable == other.callable`.
    /// Callable equality is `Rc` identity — object identity, like Dart's
    /// `==` on `AsyncCallable`. The compile context is ignored.
    pub fn equals(&self, other: &SassFunction<'parse>) -> bool {
        self.callable.identity_eq(&other.callable)
    }
}

impl<'parse> PartialEq for SassFunction<'parse> {
    fn eq(&self, other: &Self) -> bool {
        self.equals(other)
    }
}

impl<'parse> Eq for SassFunction<'parse> {}

impl<'parse> Hash for SassFunction<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.callable.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile_context::new_compile_context;
    use crate::functions::test_utils::test_callable;
    use crate::value::{Value, ValueKind};
    use bumpalo::Bump;

    #[test]
    fn test_function_equality() {
        let arena = Bump::new();
        // Equality is callable identity (Dart: callable == other.callable).
        let callable = test_callable(&arena, "f");
        let f1 = SassFunction::new(callable);
        let f2 = SassFunction::new(callable);
        assert!(f1.equals(&f2));
        assert_eq!(f1, f2);

        let f3 = SassFunction::new(test_callable(&arena, "f"));
        assert!(!f1.equals(&f3));
        assert_ne!(f1, f3);
    }

    #[test]
    fn test_function_clone_shares_identity() {
        let arena = Bump::new();
        let f1 = SassFunction::new(test_callable(&arena, "f"));
        let f2 = f1.clone();
        assert!(f1.equals(&f2));
    }

    #[test]
    fn test_function_hash_code() {
        let arena = Bump::new();
        let callable = test_callable(&arena, "f");
        let f1 = SassFunction::new(callable);
        let f2 = SassFunction::new(callable);
        assert_eq!(f1.hash_code(), f1.callable.identity_hash() as i32);
        assert_eq!(f1.hash_code(), f2.hash_code());

        let f3 = SassFunction::new(test_callable(&arena, "f"));
        assert_ne!(f1.hash_code(), f3.hash_code());
    }

    #[test]
    fn test_function_is_truthy() {
        let arena = Bump::new();
        let f = SassFunction::new(test_callable(&arena, "f"));
        assert!(f.is_truthy());
    }

    #[test]
    fn test_function_to_css_string() {
        let arena = Bump::new();
        // Mirrors Go: TestFunctionToCssString — CSS mode is an error.
        let v = Value::new_with_arena(
            &arena,
            ValueKind::Function(SassFunction::new(test_callable(&arena, "my-func"))),
        );
        match *v.to_css_string(false).unwrap_err() {
            SassError::Script { message, .. } => {
                assert_eq!(
                    message,
                    "get-function(\"my-func\") isn't a valid CSS value."
                );
            }
            other => panic!("expected Script error, got {other:?}"),
        }
    }

    #[test]
    fn test_function_to_string() {
        let arena = Bump::new();
        // Mirrors Go: TestFunctionString — the name comes from the callable
        // (Dart: function.callable.name).
        let v = Value::new_with_arena(
            &arena,
            ValueKind::Function(SassFunction::new(test_callable(&arena, "my-func"))),
        );
        assert_eq!(v.to_display_string().unwrap(), "get-function(\"my-func\")");
    }

    #[test]
    fn test_function_assert_compile_context_none_passes() {
        let arena = Bump::new();
        let f = SassFunction::new(test_callable(&arena, "f"));
        let ctx = new_compile_context();
        assert!(f.assert_compile_context(&ctx).is_ok());
    }

    #[test]
    fn test_function_assert_compile_context_matching() {
        let arena = Bump::new();
        let ctx = new_compile_context();
        let f = SassFunction::with_compile_context(test_callable(&arena, "f"), Some(ctx.clone()));
        let same = f.assert_compile_context(&ctx).unwrap();
        assert!(same.equals(&f));
    }

    #[test]
    fn test_function_assert_compile_context_mismatch() {
        let arena = Bump::new();
        // Mirrors Go: TestFunctionAssertCompileContextMismatch.
        let ctx1 = new_compile_context();
        let ctx2 = new_compile_context();
        let f = SassFunction::with_compile_context(test_callable(&arena, "f"), Some(ctx1));
        match *f.assert_compile_context(&ctx2).unwrap_err() {
            SassError::Script {
                message,
                argument_name,
            } => {
                assert_eq!(
                    message,
                    "get-function(\"f\") does not belong to current compilation."
                );
                assert_eq!(argument_name, None);
            }
            other => panic!("expected Script error, got {other:?}"),
        }
    }
}
