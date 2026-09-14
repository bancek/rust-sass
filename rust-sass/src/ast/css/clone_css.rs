// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/clone_css.dart
// go-source: go/sassclonecss/clone_css.go

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use bumpalo::Bump;

use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::extend::store::{ExtensionStore, StoreBox};
use crate::selector::list::{SelectorList, SelectorListIdentity};

use crate::ast::css::at_rule::ModifiableCssAtRule;
use crate::ast::css::comment::ModifiableCssComment;
use crate::ast::css::declaration::ModifiableCssDeclaration;
use crate::ast::css::import::ModifiableCssImport;
use crate::ast::css::keyframe_block::ModifiableCssKeyframeBlock;
use crate::ast::css::media_rule::ModifiableCssMediaRule;
use crate::ast::css::modifiable_node::{ModifiableCssNode, ModifiableCssNodeKind};
use crate::ast::css::modifiable_visitor::CloneCssVisitor;
use crate::ast::css::style_rule::ModifiableCssStyleRule;
use crate::ast::css::stylesheet::ModifiableCssStylesheet;
use crate::ast::css::supports_rule::ModifiableCssSupportsRule;

// =============================================================================
// visit_children — shared free function
// =============================================================================

/// Clones each of `old_children` and attaches the clones to `new_parent`,
/// then returns `new_parent`.
///
/// Carries the `is_group_end` flag across so the blank-line grouping the
/// serializer emits is preserved in the copy.
// Matches Dart: `_CloneCssVisitor._visitChildren` (clone_css.dart).
fn visit_children<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    new_parent: &ModifiableCssNode<'parse>,
    old_children: &[ModifiableCssNode<'parse>],
    visitor: &impl CloneCssVisitor<'parse>,
) -> SassResult<ModifiableCssNode<'parse>> {
    for old_child in old_children {
        let new_child = old_child.clone_from(arena, visitor)?;
        new_child.set_is_group_end(old_child.is_group_end());
        // `tabs` is intrinsic (source depth), so clones inherit the stamp.
        new_child.add_tabs(old_child.tabs());
        new_parent.add_child(&new_child)?;
    }
    Ok(new_parent.clone())
}

// =============================================================================
// CloneCssVisitorWithExt — with extension store selector mapping
// =============================================================================

/// Visitor that creates a deep (and mutable) copy of a stylesheet.
///
/// Style-rule selectors are remapped through the cloned extension store, so
/// the copy stays associated with the new store rather than the original.
struct CloneCssVisitorWithExt<'parse> {
    old_to_new: HashMap<SelectorListIdentity<'parse>, StoreBox<SelectorList<'parse>>>,
}

impl<'parse> CloneCssVisitor<'parse> for CloneCssVisitorWithExt<'parse> {
    fn visit_css_at_rule<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssAtRule<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let rule = ModifiableCssAtRule::new(
            node.name.clone(),
            node.span,
            node.childless,
            node.value.clone(),
        );
        let new_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::AtRule(rule));
        if node.childless {
            return Ok(new_node);
        }
        visit_children(arena, &new_node, &node.children, self)
    }

    fn visit_css_comment<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssComment<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let c = ModifiableCssComment::new(node.text.clone(), node.span);
        Ok(ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::Comment(c),
        ))
    }

    fn visit_css_declaration<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssDeclaration<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let d = ModifiableCssDeclaration::new(
            node.name.clone(),
            node.value.clone(),
            node.span,
            node.parsed_as_sass_script,
            Some(node.value_span_for_map),
        )?;
        Ok(ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::Declaration(d),
        ))
    }

    fn visit_css_import<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssImport<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let i = ModifiableCssImport {
            url: node.url.clone(),
            modifiers: node.modifiers.clone(),
            span: node.span,
        };
        Ok(ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::Import(i),
        ))
    }

    fn visit_css_keyframe_block<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssKeyframeBlock<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let b = ModifiableCssKeyframeBlock::new(node.selector.clone(), node.span);
        let new_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::KeyframeBlock(b));
        visit_children(arena, &new_node, &node.children, self)
    }

    fn visit_css_media_rule<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssMediaRule<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let r = ModifiableCssMediaRule::new(node.queries.clone(), node.span)?;
        let new_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::MediaRule(r));
        visit_children(arena, &new_node, &node.children, self)
    }

    fn visit_css_style_rule<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssStyleRule<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        // Looks up the cloned selector for this rule's original selector.
        // Errors when the store and the stylesheet come from different
        // compilations, since no mapping exists for the selector.
        let selector_identity = SelectorListIdentity::from(*node.selector.borrow());
        let new_box = self
            .old_to_new
            .get(&selector_identity)
            .ok_or_else(|| SassError::Script {
                message: "The ExtensionStore and CssStylesheet passed to \
                     clone_css_stylesheet() must come from the same compilation."
                    .into(),
                argument_name: None,
            })?;
        // Dart `clone_css.dart visitCssStyleRule` omits `fromPlainCss`
        // (defaults to false).
        let rule = ModifiableCssStyleRule::new(
            Rc::clone(&new_box.inner),
            node.span,
            Some(node.original_selector),
            false,
        );
        let new_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::StyleRule(rule));
        visit_children(arena, &new_node, &node.children, self)
    }

    fn visit_css_stylesheet<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssStylesheet<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let s = ModifiableCssStylesheet::new(node.span);
        let new_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::Stylesheet(s));
        visit_children(arena, &new_node, &node.children, self)
    }

    fn visit_css_supports_rule<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssSupportsRule<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let r = ModifiableCssSupportsRule::new(node.condition.clone(), node.span);
        let new_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::SupportsRule(r));
        visit_children(arena, &new_node, &node.children, self)
    }
}

// =============================================================================
// CloneCssVisitorNoExt — without extension store
// =============================================================================

/// Same deep copy as [`CloneCssVisitorWithExt`], but clones style-rule
/// selectors in place instead of remapping them through an extension store.
struct CloneCssVisitorNoExt;

impl<'parse> CloneCssVisitor<'parse> for CloneCssVisitorNoExt {
    fn visit_css_at_rule<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssAtRule<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let rule = ModifiableCssAtRule::new(
            node.name.clone(),
            node.span,
            node.childless,
            node.value.clone(),
        );
        let new_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::AtRule(rule));
        if node.childless {
            return Ok(new_node);
        }
        visit_children(arena, &new_node, &node.children, self)
    }

    fn visit_css_comment<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssComment<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let c = ModifiableCssComment::new(node.text.clone(), node.span);
        Ok(ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::Comment(c),
        ))
    }

    fn visit_css_declaration<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssDeclaration<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let d = ModifiableCssDeclaration::new(
            node.name.clone(),
            node.value.clone(),
            node.span,
            node.parsed_as_sass_script,
            Some(node.value_span_for_map),
        )?;
        Ok(ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::Declaration(d),
        ))
    }

    fn visit_css_import<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssImport<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let i = ModifiableCssImport {
            url: node.url.clone(),
            modifiers: node.modifiers.clone(),
            span: node.span,
        };
        Ok(ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::Import(i),
        ))
    }

    fn visit_css_keyframe_block<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssKeyframeBlock<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let b = ModifiableCssKeyframeBlock::new(node.selector.clone(), node.span);
        let new_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::KeyframeBlock(b));
        visit_children(arena, &new_node, &node.children, self)
    }

    fn visit_css_media_rule<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssMediaRule<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let r = ModifiableCssMediaRule::new(node.queries.clone(), node.span)?;
        let new_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::MediaRule(r));
        visit_children(arena, &new_node, &node.children, self)
    }

    fn visit_css_style_rule<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssStyleRule<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let sel = Rc::new(RefCell::new(*node.selector.borrow()));
        // Dart `clone_css.dart visitCssStyleRule` omits `fromPlainCss`
        // (defaults to false).
        let rule = ModifiableCssStyleRule::new(sel, node.span, Some(node.original_selector), false);
        let new_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::StyleRule(rule));
        visit_children(arena, &new_node, &node.children, self)
    }

    fn visit_css_stylesheet<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssStylesheet<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let s = ModifiableCssStylesheet::new(node.span);
        let new_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::Stylesheet(s));
        visit_children(arena, &new_node, &node.children, self)
    }

    fn visit_css_supports_rule<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        node: &ModifiableCssSupportsRule<'parse>,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        let r = ModifiableCssSupportsRule::new(node.condition.clone(), node.span);
        let new_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::SupportsRule(r));
        visit_children(arena, &new_node, &node.children, self)
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Returns deep copies of both the stylesheet and its extension store.
///
/// The store must be associated with the stylesheet: a style-rule selector
/// with no entry in the cloned store is an error, not a silent mismatch.
pub fn clone_css_stylesheet<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    sheet: &ModifiableCssNode<'parse>,
    store: &ExtensionStore<'parse>,
) -> SassResult<(ModifiableCssNode<'parse>, ExtensionStore<'parse>)> {
    let (new_store, old_to_new) = store.clone_store()?;
    let visitor = CloneCssVisitorWithExt { old_to_new };
    let span = sheet.span()?;
    let new_sheet = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::Stylesheet(ModifiableCssStylesheet::new(span)),
    );
    let _ = arena;
    if let Some(old_children) = sheet.children() {
        for old_child in &old_children {
            let new_child = old_child.clone_from(arena, &visitor)?;
            new_child.set_is_group_end(old_child.is_group_end());
            new_child.add_tabs(old_child.tabs());
            new_sheet.add_child(&new_child)?;
        }
    }
    Ok((new_sheet, new_store))
}

/// Deep-copies a single node (and its subtree) without an extension store.
///
/// Style-rule selectors are cloned in place; unlike
/// [`clone_css_stylesheet`], no same-compilation check applies.
pub fn clone_css_node_no_ext<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    node: &ModifiableCssNode<'parse>,
) -> SassResult<ModifiableCssNode<'parse>> {
    let _ = arena;
    node.clone_from(arena, &CloneCssVisitorNoExt)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::css::media_query::CssMediaQuery;
    use crate::common::ast_css_value::CssValue;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::io::VirtualIo;
    use crate::selector::class::ClassSelector;
    use crate::selector::complex::ComplexSelector;
    use crate::selector::complex_component::ComplexSelectorComponent;
    use crate::selector::compound::CompoundSelector;
    use crate::selector::SimpleSelector;
    use crate::value::string::SassString;
    use crate::value::{Value, ValueKind};
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

    fn make_css_media_query(cond: &str) -> CssMediaQuery {
        CssMediaQuery::new_condition(vec![cond.into()], None).unwrap()
    }

    fn make_stylesheet<'compile, 'parse>(arena: &'compile Bump) -> ModifiableCssNode<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::Stylesheet(ModifiableCssStylesheet::new(make_span(arena, ""))),
        )
    }

    // -----------------------------------------------------------------------
    // clone_css_stylesheet tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_clone_css_stylesheet_empty() {
        let arena = Bump::new();
        let sheet = make_stylesheet(&arena);
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        let (result, _) = clone_css_stylesheet(&arena, &sheet, &store).unwrap();
        assert_eq!(result.children().unwrap().len(), 0);
    }

    #[test]
    fn test_clone_css_stylesheet_comment() {
        let arena = Bump::new();
        let sheet = make_stylesheet(&arena);
        let span = make_span(&arena, "/* hello */");
        let comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new("/* hello */".into(), span)),
        );
        sheet.add_child(&comment).unwrap();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        let (result, _) = clone_css_stylesheet(&arena, &sheet, &store).unwrap();
        let children = result.children().unwrap();
        assert_eq!(children.len(), 1);
        let kind = children[0].kind();
        match &*kind {
            ModifiableCssNodeKind::Comment(c) => {
                assert_eq!(c.text, "/* hello */");
            }
            _ => panic!("expected Comment"),
        }
    }

    #[test]
    fn test_clone_css_stylesheet_declaration() {
        let arena = Bump::new();
        let sheet = make_stylesheet(&arena);
        let span = make_span(&arena, "color: red;");
        let name = make_val(&arena, "color");
        let s = SassString::new("red", false);
        let val = CssValue::new(
            Value::new_with_arena(&arena, ValueKind::String(s)),
            make_span(&arena, "red"),
        );
        let decl = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Declaration(
                ModifiableCssDeclaration::new(name, val, span, true, None).unwrap(),
            ),
        );
        sheet.add_child(&decl).unwrap();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        let (result, _) = clone_css_stylesheet(&arena, &sheet, &store).unwrap();
        let children = result.children().unwrap();
        assert_eq!(children.len(), 1);
        let kind = children[0].kind();
        match &*kind {
            ModifiableCssNodeKind::Declaration(d) => {
                assert!(d.parsed_as_sass_script);
            }
            _ => panic!("expected Declaration"),
        }
    }

    #[test]
    fn test_clone_css_stylesheet_import() {
        let arena = Bump::new();
        let sheet = make_stylesheet(&arena);
        let span = make_span(&arena, "test");
        let url = make_val(&arena, "\"foo.css\"");
        let imp = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Import(ModifiableCssImport {
                url,
                modifiers: None,
                span,
            }),
        );
        sheet.add_child(&imp).unwrap();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        let (result, _) = clone_css_stylesheet(&arena, &sheet, &store).unwrap();
        let children = result.children().unwrap();
        assert_eq!(children.len(), 1);
        let kind = children[0].kind();
        match &*kind {
            ModifiableCssNodeKind::Import(i) => {
                assert_eq!(i.url.value, "\"foo.css\"");
            }
            _ => panic!("expected Import"),
        }
    }

    #[test]
    fn test_clone_css_stylesheet_at_rule_childless() {
        let arena = Bump::new();
        let sheet = make_stylesheet(&arena);
        let span = make_span(&arena, "test");
        let name = make_val(&arena, "import");
        let at_rule = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::AtRule(ModifiableCssAtRule::new(name, span, true, None)),
        );
        sheet.add_child(&at_rule).unwrap();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        let (result, _) = clone_css_stylesheet(&arena, &sheet, &store).unwrap();
        assert_eq!(result.children().unwrap().len(), 1);
    }

    #[test]
    fn test_clone_css_stylesheet_at_rule_with_children() {
        let arena = Bump::new();
        let sheet = make_stylesheet(&arena);
        let span = make_span(&arena, "test");
        let name = make_val(&arena, "media");
        let at_rule = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::AtRule(ModifiableCssAtRule::new(name, span, false, None)),
        );
        let comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new("/* nested */".into(), span)),
        );
        at_rule.add_child(&comment).unwrap();
        sheet.add_child(&at_rule).unwrap();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        let (result, _) = clone_css_stylesheet(&arena, &sheet, &store).unwrap();
        let children = result.children().unwrap();
        assert_eq!(children.len(), 1);
        let kind = children[0].kind();
        match &*kind {
            ModifiableCssNodeKind::AtRule(r) => {
                assert_eq!(r.children.len(), 1);
            }
            _ => panic!("expected AtRule"),
        }
    }

    #[test]
    fn test_clone_css_stylesheet_style_rule() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));
        let sel = make_selector_list(&arena, ".foo");
        let store_box = store.add_selector(&arena, &sel, None).unwrap();
        let span = make_span(&arena, "test");
        let rule = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::StyleRule(ModifiableCssStyleRule::new(
                Rc::clone(&store_box.inner),
                span,
                Some(sel),
                false,
            )),
        );
        let sheet = make_stylesheet(&arena);
        sheet.add_child(&rule).unwrap();

        let (_, new_store) = clone_css_stylesheet(&arena, &sheet, &store).unwrap();
        // verify the cloned store is not empty
        assert!(!matches!(new_store, ExtensionStore::Empty));
    }

    #[test]
    fn test_clone_css_stylesheet_media_rule() {
        let arena = Bump::new();
        let sheet = make_stylesheet(&arena);
        let span = make_span(&arena, "test");
        let media = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::MediaRule(
                ModifiableCssMediaRule::new(vec![make_css_media_query("(color)")], span).unwrap(),
            ),
        );
        let comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new("/* inside */".into(), span)),
        );
        media.add_child(&comment).unwrap();
        sheet.add_child(&media).unwrap();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        let (result, _) = clone_css_stylesheet(&arena, &sheet, &store).unwrap();
        let children = result.children().unwrap();
        assert_eq!(children.len(), 1);
        let kind = children[0].kind();
        match &*kind {
            ModifiableCssNodeKind::MediaRule(m) => {
                assert_eq!(m.children.len(), 1);
            }
            _ => panic!("expected MediaRule"),
        }
    }

    #[test]
    fn test_clone_css_stylesheet_supports_rule() {
        let arena = Bump::new();
        let sheet = make_stylesheet(&arena);
        let span = make_span(&arena, "test");
        let cond = make_val(&arena, "display: grid");
        let supports = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::SupportsRule(ModifiableCssSupportsRule::new(cond, span)),
        );
        let comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new("/* inside */".into(), span)),
        );
        supports.add_child(&comment).unwrap();
        sheet.add_child(&supports).unwrap();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        let (result, _) = clone_css_stylesheet(&arena, &sheet, &store).unwrap();
        let children = result.children().unwrap();
        assert_eq!(children.len(), 1);
        let kind = children[0].kind();
        match &*kind {
            ModifiableCssNodeKind::SupportsRule(s) => {
                assert_eq!(s.condition.value, "display: grid");
            }
            _ => panic!("expected SupportsRule"),
        }
    }

    #[test]
    fn test_clone_css_stylesheet_keyframe_block() {
        let arena = Bump::new();
        let sheet = make_stylesheet(&arena);
        let span = make_span(&arena, "test");
        let sel_span = make_span(&arena, "10%");
        let sel = CssValue::new(vec!["10%".into()], sel_span);
        let block = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::KeyframeBlock(ModifiableCssKeyframeBlock::new(sel, span)),
        );
        let comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new("/* inside */".into(), span)),
        );
        block.add_child(&comment).unwrap();
        sheet.add_child(&block).unwrap();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        let (result, _) = clone_css_stylesheet(&arena, &sheet, &store).unwrap();
        let children = result.children().unwrap();
        assert_eq!(children.len(), 1);
        let kind = children[0].kind();
        match &*kind {
            ModifiableCssNodeKind::KeyframeBlock(k) => {
                assert_eq!(k.children.len(), 1);
            }
            _ => panic!("expected KeyframeBlock"),
        }
    }

    #[test]
    fn test_clone_css_stylesheet_nested() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));
        let sel_outer = make_selector_list(&arena, ".outer");
        let store_box_outer = store.add_selector(&arena, &sel_outer, None).unwrap();
        let span = make_span(&arena, "test");
        let outer = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::StyleRule(ModifiableCssStyleRule::new(
                Rc::clone(&store_box_outer.inner),
                span,
                Some(sel_outer),
                false,
            )),
        );
        let media = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::MediaRule(
                ModifiableCssMediaRule::new(vec![make_css_media_query("(color)")], span).unwrap(),
            ),
        );
        let sel_inner = make_selector_list(&arena, ".inner");
        let store_box_inner = store.add_selector(&arena, &sel_inner, None).unwrap();
        let inner = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::StyleRule(ModifiableCssStyleRule::new(
                Rc::clone(&store_box_inner.inner),
                span,
                Some(sel_inner),
                false,
            )),
        );
        let comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new("/* deep */".into(), span)),
        );
        inner.add_child(&comment).unwrap();
        media.add_child(&inner).unwrap();
        outer.add_child(&media).unwrap();
        let sheet = make_stylesheet(&arena);
        sheet.add_child(&outer).unwrap();

        let (result, _) = clone_css_stylesheet(&arena, &sheet, &store).unwrap();
        assert_eq!(result.children().unwrap().len(), 1);
    }

    #[test]
    fn test_clone_css_stylesheet_is_group_end() {
        let arena = Bump::new();
        let sheet = make_stylesheet(&arena);
        let span = make_span(&arena, "test");
        let comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new("/* test */".into(), span)),
        );
        comment.set_is_group_end(true);
        sheet.add_child(&comment).unwrap();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        let (result, _) = clone_css_stylesheet(&arena, &sheet, &store).unwrap();
        let children = result.children().unwrap();
        assert!(children[0].is_group_end());
    }

    #[test]
    fn test_clone_css_stylesheet_deep_copy() {
        let arena = Bump::new();
        let sheet = make_stylesheet(&arena);
        let span = make_span(&arena, "test");
        let comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new(
                "/* original */".into(),
                span,
            )),
        );
        sheet.add_child(&comment).unwrap();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        let (result, _) = clone_css_stylesheet(&arena, &sheet, &store).unwrap();

        // Add a child to original — should not affect clone
        let new_comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new("/* new */".into(), span)),
        );
        sheet.add_child(&new_comment).unwrap();
        assert_eq!(sheet.children().unwrap().len(), 2);
        assert_eq!(result.children().unwrap().len(), 1);
    }

    // -----------------------------------------------------------------------
    // clone_css_node_no_ext tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_clone_css_node_no_ext_style_rule() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let sel = make_selector_list(&arena, ".bar");
        let rc = Rc::new(RefCell::new(sel));
        let rule = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::StyleRule(ModifiableCssStyleRule::new(rc, span, None, false)),
        );

        let cloned = clone_css_node_no_ext(&arena, &rule).unwrap();
        let kind = cloned.kind();
        match &*kind {
            ModifiableCssNodeKind::StyleRule(sr) => {
                assert!(!sr.selector.borrow().is_bogus());
            }
            _ => panic!("expected StyleRule"),
        }
    }

    #[test]
    fn test_clone_css_node_no_ext_media_rule() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let media = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::MediaRule(
                ModifiableCssMediaRule::new(vec![make_css_media_query("(min-width: 1px)")], span)
                    .unwrap(),
            ),
        );
        let comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new("/* inside */".into(), span)),
        );
        media.add_child(&comment).unwrap();

        let cloned = clone_css_node_no_ext(&arena, &media).unwrap();
        let kind = cloned.kind();
        match &*kind {
            ModifiableCssNodeKind::MediaRule(m) => {
                assert_eq!(m.children.len(), 1);
            }
            _ => panic!("expected MediaRule"),
        }
    }

    #[test]
    fn test_clone_css_node_no_ext_nested() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let sel = make_selector_list(&arena, ".outer");
        let rc_outer = Rc::new(RefCell::new(sel));
        let outer = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::StyleRule(ModifiableCssStyleRule::new(
                rc_outer, span, None, false,
            )),
        );
        let sel_inner = make_selector_list(&arena, ".inner");
        let rc_inner = Rc::new(RefCell::new(sel_inner));
        let inner = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::StyleRule(ModifiableCssStyleRule::new(
                rc_inner, span, None, false,
            )),
        );
        let comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new("/* deep */".into(), span)),
        );
        inner.add_child(&comment).unwrap();
        outer.add_child(&inner).unwrap();

        let cloned = clone_css_node_no_ext(&arena, &outer).unwrap();
        let kind = cloned.kind();
        match &*kind {
            ModifiableCssNodeKind::StyleRule(sr) => {
                assert_eq!(sr.children.len(), 1);
            }
            _ => panic!("expected StyleRule"),
        }
    }

    #[test]
    fn test_clone_css_node_no_ext_deep_copy() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let sel = make_selector_list(&arena, ".test");
        let rc = Rc::new(RefCell::new(sel));
        let rule = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::StyleRule(ModifiableCssStyleRule::new(rc, span, None, false)),
        );
        let comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new(
                "/* original */".into(),
                span,
            )),
        );
        rule.add_child(&comment).unwrap();

        let cloned = clone_css_node_no_ext(&arena, &rule).unwrap();

        // Add child to original — should not affect clone
        let new_comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new("/* new */".into(), span)),
        );
        rule.add_child(&new_comment).unwrap();
        assert_eq!(rule.children().unwrap().len(), 2);
        let kind = cloned.kind();
        match &*kind {
            ModifiableCssNodeKind::StyleRule(sr) => {
                assert_eq!(sr.children.len(), 1);
            }
            _ => panic!("expected StyleRule"),
        }
    }
}
