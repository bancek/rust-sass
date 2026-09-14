// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/callable_invocation.dart
// go-source: go/value/sass_callable_invocation.go

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::expression_function::FunctionExpression;
use crate::ast::sass::expression_interpolated_function::InterpolatedFunctionExpression;
use crate::ast::sass::expression_legacy_if::LegacyIfExpression;
use crate::ast::sass::statement::include_rule::IncludeRule;

/// An invocation of a callable (a function or mixin).
///
/// Dart's `CallableInvocation` is a sealed abstract base class; the Rust port
/// is a closed enum over the four invokable node types (structural fidelity,
/// one dispatch site — see `ref/ast.md`).
#[derive(Clone, Debug)]
pub enum CallableInvocation<'parse> {
    FunctionExpression(FunctionExpression<'parse>),
    InterpolatedFunctionExpression(InterpolatedFunctionExpression<'parse>),
    LegacyIfExpression(LegacyIfExpression<'parse>),
    IncludeRule(IncludeRule<'parse>),
}

impl<'parse> CallableInvocation<'parse> {
    /// The arguments passed to the callable.
    pub fn arguments(&self) -> &ArgumentList<'parse> {
        match self {
            CallableInvocation::FunctionExpression(fe) => &fe.arguments,
            CallableInvocation::InterpolatedFunctionExpression(ife) => &ife.arguments,
            CallableInvocation::LegacyIfExpression(le) => &le.arguments,
            CallableInvocation::IncludeRule(ir) => &ir.arguments,
        }
    }
}

impl<'parse> AstNode<'parse> for CallableInvocation<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            CallableInvocation::FunctionExpression(fe) => fe.span(),
            CallableInvocation::InterpolatedFunctionExpression(ife) => ife.span(),
            CallableInvocation::LegacyIfExpression(le) => le.span(),
            CallableInvocation::IncludeRule(ir) => ir.span(),
        }
    }
}
