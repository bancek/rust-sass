// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/css/supports_rule.dart + lib/src/ast/css/modifiable/supports_rule.dart
// go-source: go/value/css_supports_rule.go + go/value/css_modifiable_supports_rule.go

use std::fmt;

use crate::common::ast_css_value::CssValue;
use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::css::modifiable_node::ModifiableCssNode;
use crate::ast::css::node::CssNode;

/// A plain CSS `@supports` rule.
#[derive(Clone, Debug)]
pub struct CssSupportsRule<'parse> {
    /// The supports condition.
    pub condition: CssValue<'parse, String>,
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

impl<'parse> CssSupportsRule<'parse> {
    /// Creates a supports rule with no children yet.
    pub fn new(condition: CssValue<'parse, String>, span: FileSpan<'parse>) -> Self {
        CssSupportsRule {
            condition,
            children: Vec::new(),
            span,
            is_group_end: false,
            tabs: 0,
        }
    }
}

impl<'parse> AstNode<'parse> for CssSupportsRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for CssSupportsRule<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@supports {}", self.condition)?;
        write!(f, " {{")?;
        for child in &self.children {
            write!(f, " {child}")?;
        }
        write!(f, " }}")
    }
}

// Frozen-class docs ported from `CssSupportsRule` (supports_rule.dart). The
// modifiable counterpart (`ModifiableCssSupportsRule` in
// modifiable/supports_rule.dart) implements the frozen interface for use
// during evaluation; its `equals_ignoring_children`/`copy_without_children`
// (condition comparison) live on the `ModifiableCssNode` tree API and the A8
// batch files.
#[derive(Clone, Debug)]
pub struct ModifiableCssSupportsRule<'parse> {
    /// The supports condition.
    pub condition: CssValue<'parse, String>,
    /// The source span for this rule.
    pub span: FileSpan<'parse>,
    /// The child statements of this rule.
    pub children: Vec<ModifiableCssNode<'parse>>,
}

impl<'parse> ModifiableCssSupportsRule<'parse> {
    /// Creates a modifiable supports rule with no children yet.
    pub fn new(condition: CssValue<'parse, String>, span: FileSpan<'parse>) -> Self {
        ModifiableCssSupportsRule {
            condition,
            span,
            children: Vec::new(),
        }
    }
}

impl<'parse> AstNode<'parse> for ModifiableCssSupportsRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for ModifiableCssSupportsRule<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@supports {}", self.condition)?;
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
    fn test_supports_rule_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let cond_span = make_span(&arena, "display: grid");
        let cond = CssValue::new("display: grid".into(), cond_span);
        let r = ModifiableCssSupportsRule::new(cond, span);
        assert_eq!(r.condition.value, "display: grid");
    }
}
