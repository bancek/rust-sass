// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/evaluate.dart (## Expressions section)
// go-source: go/eval/evaluate_expression.go

//! SassScript expression evaluation: the `## Expressions` section of Dart's
//! `EvaluateVisitor` (`evaluate.dart`).
//!
//! Dart implements these as `visit*Expression` methods on the visitor; here
//! each is a free function taking `(config, state, arena, …)` (see
//! `architecture.md` §7) dispatched from [`evaluate_expression`] by `match`
//! over [`Expression`]. Calculation-valued calls (`calc()`, `min()`, …)
//! delegate to `eval/calc.rs` (Dart's `_visitCalculation` family); plain-CSS
//! restrictions (operator/paren bans, skipped environment lookups) mirror
//! Dart's `_stylesheet.plainCss` gates at each visitor.

use std::collections::HashSet;

use bumpalo::Bump;

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::binary_operator::BinaryOperator;
use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression_binary_operation::BinaryOperationExpression;
use crate::ast::sass::expression_boolean::BooleanExpression;
use crate::ast::sass::expression_color::ColorExpression;
use crate::ast::sass::expression_function::FunctionExpression;
use crate::ast::sass::expression_if::{BooleanOperator, IfConditionExpression, IfExpression};
use crate::ast::sass::expression_interpolated_function::InterpolatedFunctionExpression;
use crate::ast::sass::expression_legacy_if::LegacyIfExpression;
use crate::ast::sass::expression_list::ListExpression;
use crate::ast::sass::expression_map::MapExpression;
use crate::ast::sass::expression_null::NullExpression;
use crate::ast::sass::expression_number::NumberExpression;
use crate::ast::sass::expression_parenthesized::ParenthesizedExpression;
use crate::ast::sass::expression_selector::SelectorExpression;
use crate::ast::sass::expression_string::StringExpression;
use crate::ast::sass::expression_supports::SupportsExpression;
use crate::ast::sass::expression_unary_operation::UnaryOperationExpression;
use crate::ast::sass::expression_value::ValueExpression;
use crate::ast::sass::expression_variable::VariableExpression;
use crate::ast::sass::interpolation::InterpolationPart;
use crate::ast::sass::statement::CallableDeclaration;
use crate::ast::sass::supports_condition::SupportsCondition;
use crate::ast::sass::unary_operator::UnaryOperator;
use crate::ast::sass::visitor::expression_to_calc::expression_to_calc;
#[cfg(feature = "async")]
use crate::callable::BuiltInCallback;
use crate::callable::{Callable, CallableKind, PlainCssCallable};
use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::deprecation::SLASH_DIV;
use crate::eval::calc::evaluate_css_math_function;
use crate::eval::helpers::{
    add_error_span, add_exception_span, evaluate_arguments, evaluate_macro_arguments, exception,
    expression_node, file_span_to_ctx, perform_interpolation, run_user_defined_callable,
    serialize_value, stack_trace, verify_parameter_list, without_slash,
};
use crate::eval::warn::warn_deprecation_span;
use crate::eval::{EvalConfig, EvalState};
use crate::parse::stylesheet_parse::parse_parameter_list;
use crate::util::utils::{pluralize, to_sentence};
use crate::value::{
    ListSeparator, SassArgumentList, SassBoolean, SassList, SassMap, SassNumber, SassString, Value,
    ValueKind,
};

/// Names matched case-insensitively before consulting built-ins or globals.
///
/// Matches Dart: the calculation-function `switch` in `visitFunctionExpression`
/// (the same list is also tracked in `is_plain_css_safe.dart`).
const CSS_MATH_FUNCTION_NAMES: &[&str] = &[
    "calc",
    "clamp",
    "min",
    "max",
    "round",
    "abs",
    "hypot",
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "sqrt",
    "exp",
    "sign",
    "mod",
    "rem",
    "atan2",
    "pow",
    "log",
    "calc-size",
];

/// Subset of [`CSS_MATH_FUNCTION_NAMES`] whose operands may form a
/// slash-separated number.
///
/// Matches Dart: `_operandAllowsSlash` (the same set, checked together with
/// `namespace == null` and absence of a user-defined shadowing function).
const CALC_FUNCTION_NAMES: &[&str] = &[
    "calc",
    "clamp",
    "hypot",
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "sqrt",
    "exp",
    "sign",
    "mod",
    "rem",
    "atan2",
    "pow",
    "log",
    "calc-size",
];

// ===========================================================================
// evaluate_expression — internal dispatch (free function → free function)
// ===========================================================================

/// Evaluates any SassScript [`Expression`] to a [`Value`].
///
/// Matches Dart: the `Expression.accept(this)` dispatch into the
/// `visit*Expression` family. Short-circuiting (`or`/`and`), slash-division,
/// and calculation-valued calls fan out to the helpers below; the whole binary
/// body runs under [`add_exception_span`] so division failures point at the
/// operation span.

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_expression<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &Expression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    match expr {
        Expression::Boolean(b) => evaluate_boolean_expression(config, state, arena, b),
        Expression::Null(n) => evaluate_null_expression(config, state, arena, n),
        Expression::Number(n) => evaluate_number_expression(config, state, arena, n),
        Expression::Color(c) => evaluate_color_expression(config, state, arena, c),
        Expression::Value(v) => evaluate_value_expression(config, state, v),
        Expression::Variable(v) => {
            box_rec_in!(evaluate_variable_expression(config, state, v), arena).await
        }
        Expression::String(s) => {
            box_rec_in!(evaluate_string_expression(config, state, arena, s), arena).await
        }
        Expression::List(l) => {
            box_rec_in!(evaluate_list_expression(config, state, arena, l), arena).await
        }
        Expression::Map(m) => {
            box_rec_in!(evaluate_map_expression(config, state, arena, m), arena).await
        }
        Expression::BinaryOperation(b) => {
            box_rec_in!(evaluate_binary_operation(config, state, arena, b), arena).await
        }
        Expression::UnaryOperation(u) => {
            box_rec_in!(evaluate_unary_operation(config, state, arena, u), arena).await
        }
        Expression::Function(f) => {
            box_rec_in!(evaluate_function_expression(config, state, arena, f), arena,).await
        }
        Expression::Parenthesized(p) => {
            box_rec_in!(
                evaluate_parenthesized_expression(config, state, arena, p),
                arena,
            )
            .await
        }
        Expression::Selector(s) => evaluate_selector_expression(config, state, arena, s),
        Expression::Supports(s) => {
            box_rec_in!(evaluate_supports_expression(config, state, arena, s), arena,).await
        }
        Expression::If(i) => {
            box_rec_in!(evaluate_if_expression(config, state, arena, i), arena).await
        }
        Expression::LegacyIf(l) => {
            box_rec_in!(
                evaluate_legacy_if_expression(config, state, arena, l),
                arena,
            )
            .await
        }
        Expression::InterpolatedFunction(ifn) => {
            box_rec_in!(
                evaluate_interpolated_function_expression(config, state, arena, ifn),
                arena,
            )
            .await
        }
    }
}

// ===========================================================================
// VisitValueExpression
// ===========================================================================

/// Returns the literal value unchanged.
///
/// Matches Dart: `visitValueExpression` (`=> node.value`).
pub(crate) fn evaluate_value_expression<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    expr: &ValueExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    Ok(*expr.value)
}

// ===========================================================================
// VisitVariableExpression
// ===========================================================================

/// Looks up a variable (with optional namespace) and reports
/// `"Undefined variable."` at the use-site span when missing.
///
/// Matches Dart: `visitVariableExpression`, including the `_addExceptionSpan`
/// wrapper around the environment lookup.

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_variable_expression<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    expr: &VariableExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let result = add_exception_span(config, state, expr.span, None, async |_config, state| {
        state
            .env
            .get_variable(&expr.name, expr.namespace.as_deref())
    })
    .await?;
    match result {
        Some(v) => Ok(v),
        None => Err(Box::new(exception(
            state,
            "Undefined variable.".into(),
            Some(expr.span),
        ))),
    }
}

// ===========================================================================
// VisitStringExpression
// ===========================================================================

/// Interpolates a string, clearing `in_supports_declaration` while doing so.
///
/// Matches Dart: `visitStringExpression`. Plain interpolations shortcut
/// without evaluating; embedded Sass values splice their raw text in, while
/// other values are serialized with `quote: false`. Deliberately not
/// [`perform_interpolation`]: the raw text (not the semantic value) is needed.

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_string_expression<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &StringExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let old_in_supports = state.in_supports_declaration;
    state.in_supports_declaration = false;

    let text = &expr.text;
    let result = if let Some(plain) = text.as_plain() {
        Ok(Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(plain), expr.has_quotes)),
        ))
    } else {
        let mut buf = String::new();
        for part in &text.contents {
            match part {
                InterpolationPart::Text(t) => {
                    buf.push_str(t);
                }
                InterpolationPart::Expression(e) => {
                    let val = evaluate_expression(config, state, arena, e).await?;
                    if let ValueKind::String(s) = &*val {
                        buf.push_str(s.text);
                    } else {
                        let css = serialize_value(config, state, &val, e.span()?, false).await?;
                        buf.push_str(&css);
                    }
                }
            }
        }
        Ok(Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(&buf), expr.has_quotes)),
        ))
    };

    state.in_supports_declaration = old_in_supports;
    result
}

// ===========================================================================
// VisitNumberExpression
// ===========================================================================

/// Wraps the literal value and unit in a [`SassNumber`].
///
/// Matches Dart: `visitNumberExpression` (`SassNumber(node.value, node.unit)`).
pub(crate) fn evaluate_number_expression<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &NumberExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let unit: Option<&str> = expr.unit.as_deref();
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Number(SassNumber::new(expr.value, unit)),
    ))
}

// ===========================================================================
// VisitBooleanExpression
// ===========================================================================

/// Wraps the literal in a [`SassBoolean`].
///
/// Matches Dart: `visitBooleanExpression` (`SassBoolean(node.value)`).
pub(crate) fn evaluate_boolean_expression<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &BooleanExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Boolean(if expr.value {
            SassBoolean::new(true)
        } else {
            SassBoolean::new(false)
        }),
    ))
}

// ===========================================================================
// VisitNullExpression
// ===========================================================================

/// Returns null.
///
/// Matches Dart: `visitNullExpression` (`=> sassNull`).
pub(crate) fn evaluate_null_expression<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    _expr: &NullExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    Ok(Value::new_with_arena(arena, ValueKind::Null))
}

// ===========================================================================
// VisitColorExpression
// ===========================================================================

/// Returns the literal color unchanged.
///
/// Matches Dart: `visitColorExpression` (`=> node.value`).
pub(crate) fn evaluate_color_expression<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &ColorExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Color((*expr.value).clone()),
    ))
}

// ===========================================================================
// VisitListExpression
// ===========================================================================

/// Evaluates each element in order, preserving separator and brackets.
///
/// Matches Dart: `visitListExpression` (maps `contents` through the visitor
/// into `SassList`).

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_list_expression<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &ListExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let mut contents = Vec::with_capacity(expr.contents.len());
    for element in &expr.contents {
        let val = evaluate_expression(config, state, arena, element).await?;
        contents.push(val);
    }
    let list = SassList::new(contents, expr.separator, expr.has_brackets);
    Ok(Value::new_with_arena(arena, ValueKind::List(list)))
}

// ===========================================================================
// VisitMapExpression
// ===========================================================================

/// Evaluates each key/value pair, rejecting duplicate keys with a `MultiSpan`
/// labeling the first and second occurrences.
///
/// Matches Dart: `visitMapExpression` (including the `keyNodes` table used to
/// locate the first key).

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_map_expression<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &MapExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let mut map = SassMap::empty();
    let mut key_spans: Vec<(Value<'parse>, FileSpan<'parse>)> = Vec::new();
    for (key_expr, val_expr) in &expr.pairs {
        let key = evaluate_expression(config, state, arena, key_expr).await?;
        let val = evaluate_expression(config, state, arena, val_expr).await?;
        if map.contains(&key) {
            let sp = key_expr.span()?;
            let old_span = key_spans.iter().find(|(k, _)| k == &key).map(|(_, s)| *s);
            return Err(Box::new(SassError::MultiSpan {
                message: "Duplicate key.".into(),
                span: file_span_to_ctx(&sp),
                primary_label: Some("second key".into()),
                secondary: old_span
                    .map(|s| (file_span_to_ctx(&s), "first key".into()))
                    .into_iter()
                    .collect(),
                original_source: None,
                cause: None,
                loaded_urls: vec![],
                trace: stack_trace(state, Some(sp)),
            }));
        }
        key_spans.push((key, key_expr.span()?));
        map.set(key, val);
    }
    Ok(Value::new_with_arena(arena, ValueKind::Map(map)))
}

// ===========================================================================
// VisitBinaryOperationExpression
// ===========================================================================

/// Evaluates a binary operation, short-circuiting `or`/`and` and routing `/`
/// through [`slash_divide`].
///
/// Matches Dart: `visitBinaryOperationExpression`. Only `=` and `/` are legal
/// in plain CSS; all errors are wrapped at the whole-operation span
/// (`_addExceptionSpan`).

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_binary_operation<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &BinaryOperationExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    // Go: evaluate_expression.go:188 — wraps entire body in addExceptionSpan,
    // catching SassScriptException from division (e.g. "function reference isn't
    // a valid CSS value") and wrapping it with the binary operation's span.
    let span = expr.span()?;

    // Go: evaluate_expression.go:176-186 — plain CSS operator check
    let is_plain_css = state.stylesheet.as_ref().is_some_and(|s| s.plain_css);
    if is_plain_css
        && expr.operator != BinaryOperator::SingleEquals
        && expr.operator != BinaryOperator::DividedBy
    {
        let op_span = expr.operator_span()?;
        return Err(Box::new(exception(
            state,
            "Operators aren't allowed in plain CSS.".into(),
            Some(op_span),
        )));
    }

    add_exception_span(config, state, span, None, async |cfg, st| {
        let left = evaluate_expression(cfg, st, arena, &expr.left).await?;

        match expr.operator {
            BinaryOperator::SingleEquals => {
                let right = evaluate_expression(cfg, st, arena, &expr.right).await?;
                left.single_equals(arena, &right)
            }
            BinaryOperator::Or => {
                if left.is_truthy() {
                    return Ok(left);
                }
                evaluate_expression(cfg, st, arena, &expr.right).await
            }
            BinaryOperator::And => {
                if !left.is_truthy() {
                    return Ok(left);
                }
                evaluate_expression(cfg, st, arena, &expr.right).await
            }
            BinaryOperator::DividedBy => {
                let right = evaluate_expression(cfg, st, arena, &expr.right).await?;
                slash_divide(cfg, st, arena, expr, left, right)
            }
            _ => {
                let right = evaluate_expression(cfg, st, arena, &expr.right).await?;
                evaluate_binary_op(arena, expr, left, right)
            }
        }
    })
    .await
}

/// Dispatches the non-special binary operators to the [`Value`] arithmetic.
///
/// The `SingleEquals`/`Or`/`And`/`DividedBy` arms are resolved by the caller;
/// this covers the remainder. Matches Dart: the `switch (node.operator)` in
/// `visitBinaryOperationExpression`.
fn evaluate_binary_op<'compile, 'parse>(
    arena: &'compile Bump,
    expr: &BinaryOperationExpression<'parse>,
    left: Value<'parse>,
    right: Value<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    match expr.operator {
        BinaryOperator::Plus => left.plus(arena, &right),
        BinaryOperator::Minus => left.minus(arena, &right),
        BinaryOperator::Times => left.times(arena, &right),
        BinaryOperator::DividedBy => {
            unreachable!("handled by slash_divide in evaluate_binary_operation")
        }
        BinaryOperator::Modulo => left.modulo(arena, &right),
        BinaryOperator::Equals => Ok(Value::new_with_arena(
            arena,
            ValueKind::Boolean(SassBoolean::new(left.equals(&right))),
        )),
        BinaryOperator::NotEquals => Ok(Value::new_with_arena(
            arena,
            ValueKind::Boolean(SassBoolean::new(!left.equals(&right))),
        )),
        BinaryOperator::GreaterThan => left.greater_than(arena, &right),
        BinaryOperator::GreaterThanOrEquals => left.greater_than_or_equals(arena, &right),
        BinaryOperator::LessThan => left.less_than(arena, &right),
        BinaryOperator::LessThanOrEquals => left.less_than_or_equals(arena, &right),
        // SingleEquals, Or, And are handled before calling evaluate_binary_op
        BinaryOperator::SingleEquals | BinaryOperator::Or | BinaryOperator::And => {
            unreachable!("{:?} handled by evaluate_binary_operation", expr.operator)
        }
    }
}

// ===========================================================================
// slash_divide — Go: slashDivide
// ===========================================================================

/// Returns the result of the SassScript `/` operation between `left` and
/// `right` in `expr`.
///
/// Matches Dart: `_slash`. Slash-separated numbers are preserved only when the
/// parse-time `allows_slash()` flag and both operands permit it (see
/// [`operand_allows_slash`]); a number `/` number otherwise warns
/// (`slash-div` deprecation with a `math.div(...)` / `expressionToCalc`
/// recommendation) and divides.
fn slash_divide<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &BinaryOperationExpression<'parse>,
    left: Value<'parse>,
    right: Value<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let result = left.divided_by(arena, &right)?;

    if let (ValueKind::Number(left_num), ValueKind::Number(right_num)) = (&*left, &*right) {
        if expr.allows_slash() {
            let left_allows_slash = operand_allows_slash(&expr.left, state)?;
            let right_allows_slash = operand_allows_slash(&expr.right, state)?;
            if left_allows_slash && right_allows_slash {
                if let ValueKind::Number(result_num) = &*result {
                    return Ok(Value::new_with_arena(
                        arena,
                        ValueKind::Number(
                            result_num.with_slash(left_num.clone(), right_num.clone()),
                        ),
                    ));
                }
            }
        }

        let span = expr.span()?;
        let rec_str =
            slash_division_recommendation_expr(&Expression::BinaryOperation(expr.clone()))?;
        let calc_expr = expression_to_calc(&Expression::BinaryOperation(expr.clone()))?;
        let calc_str = calc_expr.to_display_string()?;
        let message = format!(
            "Using / for division outside of calc() is deprecated \
             and will be removed in Dart Sass 2.0.0.\n\n\
             Recommendation: {} or {}\n\n\
             More info and automated migrator: https://sass-lang.com/d/slash-div",
            rec_str, calc_str
        );
        warn_deprecation_span(config, state, &message, &SLASH_DIV, span)?;
    }

    Ok(result)
}

// ===========================================================================
// operand_allows_slash — Go: operandAllowsSlash
// ===========================================================================

/// Returns whether `expr` can be a component of a slash-separated number.
///
/// Matches Dart: `_operandAllowsSlash`. Only unnamespaced calculation-function
/// calls (from [`CALC_FUNCTION_NAMES`]) that are *not* shadowed by a
/// user-defined function are excluded — everything else allows `/` — because
/// parse time cannot know whether operands will evaluate as calculations.
fn operand_allows_slash<'compile, 'parse>(
    expr: &Expression<'_>,
    state: &EvalState<'compile, 'parse>,
) -> SassResult<bool>
where
    'compile: 'parse,
{
    match expr {
        Expression::Function(fe) => {
            if fe.namespace.is_some() {
                return Ok(false);
            }
            if !CALC_FUNCTION_NAMES
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&fe.name))
            {
                return Ok(false);
            }
            let declared = state.env.get_function(&fe.name, None)?;
            Ok(declared.is_none())
        }
        _ => Ok(true),
    }
}

// ===========================================================================
// slash_division_recommendation_expr — Go: recommendation closure in slashDivide
// ===========================================================================

/// Renders the `math.div(...)` recommendation for a `/`-as-division warning.
///
/// Matches Dart: the `recommendation` closure inside `_slash`, which recurses
/// through nested `/` operations and prints parenthesized operands via their
/// inner expression.
fn slash_division_recommendation_expr(expr: &Expression<'_>) -> SassResult<String> {
    match expr {
        Expression::BinaryOperation(be) if be.operator == BinaryOperator::DividedBy => {
            let left = slash_division_recommendation_expr(&be.left)?;
            let right = slash_division_recommendation_expr(&be.right)?;
            Ok(format!("math.div({}, {})", left, right))
        }
        Expression::Parenthesized(pe) => pe.expression.to_display_string(),
        _ => expr.to_display_string(),
    }
}

// ===========================================================================
// VisitUnaryOperationExpression
// ===========================================================================

/// Evaluates the operand, then applies the unary operator.
///
/// Matches Dart: `visitUnaryOperationExpression`. Script failures are wrapped
/// at the whole-operation span (`_addExceptionSpan`).

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_unary_operation<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &UnaryOperationExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let operand = evaluate_expression(config, state, arena, &expr.operand).await?;
    let result = match expr.operator {
        UnaryOperator::Not => Ok(Value::new_with_arena(
            arena,
            ValueKind::Boolean(SassBoolean::new(!operand.is_truthy())),
        )),
        UnaryOperator::Plus => operand.unary_plus(arena),
        UnaryOperator::Minus => operand.unary_minus(arena),
        UnaryOperator::Divide => operand.unary_divide(arena),
    };
    match result {
        Err(e) if matches!(*e, SassError::Script { .. }) => {
            let SassError::Script { message, .. } = *e else {
                unreachable!()
            };
            let span = expr.span()?;
            Err(Box::new(exception(state, message, Some(span))))
        }
        other => other,
    }
}

// ===========================================================================
// VisitFunctionExpression (stub — needs full port)
// ===========================================================================

/// Resolves and invokes a function call, routing calculation-safe math
/// functions to [`evaluate_css_math_function`] and unknown names to a
/// [`PlainCssCallable`].
///
/// Matches Dart: `visitFunctionExpression`. Resolution order — environment
/// lookup (skipped in plain CSS), `originalName` `--` guard, namespaced calls
/// erroring rather than falling through, `calc()`/`clamp()`/… (with the
/// `min`/`max`/`round`/`abs` `isCalculationSafe` gate passing the legacy
/// function name), built-ins, globals, then plain CSS — plus the `inFunction`
/// flag and `_addErrorSpan` around invocation.

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_function_expression<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &FunctionExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    // Go: evaluate_expression.go:437 — if !v.stylesheet.IsPlainCss() { env lookup }
    // In plain CSS mode, skip the environment function lookup entirely.
    let is_plain_css = state.stylesheet.as_ref().is_some_and(|s| s.plain_css);
    let fn_from_env = if !is_plain_css {
        let span = expr.span()?;
        add_exception_span(config, state, span, None, async |_cfg, st| {
            st.env.get_function(&expr.name, expr.namespace.as_deref())
        })
        .await?
    } else {
        None
    };
    let fn_callable = match fn_from_env {
        Some(c) if !expr.original_name.starts_with("--") => c,
        _ => {
            // Go: if expr.Namespace != nil { return error "Undefined function." }
            // Namespaced calls must NOT fall through to built-in/global functions.
            if expr.namespace.is_some() {
                let span = expr.span()?;
                return Err(Box::new(exception(
                    state,
                    "Undefined function.".into(),
                    Some(span),
                )));
            }
            // Go: if isCssMathFunction(expr.Name) { ... } — check CSS math
            // functions BEFORE falling through to built-ins, so that e.g.
            // min(1%, 2px) produces a Calculation instead of invoking the
            // Sass built-in (which would reject incompatible units).
            if CSS_MATH_FUNCTION_NAMES
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&expr.name))
            {
                let lower = expr.name.to_lowercase();
                if lower == "min" || lower == "max" || lower == "round" || lower == "abs" {
                    if expr.arguments.named.is_empty()
                        && expr.arguments.rest.is_none()
                        && expr
                            .arguments
                            .positional
                            .iter()
                            .all(|a| a.is_calculation_safe().unwrap_or(false))
                    {
                        return evaluate_css_math_function(
                            config,
                            state,
                            arena,
                            expr,
                            Some(&lower),
                        )
                        .await;
                    }
                    // Fall through to built-in functions if args are not
                    // all calculation-safe.
                } else {
                    return evaluate_css_math_function(config, state, arena, expr, None).await;
                }
            }
            // Go: evaluate_expression.go:471 — if !v.stylesheet.IsPlainCss() { check built-ins }
            // In plain CSS mode, always use PlainCssCallable directly.
            if is_plain_css {
                Callable::new(
                    arena,
                    CallableKind::PlainCss(PlainCssCallable {
                        name: expr.original_name.clone(),
                    }),
                )
            } else {
                let bf = config.built_in_functions.borrow();
                if let Some(c) = bf.get(&expr.name) {
                    *c
                } else {
                    // Check global functions
                    let gf = config.global_functions.borrow();
                    gf.iter()
                        .find(|c| c.name() == expr.name)
                        .cloned()
                        .unwrap_or_else(|| {
                            Callable::new(
                                arena,
                                CallableKind::PlainCss(PlainCssCallable {
                                    name: expr.original_name.clone(),
                                }),
                            )
                        })
                }
            }
        }
    };

    // Check: mixin used as function
    if let CallableKind::UserDefined(u) = fn_callable.kind() {
        if matches!(u.declaration, CallableDeclaration::Mixin(_)) {
            let span = expr.span()?;
            let trace = stack_trace(state, Some(span));
            return Err(Box::new(SassError::Runtime {
                message: "Mixin used as function.".into(),
                span: file_span_to_ctx(&span),
                trace,
                cause: None,
                loaded_urls: vec![],
            }));
        }
    }

    // If the function is a CSS math function that wasn't user-defined,
    // evaluate it through calc-specific evaluation (matching Go's
    // visitCssMathFunction/visitCalculation in evaluate_expression.go).
    if expr.namespace.is_none()
        && matches!(fn_callable.kind(), CallableKind::PlainCss(_))
        && CSS_MATH_FUNCTION_NAMES
            .iter()
            .any(|n| n.eq_ignore_ascii_case(&expr.name))
    {
        let lower = expr.name.to_lowercase();
        if lower == "min" || lower == "max" || lower == "round" || lower == "abs" {
            if expr.arguments.named.is_empty()
                && expr.arguments.rest.is_none()
                && expr
                    .arguments
                    .positional
                    .iter()
                    .all(|a| a.is_calculation_safe().unwrap_or(false))
            {
                return evaluate_css_math_function(config, state, arena, expr, Some(&lower)).await;
            }
            // Fall through to invoke_callable if args are not all
            // calculation-safe.
        } else {
            return evaluate_css_math_function(config, state, arena, expr, None).await;
        }
    }

    let old_in_function = state.in_function;
    state.in_function = true;

    let expr_span = expr.span()?;
    let result = add_error_span(config, state, expr_span, Some(expr_span), async |c, s| {
        invoke_callable(c, s, arena, &fn_callable, &expr.arguments, expr_span).await
    })
    .await;

    state.in_function = old_in_function;
    result
}

// ===========================================================================
// VisitParenthesizedExpression
// Go: evaluate_expression.go:1371
// ===========================================================================

/// Evaluates the inner expression; parentheses are illegal in plain CSS.
///
/// Matches Dart: `visitParenthesizedExpression` (throws at the node span when
/// `_stylesheet.plainCss`).

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_parenthesized_expression<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &ParenthesizedExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let is_plain_css = state.stylesheet.as_ref().is_some_and(|s| s.plain_css);
    if is_plain_css {
        let span = expr.span()?;
        return Err(Box::new(exception(
            state,
            "Parentheses aren't allowed in plain CSS.".into(),
            Some(span),
        )));
    }
    evaluate_expression(config, state, arena, &expr.expression).await
}

// ===========================================================================
// VisitSelectorExpression
// Go: evaluate_expression.go:1382
// ===========================================================================

/// Returns the enclosing style rule's original selector as a list, or null
/// outside a style rule.
///
/// Matches Dart: `visitSelectorExpression`
/// (`_styleRuleIgnoringAtRoot?.originalSelector.asSassList ?? sassNull`).
pub(crate) fn evaluate_selector_expression<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    state: &EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    _expr: &SelectorExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    if let Some(ref sr) = state.style_rule_ignoring_at_root {
        sr.original_selector.as_sass_list(arena)
    } else {
        Ok(Value::new_with_arena(arena, ValueKind::Null))
    }
}

// ===========================================================================
// VisitSupportsExpression (stub — complex, needs supports condition evaluation)
// Go: evaluate_expression.go:1510
// ===========================================================================

/// Serializes a `@supports` condition to an unquoted string.
///
/// Matches Dart: `visitSupportsExpression`
/// (`SassString(_visitSupportsCondition(condition), quotes: false)`).

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_supports_expression<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &SupportsExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let text = visit_supports_condition(config, state, arena, &expr.condition).await?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(&text), false)),
    ))
}

/// Parenthesizes a `@supports` condition unless it shares the surrounding
/// operator.
///
/// Matches Dart: the `_parenthesize` closure in `_visitSupportsCondition`.
/// Negations always parenthesize; operations parenthesize only when the
/// operator differs (or no outer operator is given).
#[rust_sass_macros::maybe_async]
async fn parenthesize_supports_condition<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    condition: &SupportsCondition<'parse>,
    operator: Option<BooleanOperator>,
) -> SassResult<String>
where
    'compile: 'parse,
{
    match condition {
        SupportsCondition::Negation(_) => {
            let inner = visit_supports_condition(config, state, arena, condition).await?;
            Ok(format!("({inner})"))
        }
        SupportsCondition::Operation(op) if operator != Some(op.operator) || operator.is_none() => {
            let inner = visit_supports_condition(config, state, arena, condition).await?;
            Ok(format!("({inner})"))
        }
        _ => visit_supports_condition(config, state, arena, condition).await,
    }
}

/// Serializes a `@supports` condition to CSS text.
///
/// Matches Dart: `_visitSupportsCondition`. Interpolations evaluate then
/// serialize (`_evaluateToCss`); declarations serialize name and value with
/// the `inSupportsDeclaration` flag set and join custom properties without a
/// space; functions/anything blocks interpolate their parts.
#[rust_sass_macros::maybe_async]
pub(crate) async fn visit_supports_condition<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    condition: &SupportsCondition<'parse>,
) -> SassResult<String>
where
    'compile: 'parse,
{
    match condition {
        SupportsCondition::Interpolation(interp) => {
            let val = evaluate_expression(config, state, arena, &interp.expression).await?;
            serialize_value(config, state, &val, interp.expression.span()?, false).await
        }
        SupportsCondition::Declaration(decl) => {
            let old_in_supports = state.in_supports_declaration;
            state.in_supports_declaration = true;
            let name_val = evaluate_expression(config, state, arena, &decl.name).await?;
            let name = serialize_value(config, state, &name_val, decl.name.span()?, true).await?;
            let val_val = evaluate_expression(config, state, arena, &decl.value).await?;
            let val = serialize_value(config, state, &val_val, decl.value.span()?, true).await?;
            state.in_supports_declaration = old_in_supports;
            let is_custom_prop = name.starts_with("--");
            let sep = if is_custom_prop { "" } else { " " };
            Ok(format!("({name}:{sep}{val})"))
        }
        SupportsCondition::Negation(neg) => {
            let inner = box_rec_in!(
                parenthesize_supports_condition(config, state, arena, &neg.condition, None),
                arena,
            )
            .await?;
            Ok(format!("not {inner}"))
        }
        SupportsCondition::Operation(op) => {
            let left = box_rec_in!(
                parenthesize_supports_condition(config, state, arena, &op.left, Some(op.operator)),
                arena,
            )
            .await?;
            let right = box_rec_in!(
                parenthesize_supports_condition(config, state, arena, &op.right, Some(op.operator)),
                arena,
            )
            .await?;
            Ok(format!("{left} {} {right}", op.operator))
        }
        SupportsCondition::Function(func) => {
            let name = perform_interpolation(config, state, arena, &func.name, false).await?;
            let args = perform_interpolation(config, state, arena, &func.arguments, false).await?;
            Ok(format!("{name}({args})"))
        }
        SupportsCondition::Anything(anything) => {
            let contents =
                perform_interpolation(config, state, arena, &anything.contents, false).await?;
            Ok(format!("({contents})"))
        }
    }
}

// ===========================================================================
// VisitIfExpression
// Go: evaluate_expression.go:1159
// ===========================================================================

/// The resolved form of one `if()` condition: a Sass boolean or raw CSS text.
///
/// Matches Dart: the `Object /* String | bool */` returned by the
/// `visitIfCondition*` family.
enum IfCondResult {
    Bool(bool),
    Css(String),
}

struct IfBranchResult<'parse> {
    condition_text: String,
    expr_val: Value<'parse>,
}

/// Evaluates the modern `if()` expression, returning Sass values directly or
/// an unresolved `if(cond: val; …)` string.
///
/// Matches Dart: `visitIfExpression`. A `true` condition with no CSS seen so
/// far returns its branch immediately (raw — no `_withoutSlash`, per
/// `ref/eval.md`); later `true` branches and CSS conditions accumulate into
/// `results`, which render as `if(...)`; no branch taken yields null.

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_if_expression<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &IfExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let mut results: Vec<IfBranchResult<'_>> = Vec::new();

    for branch in &expr.branches {
        let cond_result = match &branch.condition {
            Some(condition) => evaluate_if_condition(config, state, arena, condition).await?,
            None => IfCondResult::Bool(true),
        };

        match cond_result {
            IfCondResult::Css(condition_text) => {
                let expr_val =
                    evaluate_expression(config, state, arena, &branch.expression).await?;
                results.push(IfBranchResult {
                    condition_text,
                    expr_val,
                });
            }
            // Dart `visitIfExpression` returns branch values raw, without
            // `_withoutSlash`.
            IfCondResult::Bool(true) if results.is_empty() => {
                let result = evaluate_expression(config, state, arena, &branch.expression).await?;
                return Ok(result);
            }
            IfCondResult::Bool(true) => {
                let expr_val =
                    evaluate_expression(config, state, arena, &branch.expression).await?;
                results.push(IfBranchResult {
                    condition_text: "else".into(),
                    expr_val,
                });
            }
            IfCondResult::Bool(false) => {
                // skip — continue to next branch
            }
        }
    }

    if results.is_empty() {
        return Ok(Value::new_with_arena(arena, ValueKind::Null));
    }

    let mut parts: Vec<String> = Vec::new();
    for r in &results {
        // Dart `visitIfExpression` renders branch values with `toCssString`
        // (CSS form, not inspect form — #2808).
        let val_str = r.expr_val.to_css_string(true)?;
        parts.push(format!("{}: {}", r.condition_text, val_str));
    }
    let text = format!("if({})", parts.join("; "));
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(&text), false)),
    ))
}

/// Evaluates one `if()` condition to a boolean or CSS text.
///
/// Matches Dart: the `visitIfCondition*` family. Sass conditions resolve via
/// truthiness; negations/parenthesized wrap CSS (`not …`, `(…)`); operations
/// short-circuit booleans (`and` fails on `false`, `or` succeeds on `true`),
/// join surviving CSS with the operator, and strip parens from a lone
/// parenthesized survivor; functions/raw blocks interpolate.
#[rust_sass_macros::maybe_async]
async fn evaluate_if_condition<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    condition: &IfConditionExpression<'parse>,
) -> SassResult<IfCondResult>
where
    'compile: 'parse,
{
    match condition {
        IfConditionExpression::Sass(sass) => {
            let val = evaluate_expression(config, state, arena, &sass.expression).await?;
            Ok(IfCondResult::Bool(val.is_truthy()))
        }
        IfConditionExpression::Negation(neg) => {
            let inner = box_rec_in!(
                evaluate_if_condition(config, state, arena, &neg.expression),
                arena,
            )
            .await?;
            match inner {
                IfCondResult::Css(s) => Ok(IfCondResult::Css(format!("not {s}"))),
                IfCondResult::Bool(b) => Ok(IfCondResult::Bool(!b)),
            }
        }
        IfConditionExpression::Parenthesized(p) => {
            let inner = box_rec_in!(
                evaluate_if_condition(config, state, arena, &p.expression),
                arena,
            )
            .await?;
            match inner {
                IfCondResult::Css(s) => Ok(IfCondResult::Css(format!("({s})"))),
                IfCondResult::Bool(b) => Ok(IfCondResult::Bool(b)),
            }
        }
        IfConditionExpression::Operation(op) => {
            struct CssEntry {
                was_parenthesized: bool,
                text: String,
            }
            let mut css_parts: Vec<CssEntry> = Vec::new();
            for (i, expr) in op.expressions.iter().enumerate() {
                let result =
                    box_rec_in!(evaluate_if_condition(config, state, arena, expr), arena,).await?;
                match result {
                    IfCondResult::Css(s) => {
                        let was_parenthesized =
                            matches!(op.expressions[i], IfConditionExpression::Parenthesized(_));
                        css_parts.push(CssEntry {
                            was_parenthesized,
                            text: s,
                        });
                    }
                    IfCondResult::Bool(b) => {
                        if op.operator == BooleanOperator::And {
                            if !b {
                                return Ok(IfCondResult::Bool(false));
                            }
                        } else {
                            // Or
                            if b {
                                return Ok(IfCondResult::Bool(true));
                            }
                        }
                    }
                }
            }
            if css_parts.is_empty() {
                return Ok(IfCondResult::Bool(op.operator == BooleanOperator::And));
            }
            // If the only CSS node left is parenthesized, remove parens
            if css_parts.len() == 1 {
                let entry = css_parts.swap_remove(0);
                if entry.was_parenthesized && entry.text.len() >= 2 {
                    return Ok(IfCondResult::Css(
                        entry.text[1..entry.text.len() - 1].to_string(),
                    ));
                }
                return Ok(IfCondResult::Css(entry.text));
            }
            let op_str = if op.operator == BooleanOperator::And {
                " and "
            } else {
                " or "
            };
            let joined: String = css_parts
                .into_iter()
                .map(|e| e.text)
                .collect::<Vec<_>>()
                .join(op_str);
            Ok(IfCondResult::Css(joined))
        }
        IfConditionExpression::Function(f) => {
            let name = perform_interpolation(config, state, arena, &f.name, false).await?;
            let args = perform_interpolation(config, state, arena, &f.arguments, false).await?;
            Ok(IfCondResult::Css(format!("{name}({args})")))
        }
        IfConditionExpression::Raw(r) => {
            let text = perform_interpolation(config, state, arena, &r.text, false).await?;
            Ok(IfCondResult::Css(text))
        }
    }
}

// ===========================================================================
// VisitLegacyIfExpression
// Go: evaluate_expression.go:1308
// Dart: evaluate.dart:2983 — _evaluateMacroArguments then _verifyArguments
// ===========================================================================

/// Evaluates the legacy global `if($condition, $if-true, $if-false)` call.
///
/// Matches Dart: `visitLegacyIfExpression`. Macro-style arguments are
/// evaluated unevaluated, verified against the `$condition, $if-true,
/// $if-false` parameter list, then only the chosen branch is evaluated and
/// passed through [`without_slash`].

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_legacy_if_expression<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &LegacyIfExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let span = expr.span;

    let (positional, named) =
        evaluate_macro_arguments(config, state, arena, &expr.arguments, span).await?;

    let named_keys: HashSet<String> = named.keys().cloned().collect();
    let params =
        parse_parameter_list("@function if($condition, $if-true, $if-false) {", "", arena)?;
    add_exception_span(config, state, span, Some(true), async |_, _| {
        verify_parameter_list(positional.len(), &named_keys, Some(&params), span)
    })
    .await?;

    let condition_expr = if !positional.is_empty() {
        positional[0].clone()
    } else if let Some(e) = named.get("condition") {
        e.clone()
    } else {
        return Err(Box::new(SassError::Script {
            message: "Missing argument $condition.".into(),
            argument_name: None,
        }));
    };

    let if_true_expr = if positional.len() > 1 {
        positional[1].clone()
    } else if let Some(e) = named.get("if-true") {
        e.clone()
    } else {
        return Err(Box::new(SassError::Script {
            message: "Missing argument $if-true.".into(),
            argument_name: None,
        }));
    };

    let if_false_expr = if positional.len() > 2 {
        Some(positional[2].clone())
    } else {
        named.get("if-false").cloned()
    };

    let condition_val = evaluate_expression(config, state, arena, &condition_expr).await?;

    let chosen = if condition_val.is_truthy() {
        if_true_expr
    } else if let Some(if_false) = if_false_expr {
        if_false
    } else {
        return Ok(Value::new_with_arena(arena, ValueKind::Null));
    };

    let result = evaluate_expression(config, state, arena, &chosen).await?;
    let chosen_span = chosen.span()?;
    without_slash(config, state, arena, result, chosen_span)
}

// ===========================================================================
// VisitInterpolatedFunctionExpression (stub — needs invokeCallable)
// Go: evaluate_expression.go:1390
// ===========================================================================

/// Invokes a call whose name itself needed interpolation as plain CSS.
///
/// Matches Dart: `visitInterpolatedFunctionExpression` (wraps the interpolated
/// name in `PlainCssCallable`, sets `inFunction`, evaluates under
/// `_addErrorSpan`).

#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_interpolated_function_expression<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &InterpolatedFunctionExpression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let name = perform_interpolation(config, state, arena, &expr.name, false).await?;
    let fn_callable = Callable::new(arena, CallableKind::PlainCss(PlainCssCallable { name }));

    let old_in_function = state.in_function;
    state.in_function = true;

    let expr_span = expr.span()?;
    let result = add_error_span(config, state, expr_span, Some(expr_span), async |c, s| {
        invoke_callable(c, s, arena, &fn_callable, &expr.arguments, expr_span).await
    })
    .await;

    state.in_function = old_in_function;
    result
}

// ===========================================================================
// invokeCallable — dispatches function invocation to callable type
// Go: evaluate_expression.go:1519
// ===========================================================================

/// Evaluates `arguments` as applied to `fn_callable`, tracking the call-site
/// span for error traces.
///
/// Matches Dart: `_runFunctionCallable`'s contract (built-in/user-defined/
/// plain-CSS dispatch lives in [`invoke_callable_inner`]); the
/// save/restore of the callable span mirrors Dart's `_callableNode`
/// swap in `_runBuiltInCallable`.

#[rust_sass_macros::maybe_async]
pub(crate) async fn invoke_callable<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    fn_callable: &Callable<'compile, 'parse>,
    arguments: &ArgumentList<'parse>,
    span: FileSpan<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let old_callable_span = state.callable_span;
    state.callable_span = Some(span);
    let result = invoke_callable_inner(config, state, arena, fn_callable, arguments, span).await;
    state.callable_span = old_callable_span;
    result
}

/// Dispatches to built-in (overload resolution, default/`$kwargs` handling,
/// unused-keyword `MultiSpan`), user-defined, or plain-CSS invocation.
///
/// Matches Dart: `_runFunctionCallable` + `_runBuiltInCallable`. Built-in
/// results pass through [`without_slash`]; plain-CSS calls reject keyword
/// arguments and reframe trailing `"isn't a valid CSS value."` failures as a
/// `MultiSpan` pointing at the unknown function.
#[rust_sass_macros::maybe_async]
async fn invoke_callable_inner<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    fn_callable: &Callable<'compile, 'parse>,
    arguments: &ArgumentList<'parse>,
    span: FileSpan<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    match fn_callable.kind() {
        CallableKind::BuiltIn(b) => {
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

            // Go: evaluate_statement.go:1600-1633 — invoke callback + wrap errors
            #[cfg(feature = "async")]
            let result = match &overload.callback {
                BuiltInCallback::Sync(cb) => cb(config, state, padded, arena),
                BuiltInCallback::Async(cb) => cb(config, state, padded, arena).await,
            };
            #[cfg(not(feature = "async"))]
            let result = (overload.callback)(config, state, padded, arena);
            let result = result.map_err(|e| match *e {
                SassError::Script { .. } => {
                    let message = e.full_message();
                    Box::new(exception(state, message, Some(span)))
                }
                SassError::MultiSpanScript { .. } => {
                    let trace = stack_trace(state, Some(span));
                    Box::new(e.with_member_use_span(file_span_to_ctx(&span), trace))
                }
                other => Box::new(other),
            })?;

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

            let cleaned = without_slash(config, state, arena, result, span)?;
            Ok(cleaned)
        }
        CallableKind::UserDefined(u) => {
            run_user_defined_callable(config, state, arena, u, arguments, None, false, span).await
        }
        CallableKind::PlainCss(p) => {
            if !arguments.named.is_empty() || arguments.keyword_rest.is_some() {
                return Err(Box::new(exception(
                    state,
                    "Plain CSS functions don't support keyword arguments.".into(),
                    Some(span),
                )));
            }
            let result = serialize_plain_css_call(config, state, arena, &p.name, arguments).await;

            match result {
                Ok(result) => Ok(Value::new_with_arena(
                    arena,
                    ValueKind::String(SassString::new(arena.alloc_str(&result), false)),
                )),
                Err(e)
                    if matches!(*e, SassError::Runtime { ref message, .. }
                        if message.ends_with("isn't a valid CSS value.")) =>
                {
                    let SassError::Runtime {
                        message,
                        span: err_span,
                        trace,
                        cause,
                        loaded_urls,
                    } = *e
                    else {
                        unreachable!()
                    };
                    Err(Box::new(SassError::MultiSpan {
                        message,
                        span: err_span,
                        primary_label: Some("value".into()),
                        secondary: vec![(
                            file_span_to_ctx(&span),
                            "unknown function treated as plain CSS".into(),
                        )],
                        original_source: None,
                        cause,
                        loaded_urls,
                        trace,
                    }))
                }
                Err(e) => Err(e),
            }
        }
    }
}

/// Serializes an unknown-function call as plain CSS (`foo(a, b)`), evaluating
/// and serializing each argument. Extracted into its own `#[maybe_async]` fn
/// (rather than an inline async block) because it uses `?`: inside an async
/// block `?` returns from the block, but the sync conversion turns the block
/// into a plain one where `?` would escape the enclosing function.
#[rust_sass_macros::maybe_async]
async fn serialize_plain_css_call<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    name: &str,
    arguments: &ArgumentList<'parse>,
) -> SassResult<String>
where
    'compile: 'parse,
{
    let mut result = String::new();
    result.push_str(name);
    result.push('(');
    let mut first = true;
    for expr in &arguments.positional {
        if !first {
            result.push_str(", ");
        }
        first = false;
        let val = evaluate_expression(config, state, arena, expr).await?;
        let css = serialize_value(config, state, &val, expr.span()?, true).await?;
        result.push_str(&css);
    }
    if let Some(ref rest) = arguments.rest {
        let rest_val = evaluate_expression(config, state, arena, rest).await?;
        if !first {
            result.push_str(", ");
        }
        let css = serialize_value(config, state, &rest_val, rest.span()?, true).await?;
        result.push_str(&css);
    }
    result.push(')');
    Ok(result)
}

// ===========================================================================
// TESTS
// ===========================================================================

#[cfg(test)]
mod tests {
    use crate::compile::compile_string;
    use crate::compile::CompileOptions;
    use crate::io::Io;
    use crate::io::VirtualIo;
    use crate::logger::test_utils::RecordLogger;
    use std::rc::Rc;

    use super::*;
    use crate::ast::sass::expression_list::ListExpression;
    use crate::ast::sass::expression_map::MapExpression;
    use crate::ast::sass::expression_string::StringExpression;
    use crate::ast::sass::interpolation::Interpolation;

    use crate::common::source_span_file_source::FileSource;
    use crate::common::span::Span;
    use crate::compile_context::new_compile_context;
    use crate::logger::{Logger, QuietLogger};
    use bumpalo::Bump;

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
        let mut state = EvalState::new(arena, span);
        state.member = "root stylesheet".to_string();
        (
            EvalConfig::new(arena, log, new_compile_context(), Rc::new(VirtualIo::new())),
            state,
        )
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_null() {
        let arena = Bump::new();
        let span = make_span(&arena, "null");
        let (config, mut state) = test_visitor(&arena);
        let expr = NullExpression { span };
        let result = evaluate_null_expression(&config, &mut state, &arena, &expr).unwrap();
        assert!(matches!(*result, ValueKind::Null));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_boolean_true() {
        let arena = Bump::new();
        let span = make_span(&arena, "true");
        let (config, mut state) = test_visitor(&arena);
        let expr = BooleanExpression { value: true, span };
        let result = evaluate_boolean_expression(&config, &mut state, &arena, &expr).unwrap();
        assert!(result.is_truthy());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_boolean_false() {
        let arena = Bump::new();
        let span = make_span(&arena, "false");
        let (config, mut state) = test_visitor(&arena);
        let expr = BooleanExpression { value: false, span };
        let result = evaluate_boolean_expression(&config, &mut state, &arena, &expr).unwrap();
        assert!(!result.is_truthy());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_number() {
        let arena = Bump::new();
        let span = make_span(&arena, "42px");
        let (config, mut state) = test_visitor(&arena);
        let expr = NumberExpression {
            value: 42.0,
            unit: Some("px".into()),
            span,
        };
        let result = evaluate_number_expression(&config, &mut state, &arena, &expr).unwrap();
        match &*result {
            ValueKind::Number(n) => assert!((n.value - 42.0).abs() < 0.001),
            other => panic!("expected Number, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_value() {
        let arena = Bump::new();
        let span = make_span(&arena, "null");
        let (config, mut state) = test_visitor(&arena);
        let expr = ValueExpression {
            value: Box::new(Value::new_with_arena(&arena, ValueKind::Null)),
            span,
        };
        let result = evaluate_value_expression(&config, &mut state, &expr).unwrap();
        assert!(matches!(*result, ValueKind::Null));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_variable_undefined() {
        let arena = Bump::new();
        let span = make_span(&arena, "$x");
        let (config, mut state) = test_visitor(&arena);
        let expr = VariableExpression {
            name: "x".into(),
            span,
            namespace: None,
        };
        let err = evaluate_variable_expression(&config, &mut state, &expr)
            .await
            .unwrap_err();
        assert!(err.message().contains("Undefined variable."));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_variable_defined() {
        let arena = Bump::new();
        let span = make_span(&arena, "$y");
        let (config, mut state) = test_visitor(&arena);
        state
            .env
            .set_local_variable("y", Value::new_with_arena(&arena, ValueKind::Null), span);
        let expr = VariableExpression {
            name: "y".into(),
            span,
            namespace: None,
        };
        let result = evaluate_variable_expression(&config, &mut state, &expr)
            .await
            .unwrap();
        assert!(matches!(*result, ValueKind::Null));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_string_plain() {
        let arena = Bump::new();
        let span = make_span(&arena, "hello");
        let (config, mut state) = test_visitor(&arena);
        let interp = Interpolation::plain("hello".into(), Span::File(span));
        let expr = StringExpression {
            text: interp,
            has_quotes: false,
        };
        let result = evaluate_string_expression(&config, &mut state, &arena, &expr)
            .await
            .unwrap();
        match &*result {
            ValueKind::String(s) => assert_eq!(s.text, "hello"),
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_list_empty() {
        let arena = Bump::new();
        let span = make_span(&arena, "()");
        let (config, mut state) = test_visitor(&arena);
        let expr = ListExpression {
            contents: vec![],
            separator: ListSeparator::Comma,
            has_brackets: true,
            span,
        };
        let result = evaluate_list_expression(&config, &mut state, &arena, &expr)
            .await
            .unwrap();
        match &*result {
            ValueKind::List(l) => assert_eq!(l.length_as_list(), 0),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_map_empty() {
        let arena = Bump::new();
        let span = make_span(&arena, "()");
        let (config, mut state) = test_visitor(&arena);
        let expr = MapExpression {
            pairs: vec![],
            span,
        };
        let result = evaluate_map_expression(&config, &mut state, &arena, &expr)
            .await
            .unwrap();
        match &*result {
            ValueKind::Map(m) => assert!(m.is_empty()),
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_unary_not() {
        let arena = Bump::new();
        let span = make_span(&arena, "not true");
        let (config, mut state) = test_visitor(&arena);
        let expr = UnaryOperationExpression {
            operator: UnaryOperator::Not,
            operand: Box::new(Expression::Boolean(BooleanExpression { value: true, span })),
            span,
        };
        let result = evaluate_unary_operation(&config, &mut state, &arena, &expr)
            .await
            .unwrap();
        assert!(!result.is_truthy());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_binary_or_short_circuit() {
        let arena = Bump::new();
        let span = make_span(&arena, "true or ...");
        let (config, mut state) = test_visitor(&arena);
        let expr = BinaryOperationExpression {
            operator: BinaryOperator::Or,
            left: Box::new(Expression::Boolean(BooleanExpression { value: true, span })),
            right: Box::new(Expression::Null(NullExpression { span })),
            allows_slash: false,
        };
        let result = evaluate_binary_operation(&config, &mut state, &arena, &expr)
            .await
            .unwrap();
        assert!(result.is_truthy());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_binary_and_short_circuit() {
        let arena = Bump::new();
        let span = make_span(&arena, "false and ...");
        let (config, mut state) = test_visitor(&arena);
        let expr = BinaryOperationExpression {
            operator: BinaryOperator::And,
            left: Box::new(Expression::Boolean(BooleanExpression {
                value: false,
                span,
            })),
            right: Box::new(Expression::Null(NullExpression { span })),
            allows_slash: false,
        };
        let result = evaluate_binary_operation(&config, &mut state, &arena, &expr)
            .await
            .unwrap();
        assert!(!result.is_truthy());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_binary_plus() {
        let arena = Bump::new();
        let span = make_span(&arena, "1 + 2");
        let (config, mut state) = test_visitor(&arena);
        let expr = BinaryOperationExpression {
            operator: BinaryOperator::Plus,
            left: Box::new(Expression::Number(NumberExpression {
                value: 1.0,
                unit: None,
                span,
            })),
            right: Box::new(Expression::Number(NumberExpression {
                value: 2.0,
                unit: None,
                span,
            })),
            allows_slash: false,
        };
        let result = evaluate_binary_operation(&config, &mut state, &arena, &expr)
            .await
            .unwrap();
        match &*result {
            ValueKind::Number(n) => assert!((n.value - 3.0).abs() < 0.001),
            other => panic!("expected Number, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_modern_if_returns_raw() {
        // Dart `visitIfExpression` returns branch values raw, without
        // `_withoutSlash`: a slash-number still divides exactly once (at
        // creation) and `if()` adds no second warning.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let logger = Rc::new(RecordLogger::new());
        let opts = CompileOptions {
            logger: Some(logger.clone()),
            ..CompileOptions::new(&arena)
        };
        let result = compile_string(
            "$s: 1/2;\na { b: if(sass(true): $s; else: 0); }",
            io,
            opts,
            &arena,
        )
        .await
        .unwrap();
        assert!(result.css().contains("b: 0.5;"), "got: {}", result.css());
        assert_eq!(logger.messages().len(), 1, "got: {:?}", logger.messages());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_modern_if_renders_branches_as_css() {
        // Dart `visitIfExpression` renders unresolved branch values with
        // `toCssString`, not inspect form (#2808): a comma list loses its
        // inspect parens, while quoted strings keep their quotes.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let opts = CompileOptions::new(&arena);
        let result = compile_string(
            "a { b: if(css(): (x, y)); c: if(css(): \"s\"); }",
            io,
            opts,
            &arena,
        )
        .await
        .unwrap();
        assert!(
            result.css().contains("b: if(css(): x, y);"),
            "got: {}",
            result.css()
        );
        assert!(
            result.css().contains("c: if(css(): \"s\");"),
            "got: {}",
            result.css()
        );
    }
}
