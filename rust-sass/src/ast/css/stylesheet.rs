// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/css/stylesheet.dart + lib/src/ast/css/modifiable/stylesheet.dart
// go-source: go/value/css_stylesheet.go + go/value/css_modifiable_stylesheet.go

use std::fmt;

use bumpalo::Bump;

use crate::url::SassUrl;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::common::source_span_file_source::FileSource;

use crate::ast::css::modifiable_node::ModifiableCssNode;
use crate::ast::css::node::CssNode;

/// A plain CSS stylesheet.
///
/// This is the root plain CSS node. It contains top-level statements.
#[derive(Clone, Debug)]
pub struct CssStylesheet<'parse> {
    /// The top-level statements of this stylesheet.
    pub children: Vec<CssNode<'parse>>,
    /// The source span for this stylesheet.
    pub span: FileSpan<'parse>,
    /// Whether this node was the last in a nested Sass tree flattened during
    /// evaluation (always `false` for the frozen root; transferred at freeze
    /// via `to_css_node`). See [`CssNode::is_group_end`](super::node::CssNode::is_group_end).
    pub is_group_end: bool,
}

impl<'parse> CssStylesheet<'parse> {
    /// Creates a stylesheet containing `children`.
    pub fn new(children: Vec<CssNode<'parse>>, span: FileSpan<'parse>) -> Self {
        CssStylesheet {
            children,
            span,
            is_group_end: false,
        }
    }

    /// Creates an empty stylesheet with the given source URL.
    pub fn empty<'compile: 'parse>(source_url: Option<SassUrl>, arena: &'compile Bump) -> Self {
        let fs = FileSource::new_in(arena, "", source_url);
        let span = FileSpan::new(Some(fs), 0, 0);
        CssStylesheet {
            children: Vec::new(),
            span,
            is_group_end: false,
        }
    }
}

impl<'parse> AstNode<'parse> for CssStylesheet<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for CssStylesheet<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for child in &self.children {
            write!(f, "{child}")?;
        }
        Ok(())
    }
}

// Frozen-class docs ported from `CssStylesheet` (stylesheet.dart). The
// modifiable counterpart (`ModifiableCssStylesheet` in
// modifiable/stylesheet.dart) implements the frozen interface for use during
// evaluation; its `equals_ignoring_children`/`copy_without_children` live on
// the `ModifiableCssNode` tree API and the A8 batch files. Dart's
// `parent => null` has no Rust counterpart: parent links live on the outer
// `ModifiableCssNode` wrapper (see `docs/ref/ast.md` §9).
#[derive(Clone, Debug)]
pub struct ModifiableCssStylesheet<'parse> {
    /// The source span for this stylesheet.
    pub span: FileSpan<'parse>,
    /// The top-level statements of this stylesheet.
    pub children: Vec<ModifiableCssNode<'parse>>,
}

impl<'parse> ModifiableCssStylesheet<'parse> {
    /// Creates a modifiable stylesheet with no children yet.
    pub fn new(span: FileSpan<'parse>) -> Self {
        ModifiableCssStylesheet {
            span,
            children: Vec::new(),
        }
    }
}

impl<'parse> AstNode<'parse> for ModifiableCssStylesheet<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for ModifiableCssStylesheet<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for child in &self.children {
            write!(f, "{child}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn test_stylesheet_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let ss = ModifiableCssStylesheet::new(span);
        assert_eq!(ss.children.len(), 0);
    }

    #[test]
    fn test_stylesheet_empty() {
        let arena = Bump::new();
        let ss = CssStylesheet::empty(None::<SassUrl>, &arena);
        assert_eq!(ss.children.len(), 0);
        assert_eq!(ss.span().unwrap().text(), "");
    }
}
