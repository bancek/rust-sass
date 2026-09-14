// Copyright 2021 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/declaration.dart
// go-source: go/value/sass_declaration.go

use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::configured_variable::ConfiguredVariable;
use crate::ast::sass::parameter::Parameter;
use crate::ast::sass::statement::function_rule::FunctionRule;
use crate::ast::sass::statement::mixin_rule::MixinRule;
use crate::ast::sass::statement::variable_declaration::VariableDeclaration;

/// A node that declares a Sass member.
///
/// Dart's `SassDeclaration` is a sealed interface implemented by parameters,
/// configured variables, variable declarations, function rules, and mixin
/// rules; the Rust port is a closed enum over those five (structural
/// fidelity, dead code — see `ref/ast.md`).
#[derive(Clone, Debug)]
pub enum SassDeclaration<'parse> {
    Parameter(Parameter<'parse>),
    ConfiguredVariable(ConfiguredVariable<'parse>),
    VariableDeclaration(VariableDeclaration<'parse>),
    FunctionRule(FunctionRule<'parse>),
    MixinRule(MixinRule<'parse>),
}

impl<'parse> SassDeclaration<'parse> {
    /// The name of the declaration, with underscores converted to hyphens.
    ///
    /// This does not include the `$` for variables.
    pub fn name(&self) -> &str {
        match self {
            SassDeclaration::Parameter(p) => &p.name,
            SassDeclaration::ConfiguredVariable(cv) => &cv.name,
            SassDeclaration::VariableDeclaration(vd) => &vd.name,
            SassDeclaration::FunctionRule(fr) => &fr.name,
            SassDeclaration::MixinRule(mr) => &mr.name,
        }
    }

    /// The span containing this declaration's name.
    ///
    /// This includes the `$` for variables.
    pub fn name_span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            SassDeclaration::Parameter(p) => p.name_span(),
            SassDeclaration::ConfiguredVariable(cv) => cv.name_span(),
            SassDeclaration::VariableDeclaration(vd) => vd.name_span(),
            SassDeclaration::FunctionRule(fr) => fr.name_span(),
            SassDeclaration::MixinRule(mr) => mr.name_span(),
        }
    }
}
