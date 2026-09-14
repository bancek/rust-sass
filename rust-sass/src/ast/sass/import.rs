// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/import.dart
// go-source: go/value/sass_import.go

use std::fmt;

use crate::ast::sass::dynamic_import::DynamicImport;
use crate::ast::sass::static_import::StaticImport;

/// An import: either a Sass-loading dynamic import or a plain-CSS static
/// `@import`.
///
/// Dart's `Import` is an abstract interface; the Rust port is a closed enum
/// over the two (only `Import` of the wrapper enums is actually used —
/// see `ref/ast.md`).
#[derive(Clone, Debug)]
pub enum Import<'parse> {
    /// An import that will load a Sass file at runtime.
    Dynamic(DynamicImport<'parse>),
    /// An import that produces a plain CSS `@import` rule.
    Static(StaticImport<'parse>),
}

impl<'parse> fmt::Display for Import<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Import::Dynamic(d) => write!(f, "{d}"),
            Import::Static(s) => write!(f, "{s}"),
        }
    }
}
