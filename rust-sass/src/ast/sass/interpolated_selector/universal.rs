// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolated_selector/universal.dart
// go-source: go/value/sass_interpolated_selector_universal.go

use std::fmt;
use std::fmt::Write;

use crate::ast::sass::interpolation::Interpolation;
use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

/// A universal selector, still containing interpolation at parse time.
///
/// Unlike the resolved universal selector, this is parsed during the initial
/// stylesheet parse.
#[derive(Clone, Debug)]
pub struct InterpolatedUniversalSelector<'parse> {
    /// The selector namespace, if any.
    pub namespace: Option<Interpolation<'parse>>,
    /// The source span covering this selector.
    pub span: FileSpan<'parse>,
}

impl<'parse> InterpolatedUniversalSelector<'parse> {
    /// Creates a universal selector with an optional namespace.
    pub fn new(span: FileSpan<'parse>, namespace: Option<Interpolation<'parse>>) -> Self {
        InterpolatedUniversalSelector { namespace, span }
    }

    /// Renders this selector as `*`, or `namespace|*` when namespaced.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        if let Some(ref namespace) = self.namespace {
            write!(buf, "{}|*", namespace).unwrap();
        } else {
            write!(buf, "*").unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> AstNode<'parse> for InterpolatedUniversalSelector<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for InterpolatedUniversalSelector<'parse> {
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
    fn test_universal_selector_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "*");
        let sel = InterpolatedUniversalSelector::new(span, None);
        assert_eq!(sel.span().unwrap(), span);
    }

    #[test]
    fn test_universal_selector_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "*");
        let sel = InterpolatedUniversalSelector::new(span, None);
        assert_eq!(format!("{sel}"), "*");
    }

    #[test]
    fn test_universal_selector_with_namespace() {
        let arena = Bump::new();
        let span = make_span(&arena, "svg|*");
        let ns_span = make_span(&arena, "svg");
        let ns = Interpolation::plain("svg".into(), ns_span);
        let sel = InterpolatedUniversalSelector::new(span, Some(ns));
        assert!(sel.namespace.is_some());
    }
}
