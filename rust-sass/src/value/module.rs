// Copyright 2026 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/module.dart
// go-source: go/value/module.go

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::common::exception::{SassError, SassResult};
use crate::compile_context::CompileContext;
use crate::module::Module;
use crate::serialize::SerializeVisitor;

use crate::value::{Value, ValueKind, ValueVisitor};

/// A SassScript module reference.
///
/// A module reference allows SassScript to reflect on and interact with
/// modules.
#[derive(Clone, Debug)]
pub struct SassModule<'parse> {
    /// The wrapped module.
    ///
    /// Dart types this as containing `AsyncCallable`s so it works with both
    /// evaluators; the sync evaluator requires a sync callable. Rust stores
    /// the unified [`Module`] handle.
    pub module: Module<'parse, 'parse>,

    /// The unique compile context for tracking whether this module belongs to
    /// the current compilation.
    /// Matches Dart: `final Object? _compileContext` (`None` outside a
    /// compilation).
    compile_context: Option<CompileContext>,
}

impl<'parse> SassModule<'parse> {
    /// Matches Dart: `SassModule(this.module) : _compileContext = null`.
    pub fn new(module: Module<'parse, 'parse>) -> Self {
        SassModule {
            module,
            compile_context: None,
        }
    }

    /// Matches Dart: `SassModule.withCompileContext`.
    pub fn with_compile_context(
        module: Module<'parse, 'parse>,
        compile_context: Option<CompileContext>,
    ) -> Self {
        SassModule {
            module,
            compile_context,
        }
    }

    /// Returns a valid CSS representation of `self`.
    ///
    /// Module references are never valid CSS; this always errors. Use
    /// [`to_display_string`](Self::to_display_string) for the inspect form.
    pub fn to_css_string(&self, quote: bool) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(quote, false);
        visitor.visit_module(self)?;
        Ok(visitor.into_string())
    }

    /// Returns a string representation of `self`.
    ///
    /// This is `meta.inspect()`'s form (`get-module("namespace")`).
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut visitor = SerializeVisitor::new_plain(true, true);
        visitor.visit_module(self)?;
        Ok(visitor.into_string())
    }

    /// Asserts that this module belongs to `compile_context` and returns it.
    ///
    /// Matches Dart: `SassModule.assertCompileContext`.
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

    /// Matches Dart: `hashCode => module.hashCode` (identity hash).
    pub fn hash_code(&self) -> i32 {
        self.module.hash_code() as i32
    }

    /// Matches Dart: `other is SassModule && _compileContext ==
    /// other._compileContext && module == other.module`.
    pub fn equals(&self, other: &SassModule<'parse>) -> bool {
        let same_context = match (&self.compile_context, &other.compile_context) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        same_context && self.module == other.module
    }

    /// Matches Dart: `Value.plus` override on `SassModule`.
    pub fn plus(&self, other: &ValueKind<'parse>) -> SassResult<Value<'parse>> {
        undefined_binary_operation(self, other, "+")
    }

    /// Matches Dart: `Value.minus` override on `SassModule`.
    pub fn minus(&self, other: &ValueKind<'parse>) -> SassResult<Value<'parse>> {
        undefined_binary_operation(self, other, "-")
    }

    /// Matches Dart: `Value.dividedBy` override on `SassModule`.
    pub fn divided_by(&self, other: &ValueKind<'parse>) -> SassResult<Value<'parse>> {
        undefined_binary_operation(self, other, "/")
    }

    /// Matches Dart: `Value.unaryPlus` override on `SassModule`.
    pub fn unary_plus(&self) -> SassResult<Value<'parse>> {
        undefined_unary_operation(self, "+", false)
    }

    /// Matches Dart: `Value.unaryMinus` override on `SassModule`.
    pub fn unary_minus(&self) -> SassResult<Value<'parse>> {
        undefined_unary_operation(self, "-", true)
    }

    /// Matches Dart: `Value.unaryDivide` override on `SassModule`.
    pub fn unary_divide(&self) -> SassResult<Value<'parse>> {
        undefined_unary_operation(self, "/", false)
    }
}

impl<'parse> PartialEq for SassModule<'parse> {
    fn eq(&self, other: &Self) -> bool {
        self.equals(other)
    }
}

impl Eq for SassModule<'_> {}

impl Hash for SassModule<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.module.hash(state);
    }
}

fn undefined_binary_operation<'parse>(
    left: &SassModule<'parse>,
    right: &ValueKind<'parse>,
    operator: &str,
) -> SassResult<Value<'parse>> {
    let left_str = left.to_display_string()?;
    let right_str = right.to_display_string()?;
    Err(Box::new(SassError::Script {
        message: format!("Undefined operation \"{left_str} {operator} {right_str}\"."),
        argument_name: None,
    }))
}

fn undefined_unary_operation<'parse>(
    value: &SassModule<'parse>,
    operator: &str,
    parenthesize_operand: bool,
) -> SassResult<Value<'parse>> {
    let repr = value.to_display_string()?;
    let operand = if parenthesize_operand {
        format!("({repr})")
    } else {
        repr
    };
    Err(Box::new(SassError::Script {
        message: format!("Undefined operation \"{operator}{operand}\"."),
        argument_name: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::{BuiltInModule, ModuleKind};
    use bumpalo::Bump;
    use indexmap::IndexMap;

    fn test_module<'parse>(arena: &'parse Bump, name: &str) -> Module<'parse, 'parse> {
        Module::new(
            arena,
            ModuleKind::BuiltIn(BuiltInModule::new(
                arena,
                name.to_string(),
                &[],
                &[],
                IndexMap::new(),
            )),
        )
    }

    #[test]
    fn test_module_equality_is_identity() {
        let arena = Bump::new();
        let a = test_module(&arena, "a");
        let b = test_module(&arena, "b");
        assert!(SassModule::new(a).equals(&SassModule::new(a)));
        assert!(!SassModule::new(a).equals(&SassModule::new(b)));
    }

    #[test]
    fn test_module_inspect_and_css_serialization() {
        let arena = Bump::new();
        let value = SassModule::new(test_module(&arena, "meta"));
        assert_eq!(value.to_display_string().unwrap(), "get-module(\"meta\")");
        let err = value.to_css_string(true).unwrap_err();
        assert!(
            err.to_string()
                .contains("get-module(\"meta\") isn't a valid CSS value."),
            "{err}"
        );
    }

    #[test]
    fn test_module_invalid_namespace_serializes_without_argument() {
        let arena = Bump::new();
        let value = SassModule::new(test_module(&arena, "1"));
        assert_eq!(value.to_display_string().unwrap(), "get-module()");
    }

    #[test]
    fn test_module_unary_minus_parenthesizes_operand() {
        let arena = Bump::new();
        let value = SassModule::new(test_module(&arena, "meta"));
        let err = value.unary_minus().unwrap_err();
        assert_eq!(
            err.to_string(),
            "Undefined operation \"-(get-module(\"meta\"))\"."
        );
    }

    #[test]
    fn test_module_compile_context() {
        let arena = Bump::new();
        let module = test_module(&arena, "a");
        let ctx = crate::compile_context::new_compile_context();
        let value = SassModule::with_compile_context(module, Some(ctx.clone()));
        assert!(value.assert_compile_context(&ctx).is_ok());
        let other = crate::compile_context::new_compile_context();
        let err = value.assert_compile_context(&other).unwrap_err();
        assert!(err
            .to_string()
            .contains("does not belong to current compilation"));
    }
}
