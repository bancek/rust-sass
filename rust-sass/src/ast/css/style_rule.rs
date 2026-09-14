// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/css/style_rule.dart + lib/src/ast/css/modifiable/style_rule.dart
// go-source: go/value/css_style_rule.go + go/value/css_modifiable_style_rule.go

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::selector::list::{SelectorList, SelectorListIdentity};

use crate::ast::css::modifiable_node::ModifiableCssNode;
use crate::ast::css::node::CssNode;

/// A plain CSS style rule.
///
/// This applies style declarations to elements that match a given selector.
/// Note that this isn't *strictly* plain CSS, since [`selector`](CssStyleRule::selector)
/// may still contain placeholder selectors.
#[derive(Clone, Debug)]
pub struct CssStyleRule<'parse> {
    /// The selector for this rule.
    pub selector: SelectorList<'parse>,
    /// The selector for this rule, before any extensions were applied.
    pub original_selector: SelectorList<'parse>,
    /// Whether this style rule was originally defined in a plain CSS stylesheet.
    //
    // Matches Dart: `CssStyleRule.fromPlainCss` (style_rule.dart) — `@nodoc`
    // `@internal`, so a plain `//` comment per rule 2.
    pub from_plain_css: bool,
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

impl<'parse> CssStyleRule<'parse> {
    /// Creates a style rule with no children yet.
    pub fn new(
        selector: SelectorList<'parse>,
        span: FileSpan<'parse>,
        original_selector: SelectorList<'parse>,
        from_plain_css: bool,
    ) -> Self {
        CssStyleRule {
            selector,
            original_selector,
            from_plain_css,
            children: Vec::new(),
            span,
            is_group_end: false,
            tabs: 0,
        }
    }
}

impl<'parse> AstNode<'parse> for CssStyleRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for CssStyleRule<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "selector {{")?;
        for child in &self.children {
            write!(f, " {child}")?;
        }
        write!(f, " }}")
    }
}

// Frozen-class docs ported from `CssStyleRule` (style_rule.dart). The
// modifiable counterpart (`ModifiableCssStyleRule` in
// modifiable/style_rule.dart) implements the frozen interface for use during
// evaluation, holding the selector behind a shared `Rc<RefCell<..>>` that the
// extension store may update as new extensions are applied; its
// `equals_ignoring_children`/`copy_without_children` live on the
// `ModifiableCssNode` tree API and the A8 batch files.
#[derive(Clone, Debug)]
pub struct ModifiableCssStyleRule<'parse> {
    /// A reference to the selector provided by the extension store, which may
    /// update it over time as new extensions are applied.
    pub selector: Rc<RefCell<SelectorList<'parse>>>,
    /// The selector for this rule, before any extensions were applied.
    pub original_selector: SelectorList<'parse>,
    /// Whether this style rule was originally defined in a plain CSS stylesheet.
    //
    // Matches Dart: `fromPlainCss` — `@nodoc` `@internal`, plain `//` per rule 2.
    pub from_plain_css: bool,
    /// The source span for this rule.
    pub span: FileSpan<'parse>,
    /// The child statements of this rule.
    pub children: Vec<ModifiableCssNode<'parse>>,
}

impl<'parse> ModifiableCssStyleRule<'parse> {
    /// Creates a modifiable style rule.
    ///
    /// When `original_selector` is `None`, it defaults to the current value
    /// of `selector`.
    pub fn new(
        selector: Rc<RefCell<SelectorList<'parse>>>,
        span: FileSpan<'parse>,
        original_selector: Option<SelectorList<'parse>>,
        from_plain_css: bool,
    ) -> Self {
        let orig = original_selector.unwrap_or_else(|| *selector.borrow());
        ModifiableCssStyleRule {
            selector,
            original_selector: orig,
            from_plain_css,
            span,
            children: Vec::new(),
        }
    }

    /// Replaces the current selector value in place.
    pub fn set_selector(&self, sel: SelectorList<'parse>) {
        *self.selector.borrow_mut() = sel;
    }

    /// Returns a copy of the current selector value.
    pub fn selector_value(&self) -> SelectorList<'parse> {
        *self.selector.borrow()
    }

    /// Returns the identity key for the current selector, used to look up
    /// extension-store entries (see `clone_css` in `docs/ref/ast.md`).
    pub fn selector_identity(&self) -> SelectorListIdentity<'parse> {
        SelectorListIdentity::from(*self.selector.borrow())
    }
}

impl<'parse> AstNode<'parse> for ModifiableCssStyleRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for ModifiableCssStyleRule<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "selector {{")?;
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
    use crate::selector::class::ClassSelector;
    use crate::selector::complex::ComplexSelector;
    use crate::selector::complex_component::ComplexSelectorComponent;
    use crate::selector::compound::CompoundSelector;
    use crate::selector::SimpleSelector;
    use bumpalo::Bump;

    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    fn make_selector_list<'compile, 'parse>(
        arena: &'compile Bump,
        name: &str,
    ) -> SelectorList<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let span = make_span(arena, name);
        let class = ClassSelector::new(name.into(), span);
        let simple = SimpleSelector::Class(class);
        let compound = CompoundSelector::new(vec![simple], span).unwrap();
        let comp = ComplexSelectorComponent::new(Box::new(compound), Vec::new(), span);
        let complex = ComplexSelector::new(Vec::new(), vec![comp], span, false).unwrap();
        SelectorList::new(arena, vec![complex], span).unwrap()
    }

    #[test]
    fn test_style_rule_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let sel = make_selector_list(&arena, "foo");
        let rc = Rc::new(RefCell::new(sel));
        let r = ModifiableCssStyleRule::new(rc, span, Some(sel), false);
        assert!(!r.from_plain_css);
    }

    #[test]
    fn test_style_rule_from_plain_css() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let sel = make_selector_list(&arena, "foo");
        let rc = Rc::new(RefCell::new(sel));
        let r = ModifiableCssStyleRule::new(rc, span, Some(sel), true);
        assert!(r.from_plain_css);
    }
}
