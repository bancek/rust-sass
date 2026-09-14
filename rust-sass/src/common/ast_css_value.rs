// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/css/value.dart
// go-source: go/sasscommon/ast_css_value.go

use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

/// A value in a plain CSS tree.
///
/// Mirrors Dart's `CssValue`: associates a span with a value that doesn't
/// otherwise track its span. It has value equality semantics — equality
/// (and hashing, via the value) ignores the span.
#[derive(Clone)]
pub struct CssValue<'parse, T> {
    /// The value.
    pub value: T,
    span: FileSpan<'parse>,
}

impl<'parse, T: fmt::Debug> fmt::Debug for CssValue<'parse, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}

impl<'parse, T> CssValue<'parse, T> {
    /// Creates a [`CssValue`] pairing `value` with its source `span`.
    ///
    /// Mirrors Dart's `CssValue(value, span)`.
    pub fn new(value: T, span: FileSpan<'parse>) -> Self {
        CssValue { value, span }
    }
}

impl<'parse, T: fmt::Display> fmt::Display for CssValue<'parse, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}

impl<'parse, T> AstNode<'parse> for CssValue<'parse, T> {
    /// The span associated with the value.
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

// `PartialEq` compares only `value`, ignoring `span` — matching Dart's
// `operator ==` (and `hashCode`, which hashes only the value).

impl<'parse, T: PartialEq> PartialEq for CssValue<'parse, T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<'parse, T: Eq> Eq for CssValue<'parse, T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    #[test]
    fn test_css_value_span() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "source", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let v = CssValue::new(42, span);
        let got = v.span().unwrap();
        assert_eq!(got.text(), "source");
    }

    #[test]
    fn test_css_value_display() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "x", None);
        let span = FileSpan::new(Some(fs), 0, 1);
        let v = CssValue::new("hello", span);
        assert_eq!(v.to_string(), "hello");
    }

    #[test]
    fn test_css_value_eq() {
        let arena = Bump::new();
        let fs1 = FileSource::new_in(&arena, "abc", None);
        let fs2 = FileSource::new_in(&arena, "xyz", None);
        let a = CssValue::new(42, FileSpan::new(Some(fs1), 0, 3));
        let b = CssValue::new(42, FileSpan::new(Some(fs2), 0, 3));
        let c = CssValue::new(99, FileSpan::new(Some(fs1), 0, 3));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_css_value_is_ast_node() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "x", None);
        let v = CssValue::new("hello", FileSpan::new(Some(fs), 0, 1));
        let node: &dyn AstNode = &v;
        assert_eq!(node.span().unwrap().text(), "x");
    }
}
