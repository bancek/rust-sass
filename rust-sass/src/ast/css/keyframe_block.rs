// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/css/keyframe_block.dart + lib/src/ast/css/modifiable/keyframe_block.dart
// go-source: go/value/css_keyframe_block.go + go/value/css_modifiable_keyframe_block.go

use std::fmt;

use crate::common::ast_css_value::CssValue;
use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::css::modifiable_node::ModifiableCssNode;
use crate::ast::css::node::CssNode;

/// A block within a `@keyframes` rule.
///
/// For example, `10% {opacity: 0.5}`.
#[derive(Clone, Debug)]
pub struct CssKeyframeBlock<'parse> {
    /// The selector for this block.
    pub selector: CssValue<'parse, Vec<String>>,
    /// The child statements of this block.
    pub children: Vec<CssNode<'parse>>,
    /// The source span for this block.
    pub span: FileSpan<'parse>,
    /// Whether this node was the last in a nested Sass tree flattened during
    /// evaluation. See [`CssNode::is_group_end`](super::node::CssNode::is_group_end).
    pub is_group_end: bool,
}

impl<'parse> CssKeyframeBlock<'parse> {
    /// Creates a keyframe block with no children yet.
    pub fn new(selector: CssValue<'parse, Vec<String>>, span: FileSpan<'parse>) -> Self {
        CssKeyframeBlock {
            selector,
            children: Vec::new(),
            span,
            is_group_end: false,
        }
    }
}

impl<'parse> AstNode<'parse> for CssKeyframeBlock<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for CssKeyframeBlock<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, s) in self.selector.value.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{s}")?;
        }
        write!(f, " {{")?;
        for child in &self.children {
            write!(f, " {child}")?;
        }
        write!(f, " }}")
    }
}

// Frozen-class docs ported from `CssKeyframeBlock` (keyframe_block.dart).
// The modifiable counterpart (`ModifiableCssKeyframeBlock` in
// modifiable/keyframe_block.dart) implements the frozen interface for use
// during evaluation; its `equals_ignoring_children`/`copy_without_children`
// (selector-value comparison via `listEquals`) live on the
// `ModifiableCssNode` tree API and the A8 batch files.
#[derive(Clone, Debug)]
pub struct ModifiableCssKeyframeBlock<'parse> {
    /// The selector for this block.
    pub selector: CssValue<'parse, Vec<String>>,
    /// The source span for this block.
    pub span: FileSpan<'parse>,
    /// The child statements of this block.
    pub children: Vec<ModifiableCssNode<'parse>>,
}

impl<'parse> ModifiableCssKeyframeBlock<'parse> {
    /// Creates a modifiable keyframe block with no children yet.
    pub fn new(selector: CssValue<'parse, Vec<String>>, span: FileSpan<'parse>) -> Self {
        ModifiableCssKeyframeBlock {
            selector,
            span,
            children: Vec::new(),
        }
    }
}

impl<'parse> AstNode<'parse> for ModifiableCssKeyframeBlock<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for ModifiableCssKeyframeBlock<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, s) in self.selector.value.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{s}")?;
        }
        write!(f, " {{")?;
        for child in &self.children {
            write!(f, " {child}")?;
        }
        write!(f, " }}")
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
    fn test_keyframe_block_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let sel_span = make_span(&arena, "10%");
        let sel = CssValue::new(vec!["10%".into()], sel_span);
        let b = ModifiableCssKeyframeBlock::new(sel, span);
        assert_eq!(b.selector.value, vec!["10%"]);
    }
}
