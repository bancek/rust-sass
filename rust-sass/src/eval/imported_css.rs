// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/async_evaluate.dart (_ImportedCssVisitor,
//   async_evaluate.dart:4832-4891; the class is duplicated verbatim in the
//   generated evaluate.dart:4811-4870)
// go-source: go/eval/imported_css_visitor.go

// ImportedCssVisitor — free functions standing in for Dart's private
// `_ImportedCssVisitor` struct (evaluate.dart:4811), which borrows the main
// `_EvaluateVisitor` as `_visitor` and implements `ModifiableCssVisitor` over
// the already-modifiable CSS of an `@import`ed module. In Rust these are free
// functions taking `(config, state, arena, node)` that call `add_child` on
// the main visitor's state (split-borrow style, no visitor-trait impl).
//
// Matches Dart's block role: unlike the `visitCss*` re-evaluation in
// `css.rs` (frozen `CssNode` in, new `ModifiableCssNode` out), these methods
// receive nodes that are *already* modifiable and only place them with the
// right `through`-predicate. Callers that hold frozen CSS must go through
// `css.rs`, not here.

use crate::ast::css::at_rule::CssAtRule;
use crate::ast::css::at_rule::ModifiableCssAtRule;
use crate::ast::css::comment::CssComment;
use crate::ast::css::comment::ModifiableCssComment;
use crate::ast::css::declaration::CssDeclaration;
use crate::ast::css::declaration::ModifiableCssDeclaration;
use crate::ast::css::import::CssImport;
use crate::ast::css::import::ModifiableCssImport;
use crate::ast::css::keyframe_block::CssKeyframeBlock;
use crate::ast::css::media_rule::CssMediaRule;
use crate::ast::css::media_rule::ModifiableCssMediaRule;
use crate::ast::css::modifiable_node::{ModifiableCssNode, ModifiableCssNodeKind};
use crate::ast::css::node::CssNode;
use crate::ast::css::style_rule::CssStyleRule;
use crate::ast::css::style_rule::ModifiableCssStyleRule;
use crate::ast::css::stylesheet::CssStylesheet;
use crate::ast::css::supports_rule::CssSupportsRule;
use crate::ast::css::supports_rule::ModifiableCssSupportsRule;
use crate::common::exception::{SassError, SassResult};
use crate::eval::helpers;
use crate::eval::{EvalConfig, EvalState};
use bumpalo::Bump;
use std::cell::RefCell;
use std::rc::Rc;

// ===========================================================================
// evaluate_imported_css_at_rule
// Dart: _ImportedCssVisitor.visitCssAtRule (evaluate.dart:4817-4822) —
//   `addChild` with a style-rule `through`-predicate, or no predicate when
//   the node is childless.
// ===========================================================================

/// Places an already-modifiable imported at-rule, bubbling through style rules.
///
/// The childless fast path adds the node directly; otherwise the style-rule
/// `through`-predicate applies. Children are then visited under the node as
/// the new parent (direct `state.parent` save/restore — no environment scope,
/// unlike [`with_parent`]).
#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_imported_css_at_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssAtRule<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let mod_node = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::AtRule(ModifiableCssAtRule::new(
            node.name.clone(),
            node.span,
            node.childless,
            node.value.clone(),
        )),
    );
    if node.childless {
        helpers::add_child(arena, state, &mod_node, None)
    } else {
        mod_node.set_is_group_end(node.is_group_end);
        let through: &dyn Fn(&ModifiableCssNode<'parse>) -> bool =
            &|n: &ModifiableCssNode<'parse>| -> bool { n.is_style_rule() };
        helpers::add_child(arena, state, &mod_node, Some(through))?;
        let old_parent = state.parent.replace(mod_node);
        for child in &node.children {
            box_rec_in!(
                dispatch_imported_css_child(config, state, arena, child),
                arena,
            )
            .await?;
        }
        state.parent = old_parent;
        Ok(())
    }
}

// ===========================================================================
// evaluate_imported_css_comment
// Dart: _ImportedCssVisitor.visitCssComment (evaluate.dart:4824) —
//   `_visitor._addChild(node)` with no `through`-predicate.
// ===========================================================================

/// Places an already-modifiable imported comment as a direct child.
pub(crate) fn evaluate_imported_css_comment<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssComment<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let mod_node = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::Comment(ModifiableCssComment::new(node.text.clone(), node.span)),
    );
    helpers::add_child(arena, state, &mod_node, None)
}

// ===========================================================================
// evaluate_imported_css_declaration
// Dart: _ImportedCssVisitor.visitCssDeclaration (evaluate.dart:4826-4827) —
//   `_visitor._addChild(node)` with no `through`-predicate.
// ===========================================================================

/// Places an already-modifiable imported declaration as a direct child.
pub(crate) fn evaluate_imported_css_declaration<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssDeclaration<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let mod_node = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::Declaration(ModifiableCssDeclaration::new(
            node.name.clone(),
            node.value.clone(),
            node.span,
            node.parsed_as_sass_script,
            Some(node.value_span_for_map),
        )?),
    );
    helpers::add_child(arena, state, &mod_node, None)
}

// ===========================================================================
// evaluate_imported_css_import
// Dart: _ImportedCssVisitor.visitCssImport (evaluate.dart:4829-4838) — a
//   nested import is added directly; a root-level import in order extends
//   `_endOfImports`, while an out-of-order root import is deferred into
//   `_outOfOrderImports`.
// ===========================================================================

/// Places an already-modifiable imported `@import`, tracking import order.
///
/// Same ordering contract as [`crate::eval::css::evaluate_css_import`]:
/// nested imports attach to the current parent, in-order root imports bump
/// [`EvalState::end_of_imports`], out-of-order root imports wait in
/// [`EvalState::out_of_order_imports`].
pub(crate) fn evaluate_imported_css_import<'compile, 'parse>(
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
        return helpers::add_child(arena, state, &import, None);
    }

    let root_len = state
        .root
        .as_ref()
        .unwrap()
        .children()
        .map(|c| c.len())
        .unwrap_or(0);
    if state.end_of_imports == root_len {
        helpers::add_child(arena, state, &import, None)?;
        state.end_of_imports += 1;
    } else {
        state
            .out_of_order_imports
            .push(ModifiableCssImport::from_css_import(node));
    }
    Ok(())
}

// ===========================================================================
// evaluate_imported_css_keyframe_block
// Dart: _ImportedCssVisitor.visitCssKeyframeBlock (evaluate.dart:4840-4842) —
//   `assert(false, "visitCssKeyframeBlock() should never be called.")`.
//   Keyframe children reach the output tree through the `visitCss*`
//   re-evaluation path, never as already-modifiable nodes.
// ===========================================================================

/// Unreachable imported-CSS entry point for keyframe blocks.
///
/// Matches Dart: keyframe blocks never arrive here as modifiable nodes, so
/// this always returns a `Script` error (the `assert(false, …)` translated
/// to a result, since asserts are stripped in release builds).
pub(crate) fn evaluate_imported_css_keyframe_block<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    _arena: &'compile Bump,
    _node: &CssKeyframeBlock<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    Err(Box::new(SassError::Script {
        message: "visitCssKeyframeBlock() should never be called".into(),
        argument_name: None,
    }))
}

// ===========================================================================
// evaluate_imported_css_media_rule
// Dart: _ImportedCssVisitor.visitCssMediaRule (evaluate.dart:4844-4857).
// ===========================================================================

/// Places an already-modifiable imported `@media` rule, bubbling through
/// style rules and already-merged media rules.
///
/// Matches Dart: `has_been_merged` is true when there is no enclosing media
/// context, or when re-merging the context with the node's queries still
/// succeeds (a merged query re-merges as a no-op; an unmerged one would fail).
/// The `through`-predicate then passes style rules, plus media rules when the
/// node has been merged. Children run under the node as the new parent.
#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_imported_css_media_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssMediaRule<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let mod_node = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::MediaRule(ModifiableCssMediaRule::new(
            node.queries.clone(),
            node.span,
        )?),
    );
    mod_node.set_is_group_end(node.is_group_end);

    // Dart: hasBeenMerged = mediaQueries == null || _mergeMediaQueries(mediaQueries, node.queries) != null
    let has_been_merged = match &state.media_queries {
        None => true,
        Some(mq) => helpers::merge_media_queries(mq, &node.queries).is_some(),
    };

    let through: &dyn Fn(&ModifiableCssNode<'parse>) -> bool =
        &move |n: &ModifiableCssNode<'parse>| -> bool {
            if n.is_style_rule() {
                return true;
            }
            if has_been_merged && n.is_media_rule() {
                return true;
            }
            false
        };

    helpers::add_child(arena, state, &mod_node, Some(through))?;
    let old_parent = state.parent.replace(mod_node);
    for child in &node.children {
        box_rec_in!(
            dispatch_imported_css_child(config, state, arena, child),
            arena,
        )
        .await?;
    }
    state.parent = old_parent;
    Ok(())
}

// ===========================================================================
// evaluate_imported_css_style_rule
// Dart: _ImportedCssVisitor.visitCssStyleRule (evaluate.dart:4859-4860) —
//   `_visitor._addChild(node, through: (node) => node is CssStyleRule)`.
// ===========================================================================

/// Places an already-modifiable imported style rule, bubbling through style
/// rules. Children run under the node as the new parent.
#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_imported_css_style_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssStyleRule<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let mod_node = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::StyleRule(ModifiableCssStyleRule::new(
            Rc::new(RefCell::new(node.selector)),
            node.span,
            Some(node.original_selector),
            node.from_plain_css,
        )),
    );
    mod_node.set_is_group_end(node.is_group_end);
    let through: &dyn Fn(&ModifiableCssNode<'parse>) -> bool =
        &|n: &ModifiableCssNode<'parse>| -> bool { n.is_style_rule() };
    helpers::add_child(arena, state, &mod_node, Some(through))?;
    let old_parent = state.parent.replace(mod_node);
    for child in &node.children {
        dispatch_imported_css_child(config, state, arena, child).await?;
    }
    state.parent = old_parent;
    Ok(())
}

// ===========================================================================
// evaluate_imported_css_stylesheet
// Dart: _ImportedCssVisitor.visitCssStylesheet (evaluate.dart:4862-4866) —
//   iterates children via `accept`; the Rust free function below is the same
//   traversal with `box_rec_in!`-guarded recursion on the stylesheet arm.
// ===========================================================================

/// Visits each child of an already-modifiable imported stylesheet.
///
/// The stylesheet itself adds no output node; see
/// [`dispatch_imported_css_child`] for the per-node dispatch.
#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_imported_css_stylesheet<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssStylesheet<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    for child in &node.children {
        match child {
            CssNode::Stylesheet(n) => {
                box_rec_in!(
                    evaluate_imported_css_stylesheet(config, state, arena, n),
                    arena,
                )
                .await?
            }
            CssNode::StyleRule(n) => {
                evaluate_imported_css_style_rule(config, state, arena, n).await?
            }
            CssNode::AtRule(n) => evaluate_imported_css_at_rule(config, state, arena, n).await?,
            CssNode::Comment(n) => evaluate_imported_css_comment(config, state, arena, n)?,
            CssNode::Declaration(n) => evaluate_imported_css_declaration(config, state, arena, n)?,
            CssNode::Import(n) => evaluate_imported_css_import(config, state, arena, n)?,
            CssNode::KeyframeBlock(n) => {
                evaluate_imported_css_keyframe_block(config, state, arena, n)?
            }
            CssNode::MediaRule(n) => {
                evaluate_imported_css_media_rule(config, state, arena, n).await?
            }
            CssNode::SupportsRule(n) => {
                evaluate_imported_css_supports_rule(config, state, arena, n).await?
            }
        }
    }
    Ok(())
}

// ===========================================================================
// evaluate_imported_css_supports_rule
// Dart: _ImportedCssVisitor.visitCssSupportsRule (evaluate.dart:4868-4869) —
//   `_visitor._addChild(node, through: (node) => node is CssStyleRule)`.
// ===========================================================================

/// Places an already-modifiable imported `@supports` rule, bubbling through
/// style rules. Children run under the node as the new parent.
#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_imported_css_supports_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssSupportsRule<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let mod_node = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::SupportsRule(ModifiableCssSupportsRule::new(
            node.condition.clone(),
            node.span,
        )),
    );
    mod_node.set_is_group_end(node.is_group_end);
    let through: &dyn Fn(&ModifiableCssNode<'parse>) -> bool =
        &|n: &ModifiableCssNode<'parse>| -> bool { n.is_style_rule() };
    helpers::add_child(arena, state, &mod_node, Some(through))?;
    let old_parent = state.parent.replace(mod_node);
    for child in &node.children {
        dispatch_imported_css_child(config, state, arena, child).await?;
    }
    state.parent = old_parent;
    Ok(())
}

// ===========================================================================
// Dispatch helper
// ===========================================================================

/// Dispatches one already-modifiable imported CSS child to its
/// `evaluate_imported_css_*` placement.
///
/// Rust-only split of Dart's `child.accept(this)`: one `match` arm per
/// [`CssNode`] variant. Deeply recursive arms are `box_rec_in!`-guarded so
/// the async build doesn't grow unbounded futures.
#[rust_sass_macros::maybe_async]
pub(crate) async fn dispatch_imported_css_child<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &CssNode<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    match node {
        CssNode::Stylesheet(n) => {
            box_rec_in!(
                evaluate_imported_css_stylesheet(config, state, arena, n),
                arena,
            )
            .await
        }
        CssNode::StyleRule(n) => {
            box_rec_in!(
                evaluate_imported_css_style_rule(config, state, arena, n),
                arena,
            )
            .await
        }
        CssNode::AtRule(n) => evaluate_imported_css_at_rule(config, state, arena, n).await,
        CssNode::Comment(n) => evaluate_imported_css_comment(config, state, arena, n),
        CssNode::Declaration(n) => evaluate_imported_css_declaration(config, state, arena, n),
        CssNode::Import(n) => evaluate_imported_css_import(config, state, arena, n),
        CssNode::KeyframeBlock(n) => evaluate_imported_css_keyframe_block(config, state, arena, n),
        CssNode::MediaRule(n) => evaluate_imported_css_media_rule(config, state, arena, n).await,
        CssNode::SupportsRule(n) => {
            box_rec_in!(
                evaluate_imported_css_supports_rule(config, state, arena, n),
                arena,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::css::at_rule::ModifiableCssAtRule;
    use crate::ast::css::media_query::CssMediaQuery;
    use crate::ast::css::stylesheet::ModifiableCssStylesheet;
    use crate::common::ast_css_value::CssValue;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::compile_context::new_compile_context;
    use crate::io::VirtualIo;
    use crate::logger::Logger;
    use crate::logger::QuietLogger;
    use crate::selector::complex::ComplexSelector;
    use crate::selector::complex_component::ComplexSelectorComponent;
    use crate::selector::compound::CompoundSelector;
    use crate::selector::list::SelectorList;
    use crate::selector::universal::UniversalSelector;
    use crate::selector::SimpleSelector;
    use crate::value::string::SassString;
    use crate::value::{Value, ValueKind};
    use bumpalo::Bump;
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

    #[rust_sass_macros::maybe_test]
    async fn test_imported_css_comment() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let c = CssComment::new("/* imported */".into(), make_span(&arena, "/* imported */"));
        evaluate_imported_css_comment(&config, &mut state, &arena, &c).unwrap();
    }

    #[rust_sass_macros::maybe_test]
    async fn test_imported_css_declaration() {
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
        evaluate_imported_css_declaration(&config, &mut state, &arena, &d).unwrap();
    }

    #[rust_sass_macros::maybe_test]
    async fn test_imported_css_keyframe_block_error() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let kb = CssKeyframeBlock::new(
            CssValue::new(vec!["10%".into()], make_span(&arena, "10%")),
            make_span(&arena, "10% {}"),
        );
        let err =
            evaluate_imported_css_keyframe_block(&config, &mut state, &arena, &kb).unwrap_err();
        assert_eq!(
            err.message(),
            "visitCssKeyframeBlock() should never be called"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_imported_css_at_rule_childless() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let at = CssAtRule::new(
            make_val(&arena, "import"),
            make_span(&arena, "@import;"),
            true,
            None,
        );
        evaluate_imported_css_at_rule(&config, &mut state, &arena, &at)
            .await
            .unwrap();
    }

    #[rust_sass_macros::maybe_test]
    async fn test_imported_css_at_rule_non_childless() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let at = CssAtRule::new(
            make_val(&arena, "media"),
            make_span(&arena, "@media {}"),
            false,
            None,
        );
        evaluate_imported_css_at_rule(&config, &mut state, &arena, &at)
            .await
            .unwrap();
    }

    #[rust_sass_macros::maybe_test]
    async fn test_imported_css_supports_rule() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let sr = CssSupportsRule::new(
            make_val(&arena, "display: grid"),
            make_span(&arena, "@supports (display: grid) {}"),
        );
        evaluate_imported_css_supports_rule(&config, &mut state, &arena, &sr)
            .await
            .unwrap();
    }

    #[rust_sass_macros::maybe_test]
    async fn test_imported_css_media_rule_unmerged() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let mr = CssMediaRule::new(
            vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])],
            make_span(&arena, "@media screen {}"),
        )
        .unwrap();
        evaluate_imported_css_media_rule(&config, &mut state, &arena, &mr)
            .await
            .unwrap();
    }

    #[rust_sass_macros::maybe_test]
    async fn test_imported_css_import_at_root_end_of_imports() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        state.end_of_imports = 0;
        let imp = CssImport::new(
            make_val(&arena, "\"foo.css\""),
            make_span(&arena, "@import \"foo.css\";"),
            None,
        );
        evaluate_imported_css_import(&config, &mut state, &arena, &imp).unwrap();
        assert_eq!(state.end_of_imports, 1);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_imported_css_import_not_at_root() {
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
        state.parent = Some(at_rule);
        let imp = CssImport::new(
            make_val(&arena, "\"bar.css\""),
            make_span(&arena, "@import \"bar.css\";"),
            None,
        );
        evaluate_imported_css_import(&config, &mut state, &arena, &imp).unwrap();
    }

    #[rust_sass_macros::maybe_test]
    async fn test_imported_css_stylesheet() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        let c = CssComment::new("/* imported */".into(), make_span(&arena, "/* imported */"));
        let ss = CssStylesheet::new(vec![CssNode::Comment(c)], make_span(&arena, ""));
        evaluate_imported_css_stylesheet(&config, &mut state, &arena, &ss)
            .await
            .unwrap();
    }

    // ── imported css style rule ──

    #[rust_sass_macros::maybe_test]
    async fn test_imported_css_style_rule() {
        let arena = Bump::new();
        let (config, mut state) = test_state(&arena);
        // Create a minimal valid selector list
        let span = make_span(&arena, "*");
        let simple = SimpleSelector::Universal(UniversalSelector::new(span, None));
        let compound = CompoundSelector::new(vec![simple], span).unwrap();
        let comp = ComplexSelectorComponent::new(Box::new(compound), vec![], span);
        let complex = ComplexSelector::new(vec![], vec![comp], span, false).unwrap();
        let sel_list = SelectorList::new(&arena, vec![complex], span).unwrap();
        let span_rule = make_span(&arena, ".foo {}");
        let sr = CssStyleRule {
            selector: sel_list,
            original_selector: sel_list,
            from_plain_css: false,
            children: vec![],
            span: span_rule,
            is_group_end: false,
            tabs: 0,
        };
        evaluate_imported_css_style_rule(&config, &mut state, &arena, &sr)
            .await
            .unwrap();
    }
}
