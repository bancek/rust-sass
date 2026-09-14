// Copyright 2021 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/dependency.dart
// go-source: go/value/sass_dependency.go

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::url::SassUrl;

use crate::ast::sass::dynamic_import::DynamicImport;
use crate::ast::sass::statement::forward_rule::ForwardRule;
use crate::ast::sass::statement::use_rule::UseRule;

/// A load dependency: a `@use` rule, a `@forward` rule, or a dynamic
/// (Sass-loading `@import`) import.
///
/// Dart's `SassDependency` is a sealed interface; the Rust port is a closed
/// enum over the three (structural fidelity, dead code — see `ref/ast.md`).
// Variant sizes mirror Dart's subclasses; boxing would churn every
// constructor/match for no observable change.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum SassDependency<'parse> {
    DynamicImport(DynamicImport<'parse>),
    UseRule(UseRule<'parse>),
    ForwardRule(ForwardRule<'parse>),
}

impl<'parse> SassDependency<'parse> {
    /// The URL of the dependency this rule loads.
    pub fn url(&self) -> SassUrl {
        match self {
            SassDependency::DynamicImport(di) => di.url(),
            SassDependency::UseRule(ur) => ur.url.clone(),
            SassDependency::ForwardRule(fr) => fr.url.clone(),
        }
    }

    /// The span of the URL for this dependency, including the quotes.
    pub fn url_span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            SassDependency::DynamicImport(di) => di.url_span(),
            SassDependency::UseRule(ur) => ur.url_span(),
            SassDependency::ForwardRule(fr) => fr.url_span(),
        }
    }
}

impl<'parse> AstNode<'parse> for SassDependency<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            SassDependency::DynamicImport(di) => di.span(),
            SassDependency::UseRule(ur) => ur.span(),
            SassDependency::ForwardRule(fr) => fr.span(),
        }
    }
}
