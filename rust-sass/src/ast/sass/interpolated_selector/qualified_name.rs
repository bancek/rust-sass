// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolated_selector/qualified_name.dart
// go-source: go/value/sass_interpolated_selector_qualified_name.go

use std::fmt;
use std::fmt::Write;

use crate::ast::sass::interpolation::Interpolation;
use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

// Note: Dart's doc header names the complex-selector component by mistake;
// this type is the namespace-qualified name shared by the type and attribute
// selectors.

/// A namespace-qualified name, still containing interpolation at parse time.
///
/// Unlike the resolved qualified name, this is parsed during the initial
/// stylesheet parse.
#[derive(Clone, Debug)]
pub struct InterpolatedQualifiedName<'parse> {
    /// The identifier name.
    pub name: Interpolation<'parse>,
    /// The source span covering this name.
    pub span: FileSpan<'parse>,
    /// The namespace name, if any.
    pub namespace: Option<Interpolation<'parse>>,
}

impl<'parse> InterpolatedQualifiedName<'parse> {
    /// Creates a qualified name with an optional namespace.
    pub fn new(
        name: Interpolation<'parse>,
        span: FileSpan<'parse>,
        namespace: Option<Interpolation<'parse>>,
    ) -> Self {
        InterpolatedQualifiedName {
            name,
            span,
            namespace,
        }
    }

    /// Renders this name as `namespace|name`, or the bare name when there is
    /// no namespace.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        if let Some(ref namespace) = self.namespace {
            write!(buf, "{}|{}", namespace, self.name).unwrap();
        } else {
            write!(buf, "{}", self.name).unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> AstNode<'parse> for InterpolatedQualifiedName<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for InterpolatedQualifiedName<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    #[test]
    fn test_qualified_name_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "div");
        let name = Interpolation::plain("div".into(), span);
        let qn = InterpolatedQualifiedName::new(name, span, None);
        assert!(qn.namespace.is_none());
    }

    #[test]
    fn test_qualified_name_with_namespace() {
        let arena = Bump::new();
        let span = make_span(&arena, "svg|div");
        let name = Interpolation::plain("div".into(), span);
        let ns_span = make_span(&arena, "svg");
        let ns = Interpolation::plain("svg".into(), ns_span);
        let qn = InterpolatedQualifiedName::new(name, span, Some(ns));
        assert!(qn.namespace.is_some());
    }

    #[test]
    fn test_qualified_name_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "div");
        let name = Interpolation::plain("div".into(), span);
        let qn = InterpolatedQualifiedName::new(name, span, None);
        assert_eq!(format!("{qn}"), "div");
    }

    #[test]
    fn test_qualified_name_display_with_namespace() {
        let arena = Bump::new();
        let span = make_span(&arena, "svg|div");
        let name = Interpolation::plain("div".into(), span);
        let ns_span = make_span(&arena, "svg");
        let ns = Interpolation::plain("svg".into(), ns_span);
        let qn = InterpolatedQualifiedName::new(name, span, Some(ns));
        assert_eq!(format!("{qn}"), "svg|div");
    }
}
