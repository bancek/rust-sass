// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/async_evaluate.dart (CSS-visitor sections,
//   visitCss* at async_evaluate.dart:4020-4322; duplicated verbatim in the
//   generated evaluate.dart:4019-4321)
// go-source: go/eval/evaluate_css.go

// CSS re-evaluation phase — free functions (no Visitor trait impls).
// Matches Dart: the `visitCss*` block intro (evaluate.dart:4010-4018) — when a
// module is loaded via `@import` of a stylesheet that itself contains `@use`,
// or via the `load-css()` function, it is first compiled to CSS (it must be
// evaluated exactly once and may be reused elsewhere), then that CSS is
// executed more or less as though it were Sass, since a nested `@import` can't
// be injected into the output tree as-is.

use crate::ast::css::at_rule::ModifiableCssAtRule;
use crate::ast::css::comment::ModifiableCssComment;
use crate::ast::css::declaration::ModifiableCssDeclaration;
use crate::ast::css::import::ModifiableCssImport;
use crate::ast::css::keyframe_block::ModifiableCssKeyframeBlock;
use crate::ast::css::media_rule::ModifiableCssMediaRule;
use crate::ast::css::supports_rule::ModifiableCssSupportsRule;
use crate::extend::StoreBox;
use crate::unvendor::unvendor;
use std::cell::RefCell;
use std::rc::Rc;

use bumpalo::Bump;

use crate::ast::css::at_rule::CssAtRule;
use crate::ast::css::comment::CssComment;
use crate::ast::css::declaration::CssDeclaration;
use crate::ast::css::import::CssImport;
use crate::ast::css::keyframe_block::CssKeyframeBlock;
use crate::ast::css::media_rule::CssMediaRule;
use crate::ast::css::modifiable_node::{ModifiableCssNode, ModifiableCssNodeKind};
use crate::ast::css::node::CssNode;
use crate::ast::css::style_rule::{CssStyleRule, ModifiableCssStyleRule};
use crate::ast::css::stylesheet::CssStylesheet;
use crate::ast::css::supports_rule::CssSupportsRule;
use crate::common::exception::SassResult;
use crate::eval::helpers;
use crate::eval::{EvalConfig, EvalState};

// ===========================================================================
// evaluate_css_stylesheet
// Dart: visitCssStylesheet (evaluate.dart:4271-4275) — iterates children via
//   `accept`; the Rust free function below is the same traversal (see
//   `dispatch_css_child` for the per-node dispatch).
// ===========================================================================

/// Re-evaluates an already-compiled CSS stylesheet node by node.
///
/// [`dispatch_css_child`] handles each child; the stylesheet itself adds no
/// output node.
#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_css_stylesheet<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssStylesheet<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    for child in &node.children {
        box_rec_in!(dispatch_css_child(config, state, arena, child), arena).await?;
    }
    Ok(())
}

/// Dispatches one frozen CSS child to its `evaluate_css_*` re-evaluation.
///
/// Rust-only split of Dart's `child.accept(this)`: one `match` arm per
/// [`CssNode`] variant so recursion stays in free functions.
#[rust_sass_macros::maybe_async]
async fn dispatch_css_child<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    child: &CssNode<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    match child {
        CssNode::Stylesheet(n) => evaluate_css_stylesheet(config, state, arena, n).await,
        CssNode::StyleRule(n) => {
            box_rec_in!(evaluate_css_style_rule(config, state, arena, n), arena).await
        }
        CssNode::AtRule(n) => {
            box_rec_in!(evaluate_css_at_rule(config, state, arena, n), arena).await
        }
        CssNode::Comment(n) => evaluate_css_comment(config, state, arena, n),
        CssNode::Declaration(n) => evaluate_css_declaration(config, state, arena, n),
        CssNode::Import(n) => evaluate_css_import(config, state, arena, n),
        CssNode::KeyframeBlock(n) => {
            box_rec_in!(evaluate_css_keyframe_block(config, state, arena, n), arena).await
        }
        CssNode::MediaRule(n) => {
            box_rec_in!(evaluate_css_media_rule(config, state, arena, n), arena).await
        }
        CssNode::SupportsRule(n) => {
            box_rec_in!(evaluate_css_supports_rule(config, state, arena, n), arena).await
        }
    }
}

// ===========================================================================
// evaluate_css_comment
// Dart: visitCssComment (evaluate.dart:4084-4095). NOTE there (mirrored here
//   in the surrounding code): largely duplicated in `visitLoudComment` —
//   most changes should be mirrored there.
// ===========================================================================

/// Copies a frozen CSS comment into the modifiable output tree.
///
/// Matches Dart: comments may appear between CSS imports, so when the current
/// parent is the root and [`EvalState::end_of_imports`] still points at the
/// end of the root's children it is advanced past the new comment.
pub(crate) fn evaluate_css_comment<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssComment<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let parent_is_root = match (&state.parent, &state.root) {
        (Some(p), Some(r)) => p == r,
        _ => false,
    };
    if parent_is_root {
        let root_len = state
            .root
            .as_ref()
            .unwrap()
            .children()
            .map(|c| c.len())
            .unwrap_or(0);
        if state.end_of_imports == root_len {
            state.end_of_imports += 1;
        }
    }

    helpers::copy_parent_after_sibling(arena, state)?;
    let comment = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::Comment(ModifiableCssComment::new(node.text.clone(), node.span)),
    );
    helpers::add_child(arena, state, &comment, None)
}

// ===========================================================================
// evaluate_css_import
// Dart: visitCssImport (evaluate.dart:4110-4128). NOTE there (mirrored here
//   in the surrounding code): largely duplicated in `_visitStaticImport` —
//   most changes should be mirrored there.
// ===========================================================================

/// Copies a frozen CSS `@import` into the modifiable output tree.
///
/// Matches Dart: a nested import is copied into the current parent; a
/// root-level import in order extends [`EvalState::end_of_imports`], while an
/// out-of-order root import is deferred into
/// [`EvalState::out_of_order_imports`].
pub(crate) fn evaluate_css_import<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssImport<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let import = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::Import(ModifiableCssImport::from_css_import(node)),
    );

    let parent_is_root = match (&state.parent, &state.root) {
        (Some(p), Some(r)) => p == r,
        _ => false,
    };

    if !parent_is_root {
        helpers::copy_parent_after_sibling(arena, state)?;
        if let Some(ref parent) = state.parent {
            parent.add_child(&import)?;
        }
    } else {
        let root_len = state
            .root
            .as_ref()
            .unwrap()
            .children()
            .map(|c| c.len())
            .unwrap_or(0);
        if state.end_of_imports == root_len {
            if let Some(ref root) = state.root {
                root.add_child(&import)?;
            }
            state.end_of_imports += 1;
        } else {
            state
                .out_of_order_imports
                .push(ModifiableCssImport::from_css_import(node));
        }
    }
    Ok(())
}

// ===========================================================================
// evaluate_css_declaration
// Dart: visitCssDeclaration (evaluate.dart:4097-4108) — copies the parent
//   after a following sibling, then adds a `ModifiableCssDeclaration` built
//   from the frozen node (including `parsedAsSassScript`/`valueSpanForMap`).
// ===========================================================================

/// Copies a frozen CSS declaration into the modifiable output tree.
pub(crate) fn evaluate_css_declaration<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssDeclaration<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    helpers::copy_parent_after_sibling(arena, state)?;
    let child = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::Declaration(ModifiableCssDeclaration::new(
            node.name.clone(),
            node.value.clone(),
            node.span,
            node.parsed_as_sass_script,
            Some(node.value_span_for_map),
        )?),
    );
    helpers::add_child(arena, state, &child, None)
}

// ===========================================================================
// evaluate_css_keyframe_block
// Dart: visitCssKeyframeBlock (evaluate.dart:4130-4145). NOTE there (mirrored
//   here in the surrounding code): largely duplicated in `visitStyleRule` —
//   most changes should be mirrored there.
// ===========================================================================

/// Re-evaluates a frozen keyframe block under a fresh modifiable parent.
///
/// Matches Dart: children run inside [`with_parent`] with a
/// `through`-predicate that only passes style rules, and `scope_when` false
/// (no new environment scope for CSS children).
#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_css_keyframe_block<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssKeyframeBlock<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let rule = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::KeyframeBlock(ModifiableCssKeyframeBlock::new(
            node.selector.clone(),
            node.span,
        )),
    );

    let through: &dyn Fn(&ModifiableCssNode<'parse>) -> bool =
        &|n: &ModifiableCssNode<'parse>| -> bool { n.is_style_rule() };

    helpers::with_parent(
        arena,
        config,
        state,
        rule,
        Some(through),
        Some(false),
        async |c, s| {
            for child in &node.children {
                dispatch_css_child(c, s, arena, child).await?;
            }
            Ok(())
        },
    )
    .await
}

// ===========================================================================
// evaluate_css_at_rule
// Dart: visitCssAtRule (evaluate.dart:4019-4082). NOTE there (mirrored here
//   in the surrounding code): largely duplicated in `visitAtRule` — most
//   changes should be mirrored there.
// ===========================================================================

/// Re-evaluates a frozen CSS at-rule, bubbling it through style-rule parents.
///
/// Matches Dart: at-rules are rejected inside nested declarations; a
/// childless rule is copied straight into the current parent. Otherwise the
/// `in_keyframes`/`in_unknown_at_rule` flags are set from the (unvendored)
/// rule name and restored afterwards, and — unless plain-CSS nesting already
/// applies, in which case no merging or bubbling happens — children run under
/// [`with_parent`] with a `through`-predicate that only passes style rules.
/// No unknown-at-rule-in-style-rule check is needed here because the previous
/// compilation already bubbled the at-rule to the root.
#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_css_at_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssAtRule<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    if state.declaration_name.is_some() {
        return Err(Box::new(helpers::exception(
            state,
            "At-rules may not be used within nested declarations.".into(),
            Some(node.span),
        )));
    }

    if node.childless {
        helpers::copy_parent_after_sibling(arena, state)?;
        let rule = ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::AtRule(ModifiableCssAtRule::new(
                node.name.clone(),
                node.span,
                true,
                node.value.clone(),
            )),
        );
        return helpers::add_child(arena, state, &rule, None);
    }

    let was_in_keyframes = state.in_keyframes;
    let was_in_unknown_at_rule = state.in_unknown_at_rule;
    // Dart `visitCssAtRule`: `unvendor(node.name.value) == 'keyframes'` —
    // vendor-prefixed keyframes take the keyframes path, like the
    // statement-path check.
    if unvendor(&node.name.value) == "keyframes" {
        state.in_keyframes = true;
    } else {
        state.in_unknown_at_rule = true;
    }

    let rule = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::AtRule(ModifiableCssAtRule::new(
            node.name.clone(),
            node.span,
            false,
            node.value.clone(),
        )),
    );

    if helpers::has_css_nesting(state) {
        let result = helpers::with_parent(
            arena,
            config,
            state,
            rule,
            None,
            Some(false),
            async |c, s| {
                for child in &node.children {
                    dispatch_css_child(c, s, arena, child).await?;
                }
                Ok(())
            },
        )
        .await;
        state.in_unknown_at_rule = was_in_unknown_at_rule;
        state.in_keyframes = was_in_keyframes;
        return result;
    }

    let through: &dyn Fn(&ModifiableCssNode<'parse>) -> bool =
        &|n: &ModifiableCssNode<'parse>| -> bool { n.is_style_rule() };

    let result = helpers::with_parent(
        arena,
        config,
        state,
        rule,
        Some(through),
        Some(false),
        async |c, s| {
            for child in &node.children {
                dispatch_css_child(c, s, arena, child).await?;
            }
            Ok(())
        },
    )
    .await;

    state.in_unknown_at_rule = was_in_unknown_at_rule;
    state.in_keyframes = was_in_keyframes;
    result
}

// ===========================================================================
// evaluate_css_supports_rule
// Dart: visitCssSupportsRule (evaluate.dart:4277-4321). NOTE there (mirrored
//   here in the surrounding code): largely duplicated in `visitSupportsRule`
//   — most changes should be mirrored there.
// ===========================================================================

/// Re-evaluates a frozen CSS `@supports` rule, bubbling it through style rules.
///
/// Matches Dart: rejected inside nested declarations; plain-CSS nesting
/// skips merging/bubbling. Otherwise, when the current style rule exists it
/// is copied childless into the supports rule first, so that declarations
/// immediately inside `@supports` have somewhere to go (e.g. `a {@supports
/// (a: b) {b: c}}` produces `@supports (a: b) {a {b: c}}`).
#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_css_supports_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssSupportsRule<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    if state.declaration_name.is_some() {
        return Err(Box::new(helpers::exception(
            state,
            "Supports rules may not be used within nested declarations.".into(),
            Some(node.span),
        )));
    }

    let rule = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::SupportsRule(ModifiableCssSupportsRule::new(
            node.condition.clone(),
            node.span,
        )),
    );

    if helpers::has_css_nesting(state) {
        return helpers::with_parent(
            arena,
            config,
            state,
            rule,
            None,
            Some(false),
            async |c, s| {
                for child in &node.children {
                    dispatch_css_child(c, s, arena, child).await?;
                }
                Ok(())
            },
        )
        .await;
    }

    let through: &dyn Fn(&ModifiableCssNode<'parse>) -> bool =
        &|n: &ModifiableCssNode<'parse>| -> bool { n.is_style_rule() };

    helpers::with_parent(
        arena,
        config,
        state,
        rule,
        Some(through),
        Some(false),
        async |c, s| {
            let sr_node_clone = s.css_style_rule_node.clone();
            if let Some(ref sr_node) = sr_node_clone {
                let new_parent = sr_node.copy_without_children(arena);
                // Dart: _withParent(styleRule.copyWithoutChildren(), callback) — defaults cropWhen: true
                helpers::with_parent(arena, c, s, new_parent, None, None, async |c, s| {
                    for child in &node.children {
                        dispatch_css_child(c, s, arena, child).await?;
                    }
                    Ok(())
                })
                .await
            } else {
                for child in &node.children {
                    dispatch_css_child(c, s, arena, child).await?;
                }
                Ok(())
            }
        },
    )
    .await
}

// ===========================================================================
// evaluate_css_style_rule
// Dart: visitCssStyleRule (evaluate.dart:4214-4269). NOTE there (mirrored
//   here in the surrounding code): largely duplicated in `visitStyleRule` —
//   most changes should be mirrored there.
// ===========================================================================

/// Re-evaluates a frozen CSS style rule, merging its selector with the
/// enclosing style rule and registering it with the extension store.
///
/// Matches Dart: rejected inside nested declarations and inside keyframe
/// blocks. The selector merges with the enclosing style rule (`merge` is true
/// when there is no enclosing rule, false when the enclosing rule came from
/// plain CSS, and otherwise false only when this rule came from plain CSS and
/// contains a parent selector; merging uses `nest_within` with
/// `implicit_parent`/`preserve_parent_selectors`). The new rule runs under [`with_parent`] (style rules bubble)
/// plus [`with_css_style_rule`], `at_root_excluding_style_rule` is cleared
/// for the duration and restored after, and when there was no enclosing style
/// rule the parent's last child is marked as a group end.
#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_css_style_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssStyleRule<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    if state.declaration_name.is_some() {
        return Err(Box::new(helpers::exception(
            state,
            "Style rules may not be used within nested declarations.".into(),
            Some(node.span),
        )));
    }
    if state.in_keyframes {
        let parent_is_keyframe = matches!(&state.parent, Some(p) if matches!(&p.to_css_node(), CssNode::KeyframeBlock(_)));
        if parent_is_keyframe {
            return Err(Box::new(helpers::exception(
                state,
                "Style rules may not be used within keyframe blocks.".into(),
                Some(node.span),
            )));
        }
    }

    let style_rule_info = if state.at_root_excluding_style_rule {
        None
    } else {
        state
            .style_rule_ignoring_at_root
            .as_ref()
            .map(|r| (r.from_plain_css, r.original_selector))
    };

    let (style_rule_is_none, style_rule_plain_css, style_rule_original_selector) =
        match style_rule_info {
            None => (true, false, None),
            Some((plain_css, orig_sel)) => (false, plain_css, Some(orig_sel)),
        };

    let merge = if style_rule_is_none {
        true
    } else if style_rule_plain_css {
        false
    } else {
        !(node.from_plain_css && node.selector.contains_parent_selector()?)
    };

    let original_selector = if merge {
        node.selector.nest_within(
            arena,
            style_rule_original_selector.as_ref(),
            !state.at_root_excluding_style_rule,
            node.from_plain_css,
        )?
    } else {
        node.selector
    };

    let selector_box = match state.extension_store.as_mut() {
        Some(ref mut store) => {
            store.add_selector(arena, &original_selector, state.media_queries.clone())?
        }
        None => {
            let sel = Rc::new(RefCell::new(original_selector));
            StoreBox { inner: sel }
        }
    };

    let msr = ModifiableCssStyleRule::new(
        Rc::clone(&selector_box.inner),
        node.span,
        Some(original_selector),
        node.from_plain_css,
    );
    let rule_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::StyleRule(msr));

    let old_at_root = state.at_root_excluding_style_rule;
    state.at_root_excluding_style_rule = false;

    let through: Option<&dyn Fn(&ModifiableCssNode<'parse>) -> bool> = if merge {
        Some(&|n: &ModifiableCssNode<'parse>| -> bool { n.is_style_rule() })
    } else {
        None
    };

    let result = helpers::with_parent(
        arena,
        config,
        state,
        rule_node.clone(),
        through,
        Some(false),
        async |c, s| {
            helpers::with_css_style_rule(c, s, node, Some(rule_node.clone()), async |c, s| {
                for child in &node.children {
                    dispatch_css_child(c, s, arena, child).await?;
                }
                Ok(())
            })
            .await
        },
    )
    .await;

    state.at_root_excluding_style_rule = old_at_root;

    // Dart: if (_parent.children case [..., var lastChild] when styleRule == null)
    //   lastChild.isGroupEnd = true;
    if style_rule_is_none {
        if let Some(ref parent) = state.parent {
            if let Some(children) = parent.children() {
                if let Some(last_child) = children.last() {
                    last_child.set_is_group_end(true);
                }
            }
        }
    }

    result
}

// ===========================================================================
// evaluate_css_media_rule
// Dart: visitCssMediaRule (evaluate.dart:4147-4212). NOTE there (mirrored
//   here in the surrounding code): largely duplicated in `visitMediaRule` —
//   most changes should be mirrored there.
// ===========================================================================

/// Re-evaluates a frozen CSS `@media` rule, merging its queries with the
/// enclosing media context and bubbling through style rules.
///
/// Matches Dart: rejected inside nested declarations; plain-CSS nesting
/// skips merging/bubbling. Otherwise the queries merge with the current media
/// context (unrepresentable merges fall back to the node's own queries;
/// fully-merged-away queries produce no output), the merged query set plus
/// sources run under [`with_media_queries`], and the `through`-predicate
/// passes style rules plus media rules whose queries are all in the merged
/// source set (so it is safe to bubble one query through another). As with
/// `@supports`, the current style rule — if any — is copied childless into
/// the media rule first so bare declarations have somewhere to go (e.g. `a
/// {@media screen {b: c}}` produces `@media screen {a {b: c}}`).
#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_css_media_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssMediaRule<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    if state.declaration_name.is_some() {
        return Err(Box::new(helpers::exception(
            state,
            "Media rules may not be used within nested declarations.".into(),
            Some(node.span),
        )));
    }

    if helpers::has_css_nesting(state) {
        let rule_node = ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::MediaRule(ModifiableCssMediaRule::new(
                node.queries.clone(),
                node.span,
            )?),
        );
        return helpers::with_parent(
            arena,
            config,
            state,
            rule_node,
            None,
            Some(false),
            async |c, s| {
                for child in &node.children {
                    dispatch_css_child(c, s, arena, child).await?;
                }
                Ok(())
            },
        )
        .await;
    }

    let (merged_queries, merged_sources) = if let Some(ref mq) = state.media_queries {
        match helpers::merge_media_queries(mq, &node.queries) {
            None => {
                // Unrepresentable — use original queries, empty sources
                (Some(node.queries.clone()), Some(vec![]))
            }
            Some(m) if m.is_empty() => {
                // All merged away — skip
                return Ok(());
            }
            Some(m) => {
                // Build mergedSources = sources ∪ mediaQueries ∪ node.queries
                let mut sources = state.media_query_sources.clone().unwrap_or_default();
                sources.extend(mq.clone());
                sources.extend(node.queries.clone());
                (Some(m), Some(sources))
            }
        }
    } else {
        // Not in media context
        (Some(node.queries.clone()), Some(vec![]))
    };

    let queries_for_rule = merged_queries
        .clone()
        .unwrap_or_else(|| node.queries.clone());
    let rule_node = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::MediaRule(ModifiableCssMediaRule::new(
            queries_for_rule.clone(),
            node.span,
        )?),
    );

    let sources_for_through = merged_sources.clone();
    let through: &dyn Fn(&ModifiableCssNode<'parse>) -> bool =
        &move |n: &ModifiableCssNode<'parse>| -> bool {
            if n.is_style_rule() {
                return true;
            }
            if n.is_media_rule() {
                if let Some(ref srcs) = sources_for_through {
                    if !srcs.is_empty() {
                        let kind = n.kind();
                        if let ModifiableCssNodeKind::MediaRule(mr) = &*kind {
                            return mr.queries.iter().all(|q| srcs.iter().any(|s| s == q));
                        }
                    }
                }
            }
            false
        };

    helpers::with_parent(
        arena,
        config,
        state,
        rule_node,
        Some(through),
        Some(false),
        async |c, s| {
            let sr_node_clone = s.css_style_rule_node.clone();
            helpers::with_media_queries(c, s, merged_queries, merged_sources, async |c, s| {
                if let Some(ref sr_node) = sr_node_clone {
                    let new_parent = sr_node.copy_without_children(arena);
                    helpers::with_parent(
                        arena,
                        c,
                        s,
                        new_parent,
                        None,
                        Some(false),
                        async |c, s| {
                            for child in &node.children {
                                dispatch_css_child(c, s, arena, child).await?;
                            }
                            Ok(())
                        },
                    )
                    .await
                } else {
                    for child in &node.children {
                        dispatch_css_child(c, s, arena, child).await?;
                    }
                    Ok(())
                }
            })
            .await
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::css::at_rule::ModifiableCssAtRule;
    use crate::ast::css::comment::ModifiableCssComment;
    use crate::ast::css::keyframe_block::ModifiableCssKeyframeBlock;
    use crate::ast::css::media_query::CssMediaQuery;
    use crate::ast::css::stylesheet::ModifiableCssStylesheet;
    use crate::common::ast_css_value::CssValue;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::compile::compile;
    use crate::compile::CompileOptions;
    use crate::compile_context::new_compile_context;
    use crate::eval::helpers::has_css_nesting;
    use crate::eval::helpers::with_css_style_rule;
    use crate::io::Io;
    use crate::io::VirtualIo;
    use crate::logger::Logger;
    use crate::logger::QuietLogger;
    use crate::selector::class::ClassSelector;
    use crate::selector::complex::ComplexSelector;
    use crate::selector::complex_component::ComplexSelectorComponent;
    use crate::selector::compound::CompoundSelector;
    use crate::selector::list::SelectorList;
    use crate::selector::SimpleSelector;
    use crate::value::string::SassString;
    use crate::value::{Value, ValueKind};
    use bumpalo::Bump;
    use std::collections::HashMap;
    use std::rc::Rc;

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

    fn test_state<'compile, 'parse>(
        arena: &'compile Bump,
    ) -> (EvalConfig<'compile, 'parse>, EvalState<'compile, 'parse>)
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let span = make_span(arena, "test { color: red; }");
        let logger: Rc<dyn Logger> = Rc::new(QuietLogger);
        let config = EvalConfig::new(
            arena,
            logger,
            new_compile_context(),
            Rc::new(VirtualIo::new()),
        );
        let mut state = EvalState::new(arena, span);
        let root = ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::Stylesheet(ModifiableCssStylesheet::new(span)),
        );
        state.root = Some(root.clone());
        state.parent = Some(root);
        state.member = "root stylesheet".to_string();
        (config, state)
    }

    // ── evaluate_css_stylesheet ──

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_stylesheet_empty() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let ss = CssStylesheet::new(vec![], make_span(&arena, ""));
        evaluate_css_stylesheet(&config, &mut state, &arena, &ss)
            .await
            .unwrap();
    }

    // ── evaluate_css_comment ──

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_comment_adds_child() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let c = CssComment::new("/* hello */".into(), make_span(&arena, "/* hello */"));
        evaluate_css_comment(&config, &mut state, &arena, &c).unwrap();
        assert_eq!(state.root.unwrap().children().unwrap().len(), 1);
    }

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_comment_increments_end_of_imports() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        state.end_of_imports = 0;
        let c = CssComment::new("/* hello */".into(), make_span(&arena, "/* hello */"));
        evaluate_css_comment(&config, &mut state, &arena, &c).unwrap();
        assert_eq!(state.end_of_imports, 1);
    }

    // ── evaluate_css_import ──

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_import_at_root_end_of_imports() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        state.end_of_imports = 0;
        let imp = CssImport::new(
            make_val(&arena, "\"foo.css\""),
            make_span(&arena, "@import \"foo.css\";"),
            None,
        );
        evaluate_css_import(&config, &mut state, &arena, &imp).unwrap();
        assert_eq!(state.end_of_imports, 1);
    }

    // ── evaluate_css_declaration ──

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_declaration_adds_child() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let d = CssDeclaration::new(
            make_val(&arena, "color"),
            CssValue::new(
                Value::new_with_arena(&arena, ValueKind::String(SassString::new("red", false))),
                make_span(&arena, "red"),
            ),
            make_span(&arena, "color: red;"),
            true,
            None,
        )
        .unwrap();
        evaluate_css_declaration(&config, &mut state, &arena, &d).unwrap();
        assert_eq!(state.root.unwrap().children().unwrap().len(), 1);
    }

    // ── evaluate_css_keyframe_block ──

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_keyframe_block_adds_child() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let kb = CssKeyframeBlock::new(
            CssValue::new(vec!["10%".into()], make_span(&arena, "10%")),
            make_span(&arena, "10% {}"),
        );
        evaluate_css_keyframe_block(&config, &mut state, &arena, &kb)
            .await
            .unwrap();
        assert_eq!(state.root.unwrap().children().unwrap().len(), 1);
    }

    // ── evaluate_css_at_rule ──

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_at_rule_childless() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let at = CssAtRule::new(
            make_val(&arena, "import"),
            make_span(&arena, "@import;"),
            true,
            None,
        );
        evaluate_css_at_rule(&config, &mut state, &arena, &at)
            .await
            .unwrap();
        assert_eq!(state.root.unwrap().children().unwrap().len(), 1);
    }

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_at_rule_non_childless() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let at = CssAtRule::new(
            make_val(&arena, "media"),
            make_span(&arena, "@media {}"),
            false,
            None,
        );
        evaluate_css_at_rule(&config, &mut state, &arena, &at)
            .await
            .unwrap();
        assert_eq!(state.root.unwrap().children().unwrap().len(), 1);
    }

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_css_at_rule_unvendors_keyframes() {
        // Dart `visitCssAtRule` checks `unvendor(node.name.value) ==
        // 'keyframes'`: vendor-prefixed keyframes take the keyframes path,
        // so a style rule inside flags "may not be used within keyframe
        // blocks" instead of silently evaluating as unknown content.
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let sel = make_test_selector_list(&arena);
        let sel_clone = sel;
        let sr = CssStyleRule::new(sel, make_span(&arena, ".foo {}"), sel_clone, false);
        let mut kb = CssKeyframeBlock::new(
            CssValue::new(vec!["10%".into()], make_span(&arena, "10%")),
            make_span(&arena, "10% {}"),
        );
        kb.children.push(CssNode::StyleRule(sr));
        let mut at = CssAtRule::new(
            make_val(&arena, "-webkit-keyframes"),
            make_span(&arena, "@-webkit-keyframes {}"),
            false,
            None,
        );
        at.children.push(CssNode::KeyframeBlock(kb));
        let err = evaluate_css_at_rule(&config, &mut state, &arena, &at)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "Style rules may not be used within keyframe blocks."
        );
    }

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_at_rule_declaration_name_error() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        state.declaration_name = Some("color".into());
        let at = CssAtRule::new(
            make_val(&arena, "media"),
            make_span(&arena, "@media {}"),
            false,
            None,
        );
        let err = evaluate_css_at_rule(&config, &mut state, &arena, &at)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "At-rules may not be used within nested declarations."
        );
    }

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_at_rule_restores_flags() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let at = CssAtRule::new(
            make_val(&arena, "keyframes"),
            make_span(&arena, "@keyframes {}"),
            false,
            None,
        );
        evaluate_css_at_rule(&config, &mut state, &arena, &at)
            .await
            .unwrap();
        assert!(!state.in_keyframes);
    }

    // ── evaluate_css_supports_rule ──

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_supports_rule_adds_child() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let sr = CssSupportsRule::new(
            make_val(&arena, "display: grid"),
            make_span(&arena, "@supports (display: grid) {}"),
        );
        evaluate_css_supports_rule(&config, &mut state, &arena, &sr)
            .await
            .unwrap();
        assert_eq!(state.root.unwrap().children().unwrap().len(), 1);
    }

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_supports_rule_declaration_name_error() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        state.declaration_name = Some("color".into());
        let sr = CssSupportsRule::new(
            make_val(&arena, "display: grid"),
            make_span(&arena, "@supports (display: grid) {}"),
        );
        let err = evaluate_css_supports_rule(&config, &mut state, &arena, &sr)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "Supports rules may not be used within nested declarations."
        );
    }

    // ── evaluate_css_style_rule ──

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_style_rule_declaration_name_error() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        state.declaration_name = Some("color".into());
        let sel = make_test_selector_list(&arena);
        let sel_clone = sel;
        let sr = CssStyleRule::new(sel, make_span(&arena, ".foo {}"), sel_clone, false);
        let err = evaluate_css_style_rule(&config, &mut state, &arena, &sr)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "Style rules may not be used within nested declarations."
        );
    }

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_style_rule_in_keyframes_error() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        state.in_keyframes = true;
        let kb = CssKeyframeBlock::new(
            CssValue::new(vec!["10%".into()], make_span(&arena, "10%")),
            make_span(&arena, "10% {}"),
        );
        state.parent = Some(ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::KeyframeBlock(ModifiableCssKeyframeBlock::new(
                kb.selector.clone(),
                kb.span,
            )),
        ));
        let sel = make_test_selector_list(&arena);
        let sel_clone = sel;
        let sr = CssStyleRule::new(sel, make_span(&arena, ".foo {}"), sel_clone, false);
        let err = evaluate_css_style_rule(&config, &mut state, &arena, &sr)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "Style rules may not be used within keyframe blocks."
        );
    }

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_load_css_bubbles_plain_media() {
        // Plain-CSS `@media`-in-style loaded via `load-css` bubbles and
        // flattens (`a b`): style-rule copies and clones drop `fromPlainCss`
        // (Dart default false), so re-evaluation merges instead of nesting.
        let arena = Bump::new();
        let mut files = HashMap::new();
        files.insert(
            "/main.scss".to_string(),
            "@use \"sass:meta\";\n@include meta.load-css(\"nested.css\");".to_string(),
        );
        files.insert(
            "/nested.css".to_string(),
            "a {\n  @media screen { b { c: d; } }\n}\n".to_string(),
        );
        let io: Rc<dyn Io> = Rc::new(VirtualIo::with_files(files));
        let result = compile("/main.scss", io, CompileOptions::new(&arena), &arena)
            .await
            .unwrap();
        assert_eq!(result.css(), "@media screen {\n  a b {\n    c: d;\n  }\n}");
    }

    // ── evaluate_css_media_rule ──

    #[rust_sass_macros::maybe_test]
    #[rust_sass_macros::maybe_async]
    async fn test_evaluate_css_media_rule_declaration_name_error() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        state.declaration_name = Some("color".into());
        let mr = CssMediaRule::new(
            vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])],
            make_span(&arena, "@media screen {}"),
        )
        .unwrap();
        let err = evaluate_css_media_rule(&config, &mut state, &arena, &mr)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "Media rules may not be used within nested declarations."
        );
    }

    // ── evaluate_css_stylesheet with children ──

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_css_stylesheet_with_children() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let comment = CssComment::new("/* hello */".into(), make_span(&arena, "/* hello */"));
        let ss = CssStylesheet::new(vec![CssNode::Comment(comment)], make_span(&arena, ""));
        evaluate_css_stylesheet(&config, &mut state, &arena, &ss)
            .await
            .unwrap();
        assert_eq!(state.root.unwrap().children().unwrap().len(), 1);
    }

    // ── evaluate_css_comment not at root ──

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_css_comment_not_at_root() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let at_rule = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::AtRule(ModifiableCssAtRule::new(
                make_val(&arena, "media"),
                make_span(&arena, "@media"),
                false,
                None,
            )),
        );
        state.parent = Some(at_rule);
        let c = CssComment::new("/* hello */".into(), make_span(&arena, "/* hello */"));
        evaluate_css_comment(&config, &mut state, &arena, &c).unwrap();
        assert_eq!(state.end_of_imports, 0);
    }

    // ── evaluate_css_import parent not root ──

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_css_import_parent_not_root() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let at_rule = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::AtRule(ModifiableCssAtRule::new(
                make_val(&arena, "custom"),
                make_span(&arena, "@custom"),
                false,
                None,
            )),
        );
        state.parent = Some(at_rule.clone());
        let imp = CssImport::new(
            make_val(&arena, "\"foo.css\""),
            make_span(&arena, "@import \"foo.css\";"),
            None,
        );
        evaluate_css_import(&config, &mut state, &arena, &imp).unwrap();
        assert_eq!(at_rule.children().unwrap().len(), 1);
    }

    // ── evaluate_css_import at root out of order ──

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_css_import_at_root_out_of_order() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let root = state.root.clone().unwrap();
        let comment = ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::Comment(ModifiableCssComment::new(
                "/* hello */".into(),
                make_span(&arena, "/* hello */"),
            )),
        );
        root.add_child(&comment).unwrap();
        state.end_of_imports = 0;
        let imp = CssImport::new(
            make_val(&arena, "\"foo.css\""),
            make_span(&arena, "@import \"foo.css\";"),
            None,
        );
        evaluate_css_import(&config, &mut state, &arena, &imp).unwrap();
        assert_eq!(state.out_of_order_imports.len(), 1);
    }

    // ── with_style_rule ──

    #[rust_sass_macros::maybe_test]
    async fn test_with_css_style_rule_save_restore() {
        let arena = Bump::new();
        let (_, _state) = test_state(&arena);
        let sel = make_test_selector_list(&arena);
        let span = make_span(&arena, ".foo {}");
        let sr = CssStyleRule {
            selector: sel,
            original_selector: sel,
            from_plain_css: false,
            children: vec![],
            span,
            is_group_end: false,
            tabs: 0,
        };
        let (config, mut state) = test_state(&arena);
        with_css_style_rule(&config, &mut state, &sr, None, async |c, s| {
            let _ = (c, s);
            Ok(())
        })
        .await
        .unwrap();
        assert!(state.style_rule_ignoring_at_root.is_none());
    }

    // ── has_css_nesting ──

    #[rust_sass_macros::maybe_test]
    async fn test_has_css_nesting_false_no_style_rule() {
        let arena = Bump::new();
        let (_, state) = test_state(&arena);
        assert!(!has_css_nesting(&state));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_has_css_nesting_false_single_style_rule() {
        let arena = Bump::new();
        let (_, mut state) = test_state(&arena);
        let sel = make_test_selector_list(&arena);
        let span = make_span(&arena, ".foo {}");
        let sr = CssStyleRule {
            selector: sel,
            original_selector: sel,
            from_plain_css: false,
            children: vec![],
            span,
            is_group_end: false,
            tabs: 0,
        };
        state.style_rule_ignoring_at_root = Some(sr);
        assert!(!has_css_nesting(&state));
    }

    // ── helpers ──

    fn make_test_selector_list(arena: &'_ Bump) -> SelectorList<'_> {
        let span = make_span(arena, ".foo");
        let class = ClassSelector::new(".foo".into(), span);
        let simple = SimpleSelector::Class(class);
        let compound = CompoundSelector::new(vec![simple], span).unwrap();
        let comp = ComplexSelectorComponent::new(Box::new(compound), vec![], span);
        let complex = ComplexSelector::new(vec![], vec![comp], span, false).unwrap();
        SelectorList::new(arena, vec![complex], span).unwrap()
    }
}
