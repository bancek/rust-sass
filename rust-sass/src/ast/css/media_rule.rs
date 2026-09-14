// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/css/media_rule.dart + lib/src/ast/css/modifiable/media_rule.dart
// go-source: go/value/css_media_rule.go + go/value/css_modifiable_media_rule.go

use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;

use crate::ast::css::media_query::CssMediaQuery;
use crate::ast::css::modifiable_node::ModifiableCssNode;
use crate::ast::css::node::CssNode;

/// A plain CSS `@media` rule.
#[derive(Clone, Debug)]
pub struct CssMediaRule<'parse> {
    /// The queries for this rule. This is never empty.
    pub queries: Vec<CssMediaQuery>,
    /// The child statements of this rule.
    pub children: Vec<CssNode<'parse>>,
    /// The source span for this rule.
    pub span: FileSpan<'parse>,
    /// Whether this node was the last in a nested Sass tree flattened during
    /// evaluation. See [`CssNode::is_group_end`](super::node::CssNode::is_group_end).
    pub is_group_end: bool,
    /// Extra NESTED-output indent levels stamped by the evaluator (libsass
    /// `tabs()`); read by the serializer only for `OutputStyle::Nested`.
    pub tabs: u32,
}

impl<'parse> CssMediaRule<'parse> {
    /// Creates a media rule. Fails when `queries` is empty.
    pub fn new(queries: Vec<CssMediaQuery>, span: FileSpan<'parse>) -> SassResult<Self> {
        if queries.is_empty() {
            return Err(Box::new(SassError::Script {
                message: "queries may not be empty.".into(),
                argument_name: Some("queries".into()),
            }));
        }
        Ok(CssMediaRule {
            queries,
            children: Vec::new(),
            span,
            is_group_end: false,
            tabs: 0,
        })
    }
}

impl<'parse> AstNode<'parse> for CssMediaRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for CssMediaRule<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@media")?;
        for (i, q) in self.queries.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, " {}", q)?;
        }
        write!(f, " {{")?;
        for child in &self.children {
            write!(f, " {child}")?;
        }
        write!(f, " }}")
    }
}

// Frozen-class docs ported from `CssMediaRule` (media_rule.dart). The
// modifiable counterpart (`ModifiableCssMediaRule` in
// modifiable/media_rule.dart) implements the frozen interface for use during
// evaluation; its `equals_ignoring_children`/`copy_without_children`
// (query-list comparison) live on the `ModifiableCssNode` tree API and the A8
// batch files.
#[derive(Clone, Debug)]
pub struct ModifiableCssMediaRule<'parse> {
    /// The queries for this rule. This is never empty.
    pub queries: Vec<CssMediaQuery>,
    /// The source span for this rule.
    pub span: FileSpan<'parse>,
    /// The child statements of this rule.
    pub children: Vec<ModifiableCssNode<'parse>>,
}

impl<'parse> ModifiableCssMediaRule<'parse> {
    /// Creates a modifiable media rule. Fails when `queries` is empty.
    pub fn new(queries: Vec<CssMediaQuery>, span: FileSpan<'parse>) -> SassResult<Self> {
        if queries.is_empty() {
            return Err(Box::new(SassError::Script {
                message: "queries may not be empty.".into(),
                argument_name: Some("queries".into()),
            }));
        }
        Ok(ModifiableCssMediaRule {
            queries,
            span,
            children: Vec::new(),
        })
    }
}

impl<'parse> AstNode<'parse> for ModifiableCssMediaRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for ModifiableCssMediaRule<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@media")?;
        for (i, q) in self.queries.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, " {}", q)?;
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
    fn test_media_rule_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let q = CssMediaQuery::new_type(Some("screen".into()), None, vec![]);
        let r = ModifiableCssMediaRule::new(vec![q], span).unwrap();
        assert_eq!(r.queries.len(), 1);
    }

    #[test]
    fn test_media_rule_empty_queries() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let result = ModifiableCssMediaRule::new(vec![], span);
        assert!(result.is_err());
    }
}
