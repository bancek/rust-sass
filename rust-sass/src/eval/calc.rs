// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/evaluate.dart (_visitCalculation family)
// go-source: go/eval/evaluate_expression.go (visitCalculation, visitCalculationExpression)

//! Calculation evaluation: Dart's `_visitCalculation` family.
//!
//! [`expression_to_calc_argument`] walks one argument expression into a
//! [`CalcArgument`] (Dart's `_visitCalculationExpression`, returning
//! `Object`); [`evaluate_css_math_function`] validates arity/keywords, checks
//! each argument, then dispatches to the `SassCalculation` constructors
//! (Dart's `_visitCalculation`). Both take the legacy-function name so
//! unitless numbers may mix with units inside the old global `min()`/`max()`/
//! `round()`/`abs()`; the compatibility re-verification that turns
//! simplification failures into per-operand `MultiSpan`s lives in
//! [`verify_compatible_numbers_with_spans`].

use crate::eval::expression::evaluate_expression;
use crate::eval::helpers::add_exception_span;
use crate::value::calculation::calc_arg_to_string;
use crate::value::calculation::new_abs_internal;
use crate::value::calculation::new_acos;
use crate::value::calculation::new_asin;
use crate::value::calculation::new_atan;
use crate::value::calculation::new_atan2;
use crate::value::calculation::new_calc;
use crate::value::calculation::new_calc_size;
use crate::value::calculation::new_clamp;
use crate::value::calculation::new_cos;
use crate::value::calculation::new_exp;
use crate::value::calculation::new_hypot;
use crate::value::calculation::new_log;
use crate::value::calculation::new_max;
use crate::value::calculation::new_min;
use crate::value::calculation::new_mod;
use crate::value::calculation::new_pow;
use crate::value::calculation::new_rem;
use crate::value::calculation::new_round_internal;
use crate::value::calculation::new_sign;
use crate::value::calculation::new_sin;
use crate::value::calculation::new_sqrt;
use crate::value::calculation::new_tan;
use crate::value::calculation::operate_internal;
use crate::value::SassCalculation;
use bumpalo::Bump;
use std::f64::consts::E;
use std::f64::consts::PI;

use crate::ast::sass::binary_operator::BinaryOperator;
use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression_function::FunctionExpression;
use crate::ast::sass::unary_operator::UnaryOperator;
use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::span::Span;
use crate::eval::helpers::{exception, file_span_to_ctx, stack_trace};
use crate::eval::warn::flush_buffered_warnings;
use crate::eval::{EvalConfig, EvalState};
use crate::logger::{BufferedWarnLogger, WarnLogger};
use crate::util::character;
use crate::value::{
    CalcArgument, CalculationOperator, ListSeparator, SassNumber, Value, ValueKind,
};

/// Converts an [`Expression`] to a [`CalcArgument`] for use in CSS math functions.
///
/// Matches Dart: `_visitCalculationExpression`. Parenthesized strings keep
/// their parens as `"(…)"`; `+`/`-` require surrounding whitespace; binary
/// operators simplify eagerly through `operateInternal` under the operation
/// span (`_addExceptionSpan`); unary operations of any kind are rejected;
/// strings pass the `isCalculationSafe` gate (with `pi`/`e`/`infinity`/
/// `-infinity`/`nan` folded to numbers); numbers, variables, values, legacy
/// `if()`s, and function calls evaluate first and only numbers, calculations,
/// and unquoted strings survive; space-separated lists of 2+ items join with
/// `" "` (parenthesized operations keep parens, adjacent non-strings error).
#[rust_sass_macros::maybe_async]
pub(crate) async fn expression_to_calc_argument<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &Expression<'parse>,
    in_legacy_sass_function: Option<&str>,
    warn: &dyn WarnLogger<'parse>,
) -> SassResult<CalcArgument>
where
    'compile: 'parse,
{
    match expr {
        Expression::Parenthesized(p) => {
            let inner = box_rec_in!(
                expression_to_calc_argument(
                    config,
                    state,
                    arena,
                    &p.expression,
                    in_legacy_sass_function,
                    warn,
                ),
                arena,
            )
            .await?;
            match inner {
                CalcArgument::String(text, _) => {
                    Ok(CalcArgument::String(format!("({text})"), false))
                }
                other => Ok(other),
            }
        }
        Expression::Number(_) => {
            let val = evaluate_expression(config, state, arena, expr).await?;
            match &*val {
                ValueKind::Number(sn) => Ok(CalcArgument::Number(sn.clone())),
                _ => unreachable!("number expression always evaluates to number"),
            }
        }
        Expression::BinaryOperation(b) => {
            // Match Go: checkWhitespaceAroundCalculationOperator
            if b.operator == BinaryOperator::Plus || b.operator == BinaryOperator::Minus {
                let left_span = b.left.span()?;
                let right_span = b.right.span()?;
                let left_end = left_span.end_location().offset;
                let right_start = right_span.start_location().offset;
                if left_end < right_start {
                    if let Some(file) = left_span.file() {
                        let text_between = &file.text()[left_end..right_start];
                        if !text_between.is_empty() {
                            let first = text_between.chars().next().unwrap();
                            let last = text_between.chars().last().unwrap();
                            if !(character::is_whitespace(first) || first == '/')
                                || !(character::is_whitespace(last) || last == '/')
                            {
                                let op_span = b.operator_span()?;
                                return Err(Box::new(exception(
                                    state,
                                    "\"+\" and \"-\" must be surrounded by whitespace in calculations.".into(),
                                    Some(op_span),
                                )));
                            }
                        }
                    }
                }
            }
            let left = box_rec_in!(
                expression_to_calc_argument(
                    config,
                    state,
                    arena,
                    &b.left,
                    in_legacy_sass_function,
                    warn,
                ),
                arena,
            )
            .await?;
            let right = box_rec_in!(
                expression_to_calc_argument(
                    config,
                    state,
                    arena,
                    &b.right,
                    in_legacy_sass_function,
                    warn,
                ),
                arena,
            )
            .await?;
            let calc_op = match b.operator {
                BinaryOperator::Plus => CalculationOperator::Plus,
                BinaryOperator::Minus => CalculationOperator::Minus,
                BinaryOperator::Times => CalculationOperator::Times,
                BinaryOperator::DividedBy => CalculationOperator::DividedBy,
                _ => {
                    // Dart highlights just the operator (e.g. `7 % 3` -> `%`).
                    let span = b.operator_span()?;
                    return Err(Box::new(exception(
                        state,
                        "This operation can't be used in a calculation.".into(),
                        Some(span),
                    )));
                }
            };
            let span = b.span()?;
            let result = add_exception_span(config, state, span, None, async |_config, state| {
                operate_internal(
                    calc_op,
                    left,
                    right,
                    in_legacy_sass_function,
                    !state.in_supports_declaration,
                    warn,
                    Some(span),
                )
            })
            .await?;
            Ok(result)
        }
        // Dart `_visitCalculationExpression` has no unary arm: any unary
        // operation falls to the rejection default.
        Expression::UnaryOperation(u) => {
            let span = u.span()?;
            Err(Box::new(exception(
                state,
                "This expression can't be used in a calculation.".into(),
                Some(span),
            )))
        }
        Expression::String(s) => {
            // Dart `_visitCalculationExpression`: `StringExpression() when
            // node.isCalculationSafe` — unsafe strings fall to the rejection
            // default below.
            if !expr.is_calculation_safe()? {
                let span = expr.span()?;
                return Err(Box::new(exception(
                    state,
                    "This expression can't be used in a calculation.".into(),
                    Some(span),
                )));
            }
            if s.has_quotes {
                let span = expr.span()?;
                return Err(Box::new(exception(
                    state,
                    "This expression can't be used in a calculation.".into(),
                    Some(span),
                )));
            }
            let lower = s.text.initial_plain().to_lowercase();
            match lower.as_str() {
                "pi" => Ok(CalcArgument::Number(SassNumber::new(PI, None))),
                "e" => Ok(CalcArgument::Number(SassNumber::new(E, None))),
                "infinity" => Ok(CalcArgument::Number(SassNumber::new(f64::INFINITY, None))),
                "-infinity" => Ok(CalcArgument::Number(SassNumber::new(
                    f64::NEG_INFINITY,
                    None,
                ))),
                "nan" => Ok(CalcArgument::Number(SassNumber::new(f64::NAN, None))),
                _ => {
                    let val = evaluate_expression(config, state, arena, expr).await?;
                    match &*val {
                        ValueKind::String(s) if !s.has_quotes => {
                            Ok(CalcArgument::String(s.text.to_string(), false))
                        }
                        _ => {
                            let display = val.to_display_string()?;
                            Ok(CalcArgument::String(display, false))
                        }
                    }
                }
            }
        }
        Expression::Variable(_) | Expression::Value(_) | Expression::LegacyIf(_) => {
            let val = evaluate_expression(config, state, arena, expr).await?;
            match &*val {
                ValueKind::Number(n) => Ok(CalcArgument::Number(n.clone())),
                ValueKind::Calculation(c) => Ok(CalcArgument::Calculation(c.clone())),
                ValueKind::String(s) if !s.has_quotes => {
                    Ok(CalcArgument::String(s.text.to_string(), false))
                }
                _ => {
                    let span = expr.span()?;
                    let display = val.to_display_string()?;
                    Err(Box::new(exception(
                        state,
                        format!("Value {display} can't be used in a calculation."),
                        Some(span),
                    )))
                }
            }
        }
        Expression::Function(_) => {
            let val = evaluate_expression(config, state, arena, expr).await?;
            match &*val {
                ValueKind::Number(n) => Ok(CalcArgument::Number(n.clone())),
                ValueKind::Calculation(c) => Ok(CalcArgument::Calculation(c.clone())),
                ValueKind::String(s) if !s.has_quotes => {
                    Ok(CalcArgument::String(s.text.to_string(), false))
                }
                _ => {
                    let span = expr.span()?;
                    let display = val.to_display_string()?;
                    Err(Box::new(exception(
                        state,
                        format!("Value {display} can't be used in a calculation."),
                        Some(span),
                    )))
                }
            }
        }
        Expression::List(l) => {
            if l.has_brackets || l.separator != ListSeparator::Space || l.contents.len() < 2 {
                let span = expr.span()?;
                return Err(Box::new(exception(
                    state,
                    "This expression can't be used in a calculation.".into(),
                    Some(span),
                )));
            }
            let mut elements: Vec<CalcArgument> = Vec::new();
            for elem in &l.contents {
                let val = box_rec_in!(
                    expression_to_calc_argument(
                        config,
                        state,
                        arena,
                        elem,
                        in_legacy_sass_function,
                        warn,
                    ),
                    arena,
                )
                .await?;
                elements.push(val);
            }
            // Match Go's checkAdjacentCalculationValues: adjacent non-string
            // values in a space-separated list are invalid.
            for i in 1..elements.len() {
                let prev = &elements[i - 1];
                let curr = &elements[i];
                let prev_is_str = matches!(prev, CalcArgument::String(..));
                let curr_is_str = matches!(curr, CalcArgument::String(..));
                if !prev_is_str && !curr_is_str {
                    // Go: check if curr_node is unary or negative for special error
                    let curr_node = &l.contents[i];
                    let is_unary_or_negative = match curr_node {
                        Expression::UnaryOperation(u) => {
                            matches!(u.operator, UnaryOperator::Minus | UnaryOperator::Plus)
                        }
                        Expression::Number(n) => n.value < 0.0,
                        _ => false,
                    };
                    if is_unary_or_negative {
                        let curr_span = l.contents[i].span()?;
                        let op_span = curr_span
                            .subspan(0, 1)
                            .map_err(|e| exception(state, e.to_string(), Some(curr_span)))?;
                        return Err(Box::new(exception(
                            state,
                            "\"+\" and \"-\" must be surrounded by whitespace in calculations."
                                .into(),
                            Some(op_span),
                        )));
                    }
                    let prev_span = l.contents[i - 1].span()?;
                    let curr_span = l.contents[i].span()?;
                    let expanded = prev_span
                        .expand(&Span::File(curr_span))
                        .map_err(|e| exception(state, e.to_string(), Some(prev_span)))?;
                    return Err(Box::new(exception(
                        state,
                        "Missing math operator.".into(),
                        Some(expanded),
                    )));
                }
            }
            // Dart `_visitCalculationExpression`: a parenthesized
            // `CalculationOperation` in a space-separated list keeps its
            // parens (`"(…)"`).
            for (element, content) in elements.iter_mut().zip(l.contents.iter()) {
                if matches!(element, CalcArgument::Operation(_))
                    && matches!(content, Expression::Parenthesized(_))
                {
                    let inner = calc_arg_to_string(element)?;
                    *element = CalcArgument::String(format!("({inner})"), false);
                }
            }
            let mut parts = Vec::new();
            for e in &elements {
                parts.push(calc_arg_to_string(e)?);
            }
            Ok(CalcArgument::String(parts.join(" "), false))
        }
        _ => {
            let span = expr.span()?;
            Err(Box::new(exception(
                state,
                "This expression can't be used in a calculation.".into(),
                Some(span),
            )))
        }
    }
}

/// Evaluates a CSS math function call (`calc()`, `min()`, `clamp()`, …).
///
/// Matches Dart: `_visitCalculation`. Rejects keyword/rest arguments, checks
/// arity (`_checkCalculationArguments`), converts each positional argument
/// (wrapping bare `Script` failures at the call span), returns calculations
/// unsimplified inside `@supports` declarations, and reframes incompatible-
/// unit failures against the original operand spans via
/// [`verify_compatible_numbers_with_spans`].
#[rust_sass_macros::maybe_async]
pub(crate) async fn evaluate_css_math_function<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    expr: &FunctionExpression<'parse>,
    in_legacy_sass_function: Option<&str>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let name = expr.name.to_lowercase();
    let args = &expr.arguments;

    // Validate: no named args
    if !args.named.is_empty() {
        let span = expr.span()?;
        return Err(Box::new(exception(
            state,
            "Keyword arguments can't be used with calculations.".into(),
            Some(span),
        )));
    }

    // Validate: no rest args
    if args.rest.is_some() {
        let span = expr.span()?;
        return Err(Box::new(exception(
            state,
            "Rest arguments can't be used with calculations.".into(),
            Some(span),
        )));
    }

    // Validate: correct number of args
    let max_args = calculation_max_args(&name);
    if max_args > 0 && args.positional.len() > max_args {
        let span = expr.span()?;
        return Err(Box::new(exception(
            state,
            format!(
                "Only {} argument{} allowed, but {} {} passed.",
                max_args,
                if max_args == 1 { "" } else { "s" },
                args.positional.len(),
                if args.positional.len() == 1 {
                    "was"
                } else {
                    "were"
                },
            ),
            Some(span),
        )));
    }
    if args.positional.is_empty() {
        let span = expr.span()?;
        return Err(Box::new(exception(
            state,
            "Missing argument.".into(),
            Some(span),
        )));
    }

    // Evaluate each argument as a calc expression.
    // Catch Script errors here so we can wrap with the function span.
    let warn_buf = BufferedWarnLogger::new(arena);
    let mut evaluated = Vec::new();
    for arg in &args.positional {
        match expression_to_calc_argument(
            config,
            state,
            arena,
            arg,
            in_legacy_sass_function,
            &warn_buf,
        )
        .await
        {
            Ok(val) => evaluated.push(val),
            Err(e) if matches!(*e, SassError::Script { .. }) => {
                let span = expr.span()?;
                return Err(Box::new(exception(state, e.full_message(), Some(span))));
            }
            Err(e) => return Err(e),
        }
    }

    // Match Go: visitCalculation — if v.inSupportsDeclaration, return unsimplified
    if state.in_supports_declaration {
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                &name, evaluated,
            ))),
        ));
    }

    // Helper: get first arg without consuming the vec
    let first = || evaluated.first().cloned().unwrap();
    // Clone for the span-aware compatibility re-verification (the dispatch
    // below moves `evaluated` into min/max/hypot).
    let evaluated_verify = evaluated.clone();

    // Dispatch by function name
    let result = match name.as_str() {
        "calc" => new_calc(arena, first()),
        "min" => new_min(arena, evaluated),
        "max" => new_max(arena, evaluated),
        "clamp" => new_clamp(
            arena,
            evaluated
                .first()
                .cloned()
                .unwrap_or(CalcArgument::String(String::new(), false)),
            evaluated.get(1).cloned(),
            evaluated.get(2).cloned(),
        ),
        "mod" => new_mod(arena, first(), evaluated.get(1).cloned()),
        "rem" => new_rem(arena, first(), evaluated.get(1).cloned()),
        "sin" => new_sin(arena, first()),
        "cos" => new_cos(arena, first()),
        "tan" => new_tan(arena, first()),
        "asin" => new_asin(arena, first()),
        "acos" => new_acos(arena, first()),
        "atan" => new_atan(arena, first()),
        "atan2" => new_atan2(arena, first(), evaluated.get(1).cloned()),
        "pow" => new_pow(arena, first(), evaluated.get(1).cloned()),
        "sqrt" => new_sqrt(arena, first()),
        "hypot" => new_hypot(arena, evaluated),
        "log" => new_log(arena, first(), evaluated.get(1).cloned()),
        "exp" => new_exp(arena, first()),
        "abs" => {
            let span = expr.span()?;
            new_abs_internal(arena, first(), &warn_buf, Some(span))
        }
        "round" => {
            let span = expr.span()?;
            new_round_internal(
                arena,
                first(),
                evaluated.get(1).cloned(),
                evaluated.get(2).cloned(),
                in_legacy_sass_function,
                &warn_buf,
                Some(span),
            )
        }
        "sign" => new_sign(arena, first()),
        "calc-size" => new_calc_size(first(), evaluated.get(1).cloned())
            .map(|calc| Value::new_with_arena(arena, ValueKind::Calculation(calc))),
        _ => {
            let span = expr.span()?;
            return Err(Box::new(exception(
                state,
                format!("Unknown calculation name \"{name}\"."),
                Some(span),
            )));
        }
    };

    // Flush any buffered deprecation warnings through the eval pipeline.
    flush_buffered_warnings(&warn_buf, config, state)?;

    // Wrap Script errors with proper spans.
    match result {
        Ok(v) => Ok(v),
        Err(e) if matches!(*e, SassError::Script { .. }) => {
            // Mirrors Dart's `_visitCalculation` catch block
            // (async_evaluate.dart:3199-3206): when the simplification logic
            // reports incompatible units, re-verify against the original
            // argument spans so the error points at the offending operand(s).
            if e.message().contains("compatible") {
                verify_compatible_numbers_with_spans(state, &evaluated_verify, &args.positional)?
            }
            let span = expr.span()?;
            Err(Box::new(exception(state, e.full_message(), Some(span))))
        }
        Err(other) => Err(other),
    }
}

/// Mirrors Dart's `EvaluateVisitor._verifyCompatibleNumbers`
/// (async_evaluate.dart:3261-3295). Re-checks the evaluated calc [arguments]
/// against the AST [arg_nodes]' spans, throwing a single-span error for numbers
/// with complex units and a MultiSpan error for incompatible pairs.
///
/// Returns `Ok(())` if no incompatibility is found (the caller falls back to
/// the whole-call span).
///
/// Matches Dart: `_verifyCompatibleNumbers`, including the documented
/// duplication with `SassCalculation._verifyCompatibleNumbers` — most changes
/// here should also be reflected there.
fn verify_compatible_numbers_with_spans<'compile, 'parse>(
    state: &mut EvalState<'compile, 'parse>,
    arguments: &[CalcArgument],
    arg_nodes: &[Expression<'parse>],
) -> SassResult<()>
where
    'compile: 'parse,
{
    for (i, arg) in arguments.iter().enumerate() {
        if let CalcArgument::Number(num) = arg {
            if num.has_complex_units() {
                let span = arg_nodes[i].span()?;
                return Err(Box::new(exception(
                    state,
                    format!(
                        "Number {} isn't compatible with CSS calculations.",
                        num.to_display_string()?
                    ),
                    Some(span),
                )));
            }
        }
    }

    for i in 0..arguments.len() {
        let number1 = match &arguments[i] {
            CalcArgument::Number(n) => n,
            _ => continue,
        };
        for j in (i + 1)..arguments.len() {
            let number2 = match &arguments[j] {
                CalcArgument::Number(n) => n,
                _ => continue,
            };
            if number1.has_possibly_compatible_units(number2) {
                continue;
            }
            let span_i = arg_nodes[i].span()?;
            let span_j = arg_nodes[j].span()?;
            let n1 = number1.to_display_string()?;
            let n2 = number2.to_display_string()?;
            return Err(Box::new(SassError::MultiSpan {
                message: format!("{n1} and {n2} are incompatible."),
                span: file_span_to_ctx(&span_i),
                primary_label: Some(n1.clone()),
                secondary: vec![(file_span_to_ctx(&span_j), n2)],
                original_source: None,
                cause: None,
                loaded_urls: vec![],
                trace: stack_trace(state, Some(span_i)),
            }));
        }
    }
    Ok(())
}

/// Returns the maximum positional-argument count for a calculation name.
///
/// Matches Dart: `_checkCalculationArguments` (`min`/`max`/`hypot` take any
/// non-zero count, hence `0` = no upper bound; anything unknown errors in the
/// caller rather than here).
fn calculation_max_args(name: &str) -> usize {
    match name {
        "calc" | "sqrt" | "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "abs" | "exp"
        | "sign" => 1,
        "min" | "max" | "hypot" => 0,
        "pow" | "atan2" | "log" | "mod" | "rem" | "calc-size" => 2,
        "clamp" | "round" => 3,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile_string;
    use crate::compile::CompileOptions;
    use crate::io::Io;
    use crate::io::VirtualIo;
    use std::rc::Rc;

    #[rust_sass_macros::maybe_test]
    async fn test_calc_rejects_unary_variable() {
        // Dart `_visitCalculationExpression` has no unary arm: `calc(-$x)`
        // and `calc(+$x)` both throw "This expression can't be used in a
        // calculation."
        for op in ["-", "+"] {
            let arena = Bump::new();
            let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
            let src = format!("$x: 1px;\na {{ b: calc({op}$x); }}");
            let err = compile_string(&src, io, CompileOptions::new(&arena), &arena)
                .await
                .unwrap_err();
            match *err {
                SassError::Runtime { message, .. } => assert_eq!(
                    message, "This expression can't be used in a calculation.",
                    "op: {op}"
                ),
                other => panic!("expected Runtime rejection, got {other:?}"),
            }
        }

        // Binary minus over literals still simplifies.
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let result = compile_string(
            "$x: 1px;\na { b: calc(-1px + 2px); }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert!(result.css().contains("1px"), "got: {}", result.css());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_calc_rejects_unsafe_strings() {
        // Dart `_visitCalculationExpression` only accepts `StringExpression()
        // when node.isCalculationSafe`; anything else throws "This expression
        // can't be used in a calculation."
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "a { b: calc(!important); }",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, .. } => {
                assert_eq!(message, "This expression can't be used in a calculation.")
            }
            other => panic!("expected Runtime rejection, got {other:?}"),
        }

        // Safe unquoted strings still pass through, as do pi constants.
        for (src, expected) in [
            ("a { b: calc(auto + 1px); }", "calc(auto + 1px)"),
            ("a { b: calc(pi * 2px); }", "6.2831853072px"),
        ] {
            let arena = Bump::new();
            let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
            let result = compile_string(src, io, CompileOptions::new(&arena), &arena)
                .await
                .unwrap();
            assert!(
                result.css().contains(expected),
                "src {src}: got {}",
                result.css()
            );
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_calc_space_list_keeps_parens() {
        // Dart `_visitCalculationExpression`: a parenthesized
        // `CalculationOperation` inside a space-separated list is rewrapped
        // as `"(…)"` before joining.
        for (src, expected) in [
            ("a { b: calc((1px + 1em) foo); }", "calc((1px + 1em) foo)"),
            ("a { b: calc(1px foo 2px); }", "calc(1px foo 2px)"),
        ] {
            let arena = Bump::new();
            let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
            let result = compile_string(src, io, CompileOptions::new(&arena), &arena)
                .await
                .unwrap();
            assert!(
                result.css().contains(expected),
                "src {src}: got {}",
                result.css()
            );
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_operate_binary_op_span_layering() {
        // Dart layers operateInternal errors as unspanned Script, with the
        // BinaryOperation visitor adding the operation span via
        // _addExceptionSpan (evaluate.dart:3331-3350), and _visitCalculation
        // re-verifying to a per-operand MultiSpan (evaluate.dart:3198-3205).
        // calc() paths report the whole-operation span, never a bare
        // single-operand span or a spanless error.
        for (src, want_span_text) in [
            ("a{width:calc(1px + 1s)}", "1px + 1s"),
            ("a{width:calc(1px*2 + 1s)}", "1px*2 + 1s"),
        ] {
            let arena = Bump::new();
            let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
            let err = compile_string(src, io, CompileOptions::new(&arena), &arena)
                .await
                .unwrap_err();
            match *err {
                SassError::Runtime { message, span, .. } => {
                    assert!(message.contains("incompatible"), "src {src}: {message}");
                    assert!(
                        span.text.contains(want_span_text),
                        "src {src}: span text {:?} should contain {want_span_text:?}",
                        span.text
                    );
                }
                other => panic!("src {src}: expected Runtime, got {other:?}"),
            }
        }
        // Multi-operand path still yields per-operand MultiSpan labels.
        {
            let arena = Bump::new();
            let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
            let err = compile_string(
                "a{width:min(1px, 1s)}",
                io,
                CompileOptions::new(&arena),
                &arena,
            )
            .await
            .unwrap_err();
            match *err {
                SassError::MultiSpan {
                    message,
                    primary_label,
                    secondary,
                    ..
                } => {
                    assert!(message.contains("incompatible"), "{message}");
                    assert_eq!(primary_label.as_deref(), Some("1px"));
                    assert_eq!(secondary.len(), 1);
                    assert_eq!(secondary[0].1, "1s");
                }
                other => panic!("expected MultiSpan, got {other:?}"),
            }
        }
    }
}
