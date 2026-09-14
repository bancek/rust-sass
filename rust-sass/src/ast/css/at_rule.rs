// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/css/at_rule.dart + lib/src/ast/css/modifiable/at_rule.dart
// go-source: go/value/css_at_rule.go + go/value/css_modifiable_at_rule.go

use std::fmt;

use crate::common::ast_css_value::CssValue;
use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::css::modifiable_node::ModifiableCssNode;
use crate::ast::css::node::CssNode;

/// An unknown plain CSS at-rule.
#[derive(Clone, Debug)]
pub struct CssAtRule<'parse> {
    /// The name of this rule.
    pub name: CssValue<'parse, String>,
    /// The value of this rule, if it has one.
    pub value: Option<CssValue<'parse, String>>,
    /// The child statements of this rule.
    pub children: Vec<CssNode<'parse>>,
    /// Whether the rule has no children and is emitted without curly braces.
    ///
    /// This implies `children` is empty, but the reverse is not true — for a
    /// rule like `@foo {}`, [`children`](CssAtRule::children) is empty but
    /// `childless` is `false`.
    pub childless: bool,
    /// The source span for this rule.
    pub span: FileSpan<'parse>,
    /// Whether this node was the last in a nested Sass tree flattened during
    /// evaluation. See [`CssNode::is_group_end`](super::node::CssNode::is_group_end).
    pub is_group_end: bool,
}

impl<'parse> CssAtRule<'parse> {
    /// Creates an at-rule with no children.
    ///
    /// Pass `childless` as `true` for rules emitted without curly braces
    /// (such as `@import "foo.css";`); `@foo {}` gets `childless: false`
    /// with an empty [`children`](CssAtRule::children) list.
    pub fn new(
        name: CssValue<'parse, String>,
        span: FileSpan<'parse>,
        childless: bool,
        value: Option<CssValue<'parse, String>>,
    ) -> Self {
        CssAtRule {
            name,
            value,
            children: Vec::new(),
            childless,
            span,
            is_group_end: false,
        }
    }
}

impl<'parse> AstNode<'parse> for CssAtRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for CssAtRule<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.name)?;
        if let Some(ref v) = self.value {
            write!(f, " {}", v)?;
        }
        if self.childless {
            write!(f, ";")
        } else {
            write!(f, " {{")?;
            for child in &self.children {
                write!(f, " {child}")?;
            }
            write!(f, " }}")
        }
    }
}

// Frozen-class docs ported from `CssAtRule` (at_rule.dart). The modifiable
// counterpart (`ModifiableCssAtRule` in modifiable/at_rule.dart) implements
// the frozen interface for use during evaluation; its docs
// (`equals_ignoring_children`, `copy_without_children`, `add_child`) live on
// the `ModifiableCssNode` tree API and the A8 batch files. See
// `docs/ref/ast.md` (CSS AST section).
#[derive(Clone, Debug)]
pub struct ModifiableCssAtRule<'parse> {
    /// The name of this rule.
    pub name: CssValue<'parse, String>,
    /// The value of this rule, if it has one.
    pub value: Option<CssValue<'parse, String>>,
    /// Whether the rule has no children; childless rules reject `add_child`.
    pub childless: bool,
    /// The source span for this rule.
    pub span: FileSpan<'parse>,
    /// The child statements of this rule.
    pub children: Vec<ModifiableCssNode<'parse>>,
}

impl<'parse> ModifiableCssAtRule<'parse> {
    /// Creates an at-rule with no children yet attached.
    pub fn new(
        name: CssValue<'parse, String>,
        span: FileSpan<'parse>,
        childless: bool,
        value: Option<CssValue<'parse, String>>,
    ) -> Self {
        ModifiableCssAtRule {
            name,
            value,
            childless,
            span,
            children: Vec::new(),
        }
    }
}

impl<'parse> AstNode<'parse> for ModifiableCssAtRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for ModifiableCssAtRule<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.name)?;
        if let Some(ref v) = self.value {
            write!(f, " {}", v)?;
        }
        if self.childless {
            write!(f, ";")
        } else {
            write!(f, " {{")?;
            for child in &self.children {
                write!(f, " {child}")?;
            }
            write!(f, " }}")
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

    fn make_val<'compile, 'parse>(arena: &'compile Bump, s: &str) -> CssValue<'parse, String>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        CssValue::new(s.into(), make_span(arena, s))
    }

    #[test]
    fn test_at_rule_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "@media {}");
        let name = make_val(&arena, "media");
        let r = ModifiableCssAtRule::new(name, span, false, None);
        assert_eq!(r.name.value, "media");
        assert!(!r.childless);
    }

    #[test]
    fn test_at_rule_childless() {
        let arena = Bump::new();
        let span = make_span(&arena, "@import;");
        let name = make_val(&arena, "import");
        let r = ModifiableCssAtRule::new(name, span, true, None);
        assert!(r.childless);
    }

    #[test]
    fn test_at_rule_display_childless() {
        let arena = Bump::new();
        let span = make_span(&arena, "@import;");
        let name = make_val(&arena, "import");
        let r = CssAtRule::new(name, span, true, None);
        assert_eq!(format!("{r}"), "@import;");
    }
}
