// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/css/comment.dart + lib/src/ast/css/modifiable/comment.dart
// go-source: go/value/css_comment.go + go/value/css_modifiable_comment.go

use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

/// A plain CSS comment.
///
/// This is always a multi-line comment.
#[derive(Clone, Debug)]
pub struct CssComment<'parse> {
    /// The contents of this comment, including `/*` and `*/`.
    pub text: String,
    /// Whether this comment starts with `/*!` and so is preserved even in
    /// compressed mode.
    pub is_preserved: bool,
    /// The source span for this comment.
    pub span: FileSpan<'parse>,
    /// Whether this node was the last in a nested Sass tree flattened during
    /// evaluation. See [`CssNode::is_group_end`](super::node::CssNode::is_group_end).
    pub is_group_end: bool,
}

impl<'parse> CssComment<'parse> {
    /// Creates a comment, deriving [`is_preserved`](CssComment::is_preserved)
    /// from whether the third byte of `text` is `!`.
    pub fn new(text: String, span: FileSpan<'parse>) -> Self {
        let is_preserved = text.as_bytes().get(2) == Some(&b'!');
        CssComment {
            text,
            is_preserved,
            span,
            is_group_end: false,
        }
    }
}

impl<'parse> AstNode<'parse> for CssComment<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for CssComment<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.text)
    }
}

// Frozen-class docs ported from `CssComment` (comment.dart). The modifiable
// counterpart (`ModifiableCssComment` in modifiable/comment.dart) implements
// this interface for use during evaluation; see `docs/ref/ast.md` (CSS AST).
#[derive(Clone, Debug)]
pub struct ModifiableCssComment<'parse> {
    /// The contents of this comment, including `/*` and `*/`.
    pub text: String,
    /// The source span for this comment.
    pub span: FileSpan<'parse>,
}

impl<'parse> ModifiableCssComment<'parse> {
    /// Creates a modifiable comment.
    pub fn new(text: String, span: FileSpan<'parse>) -> Self {
        ModifiableCssComment { text, span }
    }

    /// Whether this comment starts with `/*!` and so is preserved even in
    /// compressed mode.
    pub fn is_preserved(&self) -> bool {
        self.text.as_bytes().get(2) == Some(&b'!')
    }
}

impl<'parse> AstNode<'parse> for ModifiableCssComment<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for ModifiableCssComment<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.text)
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
    fn test_comment_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "/* hello */");
        let c = ModifiableCssComment::new("/* hello */".into(), span);
        assert_eq!(c.text, "/* hello */");
    }

    #[test]
    fn test_comment_is_preserved() {
        let arena = Bump::new();
        let span = make_span(&arena, "/*! imp */");
        let c = ModifiableCssComment::new("/*! important */".into(), span);
        assert!(c.is_preserved());

        let c2 = ModifiableCssComment::new("/* normal */".into(), span);
        assert!(!c2.is_preserved());
    }

    #[test]
    fn test_immutable_comment_preserved() {
        let arena = Bump::new();
        let span = make_span(&arena, "/*! test */");
        let c = CssComment::new("/*! test */".into(), span);
        assert!(c.is_preserved);
    }

    #[test]
    fn test_comment_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "/* test */");
        let c = CssComment::new("/* test */".into(), span);
        assert_eq!(format!("{c}"), "/* test */");
    }

    #[test]
    fn test_modifiable_comment_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "/* test */");
        let c = ModifiableCssComment::new("/* test */".into(), span);
        assert_eq!(format!("{c}"), "/* test */");
    }
}
