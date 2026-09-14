// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/css/modifiable/node.dart
// go-source: go/value/css_modifiable_node.go

use std::cell::Ref;
use std::cell::RefCell;
use std::fmt;
use std::rc::{Rc, Weak};

use bumpalo::Bump;

use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;

use crate::ast::css::at_rule::{CssAtRule, ModifiableCssAtRule};
use crate::ast::css::comment::{CssComment, ModifiableCssComment};
use crate::ast::css::declaration::{CssDeclaration, ModifiableCssDeclaration};
use crate::ast::css::import::{CssImport, ModifiableCssImport};
use crate::ast::css::keyframe_block::{CssKeyframeBlock, ModifiableCssKeyframeBlock};
use crate::ast::css::media_rule::{CssMediaRule, ModifiableCssMediaRule};
use crate::ast::css::modifiable_visitor::{CloneCssVisitor, ModifiableCssVisitor};
use crate::ast::css::node::CssNode;
use crate::ast::css::style_rule::{CssStyleRule, ModifiableCssStyleRule};
use crate::ast::css::stylesheet::{CssStylesheet, ModifiableCssStylesheet};
use crate::ast::css::supports_rule::{CssSupportsRule, ModifiableCssSupportsRule};
use crate::ast::css::visitor::CssVisitor;

// =============================================================================
// ModifiableCssNodeKind — plain enum
// =============================================================================

/// The mutable payload of a [`ModifiableCssNode`]: the same 9 node kinds as
/// the frozen [`CssNode`], in their modifiable forms.
///
/// Unlike Dart's hierarchy this enum carries no parent state; all parent
/// logic lives on the outer [`ModifiableCssNode`] wrapper.
#[derive(Clone, Debug)]
pub enum ModifiableCssNodeKind<'parse> {
    Stylesheet(ModifiableCssStylesheet<'parse>),
    StyleRule(ModifiableCssStyleRule<'parse>),
    AtRule(ModifiableCssAtRule<'parse>),
    Comment(ModifiableCssComment<'parse>),
    Declaration(ModifiableCssDeclaration<'parse>),
    Import(ModifiableCssImport<'parse>),
    KeyframeBlock(ModifiableCssKeyframeBlock<'parse>),
    MediaRule(ModifiableCssMediaRule<'parse>),
    SupportsRule(ModifiableCssSupportsRule<'parse>),
}

impl<'parse> ModifiableCssNodeKind<'parse> {
    /// Dispatches to the matching [`ModifiableCssVisitor`] method.
    pub fn accept<V: ModifiableCssVisitor<'parse> + ?Sized>(
        &self,
        visitor: &mut V,
    ) -> SassResult<V::Output> {
        match self {
            Self::Stylesheet(node) => visitor.visit_css_stylesheet(node),
            Self::StyleRule(node) => visitor.visit_css_style_rule(node),
            Self::AtRule(node) => visitor.visit_css_at_rule(node),
            Self::Comment(node) => visitor.visit_css_comment(node),
            Self::Declaration(node) => visitor.visit_css_declaration(node),
            Self::Import(node) => visitor.visit_css_import(node),
            Self::KeyframeBlock(node) => visitor.visit_css_keyframe_block(node),
            Self::MediaRule(node) => visitor.visit_css_media_rule(node),
            Self::SupportsRule(node) => visitor.visit_css_supports_rule(node),
        }
    }

    /// Freezes `self` to a [`CssNode`] and dispatches it to `visitor`,
    /// forwarding the stored group-end flag.
    pub fn accept_as_css<V: CssVisitor<'parse> + ?Sized>(
        &self,
        visitor: &mut V,
        is_group_end: bool,
        tabs: u32,
    ) -> SassResult<V::Output> {
        let css_node = self.to_css_node(is_group_end, tabs);
        css_node.accept(visitor)
    }

    /// Deep-copies `self` (and its subtree) through `visitor`.
    pub fn clone_from<'compile: 'parse, V: CloneCssVisitor<'parse> + ?Sized>(
        &self,
        arena: &'compile Bump,
        visitor: &V,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        match self {
            Self::AtRule(node) => visitor.visit_css_at_rule(arena, node),
            Self::Comment(node) => visitor.visit_css_comment(arena, node),
            Self::Declaration(node) => visitor.visit_css_declaration(arena, node),
            Self::Import(node) => visitor.visit_css_import(arena, node),
            Self::KeyframeBlock(node) => visitor.visit_css_keyframe_block(arena, node),
            Self::MediaRule(node) => visitor.visit_css_media_rule(arena, node),
            Self::StyleRule(node) => visitor.visit_css_style_rule(arena, node),
            Self::Stylesheet(node) => visitor.visit_css_stylesheet(arena, node),
            Self::SupportsRule(node) => visitor.visit_css_supports_rule(arena, node),
        }
    }

    /// Performs a one-time deep conversion to the frozen [`CssNode`] enum.
    /// Called once at the end of evaluation, at the serializer boundary.
    /// `tabs` is libsass's extra NESTED indent (see [`ModifiableCssNode::tabs`]);
    /// only the style/media/supports arms consume it.
    pub fn to_css_node(&self, is_group_end: bool, tabs: u32) -> CssNode<'parse> {
        match self {
            Self::Stylesheet(s) => {
                let mut css = CssStylesheet::new(
                    s.children.iter().map(|c| c.to_css_node()).collect(),
                    s.span,
                );
                css.is_group_end = is_group_end;
                CssNode::Stylesheet(css)
            }
            Self::StyleRule(sr) => {
                let selector = *sr.selector.borrow();
                let mut css =
                    CssStyleRule::new(selector, sr.span, sr.original_selector, sr.from_plain_css);
                css.children = sr.children.iter().map(|c| c.to_css_node()).collect();
                css.is_group_end = is_group_end;
                css.tabs = tabs;
                CssNode::StyleRule(css)
            }
            Self::AtRule(r) => {
                let mut css_r =
                    CssAtRule::new(r.name.clone(), r.span, r.childless, r.value.clone());
                css_r.children = r.children.iter().map(|c| c.to_css_node()).collect();
                css_r.is_group_end = is_group_end;
                CssNode::AtRule(css_r)
            }
            Self::Comment(c) => {
                let mut css = CssComment::new(c.text.clone(), c.span);
                css.is_group_end = is_group_end;
                CssNode::Comment(css)
            }
            Self::Declaration(d) => {
                // `CssDeclaration::new` only fails when `parsed_as_sass_script`
                // is false with a non-string value — but `d` was already
                // validated at construction, so re-freezing is infallible.
                // (Kept fallible-shaped like Dart's constructor; the `expect`
                // documents the invariant.)
                let mut css = CssDeclaration::new(
                    d.name.clone(),
                    d.value.clone(),
                    d.span,
                    d.parsed_as_sass_script,
                    Some(d.value_span_for_map),
                )
                .expect("re-freezing a validated declaration must succeed");
                css.is_group_end = is_group_end;
                CssNode::Declaration(css)
            }
            Self::Import(i) => {
                let mut css = CssImport::new(i.url.clone(), i.span, i.modifiers.clone());
                css.is_group_end = is_group_end;
                CssNode::Import(css)
            }
            Self::KeyframeBlock(k) => {
                let mut css = CssKeyframeBlock::new(k.selector.clone(), k.span);
                css.children = k.children.iter().map(|c| c.to_css_node()).collect();
                css.is_group_end = is_group_end;
                CssNode::KeyframeBlock(css)
            }
            Self::MediaRule(m) => {
                // `CssMediaRule::new` only fails on empty queries — `m` was
                // validated at construction and queries are never cleared, so
                // re-freezing is infallible (see Declaration arm above).
                let mut css = CssMediaRule::new(m.queries.clone(), m.span)
                    .expect("re-freezing a validated media rule must succeed");
                css.children = m.children.iter().map(|c| c.to_css_node()).collect();
                css.is_group_end = is_group_end;
                css.tabs = tabs;
                CssNode::MediaRule(css)
            }
            Self::SupportsRule(s) => {
                let mut css = CssSupportsRule::new(s.condition.clone(), s.span);
                css.children = s.children.iter().map(|c| c.to_css_node()).collect();
                css.is_group_end = is_group_end;
                css.tabs = tabs;
                CssNode::SupportsRule(css)
            }
        }
    }

    /// Returns the children Vec for parent variants, `None` for leaf types.
    pub fn children_ref(&self) -> Option<&Vec<ModifiableCssNode<'parse>>> {
        match self {
            Self::Stylesheet(s) => Some(&s.children),
            Self::StyleRule(sr) => Some(&sr.children),
            Self::AtRule(r) => Some(&r.children),
            Self::KeyframeBlock(k) => Some(&k.children),
            Self::MediaRule(m) => Some(&m.children),
            Self::SupportsRule(sr) => Some(&sr.children),
            _ => None,
        }
    }

    /// Returns mutable children Vec for parent variants, `None` for leaf types.
    pub fn children_mut(&mut self) -> Option<&mut Vec<ModifiableCssNode<'parse>>> {
        match self {
            Self::Stylesheet(s) => Some(&mut s.children),
            Self::StyleRule(sr) => Some(&mut sr.children),
            Self::AtRule(r) => Some(&mut r.children),
            Self::KeyframeBlock(k) => Some(&mut k.children),
            Self::MediaRule(m) => Some(&mut m.children),
            Self::SupportsRule(sr) => Some(&mut sr.children),
            _ => None,
        }
    }

    /// Whether this kind is a parent-capable variant.
    pub fn is_parent(&self) -> bool {
        self.children_ref().is_some()
    }
}

impl<'parse> AstNode<'parse> for ModifiableCssNodeKind<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            Self::Stylesheet(node) => node.span(),
            Self::StyleRule(node) => node.span(),
            Self::AtRule(node) => node.span(),
            Self::Comment(node) => node.span(),
            Self::Declaration(node) => node.span(),
            Self::Import(node) => node.span(),
            Self::KeyframeBlock(node) => node.span(),
            Self::MediaRule(node) => node.span(),
            Self::SupportsRule(node) => node.span(),
        }
    }
}

impl<'parse> fmt::Display for ModifiableCssNodeKind<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stylesheet(node) => write!(f, "{node}"),
            Self::StyleRule(node) => write!(f, "{node}"),
            Self::AtRule(node) => write!(f, "{node}"),
            Self::Comment(node) => write!(f, "{node}"),
            Self::Declaration(node) => write!(f, "{node}"),
            Self::Import(node) => write!(f, "{node}"),
            Self::KeyframeBlock(node) => write!(f, "{node}"),
            Self::MediaRule(node) => write!(f, "{node}"),
            Self::SupportsRule(node) => write!(f, "{node}"),
        }
    }
}

// =============================================================================
// ModifiableCssNodeInner — all mutable state in one RefCell
// =============================================================================

#[derive(Debug)]
pub(super) struct ModifiableCssNodeInner<'parse> {
    pub kind: ModifiableCssNodeKind<'parse>,
    parent: Weak<RefCell<ModifiableCssNodeInner<'parse>>>,
    index_in_parent: usize,
    is_group_end: bool,
    /// Extra NESTED-output indent levels for libsass parity (libsass's
    /// `tabs()`: accumulated `+1` per enclosing props-bearing style rule,
    /// see `Cssize` in `libsass/src/cssize.cpp` and `Output`/`Inspect` in
    /// `libsass/src/output.cpp`/`inspect.cpp`). Stamped by the evaluator on
    /// hoisted style/media/supports nodes; read by the serializer only for
    /// `OutputStyle::Nested`. No Dart counterpart.
    pub tabs: u32,
}

// =============================================================================
// ModifiableCssNode — outer wrapper (cheap Clone via Rc::clone)
// =============================================================================

/// A modifiable CSS node.
///
/// Almost all CSS nodes are the modifiable forms under the covers, but
/// modification is only done within evaluation, so the frozen [`CssNode`]
/// types are used elsewhere to enforce that constraint.
///
/// Cloning is cheap: the handle shares the inner node, so a clone sees the
/// same parent, children, and flags until the tree is restructured.
#[derive(Clone, Debug)]
pub struct ModifiableCssNode<'parse> {
    inner: Rc<RefCell<ModifiableCssNodeInner<'parse>>>,
}

impl<'parse> ModifiableCssNode<'parse> {
    // ------------------------------------------------------------------
    // Constructor
    // ------------------------------------------------------------------

    /// Creates a node with no parent and `is_group_end` unset.
    pub fn new<'compile: 'parse>(
        _arena: &'compile Bump,
        kind: ModifiableCssNodeKind<'parse>,
    ) -> Self {
        ModifiableCssNode {
            inner: Rc::new(RefCell::new(ModifiableCssNodeInner {
                kind,
                parent: Weak::new(),
                index_in_parent: 0,
                is_group_end: false,
                tabs: 0,
            })),
        }
    }

    // ------------------------------------------------------------------
    // Delegates to Kind
    // ------------------------------------------------------------------

    /// Dispatches `self` to the matching [`ModifiableCssVisitor`] method.
    pub fn accept<V: ModifiableCssVisitor<'parse> + ?Sized>(
        &self,
        visitor: &mut V,
    ) -> SassResult<V::Output> {
        self.inner.borrow().kind.accept(visitor)
    }

    /// Freezes `self` and dispatches it to the frozen [`CssVisitor`].
    pub fn accept_as_css<V: CssVisitor<'parse> + ?Sized>(
        &self,
        visitor: &mut V,
    ) -> SassResult<V::Output> {
        let inner = self.inner.borrow();
        inner
            .kind
            .accept_as_css(visitor, inner.is_group_end, inner.tabs)
    }

    /// Deep-copies `self` (and its subtree) through `visitor`.
    pub fn clone_from<'compile: 'parse, V: CloneCssVisitor<'parse> + ?Sized>(
        &self,
        arena: &'compile Bump,
        visitor: &V,
    ) -> SassResult<ModifiableCssNode<'parse>> {
        self.inner.borrow().kind.clone_from(arena, visitor)
    }

    /// Freezes `self` (and its subtree) to the immutable [`CssNode`] form.
    pub fn to_css_node(&self) -> CssNode<'parse> {
        let inner = self.inner.borrow();
        inner.kind.to_css_node(inner.is_group_end, inner.tabs)
    }

    // ------------------------------------------------------------------
    // Parent chain: getters / setters
    // ------------------------------------------------------------------

    /// The parent node, if this node is attached to a tree.
    pub fn parent(&self) -> Option<ModifiableCssNode<'parse>> {
        self.inner
            .borrow()
            .parent
            .upgrade()
            .map(|rc| ModifiableCssNode { inner: rc })
    }

    /// Sets the back-pointer directly, without touching any child list.
    /// Prefer [`add_child`](Self::add_child) when attaching through a parent.
    pub fn set_parent(&self, parent: Option<&ModifiableCssNode<'parse>>) {
        self.inner.borrow_mut().parent =
            parent.map(|p| Rc::downgrade(&p.inner)).unwrap_or_default();
    }

    pub fn set_index_in_parent(&self, i: usize) {
        self.inner.borrow_mut().index_in_parent = i;
    }

    /// The index of `self` in the parent's child list, which makes
    /// [`remove`](Self::remove) efficient.
    pub fn index_in_parent(&self) -> usize {
        self.inner.borrow().index_in_parent
    }

    /// Whether this node closes a root-level group (serializer emits an extra
    /// blank line after it). Set by the evaluator, copied across by clone,
    /// transferred at freeze via [`to_css_node`](Self::to_css_node).
    pub fn set_is_group_end(&self, v: bool) {
        self.inner.borrow_mut().is_group_end = v;
    }

    pub fn is_group_end(&self) -> bool {
        self.inner.borrow().is_group_end
    }

    /// Extra NESTED-output indent levels (see `ModifiableCssNodeInner::tabs`).
    /// Stamped by the evaluator; the serializer reads it only for
    /// `OutputStyle::Nested`.
    pub fn add_tabs(&self, n: u32) {
        let mut inner = self.inner.borrow_mut();
        inner.tabs = inner.tabs.saturating_add(n);
    }

    pub fn tabs(&self) -> u32 {
        self.inner.borrow().tabs
    }

    /// Matches Dart: `CssNode.isInvisible` via `_IsInvisibleVisitor`
    /// (`ast/css/node.dart`): style rules check the selector + children,
    /// at-rules are never invisible, and every other parent (stylesheet,
    /// keyframe block, media/supports rules) is invisible iff all children
    /// are. Leaf types (comment, declaration, import) are visible.
    pub fn is_invisible(&self) -> bool {
        match &self.inner.borrow().kind {
            ModifiableCssNodeKind::StyleRule(sr) => {
                sr.selector.borrow().is_invisible() || sr.children.iter().all(|c| c.is_invisible())
            }
            ModifiableCssNodeKind::AtRule(_) => false,
            ModifiableCssNodeKind::Stylesheet(s) => s.children.iter().all(|c| c.is_invisible()),
            ModifiableCssNodeKind::KeyframeBlock(k) => k.children.iter().all(|c| c.is_invisible()),
            ModifiableCssNodeKind::MediaRule(m) => m.children.iter().all(|c| c.is_invisible()),
            ModifiableCssNodeKind::SupportsRule(s) => s.children.iter().all(|c| c.is_invisible()),
            ModifiableCssNodeKind::Comment(_)
            | ModifiableCssNodeKind::Declaration(_)
            | ModifiableCssNodeKind::Import(_) => false,
        }
    }

    /// Whether this is a parent-capable variant.
    pub fn is_parent(&self) -> bool {
        self.inner.borrow().kind.is_parent()
    }

    /// Whether this is a style rule node.
    pub fn is_style_rule(&self) -> bool {
        matches!(
            &self.inner.borrow().kind,
            ModifiableCssNodeKind::StyleRule(_)
        )
    }

    /// Whether this is a media rule node.
    pub fn is_media_rule(&self) -> bool {
        matches!(
            &self.inner.borrow().kind,
            ModifiableCssNodeKind::MediaRule(_)
        )
    }

    /// Owned children — each element is an Rc clone. `None` for leaf types.
    pub fn children(&self) -> Option<Vec<ModifiableCssNode<'parse>>> {
        self.inner.borrow().kind.children_ref().map(|v| v.to_vec())
    }

    /// Returns the last child without cloning the entire children Vec.
    /// `None` for leaf types or empty children lists.
    pub(crate) fn last_child(&self) -> Option<ModifiableCssNode<'parse>> {
        self.inner
            .borrow()
            .kind
            .children_ref()
            .and_then(|v| v.last().cloned())
    }

    /// Returns the number of children without cloning the Vec.
    /// Returns 0 for leaf types.
    pub(crate) fn children_len(&self) -> usize {
        self.inner
            .borrow()
            .kind
            .children_ref()
            .map_or(0, |v| v.len())
    }

    /// Whether this is a childless at-rule.
    pub fn is_childless(&self) -> bool {
        match &self.inner.borrow().kind {
            ModifiableCssNodeKind::AtRule(r) => r.childless,
            _ => false,
        }
    }

    /// Returns a reference to the inner kind.
    pub(crate) fn kind(&self) -> Ref<'_, ModifiableCssNodeKind<'parse>> {
        Ref::map(self.inner.borrow(), |i| &i.kind)
    }

    // ------------------------------------------------------------------
    // Parent chain: tree manipulation
    // ------------------------------------------------------------------

    /// Adds `child` as a child of this node, stamping its parent and index.
    pub fn add_child(&self, child: &ModifiableCssNode<'parse>) -> SassResult<()> {
        // Dart `ModifiableCssAtRule.addChild` asserts `!isChildless`, and
        // leaf types have no `addChild` at all. Guard both: error on leaves
        // and on childless at-rules instead of silently succeeding.
        let idx = {
            let inner = self.inner.borrow();
            match &inner.kind {
                ModifiableCssNodeKind::AtRule(r) if r.childless => {
                    return Err(Box::new(SassError::Script {
                        message: "addChild can't be called for a childless at-rule.".into(),
                        argument_name: None,
                    }));
                }
                _ => match inner.kind.children_ref() {
                    Some(c) => c.len(),
                    None => {
                        return Err(Box::new(SassError::Script {
                            message: "addChild can't be called for a leaf node.".into(),
                            argument_name: None,
                        }));
                    }
                },
            }
        };
        child.set_index_in_parent(idx);
        child.set_parent(Some(self));
        let mut inner = self.inner.borrow_mut();
        if let Some(c) = inner.kind.children_mut() {
            c.push(child.clone());
        }
        Ok(())
    }

    /// Whether this node has a visible sibling after it.
    pub fn has_following_sibling(&self) -> bool {
        let parent_node = match self.parent() {
            Some(p) => p,
            None => return false,
        };
        let parent_inner = parent_node.inner.borrow();
        let idx = self.index_in_parent();
        match parent_inner.kind.children_ref() {
            Some(children) => children.iter().skip(idx + 1).any(|c| !c.is_invisible()),
            None => false,
        }
    }

    /// Removes `self` from the parent's child list, reindexing the siblings
    /// after it.
    ///
    /// Errors when the node has no parent.
    pub fn remove(&self) -> SassResult<()> {
        // Dart `ModifiableCssNode.remove` throws StateError when the parent
        // is null ("Can't remove a node without a parent.").
        let parent_node = match self.parent() {
            Some(p) => p,
            None => {
                return Err(Box::new(SassError::Script {
                    message: "Can't remove a node without a parent.".into(),
                    argument_name: None,
                }));
            }
        };
        let idx = self.index_in_parent();

        {
            let mut parent_inner = parent_node.inner.borrow_mut();
            if let Some(children) = parent_inner.kind.children_mut() {
                children.remove(idx);
                for (i, child) in children.iter().enumerate().skip(idx) {
                    child.set_index_in_parent(i);
                }
            }
        }

        self.set_parent(None);
        Ok(())
    }

    /// Destructively removes all elements from the child list.
    pub fn clear_children(&self) -> SassResult<()> {
        // Dart `ModifiableCssParentNode.clearChildren` nulls both the parent
        // and the index of each removed child.
        let mut inner = self.inner.borrow_mut();
        let taken = match inner.kind.children_mut() {
            Some(c) => std::mem::take(c),
            None => return Ok(()),
        };
        for child in &taken {
            child.set_parent(None);
            // `usize::MAX` is the unset sentinel: indices are always valid
            // positions while attached (`add_child` re-stamps on insert).
            child.set_index_in_parent(usize::MAX);
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Parent chain: semantic methods (from Dart's ModifiableCssParentNode)
    // ------------------------------------------------------------------

    /// Matches Dart: ModifiableCssParentNode.equalsIgnoringChildren (per-type impls).
    /// Only meaningful for parent types; leaf types return false.
    pub fn equals_ignoring_children(&self, other: &ModifiableCssNode<'parse>) -> bool {
        let a = self.inner.borrow();
        let b = other.inner.borrow();
        match (&a.kind, &b.kind) {
            (ModifiableCssNodeKind::Stylesheet(_), ModifiableCssNodeKind::Stylesheet(_)) => true,
            (ModifiableCssNodeKind::StyleRule(sr_a), ModifiableCssNodeKind::StyleRule(sr_b)) => {
                // Dart `ModifiableCssStyleRule.equalsIgnoringChildren` compares
                // `other.selector == selector` — `SelectorList` value equality,
                // not box/allocation identity.
                sr_a.selector_value() == sr_b.selector_value()
            }
            (ModifiableCssNodeKind::AtRule(ra), ModifiableCssNodeKind::AtRule(rb)) => {
                ra.name == rb.name && ra.value == rb.value && ra.childless == rb.childless
            }
            (
                ModifiableCssNodeKind::KeyframeBlock(ka),
                ModifiableCssNodeKind::KeyframeBlock(kb),
            ) => ka.selector.value == kb.selector.value,
            (ModifiableCssNodeKind::MediaRule(ma), ModifiableCssNodeKind::MediaRule(mb)) => {
                ma.queries == mb.queries
            }
            (ModifiableCssNodeKind::SupportsRule(sa), ModifiableCssNodeKind::SupportsRule(sb)) => {
                sa.condition == sb.condition
            }
            _ => false,
        }
    }

    /// Matches Dart: ModifiableCssParentNode.copyWithoutChildren (per-type impls).
    /// Shallow copy with empty children, no parent. Leaf types clone all fields.
    pub fn copy_without_children<'compile: 'parse>(&self, arena: &'compile Bump) -> Self {
        let inner = self.inner.borrow();
        let kind = match &inner.kind {
            ModifiableCssNodeKind::Stylesheet(s) => {
                ModifiableCssNodeKind::Stylesheet(ModifiableCssStylesheet::new(s.span))
            }
            ModifiableCssNodeKind::StyleRule(sr) => {
                // Dart `ModifiableCssStyleRule.copyWithoutChildren` omits
                // `fromPlainCss` (defaults to false) — copies merge as
                // non-plain rules on re-evaluation.
                ModifiableCssNodeKind::StyleRule(ModifiableCssStyleRule::new(
                    Rc::clone(&sr.selector),
                    sr.span,
                    Some(sr.original_selector),
                    false,
                ))
            }
            ModifiableCssNodeKind::AtRule(r) => ModifiableCssNodeKind::AtRule(
                ModifiableCssAtRule::new(r.name.clone(), r.span, r.childless, r.value.clone()),
            ),
            ModifiableCssNodeKind::KeyframeBlock(k) => ModifiableCssNodeKind::KeyframeBlock(
                ModifiableCssKeyframeBlock::new(k.selector.clone(), k.span),
            ),
            ModifiableCssNodeKind::MediaRule(m) => ModifiableCssNodeKind::MediaRule(
                // Infallible: `m` was validated at construction and queries
                // are never cleared (see the `to_css_node` MediaRule arm).
                ModifiableCssMediaRule::new(m.queries.clone(), m.span)
                    .expect("copying a validated media rule must succeed"),
            ),
            ModifiableCssNodeKind::SupportsRule(sr) => ModifiableCssNodeKind::SupportsRule(
                ModifiableCssSupportsRule::new(sr.condition.clone(), sr.span),
            ),
            ModifiableCssNodeKind::Comment(c) => ModifiableCssNodeKind::Comment(c.clone()),
            ModifiableCssNodeKind::Declaration(d) => ModifiableCssNodeKind::Declaration(d.clone()),
            ModifiableCssNodeKind::Import(i) => ModifiableCssNodeKind::Import(i.clone()),
        };
        // `tabs` is intrinsic (source depth), unlike `is_group_end` which is
        // positional (last-child status changes on split) — the copy is the
        // same logical node, so it inherits the stamp.
        let tabs = inner.tabs;
        let out = Self::new(arena, kind);
        out.add_tabs(tabs);
        out
    }
}

impl<'parse> AstNode<'parse> for ModifiableCssNode<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        self.inner.borrow().kind.span()
    }
}

impl<'parse> fmt::Display for ModifiableCssNode<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.borrow().kind.fmt(f)
    }
}

impl<'parse> PartialEq for ModifiableCssNode<'parse> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }
}

impl<'parse> Eq for ModifiableCssNode<'parse> {}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ast_css_value::CssValue;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::common::source_span_file_source::FileSource;
    use crate::parse::selector_parse::SelectorParser;
    use crate::selector::class::ClassSelector;
    use crate::selector::complex::ComplexSelector;
    use crate::selector::complex_component::ComplexSelectorComponent;
    use crate::selector::compound::CompoundSelector;
    use crate::selector::list::SelectorList;
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

    fn make_style_rule<'compile, 'parse>(
        arena: &'compile Bump,
        from_plain_css: bool,
    ) -> ModifiableCssNode<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let sel = SimpleSelector::Class(ClassSelector::new(".foo".into(), BOGUS_SPAN));
        let compound = CompoundSelector::new(vec![sel], BOGUS_SPAN).unwrap();
        let complex = ComplexSelector::new(
            vec![],
            vec![ComplexSelectorComponent::new(
                std::boxed::Box::new(compound),
                vec![],
                BOGUS_SPAN,
            )],
            BOGUS_SPAN,
            false,
        )
        .unwrap();
        let list = SelectorList::new(arena, vec![complex], BOGUS_SPAN).unwrap();
        let rc = Rc::new(RefCell::new(list));
        ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::StyleRule(ModifiableCssStyleRule::new(
                rc,
                make_span(arena, ""),
                None,
                from_plain_css,
            )),
        )
    }

    fn make_declaration<'compile, 'parse>(arena: &'compile Bump) -> ModifiableCssNode<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let name = make_val(arena, "color");
        let span = make_span(arena, "red");
        let s = SassString::new("red", false);
        let val = CssValue::new(Value::new_with_arena(arena, ValueKind::String(s)), span);
        ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::Declaration(
                ModifiableCssDeclaration::new(name, val, make_span(arena, ""), true, None).unwrap(),
            ),
        )
    }

    // ----- existing Kind tests (unchanged) -----

    #[test]
    fn test_to_css_node_comment() {
        let arena = Bump::new();
        let span = make_span(&arena, "/* test */");
        let mc = ModifiableCssComment::new("/* test */".into(), span);
        let node = ModifiableCssNodeKind::Comment(mc);
        let css = node.to_css_node(false, 0);
        match css {
            CssNode::Comment(c) => {
                assert_eq!(c.text, "/* test */");
                assert!(!c.is_preserved);
            }
            _ => panic!("expected CssNode::Comment"),
        }
    }

    #[test]
    fn test_to_css_node_preserved_comment() {
        let arena = Bump::new();
        let span = make_span(&arena, "/*! test */");
        let mc = ModifiableCssComment::new("/*! test */".into(), span);
        let node = ModifiableCssNodeKind::Comment(mc);
        let css = node.to_css_node(false, 0);
        match css {
            CssNode::Comment(c) => {
                assert!(c.is_preserved);
            }
            _ => panic!("expected CssNode::Comment"),
        }
    }

    #[test]
    fn test_to_css_node_declaration() {
        let arena = Bump::new();
        let span = make_span(&arena, "color: red;");
        let name = make_val(&arena, "color");
        let s = SassString::new("red", false);
        let val_span = make_span(&arena, "red");
        let val = CssValue::new(
            Value::new_with_arena(&arena, ValueKind::String(s)),
            val_span,
        );
        let md = ModifiableCssDeclaration::new(name, val, span, true, None).unwrap();
        let node = ModifiableCssNodeKind::Declaration(md);
        let css = node.to_css_node(false, 0);
        match css {
            CssNode::Declaration(d) => {
                assert_eq!(d.name.value, "color");
            }
            _ => panic!("expected CssNode::Declaration"),
        }
    }

    #[test]
    fn test_to_css_node_import() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let url = make_val(&arena, "\"foo.css\"");
        let mi = ModifiableCssImport::from_css_import(&CssImport::new(url, span, None));
        let node = ModifiableCssNodeKind::Import(mi);
        let css = node.to_css_node(false, 0);
        match css {
            CssNode::Import(i) => {
                assert_eq!(i.url.value, "\"foo.css\"");
            }
            _ => panic!("expected CssNode::Import"),
        }
    }

    #[test]
    fn test_to_css_node_stylesheet() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let ss = ModifiableCssStylesheet::new(span);
        let node = ModifiableCssNodeKind::Stylesheet(ss);
        let css = node.to_css_node(false, 0);
        match css {
            CssNode::Stylesheet(s) => {
                assert!(s.children.is_empty());
            }
            _ => panic!("expected CssNode::Stylesheet"),
        }
    }

    #[test]
    fn test_to_css_node_at_rule() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let name = make_val(&arena, "media");
        let r = ModifiableCssAtRule::new(name, span, false, None);
        let node = ModifiableCssNodeKind::AtRule(r);
        let css = node.to_css_node(false, 0);
        match css {
            CssNode::AtRule(r) => {
                assert_eq!(r.name.value, "media");
            }
            _ => panic!("expected CssNode::AtRule"),
        }
    }

    #[test]
    fn test_modifiable_node_children() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let ss = ModifiableCssStylesheet::new(span);
        let node = ModifiableCssNode::new(&arena, ModifiableCssNodeKind::Stylesheet(ss));
        assert_eq!(node.children().unwrap().len(), 0);
        assert!(!node.is_childless());
    }

    // ----- new parent chain tests -----

    #[test]
    fn test_new_node_no_parent() {
        let arena = Bump::new();
        let node = make_stylesheet(&arena);
        assert!(node.parent().is_none());
    }

    #[test]
    fn test_set_parent_and_retrieve() {
        let arena = Bump::new();
        let parent = make_stylesheet(&arena);
        let child = make_declaration(&arena);

        child.set_parent(Some(&parent));
        assert!(child.parent().is_some());
    }

    #[test]
    fn test_set_parent_none_clears() {
        let arena = Bump::new();
        let parent = make_stylesheet(&arena);
        let child = make_declaration(&arena);

        child.set_parent(Some(&parent));
        assert!(child.parent().is_some());
        child.set_parent(None);
        assert!(child.parent().is_none());
    }

    #[test]
    fn test_add_child_parent_and_index() {
        let arena = Bump::new();
        let parent = make_stylesheet(&arena);
        let child = make_declaration(&arena);

        parent.add_child(&child).unwrap();
        assert!(child.parent().is_some());
        assert_eq!(child.index_in_parent(), 0);
    }

    #[test]
    fn test_add_child_two_indices() {
        let arena = Bump::new();
        let parent = make_stylesheet(&arena);
        let a = make_declaration(&arena);
        let b = make_declaration(&arena);

        parent.add_child(&a).unwrap();
        parent.add_child(&b).unwrap();

        assert_eq!(a.index_in_parent(), 0);
        assert_eq!(b.index_in_parent(), 1);
    }

    #[test]
    fn test_has_following_sibling_first() {
        let arena = Bump::new();
        let parent = make_stylesheet(&arena);
        let a = make_declaration(&arena);
        let b = make_declaration(&arena);

        parent.add_child(&a).unwrap();
        parent.add_child(&b).unwrap();

        assert!(a.has_following_sibling());
    }

    #[test]
    fn test_has_following_sibling_last() {
        let arena = Bump::new();
        let parent = make_stylesheet(&arena);
        let a = make_declaration(&arena);
        let b = make_declaration(&arena);

        parent.add_child(&a).unwrap();
        parent.add_child(&b).unwrap();

        assert!(!b.has_following_sibling());
    }

    #[test]
    fn test_has_following_sibling_root() {
        let arena = Bump::new();
        let root = make_stylesheet(&arena);
        assert!(!root.has_following_sibling());
    }

    #[test]
    fn test_remove_from_middle_reindexes() {
        let arena = Bump::new();
        let parent = make_stylesheet(&arena);
        let a = make_declaration(&arena);
        let b = make_declaration(&arena);
        let c = make_declaration(&arena);

        parent.add_child(&a).unwrap();
        parent.add_child(&b).unwrap();
        parent.add_child(&c).unwrap();

        {
            assert_eq!(parent.children().unwrap().len(), 3);
        }

        b.remove().unwrap();

        {
            assert_eq!(parent.children().unwrap().len(), 2);
        }
        assert!(b.parent().is_none());
        assert_eq!(c.index_in_parent(), 1);
    }

    #[test]
    fn test_remove_idempotent() {
        let arena = Bump::new();
        let parent = make_stylesheet(&arena);
        let child = make_declaration(&arena);
        parent.add_child(&child).unwrap();

        child.remove().unwrap();
        // second remove errors like Dart's StateError (parent is null).
        let err = child.remove().unwrap_err();
        match *err {
            SassError::Script { message, .. } => {
                assert!(message.contains("without a parent"), "got {message:?}")
            }
            other => panic!("expected Script, got {other:?}"),
        }
        assert!(child.parent().is_none());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_remove_without_parent_errors() {
        // `remove()` on a parentless node must error (Dart
        // StateError), not silently succeed.
        let arena = Bump::new();
        let orphan = make_declaration(&arena);
        assert!(orphan.parent().is_none());
        let err = orphan.remove().unwrap_err();
        match *err {
            SassError::Script { message, .. } => {
                assert_eq!(message, "Can't remove a node without a parent.")
            }
            other => panic!("expected Script, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_add_child_guards() {
        // `add_child` on leaf types and on childless
        // at-rules must error (Dart: no method / assert !isChildless).
        let arena = Bump::new();
        let leaf = make_declaration(&arena);
        let child = make_declaration(&arena);
        assert!(
            leaf.add_child(&child).is_err(),
            "add_child on a leaf must error"
        );
        let span = make_span(&arena, "@import x;");
        let name = make_val(&arena, "import");
        let childless = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::AtRule(ModifiableCssAtRule::new(name, span, true, None)),
        );
        assert!(
            childless.add_child(&child).is_err(),
            "add_child on a childless at-rule must error"
        );
        // Non-childless at-rule still accepts children.
        let open = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::AtRule(ModifiableCssAtRule::new(
                make_val(&arena, "media"),
                span,
                false,
                None,
            )),
        );
        open.add_child(&child).unwrap();
        assert_eq!(open.children().unwrap().len(), 1);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_clear_children_resets_index() {
        // `clear_children` must reset `index_in_parent`
        // (Dart nulls both parent and index).
        let arena = Bump::new();
        let parent = make_stylesheet(&arena);
        let a = make_declaration(&arena);
        parent.add_child(&a).unwrap();
        assert_eq!(a.index_in_parent(), 0);
        parent.clear_children().unwrap();
        assert!(a.parent().is_none());
        assert_eq!(
            a.index_in_parent(),
            usize::MAX,
            "cleared child index must reset to the unset sentinel"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_stylesheet_keyframe_invisible() {
        // Empty modifiable stylesheet / keyframe block are
        // invisible (Dart shared `CssNode.isInvisible` via EveryCssVisitor —
        // all-children-invisible, vacuously true when empty).
        let arena = Bump::new();
        assert!(
            make_stylesheet(&arena).is_invisible(),
            "empty stylesheet must be invisible"
        );
        let span = make_span(&arena, "10%");
        let sel_span = make_span(&arena, "10%");
        let sel = CssValue::new(vec!["10%".into()], sel_span);
        let block = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::KeyframeBlock(ModifiableCssKeyframeBlock::new(sel, span)),
        );
        assert!(
            block.is_invisible(),
            "empty keyframe block must be invisible"
        );
    }

    #[test]
    fn test_clear_children_detaches() {
        let arena = Bump::new();
        let parent = make_stylesheet(&arena);
        let a = make_declaration(&arena);
        let b = make_declaration(&arena);

        parent.add_child(&a).unwrap();
        parent.add_child(&b).unwrap();
        assert!(a.parent().is_some());
        assert!(b.parent().is_some());

        parent.clear_children().unwrap();
        {
            assert!(parent.children().unwrap().is_empty());
        }
        assert!(a.parent().is_none());
        assert!(b.parent().is_none());
    }

    #[test]
    fn test_is_group_end_roundtrip() {
        let arena = Bump::new();
        let node = make_declaration(&arena);

        assert!(!node.is_group_end());
        node.set_is_group_end(true);
        assert!(node.is_group_end());
        node.set_is_group_end(false);
        assert!(!node.is_group_end());
    }

    #[test]
    fn test_equals_ignoring_children_same_selector() {
        let arena = Bump::new();
        let a = make_style_rule(&arena, false);

        // Children don't matter: inserting a child must not change equality.
        let a2 = make_style_rule(&arena, false);
        let child = make_declaration(&arena);
        a.add_child(&child).unwrap();
        assert!(
            a.equals_ignoring_children(&a2),
            "two separately-built but value-equal `.foo` rules must compare equal \
             (Dart ModifiableCssStyleRule.equalsIgnoringChildren uses SelectorList ==)"
        );

        // Same node (shared inner) still compares equal.
        let cloned = a.clone();
        assert!(a.equals_ignoring_children(&cloned));

        // Different kinds: false
        let decl = make_declaration(&arena);
        assert!(!a.equals_ignoring_children(&decl));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_style_rule_value_equality() {
        // Two separately-parsed but equal `.a` rules →
        // expect `true` (Dart ModifiableCssStyleRule.equalsIgnoringChildren
        // uses SelectorList value equality, not box identity).
        let arena = Bump::new();
        let span = make_span(&arena, "");
        fn parse_list<'compile: 'parse, 'parse>(
            arena: &'compile Bump,
            text: &str,
        ) -> SelectorList<'parse>
        where
            'parse: 'compile,
        {
            // Note: `text` must outlive the parse; the literals below are
            // 'static so the returned refs are valid for the arena lifetime.
            let fs: &'parse FileSource<'parse> = FileSource::new_in(arena, text, None);
            let mut p = SelectorParser::new(arena, fs);
            p.parse().unwrap()
        }
        fn mk<'compile: 'parse, 'parse>(
            arena: &'compile Bump,
            sel: SelectorList<'parse>,
            span: FileSpan<'parse>,
        ) -> ModifiableCssNode<'parse>
        where
            'parse: 'compile,
        {
            ModifiableCssNode::new(
                arena,
                ModifiableCssNodeKind::StyleRule(ModifiableCssStyleRule::new(
                    Rc::new(RefCell::new(sel)),
                    span,
                    None,
                    false,
                )),
            )
        }
        let a = mk(&arena, parse_list(&arena, ".a"), span);
        let b = mk(&arena, parse_list(&arena, ".a"), span);
        assert!(
            a.equals_ignoring_children(&b) && b.equals_ignoring_children(&a),
            "separately-parsed `.a` rules must compare equal"
        );
        let c = mk(&arena, parse_list(&arena, ".b"), span);
        assert!(
            !a.equals_ignoring_children(&c),
            "different selectors must compare unequal"
        );
    }

    #[test]
    fn test_copy_without_children_empty() {
        let arena = Bump::new();
        let rule = make_style_rule(&arena, false);
        let child = make_declaration(&arena);
        rule.add_child(&child).unwrap();

        let copy = rule.copy_without_children(&arena);
        assert!(copy.children().unwrap().is_empty());
        assert!(copy.parent().is_none());
    }

    #[test]
    fn test_copy_without_children_drops_from_plain_css() {
        // Matches Dart: `ModifiableCssStyleRule.copyWithoutChildren` omits
        // `fromPlainCss` (defaults to false), so copies merge as non-plain
        // rules on re-evaluation.
        let arena = Bump::new();
        let rule = make_style_rule(&arena, true);
        let copy = rule.copy_without_children(&arena);
        assert!(
            matches!(
                &*copy.kind(),
                ModifiableCssNodeKind::StyleRule(sr) if !sr.from_plain_css
            ),
            "copy must drop from_plain_css"
        );
    }
}
