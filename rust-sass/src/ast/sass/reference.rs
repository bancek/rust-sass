// Copyright 2021 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/reference.dart
// go-source: go/value/sass_reference.go

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression_function::FunctionExpression;
use crate::ast::sass::expression_variable::VariableExpression;
use crate::ast::sass::statement::include_rule::IncludeRule;

/// A node that references a Sass member.
///
/// Dart's `SassReference` is an abstract interface; the Rust port is a closed
/// enum over variable and function references plus `@include` (structural
/// fidelity, dead code — see `ref/ast.md`).
#[derive(Clone, Debug)]
pub enum SassReference<'parse> {
    VariableExpression(VariableExpression<'parse>),
    FunctionExpression(FunctionExpression<'parse>),
    IncludeRule(IncludeRule<'parse>),
}

impl<'parse> SassReference<'parse> {
    /// The name of the member being referenced, with underscores converted to
    /// hyphens.
    ///
    /// This does not include the `$` for variables.
    pub fn name(&self) -> &str {
        match self {
            SassReference::VariableExpression(ve) => &ve.name,
            SassReference::FunctionExpression(fe) => &fe.name,
            SassReference::IncludeRule(ir) => &ir.name,
        }
    }

    /// The span containing this reference's name.
    ///
    /// For variables, this includes the `$`.
    pub fn name_span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            SassReference::VariableExpression(ve) => ve.name_span(),
            SassReference::FunctionExpression(fe) => fe.name_span(),
            SassReference::IncludeRule(ir) => ir.name_span(),
        }
    }

    /// The namespace of the member being referenced, or `None` if it's
    /// referenced without a namespace.
    pub fn namespace(&self) -> Option<&str> {
        match self {
            SassReference::VariableExpression(ve) => ve.namespace.as_deref(),
            SassReference::FunctionExpression(fe) => fe.namespace.as_deref(),
            SassReference::IncludeRule(ir) => ir.namespace.as_deref(),
        }
    }

    /// The span containing this reference's namespace, or `None` if
    /// [`namespace`](Self::namespace) is `None`.
    pub fn namespace_span(&self) -> SassResult<Option<FileSpan<'parse>>> {
        match self {
            SassReference::VariableExpression(ve) => ve.namespace_span(),
            SassReference::FunctionExpression(fe) => fe.namespace_span(),
            SassReference::IncludeRule(ir) => ir.namespace_span(),
        }
    }
}

impl<'parse> AstNode<'parse> for SassReference<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            SassReference::VariableExpression(ve) => ve.span(),
            SassReference::FunctionExpression(fe) => fe.span(),
            SassReference::IncludeRule(ir) => ir.span(),
        }
    }
}
