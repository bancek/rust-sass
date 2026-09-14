// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/evaluate.dart (statement visitors, _loadModule,
//   _execute, _combineCss/_extendModules; visit methods ~ll.1167-2760)
// go-source: go/eval/evaluate_statement.go

//! Statement evaluation: Sass statements in, modifiable CSS tree out.
//!
//! Each free function here realizes one Dart `_EvaluateVisitor.visit*`
//! member (pointers below name the member and its `evaluate.dart` line).
//! Dispatch is a plain `match` over [`Statement`]: the evaluator implements
//! no visitor trait, so call sites split-borrow `config` (shared) plus
//! `state` (mutable) directly (see `architecture.md` §7). Save/restore of
//! evaluator state (parent, media queries, declaration name, keyframes and
//! unknown-at-rule flags) is done inline at each rule that needs it,
//! mirroring Dart's `_withParent`/`_withMediaQueries`/scope combinators.

use crate::ast::sass::argument_list::ArgumentList;
use crate::eval::helpers::tabs_frame_pop_and_stamp;
use crate::eval::helpers::tabs_frame_push;
use crate::eval::helpers::tabs_opaque_pop;
use crate::eval::helpers::tabs_opaque_push;
use crate::serialize::serialize_value_inspect;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::ast::css::at_rule::ModifiableCssAtRule;
use crate::ast::css::declaration::ModifiableCssDeclaration;
use crate::ast::css::media_query::parse_list_with_map;
use crate::ast::css::supports_rule::ModifiableCssSupportsRule;
use crate::ast::sass::at_root_query::AtRootQuery;
use crate::ast::sass::import::Import;
use crate::ast::sass::interpolation_map::InterpolationMap;
#[cfg(feature = "async")]
use crate::callable::BuiltInCallback;
use crate::callable::{Callable, CallableKind, UserDefinedCallable};
use crate::common::pretty_uri::pretty_uri;
use crate::common::span::Span;
use crate::common::span_error::SpanError;
use crate::eval::importer::Importer;
use crate::extend::{ExtendMode, Extension, StoreBox};
use crate::parse::keyframe_selector::KeyframeSelectorParser;
use crate::parse::selector_parse::SelectorParser;
use crate::unvendor::unvendor;
use crate::url::SassUrl;
use crate::util::trim_ascii::trim_ascii;
use crate::util::utils::{pluralize, to_sentence};
use bumpalo::Bump;
use indexmap::IndexMap;

use std::collections::{HashMap, HashSet};

use crate::ast::css::comment::{CssComment, ModifiableCssComment};
use crate::ast::css::import::{CssImport, ModifiableCssImport};
use crate::ast::css::keyframe_block::ModifiableCssKeyframeBlock;
use crate::ast::css::media_rule::ModifiableCssMediaRule;
use crate::ast::css::modifiable_node::{ModifiableCssNode, ModifiableCssNodeKind};
use crate::ast::css::node::CssNode;
use crate::ast::css::style_rule::{CssStyleRule, ModifiableCssStyleRule};
use crate::ast::css::stylesheet::{CssStylesheet, ModifiableCssStylesheet};
use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression_null::NullExpression;
use crate::ast::sass::statement::at_root_rule::AtRootRule;
use crate::ast::sass::statement::at_rule::AtRule;
use crate::ast::sass::statement::content_block::ContentBlock;
use crate::ast::sass::statement::content_rule::ContentRule;
use crate::ast::sass::statement::debug_rule::DebugRule;
use crate::ast::sass::statement::declaration::Declaration;
use crate::ast::sass::statement::each_rule::EachRule;
use crate::ast::sass::statement::error_rule::ErrorRule;
use crate::ast::sass::statement::extend_rule::ExtendRule;
use crate::ast::sass::statement::for_rule::ForRule;
use crate::ast::sass::statement::forward_rule::ForwardRule;
use crate::ast::sass::statement::function_rule::FunctionRule;
use crate::ast::sass::statement::if_rule::IfRule;
use crate::ast::sass::statement::include_rule::IncludeRule;
use crate::ast::sass::statement::loud_comment::LoudComment;
use crate::ast::sass::statement::media_rule::MediaRule;
use crate::ast::sass::statement::mixin_rule::MixinRule;
use crate::ast::sass::statement::return_rule::ReturnRule;
use crate::ast::sass::statement::silent_comment::SilentComment;
use crate::ast::sass::statement::style_rule::StyleRule;
use crate::ast::sass::statement::stylesheet::Stylesheet;
use crate::ast::sass::statement::variable_declaration::VariableDeclaration;
use crate::ast::sass::statement::warn_rule::WarnRule;
use crate::ast::sass::statement::while_rule::WhileRule;
use crate::ast::sass::statement::{
    CallableDeclaration, ImportRule, Statement, SupportsRule, UseRule,
};
use crate::common::ast_css_value::CssValue;
use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::common::source_span_file_source::FileSource;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::configuration::Configuration;
use crate::configuration::ConfiguredValue;
use crate::deprecation;
use crate::environment::Environment;
use crate::eval::css::evaluate_css_stylesheet;
use crate::eval::expression::{evaluate_expression, visit_supports_condition};
use crate::eval::helpers::{
    add_child, add_exception_span, add_exception_trace, copy_parent_after_sibling,
    evaluate_arguments, exception, expression_node, file_span_to_ctx, has_css_nesting,
    merge_media_queries, perform_interpolation, perform_interpolation_with_map,
    run_user_defined_callable, serialize_value, stack_trace, style_rule, with_environment,
    with_media_queries, with_parent, with_stack_frame, without_slash,
};
use crate::eval::imported_css::dispatch_imported_css_child;
use crate::eval::warn::{
    flush_buffered_warnings, warn, warn_deprecation, warn_deprecation_multi_span,
    warn_deprecation_span,
};
use crate::eval::{EvalConfig, EvalState};
use crate::extend::store::{DefaultExtensionStore, ExtensionStore};
use crate::logger::BufferedWarnLogger;
use crate::module::{Module, ModuleKind};
use crate::selector::SimpleSelector;
use crate::value::{assert_number, ListSeparator, SassArgumentList, SassNumber, Value, ValueKind};

// ===========================================================================
// evaluate_statement — internal dispatch
// ===========================================================================

/// Evaluates one statement, returning the `@return` value when the statement
/// (or a nested block) returns one, else `None`.
///
/// Matches Dart: `Statement.accept(this)` dispatch into the `visit*` family
/// (`evaluate.dart:1167+`). There is no visitor-trait impl: this `match` over
/// [`Statement`] *is* the dispatch. Async-recursing arms go through
/// `box_rec_in!` so the future stays boxed (see `ref/macros.md`); leaf arms
/// (`mixin`/`function`/`content-block`/`silent-comment`) run inline because
/// they only register a callable and never recurse.

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_statement<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    stmt: &Statement<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    match stmt {
        Statement::Stylesheet(s) => {
            box_rec_in!(evaluate_stylesheet(config, state, arena, s), arena).await
        }
        Statement::VariableDeclaration(v) => {
            box_rec_in!(
                evaluate_variable_declaration(config, state, arena, v),
                arena,
            )
            .await
        }
        Statement::WarnRule(w) => {
            box_rec_in!(evaluate_warn_rule(config, state, arena, w), arena).await
        }
        Statement::DebugRule(d) => {
            box_rec_in!(evaluate_debug_rule(config, state, arena, d), arena).await
        }
        Statement::ErrorRule(e) => {
            box_rec_in!(evaluate_error_rule(config, state, arena, e), arena).await
        }
        Statement::ReturnRule(r) => {
            box_rec_in!(evaluate_return_rule(config, state, arena, r), arena).await
        }
        Statement::StyleRule(s) => {
            box_rec_in!(evaluate_style_rule(config, state, arena, s), arena).await
        }
        Statement::MediaRule(m) => {
            box_rec_in!(evaluate_media_rule(config, state, arena, m), arena).await
        }
        Statement::IfRule(i) => {
            box_rec_in!(evaluate_statement_if_rule(config, state, arena, i), arena).await
        }
        Statement::EachRule(e) => {
            box_rec_in!(evaluate_statement_each_rule(config, state, arena, e), arena,).await
        }
        Statement::ForRule(f) => {
            box_rec_in!(evaluate_statement_for_rule(config, state, arena, f), arena).await
        }
        Statement::WhileRule(w) => {
            box_rec_in!(
                evaluate_statement_while_rule(config, state, arena, w),
                arena,
            )
            .await
        }
        Statement::IncludeRule(i) => {
            box_rec_in!(evaluate_include_rule(config, state, arena, i), arena).await
        }
        Statement::Declaration(d) => {
            box_rec_in!(evaluate_declaration(config, state, arena, d), arena).await
        }
        Statement::MixinRule(m) => evaluate_mixin_rule(config, state, arena, m),
        Statement::FunctionRule(f) => evaluate_function_rule(config, state, arena, f),
        Statement::ContentBlock(c) => evaluate_content_block(config, state, arena, c),
        Statement::ContentRule(c) => {
            box_rec_in!(evaluate_content_rule(config, state, arena, c), arena).await
        }
        Statement::LoudComment(l) => {
            box_rec_in!(evaluate_loud_comment(config, state, arena, l), arena).await
        }
        Statement::SilentComment(s) => evaluate_silent_comment(config, state, s),
        Statement::ExtendRule(e) => {
            box_rec_in!(evaluate_extend_rule(config, state, arena, e), arena).await
        }
        Statement::AtRule(a) => box_rec_in!(evaluate_at_rule(config, state, arena, a), arena).await,
        Statement::AtRootRule(a) => {
            box_rec_in!(evaluate_at_root_rule(config, state, arena, a), arena).await
        }
        Statement::ForwardRule(f) => {
            box_rec_in!(evaluate_forward_rule(config, state, arena, f), arena).await
        }
        Statement::ImportRule(i) => {
            box_rec_in!(evaluate_import_rule(config, state, arena, i), arena).await
        }
        Statement::SupportsRule(s) => {
            box_rec_in!(evaluate_supports_rule(config, state, arena, s), arena).await
        }
        Statement::UseRule(u) => {
            box_rec_in!(evaluate_use_rule(config, state, arena, u), arena).await
        }
    }
}

// ===========================================================================
// evaluateBlock — evaluates a vec of statements
// Go: evaluate_statement.go:39
// ===========================================================================

/// Runs `children` in order, short-circuiting on the first `@return` value.
///
/// Matches Dart: `_handleReturn` (`evaluate.dart:4329`) as used by every
/// block visitor — `Ok(Some(v))` stops iteration and propagates outward, so
/// `@return` unwinds through nested blocks without a `return_value` field on
/// state.

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_block<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    children: &[Statement<'parse>],
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    for child in children {
        if let Some(val) = evaluate_statement(config, state, arena, child).await? {
            return Ok(Some(val));
        }
    }
    Ok(None)
}

// ===========================================================================
// VisitStylesheet
// Go: evaluate_statement.go:375
// ===========================================================================

/// Evaluates a whole stylesheet: parse-time warnings first, then each child.
///
/// Matches Dart: `visitStylesheet` (`evaluate.dart:1167`). The trailing
/// `global_variables` loop pre-declares `!global` slots from unreachable code
/// as guarded `null` so they still appear in the module's exported variables.

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_stylesheet<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &Stylesheet<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    // Emit parse-time warnings (matching Go VisitStylesheet line 376-380).
    for warning in &node.parse_time_warnings {
        if let Some(dep) = warning.deprecation {
            if !warning.secondary.is_empty() {
                warn_deprecation_multi_span(
                    config,
                    state,
                    &warning.message,
                    warning.span,
                    warning.primary_label.as_deref().unwrap_or(""),
                    warning.secondary.clone(),
                    dep,
                )?;
            } else {
                warn_deprecation_span(config, state, &warning.message, dep, warning.span)?;
            }
        } else {
            warn(config, state, &warning.message, warning.span)?;
        }
    }
    for child in &node.children {
        evaluate_statement(config, state, arena, child).await?;
    }

    // Go: VisitStylesheet lines 388-404 — pre-declare !global variable slots
    // from dead code as guarded null declarations so they appear in the module's
    // exported variables even though the !global assignment never executed.
    for (name, span) in &node.global_variables {
        let null_expr = Expression::Null(NullExpression::new(*span));
        let decl = VariableDeclaration::new(
            name.clone(),
            null_expr,
            *span,
            None,  // no namespace
            true,  // guarded — won't overwrite existing non-null value
            false, // not global — just creating the slot
            None,  // no comment
        )?;
        evaluate_variable_declaration(config, state, arena, &decl).await?;
    }

    Ok(None)
}

// ===========================================================================
// VisitVariableDeclaration
// Go: evaluate_statement.go:409
// ===========================================================================

/// Assigns a variable, honoring `!guarded` (`!default`/`!global` interplay)
/// and module configuration overrides.
///
/// Matches Dart: `visitVariableDeclaration` (`evaluate.dart:2650`). Guarded
/// declarations at the module root first consult the pending configuration
/// (a non-`null` override wins and is set `global`); otherwise an existing
/// non-`null` value is kept. New `!global` declarations warn
/// (`new-global` deprecation). The RHS evaluates bare — only the
/// `set_variable` call is span-wrapped — and `/`-as-division values are
/// slash-stripped via `without_slash`.

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_variable_declaration<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &VariableDeclaration<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    if node.guarded {
        if node.namespace.is_none() && state.env.at_root() {
            state.env.mark_variable_configurable(&node.name);
            if let Some(override_val) = state.configuration.remove(&node.name) {
                if override_val.value != Value::new_with_arena(arena, ValueKind::Null) {
                    add_exception_span(config, state, node.span, None, async |_config, _state| {
                        _state.env.set_variable(
                            &node.name,
                            override_val.value,
                            override_val.assignment_span,
                            None,
                            true,
                        )
                    })
                    .await?;
                    return Ok(None);
                }
            }
        }

        let existing =
            add_exception_span(config, state, node.span, None, async |_config, _state| {
                _state
                    .env
                    .get_variable(&node.name, node.namespace.as_deref())
            })
            .await?;
        if let Some(ref existing) = existing {
            if *existing != Value::new_with_arena(arena, ValueKind::Null) {
                return Ok(None);
            }
        }
    }

    if node.is_global {
        let exists = state.env.global_variable_exists(&node.name, None)?;
        if !exists {
            let message = if state.env.at_root() {
                "As of Dart Sass 2.0.0, !global assignments won't be able to declare new variables.\n\nSince this assignment is at the root of the stylesheet, the !global flag is\nunnecessary and can safely be removed.".to_string()
            } else {
                let original_name = node.original_name()?;
                format!(
                    "As of Dart Sass 2.0.0, !global assignments won't be able to declare new variables.\n\nRecommendation: add `{original_name}: null` at the stylesheet root."
                )
            };
            warn_deprecation_span(config, state, &message, &deprecation::NEW_GLOBAL, node.span)?;
        }
    }

    // Dart `visitVariableDeclaration` evaluates the RHS bare (only the
    // `setVariable` call is span-wrapped); the extra wrapper converted
    // unspanned errors at the declaration span instead of deferring to the
    // outer call-site span.
    let val = evaluate_expression(config, state, arena, &node.expression).await?;
    let val = without_slash(config, state, arena, val, node.expression.span()?)?;
    let expr_node = expression_node(config, state, &node.expression)?;
    add_exception_span(config, state, node.span, None, async |_config, _state| {
        _state.env.set_variable(
            &node.name,
            val,
            expr_node,
            node.namespace.as_deref(),
            node.is_global,
        )
    })
    .await?;
    Ok(None)
}

// ===========================================================================
// VisitWarnRule
// Go: evaluate_statement.go:2384
// ===========================================================================

/// Emits `@warn`: evaluates the message inside the rule span and logs it with
/// a stack trace.
///
/// Matches Dart: `visitWarnRule` (`evaluate.dart:2737`). Strings log verbatim;
/// other values are serialized (inspect mode).

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_warn_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &WarnRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    // Dart `visitWarnRule` evaluates the expression inside
    // `_addExceptionSpan(node, ...)` so unspanned errors surface at the
    // `@warn` node span.
    let val = add_exception_span(config, state, node.span, None, async |config, state| {
        evaluate_expression(config, state, arena, &node.expression).await
    })
    .await?;
    let msg = match &*val {
        ValueKind::String(s) => s.text.to_string(),
        _ => serialize_value(config, state, &val, node.expression.span()?, true).await?,
    };
    let trace = stack_trace(state, Some(node.span));
    config.logger.warn(&msg, None, Some(&trace));
    Ok(None)
}

// ===========================================================================
// VisitDebugRule
// Go: evaluate_statement.go:2409
// ===========================================================================

/// Emits `@debug`: evaluates the expression and logs its inspect string.
///
/// Matches Dart: `visitDebugRule` (`evaluate.dart:1363`). Like `@warn` but
/// debug-channel, and unlike `@warn` the expression is *not* span-wrapped;
/// plain strings log verbatim, other values via inspect serialization.

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_debug_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &DebugRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let val = evaluate_expression(config, state, arena, &node.expression).await?;
    let msg = match &*val {
        ValueKind::String(s) => s.text.to_string(),
        _ => serialize_value_inspect(&val)?,
    };
    let span_s = Span::File(node.span);
    config.logger.debug(&msg, Some(&span_s));
    Ok(None)
}

// ===========================================================================
// VisitErrorRule
// Go: evaluate_statement.go:2431
// ===========================================================================

/// Throws `@error`: evaluates the message and raises it at the rule span.
///
/// Matches Dart: `visitErrorRule` (`evaluate.dart:1476`). The message value
/// is stringified (inspect serialization) and thrown via `exception` so the
/// error carries this rule's span and trace.

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_error_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &ErrorRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let val = evaluate_expression(config, state, arena, &node.expression).await?;
    let msg = serialize_value(config, state, &val, node.expression.span()?, true).await?;
    Err(Box::new(exception(state, msg, Some(node.span))))
}

// ===========================================================================
// VisitReturnRule
// Go: evaluate_statement.go:1774
// ===========================================================================

/// Evaluates `@return` and yields its value to the enclosing block.
///
/// Matches Dart: `visitReturnRule` (`evaluate.dart:2368`). The `Some` return
/// is what [`evaluate_block`] short-circuits on; the value is slash-stripped
/// (`_withoutSlash`) at the expression span.

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_return_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &ReturnRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let val = evaluate_expression(config, state, arena, &node.expression).await?;
    let span = node.expression.span()?;
    let val = without_slash(config, state, arena, val, span)?;
    Ok(Some(val))
}

// ===========================================================================
// VisitStyleRule — complex (needs selectors, withParent, scope)
// Go: evaluate_statement.go:477
// ===========================================================================

/// Splits `visitStyleRule`'s selector phase from its child-evaluation phase.
///
/// Interpolates the selector, parses it (keyframe selectors take a separate
/// parser when `in_keyframes`), nests it within the enclosing style rule
/// unless merging is disabled (top-level, `from_plain_css`, or plain-CSS with
/// a parent selector), and registers it with the extension store. Returns the
/// prepared CSS node plus the `merge` flag that controls the `through`
/// predicate used when attaching children. Matches Dart: `visitStyleRule`
/// selector half (`evaluate.dart:2373-2470`).
struct PreparedStyleRule<'parse> {
    rule_node: ModifiableCssNode<'parse>,
    css_rule: CssStyleRule<'parse>,
    merge: bool,
}

fn prepare_style_rule_nodes<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &StyleRule<'parse>,
    selector_text: String,
    selector_map: &'parse InterpolationMap<'parse>,
) -> SassResult<PreparedStyleRule<'parse>>
where
    'compile: 'parse,
{
    let plain_css = state.stylesheet.as_ref().is_some_and(|s| s.plain_css);
    let mut parsed_selector = {
        let source = FileSource::new_in(arena, &selector_text, None);
        let mut parser = SelectorParser::new_with_options(
            arena,
            source,
            true,
            plain_css,
            Some(config.logger.clone()),
            None,
            Some(selector_map),
        );
        parser.parse()?
    };

    // Merge logic
    let merge = {
        let parent = style_rule(state);
        match parent {
            None => true,
            Some(sr) if sr.from_plain_css => false,
            Some(_) => {
                let has_parent = parsed_selector.contains_parent_selector()?;
                !(plain_css && has_parent)
            }
        }
    };

    if merge {
        if plain_css {
            for complex in &parsed_selector.0.components {
                if !complex.leading_combinators.is_empty() {
                    let first = &complex.leading_combinators[0];
                    let first_span = first.span()?;
                    return Err(Box::new(exception(
                        state,
                        "Top-level leading combinators aren't allowed in plain CSS.".into(),
                        Some(first_span),
                    )));
                }
            }
        }
        let parent = state
            .style_rule_ignoring_at_root
            .as_ref()
            .map(|sr| &sr.original_selector);
        let implicit = !state.at_root_excluding_style_rule;
        parsed_selector = parsed_selector.nest_within(arena, parent, implicit, plain_css)?;
    }

    // Extension store
    let selector_box = match state.extension_store.as_mut() {
        Some(ref mut store) => {
            store.add_selector(arena, &parsed_selector, state.media_queries.clone())?
        }
        None => {
            let inner = Rc::new(RefCell::new(parsed_selector));
            StoreBox { inner }
        }
    };

    let from_plain_css = state.stylesheet.as_ref().is_some_and(|s| s.plain_css);
    let msr = ModifiableCssStyleRule::new(
        Rc::clone(&selector_box.inner),
        node.span,
        Some(parsed_selector),
        from_plain_css,
    );
    let rule_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::StyleRule(msr));
    let css_rule = CssStyleRule::new(
        *selector_box.inner.borrow(),
        node.span,
        parsed_selector,
        from_plain_css,
    );

    Ok(PreparedStyleRule {
        rule_node,
        css_rule,
        merge,
    })
}

#[rust_sass_macros::maybe_async]
/// Evaluates a style rule: selector resolution plus scoped child evaluation.
///
/// Matches Dart: `visitStyleRule` (`evaluate.dart:2373-2478`). Guards (nested
/// declarations, keyframe blocks) come first; the selector phase is delegated
/// to [`prepare_style_rule_nodes`], children to `evaluate_style_rule_children`.
/// Clears `at_root_excluding_style_rule` for the rule body (restored after),
/// warns for bogus combinators, and marks the last top-level child as group
/// end so the serializer emits a blank line between groups.
pub(crate) async fn evaluate_style_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &StyleRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    if state.declaration_name.is_some() {
        return Err(Box::new(exception(
            state,
            "Style rules may not be used within nested declarations.".into(),
            Some(node.span),
        )));
    }

    if state.in_keyframes {
        if let Some(ref parent) = state.parent {
            let is_kfb = {
                let kind = parent.kind();
                matches!(&*kind, ModifiableCssNodeKind::KeyframeBlock(_))
            };
            if is_kfb {
                return Err(Box::new(exception(
                    state,
                    "Style rules may not be used within keyframe blocks.".into(),
                    Some(node.span),
                )));
            }
        }
    }

    let selector = node
        .selector
        .as_ref()
        .ok_or_else(|| exception(state, "Expected a selector.".into(), Some(node.span)))?;
    let (selector_text, selector_map) =
        perform_interpolation_with_map(config, state, arena, selector, true).await?;

    // Keyframe branch
    if state.in_keyframes {
        let source = FileSource::new_in(arena, &selector_text, None);
        let mut kp = KeyframeSelectorParser::new(source);
        let parsed = kp.parse()?;
        let selector_span = selector.span()?;
        let rule = ModifiableCssKeyframeBlock::new(CssValue::new(parsed, selector_span), node.span);
        let rule_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::KeyframeBlock(rule));
        let through: Option<&dyn Fn(&ModifiableCssNode<'parse>) -> bool> =
            Some(&|n| n.is_style_rule());
        return with_parent(
            arena,
            config,
            state,
            rule_node,
            through,
            Some(has_declarations(&node.children)),
            async |c, s| {
                for child in &node.children {
                    evaluate_statement(c, s, arena, child).await?;
                }
                Ok(())
            },
        )
        .await
        .map(|_| None);
    }

    let prep = Box::new(
        add_exception_trace(state, async |state| {
            prepare_style_rule_nodes(config, state, arena, node, selector_text, selector_map)
        })
        .await?,
    );
    evaluate_style_rule_children(config, state, arena, node, &prep).await
}

#[rust_sass_macros::maybe_async]
/// Evaluates style-rule children inside the new rule node.
///
/// Attaches the prepared node via `add_child` (with a style-rule `through`
/// predicate only when selector merging is on, so bubbled rules route
/// correctly), swaps in `parent`, evaluates each child in an environment
/// scope gated on `has_declarations`, then restores. The `parent` save/restore
/// replaces Dart's `_withParent` here because the rule node is constructed up
/// front rather than inside the combinator callback.
async fn evaluate_style_rule_children<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &StyleRule<'parse>,
    prep: &PreparedStyleRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let through: Option<&dyn Fn(&ModifiableCssNode<'parse>) -> bool> = if prep.merge {
        Some(&|n| n.is_style_rule())
    } else {
        None
    };

    add_child(arena, state, &prep.rule_node, through)?;
    // libsass NESTED tabs: stamp hoisted nodes on completion when this rule
    // bears props (see `tabs_frame_pop_and_stamp`).
    tabs_frame_push(state, &prep.rule_node);

    let old_parent = state.parent.replace(prep.rule_node.clone());
    let old_at_root = state.at_root_excluding_style_rule;
    state.at_root_excluding_style_rule = false;

    let env = state.env;
    env.scope(
        arena,
        async || {
            let old_rule = std::mem::take(&mut state.style_rule_ignoring_at_root);
            let old_node = state.css_style_rule_node.take();
            state.style_rule_ignoring_at_root = Some(prep.css_rule.clone());
            state.css_style_rule_node = Some(prep.rule_node.clone());

            for child in &node.children {
                evaluate_statement(config, state, arena, child).await?;
            }

            state.style_rule_ignoring_at_root = old_rule;
            state.css_style_rule_node = old_node;
            Ok(())
        },
        false,
        has_declarations(&node.children),
    )
    .await?;

    state.at_root_excluding_style_rule = old_at_root;
    state.parent = old_parent;

    if prep.rule_node.children_len() > 0 {
        warn_for_bogus_combinators(config, state, &prep.css_rule, &prep.rule_node)?;
    }

    // Group end
    let style_rule_none = style_rule(state).is_none();
    if style_rule_none {
        if let Some(ref parent) = state.parent {
            if let Some(last_child) = parent.last_child() {
                last_child.set_is_group_end(true);
            }
        }
    }

    tabs_frame_pop_and_stamp(state);

    Ok(None)
}

/// Emits `bogus-combinators` deprecation warnings for `rule`.
///
/// Matches Dart: `_warnForBogusCombinators` (`evaluate.dart:2488`). Skipped
/// when the selector is invisible for other reasons; otherwise useless
/// selectors, leading-combinator selectors (non-plain-CSS only), and
/// nesting-only selectors with non-style-rule children each get their message
/// shape, the last as a multi-span pointing at the offending child.
pub(crate) fn warn_for_bogus_combinators<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    rule: &CssStyleRule<'parse>,
    rule_node: &ModifiableCssNode<'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    if rule.selector.is_invisible_other_than_bogus_combinators() {
        return Ok(());
    }
    for complex in &rule.selector.0.components {
        if !complex.is_bogus() {
            continue;
        }
        let trimmed = complex.to_css_string(true)?.trim().to_string();
        let comp_span = complex.span()?;
        let trimmed_span = comp_span.trim_right().map_err(|e| match e {
            SpanError::Argument(msg) => Box::new(SassError::Script {
                message: msg,
                argument_name: None,
            }),
            SpanError::Range(msg) => Box::new(SassError::Script {
                message: msg,
                argument_name: None,
            }),
            SpanError::Sass(s) => s,
        })?;

        if complex.is_useless() {
            warn_deprecation_span(
                config,
                state,
                &format!(
                    "The selector \"{}\" is invalid CSS. It will be omitted from the generated CSS.\nThis will be an error in Dart Sass 2.0.0.\n\nMore info: https://sass-lang.com/d/bogus-combinators",
                    trimmed,
                ),
                &deprecation::BOGUS_COMBINATORS,
                trimmed_span,
            )?;
        } else if !complex.leading_combinators.is_empty() {
            let plain_css = state.stylesheet.as_ref().is_some_and(|s| s.plain_css);
            if !plain_css {
                warn_deprecation_span(
                    config,
                    state,
                    &format!(
                        "The selector \"{}\" is invalid CSS.\nThis will be an error in Dart Sass 2.0.0.\n\nMore info: https://sass-lang.com/d/bogus-combinators",
                        trimmed,
                    ),
                    &deprecation::BOGUS_COMBINATORS,
                    trimmed_span,
                )?;
            }
        } else {
            let mut msg = format!(
                "The selector \"{}\" is only valid for nesting and shouldn't\n\
                 have children other than style rules.",
                trimmed
            );
            if complex.is_bogus_other_than_leading_combinator() {
                msg.push_str(" It will be omitted from the generated CSS.");
            }
            msg.push_str(
                "\nThis will be an error in Dart Sass 2.0.0.\n\n\
                 More info: https://sass-lang.com/d/bogus-combinators",
            );
            let children = rule_node.children();
            let secondary: Vec<(FileSpan<'parse>, String)> = match children {
                Some(c) if !c.is_empty() => {
                    let child = &c[0];
                    let label = if c
                        .iter()
                        .all(|child| matches!(&*child.kind(), ModifiableCssNodeKind::Comment(_)))
                    {
                        "this is not a style rule\n(try converting to a //-style comment)"
                    } else {
                        "this is not a style rule"
                    };
                    vec![(child.span()?, label.to_string())]
                }
                _ => vec![],
            };
            warn_deprecation_multi_span(
                config,
                state,
                &msg,
                trimmed_span,
                "invalid selector",
                secondary,
                &deprecation::BOGUS_COMBINATORS,
            )?;
        }
    }
    Ok(())
}

// ===========================================================================
// combineCss — combines CSS from a module and its transitive upstream modules
// Go: evaluate.go:457
// Dart: evaluate.dart:1024
// ===========================================================================

/// Returns the index of the first child after the leading `@import` block.
///
/// Matches Dart: `_indexAfterImports` (`evaluate.dart:1149`). Comments between
/// imports count as part of the block; anything else ends it.
fn index_after_imports(statements: &[ModifiableCssNode<'_>]) -> usize {
    let mut last_import: isize = -1;
    for (i, stmt) in statements.iter().enumerate() {
        let css = stmt.to_css_node();
        match css {
            CssNode::Import(_) => last_import = i as isize,
            CssNode::Comment(_) => continue,
            _ => break,
        }
    }
    (last_import + 1) as usize
}

/// Reads a module's CSS root regardless of module kind.
///
/// Recurses through `Forwarded`/`Shadowed` wrappers to the underlying
/// environment or built-in module CSS. Needed because [`combine_css`] walks
/// upstream CSS without borrowing across the `ModuleKind` variants.
fn css_of_module<'r, 'compile: 'parse, 'parse>(
    module: &'r Module<'compile, 'parse>,
) -> &'r ModifiableCssNode<'parse> {
    match module.kind() {
        ModuleKind::Environment(e) => &e.css,
        ModuleKind::BuiltIn(b) => &b.css,
        ModuleKind::Forwarded(f) => css_of_module(&f.inner),
        ModuleKind::Shadowed(s) => css_of_module(&s.inner),
    }
}

/// Applies downstream extension stores to each module in reverse topological
/// order. Matches Dart: _extendModules (evaluate.dart:1083)
// Local `HashSet<Extension>`/`HashSet<SimpleSelector>` keys carry span memo
// `Cell`s excluded from `Eq`/`Hash`; the lint is a false positive by design.
#[allow(clippy::mutable_key_type)]
fn extend_modules<'compile, 'parse>(
    arena: &'compile Bump,
    sorted: &VecDeque<Module<'compile, 'parse>>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let mut downstream_stores: HashMap<Option<SassUrl>, Vec<ExtensionStore<'parse>>> =
        HashMap::new();

    // Dart:1089-1094 / Go:580-581 — global unsatisfied tracking, not per-module immediate error.
    // Uses HashSet with identity-based equality (Rc::ptr_eq, matching Dart's Set.identity).
    let mut unsatisfied: HashSet<Extension<'parse>> = HashSet::new();

    for module in sorted {
        let store = module.extension_store();

        // Dart:1097-1101 / Go:584-587 — snapshot original selectors BEFORE addExtensions.
        // Without this, selectors added by downstream stores could falsely satisfy
        // the CURRENT module's extensions (sibling extensions).
        let original_selectors: HashSet<SimpleSelector> =
            HashSet::from_iter(store.simple_selectors());

        // Dart:1103-1110 / Go:589-594 — accumulate unsatisfied globally.
        // extensions_where_target returns owned Extensions (Rc clones), no borrow of store.
        for ext in store.extensions_where_target(&|target| !original_selectors.contains(target)) {
            unsatisfied.insert(ext);
        }

        // Dart:1112-1114 / Go:600-604 — add downstream stores.
        if let Some(url) = module.url().ok().flatten() {
            if let Some(downstream) = downstream_stores.remove(&Some(url)) {
                store.add_extensions(arena, &downstream)?;
            }
        }

        // Dart:1117-1123 / Go:609-620 — push to upstream.
        if !store.is_empty() {
            for upstream in module.upstream() {
                if let Some(up_url) = upstream.url().ok().flatten() {
                    downstream_stores
                        .entry(Some(up_url))
                        .or_default()
                        .push(store.clone());
                }
            }
        }

        // Dart:1125-1130 / Go:622-627 — remove satisfied from global set.
        // Uses ORIGINAL selectors snapshot (not post-addExtensions), matching Dart.
        let satisfied: Vec<Extension<'parse>> =
            store.extensions_where_target(&|target| original_selectors.contains(target));
        for ext in &satisfied {
            unsatisfied.remove(ext);
        }
    }

    // Dart:1133-1135 / Go:630-634 — error only at end, after all modules processed.
    if let Some(unsat) = unsatisfied.iter().next() {
        let span = unsat.span()?;
        return Err(Box::new(SassError::Sass {
            message: format!(
                "The target selector was not found.\n\
                 Use \"@extend {} !optional\" to avoid this error.",
                unsat.target()
            ),
            span: SourceSpanWithContext::from_file_span(&span)?,
            cause: None,
            loaded_urls: vec![],
        }));
    }

    Ok(())
}

/// Merges a module's transitive CSS into one stylesheet.
///
/// Matches Dart: `_combineCss` (`evaluate.dart:1024`). Fast path: with no
/// upstream CSS, checks extensions and returns the root CSS as-is. Otherwise
/// walks upstream modules in reverse topological order, hoisting leading
/// `@import`s (plus interleaved comments) into a shared import block, then
/// applies downstream extensions when any module contains them. `clone`
/// copies each module's CSS first so `load-css()` re-evaluation never mutates
/// the cached module (Dart's `clone: true`).
// Local `HashSet<SimpleSelector>`/`HashSet<Module>` keys as in
// `extend_modules` above; false positive by design.
#[allow(clippy::mutable_key_type)]
pub(crate) fn combine_css<'compile, 'parse>(
    arena: &'compile Bump,
    root: &Module<'compile, 'parse>,
    clone: bool,
) -> SassResult<CssStylesheet<'parse>>
where
    'compile: 'parse,
{
    if !root
        .upstream()
        .iter()
        .any(|m| m.transitively_contains_css())
    {
        let ext_store = root.extension_store();
        let selectors: HashSet<SimpleSelector> = HashSet::from_iter(ext_store.simple_selectors());
        if let Some(unsatisfied) = ext_store
            .extensions_where_target(&|target| !selectors.contains(target))
            .first()
        {
            let span = unsatisfied.span()?;
            return Err(Box::new(SassError::Sass {
                message: format!(
                    "The target selector was not found.\n\
                     Use \"@extend {} !optional\" to avoid this error.",
                    unsatisfied.target()
                ),
                span: SourceSpanWithContext::from_file_span(&span)?,
                cause: None,
                loaded_urls: vec![],
            }));
        }
        let root_css = css_of_module(root);
        let frozen: Vec<CssNode<'parse>> = root_css
            .children()
            .unwrap_or_default()
            .iter()
            .map(|c| c.to_css_node())
            .collect();
        return Ok(CssStylesheet::new(frozen, root_css.span()?));
    }

    let mut imports: Vec<ModifiableCssNode<'parse>> = Vec::new();
    let mut css: Vec<ModifiableCssNode<'parse>> = Vec::new();
    let mut sorted: VecDeque<Module<'compile, 'parse>> = VecDeque::new();
    let mut seen: HashSet<Module<'compile, 'parse>> = HashSet::new();

    // `seen` key rationale as in `combine_css` above.
    #[allow(clippy::mutable_key_type)]
    fn visit<'compile, 'parse>(
        // Recursive import/CSS collector for [`combine_css`].
        //
        // Matches Dart: the `visitModule` closure inside `_combineCss`
        // (`evaluate.dart:1051`). Dedups via `seen` (original modules when
        // `clone` is set), interleaves pre-module comments with the leading
        // `@import` block, and prepends each module to `sorted` so extensions
        // apply in reverse topological order.
        arena: &'compile Bump,
        module: &Module<'compile, 'parse>,
        imports: &mut Vec<ModifiableCssNode<'parse>>,
        css: &mut Vec<ModifiableCssNode<'parse>>,
        sorted: &mut VecDeque<Module<'compile, 'parse>>,
        seen: &mut HashSet<Module<'compile, 'parse>>,
        clone: bool,
    ) -> SassResult<()>
    where
        'compile: 'parse,
    {
        if !seen.insert(*module) {
            return Ok(());
        }
        let mod_ = if clone {
            module.clone_css(arena)?
        } else {
            *module
        };

        for upstream in mod_.upstream() {
            if upstream.transitively_contains_css() {
                if let Some(comments) = mod_.pre_module_comments().get(&upstream) {
                    if !comments.is_empty() {
                        let nodes: Vec<ModifiableCssNode<'parse>> = comments
                            .iter()
                            .map(|c| {
                                ModifiableCssNode::new(
                                    arena,
                                    ModifiableCssNodeKind::Comment(ModifiableCssComment::new(
                                        c.text.clone(),
                                        c.span,
                                    )),
                                )
                            })
                            .collect();
                        if css.is_empty() {
                            imports.extend(nodes);
                        } else {
                            css.extend(nodes);
                        }
                    }
                }
                visit(arena, &upstream, imports, css, sorted, seen, clone)?;
            }
        }

        sorted.push_front(mod_);
        let mut statements = css_of_module(&mod_).children().unwrap_or_default();
        let index = index_after_imports(&statements);
        let rest = statements.split_off(index);
        imports.extend(statements);
        css.extend(rest);
        Ok(())
    }

    visit(
        arena,
        root,
        &mut imports,
        &mut css,
        &mut sorted,
        &mut seen,
        clone,
    )?;

    if root.transitively_contains_extensions() {
        extend_modules(arena, &sorted)?;
    }

    let frozen_imports: Vec<CssNode<'parse>> = imports.iter().map(|c| c.to_css_node()).collect();
    let mut frozen_css: Vec<CssNode<'parse>> = css.iter().map(|c| c.to_css_node()).collect();
    let mut combined = frozen_imports;
    combined.append(&mut frozen_css);
    Ok(CssStylesheet::new(combined, css_of_module(root).span()?))
}

// ===========================================================================
// loadStylesheet — load a stylesheet via the import cache
// Go: evaluate_statement.go:945
// Dart: evaluate.dart:1979
// ===========================================================================

/// The result of [`load_stylesheet`]: the parsed stylesheet plus the importer
/// that resolved it and whether it counts as a dependency.
///
/// Matches Dart: the `_LoadedStylesheet` record (`evaluate.dart:4926`).
pub(crate) struct LoadedStylesheet<'parse> {
    pub stylesheet: &'parse Stylesheet<'parse>,
    pub importer: Importer<'parse>,
    pub is_dependency: bool,
}

/// Canonicalizes `parsed_url` and loads the stylesheet, or throws when it
/// cannot be found.
///
/// Matches Dart: `_loadStylesheet` (`evaluate.dart:1984`). Tries the import
/// cache relative to `base_url` (defaulting to the current stylesheet's URL),
/// warning on relative canonical URLs and recording the canonical URL as
/// loaded even when the load itself fails (so watchers still watch it), then
/// falls back to the node importer and the `package:` platform error. Every
/// failure surfaces as a spanned error at `span`; `import_span` is cleared on
/// exit.
#[rust_sass_macros::maybe_async]
pub(crate) async fn load_stylesheet<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    parsed_url: &SassUrl,
    span: FileSpan<'parse>,
    base_url: Option<&SassUrl>,
    for_import: bool,
) -> SassResult<LoadedStylesheet<'parse>>
where
    'compile: 'parse,
{
    state.import_span = Some(span);

    let resolved_base_url: Option<SassUrl> = base_url.cloned().or({
        state
            .stylesheet
            .as_ref()
            .and_then(|s| s.span.source_url().cloned())
    });

    let canonicalize_result = {
        let cache = match state.import_cache.as_mut() {
            Some(c) => c,
            None => {
                state.import_span = None;
                return Err(Box::new(exception(
                    state,
                    "No import cache available.".into(),
                    Some(span),
                )));
            }
        };
        let warn_logger = BufferedWarnLogger::new(arena);
        let result = cache
            .canonicalize(
                parsed_url,
                Some(&state.importer),
                resolved_base_url.as_ref(),
                for_import,
                &warn_logger,
            )
            .await?;
        flush_buffered_warnings(&warn_logger, config, state)?;
        result
    };

    if let Some(ref cr) = canonicalize_result {
        if cr.canonical_url.scheme() == "" {
            warn_deprecation(
                config,
                state,
                &format!(
                    "Importer {} canonicalized {} to {}.\n\
                     Relative canonical URLs are deprecated and will eventually be disallowed.",
                    cr.importer, parsed_url, cr.canonical_url,
                ),
                &deprecation::RELATIVE_CANONICAL,
            )?;
        }

        let canonical_key = cr.canonical_url.to_string();
        // Dart: `_loadedUrls` is a `Set<Uri>` (async_evaluate.dart:254), so a
        // stylesheet loaded more than once appears only once.
        if !state.loaded_urls.contains(&canonical_key) {
            state.loaded_urls.push(canonical_key);
        }

        let is_dependency = state.in_dependency || cr.importer != state.importer;

        let sheet = state
            .import_cache
            .as_mut()
            .unwrap()
            .import_canonical(&cr.importer, &cr.canonical_url, Some(&cr.original_url))
            .await?;

        if let Some(stylesheet) = sheet {
            state.import_span = None;
            return Ok(LoadedStylesheet {
                stylesheet,
                importer: cr.importer,
                is_dependency,
            });
        }
    }

    state.import_span = None;

    // Dart: `_canonicalize` throws a plain `SassException` (no stack trace) for
    // an unresolvable URL, so the reported trace is the span-derived
    // "root stylesheet" frame — not the enclosing @use/@forward/@import frame.
    let span = SourceSpanWithContext::from_file_span(&span)?;
    Err(Box::new(SassError::Sass {
        message: "Can't find stylesheet to import.".into(),
        span,
        cause: None,
        loaded_urls: vec![],
    }))
}

// ===========================================================================
// VisitImportRule — @import directive
// Go: evaluate_statement.go:1166
// ===========================================================================

/// Rebuilds the root child list with out-of-order `@import`s spliced in.
///
/// Matches Dart: `_addOutOfOrderImports` (`evaluate.dart:1007`). Leading
/// imports stay in place; imports that arrived after real CSS are inserted at
/// the `end_of_imports` mark.
pub(crate) fn build_out_of_order_children<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    state: &EvalState<'_, 'parse>,
) -> Vec<ModifiableCssNode<'parse>> {
    if state.out_of_order_imports.is_empty() {
        return state
            .root
            .as_ref()
            .and_then(|r| r.children())
            .map(|c| c.to_vec())
            .unwrap_or_default();
    }
    let children = state
        .root
        .as_ref()
        .and_then(|r| r.children())
        .map(|c| c.to_vec())
        .unwrap_or_default();
    let mut result = Vec::new();
    for child in children
        .iter()
        .take(state.end_of_imports.min(children.len()))
    {
        result.push(child.clone());
    }
    for imp in &state.out_of_order_imports {
        result.push(ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::Import(imp.clone()),
        ));
    }
    for child in children.iter().skip(state.end_of_imports) {
        result.push(child.clone());
    }
    result
}

#[rust_sass_macros::maybe_async]
/// Evaluates legacy `@import`: dynamic imports inline stylesheets, static
/// imports become CSS `@import` nodes.
///
/// Matches Dart: `visitImportRule` + `_visitDynamicImport`/`_visitStaticImport`
/// (`evaluate.dart:1847-2112`). Dynamic imports run under an `@import` stack
/// frame with loop detection; module-free stylesheets evaluate directly in the
/// current environment, while stylesheets that `@use`/`@forward` get an
/// isolated environment/root plus hermetic `@extend` resolution before their
/// CSS is injected. Static imports nest under the current parent or join the
/// leading-import block (`end_of_imports`/`out_of_order_imports` bookkeeping).
pub(crate) async fn evaluate_import_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &ImportRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    for imp in &node.imports {
        match imp {
            Import::Static(s) => {
                let url_str = perform_interpolation(config, state, arena, &s.url, false).await?;
                let url_span = s.url.span()?;
                let url_val = CssValue::new(url_str, url_span);
                let modifiers = if let Some(ref mod_interp) = s.modifiers {
                    let mod_str =
                        perform_interpolation(config, state, arena, mod_interp, false).await?;
                    let mod_span = mod_interp.span()?;
                    Some(CssValue::new(mod_str, mod_span))
                } else {
                    None
                };

                let css_import = CssImport::new(url_val, s.span, modifiers);
                let import_node = ModifiableCssImport::from_css_import(&css_import);
                let child_node = ModifiableCssNode::new(
                    arena,
                    ModifiableCssNodeKind::Import(import_node.clone()),
                );

                let root = state.root.as_ref().unwrap();
                let parent = state.parent.as_ref().unwrap();
                if parent != root {
                    copy_parent_after_sibling(arena, state)?;
                    add_child(arena, state, &child_node, None)?;
                } else if state.end_of_imports == root.children().map_or(0, |c| c.len()) {
                    add_child(arena, state, &child_node, None)?;
                    state.end_of_imports += 1;
                } else {
                    state.out_of_order_imports.push(import_node);
                }
            }

            Import::Dynamic(d) => {
                let url = d.url();
                let import_span = d.span;

                // Dart `_visitDynamicImport` wraps the whole load+evaluate in
                // `_withStackFrame("@import", ...)` (async_evaluate.dart:1852),
                // so load-time parse errors get the `@import` frame. But
                // `Script` errors from the importer (e.g. the ambiguous-import
                // "It's not clear which file to import.") are converted inside
                // Dart's `_loadStylesheet` to a RuntimeException with a
                // stack-frames-only trace (`_exception(msg)` — no current-member
                // frame), so we must NOT add the "@import" member frame here
                // (`Some(false)`, matching the `@use`/`@forward` load path).
                let result = with_stack_frame(
                    config,
                    state,
                    arena,
                    "@import",
                    import_span,
                    None,
                    async |c, s| -> SassResult<()> {
                        let loaded =
                            add_exception_span(c, s, import_span, Some(false), async |c2, s2| {
                                add_exception_trace(s2, async |s3| {
                                    load_stylesheet(c2, s3, arena, &url, import_span, None, true)
                                        .await
                                })
                                .await
                            })
                            .await?;

                        let sheet_url_key =
                            loaded.stylesheet.span.source_url().map(|u| u.to_string());

                        // Circular import detection
                        if let Some(ref key) = sheet_url_key {
                            if !key.is_empty() {
                                if let Some(prev) = s.active_modules.get(key).copied().flatten() {
                                    return Err(Box::new(SassError::MultiSpan {
                                        message: "This file is already being loaded.".into(),
                                        span: SourceSpanWithContext::from_file_span(&import_span)?,
                                        primary_label: Some("new load".into()),
                                        secondary: vec![(
                                            SourceSpanWithContext::from_file_span(&prev)?,
                                            "original load".into(),
                                        )],
                                        original_source: None,
                                        cause: None,
                                        loaded_urls: vec![],
                                        // Dart `_exception(...)` (no span):
                                        // stack frames only, no current-member
                                        // frame (async_evaluate.dart:1877-1879).
                                        trace: stack_trace(s, None),
                                    }));
                                }
                                if s.active_modules.contains_key(key) {
                                    return Err(Box::new(SassError::Runtime {
                                        message: "This file is already being loaded.".into(),
                                        span: SourceSpanWithContext::from_file_span(&import_span)?,
                                        trace: stack_trace(s, None),
                                        cause: None,
                                        loaded_urls: vec![],
                                    }));
                                }
                                s.active_modules.insert(key.clone(), Some(import_span));
                            }
                        }

                        // Fast path: no @use/@forward
                        if loaded.stylesheet.uses().is_empty()
                            && loaded.stylesheet.forwards().is_empty()
                        {
                            let old_importer = s.importer;
                            let old_stylesheet = s.stylesheet;
                            let old_in_dep = s.in_dependency;
                            s.importer = loaded.importer;
                            s.stylesheet = Some(loaded.stylesheet);
                            s.in_dependency = loaded.is_dependency;
                            evaluate_stylesheet(c, s, arena, loaded.stylesheet).await?;
                            s.importer = old_importer;
                            s.stylesheet = old_stylesheet;
                            s.in_dependency = old_in_dep;
                            // Dart's `_visitDynamicImport` fast path removes the
                            // active-module marker before returning
                            // (async_evaluate.dart:1889), so sibling imports of
                            // the same file aren't mistaken for cycles.
                            if let Some(ref key) = sheet_url_key {
                                if !key.is_empty() {
                                    s.active_modules.shift_remove(key);
                                }
                            }
                            return Ok(());
                        }

                        // Full path: uses/forwards modules
                        let loads_user_defined = loaded
                            .stylesheet
                            .uses()
                            .iter()
                            .any(|u| u.url.scheme() != "sass")
                            || loaded
                                .stylesheet
                                .forwards()
                                .iter()
                                .any(|f| f.url.scheme() != "sass");

                        let env = s.env.for_import(arena);
                        let mut children: Vec<ModifiableCssNode<'parse>> = Vec::new();

                        with_environment(c, s, env, async |c2, s2| {
                            let old_importer = s2.importer;
                            let old_stylesheet = s2.stylesheet;
                            let old_root = s2.root.clone();
                            let old_parent = s2.parent.clone();
                            let old_eoi = s2.end_of_imports;
                            let old_ooi = std::mem::take(&mut s2.out_of_order_imports);
                            let old_config = s2.configuration.clone();
                            let old_in_dep = s2.in_dependency;

                            s2.importer = loaded.importer;
                            s2.stylesheet = Some(loaded.stylesheet);
                            if loads_user_defined {
                                let new_root = ModifiableCssNode::new(
                                    arena,
                                    ModifiableCssNodeKind::Stylesheet(
                                        ModifiableCssStylesheet::new(loaded.stylesheet.span),
                                    ),
                                );
                                s2.root = Some(new_root.clone());
                                s2.parent = Some(new_root);
                                s2.end_of_imports = 0;
                                s2.out_of_order_imports.clear();
                            }
                            s2.in_dependency = loaded.is_dependency;

                            if !loaded.stylesheet.forwards().is_empty() {
                                s2.configuration = env.to_implicit_configuration(arena)?;
                            }

                            evaluate_stylesheet(c2, s2, arena, loaded.stylesheet).await?;
                            if loads_user_defined {
                                children = build_out_of_order_children(arena, s2);
                            }

                            s2.importer = old_importer;
                            s2.stylesheet = old_stylesheet;
                            if loads_user_defined {
                                s2.root = old_root;
                                s2.parent = old_parent;
                                s2.end_of_imports = old_eoi;
                                s2.out_of_order_imports = old_ooi;
                            }
                            s2.configuration = old_config;
                            s2.in_dependency = old_in_dep;
                            Ok(())
                        })
                        .await?;

                        let module = env.to_dummy_module(arena);
                        s.env.import_forwards(arena, &module)?;

                        if loads_user_defined {
                            if module.transitively_contains_css() {
                                let combined = combine_css(
                                    arena,
                                    &module,
                                    module.transitively_contains_extensions(),
                                )?;
                                evaluate_css_stylesheet(c, s, arena, &combined).await?;
                            }
                            for child in &children {
                                dispatch_imported_css_child(c, s, arena, &child.to_css_node())
                                    .await?;
                            }
                        }

                        // Cleanup active_modules regardless of success/failure
                        if let Some(ref key) = sheet_url_key {
                            if !key.is_empty() {
                                s.active_modules.shift_remove(key);
                            }
                        }
                        Ok(())
                    },
                )
                .await;

                result?;
            }
        }
    }
    Ok(None)
}

// ===========================================================================
// VisitMediaRule — complex (needs media query merging, CSS tree)
// Go: evaluate_statement.go:1968
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Evaluates `@media`: interpolates and merges queries, then evaluates
/// children with bubbling.
///
/// Matches Dart: `visitMediaRule` + `_visitMediaQueries`/`_mergeMediaQueries`
/// (`evaluate.dart:2258-2366`). Queries parse from the interpolated text with
/// the interpolation map so errors locate the `#{}` site; they merge with any
/// enclosing queries (empty merge drops the rule). In a style rule the rule
/// is copied inward so bare declarations have a home; bubbling passes through
/// style rules and media rules whose queries are all in the merged source set.
/// Children scope on `has_declarations`.
pub(crate) async fn evaluate_media_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &MediaRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    if state.declaration_name.is_some() {
        return Err(Box::new(exception(
            state,
            "Media rules may not be used within nested declarations.".into(),
            Some(node.span),
        )));
    }

    // Evaluate query interpolation + parse media queries (Dart
    // `_visitMediaQueries`: `_performInterpolationWithMap` + parse with the
    // interpolation map so errors locate the interpolation site).
    let (query_text, query_map) =
        perform_interpolation_with_map(config, state, arena, &node.query, false).await?;
    let css_queries = if query_text.is_empty() {
        vec![]
    } else {
        parse_list_with_map(arena, &query_text, Some(query_map))?
    };

    // Go: evaluate_statement.go:1984-2004 — if hasCssNesting(), skip merging/bubbling
    // and add the media rule as a direct child.
    if has_css_nesting(state) {
        let span = node.span()?;
        let rule = ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::MediaRule(ModifiableCssMediaRule::new(css_queries, span)?),
        );
        let tabs_node = rule.clone();
        with_parent(
            arena,
            config,
            state,
            rule,
            None, // through = none (no bubbling)
            Some(false),
            async |config, state| {
                // Opaque NESTED-tabs scope (libsass bubble boundary).
                let tabs_start = tabs_opaque_push(state);
                for child in &node.children {
                    evaluate_statement(config, state, arena, child).await?;
                }
                tabs_opaque_pop(state, &tabs_node, tabs_start);
                Ok(())
            },
        )
        .await?;
        return Ok(None);
    }

    // Merge queries with parent media context if present.
    let (rule_queries, merged_sources) = if let Some(ref mq) = state.media_queries {
        match merge_media_queries(mq, &css_queries) {
            None => (css_queries, vec![]),
            Some(m) if m.is_empty() => {
                return Ok(None);
            }
            Some(m) => {
                let mut sources = state.media_query_sources.clone().unwrap_or_default();
                sources.extend(mq.clone());
                sources.extend(css_queries.clone());
                (m, sources)
            }
        }
    } else {
        (css_queries, vec![])
    };

    let span = node.span()?;

    let rule = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::MediaRule(ModifiableCssMediaRule::new(rule_queries.clone(), span)?),
    );

    // The through function skips over style rules (always) and media rules
    // whose queries are already covered by merged_sources (matching Go:
    // evaluate_statement.go:2043-2063).
    let through_sources = merged_sources.clone();
    let through_fn = move |n: &ModifiableCssNode| {
        if matches!(&*n.kind(), ModifiableCssNodeKind::StyleRule(_)) {
            return true;
        }
        if !through_sources.is_empty() {
            if let ModifiableCssNodeKind::MediaRule(mr) = &*n.kind() {
                let all_contained = mr.queries.iter().all(|q| through_sources.contains(q));
                if all_contained {
                    return true;
                }
            }
        }
        false
    };
    let through: Option<&dyn Fn(&ModifiableCssNode) -> bool> = Some(&through_fn);

    let tabs_node = rule.clone();
    with_parent(
        arena,
        config,
        state,
        rule,
        through,
        Some(has_declarations(&node.children)),
        async |config, state| {
            // Opaque NESTED-tabs scope (libsass bubble boundary).
            let tabs_start = tabs_opaque_push(state);
            with_media_queries(
                config,
                state,
                Some(rule_queries),
                Some(merged_sources),
                async |config, state| {
                    // If inside a style rule, duplicate it without children so
                    // declarations inside the media rule inherit the selector.
                    if let Some(ref sr_node) = state.css_style_rule_node {
                        let sr_copy = sr_node.copy_without_children(arena);
                        // The bubble copy is a style-rule frame for NESTED
                        // tabs: nested bubbles collect `+1` when it bears
                        // props (libsass re-performs bubble content with the
                        // copied rule as parent).
                        tabs_frame_push(state, &sr_copy);
                        with_parent(
                            arena,
                            config,
                            state,
                            sr_copy,
                            None,
                            Some(false),
                            async |c, s| {
                                for child in &node.children {
                                    evaluate_statement(c, s, arena, child).await?;
                                }
                                Ok(())
                            },
                        )
                        .await?;
                        tabs_frame_pop_and_stamp(state);
                    } else {
                        for child in &node.children {
                            if let Statement::StyleRule(sr) = child {
                                evaluate_style_rule(config, state, arena, sr).await?;
                            } else {
                                evaluate_statement(config, state, arena, child).await?;
                            }
                        }
                    }
                    Ok(())
                },
            )
            .await?;
            tabs_opaque_pop(state, &tabs_node, tabs_start);
            Ok(())
        },
    )
    .await?;

    Ok(None)
}

// ===========================================================================
// VisitForRule — @for $var from X through/to Y
// Go: evaluate_statement.go:1833
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Evaluates `@for $var from X through/to Y`, binding an integer per step.
///
/// Matches Dart: `visitForRule` (`evaluate.dart:1632`). Both bounds evaluate
/// inside the bound-expression spans and coerce to the `from` units; the loop
/// runs inclusive (`through`) or exclusive (`to`), counting down when `from >
/// to`. The variable is defined at the `from` expression node inside a
/// semi-global scope, and `@return` inside the body unwinds the loop.
pub(crate) async fn evaluate_statement_for_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &ForRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let from_span = node.from.span()?;
    let to_span = node.to.span()?;

    // Dart `visitForRule`: `node.from`/`node.to` are evaluated *inside*
    // `_addExceptionSpan`, so evaluation errors surface at the bound spans.
    let from_num_ref = add_exception_span(
        config,
        state,
        from_span,
        Some(true),
        async |config, state| {
            let from_val = evaluate_expression(config, state, arena, &node.from).await?;
            Ok(assert_number(&from_val, None)?.clone())
        },
    )
    .await?;

    let to_num_ref =
        add_exception_span(config, state, to_span, Some(true), async |config, state| {
            let to_val = evaluate_expression(config, state, arena, &node.to).await?;
            Ok(assert_number(&to_val, None)?.clone())
        })
        .await?;

    let from: i64 = add_exception_span(config, state, from_span, Some(true), async |_, _| {
        from_num_ref.assert_int(None)
    })
    .await?;

    let from_units = from_num_ref.numerator_units.clone();
    let from_denom = from_num_ref.denominator_units.clone();
    let coerced_to_num = add_exception_span(config, state, to_span, Some(true), async |_, _| {
        to_num_ref.coerce(
            &from_num_ref.numerator_units,
            &from_num_ref.denominator_units,
            None,
        )
    })
    .await?;
    let to = add_exception_span(config, state, to_span, Some(true), async |_, _| {
        coerced_to_num.assert_int(None)
    })
    .await?;

    let mut dir: i64 = 1;
    if from > to {
        dir = -1;
    }
    let mut to_val_i64 = to;
    if !node.is_exclusive {
        to_val_i64 += dir;
    }
    if from == to_val_i64 {
        return Ok(None);
    }

    // Dart `visitForRule`: the loop variable is defined at
    // `_expressionNode(node.from)`, not the rule span.
    let var_span = expression_node(config, state, &node.from)?;
    let env = state.env;
    env.scope(
        arena,
        async || -> SassResult<Option<Value<'parse>>> {
            let mut i = from;
            while i != to_val_i64 {
                let unit_num = SassNumber::new(
                    i as f64,
                    if from_units.is_empty() && from_denom.is_empty() {
                        None
                    } else {
                        Some(&from_units[0])
                    },
                );
                state.env.set_local_variable(
                    &node.variable,
                    Value::new_with_arena(arena, ValueKind::Number(unit_num)),
                    var_span,
                );

                for child in &node.children {
                    if let Some(val) = evaluate_statement(config, state, arena, child).await? {
                        return Ok(Some(val));
                    }
                }

                i += dir;
            }
            Ok(None)
        },
        true,
        true,
    )
    .await
}

// ===========================================================================
// VisitIncludeRule — complex (needs mixin lookup + applyMixin)
// Go: evaluate_statement.go:1448
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Evaluates `@include`: resolves the mixin and runs it with content.
///
/// Matches Dart: `visitIncludeRule` (`evaluate.dart:2187`). Lookup is
/// span-wrapped so a missing namespace reports at the call site; `--`-named
/// mixins reject non-`--` declarations (plain-CSS mixin future-proofing). The
/// content block (if any) becomes a closure-capturing callable, and the body
/// runs through [`apply_mixin`] with a content-less invocation span.
pub(crate) async fn evaluate_include_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &IncludeRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    // Dart `visitIncludeRule`: `_addExceptionSpan(node, () =>
    // _environment.getMixin(node.name, namespace: node.namespace))` — the
    // missing-module `Script` from `get_module` gains its span here.
    let mixin = add_exception_span(config, state, node.span, None, async |_cfg, st| {
        st.env.get_mixin(&node.name, node.namespace.as_deref())
    })
    .await?
    .ok_or_else(|| exception(state, "Undefined mixin.".into(), Some(node.span)))?;

    let mixin_callable = mixin;

    // Go: evaluate_statement.go:1487-1494 — check for custom-ident include
    if node.original_name.starts_with("--") {
        if let CallableKind::UserDefined(u) = mixin_callable.kind() {
            if !u.declaration.original_name().starts_with("--") {
                let name_span = node.name_span()?;
                return Err(Box::new(exception(state,
                    "Sass @mixin names beginning with -- are forbidden for forward-compatibility with plain CSS mixins.\n\nFor details, see https://sass-lang.com/d/css-function-mixin".into(),
                    Some(name_span))));
            }
        }
    }

    // Set up content block if present; inMixin is handled inside apply_mixin

    // Create content callable if @content block is present; will be set
    // inside run_user_defined_callable after with_environment swaps state.env.
    // This ensures the content is scoped to the mixin's closure env, matching
    // Go's pattern (evaluate_statement.go:1737-1757).
    let content = node.content.as_ref().map(|content_block| {
        let content_callable = UserDefinedCallable::new(
            CallableDeclaration::ContentBlock(content_block.clone().as_ref().clone()),
            state.env.closure(arena),
            state.in_dependency,
        );
        Callable::new(arena, CallableKind::UserDefined(content_callable))
    });

    let invocation_span = node.span_without_content()?;
    let _result = apply_mixin(
        config,
        state,
        arena,
        &mixin_callable,
        &node.arguments,
        content,
        node.span()?,
        invocation_span,
    )
    .await?;

    Ok(None)
}

// ===========================================================================
// applyMixin — mixin invocation dispatcher (reused by meta::apply)
// Go: evaluate_statement.go:1662
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Runs a mixin callable with `arguments` and optional `content`.
///
/// Matches Dart: `_applyMixin` (`evaluate.dart:2114`). `None` errors as
/// undefined; built-ins evaluate arguments, pick the overload, fill
/// named/default params, pack the rest into `$kwargs`, and run the callback
/// with `content`/`in_mixin` swapped in (restored after), wrapping `Script`
/// errors at the invocation span; user-defined mixins run through
/// `run_user_defined_callable` with per-statement error spans. Both arms
/// reject content blocks when the declaration does not accept one (multi-span
/// invocation/declaration error). Also the reentrant entry for
/// `meta.apply` — hence `pub(crate)`.
// Arity mirrors Dart's `_applyMixin`; packing into a struct would diverge
// from the port.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_mixin<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    mixin_callable: &Callable<'compile, 'parse>,
    arguments: &ArgumentList<'parse>,
    content: Option<Callable<'compile, 'parse>>,
    span: FileSpan<'parse>,
    invocation_span: FileSpan<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    match mixin_callable.kind() {
        CallableKind::UserDefined(u) => {
            // Go: evaluate_statement.go:1714-1735 — content-acceptance check
            if content.is_some() {
                if let CallableDeclaration::Mixin(m) = &u.declaration {
                    if !m.has_content() {
                        let decl_span = m.parameters.span_with_name();
                        return Err(Box::new(SassError::MultiSpan {
                            message: "Mixin doesn't accept a content block.".into(),
                            span: SourceSpanWithContext::from_file_span(&invocation_span)?,
                            primary_label: Some("invocation".into()),
                            secondary: vec![(
                                SourceSpanWithContext::from_file_span(&decl_span)?,
                                "declaration".into(),
                            )],
                            original_source: None,
                            cause: None,
                            loaded_urls: vec![],
                            trace: stack_trace(state, Some(invocation_span)),
                        }));
                    }
                }
            }
            run_user_defined_callable(
                config,
                state,
                arena,
                u,
                arguments,
                content,
                true,
                invocation_span,
            )
            .await
        }
        CallableKind::BuiltIn(b) => {
            // Go: evaluate_statement.go:1673-1700 — content-acceptance check
            if !b.accepts_content() && content.is_some() {
                let results = evaluate_arguments(config, state, arena, arguments).await?;
                let names: HashSet<&str> = results.named.keys().map(|s| s.as_str()).collect();
                let overload = b.callback_for(results.positional.len(), &names)?;
                let decl_span = overload.params.span_with_name();
                return Err(Box::new(SassError::MultiSpan {
                    message: "Mixin doesn't accept a content block.".into(),
                    span: SourceSpanWithContext::from_file_span(&invocation_span)?,
                    primary_label: Some("invocation".into()),
                    secondary: vec![(
                        SourceSpanWithContext::from_file_span(&decl_span)?,
                        "declaration".into(),
                    )],
                    original_source: None,
                    cause: None,
                    loaded_urls: vec![],
                    trace: stack_trace(state, Some(invocation_span)),
                }));
            }

            // Go: evaluate_statement.go:1526-1534 — evaluate arguments
            let results = evaluate_arguments(config, state, arena, arguments).await?;
            let mut positional: Vec<Value<'parse>> = results.positional;
            let mut named = results.named;
            let separator = results.separator;

            // Go: evaluate_statement.go:1536-1539 — build names set
            let names: HashSet<&str> = named.keys().map(|s| s.as_str()).collect();

            // Go: evaluate_statement.go:1540-1544 — find overload
            let overload = b.callback_for(positional.len(), &names)?;

            // Go: evaluate_statement.go:1546-1553 — verify params
            add_exception_span(config, state, span, Some(true), async |_, _| {
                overload.params.verify(positional.len(), &names, &span)
            })
            .await?;

            // Go: evaluate_statement.go:1554-1577 — fill in named/default params
            for i in positional.len()..overload.params.parameters.len() {
                let param = &overload.params.parameters[i];
                if let Some(val) = named.swap_remove(param.name.as_str()) {
                    positional.push(val);
                } else if let Some(ref default_expr) = param.default_value {
                    let val = evaluate_expression(config, state, arena, default_expr).await?;
                    let node = expression_node(config, state, default_expr)?;
                    let cleaned = without_slash(config, state, arena, val, node)?;
                    positional.push(cleaned);
                }
            }

            // Go: evaluate_statement.go:1580-1598 — build $kwargs... ArgumentList
            let rest_arg_check = if overload.params.rest_parameter.is_some() {
                let rest: Vec<Value<'parse>> =
                    if positional.len() > overload.params.parameters.len() {
                        positional
                            .drain(overload.params.parameters.len()..)
                            .collect()
                    } else {
                        vec![]
                    };
                let sep = if separator == ListSeparator::Undecided {
                    ListSeparator::Comma
                } else {
                    separator
                };
                let arg_list = SassArgumentList::new(arena, rest, named, sep);
                let check = Value::new_with_arena(arena, ValueKind::ArgumentList(arg_list));
                positional.push(check);
                Some(check)
            } else {
                None
            };

            // Wrap in Rc for the callback
            let padded: Vec<Value<'parse>> = positional.into_iter().collect();

            // Go: evaluate_statement.go:1701-1711 — content + in_mixin save/restore
            let old_content = state.env.content();
            if let Some(ref c) = content {
                state.env.set_content(Some(*c));
            } else {
                state.env.set_content(None);
            }
            let old_in_mixin = state.env.in_mixin();
            state.env.set_in_mixin(true);

            // Go: evaluate_statement.go:1600-1633 — invoke callback + error wrapping
            let old_callable_span = state.callable_span;
            // Dart `_runBuiltInCallable` sets `_callableNode = nodeWithSpanWithoutContent`
            // (async_evaluate.dart:3687, applied via applyMixin:2139), so the
            // callable span excludes the content block.
            state.callable_span = Some(invocation_span);
            #[cfg(feature = "async")]
            let _result = match &overload.callback {
                BuiltInCallback::Sync(cb) => cb(config, state, padded, arena),
                BuiltInCallback::Async(cb) => cb(config, state, padded, arena).await,
            };
            #[cfg(not(feature = "async"))]
            let _result = (overload.callback)(config, state, padded, arena);
            let _result = _result
                .map(|_| Value::new_with_arena(arena, ValueKind::Null))
                .map_err(|e| match *e {
                    SassError::Script { .. } => {
                        let message = e.full_message();
                        Box::new(exception(state, message, Some(span)))
                    }
                    other => Box::new(other),
                })?;
            state.callable_span = old_callable_span;

            state.env.set_in_mixin(old_in_mixin);
            state.env.set_content(old_content);

            // Matches Dart (async_evaluate.dart:3746-3757): unused argument-list
            // keywords raise a MultiSpan error with the invocation and
            // declaration spans.
            if let Some(ref check) = rest_arg_check {
                if let ValueKind::ArgumentList(arg_list) = &**check {
                    if !arg_list.were_keywords_accessed.get() && !arg_list.keywords.is_empty() {
                        let names: Vec<String> =
                            arg_list.keywords.keys().map(|k| format!("${k}")).collect();
                        let message = format!(
                            "No {} named {}.",
                            pluralize("parameter", names.len() as i32, None),
                            to_sentence(&names, "or")
                        );
                        return Err(Box::new(SassError::MultiSpan {
                            message,
                            span: file_span_to_ctx(&span),
                            primary_label: Some("invocation".into()),
                            secondary: vec![(
                                file_span_to_ctx(&overload.params.span_with_name()),
                                "declaration".into(),
                            )],
                            original_source: None,
                            cause: None,
                            loaded_urls: vec![],
                            trace: stack_trace(state, Some(span)),
                        }));
                    }
                }
            }

            Ok(Value::new_with_arena(arena, ValueKind::Null))
        }
        _ => Err(Box::new(exception(
            state,
            "Mixin used as function.".into(),
            None,
        ))),
    }
}

// ===========================================================================
// VisitDeclaration
// Go: evaluate_statement.go:2256
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Evaluates a declaration: emits a CSS declaration and/or scopes nested
/// children.
///
/// Matches Dart: `visitDeclaration` (`evaluate.dart:1372`). Rejects
/// declarations outside style rules (modulo unknown at-rules/keyframes) and
/// nested raw-CSS/`--` declarations; prefixes names with the enclosing
/// `declaration_name`. Blank values are dropped except empty lists (kept so
/// CSS conversion throws visibly) and custom properties (allowed empty).
/// Children evaluate in a scope gated on `has_declarations` with the new name
/// as `declaration_name` (save/restore).
pub(crate) async fn evaluate_declaration<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &Declaration<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let in_style_rule = (!state.at_root_excluding_style_rule
        && state.style_rule_ignoring_at_root.is_some())
        || state.in_unknown_at_rule
        || state.in_keyframes;
    if !in_style_rule {
        return Err(Box::new(exception(
            state,
            "Declarations may only be used within style rules.".into(),
            Some(node.span),
        )));
    }

    if state.declaration_name.is_some() && !node.parsed_as_sass_script {
        let starts_with_dash = node.name.as_plain().is_some_and(|s| s.starts_with("--"));
        let msg = if starts_with_dash {
            "Declarations whose names begin with \"--\" may not be nested."
        } else {
            "Declarations parsed as raw CSS may not be nested."
        };
        return Err(Box::new(exception(state, msg.into(), Some(node.span))));
    }

    let name_text = perform_interpolation(config, state, arena, &node.name, true).await?;
    let name = CssValue::new(name_text, node.span);

    let full_name = if let Some(ref prefix) = state.declaration_name {
        CssValue::new(format!("{prefix}-{}", name.value), node.span)
    } else {
        name
    };

    let decl_name_text = full_name.value.clone();

    if let Some(ref value_expr) = node.value {
        let val = evaluate_expression(config, state, arena, value_expr).await?;
        let is_blank = val.is_blank();
        let is_custom_prop = decl_name_text.starts_with("--");
        let is_empty_list = matches!(&*val, ValueKind::List(l) if l.contents.is_empty());

        if !is_blank || is_empty_list || is_custom_prop {
            copy_parent_after_sibling(arena, state)?;
            let value_span = value_expr.span()?;
            // Dart visitDeclaration: `valueSpanForMap: _sourceMap ?
            // node.value.andThen(_expressionNode)?.span : null` — for a plain
            // variable reference this resolves to the variable's declaration
            // node span (so the value mapping points at the definition, not the
            // use site). `expression_node` mirrors Dart's `_expressionNode`.
            let value_span_for_map = if config.source_map {
                Some(expression_node(config, state, value_expr)?)
            } else {
                None
            };
            let decl = ModifiableCssDeclaration::new(
                full_name,
                CssValue::new(val, value_span),
                node.span,
                node.parsed_as_sass_script,
                value_span_for_map,
            )?;
            let decl_node = ModifiableCssNode::new(arena, ModifiableCssNodeKind::Declaration(decl));
            add_child(arena, state, &decl_node, None)?;
        }
    }

    if let Some(ref children) = node.children {
        if !children.is_empty() {
            let old_declaration_name = state.declaration_name.clone();
            state.declaration_name = Some(decl_name_text);

            let env = state.env;
            env.scope(
                arena,
                async || {
                    for child in children {
                        evaluate_statement(config, state, arena, child).await?;
                    }
                    Ok(())
                },
                false,
                has_declarations(children),
            )
            .await?;

            state.declaration_name = old_declaration_name;
        }
    }

    Ok(None)
}

// ===========================================================================
// VisitMixinRule — registers a user-defined mixin
// Go: evaluate_statement.go:1442
// ===========================================================================

/// Registers a user-defined mixin in the current environment.
///
/// Matches Dart: `visitMixinRule` (`evaluate.dart:2227`). Captures the
/// environment closure plus the `in_dependency` flag; sync (no evaluation).
pub(crate) fn evaluate_mixin_rule<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &MixinRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let callable = UserDefinedCallable::new(
        CallableDeclaration::Mixin(node.clone()),
        state.env.closure(arena),
        state.in_dependency,
    );
    let c = Callable::new(arena, CallableKind::UserDefined(callable));
    state.env.set_mixin(c);
    Ok(None)
}

// ===========================================================================
// VisitFunctionRule — registers a user-defined function
// Go: evaluate_statement.go:1767
// ===========================================================================

/// Registers a user-defined function in the current environment.
///
/// Matches Dart: `visitFunctionRule` (`evaluate.dart:1815`). Same shape as
/// [`evaluate_mixin_rule`]: closure-capturing callable stored on the
/// environment; sync.
pub(crate) fn evaluate_function_rule<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &FunctionRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let callable = UserDefinedCallable::new(
        CallableDeclaration::Function(node.clone()),
        state.env.closure(arena),
        state.in_dependency,
    );
    let c = Callable::new(arena, CallableKind::UserDefined(callable));
    state.env.set_function(c);
    Ok(None)
}

// ===========================================================================
// VisitContentBlock — registers content block
// Go: evaluate_statement.go:2902
// ===========================================================================

/// Registers a `@include` content block as the current content callable.
///
/// Matches Dart: `visitContentBlock` (`evaluate.dart:1345`) — which throws
/// `UnsupportedError` ("evaluation handles `@include` and its content block
/// together") because content blocks never evaluate standalone. Here the
/// registration half lives with `@include` handling while invocation lives in
/// [`evaluate_content_rule`]; sync.
pub(crate) fn evaluate_content_block<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &ContentBlock<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let callable = UserDefinedCallable::new(
        CallableDeclaration::ContentBlock(node.clone()),
        state.env.closure(arena),
        state.in_dependency,
    );
    let c = Callable::new(arena, CallableKind::UserDefined(callable));
    state.env.set_content(Some(c));
    Ok(None)
}

// ===========================================================================
// VisitContentRule — invokes content block
// Go: evaluate_statement.go:2882
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Invokes the ambient content block for `@content`.
///
/// Matches Dart: `visitContentRule` (`evaluate.dart:1349`). No-op when no
/// content is active; otherwise runs the content callable's closure
/// environment through `run_user_defined_callable` without managing content
/// forwarding (so nested `@content` resolves to the outer block, not itself).
pub(crate) async fn evaluate_content_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &ContentRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    if let Some(content) = state.env.content() {
        if let CallableKind::UserDefined(ud) = content.kind() {
            // Go: evaluate_statement.go:2891 — calls runUserDefinedCallable
            // to activate the content callable's closure env. This ensures
            // @content inside the content block resolves to the outer
            // content (captured when the content callable was created),
            // not to itself (which would cause infinite recursion).
            run_user_defined_callable(
                config,
                state,
                arena,
                ud,
                &node.arguments,
                None,  // no further content forwarding
                false, // don't manage content — preserve closure env's chain
                node.span()?,
            )
            .await?;
        }
    }
    Ok(None)
}

// ===========================================================================
// VisitLoudComment
// Go: evaluate_statement.go:2908
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Evaluates a loud (`/* */`) comment: interpolates and appends it to the CSS.
///
/// Matches Dart: `visitLoudComment` (`evaluate.dart:2238`, mirrored in
/// `visitCssComment`). No-op inside functions; comments at the root import
/// boundary advance `end_of_imports`; indented-syntax text without a closing
/// `*/` gets one appended.
pub(crate) async fn evaluate_loud_comment<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &LoudComment<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    if state.in_function {
        return Ok(None);
    }
    let text = perform_interpolation(config, state, arena, &node.text, false).await?;
    let text = if !text.ends_with("*/") {
        format!("{} */", text)
    } else {
        text
    };
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
    copy_parent_after_sibling(arena, state)?;
    let comment = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::Comment(ModifiableCssComment::new(text, node.span()?)),
    );
    add_child(arena, state, &comment, None)?;
    Ok(None)
}

// ===========================================================================
// VisitExtendRule — adds extension to store
// Go: evaluate_statement.go:2775
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Evaluates `@extend`: parses the target and records the extension.
///
/// Matches Dart: `visitExtendRule` (`evaluate.dart:1483`). Requires an
/// enclosing style rule and no nested declaration name; warns on bogus
/// extender combinators, rejects complex targets and multi-simple compounds
/// with format errors, and adds the extension with the current media context
/// (the cross-media check throws eagerly, trace-wrapped so mixin/`@content`
/// frames survive).
pub(crate) async fn evaluate_extend_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &ExtendRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    if style_rule(state).is_none() || state.declaration_name.is_some() {
        return Err(Box::new(exception(
            state,
            "@extend may only be used within style rules.".into(),
            Some(node.span),
        )));
    }

    // ── Bogus combinator check (matching Go VisitExtendRule lines 2785–2815) ──
    let bogus_check_components = state
        .style_rule_ignoring_at_root
        .as_ref()
        .unwrap()
        .original_selector
        .0
        .components
        .clone();
    for complex in &bogus_check_components {
        if !complex.is_bogus() {
            continue;
        }
        let trimmed = complex.to_css_string(true)?.trim().to_string();
        let comp_span = complex.span()?;
        let trimmed_span = comp_span.trim_right().map_err(|e| match e {
            SpanError::Argument(msg) => Box::new(SassError::Script {
                message: msg,
                argument_name: None,
            }),
            SpanError::Range(msg) => Box::new(SassError::Script {
                message: msg,
                argument_name: None,
            }),
            SpanError::Sass(s) => s,
        })?;
        let msg = format!(
            "The selector \"{}\" is invalid CSS and {} be an extender.\n\
             This will be an error in Dart Sass 2.0.0.\n\n\
             More info: https://sass-lang.com/d/bogus-combinators",
            trimmed,
            if complex.is_useless() {
                "can't"
            } else {
                "shouldn't"
            },
        );
        warn_deprecation_multi_span(
            config,
            state,
            &msg,
            trimmed_span,
            "invalid selector",
            vec![(node.span, "@extend rule".to_string())],
            &deprecation::BOGUS_COMBINATORS,
        )?;
    }

    let (target_text, target_map) =
        perform_interpolation_with_map(config, state, arena, &node.selector, true).await?;
    let trimmed = trim_ascii(&target_text, false);

    let source = FileSource::new_in(arena, &trimmed, None);
    let mut parser =
        SelectorParser::new_with_options(arena, source, false, false, None, None, Some(target_map));
    let list = parser.parse()?;

    let style_rule = state.style_rule_ignoring_at_root.as_ref().unwrap();
    let style_selector = style_rule.selector;

    for complex in &list.0.components {
        let compound = match complex.single_compound() {
            Some(c) => c,
            None => {
                let complex_span = complex.span()?;
                return Err(Box::new(exception(
                    state,
                    "complex selectors may not be extended.".into(),
                    Some(complex_span),
                )));
            }
        };

        let simple = match compound.single_simple() {
            Some(s) => s,
            None => {
                let parts: Vec<String> =
                    compound.components.iter().map(|s| format!("{s}")).collect();
                let compound_span = compound.span()?;
                return Err(Box::new(exception(
                    state,
                    format!(
                        "compound selectors may no longer be extended.\n\
                         Consider `@extend {}` instead.\n\
                         See https://sass-lang.com/d/extend-compound for details.\n",
                        parts.join(", ")
                    ),
                    Some(compound_span),
                )));
            }
        };

        // Dart's `_extendExistingSelectors`/`_extendSimple` run the media-context
        // check eagerly during `addExtension` (extension_store.dart:705,841), so
        // "You may not @extend selectors across media queries." is thrown while
        // the evaluation stack is live. Wrap in `add_exception_trace` so the
        // `SassError::Sass` gets the current stack (@content/mixin frames),
        // mirroring Dart's top-level `_addExceptionTrace` with the leaked stack.
        add_exception_trace(state, async |state| {
            let store = match state.extension_store.as_mut() {
                Some(s) => s,
                None => {
                    return Err(Box::new(exception(
                        state,
                        "No extension store available.".into(),
                        Some(node.span),
                    )))
                }
            };
            store.add_extension(
                arena,
                &style_selector,
                simple,
                node,
                state.media_queries.clone(),
            )
        })
        .await?;
    }

    Ok(None)
}

// ===========================================================================
// VisitAtRule — unknown @-rule evaluation
// Go: evaluate_statement.go:2658
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Evaluates an unknown `@rule`: interpolates name/value and evaluates
/// children with style-rule bubbling.
///
/// Matches Dart: `visitAtRule` (`evaluate.dart:1552`, mirrored in
/// `visitCssAtRule`). Rejects nested-declaration context; childless rules
/// attach directly. `keyframes` (after unvendoring) sets `in_keyframes`,
/// anything else sets `in_unknown_at_rule` (both restored after). Inside a
/// style rule the rule is copied inward so bare declarations have a home —
/// except keyframes/`font-face`/toplevel children, which evaluate flat;
/// bubbling passes through style rules only. Children scope on
/// `has_declarations`; plain-CSS nesting short-circuits to a plain scoped
/// evaluation.
pub(crate) async fn evaluate_at_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &AtRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    // Dart `visitAtRule` rejects at-rules inside nested declarations here
    // (the parser only allows declaration-context at-rules such as `@include`
    // through, e.g. inside `@include` content blocks).
    if state.declaration_name.is_some() {
        return Err(Box::new(exception(
            state,
            "At-rules may not be used within nested declarations.".into(),
            Some(node.span),
        )));
    }

    // Evaluate the name interpolation
    let name_text = perform_interpolation(config, state, arena, &node.name, false).await?;
    let name = CssValue::new(name_text, node.name.span()?);

    // Evaluate value interpolation if present
    let value = if let Some(ref v) = node.value {
        let text = perform_interpolation(config, state, arena, v, true).await?;
        let trimmed = trim_ascii(&text, true);
        Some(CssValue::new(trimmed, v.span()?))
    } else {
        None
    };

    let childless = node.children.is_none();

    if childless {
        copy_parent_after_sibling(arena, state)?;
        let at_rule = ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::AtRule(ModifiableCssAtRule::new(name, node.span, true, value)),
        );
        add_child(arena, state, &at_rule, None)?;
        return Ok(None);
    }

    let old_in_unknown = state.in_unknown_at_rule;
    let old_in_keyframes = state.in_keyframes;
    let name_value = name.value.clone();
    if unvendor(&name_value) == "keyframes" {
        state.in_keyframes = true;
    } else {
        state.in_unknown_at_rule = true;
    }

    let at_rule = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::AtRule(ModifiableCssAtRule::new(name, node.span, false, value)),
    );

    if has_css_nesting(state) {
        let tabs_node = at_rule.clone();
        with_parent(
            arena,
            config,
            state,
            at_rule,
            None,
            Some(has_declarations(
                node.children.as_ref().map(|v| &v[..]).unwrap_or(&[]),
            )),
            async |c, s| {
                // Opaque NESTED-tabs scope: content nested in unknown
                // at-rules (including `@keyframes`) collects no outer
                // stamps (libsass debubbles at-rule content opaquely).
                let tabs_start = tabs_opaque_push(s);
                if let Some(ref children) = node.children {
                    for child in children {
                        evaluate_statement(c, s, arena, child).await?;
                    }
                }
                tabs_opaque_pop(s, &tabs_node, tabs_start);
                Ok(())
            },
        )
        .await?;
        state.in_unknown_at_rule = old_in_unknown;
        state.in_keyframes = old_in_keyframes;
        return Ok(None);
    }

    let through_fn =
        |n: &ModifiableCssNode| matches!(&*n.kind(), ModifiableCssNodeKind::StyleRule(_));
    let through: Option<&dyn Fn(&ModifiableCssNode) -> bool> = Some(&through_fn);

    let tabs_node = at_rule.clone();
    with_parent(
        arena,
        config,
        state,
        at_rule,
        through,
        Some(has_declarations(
            node.children.as_ref().map(|v| &v[..]).unwrap_or(&[]),
        )),
        async |config, state| {
            // Opaque NESTED-tabs scope (see the plain-CSS path above).
            let tabs_start = tabs_opaque_push(state);
            let no_sr = style_rule(state).is_none();
            if no_sr || state.in_keyframes || name_value == "font-face" {
                if let Some(ref children) = node.children {
                    for child in children {
                        evaluate_statement(config, state, arena, child).await?;
                    }
                }
            } else if let Some(ref sr_node) = state.css_style_rule_node {
                let sr_copy = sr_node.copy_without_children(arena);
                with_parent(
                    arena,
                    config,
                    state,
                    sr_copy,
                    None,
                    Some(false),
                    async |c, s| {
                        if let Some(ref children) = node.children {
                            for child in children {
                                evaluate_statement(c, s, arena, child).await?;
                            }
                        }
                        Ok(())
                    },
                )
                .await?;
            } else if let Some(ref children) = node.children {
                for child in children {
                    evaluate_statement(config, state, arena, child).await?;
                }
            }
            tabs_opaque_pop(state, &tabs_node, tabs_start);
            Ok(())
        },
    )
    .await?;

    state.in_unknown_at_rule = old_in_unknown;
    state.in_keyframes = old_in_keyframes;
    Ok(None)
}

// ===========================================================================
// VisitAtRootRule — @at-root scoping
// Go: evaluate_statement.go:2453
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Evaluates `@at-root`: re-parents children outside the excluded ancestors.
///
/// Matches Dart: `visitAtRootRule` + `_scopeForAtRoot`/`_trimIncluded`
/// (`evaluate.dart:1186-1343`). The query parses from interpolation with the
/// map (color warnings on); ancestors not excluded collect innermost-first,
/// then `trim_included` drops a trailing run already rooted at `root` so a
/// no-exclusion rule just scopes in place. Otherwise copies of the included
/// ancestors nest inside `root` and six state fields (`parent`,
/// `style_rule_ignoring_at_root`, `at_root_excluding_style_rule`,
/// `media_queries` + sources, `in_keyframes`, `in_unknown_at_rule`) save,
/// adjust per the query (media exclusion clears queries *and* sources; the
/// unknown-at-rule flag uses the post-trim list), and restore. Children scope
/// on `has_declarations`.
pub(crate) async fn evaluate_at_root_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &AtRootRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    // Parse query (Dart `visitAtRootRule`: `_performInterpolationWithMap(
    // unparsedQuery, warnForColor: true)` + `AtRootQuery.parse(resolved,
    // interpolationMap: map)`).
    let query = if let Some(ref q_interp) = node.query {
        let (resolved, map) =
            perform_interpolation_with_map(config, state, arena, q_interp, true).await?;
        AtRootQuery::parse(arena, &resolved, Some(map))
    } else {
        AtRootQuery::default_query()
    };

    // Walk parent chain, collecting CSS parent nodes not excluded by query
    let mut cur = state.parent.clone();
    let mut included: Vec<ModifiableCssNode<'parse>> = Vec::new();
    loop {
        let parent = match cur {
            Some(ref p) => p.clone(),
            None => break,
        };
        let kind = parent.kind();
        if matches!(&*kind, ModifiableCssNodeKind::Stylesheet(_)) {
            break;
        }
        let excluded = if query.is_all() {
            !query.include
        } else {
            match &*kind {
                ModifiableCssNodeKind::StyleRule(_) => query.excludes_style_rules(),
                ModifiableCssNodeKind::MediaRule(_) => query.excludes_name("media"),
                ModifiableCssNodeKind::SupportsRule(_) => query.excludes_name("supports"),
                ModifiableCssNodeKind::AtRule(r) => {
                    query.excludes_name(&r.name.value.to_lowercase())
                }
                _ => false,
            }
        };
        drop(kind);
        if !excluded {
            included.push(parent.clone());
        }
        cur = parent.parent();
    }

    let root = trim_included(state, &mut included);

    // If no exclusion, just evaluate in scope
    if root == state.parent.clone().unwrap() {
        let env = state.env;
        return env
            .scope(
                arena,
                async || {
                    for child in &node.children {
                        evaluate_statement(config, state, arena, child).await?;
                    }
                    Ok(None)
                },
                false,
                has_declarations(&node.children),
            )
            .await;
    }

    // Dart `_scopeForAtRoot` checks `included` for at-rules *after*
    // `_trimIncluded` mutates it, so compute the presence here (post-trim),
    // before `drain(..)` below empties it.
    let included_has_at_rule = included
        .iter()
        .any(|p| matches!(&*p.kind(), ModifiableCssNodeKind::AtRule(_)));

    // Build nested copy chain of excluded ancestors
    let inner_copy = if !included.is_empty() {
        let mut it = included.drain(..);
        let first = it.next().unwrap();
        let inner = first.copy_without_children(arena);
        let mut outer = inner.clone();
        for node in it {
            let copy = node.copy_without_children(arena);
            copy.add_child(&outer)?;
            outer = copy;
        }
        root.add_child(&outer)?;
        inner
    } else {
        root.clone()
    };

    // Save/restore flags based on query
    let old_parent = state.parent.clone();
    let old_sr = state.style_rule_ignoring_at_root.clone();
    let old_at_root = state.at_root_excluding_style_rule;
    let old_mq = state.media_queries.clone();
    let old_sources = state.media_query_sources.clone();
    let old_kf = state.in_keyframes;
    let old_unk = state.in_unknown_at_rule;

    state.parent = Some(inner_copy);
    if query.excludes_style_rules() {
        state.at_root_excluding_style_rule = true;
    }
    // Dart `_scopeForAtRoot`: `_withMediaQueries(null, null, ...)` clears
    // queries *and* sources.
    if state.media_queries.is_some() && query.excludes_name("media") {
        state.media_queries = None;
        state.media_query_sources = None;
    }
    if state.in_keyframes && query.excludes_name("keyframes") {
        state.in_keyframes = false;
    }
    if state.in_unknown_at_rule && !included_has_at_rule {
        state.in_unknown_at_rule = false;
    }

    let env = state.env;
    let result = env
        .scope(
            arena,
            async || {
                for child in &node.children {
                    evaluate_statement(config, state, arena, child).await?;
                }
                Ok(None)
            },
            false,
            has_declarations(&node.children),
        )
        .await;

    state.parent = old_parent;
    state.style_rule_ignoring_at_root = old_sr;
    state.at_root_excluding_style_rule = old_at_root;
    state.media_queries = old_mq;
    state.media_query_sources = old_sources;
    state.in_keyframes = old_kf;
    state.in_unknown_at_rule = old_unk;

    result
}

/// Destructively trims a trailing ancestor run from `included`.
///
/// Matches Dart: `_trimIncluded` (`evaluate.dart:1254`). `included` runs
/// innermost-first; when a trailing sublist is contiguous (each node the
/// direct parent of the previous) and rooted as a direct child of `root`,
/// removes it and returns its innermost node — otherwise leaves `included`
/// intact and returns `root`. Dart throws on a non-ancestor chain; here the
/// chain is built by walking real parents, so the error cases are
/// unreachable.
fn trim_included<'parse>(
    state: &EvalState<'_, 'parse>,
    included: &mut Vec<ModifiableCssNode<'parse>>,
) -> ModifiableCssNode<'parse> {
    if included.is_empty() {
        return state.root.clone().unwrap();
    }

    let mut cur = state.parent.clone();
    let mut innermost_contiguous: Option<usize> = None;

    for (i, node) in included.iter().enumerate() {
        // Walk up parent chain until cur matches node (identity via Rc::ptr_eq)
        loop {
            if cur.as_ref() == Some(node) {
                break;
            }
            innermost_contiguous = None;
            cur = cur
                .as_ref()
                .and_then(|c| c.parent())
                .or_else(|| panic!("Expected node to be an ancestor of parent"));
        }
        if innermost_contiguous.is_none() {
            innermost_contiguous = Some(i);
        }
        cur = cur
            .as_ref()
            .and_then(|c| c.parent())
            .or_else(|| panic!("Expected node to be an ancestor of parent"));
    }

    if cur != state.root {
        return state.root.clone().unwrap();
    }
    let index = innermost_contiguous.unwrap();
    let root = included[index].clone();
    included.truncate(index);
    root
}

// ===========================================================================
// VisitSilentComment — no-op
// Go: evaluate_statement.go:2945
// ===========================================================================

/// Silent comments vanish: no CSS, no evaluation.
///
/// Matches Dart: `visitSilentComment` (`evaluate.dart:2371`, `=> null`).
pub(crate) fn evaluate_silent_comment<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    _node: &SilentComment<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    Ok(None)
}

// ===========================================================================
// registerCommentsForModule — adds root.children comments to pre_module_comments
// Go: evaluate_statement.go:1090
// Dart: evaluate.dart:1760
// ===========================================================================

/// Stashes root-level comments for a freshly loaded module.
///
/// Matches Dart: `_registerCommentsForModule` (`evaluate.dart:1762`). No-op
/// outside a module root or when the module carries no CSS; otherwise moves
/// the current root children into `pre_module_comments[module]` (so leading
/// comments travel with the module through [`combine_css`]) and resets the
/// import-boundary counters.
fn register_comments_for_module<'compile, 'parse>(
    state: &mut EvalState<'compile, 'parse>,
    module: &Module<'compile, 'parse>,
) -> SassResult<()>
where
    'compile: 'parse,
{
    let root = match state.root.as_ref() {
        Some(r) => r,
        None => return Ok(()),
    };
    let children = match root.children() {
        Some(c) => c,
        None => return Ok(()),
    };
    if children.is_empty() || !module.transitively_contains_css() {
        return Ok(());
    }
    let comments: Vec<CssComment<'parse>> = children
        .iter()
        .filter_map(|c| {
            let kind = c.kind();
            match &*kind {
                ModifiableCssNodeKind::Comment(comment) => {
                    Some(CssComment::new(comment.text.clone(), comment.span))
                }
                _ => None,
            }
        })
        .collect();
    let pre = state.pre_module_comments.get_or_insert_with(IndexMap::new);
    pre.entry(*module).or_default().extend(comments);
    root.clear_children()?;
    state.end_of_imports = 0;
    state.out_of_order_imports.clear();
    Ok(())
}

// ===========================================================================
// evaluateModule — executes a stylesheet as a module
// Go: evaluate_statement.go:108
// Dart: evaluate.dart:889
// ===========================================================================

/// Collects the configured-variable names of `config`.
///
/// Helper for the already-loaded check in [`evaluate_module`]: a module that
/// could have been configured with these keys cannot be reconfigured with a
/// different original configuration.
fn config_keys(config: &Configuration<'_>) -> HashSet<String> {
    config.keys().into_iter().collect()
}

// ===========================================================================
// loadModule — shared module loading orchestration
// Go: evaluate_statement.go:656
// Dart: evaluate.dart:802
// ===========================================================================

/// Shared module loading orchestration used by `@use`, `@forward`, and
/// `load-css()`. Matches Go's `loadModule` (evaluate_statement.go:656-752)
/// and Dart's `_loadModule` (evaluate.dart:802-878).
///
/// Returns `(module, first_load)` where `first_load` is `true` if the module
/// was not previously cached in `state.modules`.
///
/// The caller must then:
/// - For `@use`/`@forward`: call `register_comments_for_module` +
///   `env.add_module`/`env.forward_module`.
/// - For `load-css`: call `combine_css` + `evaluate_css_stylesheet`.
// Arity mirrors Go/Dart `loadModule`; packing into a struct would diverge
// from the port.
#[allow(clippy::too_many_arguments)]
#[rust_sass_macros::maybe_async]
pub(crate) async fn load_module<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    url: &SassUrl,
    stack_frame: &str,
    node_with_span: FileSpan<'parse>,
    module_configuration: Option<&Configuration<'parse>>,
    names_in_errors: bool,
    base_url: Option<&SassUrl>,
) -> SassResult<(Module<'compile, 'parse>, bool)>
where
    'compile: 'parse,
{
    // ── Step 1: Built-in module check (Go 657-678, Dart 811-828) ──
    if let Some(built_in) = config.built_in_modules.get(&url.to_string()) {
        if let Some(cfg) = module_configuration {
            if cfg.is_explicit() {
                let message = if names_in_errors {
                    format!("Built-in module {} can't be configured.", url)
                } else {
                    "Built-in modules can't be configured.".into()
                };
                return Err(Box::new(exception(state, message, cfg.node_span())));
            }
        }
        return Ok((*built_in, false));
    }

    // ── Steps 2-7: Load, detect loops, evaluate, cleanup (Go 680-747, Dart 830-877) ──
    with_stack_frame(
        config,
        state,
        arena,
        stack_frame,
        node_with_span,
        None,
        async |config, state| -> SassResult<(Module<'compile, 'parse>, bool)> {
            // Step 2: Load stylesheet (Go 686, Dart 831-834). Wrapped in
            // `add_exception_trace` (so load-time parse errors get a trace that
            // includes this stack frame) and `add_exception_span` (so span-less
            // `Script` errors get a Runtime error at the load site).
            let loaded =
                add_exception_span(config, state, node_with_span, Some(false), async |c, s| {
                    add_exception_trace(s, async |s2| {
                        load_stylesheet(c, s2, arena, url, node_with_span, base_url, false).await
                    })
                    .await
                })
                .await?;

            // Step 3: Circular import detection via active_modules
            // (Go 691-731, Dart 837-853)
            let sheet_url = loaded.stylesheet.span.source_url().map(|u| u.to_string());

            if let Some(ref key) = sheet_url {
                if !key.is_empty() {
                    if let Some(prev) = state.active_modules.get(key).copied().flatten() {
                        let message = if names_in_errors {
                            let pretty = SassUrl::parse(key)
                                .map(|u| pretty_uri(&u, config.io.as_ref()))
                                .unwrap_or_else(|_| key.clone());
                            format!("Module loop: {pretty} is already being loaded.")
                        } else {
                            "Module loop: this module is already being loaded.".into()
                        };
                        return Err(Box::new(SassError::MultiSpan {
                            message,
                            span: SourceSpanWithContext::from_file_span(&node_with_span)?,
                            primary_label: Some("new load".into()),
                            secondary: vec![(
                                SourceSpanWithContext::from_file_span(&prev)?,
                                "original load".into(),
                            )],
                            original_source: None,
                            cause: None,
                            loaded_urls: vec![],
                            trace: stack_trace(state, None),
                        }));
                    }
                    if state.active_modules.contains_key(key) {
                        let message = if names_in_errors {
                            let pretty = SassUrl::parse(key)
                                .map(|u| pretty_uri(&u, config.io.as_ref()))
                                .unwrap_or_else(|_| key.clone());
                            format!("Module loop: {pretty} is already being loaded.")
                        } else {
                            "Module loop: this module is already being loaded.".into()
                        };
                        let span_ctx = SourceSpanWithContext::from_file_span(&node_with_span)?;
                        return Err(Box::new(SassError::Runtime {
                            message,
                            span: span_ctx,
                            trace: stack_trace(state, None),
                            cause: None,
                            loaded_urls: vec![],
                        }));
                    }
                    state
                        .active_modules
                        .insert(key.clone(), Some(node_with_span));
                }
            }

            // Step 4: first_load + in_dependency (Go 732-734, Dart 856-858)
            let first_load = sheet_url
                .as_ref()
                .is_none_or(|key| key.is_empty() || !state.modules.contains_key(key));
            let old_in_dep = state.in_dependency;
            state.in_dependency = loaded.is_dependency;

            // Step 5: Evaluate module (Go 735, Dart 861-867)
            let module = evaluate_module(
                config,
                state,
                arena,
                loaded.stylesheet,
                module_configuration,
                &loaded.importer,
                node_with_span,
                names_in_errors,
            )
            .await;

            // Step 6: Cleanup active_modules + in_dependency
            // (Go 736-739, Dart 868-871)
            if let Some(ref key) = sheet_url {
                if !key.is_empty() {
                    state.active_modules.shift_remove(key);
                }
            }
            state.in_dependency = old_in_dep;

            let module = module?;

            Ok((module, first_load))
        },
    )
    .await
}

/// Evaluates `stylesheet` in a fresh module context and caches the result.
///
/// Matches Dart: `_execute` (`evaluate.dart:889`). Short-circuits already
/// loaded URLs (erroring when an explicit `with` configuration conflicts with
/// the original load); otherwise swaps in a fresh environment, importer,
/// stylesheet, CSS root, extension store, and scope flags (16 fields),
/// evaluates, builds the module from the resulting CSS plus pre-module
/// comments, caches it with its configuration and load span, and restores
/// everything. The caller's ambient configuration applies when
/// `module_configuration` is `None`.
///
/// Why save/restore instead of a combinator: the field list is fixed and the
/// function needs the built module after evaluation, so explicit old-value
/// locals keep the restore order visible at the single exit point.
// Arity mirrors Dart's `_execute`; packing into a struct would diverge from
// the port.
#[allow(clippy::too_many_arguments)]
#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_module<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    stylesheet: &'parse Stylesheet<'parse>,
    module_configuration: Option<&Configuration<'parse>>,
    importer: &Importer<'parse>,
    node_with_span: FileSpan<'parse>,
    names_in_errors: bool,
) -> SassResult<Module<'compile, 'parse>>
where
    'compile: 'parse,
{
    let sheet_url = stylesheet.span.source_url().map(|u| u.to_string());
    let current_config = module_configuration
        .cloned()
        .unwrap_or_else(|| state.configuration.clone());

    // Already-loaded check
    if let Some(ref url) = sheet_url {
        if let Some(already_loaded) = state.modules.get(url) {
            let prev_config = state.module_configurations.get(url);
            if let Some(prev_config) = prev_config {
                if !prev_config.same_original(&current_config)
                    && current_config.is_explicit()
                    && already_loaded.could_have_been_configured(&config_keys(&current_config))
                {
                    let message = if names_in_errors {
                        let pretty = SassUrl::parse(url)
                            .map(|u| pretty_uri(&u, config.io.as_ref()))
                            .unwrap_or_else(|_| url.clone());
                        format!(
                            "{pretty} was already loaded, so it can't be configured using \"with\"."
                        )
                    } else {
                        "This module was already loaded, so it can't be configured using \"with\"."
                            .into()
                    };
                    let mut secondary = Vec::new();
                    if let Some(prev_span) = state.module_nodes.get(url).copied().flatten() {
                        secondary.push((
                            SourceSpanWithContext::from_file_span(&prev_span)?,
                            "original load".into(),
                        ));
                    }
                    if module_configuration.is_none() {
                        if let Some(conf_span) = current_config.node_span() {
                            secondary.push((
                                SourceSpanWithContext::from_file_span(&conf_span)?,
                                "configuration".into(),
                            ));
                        }
                    }
                    if !secondary.is_empty() {
                        return Err(Box::new(SassError::MultiSpan {
                            message,
                            span: SourceSpanWithContext::from_file_span(&node_with_span)?,
                            primary_label: Some("new load".into()),
                            secondary,
                            original_source: None,
                            cause: None,
                            loaded_urls: vec![],
                            trace: stack_trace(state, None),
                        }));
                    }
                    return Err(Box::new(exception(state, message, Some(node_with_span))));
                }
            }
            return Ok(*already_loaded);
        }
    }

    // Save environment + configuration
    let old_env = state.env;
    let old_config = state.configuration.clone();
    state.env = Environment::new(arena);
    if let Some(cfg) = module_configuration {
        state.configuration = cfg.clone();
    }

    // Save 16 fields
    let old_importer = state.importer;
    let old_source_url = state.source_url.clone();
    let old_stylesheet = state.stylesheet;
    let old_root = state.root.clone();
    let old_parent = state.parent.clone();
    let old_eoi = state.end_of_imports;
    let old_ooi = std::mem::take(&mut state.out_of_order_imports);
    let old_ext = state.extension_store.clone();
    let old_pre_mod = state.pre_module_comments.take();
    let old_sr = state.style_rule_ignoring_at_root.clone();
    let old_mq = state.media_queries.clone();
    let old_decl = state.declaration_name.clone();
    let old_unk = state.in_unknown_at_rule;
    let old_atr = state.at_root_excluding_style_rule;
    let old_kf = state.in_keyframes;

    // Set fresh context
    state.importer = *importer;
    state.stylesheet = Some(stylesheet);
    let new_root = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::Stylesheet(ModifiableCssStylesheet::new(stylesheet.span)),
    );
    state.root = Some(new_root.clone());
    state.parent = Some(new_root);
    state.end_of_imports = 0;
    state.out_of_order_imports.clear();
    state.extension_store = Some(DefaultExtensionStore::new(
        ExtendMode::Normal,
        config.io.clone(),
        config.unicode,
    ));
    state.pre_module_comments = None;
    state.style_rule_ignoring_at_root = None;
    state.media_queries = None;
    state.declaration_name = None;
    state.in_unknown_at_rule = false;
    state.at_root_excluding_style_rule = false;
    state.in_keyframes = false;
    if let Some(ref url) = sheet_url {
        state.source_url = Some(SassUrl::parse(url).map_err(|e| SassError::Script {
            message: e.to_string(),
            argument_name: None,
        })?);
    }

    // Visit the stylesheet
    evaluate_stylesheet(config, state, arena, stylesheet).await?;

    // Build CSS output
    let root = state.root.as_ref().unwrap();
    let css_children = if !state.out_of_order_imports.is_empty() {
        build_out_of_order_children(arena, state)
    } else {
        root.children().unwrap_or_default()
    };
    let css = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::Stylesheet(ModifiableCssStylesheet::new(stylesheet.span)),
    );
    for child in &css_children {
        css.add_child(child)?;
    }

    // Build module
    let ext_store = state
        .extension_store
        .as_ref()
        .map(|es| ExtensionStore::Default(Rc::new(RefCell::new(es.clone()))))
        .unwrap_or(ExtensionStore::Empty);
    let pre_mod_comments = state.pre_module_comments.take().unwrap_or_default();
    let module = state
        .env
        .to_module(arena, &css, &pre_mod_comments, ext_store);

    // Cache module
    if let Some(ref url) = sheet_url {
        state.modules.insert(url.clone(), module);
        state
            .module_configurations
            .insert(url.clone(), current_config);
        state.module_nodes.insert(url.clone(), Some(node_with_span));
    }

    // Restore 16 fields
    state.env = old_env;
    state.configuration = old_config;
    state.importer = old_importer;
    state.source_url = old_source_url;
    state.stylesheet = old_stylesheet;
    state.root = old_root;
    state.parent = old_parent;
    state.end_of_imports = old_eoi;
    state.out_of_order_imports = old_ooi;
    state.extension_store = old_ext;
    state.pre_module_comments = old_pre_mod;
    state.style_rule_ignoring_at_root = old_sr;
    state.media_queries = old_mq;
    state.declaration_name = old_decl;
    state.in_unknown_at_rule = old_unk;
    state.at_root_excluding_style_rule = old_atr;
    state.in_keyframes = old_kf;

    Ok(module)
}

// ===========================================================================
// VisitUseRule — @use directive
// Go: evaluate_statement.go:754
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Evaluates `@use`: loads the module (with configuration) and namespaces it.
///
/// Matches Dart: `visitUseRule` (`evaluate.dart:2710`). Builds an explicit
/// configuration from the `with` clause (slash-stripped values), short-circuits
/// built-ins (which cannot be configured) via exception-span-wrapped
/// `add_module`, and otherwise loads under a `@use` stack frame with loop
/// detection, `is_dependency` tracking, and trace/span wrapping so load and
/// eval errors point at the rule. Ends with
/// [`assert_configuration_is_empty`].
pub(crate) async fn evaluate_use_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &UseRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let configuration = if !node.configuration.is_empty() {
        let mut values: IndexMap<String, ConfiguredValue<'parse>> = IndexMap::new();
        for cv in &node.configuration {
            let val = evaluate_expression(config, state, arena, &cv.expression).await?;
            let val = without_slash(config, state, arena, val, cv.expression.span()?)?;
            let assignment_node = expression_node(config, state, &cv.expression)?;
            values.insert(
                cv.name.clone(),
                ConfiguredValue::explicit(val, cv.span, assignment_node),
            );
        }
        Configuration::new_explicit(arena, values, node.span)
    } else {
        Configuration::empty(arena)
    };

    // Built-in module check
    if let Some(built_in) = config.built_in_modules.get(node.url.as_str()) {
        if configuration.is_explicit() {
            return Err(Box::new(exception(
                state,
                "Built-in modules can't be configured.".into(),
                configuration.node_span(),
            )));
        }
        register_comments_for_module(state, built_in)?;
        add_exception_span(config, state, node.span, None, async |_, s| {
            s.env
                .add_module(*built_in, node.span, node.namespace.as_deref())
        })
        .await?;
        assert_configuration_is_empty(state, &configuration)?;
        return Ok(None);
    }

    // User-defined module: load + evaluate
    with_stack_frame(
        config,
        state,
        arena,
        "@use",
        node.span,
        None,
        async |c, s| -> SassResult<Option<Value<'parse>>> {
            // Dart: `_loadModule` wraps `_loadStylesheet` in `_addExceptionTrace`
            // while the `@use` stack frame is active, so load-time errors get a
            // trace that includes this frame. `add_exception_span` converts
            // span-less `Script` errors (e.g. ambiguous imports) to a Runtime
            // error at the `@use` rule with a stack-frame-only trace.
            let loaded = add_exception_span(c, s, node.span, Some(false), async |c2, s2| {
                add_exception_trace(s2, async |s3| {
                    load_stylesheet(c2, s3, arena, &node.url, node.span, None, false).await
                })
                .await
            })
            .await?;

            let sheet_url = loaded.stylesheet.span.source_url().map(|u| u.to_string());

            // Active modules: circular import detection
            if let Some(ref key) = sheet_url {
                if !key.is_empty() {
                    if let Some(prev) = s.active_modules.get(key).copied().flatten() {
                        return Err(Box::new(SassError::MultiSpan {
                            message: "Module loop: this module is already being loaded.".into(),
                            span: SourceSpanWithContext::from_file_span(&node.span)?,
                            primary_label: Some("new load".into()),
                            secondary: vec![(
                                SourceSpanWithContext::from_file_span(&prev)?,
                                "original load".into(),
                            )],
                            original_source: None,
                            cause: None,
                            loaded_urls: vec![],
                            trace: stack_trace(s, None),
                        }));
                    }
                    if s.active_modules.contains_key(key) {
                        let span_ctx = SourceSpanWithContext::from_file_span(&node.span)?;
                        return Err(Box::new(SassError::Runtime {
                            message: "Module loop: this module is already being loaded.".into(),
                            span: span_ctx,
                            trace: stack_trace(s, None),
                            cause: None,
                            loaded_urls: vec![],
                        }));
                    }
                    s.active_modules.insert(key.clone(), Some(node.span));
                }
            }

            let is_dependency = s.in_dependency || loaded.importer != s.importer;
            let old_in_dep = s.in_dependency;
            s.in_dependency = is_dependency;

            let result: SassResult<Option<Value<'parse>>> = async {
                // Dart `_loadModule` evaluates the module inside `_addExceptionTrace`
                // while the `@use` frame is active, so module-eval errors get a
                // trace that includes this frame.
                let module = add_exception_trace(s, async |s2| {
                    evaluate_module(
                        c,
                        s2,
                        arena,
                        loaded.stylesheet,
                        Some(&configuration),
                        &loaded.importer,
                        node.span,
                        false,
                    )
                    .await
                })
                .await?;
                register_comments_for_module(s, &module)?;
                add_exception_span(c, s, node.span, Some(false), async |_, s2| {
                    s2.env
                        .add_module(module, node.span, node.namespace.as_deref())
                })
                .await?;
                Ok(None)
            }
            .await;

            // Cleanup active_modules regardless of success/failure (matching Go)
            if let Some(ref key) = sheet_url {
                if !key.is_empty() {
                    s.active_modules.shift_remove(key);
                }
            }
            s.in_dependency = old_in_dep;
            result
        },
    )
    .await?;

    assert_configuration_is_empty(state, &configuration)?;
    Ok(None)
}

// ===========================================================================
// add_forward_configuration — builds configuration for @forward with clause
// Dart: evaluate.dart:1728
// Go: evaluate_statement.go:891
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Folds a `@forward ... with (...)` clause into the ambient configuration.
///
/// Matches Dart: `_addForwardConfiguration` (`evaluate.dart:1728`). Guarded
/// entries keep the existing non-`null` value; others evaluate (slash-stripped
/// at the value node) into explicit configured values. Explicitness is sticky:
/// an explicit-or-empty base stays explicit, otherwise the result is implicit.
async fn add_forward_configuration<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    adjusted_config: &Configuration<'parse>,
    node: &ForwardRule<'parse>,
) -> SassResult<Configuration<'parse>>
where
    'compile: 'parse,
{
    let existing = adjusted_config.values();
    let mut new_values: IndexMap<String, ConfiguredValue<'parse>> = existing.into_iter().collect();

    for cv in &node.configuration {
        if cv.is_guarded {
            if let Some(old_val) = adjusted_config.remove(&cv.name) {
                if old_val.value != Value::new_with_arena(arena, ValueKind::Null) {
                    new_values.insert(cv.name.clone(), old_val);
                    continue;
                }
            }
        }

        let val = evaluate_expression(config, state, arena, &cv.expression).await?;
        let expr_node = expression_node(config, state, &cv.expression)?;
        let val = without_slash(config, state, arena, val, expr_node)?;
        new_values.insert(
            cv.name.clone(),
            ConfiguredValue::explicit(val, cv.span, expr_node),
        );
    }

    if adjusted_config.is_explicit() || adjusted_config.is_empty() {
        Ok(Configuration::new_explicit(arena, new_values, node.span))
    } else {
        Ok(Configuration::new_implicit(arena, new_values))
    }
}

// ===========================================================================
// remove_used_configuration — removes consumed keys from upstream config
// Dart: evaluate.dart:1776
// Go: evaluate_statement.go:287
// ===========================================================================

/// Drops upstream configuration values consumed by a `@forward`.
///
/// Matches Dart: `_removeUsedConfiguration` (`evaluate.dart:1776`). Keys
/// still present downstream survive; `except` (the unguarded `with` names)
/// pins keys that this forward explicitly manages.
fn remove_used_configuration<'parse>(
    upstream: &Configuration<'parse>,
    downstream: &Configuration<'parse>,
    except: &HashSet<String>,
) {
    let keys: Vec<String> = upstream.keys();
    for name in keys {
        if except.contains(&name) {
            continue;
        }
        if downstream.get(&name).is_none() {
            upstream.remove(&name);
        }
    }
}

// ===========================================================================
// VisitForwardRule — @forward directive
// Go: evaluate_statement.go:824
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Evaluates `@forward`: loads the module with adjusted configuration and
/// forwards its members.
///
/// Matches Dart: `visitForwardRule` (`evaluate.dart:1678`). The ambient
/// configuration passes `through_forward` first; a non-empty `with` clause
/// builds an explicit config, loads inside the `@forward` frame, forwards the
/// module, then prunes consumed keys and asserts the remainder is empty (the
/// emptiness check runs *after* the frame exits, so its trace has no
/// `@forward` frame). The empty-`with` path swaps the adjusted configuration
/// in for the load and restores the original after.
pub(crate) async fn evaluate_forward_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &ForwardRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    // Built-in module check (matches evaluate_use_rule + Go loadModule line 657)
    if let Some(built_in) = config.built_in_modules.get(node.url.as_str()) {
        if !node.configuration.is_empty() {
            return Err(Box::new(exception(
                state,
                "Built-in modules can't be configured.".into(),
                Some(node.span),
            )));
        }
        register_comments_for_module(state, built_in)?;
        state
            .env
            .forward_module(arena, *built_in, node, node.span)?;
        return Ok(None);
    }

    // Configuration evaluation outside @forward frame (matching Go: evaluate_expression.go:891-895)
    let old_configuration = state.configuration.clone();
    let adjusted = old_configuration.through_forward(node);

    let mut forward_config = if !node.configuration.is_empty() {
        Some(add_forward_configuration(config, state, arena, &adjusted, node).await?)
    } else {
        None
    };

    with_stack_frame(
        config,
        state,
        arena,
        "@forward",
        node.span,
        None,
        async |c, s| -> SassResult<Option<Value<'parse>>> {
            let loaded = add_exception_span(c, s, node.span, None, async |c2, s2| {
                load_stylesheet(c2, s2, arena, &node.url, node.span, None, false).await
            })
            .await?;

            let sheet_url = loaded.stylesheet.span.source_url().map(|u| u.to_string());

            // Active modules: circular import detection
            if let Some(ref key) = sheet_url {
                if !key.is_empty() {
                    if s.active_modules.contains_key(key) {
                        let span_ctx = SourceSpanWithContext::from_file_span(&node.span)?;
                        return Err(Box::new(SassError::Runtime {
                            message: "Module loop: this module is already being loaded.".into(),
                            span: span_ctx,
                            trace: stack_trace(s, None),
                            cause: None,
                            loaded_urls: vec![],
                        }));
                    }
                    s.active_modules.insert(key.clone(), Some(node.span));
                }
            }

            let is_dependency = s.in_dependency || loaded.importer != s.importer;
            let old_in_dep = s.in_dependency;
            s.in_dependency = is_dependency;

            let result: SassResult<Option<Value<'parse>>> = async {
                if let Some(ref mut new_config) = forward_config {
                    let module = evaluate_module(
                        c,
                        s,
                        arena,
                        loaded.stylesheet,
                        Some(&*new_config),
                        &loaded.importer,
                        node.span,
                        false,
                    )
                    .await?;
                    register_comments_for_module(s, &module)?;
                    s.env.forward_module(arena, module, node, node.span)?;

                    let except: HashSet<String> = node
                        .configuration
                        .iter()
                        .filter(|cv| !cv.is_guarded)
                        .map(|cv| cv.name.clone())
                        .collect();
                    remove_used_configuration(&adjusted, &*new_config, &except);

                    let configured: HashSet<String> = node
                        .configuration
                        .iter()
                        .map(|cv| cv.name.clone())
                        .collect();
                    for name in new_config.keys() {
                        if !configured.contains(&name) {
                            new_config.remove(&name);
                        }
                    }
                } else {
                    s.configuration = adjusted.clone();
                    let module = evaluate_module(
                        c,
                        s,
                        arena,
                        loaded.stylesheet,
                        None,
                        &loaded.importer,
                        node.span,
                        false,
                    )
                    .await?;
                    register_comments_for_module(s, &module)?;
                    s.env.forward_module(arena, module, node, node.span)?;
                    s.configuration = old_configuration.clone();
                }
                Ok(None)
            }
            .await;

            // Cleanup active_modules regardless of success/failure (matching Go)
            if let Some(ref key) = sheet_url {
                if !key.is_empty() {
                    s.active_modules.shift_remove(key);
                }
            }
            s.in_dependency = old_in_dep;
            s.configuration = old_configuration.clone();
            result
        },
    )
    .await?;

    // Dart: `_assertConfigurationIsEmpty(newConfiguration)` runs *after*
    // `_loadModule` returns, i.e. outside the `@forward` stack frame, so the
    // "not declared with !default" error's trace has no `@forward` frame.
    if let Some(ref new_config) = forward_config {
        assert_configuration_is_empty(state, new_config)?;
    }
    Ok(None)
}

/// Throws when an explicit configuration still holds unused values.
///
/// Matches Dart: `_assertConfigurationIsEmpty` (`evaluate.dart:1795`).
/// Implicit configurations always pass (subsets allowed); otherwise the first
/// leftover reports at its configuration span — with the variable name only
/// when `name_in_error` (i.e. `load-css()`, where the span alone would not
/// name it).
fn assert_configuration_is_empty<'compile: 'parse, 'parse>(
    state: &mut EvalState<'compile, 'parse>,
    config: &Configuration<'parse>,
) -> SassResult<()> {
    if !config.is_explicit() || config.is_empty() {
        return Ok(());
    }
    let vals = config.values();
    if let Some((_, cv)) = vals.into_iter().next() {
        return Err(Box::new(exception(
            state,
            "This variable was not declared with !default in the @used module.".into(),
            cv.configuration_span,
        )));
    }
    Ok(())
}

// ===========================================================================
// VisitSupportsRule
// Go: evaluate_statement.go:2183
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Evaluates `@supports`: renders the condition and evaluates children with
/// style-rule bubbling.
///
/// Matches Dart: `visitSupportsRule` (`evaluate.dart:2539`, mirrored in
/// `visitCssSupportsRule`; condition rendering is `_visitSupportsCondition`,
/// owned by `expression::visit_supports_condition`). Rejects
/// nested-declaration context; plain-CSS nesting short-circuits to a scoped
/// evaluation, otherwise children run under a style-rule-`through` parent
/// with the enclosing style rule copied inward so bare declarations have a
/// home. Children scope on `has_declarations`.
pub(crate) async fn evaluate_supports_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &SupportsRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    if state.declaration_name.is_some() {
        return Err(Box::new(exception(
            state,
            "Supports rules may not be used within nested declarations.".into(),
            Some(node.span),
        )));
    }

    let condition_text = visit_supports_condition(config, state, arena, &node.condition).await?;
    let cond_span = node.condition.span()?;
    let condition = CssValue::new(condition_text, cond_span);
    let rule = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::SupportsRule(ModifiableCssSupportsRule::new(condition, node.span)),
    );

    if has_css_nesting(state) {
        let tabs_node = rule.clone();
        with_parent(
            arena,
            config,
            state,
            rule,
            None,
            Some(has_declarations(&node.children)),
            async |c, s| {
                // Opaque NESTED-tabs scope (libsass bubble boundary).
                let tabs_start = tabs_opaque_push(s);
                for child in &node.children {
                    evaluate_statement(c, s, arena, child).await?;
                }
                tabs_opaque_pop(s, &tabs_node, tabs_start);
                Ok(())
            },
        )
        .await?;
        return Ok(None);
    }

    let through_fn =
        |n: &ModifiableCssNode| matches!(&*n.kind(), ModifiableCssNodeKind::StyleRule(_));
    let through: Option<&dyn Fn(&ModifiableCssNode) -> bool> = Some(&through_fn);

    let tabs_node = rule.clone();
    with_parent(
        arena,
        config,
        state,
        rule,
        through,
        Some(has_declarations(&node.children)),
        async |config, state| {
            // Opaque NESTED-tabs scope (libsass bubble boundary).
            let tabs_start = tabs_opaque_push(state);
            let sr_node_clone = state.css_style_rule_node.clone();
            if let Some(ref sr_node) = sr_node_clone {
                let sr_copy = sr_node.copy_without_children(arena);
                // Bubble-copy style frame for NESTED tabs (see media path).
                tabs_frame_push(state, &sr_copy);
                with_parent(arena, config, state, sr_copy, None, None, async |c, s| {
                    for child in &node.children {
                        evaluate_statement(c, s, arena, child).await?;
                    }
                    Ok(())
                })
                .await?;
                tabs_frame_pop_and_stamp(state);
            } else {
                for child in &node.children {
                    evaluate_statement(config, state, arena, child).await?;
                }
            }
            tabs_opaque_pop(state, &tabs_node, tabs_start);
            Ok(())
        },
    )
    .await?;
    Ok(None)
}

// ===========================================================================
// VisitIfRule — conditional evaluation
// Go: evaluate_statement.go:1934
// ===========================================================================

/// Reports whether `children` can declare scoped members.
///
/// Matches the `hasDeclarations` getter checks (`evaluate.dart`, e.g.
/// `visitAtRootRule:1218`, `visitDeclaration:1422`): true for variable,
/// function, or mixin declarations, or an `@import` with a dynamic import
/// (which may itself declare members once loaded). Callers pass this as the
/// `when`/`scope_when` gate so environment scopes are only pushed when they
/// can matter.
fn has_declarations<'parse>(children: &[Statement<'parse>]) -> bool {
    for child in children {
        match child {
            Statement::VariableDeclaration(_)
            | Statement::FunctionRule(_)
            | Statement::MixinRule(_) => return true,
            Statement::ImportRule(import) => {
                for imp in &import.imports {
                    if matches!(imp, Import::Dynamic(_)) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

#[rust_sass_macros::maybe_async]
/// Evaluates `@if`/`@else if`/`@else`: first truthy clause wins, `else` last.
///
/// Matches Dart: `visitIfRule` (`evaluate.dart:1826`). Walks the clauses in
/// order (the optional `last_clause` is the default); the chosen children run
/// via [`evaluate_block`] in a semi-global scope gated on that clause's
/// `has_declarations`, so `@return` propagates out.
pub(crate) async fn evaluate_statement_if_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &IfRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let mut chosen: Option<&[Statement<'parse>]> = node.last_clause.as_deref();
    for clause in &node.clauses {
        let cond = evaluate_expression(config, state, arena, &clause.expression).await?;
        if cond.is_truthy() {
            chosen = Some(&clause.children);
            break;
        }
    }

    let children = match chosen {
        Some(c) => c,
        None => return Ok(None),
    };

    let has_decls = has_declarations(children);
    let env = state.env;
    env.scope(
        arena,
        async || evaluate_block(config, state, arena, children).await,
        true,
        has_decls,
    )
    .await
}

// ===========================================================================
// VisitEachRule
// Go: evaluate_statement.go:1786
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Evaluates `@each`: binds each item (destructured when multi-variable) and
/// runs the body per item.
///
/// Matches Dart: `visitEachRule` + `_setMultipleVariables`
/// (`evaluate.dart:1432-1474`). Loop variables are defined at the list
/// expression node with per-item (and per-sub-item) slash-stripping; missing
/// destructured slots bind `null`. Runs in a semi-global scope;
/// `@return` in the body unwinds the whole loop.
pub(crate) async fn evaluate_statement_each_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &EachRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let list_val = evaluate_expression(config, state, arena, &node.list).await?;
    let items = list_val.as_list(arena)?;

    let env = state.env;
    env.scope(
        arena,
        async || -> SassResult<Option<Value<'parse>>> {
            for item in items {
                // Dart `visitEachRule`: definition spans come from
                // `_expressionNode(node.list)` and slash-stripping applies
                // per sub-item (`_setMultipleVariables`).
                let span = expression_node(config, state, &node.list)?;
                let cleaned = without_slash(config, state, arena, item, span)?;
                if node.variables.len() == 1 {
                    state
                        .env
                        .set_local_variable(&node.variables[0], cleaned, span);
                } else {
                    let sub_items = cleaned.as_list(arena)?;
                    let min_len = node.variables.len().min(sub_items.len());
                    for (vi, sub_item) in sub_items.iter().take(min_len).enumerate() {
                        let sub = without_slash(config, state, arena, *sub_item, span)?;
                        state.env.set_local_variable(&node.variables[vi], sub, span);
                    }
                    for vi in min_len..node.variables.len() {
                        state.env.set_local_variable(
                            &node.variables[vi],
                            Value::new_with_arena(arena, ValueKind::Null),
                            span,
                        );
                    }
                }

                for child in &node.children {
                    if let Some(val) = evaluate_statement(config, state, arena, child).await? {
                        return Ok(Some(val));
                    }
                }
            }
            Ok(None)
        },
        true,
        true,
    )
    .await
}

// ===========================================================================
// VisitWhileRule
// Go: evaluate_statement.go:1908
// ===========================================================================

#[rust_sass_macros::maybe_async]
/// Evaluates `@while`: re-tests the condition each iteration in a shared scope.
///
/// Matches Dart: `visitWhileRule` (`evaluate.dart:2749`). One semi-global
/// scope (gated on the body's `has_declarations`) wraps the loop so variables
/// persist across iterations; `@return` in the body unwinds the loop.
pub(crate) async fn evaluate_statement_while_rule<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    node: &WhileRule<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let has_decls = has_declarations(&node.children);
    let env = state.env;
    env.scope(
        arena,
        async || -> SassResult<Option<Value<'parse>>> {
            loop {
                let cond = evaluate_expression(config, state, arena, &node.condition).await?;
                if !cond.is_truthy() {
                    return Ok(None);
                }
                for child in &node.children {
                    if let Some(val) = evaluate_statement(config, state, arena, child).await? {
                        return Ok(Some(val));
                    }
                }
            }
        },
        true,
        has_decls,
    )
    .await
}

// ===========================================================================
// TESTS
// ===========================================================================

#[cfg(test)]
mod tests {
    use crate::ast::sass::dynamic_import::DynamicImport;
    use crate::ast::sass::expression_string::StringExpression;
    use crate::ast::sass::static_import::StaticImport;
    use crate::compile::compile_string;
    use crate::compile::CompileOptions;
    use crate::eval::import_cache::ImportCache;
    use crate::io::Io;
    use crate::io::VirtualIo;
    use crate::logger::test_utils::RecordLogger;
    use crate::logger::Logger;
    use crate::logger::QuietLogger;
    use crate::value::SassList;
    use crate::value::SassString;
    use bumpalo::Bump;

    use super::*;

    use crate::ast::css::keyframe_block::ModifiableCssKeyframeBlock;
    use crate::ast::css::modifiable_node::{ModifiableCssNode, ModifiableCssNodeKind};
    use crate::ast::css::style_rule::CssStyleRule;
    use crate::ast::css::stylesheet::ModifiableCssStylesheet;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_boolean::BooleanExpression;
    use crate::ast::sass::expression_number::NumberExpression;
    use crate::ast::sass::expression_value::ValueExpression;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::ast::sass::statement::if_rule::IfClause;
    use crate::ast::sass::statement::style_rule::StyleRule;
    use crate::ast::sass::statement::stylesheet::Stylesheet;
    use crate::common::ast_css_value::CssValue;
    use crate::common::source_span_file_source::FileSource;
    use crate::common::span::Span;
    use crate::compile_context::new_compile_context;
    use crate::extend::store::DefaultExtensionStore;
    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    fn test_visitor<'compile, 'parse>(
        arena: &'compile Bump,
    ) -> (EvalConfig<'compile, 'parse>, EvalState<'compile, 'parse>)
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let span = make_span(arena, "test");
        let log: Rc<dyn Logger> = Rc::new(QuietLogger);
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let mut state = EvalState::new(arena, span);
        state.member = "root stylesheet".to_string();
        (
            EvalConfig::new(arena, log, new_compile_context(), io),
            state,
        )
    }

    fn test_visitor_with_root<'compile, 'parse>(
        arena: &'compile Bump,
    ) -> (EvalConfig<'compile, 'parse>, EvalState<'compile, 'parse>)
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let (config, mut state) = test_visitor(arena);
        let span = make_span(arena, "root");
        let root = ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::Stylesheet(ModifiableCssStylesheet::new(span)),
        );
        state.root = Some(root.clone());
        state.parent = Some(root);
        state.extension_store = Some(DefaultExtensionStore::new(
            ExtendMode::Normal,
            config.io.clone(),
            config.unicode,
        ));
        (config, state)
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_block_empty() {
        let arena = Bump::new();
        let (config, mut state) = test_visitor(&arena);
        let result = evaluate_block(&config, &mut state, &arena, &[]).await;
        assert!(result.is_ok());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_stylesheet_empty() {
        let arena = Bump::new();
        let span = make_span(&arena, "");
        let (config, mut state) = test_visitor(&arena);
        let sheet = Stylesheet::new(vec![], span);
        let result = evaluate_stylesheet(&config, &mut state, &arena, &sheet).await;
        assert!(result.is_ok());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_error_rule() {
        let arena = Bump::new();
        let span = make_span(&arena, "@error \"msg\"");
        let (config, mut state) = test_visitor(&arena);
        let expr = Expression::String(StringExpression {
            text: Interpolation::plain("msg".into(), Span::File(span)),
            has_quotes: true,
        });
        let node = ErrorRule {
            expression: expr,
            span,
        };
        let err = evaluate_error_rule(&config, &mut state, &arena, &node)
            .await
            .unwrap_err();
        assert!(err.message().contains("msg"));
    }

    // ── @return propagation ──

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_return_rule_returns_some() {
        let arena = Bump::new();
        let span = make_span(&arena, "@return 42px");
        let (config, mut state) = test_visitor(&arena);
        let expr = Expression::Number(NumberExpression {
            value: 42.0,
            unit: Some("px".into()),
            span,
        });
        let node = ReturnRule {
            expression: expr,
            span,
        };
        let result = evaluate_return_rule(&config, &mut state, &arena, &node)
            .await
            .unwrap();
        assert!(result.is_some());
        let val = result.unwrap();
        match &*val {
            ValueKind::Number(n) => assert!((n.value - 42.0).abs() < 0.001),
            other => panic!("expected Number, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_block_short_circuits() {
        let arena = Bump::new();
        let span = make_span(&arena, "@return 1; @return 2");
        let (config, mut state) = test_visitor(&arena);
        let expr1 = Expression::Number(NumberExpression {
            value: 1.0,
            unit: None,
            span,
        });
        let expr2 = Expression::Number(NumberExpression {
            value: 2.0,
            unit: None,
            span,
        });
        let return1 = Statement::ReturnRule(ReturnRule {
            expression: expr1,
            span,
        });
        let return2 = Statement::ReturnRule(ReturnRule {
            expression: expr2,
            span,
        });
        let children = vec![return1, return2];
        let result = evaluate_block(&config, &mut state, &arena, &children)
            .await
            .unwrap();
        assert!(result.is_some());
        let val = result.unwrap();
        match &*val {
            ValueKind::Number(n) => assert!((n.value - 1.0).abs() < 0.001),
            other => panic!("expected Number(1), got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_block_no_return() {
        let arena = Bump::new();
        let span = make_span(&arena, "$x: 1");
        let (config, mut state) = test_visitor(&arena);
        let var = Statement::VariableDeclaration(VariableDeclaration {
            name: "x".into(),
            expression: Expression::Number(NumberExpression {
                value: 1.0,
                unit: None,
                span,
            }),
            span,
            namespace: None,
            guarded: false,
            is_global: false,
            comment: None,
        });
        let children = vec![var];
        let result = evaluate_block(&config, &mut state, &arena, &children)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    // ── @return propagation through control flow ──

    #[rust_sass_macros::maybe_test]
    async fn test_return_propagates_through_if_rule() {
        let arena = Bump::new();
        let span = make_span(&arena, "@if true { @return 7; }");
        let (config, mut state) = test_visitor(&arena);
        let return_stmt = Statement::ReturnRule(ReturnRule {
            expression: Expression::Number(NumberExpression {
                value: 7.0,
                unit: None,
                span,
            }),
            span,
        });
        let clause = IfClause {
            expression: Expression::Boolean(BooleanExpression { value: true, span }),
            children: vec![return_stmt],
        };
        let rule = IfRule {
            clauses: vec![clause],
            last_clause: None,
            span,
        };
        let result = evaluate_statement_if_rule(&config, &mut state, &arena, &rule)
            .await
            .unwrap();
        assert!(result.is_some());
        let val = result.unwrap();
        match &*val {
            ValueKind::Number(n) => assert!((n.value - 7.0).abs() < 0.001),
            other => panic!("expected Number(7), got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_return_propagates_through_for_rule() {
        let arena = Bump::new();
        let span = make_span(&arena, "@for $i from 1 through 3 { @return 5; }");
        let (config, mut state) = test_visitor(&arena);
        let return_stmt = Statement::ReturnRule(ReturnRule {
            expression: Expression::Number(NumberExpression {
                value: 5.0,
                unit: None,
                span,
            }),
            span,
        });
        let rule = ForRule {
            variable: "i".into(),
            from: Expression::Number(NumberExpression {
                value: 1.0,
                unit: None,
                span,
            }),
            to: Expression::Number(NumberExpression {
                value: 3.0,
                unit: None,
                span,
            }),
            is_exclusive: false,
            children: vec![return_stmt],
            span,
        };
        let result = evaluate_statement_for_rule(&config, &mut state, &arena, &rule)
            .await
            .unwrap();
        assert!(result.is_some());
        let val = result.unwrap();
        match &*val {
            ValueKind::Number(n) => assert!((n.value - 5.0).abs() < 0.001),
            other => panic!("expected Number(5), got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_return_propagates_through_each_rule() {
        let arena = Bump::new();
        let span = make_span(&arena, "@each $x in (1) { @return 3; }");
        let (config, mut state) = test_visitor(&arena);
        let return_stmt = Statement::ReturnRule(ReturnRule {
            expression: Expression::Number(NumberExpression {
                value: 3.0,
                unit: None,
                span,
            }),
            span,
        });
        let list = SassList::new(
            vec![Value::new_with_arena(&arena, ValueKind::Null)],
            ListSeparator::Undecided,
            false,
        );
        let rule = EachRule {
            variables: vec!["x".into()],
            list: Expression::Value(ValueExpression {
                value: Box::new(Value::new_with_arena(&arena, ValueKind::List(list))),
                span,
            }),
            children: vec![return_stmt],
            span,
        };
        let result = evaluate_statement_each_rule(&config, &mut state, &arena, &rule)
            .await
            .unwrap();
        assert!(result.is_some());
        let val = result.unwrap();
        match &*val {
            ValueKind::Number(n) => assert!((n.value - 3.0).abs() < 0.001),
            other => panic!("expected Number(3), got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_return_propagates_through_while_rule() {
        let arena = Bump::new();
        let span = make_span(&arena, "@while true { @return 9; }");
        let (config, mut state) = test_visitor(&arena);
        let return_stmt = Statement::ReturnRule(ReturnRule {
            expression: Expression::Number(NumberExpression {
                value: 9.0,
                unit: None,
                span,
            }),
            span,
        });
        let rule = WhileRule {
            condition: Expression::Boolean(BooleanExpression { value: true, span }),
            children: vec![return_stmt],
            span,
        };
        let result = evaluate_statement_while_rule(&config, &mut state, &arena, &rule)
            .await
            .unwrap();
        assert!(result.is_some());
        let val = result.unwrap();
        match &*val {
            ValueKind::Number(n) => assert!((n.value - 9.0).abs() < 0.001),
            other => panic!("expected Number(9), got {other:?}"),
        }
    }

    // ── at-rule value interpolation ──

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_at_rule_with_value() {
        let arena = Bump::new();
        let span = make_span(&arena, "@import url(\"foo.css\")");
        let (config, mut state) = test_visitor_with_root(&arena);
        let name = Interpolation::plain("import".into(), Span::File(span));
        let value = Interpolation::plain("url(\"foo.css\")".into(), Span::File(span));
        let rule = AtRule {
            name,
            value: Some(value),
            span,
            children: None,
        };
        evaluate_at_rule(&config, &mut state, &arena, &rule)
            .await
            .unwrap();
        let children = state.root.as_ref().unwrap().children().unwrap();
        assert_eq!(children.len(), 1);
        // Check the at-rule node has the value
        let kind = children[0].kind();
        if let ModifiableCssNodeKind::AtRule(a) = &*kind {
            let v = &a.value;
            assert!(v.is_some());
            assert_eq!(v.as_ref().unwrap().value, "url(\"foo.css\")");
        } else {
            panic!("expected AtRule");
        }
    }

    // ── declaration CSS node ──

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_declaration_creates_css_node() {
        let arena = Bump::new();
        let span = make_span(&arena, "color: red");
        let (config, mut state) = test_visitor_with_root(&arena);
        state.in_unknown_at_rule = true;
        let name = Interpolation::plain("color".into(), Span::File(span));
        let val_expr = Expression::Value(ValueExpression {
            value: Box::new(Value::new_with_arena(
                &arena,
                ValueKind::String(SassString {
                    text: "red",
                    has_quotes: false,
                }),
            )),
            span,
        });
        let node = Declaration {
            name,
            value: Some(val_expr),
            span,
            children: None,
            parsed_as_sass_script: true,
        };
        evaluate_declaration(&config, &mut state, &arena, &node)
            .await
            .unwrap();
        let children = state.root.as_ref().unwrap().children().unwrap();
        assert_eq!(children.len(), 1);
        let kind = children[0].kind();
        if let ModifiableCssNodeKind::Declaration(d) = &*kind {
            assert_eq!(d.name.value, "color");
        } else {
            panic!("expected Declaration");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_declaration_outside_style_rule() {
        let arena = Bump::new();
        let span = make_span(&arena, "color: red");
        let (config, mut state) = test_visitor_with_root(&arena);
        let name = Interpolation::plain("color".into(), Span::File(span));
        let val_expr = Expression::Value(ValueExpression {
            value: Box::new(Value::new_with_arena(&arena, ValueKind::Null)),
            span,
        });
        let node = Declaration {
            name,
            value: Some(val_expr),
            span,
            children: None,
            parsed_as_sass_script: true,
        };
        let err = evaluate_declaration(&config, &mut state, &arena, &node)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "Declarations may only be used within style rules."
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_declaration_blank_value() {
        let arena = Bump::new();
        let span = make_span(&arena, "color: null");
        let (config, mut state) = test_visitor_with_root(&arena);
        state.in_unknown_at_rule = true;
        let name = Interpolation::plain("color".into(), Span::File(span));
        let val_expr = Expression::Value(ValueExpression {
            value: Box::new(Value::new_with_arena(&arena, ValueKind::Null)),
            span,
        });
        let node = Declaration {
            name,
            value: Some(val_expr),
            span,
            children: None,
            parsed_as_sass_script: true,
        };
        evaluate_declaration(&config, &mut state, &arena, &node)
            .await
            .unwrap();
        let children = state.root.as_ref().unwrap().children().unwrap();
        assert_eq!(children.len(), 0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_declaration_nested_name_prefix() {
        let arena = Bump::new();
        let span = make_span(&arena, "font-weight: bold");
        let (config, mut state) = test_visitor_with_root(&arena);
        state.in_unknown_at_rule = true;
        state.declaration_name = Some("font".into());
        let name = Interpolation::plain("weight".into(), Span::File(span));
        let val_expr = Expression::Value(ValueExpression {
            value: Box::new(Value::new_with_arena(
                &arena,
                ValueKind::String(SassString {
                    text: "bold",
                    has_quotes: false,
                }),
            )),
            span,
        });
        let node = Declaration {
            name,
            value: Some(val_expr),
            span,
            children: None,
            parsed_as_sass_script: true,
        };
        evaluate_declaration(&config, &mut state, &arena, &node)
            .await
            .unwrap();
        let children = state.root.as_ref().unwrap().children().unwrap();
        assert_eq!(children.len(), 1);
        let kind = children[0].kind();
        if let ModifiableCssNodeKind::Declaration(d) = &*kind {
            assert_eq!(d.name.value, "font-weight");
        } else {
            panic!("expected Declaration");
        }
    }

    // ── evaluateMediaRule query parsing ──

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_media_rule_simple_query() {
        let arena = Bump::new();
        let span = make_span(&arena, "@media screen { $x: 1; }");
        let (config, mut state) = test_visitor_with_root(&arena);
        let query = Interpolation::plain("screen".into(), Span::File(span));
        // Add a child that sets a variable — this proves the with_media_queries
        // callback ran successfully (which only happens if parsing succeeded).
        let child = Statement::VariableDeclaration(VariableDeclaration {
            name: "x".into(),
            expression: Expression::Number(NumberExpression {
                value: 1.0,
                unit: None,
                span,
            }),
            span,
            namespace: None,
            guarded: false,
            is_global: false,
            comment: None,
        });
        let rule = MediaRule {
            query,
            children: vec![child],
            span,
        };
        evaluate_media_rule(&config, &mut state, &arena, &rule)
            .await
            .unwrap();
        // Dart scopes `@media` children with `when: node.hasDeclarations`
        // `$x` must not leak to the outer environment.
        let val = state.env.get_variable("x", None).unwrap();
        assert!(val.is_none(), "$x leaked out of @media scope");
        // The rule itself was still evaluated and attached.
        let children = state.root.as_ref().unwrap().children().unwrap();
        assert_eq!(children.len(), 1);
        assert!(
            matches!(&*children[0].kind(), ModifiableCssNodeKind::MediaRule(_)),
            "expected MediaRule"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_media_rule_nested_declaration_error() {
        let arena = Bump::new();
        let span = make_span(&arena, "@media screen");
        let (config, mut state) = test_visitor_with_root(&arena);
        state.declaration_name = Some("color".into());
        let query = Interpolation::plain("screen".into(), Span::File(span));
        let rule = MediaRule {
            query,
            children: vec![],
            span,
        };
        let err = evaluate_media_rule(&config, &mut state, &arena, &rule)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "Media rules may not be used within nested declarations."
        );
    }

    // ── evaluateExtendRule ──

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_extend_rule_outside_style_rule() {
        let arena = Bump::new();
        let span = make_span(&arena, "@extend .foo");
        let (config, mut state) = test_visitor_with_root(&arena);
        let selector = Interpolation::plain(".foo".into(), Span::File(span));
        let rule = ExtendRule {
            selector,
            span,
            is_optional: false,
        };
        let err = evaluate_extend_rule(&config, &mut state, &arena, &rule)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "@extend may only be used within style rules."
        );
    }

    // ── evaluateStyleRule ──

    fn test_visitor_with_root_and_stylesheet<'compile, 'parse>(
        arena: &'compile Bump,
        plain_css: bool,
    ) -> (EvalConfig<'compile, 'parse>, EvalState<'compile, 'parse>)
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let (config, mut state) = test_visitor_with_root(arena);
        let span = make_span(arena, "root");
        state.stylesheet = Some(arena.alloc(Stylesheet::detailed(
            vec![],
            span,
            vec![],
            plain_css,
            indexmap::IndexMap::new(),
        )));
        (config, state)
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_style_rule_simple() {
        let arena = Bump::new();
        let span = make_span(&arena, ".foo");
        let (config, mut state) = test_visitor_with_root_and_stylesheet(&arena, false);
        state.in_unknown_at_rule = true;
        state.in_dependency = false;
        let selector = Interpolation::plain(".foo".into(), Span::File(span));
        let rule = StyleRule::new(selector, vec![], span);

        let result = evaluate_style_rule(&config, &mut state, &arena, &rule).await;
        assert!(result.is_ok());
        let children = state.root.as_ref().unwrap().children().unwrap();
        assert_eq!(
            children.len(),
            1,
            "expected 1 child, got {:?}",
            children.len()
        );
        let kind = children[0].kind();
        assert!(
            matches!(&*kind, ModifiableCssNodeKind::StyleRule(_)),
            "expected ModifiableCssStyleRule, got {:?}",
            &*kind
        );
        if let ModifiableCssNodeKind::StyleRule(sr) = &*kind {
            let sel = sr.selector.borrow().to_css_string(false).unwrap();
            assert_eq!(sel, ".foo");
        } else {
            panic!("expected ModifiableCssStyleRule");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_style_rule_nested_declaration_error() {
        let arena = Bump::new();
        let span = make_span(&arena, ".foo");
        let (config, mut state) = test_visitor_with_root_and_stylesheet(&arena, false);
        state.declaration_name = Some("color".into());
        let selector = Interpolation::plain(".foo".into(), Span::File(span));
        let rule = StyleRule::new(selector, vec![], span);

        let err = evaluate_style_rule(&config, &mut state, &arena, &rule)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "Style rules may not be used within nested declarations."
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_style_rule_keyframe_block_error() {
        let arena = Bump::new();
        let span = make_span(&arena, ".bar");
        let (config, mut state) = test_visitor_with_root_and_stylesheet(&arena, false);
        state.in_keyframes = true;
        let kfb = ModifiableCssKeyframeBlock::new(CssValue::new(vec!["from".into()], span), span);
        state.parent = Some(ModifiableCssNode::new(
            &arena,
            ModifiableCssNodeKind::KeyframeBlock(kfb),
        ));
        let selector = Interpolation::plain(".bar".into(), Span::File(span));
        let rule = StyleRule::new(selector, vec![], span);

        let err = evaluate_style_rule(&config, &mut state, &arena, &rule)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "Style rules may not be used within keyframe blocks."
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_style_rule_keyframe() {
        let arena = Bump::new();
        let span = make_span(&arena, "from");
        let (config, mut state) = test_visitor_with_root_and_stylesheet(&arena, false);
        state.in_keyframes = true;
        let selector = Interpolation::plain("from".into(), Span::File(span));
        let rule = StyleRule::new(selector, vec![], span);

        let result = evaluate_style_rule(&config, &mut state, &arena, &rule).await;
        assert!(result.is_ok());
        let children = state.root.as_ref().unwrap().children().unwrap();
        assert_eq!(children.len(), 1);
        let kind = children[0].kind();
        assert!(
            matches!(&*kind, ModifiableCssNodeKind::KeyframeBlock(_)),
            "expected ModifiableCssKeyframeBlock, got {:?}",
            &*kind
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_style_rule_group_end_set() {
        let arena = Bump::new();
        let span = make_span(&arena, ".foo");
        let (config, mut state) = test_visitor_with_root_and_stylesheet(&arena, false);
        state.in_unknown_at_rule = true;
        state.in_dependency = false;
        let selector = Interpolation::plain(".foo".into(), Span::File(span));
        let rule = StyleRule::new(selector, vec![], span);

        evaluate_style_rule(&config, &mut state, &arena, &rule)
            .await
            .unwrap();
        let children = state.root.as_ref().unwrap().children().unwrap();
        assert_eq!(children.len(), 1);
        assert!(
            children[0].is_group_end(),
            "expected last child to be group end"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_style_rule_nested() {
        let arena = Bump::new();
        let span = make_span(&arena, ".parent");
        let (config, mut state) = test_visitor_with_root_and_stylesheet(&arena, false);
        state.in_unknown_at_rule = true;
        state.in_dependency = false;
        let parent_sel = Interpolation::plain(".parent".into(), Span::File(span));
        let parent_rule = StyleRule::new(parent_sel, vec![], span);
        evaluate_style_rule(&config, &mut state, &arena, &parent_rule)
            .await
            .unwrap();

        let parent_css = {
            let children = state.root.as_ref().unwrap().children().unwrap();
            assert!(!children.is_empty());
            let kind = children[0].kind();
            if let ModifiableCssNodeKind::StyleRule(msr) = &*kind {
                CssStyleRule::new(
                    *msr.selector.borrow(),
                    span,
                    msr.original_selector,
                    msr.from_plain_css,
                )
            } else {
                panic!("expected ModifiableCssStyleRule, got {:?}", &*kind);
            }
        };
        state.style_rule_ignoring_at_root = Some(parent_css);

        let child_sel = Interpolation::plain(".child".into(), Span::File(span));
        let child_rule = StyleRule::new(child_sel, vec![], span);
        evaluate_style_rule(&config, &mut state, &arena, &child_rule)
            .await
            .unwrap();

        let children = state.root.as_ref().unwrap().children().unwrap();
        assert_eq!(children.len(), 2);
        let kind = children[1].kind();
        if let ModifiableCssNodeKind::StyleRule(child_sr) = &*kind {
            let sel = child_sr.selector.borrow().to_css_string(false).unwrap();
            assert_eq!(sel, ".parent .child");
        } else {
            panic!(
                "expected ModifiableCssStyleRule for nested child, got {:?}",
                &*kind
            );
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_style_rule_plain_css_leading_combinator_error() {
        let arena = Bump::new();
        let span = make_span(&arena, ".parent");
        let (config, mut state) = test_visitor_with_root_and_stylesheet(&arena, false);
        state.in_unknown_at_rule = true;
        state.in_dependency = false;
        let parent_sel = Interpolation::plain(".parent".into(), Span::File(span));
        let parent_rule = StyleRule::new(parent_sel, vec![], span);
        evaluate_style_rule(&config, &mut state, &arena, &parent_rule)
            .await
            .unwrap();

        let parent_css = {
            let children = state.root.as_ref().unwrap().children().unwrap();
            let kind = children[0].kind();
            if let ModifiableCssNodeKind::StyleRule(msr) = &*kind {
                CssStyleRule::new(*msr.selector.borrow(), span, msr.original_selector, false)
            } else {
                panic!("expected ModifiableCssStyleRule, got {:?}", &*kind);
            }
        };
        state.style_rule_ignoring_at_root = Some(parent_css);

        state.stylesheet = Some(arena.alloc(Stylesheet::detailed(
            vec![],
            span,
            vec![],
            true,
            indexmap::IndexMap::new(),
        )));

        let child_sel = Interpolation::plain("> .child".into(), Span::File(span));
        let child_rule = StyleRule::new(child_sel, vec![], span);
        let err = evaluate_style_rule(&config, &mut state, &arena, &child_rule)
            .await
            .unwrap_err();
        assert_eq!(
            err.message(),
            "Top-level leading combinators aren't allowed in plain CSS."
        );
    }

    // ── evaluateImportRule ──

    fn test_visitor_with_root_stylesheet_and_cache<'compile, 'parse>(
        arena: &'compile Bump,
    ) -> (EvalConfig<'compile, 'parse>, EvalState<'compile, 'parse>)
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let (config, mut state) = test_visitor_with_root_and_stylesheet(arena, false);
        let empty_importers: Vec<Importer<'parse>> = vec![];
        state.import_cache = Some(ImportCache::new(arena, empty_importers, false));
        (config, state)
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_import_rule_static() {
        let arena = Bump::new();
        let span = make_span(&arena, "@import \"foo.css\"");
        let (config, mut state) = test_visitor_with_root_stylesheet_and_cache(&arena);
        let url = Interpolation::plain("foo.css".into(), Span::File(span));
        let static_import = StaticImport::new(url, span, None);
        let imports = vec![Import::Static(static_import)];
        let rule = ImportRule::new(imports, span);

        let result = evaluate_import_rule(&config, &mut state, &arena, &rule).await;
        assert!(result.is_ok());
        let children = state.root.as_ref().unwrap().children().unwrap();
        assert_eq!(children.len(), 1);
        let kind = children[0].kind();
        assert!(
            matches!(&*kind, ModifiableCssNodeKind::Import(_)),
            "expected ModifiableCssImport, got {:?}",
            &*kind
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_import_rule_static_with_modifiers() {
        let arena = Bump::new();
        let span = make_span(&arena, "@import \"foo.css\" screen");
        let (config, mut state) = test_visitor_with_root_stylesheet_and_cache(&arena);
        let url = Interpolation::plain("foo.css".into(), Span::File(span));
        let modifiers = Interpolation::plain("screen".into(), Span::File(span));
        let static_import = StaticImport::new(url, span, Some(modifiers));
        let imports = vec![Import::Static(static_import)];
        let rule = ImportRule::new(imports, span);

        let result = evaluate_import_rule(&config, &mut state, &arena, &rule).await;
        assert!(result.is_ok());
        let children = state.root.as_ref().unwrap().children().unwrap();
        assert_eq!(children.len(), 1);
        let kind = children[0].kind();
        assert!(
            matches!(&*kind, ModifiableCssNodeKind::Import(_)),
            "expected ModifiableCssImport, got {:?}",
            &*kind
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_declaration_children_scope() {
        // `$x` declared inside nested-declaration children must not leak to a
        // sibling declaration in the same style rule (Dart `visitDeclaration`
        // scopes children with `when: node.hasDeclarations`).
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "a { b: { c: d; $x: 1; } e: $x; }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, .. } => {
                assert_eq!(message, "Undefined variable.")
            }
            other => panic!("expected Runtime Undefined variable, got {other:?}"),
        }

        // No declarations in children: plain nesting still compiles.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let result = compile_string(
            "a { b: { c: d; } e: f; }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert!(result.css().contains("e: f;"), "got: {}", result.css());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_media_scope_and_sources() {
        // Variables declared in `@media` children must not leak out (Dart
        // `visitMediaRule` scopes the outer `_withParent` with
        // `when: node.hasDeclarations`).
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@media screen { $x: 1; }\na { color: $x; }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, .. } => {
                assert_eq!(message, "Undefined variable.")
            }
            other => panic!("expected Runtime Undefined variable, got {other:?}"),
        }

        // Triple-nested media still merges and bubbles.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let result = compile_string(
            "@media screen { @media (min-width: 100px) { @media (orientation: landscape) { d { e: f; } } } }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert!(
            result
                .css()
                .contains("@media screen and (min-width: 100px) and (orientation: landscape)"),
            "got: {}",
            result.css()
        );

        // Declarations directly inside `@media` in a style rule inherit the
        // selector via the unscoped inner copy.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let result = compile_string(
            "a { @media screen { b: c; } d: e; }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert!(result.css().contains("b: c;"), "got: {}", result.css());
        assert!(result.css().contains("d: e;"), "got: {}", result.css());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_style_rule_scope() {
        // Style rules scope children only when they contain declarations
        // (Dart `visitStyleRule`/`visitKeyframeBlock` use
        // `scopeWhen: node.hasDeclarations`). These paths must keep exact
        // output and assignment semantics.
        let cases = [
            (
                "a { b: c; d { e: f; } }",
                "a {\n  b: c;\n}\na d {\n  e: f;\n}",
            ),
            (
                "$x: outer;\na { b: $x; $x: inner; c: $x; }\nd { e: $x; }",
                "a {\n  b: outer;\n  c: inner;\n}\n\nd {\n  e: outer;\n}",
            ),
            (
                "a { @keyframes f { from { b: c; } to { d: e; } } }",
                "@keyframes f {\n  from {\n    b: c;\n  }\n  to {\n    d: e;\n  }\n}",
            ),
        ];
        for (src, expected) in cases {
            let arena = Bump::new();
            let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
            let result = compile_string(src, io, CompileOptions::new(&arena), &arena)
                .await
                .unwrap();
            assert_eq!(result.css(), expected, "src: {src}");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_namespaced_include_missing_module() {
        // Dart `visitIncludeRule` reports a missing namespace as
        // `There is no module with the namespace "ns".` (a `Script` from
        // `get_module`, spanned at the `@include` by the surrounding
        // `add_exception_span`).
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@include nosuch.mixin;",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, .. } => {
                assert_eq!(message, "There is no module with the namespace \"nosuch\".")
            }
            other => panic!("expected Runtime missing-module error, got {other:?}"),
        }

        // A missing mixin in an existing module still reports Undefined mixin.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@use \"sass:math\";\na { @include math.undefined; }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, .. } => {
                assert_eq!(message, "Undefined mixin.")
            }
            other => panic!("expected Runtime Undefined mixin, got {other:?}"),
        }

        // A plain include keeps working.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let result = compile_string(
            "@mixin foo { b: c; }\na { @include foo; }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert!(result.css().contains("b: c;"), "got: {}", result.css());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_at_rule_nested_trim_scope() {
        // At-rules (reaching eval, e.g. via `@include` content) may not appear
        // inside nested declarations (Dart `visitAtRule` guard).
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@mixin m { @content; }\na { b: { c: d; @include m { @foo { e: f; } } } }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, .. } => assert_eq!(
                message,
                "At-rules may not be used within nested declarations."
            ),
            other => panic!("expected Runtime nested-declaration error, got {other:?}"),
        }

        // Unknown at-rule values trim whitespace on both sides (Dart
        // `_interpolationToValue(value, trim: true)`).
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let result = compile_string(
            "@foo #{'  bar  '};",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert_eq!(result.css(), "@foo bar;");

        // Ordinary unknown at-rules in style rules still bubble with the
        // selector via the unscoped inner copy.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let result = compile_string(
            "a { @foo { b: c; } d: e; }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert!(result.css().contains("@foo"), "got: {}", result.css());
        assert!(result.css().contains("b: c;"), "got: {}", result.css());
        assert!(result.css().contains("d: e;"), "got: {}", result.css());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_at_root_query_sources_unknown() {
        // `in_unknown_at_rule` is preserved when the (post-trim) `included`
        // list holds an at-rule (Dart `_scopeForAtRoot`), so declarations
        // hoisted out of `@foo` stay legal.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let result = compile_string(
            "@media screen {\n  @foo {\n    a {\n      @at-root (without: media rule) { b: c; }\n    }\n  }\n}",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert!(
            result.css().contains("@foo {\n  b: c;\n}"),
            "got: {}",
            result.css()
        );

        // Excluding media drops the media wrapper but keeps the style rule.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let result = compile_string(
            "@media screen {\n  a {\n    @at-root (without: media) { b: c; }\n  }\n}",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert_eq!(result.css(), "a {\n  b: c;\n}");

        // Query interpolation warns on color values (Dart
        // `_performInterpolationWithMap(unparsedQuery, warnForColor: true)`).
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let logger = Rc::new(RecordLogger::new());
        let opts = CompileOptions {
            logger: Some(logger.clone()),
            ..CompileOptions::new(&arena)
        };
        let result = compile_string(
            "a {\n  @at-root (without: #{red}) { b: c; }\n}",
            io,
            opts,
            &arena,
        )
        .await
        .unwrap();
        assert!(result.css().contains("b: c;"), "got: {}", result.css());
        assert!(
            logger
                .messages()
                .iter()
                .any(|m| m.contains("don't mean to use the color value red")),
            "got: {:?}",
            logger.messages()
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_warn_wraps_expression() {
        // `@warn` evaluates its expression inside `add_exception_span` with
        // the `@warn` node span (Dart `visitWarnRule`), so unspanned errors
        // surface there. Errors already converted by nearer Dart-mirroring
        // converters (e.g. variable lookup) keep their inner spans.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let logger = Rc::new(RecordLogger::new());
        let opts = CompileOptions {
            logger: Some(logger.clone()),
            ..CompileOptions::new(&arena)
        };
        compile_string("a { @warn \"hi\"; }", io, opts, &arena)
            .await
            .unwrap();
        assert_eq!(logger.messages(), vec!["hi".to_string()]);

        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "a { @warn ns.$x; }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, span, .. } => {
                assert_eq!(message, "There is no module with the namespace \"ns\".");
                assert_eq!((span.line(), span.column()), (1, 11));
            }
            other => panic!("expected Runtime error, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_for_bounds_inside_span() {
        // Dart `visitForRule` evaluates the `from`/`to` bounds inside
        // `_addExceptionSpan`, so bound errors surface at the bound spans.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let result = compile_string(
            "@for $i from 1 through 2 { a { b: $i; } }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert!(result.css().contains("b: 1;"), "got: {}", result.css());
        assert!(result.css().contains("b: 2;"), "got: {}", result.css());

        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@for $i from \"a\" through 3 { a { b: $i; } }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, span, .. } => {
                assert_eq!(message, "\"a\" is not a number.");
                assert_eq!((span.line(), span.column()), (1, 14));
            }
            other => panic!("expected Runtime bound error, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_each_destructure_slash_span() {
        // Dart `visitEachRule`/`_setMultipleVariables`: slash-stripping
        // applies per destructured sub-item (with slash-div warnings), and
        // loop variables are defined at the list expression span.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let logger = Rc::new(RecordLogger::new());
        let opts = CompileOptions {
            logger: Some(logger.clone()),
            ..CompileOptions::new(&arena)
        };
        let result = compile_string(
            "@each $a, $b in ((1/2 3/4), (5/6 7/8)) { c { d: $a; e: $b; } }",
            io,
            opts,
            &arena,
        )
        .await
        .unwrap();
        assert!(result.css().contains("d: 0.5;"), "got: {}", result.css());
        assert!(result.css().contains("e: 0.75;"), "got: {}", result.css());
        assert!(
            logger
                .messages()
                .iter()
                .any(|m| m.contains("slash-div") || m.contains("Using / for division")),
            "got: {:?}",
            logger.messages()
        );

        // Single-variable form keeps working.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let result = compile_string(
            "@each $x in (a b c) { d { e: $x; } }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert!(result.css().contains("e: a;"), "got: {}", result.css());
        assert!(result.css().contains("e: c;"), "got: {}", result.css());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_var_decl_rhs_bare() {
        // Dart `visitVariableDeclaration` evaluates the RHS bare (only the
        // `setVariable` call is span-wrapped): RHS errors defer to the
        // expression's own span, not the declaration span.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "$x: ns.$y;\na { b: 1; }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, span, .. } => {
                assert_eq!(message, "There is no module with the namespace \"ns\".");
                assert_eq!((span.line(), span.column()), (1, 5));
            }
            other => panic!("expected Runtime error, got {other:?}"),
        }

        // Ordinary declarations still assign.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let result = compile_string(
            "$x: 42;\na { b: $x; }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert!(result.css().contains("b: 42;"), "got: {}", result.css());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_import_rule_dynamic_not_found() {
        let arena = Bump::new();
        let span = make_span(&arena, "@import \"nonexistent-file\"");
        let (config, mut state) = test_visitor_with_root_stylesheet_and_cache(&arena);
        let dynamic_import = DynamicImport::new("nonexistent-file.scss".into(), span);
        let imports = vec![Import::Dynamic(dynamic_import)];
        let rule = ImportRule::new(imports, span);

        let err = evaluate_import_rule(&config, &mut state, &arena, &rule)
            .await
            .unwrap_err();
        assert_eq!(err.message(), "Can't find stylesheet to import.");
    }
}
