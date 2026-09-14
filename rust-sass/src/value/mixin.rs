// Copyright 2023 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/mixin.dart
// go-source: go/value/mixin.go

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::callable::Callable;
use crate::common::exception::{SassError, SassResult};
use crate::compile_context::CompileContext;
use crate::serialize::SerializeVisitor;

use crate::value::ValueVisitor;

/// A SassScript mixin reference.
///
/// A mixin reference captures a mixin from the local environment so that
/// it may be passed between modules.
#[derive(Clone, Debug)]
pub struct SassMixin<'parse> {
    /// The callable that this mixin invokes.
    /// Matches Dart: `final AsyncCallable callable`.
    ///
    /// Typed as async-capable so the value works with both the sync and
    /// async evaluators; in practice the sync evaluator requires a sync
    /// callable.
    pub callable: Callable<'parse, 'parse>,

    /// Tracks whether this value belongs to the current compilation.
    /// Matches Dart: `final Object? _compileContext` (None for values
    /// created outside a compilation, e.g. by host plugins).
    compile_context: Option<CompileContext>,
}

impl<'parse> SassMixin<'parse> {
    /// Matches Dart: `SassMixin(this.callable) : _compileContext = null`.
    pub fn new(callable: Callable<'parse, 'parse>) -> Self {
        SassMixin {
            callable,
            compile_context: None,
        }
    }

    /// Matches Dart: `SassMixin.withCompileContext`.
    pub fn with_compile_context(
        callable: Callable<'parse, 'parse>,
        compile_context: Option<CompileContext>,
    ) -> Self {
        SassMixin {
            callable,
            compile_context,
        }
    }

    /// Returns a valid CSS representation of `self`.
    ///
    /// Use [`to_display_string`](Self::to_display_string) instead to get a
    /// string representation even if this isn't valid CSS (mixin
    /// references never are).
    ///
    /// If `quote` is `false`, quoted strings are emitted without quotes
    /// (no effect on mixin references).
    pub fn to_css_string(&self, quote: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(quote, false);
        visitor.visit_mixin(self)?;
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
        visitor.visit_mixin(self)?;
        Ok(visitor.into_string())
    }

    /// Asserts that this mixin belongs to `compile_context` and returns it.
    /// Matches Dart: `SassMixin.assertCompileContext`.
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

    /// Matches Dart: `other is SassMixin && callable == other.callable`.
    /// Callable equality is `Rc` identity — object identity, like Dart's
    /// `==` on `AsyncCallable`. The compile context is ignored.
    pub fn equals(&self, other: &SassMixin<'parse>) -> bool {
        self.callable.identity_eq(&other.callable)
    }
}

impl<'parse> PartialEq for SassMixin<'parse> {
    fn eq(&self, other: &Self) -> bool {
        self.equals(other)
    }
}

impl<'parse> Eq for SassMixin<'parse> {}

impl<'parse> Hash for SassMixin<'parse> {
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
    fn test_mixin_equality() {
        let arena = Bump::new();
        // Equality is callable identity: the same callable Rc shared by two
        // mixin values makes them equal, matching Dart's
        // `callable == other.callable` (verified against dart-sass:
        // `meta.get-mixin("a") == meta.get-mixin("a")` → true).
        let callable = test_callable(&arena, "m");
        let m1 = SassMixin::new(callable);
        let m2 = SassMixin::new(callable);
        assert!(m1.equals(&m2));
        assert_eq!(m1, m2);

        // Two separately constructed callables are distinct objects.
        let m3 = SassMixin::new(test_callable(&arena, "m"));
        assert!(!m1.equals(&m3));
        assert_ne!(m1, m3);
    }

    #[test]
    fn test_mixin_clone_shares_identity() {
        let arena = Bump::new();
        let m1 = SassMixin::new(test_callable(&arena, "m"));
        let m2 = m1.clone();
        assert!(m1.equals(&m2));
    }

    #[test]
    fn test_mixin_hash_code() {
        let arena = Bump::new();
        let callable = test_callable(&arena, "m");
        let m1 = SassMixin::new(callable);
        let m2 = SassMixin::new(callable);
        assert_eq!(m1.hash_code(), m1.callable.identity_hash() as i32);
        assert_eq!(m1.hash_code(), m2.hash_code());

        let m3 = SassMixin::new(test_callable(&arena, "m"));
        assert_ne!(m1.hash_code(), m3.hash_code());
    }

    #[test]
    fn test_mixin_is_truthy() {
        let arena = Bump::new();
        let m = SassMixin::new(test_callable(&arena, "m"));
        assert!(m.is_truthy());
    }

    #[test]
    fn test_mixin_to_css_string() {
        let arena = Bump::new();
        // Mirrors Go: TestMixinToCssString — CSS mode is an error.
        let v = Value::new_with_arena(
            &arena,
            ValueKind::Mixin(SassMixin::new(test_callable(&arena, "my-mixin"))),
        );
        match *v.to_css_string(false).unwrap_err() {
            SassError::Script { message, .. } => {
                assert_eq!(message, "get-mixin(\"my-mixin\") isn't a valid CSS value.");
            }
            other => panic!("expected Script error, got {other:?}"),
        }
    }

    #[test]
    fn test_mixin_to_string() {
        let arena = Bump::new();
        // Mirrors Go: TestMixinString — the name comes from the callable
        // (Dart: mixin.callable.name).
        let v = Value::new_with_arena(
            &arena,
            ValueKind::Mixin(SassMixin::new(test_callable(&arena, "my-mixin"))),
        );
        assert_eq!(v.to_display_string().unwrap(), "get-mixin(\"my-mixin\")");
    }

    #[test]
    fn test_mixin_assert_compile_context_none_passes() {
        let arena = Bump::new();
        // A mixin without a compile context belongs to every compilation.
        let m = SassMixin::new(test_callable(&arena, "m"));
        let ctx = new_compile_context();
        assert!(m.assert_compile_context(&ctx).is_ok());
    }

    #[test]
    fn test_mixin_assert_compile_context_matching() {
        let arena = Bump::new();
        let ctx = new_compile_context();
        let m = SassMixin::with_compile_context(test_callable(&arena, "m"), Some(ctx.clone()));
        let same = m.assert_compile_context(&ctx).unwrap();
        assert!(same.equals(&m));
    }

    #[test]
    fn test_mixin_assert_compile_context_mismatch() {
        let arena = Bump::new();
        // Mirrors Go: TestMixinAssertCompileContextMismatch.
        let ctx1 = new_compile_context();
        let ctx2 = new_compile_context();
        let m = SassMixin::with_compile_context(test_callable(&arena, "m"), Some(ctx1));
        match *m.assert_compile_context(&ctx2).unwrap_err() {
            SassError::Script {
                message,
                argument_name,
            } => {
                assert_eq!(
                    message,
                    "get-mixin(\"m\") does not belong to current compilation."
                );
                assert_eq!(argument_name, None);
            }
            other => panic!("expected Script error, got {other:?}"),
        }
    }
}
