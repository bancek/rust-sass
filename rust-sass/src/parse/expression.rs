// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/stylesheet.dart (expression sections: `_expression` family)
// go-source: go/value/parse_stylesheet_expression.go

//! Pratt expression parser: source text in, [`Expression`] out.
//!
//! Ports the `_expression` family of `stylesheet.dart`. Dart implements the
//! operator precedence loop with six nested closures sharing nullable locals;
//! Rust cannot borrow the scanner, the stylesheet state, and the shared
//! accumulators through closures that call each other, so the shared state is
//! split into [`Acc`] (partially built lists) and [`Op`] (pending operators)
//! and every closure becomes a free function taking disjoint `&mut`
//! references. Nested bracketed lists use an explicit [`BracketFrame`] stack
//! rather than recursive `_expression(bracketList: true)` calls.

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::binary_operator::BinaryOperator;
use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression_binary_operation::BinaryOperationExpression;
use crate::ast::sass::expression_boolean::BooleanExpression;
use crate::ast::sass::expression_color::ColorExpression;
use crate::ast::sass::expression_function::FunctionExpression;
use crate::ast::sass::expression_if::BooleanOperator as IfBoolOp;
use crate::ast::sass::expression_if::IfConditionFunction;
use crate::ast::sass::expression_if::IfConditionNegation;
use crate::ast::sass::expression_if::IfConditionOperation;
use crate::ast::sass::expression_if::IfConditionParenthesized;
use crate::ast::sass::expression_if::IfConditionRaw;
use crate::ast::sass::expression_if::IfConditionSass;
use crate::ast::sass::expression_if::{IfBranch, IfConditionExpression, IfExpression};
use crate::ast::sass::expression_interpolated_function::InterpolatedFunctionExpression;
use crate::ast::sass::expression_legacy_if::LegacyIfExpression;
use crate::ast::sass::expression_list::ListExpression;
use crate::ast::sass::expression_map::MapExpression;
use crate::ast::sass::expression_null::NullExpression;
use crate::ast::sass::expression_number::NumberExpression;
use crate::ast::sass::expression_parenthesized::ParenthesizedExpression;
use crate::ast::sass::expression_selector::SelectorExpression;
use crate::ast::sass::expression_string::StringExpression;
use crate::ast::sass::expression_unary_operation::UnaryOperationExpression;
use crate::ast::sass::expression_variable::VariableExpression;
use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation::InterpolationPart;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::ast::sass::statement::stylesheet::ParseTimeWarning;
use crate::ast::sass::unary_operator::UnaryOperator;
use crate::common::ast_node::AstNode;
use crate::common::file_span::FileSpan;
use crate::common::span::Span;
use crate::common::span_error::SpanError;
use crate::common::span_scanner::{LineScannerState, SpanScanner};
use crate::deprecation;
use crate::deprecation::FUNCTION_NAME;
use crate::deprecation::MISPLACED_REST;
use crate::parse::css::css_identifier_like_impl;
use crate::parse::css::css_parentheses_impl;
use crate::parse::parser::raw_text_impl;
use crate::parse::parser::ParserState;
use crate::parse::util::looking_at_expression_impl;
use crate::parse::util::looking_at_interpolated_identifier_body_impl;
use crate::unvendor::unvendor;
use crate::util::character;
use crate::value::color::{ColorFormat, SassColor};
use crate::value::ListSeparator;
use indexmap::IndexMap;

use crate::parse::anyvalue::{interpolated_declaration_value_impl, DeclarationValueOpts};
use crate::parse::identifier::{interpolated_identifier_impl, single_interpolation_impl};
use crate::parse::parser::{
    error_impl, escape_character_impl, escape_impl, expect_identifier_impl, identifier_impl,
    looking_at_identifier, scan_identifier_impl, span_from_impl, whitespace_impl,
    whitespace_without_comments_impl, ParseError, ParseResult,
};
use crate::parse::stylesheet::{StylesheetParser, StylesheetState, Syntax};
use crate::parse::util::looking_at_interpolated_identifier_impl;

// ======================================================================
// Opts
// ======================================================================

// Options threading Dart's `_expression` named parameters through the
// free-function parser. `bracket_list` parses `[...]` contents,
// `single_equals` allows the Microsoft-style `=` operator at top level,
// `consume_newlines` lets the indented syntax treat newlines as whitespace
// where a statement cannot end, and `until` ends the expression early when it
// returns true.
pub(crate) struct ExpressionOpts<'r, 'parse> {
    pub bracket_list: bool,
    pub single_equals: bool,
    pub consume_newlines: bool,
    // Idiomatic parser hook; a named alias would carry two lifetimes for one use.
    #[allow(clippy::type_complexity)]
    pub until: Option<Box<dyn FnMut(&mut SpanScanner<'parse>) -> bool + 'r>>,
}

// ======================================================================
// Pratt sub-structs
// ======================================================================

// Dart's nullable shared locals, split so borrows stay disjoint.
// `Acc` holds the list levels being built (`comma_exprs`/`space_exprs`), the
// leftmost fully parsed expression (`single`), and whether it may still
// become slash-separated numbers (`allow_slash`). `Op` holds the operators
// whose right-hand operands are not fully parsed yet (`operators`, lowest to
// highest precedence) plus their left-hand sides (`operands[n]` is the
// left-hand side of `operators[n]`).
pub(crate) struct Acc<'parse> {
    pub comma_exprs: Vec<Expression<'parse>>,
    pub space_exprs: Vec<Expression<'parse>>,
    pub single: Option<Expression<'parse>>,
    pub allow_slash: bool,
}

// Operators whose right-hand operands are not fully parsed yet, in order of
// appearance. A low-precedence operator finishes parsing all preceding
// higher-precedence operators, so this is naturally lowest-to-highest.
pub(crate) struct Op<'parse> {
    pub operators: Vec<BinaryOperator>,
    pub operands: Vec<Expression<'parse>>,
}

// ======================================================================
// number_impl + helpers
// ======================================================================

// Dart: `_number`. Consumes a number expression, including an optional unit.
// Uses the source text's float parsing so numbers with many digits do not
// accumulate floating-point error.
pub(crate) fn number_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &ParserState<'parse>,
) -> ParseResult<'parse, NumberExpression<'parse>> {
    let start = scanner.state();
    let first = scanner.peek_char(0);
    if first == '+' as i32 || first == '-' as i32 {
        scanner.read_char()?;
    }
    if scanner.peek_char(0) != '.' as i32 {
        consume_natural_number(scanner)?;
    }
    // Go: tryDecimal allows trailing dot when at least one digit was consumed
    // and the first char is not a sign (so `2...,` parses as `2` then `...,`).
    let consumed = start.position != scanner.pos();
    let allow_trailing = consumed && first != '+' as i32 && first != '-' as i32;
    try_decimal(scanner, allow_trailing)?;
    try_exponent(scanner)?;

    let text = scanner.substring(start.position, None).to_owned();
    let number: f64 = text.parse().unwrap_or(if text.starts_with('-') {
        f64::NEG_INFINITY
    } else {
        f64::INFINITY
    });

    let mut unit: Option<String> = None;
    if scanner.scan_char('%') {
        unit = Some("%".to_string());
    } else if looking_at_identifier(scanner, None)
        && !(scanner.peek_char(0) == '-' as i32 && scanner.peek_char(1) == '-' as i32)
    {
        unit = Some(identifier_impl(scanner, state, false, true)?);
    }

    let span = scanner.span_from(start);
    Ok(NumberExpression::new(number, span, unit))
}

// Dart: `_consumeNaturalNumber`. Consumes a natural number (a non-negative
// integer); no scientific notation.
fn consume_natural_number<'parse>(scanner: &mut SpanScanner<'parse>) -> ParseResult<'parse, ()> {
    let ch = scanner.read_char()?;
    if !character::is_digit(ch) {
        return Err(scanner
            .error("Expected digit.", Some(scanner.pos().saturating_sub(1)), 0)
            .into());
    }
    while character::is_digit(scanner.peek_char(0) as u8 as char) {
        scanner.read_char()?;
    }
    Ok(())
}

// Dart: `_tryDecimal`. Consumes the decimal component if present. When
// `allow_trailing_dot` is false a dot with no following digits is an error;
// otherwise the dot is left unconsumed so `1...` still parses as `1` plus a
// rest argument.
fn try_decimal<'parse>(
    scanner: &mut SpanScanner<'parse>,
    allow_trailing_dot: bool,
) -> ParseResult<'parse, ()> {
    if scanner.peek_char(0) != '.' as i32 {
        return Ok(());
    }
    if !character::is_digit(scanner.peek_char(1) as u8 as char) {
        if allow_trailing_dot {
            return Ok(());
        }
        scanner.read_char()?; // consume '.'
        let pos = scanner.pos();
        return Err(Box::new(
            scanner.error("Expected digit.", Some(pos), 1).into(),
        ));
    }
    scanner.read_char()?;
    while character::is_digit(scanner.peek_char(0) as u8 as char) {
        scanner.read_char()?;
    }
    Ok(())
}

// Dart: `_tryExponent`. Consumes the exponent component (`e`/`E` plus an
// optional sign and digits) if present.
fn try_exponent<'parse>(scanner: &mut SpanScanner<'parse>) -> ParseResult<'parse, ()> {
    let first = scanner.peek_char(0);
    if first != 'e' as i32 && first != 'E' as i32 {
        return Ok(());
    }
    let next = scanner.peek_char(1);
    if !character::is_digit(next as u8 as char) && next != '-' as i32 && next != '+' as i32 {
        return Ok(());
    }
    scanner.read_char()?;
    if next == '+' as i32 || next == '-' as i32 {
        scanner.read_char()?;
    }
    if !character::is_digit(scanner.peek_char(0) as u8 as char) {
        return Err(Box::new(scanner.error("Expected digit.", None, 0).into()));
    }
    while character::is_digit(scanner.peek_char(0) as u8 as char) {
        scanner.read_char()?;
    }
    Ok(())
}

// ======================================================================
// Pratt: resolve_one / resolve_all / resolve_space / add_single / add_op
// ======================================================================

// Dart: `resolveOneOperation`. Folds the top pending operator with `single`
// as its right operand. A `/` folds into a slash-separated number only when
// slash is still allowed, the parser is not inside parentheses, and both
// sides are slash operands; otherwise slash is disallowed from here on. A
// `+`/`-` with whitespace before the operator but not after emits the
// strict-unary deprecation warning.
fn resolve_one<'parse>(
    scanner: &SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    ops: &mut Op<'parse>,
    acc: &mut Acc<'parse>,
) -> ParseResult<'parse, ()> {
    let op = ops.operators.pop().unwrap();
    let left = ops.operands.pop().unwrap();
    let right = acc.single.take().ok_or_else(|| {
        let syn: isize = op.operator_syntax().len().try_into().unwrap();
        let err: ParseError<'parse> = scanner
            .error(
                "Expected expression.",
                Some(scanner.pos().saturating_sub(syn as usize)),
                syn,
            )
            .into();
        err
    })?;

    // Pre-extract span info before creating binary op (needed for strict-unary check)
    let strict_unary_info = if op == BinaryOperator::Plus || op == BinaryOperator::Minus {
        let left_file = left.span().map_err(|e| Box::new(ParseError::Sass(e)))?;
        let right_file = right.span().map_err(|e| Box::new(ParseError::Sass(e)))?;
        Some((left_file, right_file))
    } else {
        None
    };
    // Dart interpolates the `Expression`s in the warning, so numbers are
    // serialized (`+5.0%` renders as `5%`), not raw source text
    // (stylesheet.dart:2080-2084).
    let strict_unary_strings = if op == BinaryOperator::Plus || op == BinaryOperator::Minus {
        Some((
            left.to_display_string()
                .map_err(|e| Box::new(ParseError::Sass(e)))?,
            right
                .to_display_string()
                .map_err(|e| Box::new(ParseError::Sass(e)))?,
        ))
    } else {
        None
    };

    // Create binary operation FIRST (matching Go line 328: singleExpression = NewBinaryOperationExpression)
    if acc.allow_slash && !state.in_parentheses && op == BinaryOperator::DividedBy {
        if is_slash_operand_impl(&left) && is_slash_operand_impl(&right) {
            acc.single = Some(Expression::BinaryOperation(
                BinaryOperationExpression::new_slash(left, right),
            ));
        } else {
            acc.single = Some(Expression::BinaryOperation(BinaryOperationExpression::new(
                op, left, right,
            )));
            acc.allow_slash = false;
        }
    } else {
        acc.single = Some(Expression::BinaryOperation(BinaryOperationExpression::new(
            op, left, right,
        )));
        acc.allow_slash = false;
    }

    // Strict-unary check AFTER binary op creation (matching Go line 351: singleExpression.Span())
    if let Some((left_file, right_file)) = strict_unary_info {
        let rloc = right_file.start_location();
        let lloc = left_file.end_location();
        let syn = op.operator_syntax();
        if rloc.offset >= syn.len()
            && scanner.substring(rloc.offset - syn.len(), Some(rloc.offset)) == syn
        {
            let after_left = scanner
                .text()
                .as_bytes()
                .get(lloc.offset)
                .copied()
                .unwrap_or(0) as char;
            if character::is_whitespace(after_left) {
                let (left_str, right_str) =
                    strict_unary_strings.as_ref().cloned().unwrap_or_else(|| {
                        (left_file.text().to_string(), right_file.text().to_string())
                    });
                let warn_span = acc
                    .single
                    .as_ref()
                    .and_then(|e| e.span().ok())
                    .unwrap_or(right_file);
                state.warnings.push(ParseTimeWarning {
                    deprecation: Some(&deprecation::STRICT_UNARY),
                    message: format!(
                        "This operation is parsed as:\n\n    {} {} {}\n\n\
                         but you may have intended it to mean:\n\n    {} ({}{})\n\n\
                         Add a space after {} to clarify that it's meant to be a \
                         binary operation, or wrap\nit in parentheses to make it \
                         a unary operation. This will be an error in future\n\
                         versions of Sass.\n\n\
                         More info and automated migrator: \
                         https://sass-lang.com/d/strict-unary",
                        left_str, syn, right_str, left_str, syn, right_str, syn,
                    ),
                    span: warn_span,
                    primary_label: None,
                    secondary: vec![],
                });
            }
        }
    }
    Ok(())
}

// Dart: `resolveOperations`. Drains every pending operator.
fn resolve_all<'parse>(
    scanner: &SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    ops: &mut Op<'parse>,
    acc: &mut Acc<'parse>,
) -> ParseResult<'parse, ()> {
    while !ops.operators.is_empty() {
        resolve_one(scanner, state, ops, acc)?;
    }
    Ok(())
}

// Dart: `resolveSpaceExpressions`. Resolves pending operators, then folds
// the accumulated space-separated operands into a space-separated
// [`ListExpression`].
fn resolve_space<'parse>(
    scanner: &SpanScanner<'parse>,
    acc: &mut Acc<'parse>,
) -> ParseResult<'parse, ()> {
    if acc.space_exprs.is_empty() {
        return Ok(());
    }
    let cur = acc.single.take().ok_or_else(|| {
        let err: ParseError<'parse> = scanner.error("Expected expression.", None, 0).into();
        err
    })?;
    acc.space_exprs.push(cur);
    if acc.space_exprs.len() == 1 {
        acc.single = Some(acc.space_exprs.remove(0));
        return Ok(());
    }
    let first = acc.space_exprs[0]
        .span()
        .map_err(|e| Box::new(ParseError::Sass(e)))?;
    let last = acc
        .space_exprs
        .last()
        .unwrap()
        .span()
        .map_err(|e| Box::new(ParseError::Sass(e)))?;
    let last_span: Span = last.into();
    let span = first
        .expand(&last_span)
        .map_err(|e| error_impl(&e.to_string(), &first))?;
    acc.single = Some(Expression::List(ListExpression::new(
        acc.space_exprs.clone(),
        ListSeparator::Space,
        span,
        false,
    )));
    acc.space_exprs.clear();
    Ok(())
}

/// Whether `expression` may appear as an operand of a `/` that produces a
/// potentially slash-separated number.
///
/// Dart: `_isSlashOperand`. Only numbers, function calls, and slash-tolerant
/// binary operations qualify; lists never do.
pub(crate) fn is_slash_operand_impl(expression: &Expression<'_>) -> bool {
    // Matches Go: only NumberExpression, FunctionExpression, BinaryOperationExpression
    // Note: List is NOT a slash operand (Go does not have Expression::List(_) => true)
    match expression {
        Expression::Number(_) | Expression::Function(_) => true,
        Expression::BinaryOperation(be) => be.allows_slash(),
        _ => false,
    }
}

// Dart: `addSingleExpression`. Installs `expr` as the current expression. If
// a current expression already exists, pending operations resolve first and
// the old value becomes a space-list element. While inside parentheses, the
// first space boundary instead re-parses from the expression start outside
// the paren context so `(1/2 1)` keeps `/` as a separator rather than
// dividing.
fn add_single<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    ops: &mut Op<'parse>,
    acc: &mut Acc<'parse>,
    expr: Expression<'parse>,
    start: LineScannerState,
) -> ParseResult<'parse, ()> {
    if acc.single.is_some() && state.in_parentheses && acc.allow_slash {
        // Matches Go: addSingleExpression in parse_stylesheet_expression.go:400-429
        // When in parentheses and we discover a space-separated list,
        // reset to re-parse without division context so "/" can be a CSS separator.
        state.in_parentheses = false;
        acc.comma_exprs.clear();
        acc.space_exprs.clear();
        ops.operators.clear();
        ops.operands.clear();
        scanner.set_state(start);
        acc.allow_slash = true;
        acc.single = Some(parse_single(scanner, state)?);
        return Ok(());
    }
    if acc.single.is_some() {
        resolve_all(scanner, state, ops, acc)?;
        acc.space_exprs.push(acc.single.take().unwrap());
        acc.allow_slash = true;
    }
    acc.single = Some(expr);
    Ok(())
}

// Dart: `addOperator`. Pushes `op` after resolving pending operators of
// equal or higher precedence. Only `=` (Microsoft-style), `+`, `-`, `*`,
// and `/` are allowed in plain CSS because they may appear in calculations;
// the check happens at evaluation time. A trailing `%` with no expression
// after it is a literal `%` string, not a modulo operator.
fn add_op<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    ops: &mut Op<'parse>,
    acc: &mut Acc<'parse>,
    op: BinaryOperator,
    op_start: usize,
) -> ParseResult<'parse, ()> {
    if is_plain_css(state)
        && op != BinaryOperator::SingleEquals
        && op != BinaryOperator::Plus
        && op != BinaryOperator::Minus
        && op != BinaryOperator::Times
        && op != BinaryOperator::DividedBy
    {
        let syn: isize = op.operator_syntax().len().try_into().unwrap();
        return Err(scanner
            .error(
                "Operators aren't allowed in plain CSS.",
                Some(op_start),
                syn,
            )
            .into());
    }
    acc.allow_slash = acc.allow_slash && op == BinaryOperator::DividedBy;

    while !ops.operators.is_empty() && ops.operators.last().unwrap().precedence() >= op.precedence()
    {
        resolve_one(scanner, state, ops, acc)?;
    }
    let single = acc.single.take().ok_or_else(|| {
        let syn: isize = op.operator_syntax().len().try_into().unwrap();
        let err: ParseError<'parse> = scanner
            .error(
                "Expected expression.",
                Some(scanner.pos().saturating_sub(syn as usize)),
                syn,
            )
            .into();
        err
    })?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    ops.operators.push(op);
    ops.operands.push(single);
    acc.single = Some(parse_single(scanner, state)?);
    Ok(())
}

// Dart: `_unaryOperation` + `_unaryOperatorFor`. Consumes a unary `+`, `-`,
// or `/` operation. `+`/`-` are rejected in plain CSS; `/` is still parsed so
// calculations parse, with evaluation rejecting the use.
fn unary<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, UnaryOperationExpression<'parse>> {
    let start = scanner.state();
    let ch = scanner.read_char()?;
    let op = match ch {
        '+' => UnaryOperator::Plus,
        '-' => UnaryOperator::Minus,
        '/' => UnaryOperator::Divide,
        _ => {
            return Err(scanner
                .error(
                    "Expected unary operator.",
                    Some(scanner.pos().saturating_sub(1)),
                    1,
                )
                .into())
        }
    };
    // Matches Dart: _unaryOperation errors for +/- in plain CSS (divide is
    // allowed so calculations still parse; evaluation rejects the use).
    if is_plain_css(state) && op != UnaryOperator::Divide {
        return Err(scanner
            .error(
                "Operators aren't allowed in plain CSS.",
                Some(scanner.pos().saturating_sub(1)),
                1,
            )
            .into());
    }
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let operand = parse_single(scanner, state)?;
    let span = scanner.span_from(start);
    Ok(UnaryOperationExpression::new(op, operand, span))
}

// Whether the parser runs in plain-CSS mode (Dart's `plainCss`).
fn is_plain_css(state: &StylesheetState<'_>) -> bool {
    matches!(&state.parser_state.syntax, Syntax::Css(_))
}

// ======================================================================
// Prefix parsers — variable, selector
// ======================================================================

/// Consumes a `$name` variable expression.
///
/// Dart: `_variable`. Rejected in plain CSS.
fn variable_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &StylesheetState<'parse>,
) -> ParseResult<'parse, VariableExpression<'parse>> {
    let start = scanner.state();
    scanner.expect_char('$')?;
    let name = identifier_impl(scanner, &state.parser_state, true, false)?;
    if is_plain_css(state) {
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Err(Box::new(error_impl(
            "Sass variables aren't allowed in plain CSS.",
            &span,
        )));
    }
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(VariableExpression::new(name, span, None))
}

/// Consumes the `&` parent-selector expression.
///
/// Dart: `_selector`. Rejected in plain CSS. A second `&` is pushed back so
/// it parses as a following selector, with a no-deprecation warning that
/// `&&` means two copies rather than `and`.
fn selector_expr_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, SelectorExpression<'parse>> {
    if is_plain_css(state) {
        return Err(scanner
            .error(
                "The parent selector isn't allowed in plain CSS.",
                Some(scanner.pos()),
                1,
            )
            .into());
    }
    let start = scanner.state();
    scanner.expect_char('&')?;
    if scanner.scan_char('&') {
        let sel_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        state.warnings.push(ParseTimeWarning {
            deprecation: None,
            message: "In Sass, \"&&\" means two copies of the parent selector. You probably want to use \"and\" instead.".to_string(),
            span: sel_span,
            primary_label: None,
            secondary: vec![],
        });
        scanner.set_position(scanner.pos().saturating_sub(1))?;
    }
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(SelectorExpression::new(span))
}

// ======================================================================
// Prefix parsers — parentheses, string, hash, unicode, ident_like
// ======================================================================

/// Consumes a parenthesized expression: `()`, a map, `(a, b)`, or `(x)`.
///
/// Dart: `parentheses`. Empty parens are an empty undecided list; `key:
/// value` is a map; commas make a comma list wrapped in parentheses.
/// Mirrors `ref/parse.md` span discipline: plain-CSS parens delegate to the
/// CSS parser.
pub(crate) fn parentheses_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, Expression<'parse>> {
    if let Syntax::Css(_) = state.parser_state.syntax {
        return css_parentheses_impl(scanner, state).map(Expression::Parenthesized);
    }
    let was_in_parentheses = state.in_parentheses;
    state.in_parentheses = true;
    let start = scanner.state();
    scanner.expect_char('(')?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let inside = scanner.state();
    if !looking_at_expression_impl(scanner) {
        scanner.expect_char(')')?;
        let list_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        state.in_parentheses = was_in_parentheses;
        return Ok(Expression::List(ListExpression::new(
            vec![],
            ListSeparator::Undecided,
            list_span,
            false,
        )));
    }
    let first = _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: true,
            until: Some(Box::new(|s| s.peek_char(0) == ',' as i32)),
        },
    )?;
    if scanner.scan_char(':') {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let map = map_expr_impl(scanner, state, first, start)?;
        state.in_parentheses = was_in_parentheses;
        return Ok(Expression::Map(map));
    }
    if !scanner.scan_char(',') {
        scanner.expect_char(')')?;
        let paren_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        state.in_parentheses = was_in_parentheses;
        return Ok(Expression::Parenthesized(ParenthesizedExpression::new(
            first, paren_span,
        )));
    }
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let mut expressions = vec![first];
    loop {
        if !looking_at_expression_impl(scanner) {
            break;
        }
        let expr = _expression_impl(
            scanner,
            state,
            ExpressionOpts {
                bracket_list: false,
                single_equals: false,
                consume_newlines: true,
                until: Some(Box::new(|s| s.peek_char(0) == ',' as i32)),
            },
        )?;
        expressions.push(expr);
        if !scanner.scan_char(',') {
            break;
        }
        whitespace_impl(scanner, &mut state.parser_state, true)?;
    }
    let inside_span = span_from_impl(scanner, &state.parser_state, inside)?.file_span()?;
    let list = ListExpression::new(expressions, ListSeparator::Comma, inside_span, false);
    scanner.expect_char(')')?;
    let close_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    state.in_parentheses = was_in_parentheses;
    Ok(Expression::Parenthesized(ParenthesizedExpression::new(
        Expression::List(list),
        close_span,
    )))
}

/// Consumes the pairs of a map expression after its first colon.
///
/// Dart: `_map`. Called with `first` (the expression before the colon) and
/// `start` (before the opening paren); each `key: value` pair is parsed with
/// [`expression_until_comma`], and a trailing comma ends the map.
fn map_expr_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    first: Expression<'parse>,
    start: LineScannerState,
) -> ParseResult<'parse, MapExpression<'parse>> {
    let mut pairs: Vec<(Expression<'parse>, Expression<'parse>)> = Vec::new();
    let value = _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: true,
            until: Some(Box::new(|s| s.peek_char(0) == ',' as i32)),
        },
    )?;
    pairs.push((first, value));
    while scanner.scan_char(',') {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        if !looking_at_expression_impl(scanner) {
            break;
        }
        let key = _expression_impl(
            scanner,
            state,
            ExpressionOpts {
                bracket_list: false,
                single_equals: false,
                consume_newlines: true,
                until: Some(Box::new(|s| s.peek_char(0) == ',' as i32)),
            },
        )?;
        scanner.expect_char(':')?;
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let val = _expression_impl(
            scanner,
            state,
            ExpressionOpts {
                bracket_list: false,
                single_equals: false,
                consume_newlines: true,
                until: Some(Box::new(|s| s.peek_char(0) == ',' as i32)),
            },
        )?;
        pairs.push((key, val));
    }
    scanner.expect_char(')')?;
    let map_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(MapExpression::new(pairs, map_span))
}

/// Consumes a quoted string expression with interpolation.
///
/// Dart: `interpolatedString`. A backslash-newline is a line continuation, any
/// other escape resolves via `escape_character_impl`, and `#{...}` adds an
/// interpolation. Largely duplicated with `Parser::string` and
/// [`interpolated_string_token_impl`]; changes should be mirrored there.
fn interpolated_string_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, StringExpression<'parse>> {
    let start = scanner.state();
    let quote = scanner.read_char()?;
    if quote != '\'' && quote != '"' {
        return Err(scanner
            .error("Expected string.", Some(start.position), 0)
            .into());
    }
    let mut buffer = InterpolationBuffer::new();
    loop {
        let next = scanner.peek_char(0);
        if next == quote as i32 {
            scanner.read_char()?;
            let str_span = span_from_impl(scanner, &state.parser_state, start)?;
            let interp = buffer.interpolation(str_span).map_err(|e| {
                let sp = span_from_impl(scanner, &state.parser_state, start).unwrap();
                error_impl(&e.to_string(), &sp.file_span().unwrap())
            })?;
            return Ok(StringExpression::new(interp, true));
        }
        if next < 0 || character::is_newline(next as u8 as char) {
            return Err(Box::new(
                scanner.error(&format!("Expected {quote}."), None, 0).into(),
            ));
        }
        if next == '\\' as i32 {
            let second = scanner.peek_char(1);
            if character::is_newline(second as u8 as char) {
                scanner.read_char()?;
                scanner.read_char()?;
                if second == '\r' as i32 {
                    scanner.scan_char('\n');
                }
            } else {
                let ch = escape_character_impl(scanner)?;
                buffer.write_char_code(char::from_u32(ch as u32).unwrap_or('\u{FFFD}'));
            }
        } else if next == '#' as i32 && scanner.peek_char(1) == '{' as i32 {
            let (expr, span) = single_interpolation_impl(scanner, state)?;
            buffer.add(expr, span);
        } else {
            let ch = scanner.read_char()?;
            buffer.write_char_code(ch);
        }
    }
}

/// Consumes an expression starting with `#`.
///
/// Dart: `_hashExpression`. `#{...}` is an identifier-like expression, `#`
/// followed by a digit is a hex color, an identifier that turns out to be
/// hex digits re-parses as a hex color, and anything else is a plain string
/// starting with a literal `#`.
fn hash_expression_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, Expression<'parse>> {
    if scanner.peek_char(1) == '{' as i32 {
        return ident_like(scanner, state);
    }
    let start = scanner.state();
    scanner.expect_char('#')?;
    if character::is_digit(scanner.peek_char(0) as u8 as char) {
        let color = hex_color_contents(scanner, start)?;
        return Ok(Expression::Color(ColorExpression::new(
            color,
            scanner.span_from(start),
        )));
    }
    let after_hash = scanner.state();
    let identifier = interpolated_identifier_impl(scanner, state)?;
    if is_hex_color(&identifier) {
        scanner.set_state(after_hash);
        let color = hex_color_contents(scanner, start)?;
        return Ok(Expression::Color(ColorExpression::new(
            color,
            scanner.span_from(start),
        )));
    }
    let mut buffer = InterpolationBuffer::new();
    buffer.write_char_code('#');
    buffer.add_interpolation(&identifier);
    let hash_span = span_from_impl(scanner, &state.parser_state, start)?;
    let interp = match buffer.interpolation(hash_span) {
        Ok(i) => i,
        Err(e) => {
            let sp = span_from_impl(scanner, &state.parser_state, start)?;
            return Err(Box::new(error_impl(&e.to_string(), &sp.file_span()?)));
        }
    };
    Ok(Expression::String(StringExpression::new(interp, false)))
}

/// Consumes the digits of a hex color after the `#`.
///
/// Dart: `_hexColorContents`. Three digits expand to `0xDD` channels; four or
/// eight digits add an alpha channel normalized to `0..1`. Four- and
/// eight-digit forms keep no preserved format because browser support is
/// still uneven.
fn hex_color_contents<'parse>(
    scanner: &mut SpanScanner<'parse>,
    start: LineScannerState,
) -> ParseResult<'parse, SassColor> {
    let d1 = hex_digit(scanner)?;
    let d2 = hex_digit(scanner)?;
    let d3 = hex_digit(scanner)?;
    let (r, g, b, a) = if !character::is_hex(scanner.peek_char(0) as u8 as char) {
        (
            ((d1 << 4) + d1) as f64,
            ((d2 << 4) + d2) as f64,
            ((d3 << 4) + d3) as f64,
            None,
        )
    } else {
        let d4 = hex_digit(scanner)?;
        if !character::is_hex(scanner.peek_char(0) as u8 as char) {
            (
                ((d1 << 4) + d1) as f64,
                ((d2 << 4) + d2) as f64,
                ((d3 << 4) + d3) as f64,
                Some(((d4 << 4) + d4) as f64 / 255.0),
            )
        } else {
            let d5 = hex_digit(scanner)?;
            let d6 = hex_digit(scanner)?;
            if character::is_hex(scanner.peek_char(0) as u8 as char) {
                let d7 = hex_digit(scanner)?;
                let d8 = hex_digit(scanner)?;
                (
                    ((d1 << 4) + d2) as f64,
                    ((d3 << 4) + d4) as f64,
                    ((d5 << 4) + d6) as f64,
                    Some(((d7 << 4) + d8) as f64 / 255.0),
                )
            } else {
                (
                    ((d1 << 4) + d2) as f64,
                    ((d3 << 4) + d4) as f64,
                    ((d5 << 4) + d6) as f64,
                    None,
                )
            }
        }
    };
    let alpha = a.unwrap_or(1.0);
    // Matches Dart: SassColor.rgbInternal(r, g, b, alpha, SpanColorFormat(span))
    // Matches Go:  NewColorRGBInternal(..., &format) where format is SpanColorFormat
    let format = if a.is_none() {
        let original = scanner
            .substring(start.position, Some(scanner.pos()))
            .to_string();
        Some(ColorFormat::Preserved(original))
    } else {
        None
    };
    Ok(SassColor::rgb_internal(
        Some(r),
        Some(g),
        Some(b),
        Some(alpha),
        format,
    )?)
}

// Dart: `_hexDigit`. Consumes one hexadecimal digit.
fn hex_digit<'parse>(scanner: &mut SpanScanner<'parse>) -> ParseResult<'parse, i32> {
    let ch = scanner.peek_char(0) as u8 as char;
    if character::is_hex(ch) {
        let c = scanner.read_char()?;
        Ok(character::as_hex(c))
    } else {
        Err(Box::new(
            scanner.error("Expected hex digit.", None, 0).into(),
        ))
    }
}

/// Whether `interp` is a plain 3/4/6/8-digit hex string.
///
/// Dart: `_isHexColor`.
fn is_hex_color(interp: &Interpolation) -> bool {
    match interp.as_plain() {
        Some(p) if p.len() == 3 || p.len() == 4 || p.len() == 6 || p.len() == 8 => {
            p.chars().all(character::is_hex)
        }
        _ => false,
    }
}

/// Consumes a `U+...` unicode-range expression.
///
/// Dart: `_unicodeRange`. Parses hex digits with optional `?` wildcards or a
/// `-` range bound (at most 6 digits each); a trailing identifier character
/// after the range is an error.
fn unicode_range_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
) -> ParseResult<'parse, StringExpression<'parse>> {
    let start = scanner.state();
    scanner.read_char()?;
    scanner.expect_char('+')?;
    let mut flen: usize = 0;
    while scanner.peek_char(0) >= 0 && character::is_hex(scanner.peek_char(0) as u8 as char) {
        scanner.read_char()?;
        flen += 1;
    }
    let mut has_q = false;
    while scanner.scan_char('?') {
        has_q = true;
        flen += 1;
    }
    if flen == 0 {
        return Err(scanner
            .error("Expected hex digit or \"?\".", None, 0)
            .into());
    }
    if flen > 6 {
        let err_span = span_from_impl(
            scanner,
            &ParserState {
                syntax: Syntax::Scss,
                interpolation_map: None,
                in_expression: false,
            },
            start,
        )?
        .file_span()?;
        return Err(Box::new(error_impl(
            "Expected at most 6 digits.",
            &err_span,
        )));
    }
    if has_q {
        return Ok(StringExpression::plain(
            scanner.substring(start.position, None),
            scanner.span_from(start),
            false,
        ));
    }
    if scanner.scan_char('-') {
        let s2 = scanner.state();
        let mut s2len: usize = 0;
        while scanner.peek_char(0) >= 0 && character::is_hex(scanner.peek_char(0) as u8 as char) {
            scanner.read_char()?;
            s2len += 1;
        }
        if s2len == 0 {
            return Err(Box::new(
                scanner.error("Expected hex digit.", None, 0).into(),
            ));
        }
        if s2len > 6 {
            let s2span = span_from_impl(
                scanner,
                &ParserState {
                    syntax: Syntax::Scss,
                    interpolation_map: None,
                    in_expression: false,
                },
                s2,
            )?
            .file_span()?;
            return Err(Box::new(error_impl("Expected at most 6 digits.", &s2span)));
        }
    }
    if looking_at_interpolated_identifier_body_impl(scanner) {
        return Err(Box::new(
            scanner.error("Expected end of identifier.", None, 0).into(),
        ));
    }
    Ok(StringExpression::plain(
        scanner.substring(start.position, None),
        scanner.span_from(start),
        false,
    ))
}

/// Consumes an expression that starts like an identifier.
///
/// Dart: `identifierLike`. Dispatches on the plain name: `if(` tries the
/// deprecated Sass `if()` syntax first and falls back to the modern CSS
/// `if()` on failure, `not` builds a unary negation, `false`/`null`/`true`
/// and named colors become literals, and `try_special_function_impl` handles
/// `calc`-like names. Otherwise `.` routes to [`namespaced`] (with
/// interpolation rejected in namespaces), trailing `(` makes a plain or
/// interpolated function call, and the identifier alone is an unquoted string.
fn ident_like<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, Expression<'parse>> {
    if let Syntax::Css(_) = state.parser_state.syntax {
        return css_identifier_like_impl(scanner, state);
    }
    let start = scanner.state();
    let identifier = interpolated_identifier_impl(scanner, state)?;
    let plain = identifier.as_plain().map(|s| s.to_string());
    if let Some(ref p) = plain {
        if p == "if" && scanner.peek_char(0) == '(' as i32 {
            let before_paren = scanner.state();
            match argument_invocation(scanner, state, false, false) {
                Ok(invocation) => {
                    let ident_span = identifier.span()?;
                    let inv_span = invocation.span()?;
                    let inv_as_span: Span = inv_span.into();
                    let legacy_span = ident_span
                        .expand(&inv_as_span)
                        .map_err(|e: SpanError| error_impl(&e.to_string(), &ident_span))?;
                    let expr = LegacyIfExpression::new(invocation, legacy_span);
                    let mut msg =
                        "The Sass if() syntax is deprecated in favor of the modern CSS syntax.\n\n"
                            .to_string();
                    if let Some(sug) = expr.modern_suggestion()? {
                        msg.push_str("Suggestion: ");
                        msg.push_str(&sug);
                        msg.push_str("\n\n");
                    }
                    msg.push_str("More info: https://sass-lang.com/d/if-function");
                    let expr_span = expr.span()?;
                    state.warnings.push(ParseTimeWarning {
                        deprecation: Some(&deprecation::IF_FUNCTION),
                        message: msg,
                        span: expr_span,
                        primary_label: None,
                        secondary: vec![],
                    });
                    return Ok(Expression::LegacyIf(expr));
                }
                Err(_) => {
                    scanner.set_state(before_paren);
                }
            }
        }
        if p.to_lowercase() == "if" && scanner.peek_char(0) == '(' as i32 {
            return Ok(Expression::If(if_expression(scanner, state, start)?));
        }
        if p == "not" {
            whitespace_impl(scanner, &mut state.parser_state, true)?;
            let expr = parse_single(scanner, state)?;
            let not_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
            return Ok(Expression::UnaryOperation(UnaryOperationExpression::new(
                UnaryOperator::Not,
                expr,
                not_span,
            )));
        }
        if scanner.peek_char(0) != '(' as i32 {
            match p.as_str() {
                "false" => {
                    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                    return Ok(Expression::Boolean(BooleanExpression::new(false, span)));
                }
                "null" => {
                    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                    return Ok(Expression::Null(NullExpression::new(span)));
                }
                "true" => {
                    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                    return Ok(Expression::Boolean(BooleanExpression::new(true, span)));
                }
                _ => {}
            }
            if let Some(color) = named_color(&p.to_lowercase()) {
                let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                // Matches Dart: SassColor.rgbInternal(..., SpanColorFormat(identifier.span))
                // Matches Go:  NewColorRGBInternal(..., &SpanColorFormat{Span: span})
                let mut color = color;
                let original_text = scanner
                    .substring(start.position, Some(scanner.pos()))
                    .to_string();
                color.set_format(Some(ColorFormat::Preserved(original_text)));
                return Ok(Expression::Color(ColorExpression::new(color, span)));
            }
        }
        let lower = p.to_lowercase();
        if let Some(expr) = try_special_function_impl(scanner, state, &lower, start)? {
            return Ok(expr);
        }
    }
    match scanner.peek_char(0) {
        _ if scanner.peek_char(0) == '.' as i32 && scanner.peek_char(1) == '.' as i32 => {
            Ok(Expression::String(StringExpression::new(identifier, false)))
        }
        _ if scanner.peek_char(0) == '.' as i32 => {
            scanner.read_char()?;
            if let Some(ref namespace) = plain {
                namespaced(scanner, state, namespace, start)
            } else {
                let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                Err(Box::new(error_impl(
                    "Interpolation isn't allowed in namespaces.",
                    &span,
                )))
            }
        }
        _ if scanner.peek_char(0) == '(' as i32 && plain.is_some() => {
            let p = plain.unwrap();
            let allow_empty = p.to_lowercase() == "var";
            let invocation = argument_invocation(scanner, state, false, allow_empty)?;
            let func_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
            Ok(Expression::Function(FunctionExpression::new(
                p, invocation, func_span, None,
            )))
        }
        _ if scanner.peek_char(0) == '(' as i32 => {
            let invocation = argument_invocation(scanner, state, false, false)?;
            let interp_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
            Ok(Expression::InterpolatedFunction(
                InterpolatedFunctionExpression::new(identifier, invocation, interp_span),
            ))
        }
        _ => Ok(Expression::String(StringExpression::new(identifier, false))),
    }
}

// Looks up a CSS named color (Dart's `colorsByName`).
fn named_color(name: &str) -> Option<SassColor> {
    // Literal color table; a named alias would be single-use indirection.
    #[allow(clippy::type_complexity)]
    static PAIRS: &[(&str, (u8, u8, u8, f64))] = &[
        ("aliceblue", (240, 248, 255, 1.0)),
        ("antiquewhite", (250, 235, 215, 1.0)),
        ("aqua", (0, 255, 255, 1.0)),
        ("aquamarine", (127, 255, 212, 1.0)),
        ("azure", (240, 255, 255, 1.0)),
        ("beige", (245, 245, 220, 1.0)),
        ("bisque", (255, 228, 196, 1.0)),
        ("black", (0, 0, 0, 1.0)),
        ("blue", (0, 0, 255, 1.0)),
        ("blueviolet", (138, 43, 226, 1.0)),
        ("brown", (165, 42, 42, 1.0)),
        ("burlywood", (222, 184, 135, 1.0)),
        ("cadetblue", (95, 158, 160, 1.0)),
        ("chartreuse", (127, 255, 0, 1.0)),
        ("chocolate", (210, 105, 30, 1.0)),
        ("coral", (255, 127, 80, 1.0)),
        ("cornflowerblue", (100, 149, 237, 1.0)),
        ("cornsilk", (255, 248, 220, 1.0)),
        ("crimson", (220, 20, 60, 1.0)),
        ("cyan", (0, 255, 255, 1.0)),
        ("darkblue", (0, 0, 139, 1.0)),
        ("darkcyan", (0, 139, 139, 1.0)),
        ("darkgoldenrod", (184, 134, 11, 1.0)),
        ("darkgray", (169, 169, 169, 1.0)),
        ("darkgreen", (0, 100, 0, 1.0)),
        ("darkgrey", (169, 169, 169, 1.0)),
        ("darkkhaki", (189, 183, 107, 1.0)),
        ("darkmagenta", (139, 0, 139, 1.0)),
        ("darkolivegreen", (85, 107, 47, 1.0)),
        ("darkorange", (255, 140, 0, 1.0)),
        ("darkorchid", (153, 50, 204, 1.0)),
        ("darkred", (139, 0, 0, 1.0)),
        ("darksalmon", (233, 150, 122, 1.0)),
        ("darkseagreen", (143, 188, 143, 1.0)),
        ("darkslateblue", (72, 61, 139, 1.0)),
        ("darkslategray", (47, 79, 79, 1.0)),
        ("darkslategrey", (47, 79, 79, 1.0)),
        ("darkturquoise", (0, 206, 209, 1.0)),
        ("darkviolet", (148, 0, 211, 1.0)),
        ("deeppink", (255, 20, 147, 1.0)),
        ("deepskyblue", (0, 191, 255, 1.0)),
        ("dimgray", (105, 105, 105, 1.0)),
        ("dimgrey", (105, 105, 105, 1.0)),
        ("dodgerblue", (30, 144, 255, 1.0)),
        ("firebrick", (178, 34, 34, 1.0)),
        ("floralwhite", (255, 250, 240, 1.0)),
        ("forestgreen", (34, 139, 34, 1.0)),
        ("fuchsia", (255, 0, 255, 1.0)),
        ("gainsboro", (220, 220, 220, 1.0)),
        ("ghostwhite", (248, 248, 255, 1.0)),
        ("gold", (255, 215, 0, 1.0)),
        ("goldenrod", (218, 165, 32, 1.0)),
        ("gray", (128, 128, 128, 1.0)),
        ("green", (0, 128, 0, 1.0)),
        ("greenyellow", (173, 255, 47, 1.0)),
        ("grey", (128, 128, 128, 1.0)),
        ("honeydew", (240, 255, 240, 1.0)),
        ("hotpink", (255, 105, 180, 1.0)),
        ("indianred", (205, 92, 92, 1.0)),
        ("indigo", (75, 0, 130, 1.0)),
        ("ivory", (255, 255, 240, 1.0)),
        ("khaki", (240, 230, 140, 1.0)),
        ("lavender", (230, 230, 250, 1.0)),
        ("lavenderblush", (255, 240, 245, 1.0)),
        ("lawngreen", (124, 252, 0, 1.0)),
        ("lemonchiffon", (255, 250, 205, 1.0)),
        ("lightblue", (173, 216, 230, 1.0)),
        ("lightcoral", (240, 128, 128, 1.0)),
        ("lightcyan", (224, 255, 255, 1.0)),
        ("lightgoldenrodyellow", (250, 250, 210, 1.0)),
        ("lightgray", (211, 211, 211, 1.0)),
        ("lightgreen", (144, 238, 144, 1.0)),
        ("lightgrey", (211, 211, 211, 1.0)),
        ("lightpink", (255, 182, 193, 1.0)),
        ("lightsalmon", (255, 160, 122, 1.0)),
        ("lightseagreen", (32, 178, 170, 1.0)),
        ("lightskyblue", (135, 206, 250, 1.0)),
        ("lightslategray", (119, 136, 153, 1.0)),
        ("lightslategrey", (119, 136, 153, 1.0)),
        ("lightsteelblue", (176, 196, 222, 1.0)),
        ("lightyellow", (255, 255, 224, 1.0)),
        ("lime", (0, 255, 0, 1.0)),
        ("limegreen", (50, 205, 50, 1.0)),
        ("linen", (250, 240, 230, 1.0)),
        ("magenta", (255, 0, 255, 1.0)),
        ("maroon", (128, 0, 0, 1.0)),
        ("mediumaquamarine", (102, 205, 170, 1.0)),
        ("mediumblue", (0, 0, 205, 1.0)),
        ("mediumorchid", (186, 85, 211, 1.0)),
        ("mediumpurple", (147, 112, 219, 1.0)),
        ("mediumseagreen", (60, 179, 113, 1.0)),
        ("mediumslateblue", (123, 104, 238, 1.0)),
        ("mediumspringgreen", (0, 250, 154, 1.0)),
        ("mediumturquoise", (72, 209, 204, 1.0)),
        ("mediumvioletred", (199, 21, 133, 1.0)),
        ("midnightblue", (25, 25, 112, 1.0)),
        ("mintcream", (245, 255, 250, 1.0)),
        ("mistyrose", (255, 228, 225, 1.0)),
        ("moccasin", (255, 228, 181, 1.0)),
        ("navajowhite", (255, 222, 173, 1.0)),
        ("navy", (0, 0, 128, 1.0)),
        ("oldlace", (253, 245, 230, 1.0)),
        ("olive", (128, 128, 0, 1.0)),
        ("olivedrab", (107, 142, 35, 1.0)),
        ("orange", (255, 165, 0, 1.0)),
        ("orangered", (255, 69, 0, 1.0)),
        ("orchid", (218, 112, 214, 1.0)),
        ("palegoldenrod", (238, 232, 170, 1.0)),
        ("palegreen", (152, 251, 152, 1.0)),
        ("paleturquoise", (175, 238, 238, 1.0)),
        ("palevioletred", (219, 112, 147, 1.0)),
        ("papayawhip", (255, 239, 213, 1.0)),
        ("peachpuff", (255, 218, 185, 1.0)),
        ("peru", (205, 133, 63, 1.0)),
        ("pink", (255, 192, 203, 1.0)),
        ("plum", (221, 160, 221, 1.0)),
        ("powderblue", (176, 224, 230, 1.0)),
        ("purple", (128, 0, 128, 1.0)),
        ("rebeccapurple", (102, 51, 153, 1.0)),
        ("red", (255, 0, 0, 1.0)),
        ("rosybrown", (188, 143, 143, 1.0)),
        ("royalblue", (65, 105, 225, 1.0)),
        ("saddlebrown", (139, 69, 19, 1.0)),
        ("salmon", (250, 128, 114, 1.0)),
        ("sandybrown", (244, 164, 96, 1.0)),
        ("seagreen", (46, 139, 87, 1.0)),
        ("seashell", (255, 245, 238, 1.0)),
        ("sienna", (160, 82, 45, 1.0)),
        ("silver", (192, 192, 192, 1.0)),
        ("skyblue", (135, 206, 235, 1.0)),
        ("slateblue", (106, 90, 205, 1.0)),
        ("slategray", (112, 128, 144, 1.0)),
        ("slategrey", (112, 128, 144, 1.0)),
        ("snow", (255, 250, 250, 1.0)),
        ("springgreen", (0, 255, 127, 1.0)),
        ("steelblue", (70, 130, 180, 1.0)),
        ("tan", (210, 180, 140, 1.0)),
        ("teal", (0, 128, 128, 1.0)),
        ("thistle", (216, 191, 216, 1.0)),
        ("tomato", (255, 99, 71, 1.0)),
        ("transparent", (0, 0, 0, 0.0)),
        ("turquoise", (64, 224, 208, 1.0)),
        ("violet", (238, 130, 238, 1.0)),
        ("wheat", (245, 222, 179, 1.0)),
        ("white", (255, 255, 255, 1.0)),
        ("whitesmoke", (245, 245, 245, 1.0)),
        ("yellow", (255, 255, 0, 1.0)),
        ("yellowgreen", (154, 205, 50, 1.0)),
    ];
    for (n, (r, g, b, a)) in PAIRS {
        if *n == name {
            return Some(SassColor::rgb(*r as f64, *g as f64, *b as f64, *a));
        }
    }
    None
}

/// Consumes a `(...)` argument list: positional, `name: value`, and `...`.
///
/// Dart: `_argumentInvocation`. When `mixin` is true, `=`-style named
/// arguments use a single `=`; `allow_empty_second_arg` accepts `f(x,)`. A
/// named or positional argument after a rest argument keeps parsing but
/// emits the misplaced-rest deprecation; duplicates and positional-after-named
/// are errors.
pub(crate) fn argument_invocation<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    mixin: bool,
    allow_empty_second_arg: bool,
) -> ParseResult<'parse, ArgumentList<'parse>> {
    let start = scanner.state();
    scanner.expect_char('(')?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let mut positional: Vec<Expression<'parse>> = Vec::new();
    let mut named: IndexMap<String, Expression<'parse>> = IndexMap::new();
    let mut named_spans: IndexMap<String, FileSpan<'parse>> = IndexMap::new();
    let mut rest: Option<Expression<'parse>> = None;
    let mut keyword_rest: Option<Expression<'parse>> = None;
    let mut emitted_rest_deprecation = false;
    while looking_at_expression_impl(scanner) {
        let expression = expression_until_comma(scanner, state, !mixin)?;
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        if scanner.scan_char('.') {
            scanner.expect_char('.')?;
            scanner.expect_char('.')?;
            if rest.is_none() {
                rest = Some(expression);
            } else {
                keyword_rest = Some(expression);
                whitespace_impl(scanner, &mut state.parser_state, true)?;
                if scanner.scan_char(',') {
                    whitespace_impl(scanner, &mut state.parser_state, true)?;
                }
                break;
            }
        } else if let Expression::Variable(ref ve) = expression {
            if scanner.scan_char(':') {
                whitespace_impl(scanner, &mut state.parser_state, true)?;
                let name = ve.name.clone();
                if named.contains_key(&name) {
                    let ve_span = ve.span()?;
                    return Err(Box::new(error_impl("Duplicate argument.", &ve_span)));
                }
                let value = expression_until_comma(scanner, state, !mixin)?;
                let expr_span = expression.span()?;
                let value_span = value.span()?;
                let val_as_span: Span = value_span.into();
                let exp_span = expr_span
                    .expand(&val_as_span)
                    .map_err(|e| error_impl(&e.to_string(), &expr_span))?;
                // Deprecation: named argument after rest argument
                if rest.is_some() && !emitted_rest_deprecation {
                    emitted_rest_deprecation = true;
                    let rest_span = rest.as_ref().and_then(|r| r.span().ok());
                    let secondary = rest_span
                        .map(|rs| vec![(rs, "rest argument".to_string())])
                        .unwrap_or_default();
                    state.warnings.push(ParseTimeWarning {
                        deprecation: Some(&MISPLACED_REST),
                        message: "Named arguments must come before rest arguments.\nThis will be an error in Dart Sass 2.0.0.".to_string(),
                        span: exp_span,
                        primary_label: Some("named argument".to_string()),
                        secondary,
                    });
                }
                named_spans.insert(name.clone(), exp_span);
                named.insert(name, value);
            } else {
                if !named.is_empty() {
                    let expr_span = expression.span()?;
                    return Err(Box::new(error_impl(
                        "Positional arguments must come before keyword arguments.",
                        &expr_span,
                    )));
                }
                // Deprecation: positional argument after rest argument
                if rest.is_some() && !emitted_rest_deprecation {
                    emitted_rest_deprecation = true;
                    let expr_span = expression.span()?;
                    let rest_span = rest.as_ref().and_then(|r| r.span().ok());
                    let secondary = rest_span
                        .map(|rs| vec![(rs, "rest argument".to_string())])
                        .unwrap_or_default();
                    state.warnings.push(ParseTimeWarning {
                        deprecation: Some(&MISPLACED_REST),
                        message: "Positional arguments must come before rest arguments.\nThis will be an error in Dart Sass 2.0.0.".to_string(),
                        span: expr_span,
                        primary_label: Some("positional argument".to_string()),
                        secondary,
                    });
                }
                positional.push(expression);
            }
        } else if !named.is_empty() {
            let expr_span = expression.span()?;
            return Err(Box::new(error_impl(
                "Positional arguments must come before keyword arguments.",
                &expr_span,
            )));
        } else {
            // Deprecation: positional argument after rest argument
            if rest.is_some() && !emitted_rest_deprecation {
                emitted_rest_deprecation = true;
                let expr_span = expression.span()?;
                let rest_span = rest.as_ref().and_then(|r| r.span().ok());
                let secondary = rest_span
                    .map(|rs| vec![(rs, "rest argument".to_string())])
                    .unwrap_or_default();
                state.warnings.push(ParseTimeWarning {
                    deprecation: Some(&MISPLACED_REST),
                    message: "Positional arguments must come before rest arguments.\nThis will be an error in Dart Sass 2.0.0.".to_string(),
                    span: expr_span,
                    primary_label: Some("positional argument".to_string()),
                    secondary,
                });
            }
            positional.push(expression);
        }
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        if !scanner.scan_char(',') {
            break;
        }
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        if allow_empty_second_arg
            && positional.len() == 1
            && named.is_empty()
            && rest.is_none()
            && scanner.peek_char(0) == ')' as i32
        {
            positional.push(Expression::String(StringExpression::plain(
                "",
                scanner.empty_span(),
                false,
            )));
            break;
        }
    }
    scanner.expect_char(')')?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(ArgumentList::new(
        positional,
        named,
        named_spans,
        span,
        rest,
        keyword_rest,
    ))
}

/// Consumes an expression, including newlines, up to a top-level comma.
///
/// Dart: `expressionUntilComma` (note Dart's `inlcuding` typo). When
/// `single_equals` is true the Microsoft-style `=` operator is allowed at the
/// top level.
pub(crate) fn expression_until_comma<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    single_equals: bool,
) -> ParseResult<'parse, Expression<'parse>> {
    _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals,
            consume_newlines: true,
            until: Some(Box::new(|s| s.peek_char(0) == ',' as i32)),
        },
    )
}

/// Consumes an expression until a top-level `<`, `>`, or lone `=`.
///
/// Dart: `_expressionUntilComparison`.
pub(crate) fn expression_until_comparison_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, Expression<'parse>> {
    _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: true,
            until: Some(Box::new(|s: &mut SpanScanner<'parse>| {
                let ch = s.peek_char(0);
                if ch == '=' as i32 {
                    s.peek_char(1) != '=' as i32
                } else {
                    ch == '<' as i32 || ch == '>' as i32
                }
            })),
        },
    )
}

/// Consumes a modern CSS-style `if()` expression, `if(cond: a; else: b)`.
///
/// Dart: `ifExpression`, starting after the name. Each branch is
/// `condition: expression` or `else: expression`, separated by `;`.
pub(crate) fn if_expression<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    start: LineScannerState,
) -> ParseResult<'parse, IfExpression<'parse>> {
    scanner.expect_char('(')?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let mut branches: Vec<IfBranch<'parse>> = Vec::new();
    while scanner.peek_char(0) != ')' as i32 {
        let is_else = scan_identifier_impl(scanner, "else", false)?;
        let condition: Option<IfConditionExpression<'parse>> = if !is_else {
            Some(if_condition(scanner, state)?)
        } else {
            None
        };
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        scanner.expect_char(':')?;
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let expr = _expression_impl(
            scanner,
            state,
            ExpressionOpts {
                bracket_list: false,
                single_equals: false,
                consume_newlines: true,
                until: None,
            },
        )?;
        branches.push(IfBranch {
            condition,
            expression: expr,
        });
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        if !scanner.scan_char(';') {
            break;
        }
        whitespace_impl(scanner, &mut state.parser_state, true)?;
    }
    scanner.expect_char(')')?;
    IfExpression::new(branches, scanner.span_from(start))
        .map_err(|e| Box::new(error_impl(&e.to_string(), &scanner.span_from(start))))
}

/// Consumes a modern CSS-style `if()` condition: `not`/group chains joined by
/// one operator kind.
///
/// Dart: `_ifConditionExpression`. `and` and `or` may not mix; consecutive
/// arbitrary-substitution groups collapse into a raw condition via
/// [`if_condition_raw`].
fn if_condition<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, IfConditionExpression<'parse>> {
    let start = scanner.state();
    let is_not = scan_identifier_impl(scanner, "not", false)?;
    if is_not {
        if scanner.peek_char(0) == '(' as i32 {
            return Err(scanner
                .error("Whitespace is required between \"not\" and \"(\"", None, 0)
                .into());
        }
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let group = if_group(scanner, state)?;
        return Ok(IfConditionExpression::Negation(IfConditionNegation::new(
            group,
            scanner.span_from(start),
        )));
    }
    let first = if_group(scanner, state)?;
    let mut groups = vec![first];
    let mut op: Option<IfBoolOp> = None;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    loop {
        let is_and = if op.is_none() || op == Some(IfBoolOp::And) {
            scan_identifier_impl(scanner, "and", false)?
        } else {
            false
        };
        if is_and {
            if scanner.peek_char(0) == '(' as i32 {
                return Err(scanner
                    .error("Whitespace is required between \"and\" and \"(\"", None, 0)
                    .into());
            }
            whitespace_impl(scanner, &mut state.parser_state, true)?;
            if op.is_none() {
                op = Some(IfBoolOp::And);
            }
            groups.push(if_group(scanner, state)?);
        } else {
            let is_or = if op.is_none() || op == Some(IfBoolOp::Or) {
                scan_identifier_impl(scanner, "or", false)?
            } else {
                false
            };
            if is_or {
                if scanner.peek_char(0) == '(' as i32 {
                    return Err(scanner
                        .error("Whitespace is required between \"and\" and \"(\"", None, 0)
                        .into());
                }
                whitespace_impl(scanner, &mut state.parser_state, true)?;
                if op.is_none() {
                    op = Some(IfBoolOp::Or);
                }
                groups.push(if_group(scanner, state)?);
            } else {
                let ch = scanner.peek_char(0);
                if ch != ')' as i32
                    && ch != ':' as i32
                    && ch >= 0
                    && groups
                        .last()
                        .map(|g| g.is_arbitrary_substitution())
                        .unwrap_or(false)
                {
                    let preceding = if groups.len() == 1 {
                        groups.remove(0)
                    } else {
                        IfConditionExpression::Operation(
                            IfConditionOperation::new(groups.clone(), op.unwrap()).map_err(
                                |e| error_impl(&e.to_string(), &scanner.span_from(start)),
                            )?,
                        )
                    };
                    let next = if_group(scanner, state)?;
                    return if_condition_raw(scanner, state, preceding, next);
                }
                if let Some(substitution) = try_arbitrary_substitution(scanner, state)? {
                    let preceding = if groups.len() == 1 {
                        groups.remove(0)
                    } else {
                        IfConditionExpression::Operation(
                            IfConditionOperation::new(groups.clone(), op.unwrap()).map_err(
                                |e| error_impl(&e.to_string(), &scanner.span_from(start)),
                            )?,
                        )
                    };
                    return if_condition_raw(scanner, state, preceding, substitution);
                }
                break;
            }
        }
        whitespace_impl(scanner, &mut state.parser_state, true)?;
    }
    if groups.len() == 1 {
        return Ok(groups.remove(0));
    }
    Ok(IfConditionExpression::Operation(
        IfConditionOperation::new(groups, op.unwrap())
            .map_err(|e| error_impl(&e.to_string(), &scanner.span_from(start)))?,
    ))
}

/// Consumes one grouped `if()` condition: `(cond)`, `sass(...)`, or a function.
///
/// Dart: `_ifGroup`. A lone interpolation with no `(` is a raw condition;
/// `and`/`or`/`not` directly before `(` is an error requiring whitespace.
fn if_group<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, IfConditionExpression<'parse>> {
    let start = scanner.state();
    if scanner.peek_char(0) == '(' as i32 {
        scanner.expect_char('(')?;
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let expr = if_condition(scanner, state)?;
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        scanner.expect_char(')')?;
        return Ok(IfConditionExpression::Parenthesized(
            IfConditionParenthesized::new(expr, scanner.span_from(start)),
        ));
    }
    let is_sass = scan_identifier_impl(scanner, "sass", true)?;
    if is_sass {
        scanner.expect_char('(')?;
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let expr = _expression_impl(
            scanner,
            state,
            ExpressionOpts {
                bracket_list: false,
                single_equals: false,
                consume_newlines: false,
                until: None,
            },
        )?;
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        scanner.expect_char(')')?;
        if is_plain_css(state) {
            return Err(Box::new(error_impl(
                "sass() conditions aren't allowed in plain CSS",
                &scanner.span_from(start),
            )));
        }
        return Ok(IfConditionExpression::Sass(IfConditionSass::new(
            expr,
            scanner.span_from(start),
        )));
    }
    let identifier = interpolated_identifier_impl(scanner, state)?;
    if identifier.contents.len() == 1 {
        if let InterpolationPart::Expression(_) = &identifier.contents[0] {
            if scanner.peek_char(0) != '(' as i32 {
                return Ok(IfConditionExpression::Raw(IfConditionRaw::new(identifier)));
            }
        }
    }
    if let Some(p) = identifier.as_plain() {
        let lower = p.to_lowercase();
        if (lower == "and" || lower == "or" || lower == "not") && scanner.peek_char(0) == '(' as i32
        {
            return Err(scanner
                .error(
                    &format!("Whitespace is required between \"{p}\" and \"(\""),
                    None,
                    0,
                )
                .into());
        }
    }
    scanner.expect_char('(')?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let expression = interpolated_declaration_value_impl(
        scanner,
        state,
        DeclarationValueOpts {
            allow_empty: true,
            allow_semicolon: true,
            allow_colon: true,
            allow_open_brace: true,
            end_after_of: false,
            silent_comments: true,
            consume_newlines: true,
        },
    )?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    scanner.expect_char(')')?;
    Ok(IfConditionExpression::Function(Box::new(
        IfConditionFunction::new(identifier, expression, scanner.span_from(start)),
    )))
}

/// Consumes an `if()`/`var()`/`attr()`/`--x()` arbitrary substitution, or
/// returns `None` when there is none.
///
/// Dart: `_tryArbitrarySubstitution`. `#{...}` is always a raw substitution;
/// otherwise the name must be followed by `(` (the scanner resets when it is
/// not) and the arguments parse as an interpolated declaration value.
fn try_arbitrary_substitution<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, Option<IfConditionExpression<'parse>>> {
    if scanner.peek_char(0) == '#' as i32 && scanner.peek_char(1) == '{' as i32 {
        let (expression, span) = single_interpolation_impl(scanner, state)?;
        let mut buf = InterpolationBuffer::new();
        buf.add(expression, span);
        let raw = buf
            .interpolation(span)
            .map_err(|e| error_impl(&e.to_string(), &span))?;
        return Ok(Some(IfConditionExpression::Raw(IfConditionRaw::new(raw))));
    }

    let start = scanner.state();
    let mut name: Option<Interpolation<'parse>> = None;

    if scan_identifier_impl(scanner, "if", false)? {
        name = Some(Interpolation::plain(
            "if".to_string(),
            scanner.span_from(start),
        ));
    } else if scan_identifier_impl(scanner, "var", false)? {
        name = Some(Interpolation::plain(
            "var".to_string(),
            scanner.span_from(start),
        ));
    } else if scan_identifier_impl(scanner, "attr", false)? {
        name = Some(Interpolation::plain(
            "attr".to_string(),
            scanner.span_from(start),
        ));
    } else if scanner.peek_char(0) == '-' as i32 && scanner.peek_char(1) == '-' as i32 {
        name = Some(interpolated_identifier_impl(scanner, state)?);
    }

    let name = match name {
        Some(n) => n,
        None => {
            scanner.set_state(start);
            return Ok(None);
        }
    };

    if !scanner.scan_char('(') {
        scanner.set_state(start);
        return Ok(None);
    }

    let arguments = interpolated_declaration_value_impl(
        scanner,
        state,
        DeclarationValueOpts {
            allow_empty: true,
            allow_semicolon: true,
            allow_colon: true,
            allow_open_brace: true,
            end_after_of: false,
            silent_comments: true,
            consume_newlines: true,
        },
    )?;
    scanner.expect_char(')')?;

    let span = scanner.span_from(start);
    Ok(Some(IfConditionExpression::Function(Box::new(
        IfConditionFunction::new(name, arguments, span),
    ))))
}

/// Consumes the remainder of a would-be `if()` operation as raw text.
///
/// Dart: `_ifConditionRaw`. Called once `preceding` ends with (or `next` is)
/// an arbitrary substitution; further `and`/`or`/substitution groups append
/// to the raw interpolation instead of building operations.
fn if_condition_raw<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    preceding: IfConditionExpression<'parse>,
    next: IfConditionExpression<'parse>,
) -> ParseResult<'parse, IfConditionExpression<'parse>> {
    let next_subst: IfConditionExpression<'parse>;
    let subst: &dyn AstNode<'parse>;
    if preceding.is_arbitrary_substitution() {
        subst = &preceding;
    } else if let IfConditionExpression::Operation(ref op) = preceding {
        if !op.expressions.is_empty() && op.expressions.last().unwrap().is_arbitrary_substitution()
        {
            subst = op.expressions.last().unwrap() as &dyn AstNode<'parse>;
        } else if next.is_arbitrary_substitution() {
            next_subst = next.clone();
            subst = &next_subst;
        } else {
            panic!("if_condition_raw: either preceding must end with an arbitrary substitution or next must be one");
        }
    } else if next.is_arbitrary_substitution() {
        next_subst = next.clone();
        subst = &next_subst;
    } else {
        panic!("if_condition_raw: either preceding must end with an arbitrary substitution or next must be one");
    }

    let mut buffer = InterpolationBuffer::new();
    let preceding_interp = preceding.to_interpolation(subst)?;
    buffer.add_interpolation(&preceding_interp);
    buffer.write_char_code(' ');
    let next_interp = next.to_interpolation(subst)?;
    buffer.add_interpolation(&next_interp);

    let mut last_group = next;
    let mut op: Option<IfBoolOp> = None;
    if let IfConditionExpression::Operation(ref operation) = preceding {
        op = Some(operation.operator);
    }

    whitespace_impl(scanner, &mut state.parser_state, true)?;
    loop {
        let is_and = if op.is_none() || op != Some(IfBoolOp::Or) {
            scan_identifier_impl(scanner, "and", false)?
        } else {
            false
        };
        if is_and {
            if scanner.peek_char(0) == '(' as i32 {
                return Err(scanner
                    .error("Whitespace is required between \"and\" and \"(\"", None, 0)
                    .into());
            }
            whitespace_impl(scanner, &mut state.parser_state, true)?;
            if op.is_none() {
                op = Some(IfBoolOp::And);
            }
            // Dart: `var lastGroup = _ifGroup()` — shadow, outer not updated
            let lg = if_group(scanner, state)?;
            buffer.write(" and ");
            let last_interp = lg.to_interpolation(subst)?;
            buffer.add_interpolation(&last_interp);
        } else {
            let is_or = if op.is_none() || op != Some(IfBoolOp::And) {
                scan_identifier_impl(scanner, "or", false)?
            } else {
                false
            };
            if is_or {
                if scanner.peek_char(0) == '(' as i32 {
                    return Err(scanner
                        .error("Whitespace is required between \"or\" and \"(\"", None, 0)
                        .into());
                }
                whitespace_impl(scanner, &mut state.parser_state, true)?;
                if op.is_none() {
                    op = Some(IfBoolOp::Or);
                }
                let lg = if_group(scanner, state)?;
                whitespace_impl(scanner, &mut state.parser_state, true)?;
                buffer.write(" or ");
                let last_interp = lg.to_interpolation(subst)?;
                buffer.add_interpolation(&last_interp);
                last_group = lg;
            } else {
                let ch = scanner.peek_char(0);
                if ch != ')' as i32
                    && ch != ':' as i32
                    && ch >= 0
                    && last_group.is_arbitrary_substitution()
                {
                    let lg = if_group(scanner, state)?;
                    buffer.write_char_code(' ');
                    let last_interp = lg.to_interpolation(subst)?;
                    buffer.add_interpolation(&last_interp);
                    last_group = lg;
                } else if let Some(nx) = try_arbitrary_substitution(scanner, state)? {
                    buffer.write_char_code(' ');
                    let last_interp = nx.to_interpolation(subst)?;
                    buffer.add_interpolation(&last_interp);
                    last_group = nx;
                } else {
                    break;
                }
            }
        }
        whitespace_impl(scanner, &mut state.parser_state, true)?;
    }

    let preceding_span = preceding.span()?;
    let start_pos = preceding_span.start_location().offset;
    let end_pos = scanner.state().position;
    let span = scanner.span_from_to(start_pos, end_pos);
    let raw = buffer
        .interpolation(span)
        .map_err(|e| error_impl(&e.to_string(), &span))?;
    Ok(IfConditionExpression::Raw(IfConditionRaw::new(raw)))
}

/// Consumes `name` after a `namespace.` prefix: `$var` or `fn(...)`.
///
/// Dart: `namespacedExpression`. The scanner sits just after the `.` and
/// `start` is the namespace start. Private names are rejected via
/// `_assertPublic`.
pub(crate) fn namespaced<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    namespace: &str,
    start: LineScannerState,
) -> ParseResult<'parse, Expression<'parse>> {
    if scanner.peek_char(0) == '$' as i32 {
        scanner.expect_char('$')?;
        let name = identifier_impl(scanner, &state.parser_state, true, false)?;
        let var_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        let is_private = name.starts_with('-') || name.starts_with('_');
        if is_private {
            return Err(Box::new(error_impl(
                "Private members can't be accessed from outside their modules.",
                &var_span,
            )));
        }
        return Ok(Expression::Variable(VariableExpression::new(
            name,
            var_span,
            Some(namespace.to_string()),
        )));
    }
    let start_id = scanner.state();
    let ident = identifier_impl(scanner, &state.parser_state, true, false)?;
    let is_private = ident.starts_with('-') || ident.starts_with('_');
    let id_span = span_from_impl(scanner, &state.parser_state, start_id)?.file_span()?;
    if is_private {
        return Err(Box::new(error_impl(
            "Private members can't be accessed from outside their modules.",
            &id_span,
        )));
    }
    let invocation = argument_invocation(scanner, state, false, false)?;
    let ns_func_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(Expression::Function(FunctionExpression::new(
        ident,
        invocation,
        ns_func_span,
        Some(namespace.to_string()),
    )))
}

/// If `name` has special function syntax, consumes it; otherwise `None`.
///
/// Dart: `trySpecialFunction`, with `start` before the name. `type(` always
/// parses calc-like; vendor-prefixed `expression()` keeps legacy parsing with
/// a function-name deprecation when the argument is not plain CSS;
/// vendor-prefixed `calc()` and `element()` parse calc-like; `progid:...()`
/// parses an IE filter (vendor-prefixed use warns); `url` tries raw URL
/// contents first.
pub(crate) fn try_special_function_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    name: &str,
    start: LineScannerState,
) -> ParseResult<'parse, Option<Expression<'parse>>> {
    let normalized = unvendor(name);
    let vendored = normalized != name;
    if name == "type" && scanner.scan_char('(') {
        let mut buffer = InterpolationBuffer::new();
        buffer.write(name);
        buffer.write_char_code('(');
        return calc_like(scanner, state, name, start, Some(buffer));
    }
    match normalized.as_str() {
        "expression" => {
            if vendored && scanner.scan_char('(') {
                let mut buffer = InterpolationBuffer::new();
                buffer.write(name);
                buffer.write_char_code('(');
                let before_arg = scanner.state();
                let mut invalid_sass_script = false;
                let mut non_css_sass_script = false;
                // Dart skips the probe entirely for empty `()` (no warning
                // at all); a missing `)` after an argument counts as invalid
                // SassScript rather than propagating (both #2148, via the
                // "prefer interpolation" refactor 548e6604).
                whitespace_impl(scanner, &mut state.parser_state, true)?;
                if !scanner.scan_char(')') {
                    let arg_result = _expression_impl(
                        scanner,
                        state,
                        ExpressionOpts {
                            bracket_list: false,
                            single_equals: false,
                            consume_newlines: false,
                            until: None,
                        },
                    );
                    match arg_result {
                        Err(_) => invalid_sass_script = true,
                        Ok(ref argument) => match argument.is_plain_css(true) {
                            Ok(plain) => non_css_sass_script = !plain,
                            Err(_) => non_css_sass_script = true,
                        },
                    }
                    if scanner.expect_char(')').is_err() {
                        invalid_sass_script = true;
                    }
                }
                scanner.set_state(before_arg);
                let value = interpolated_declaration_value_impl(
                    scanner,
                    state,
                    DeclarationValueOpts {
                        allow_empty: true,
                        allow_semicolon: false,
                        allow_colon: true,
                        allow_open_brace: true,
                        end_after_of: false,
                        silent_comments: true,
                        consume_newlines: false,
                    },
                )?;
                buffer.add_interpolation(&value);
                scanner.expect_char(')')?;
                buffer.write_char_code(')');
                if invalid_sass_script || non_css_sass_script {
                    let suggest_expr = StringExpression::new(value, true);
                    let suggest_interp = suggest_expr
                        .as_interpolation(false, None)
                        .map_err(|e| error_impl(&e.to_string(), &scanner.span_from(start)))?;
                    let suggestion = suggest_interp
                        .as_plain()
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    let mut message = format!(
                        "Vendor-prefixed {}() functions will no longer have special parsing in a future release of Dart Sass. Once that happens, this argument will ",
                        normalized
                    );
                    if invalid_sass_script {
                        message.push_str("no longer be valid syntax. ");
                    } else {
                        message.push_str("be parsed as SassScript. ");
                    }
                    message.push_str(&format!("To preserve current behavior:\n\n{}(", name));
                    message.push_str("#{");
                    message.push_str(&suggestion);
                    message.push_str("})\n\nMore info: https://sass-lang.com/d/function-name");
                    let warn_span =
                        span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                    state.warnings.push(ParseTimeWarning {
                        deprecation: Some(&deprecation::FUNCTION_NAME),
                        message,
                        span: warn_span,
                        primary_label: None,
                        secondary: vec![],
                    });
                }
                let expr_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                let interp = buffer
                    .interpolation(expr_span)
                    .map_err(|e| error_impl(&e.to_string(), &scanner.span_from(start)))?;
                return Ok(Some(Expression::String(StringExpression::new(
                    interp, false,
                ))));
            }
            if !vendored && scanner.scan_char('(') {
                return calc_like(scanner, state, name, start, None);
            }
            Ok(None)
        }
        "calc" => {
            if vendored && scanner.scan_char('(') {
                return calc_like(scanner, state, name, start, None);
            }
            Ok(None)
        }
        "element" => {
            if scanner.scan_char('(') {
                return calc_like(scanner, state, name, start, None);
            }
            Ok(None)
        }
        "progid" => {
            if scanner.scan_char(':') {
                let mut buffer = InterpolationBuffer::new();
                buffer.write(name);
                buffer.write_char_code(':');
                let mut next = scanner.peek_char(0);
                while next >= 0
                    && (character::is_alphabetic(next as u8 as char) || next == '.' as i32)
                {
                    let ch = scanner.read_char()?;
                    buffer.write_char_code(ch);
                    next = scanner.peek_char(0);
                }
                scanner.expect_char('(')?;
                buffer.write_char_code('(');
                let val = interpolated_declaration_value_impl(
                    scanner,
                    state,
                    DeclarationValueOpts {
                        allow_empty: true,
                        allow_semicolon: false,
                        allow_colon: true,
                        allow_open_brace: true,
                        end_after_of: false,
                        silent_comments: true,
                        consume_newlines: false,
                    },
                )?;
                buffer.add_interpolation(&val);
                scanner.expect_char(')')?;
                buffer.write_char_code(')');
                if vendored {
                    let vendor_span =
                        span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                    let interp = buffer
                        .interpolation(vendor_span)
                        .map_err(|e| error_impl(&e.to_string(), &scanner.span_from(start)))?;
                    let suggest_expr = StringExpression::new(interp, true);
                    let suggest_interp = suggest_expr
                        .as_interpolation(false, None)
                        .map_err(|e| error_impl(&e.to_string(), &scanner.span_from(start)))?;
                    let suggestion = suggest_interp.to_display_string().unwrap_or_default();
                    state.warnings.push(ParseTimeWarning {
                        deprecation: Some(&deprecation::FUNCTION_NAME),
                        message: format!(
                            "Vendor-prefixed progid:...() functions will no longer be supported in a future release of Dart Sass. To preserve current behavior:\n\n#{{{}}}\n\nMore info: https://sass-lang.com/d/function-name",
                            suggestion
                        ),
                        span: vendor_span,
                        primary_label: None,
                        secondary: vec![],
                    });
                }
                let progid_span =
                    span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                let interp = buffer
                    .interpolation(progid_span)
                    .map_err(|e| error_impl(&e.to_string(), &scanner.span_from(start)))?;
                return Ok(Some(Expression::String(StringExpression::new(
                    interp, false,
                ))));
            }
            Ok(None)
        }
        "url" => {
            let contents = try_url_contents(scanner, state, start, "", vendored)?;
            if let Some(c) = contents {
                return Ok(Some(Expression::String(StringExpression::new(c, false))));
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

// Consumes the `(args)` tail of a `calc`-like special function into `buffer`
// (Dart: the shared tail of `trySpecialFunction`'s `calc`/`expression`/
// `element` arms).
fn calc_like<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    name: &str,
    start: LineScannerState,
    buffer: Option<InterpolationBuffer<'parse>>,
) -> ParseResult<'parse, Option<Expression<'parse>>> {
    let mut buffer = match buffer {
        Some(b) => b,
        None => {
            let mut b = InterpolationBuffer::new();
            b.write(name);
            b.write_char_code('(');
            b
        }
    };
    let val = interpolated_declaration_value_impl(
        scanner,
        state,
        DeclarationValueOpts {
            allow_empty: true,
            allow_semicolon: false,
            allow_colon: true,
            allow_open_brace: true,
            end_after_of: false,
            silent_comments: true,
            consume_newlines: false,
        },
    )?;
    buffer.add_interpolation(&val);
    scanner.expect_char(')')?;
    buffer.write_char_code(')');
    let calc_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    let interp = buffer
        .interpolation(calc_span)
        .map_err(|e| error_impl(&e.to_string(), &scanner.span_from(start)))?;
    Ok(Some(Expression::String(StringExpression::new(
        interp, false,
    ))))
}

/// Consumes one whitespace-free expression.
///
/// Dart: `_singleExpression`. Dispatches on the next character: signs fold
/// into numbers when a digit or `.` follows (a leading `-` before an
/// identifier-like start is an identifier instead), `/` is always unary,
/// `!important` and lone `%` are plain strings, and anything else routes to
/// the matching prefix parser. New arms must also appear in
/// `looking_at_expression_impl` and `_expression_impl`.
fn parse_single<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, Expression<'parse>> {
    let ch = scanner.peek_char(0);
    if ch < 0 {
        return Err(Box::new(
            scanner.error("Expected expression.", None, 0).into(),
        ));
    }
    match ch {
        _ if ch == '+' as i32 => {
            let n = scanner.peek_char(1);
            if character::is_digit(n as u8 as char) || n == '.' as i32 {
                return Ok(Expression::Number(number_impl(
                    scanner,
                    &state.parser_state,
                )?));
            }
            return Ok(Expression::UnaryOperation(unary(scanner, state)?));
        }
        _ if ch == '-' as i32 => {
            let n = scanner.peek_char(1);
            if character::is_digit(n as u8 as char) || n == '.' as i32 {
                return Ok(Expression::Number(number_impl(
                    scanner,
                    &state.parser_state,
                )?));
            }
            if looking_at_interpolated_identifier_impl(scanner) {
                return ident_like(scanner, state);
            }
            return Ok(Expression::UnaryOperation(unary(scanner, state)?));
        }
        _ if ch == '/' as i32 => {
            return Ok(Expression::UnaryOperation(unary(scanner, state)?));
        }
        _ if ch == '.' as i32 => {
            return Ok(Expression::Number(number_impl(
                scanner,
                &state.parser_state,
            )?));
        }
        _ if ch == '[' as i32 => {
            return _expression_impl(
                scanner,
                state,
                ExpressionOpts {
                    bracket_list: true,
                    single_equals: false,
                    consume_newlines: false,
                    until: None,
                },
            );
        }
        _ if ch == '$' as i32 => {
            return Ok(Expression::Variable(variable_impl(scanner, state)?));
        }
        _ if ch == '&' as i32 => {
            return Ok(Expression::Selector(selector_expr_impl(scanner, state)?));
        }
        _ if ch == '\'' as i32 || ch == '"' as i32 => {
            return Ok(Expression::String(interpolated_string_impl(
                scanner, state,
            )?));
        }
        _ if ch == '#' as i32 => {
            return hash_expression_impl(scanner, state);
        }
        _ if ch == '(' as i32 => {
            return parentheses_impl(scanner, state);
        }
        _ if ch == '!' as i32 => {
            let bang = scanner.state();
            scanner.read_char()?;
            whitespace_impl(scanner, &mut state.parser_state, true)?;
            expect_identifier_impl(scanner, "important", "", true)?;
            return Ok(Expression::String(StringExpression::plain(
                "!important",
                // Matches Dart: spanFrom(start) covers `!important`.
                scanner.span_from(bang),
                false,
            )));
        }
        _ if ch == '%' as i32 => {
            let pct = scanner.state();
            scanner.read_char()?;
            return Ok(Expression::String(StringExpression::plain(
                // Matches Dart: the span covers just `%`.
                "%",
                scanner.span_from_to(pct.position, pct.position + 1),
                false,
            )));
        }
        _ if (ch == 'u' as i32 || ch == 'U' as i32) && scanner.peek_char(1) == '+' as i32 => {
            return Ok(Expression::String(unicode_range_impl(scanner)?));
        }
        _ if character::is_digit(ch as u8 as char) => {
            return Ok(Expression::Number(number_impl(
                scanner,
                &state.parser_state,
            )?));
        }
        _ if character::is_name_start(ch as u8 as char) || ch == '\\' as i32 || ch >= 0x80 => {
            return ident_like(scanner, state);
        }
        _ => {}
    }
    Err(Box::new(
        scanner.error("Expected expression.", None, 0).into(),
    ))
}

// ======================================================================
// iterative bracket tracking (replaces recursive _expression_impl calls)
// ======================================================================

// Saved outer-parser state while a nested `[...]` parses. Dart recurses into
// `_expression(bracketList: true)`; the iterative loop pushes a frame instead
// so the pending operators, operands, and partial lists survive the nesting.
struct BracketFrame<'parse> {
    comma_exprs: Vec<Expression<'parse>>,
    space_exprs: Vec<Expression<'parse>>,
    single: Option<Expression<'parse>>,
    operators: Vec<BinaryOperator>,
    operands: Vec<Expression<'parse>>,
    allow_slash: bool,
    before: LineScannerState,
}

// Builds the bracketed-list result for a closed `[...]`: comma lists keep
// `Comma`, space lists keep `Space`, a lone value is `Undecided`, and `[]` is
// empty `Undecided` — all with `brackets: true`.
fn build_bracket_result<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    ops: &mut Op<'parse>,
    acc: &mut Acc<'parse>,
    bracket_before: LineScannerState,
) -> ParseResult<'parse, Expression<'parse>> {
    resolve_all(scanner, state, ops, acc)?;

    if !acc.comma_exprs.is_empty() {
        resolve_space(scanner, acc)?;
        let mut comma = std::mem::take(&mut acc.comma_exprs);
        if let Some(se) = acc.single.take() {
            comma.push(se);
        }
        let span = span_from_impl(scanner, &state.parser_state, bracket_before)?.file_span()?;
        return Ok(Expression::List(ListExpression::new(
            comma,
            ListSeparator::Comma,
            span,
            true,
        )));
    }

    if !acc.space_exprs.is_empty() {
        let mut sp = std::mem::take(&mut acc.space_exprs);
        if let Some(se) = acc.single.take() {
            sp.push(se);
        }
        let span = span_from_impl(scanner, &state.parser_state, bracket_before)?.file_span()?;
        return Ok(Expression::List(ListExpression::new(
            sp,
            ListSeparator::Space,
            span,
            true,
        )));
    }

    if let Some(single) = acc.single.take() {
        let span = span_from_impl(scanner, &state.parser_state, bracket_before)?.file_span()?;
        return Ok(Expression::List(ListExpression::new(
            vec![single],
            ListSeparator::Undecided,
            span,
            true,
        )));
    }

    let span = span_from_impl(scanner, &state.parser_state, bracket_before)?.file_span()?;
    Ok(Expression::List(ListExpression::new(
        vec![],
        ListSeparator::Undecided,
        span,
        true,
    )))
}

// ======================================================================
// _expression_impl — main Pratt parser entry
// ======================================================================

// Dart: `_expression` with `bracketList`/`singleEquals`/`consumeNewlines`/
// `until`. Parses one prefix expression, then loops: `Acc.single == None`
// selects the unary arms (`+`/`-` number-or-unary, `/` unary); otherwise the
// operator arms resolve precedence via `add_op`, sub-expression arms push via
// `add_single`, `,` folds space lists into comma lists (re-parsing outside
// parens when slash is still allowed, so `(1/2, 1)` does not divide), and
// `and`/`or` scan case-insensitively outside plain CSS. Exits with a comma
// list, a space list, or the single expression.
pub(crate) fn _expression_impl<'r, 'parse>(
    scanner: &'r mut SpanScanner<'parse>,
    state: &'r mut StylesheetState<'parse>,
    mut opts: ExpressionOpts<'r, 'parse>,
) -> ParseResult<'parse, Expression<'parse>>
where
    'parse: 'r,
{
    if let Some(ref mut f) = opts.until {
        if f(scanner) {
            return Err(Box::new(
                scanner.error("Expected expression.", None, 0).into(),
            ));
        }
    }

    let was_bracket_list = opts.bracket_list;
    let mut bracket_stack: Vec<BracketFrame<'parse>> = Vec::new();
    if opts.bracket_list {
        let bb = scanner.state();
        scanner.expect_char('[')?;
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        if scanner.scan_char(']') {
            let span = span_from_impl(scanner, &state.parser_state, bb)?.file_span()?;
            return Ok(Expression::List(ListExpression::new(
                vec![],
                ListSeparator::Undecided,
                span,
                true,
            )));
        }
        bracket_stack.push(BracketFrame {
            comma_exprs: vec![],
            space_exprs: vec![],
            single: None,
            operators: vec![],
            operands: vec![],
            allow_slash: true,
            before: bb,
        });
    }

    let start = scanner.state();
    let was_in_expr = state.parser_state.in_expression;
    state.parser_state.in_expression = true;

    let mut acc = Acc {
        comma_exprs: vec![],
        space_exprs: vec![],
        single: None,
        allow_slash: true,
    };
    let mut ops = Op {
        operators: vec![],
        operands: vec![],
    };
    let ch = scanner.peek_char(0);
    if ch == '[' as i32 {
        let bb = scanner.state();
        scanner.read_char()?;
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        if scanner.scan_char(']') {
            let span = span_from_impl(scanner, &state.parser_state, bb)?.file_span()?;
            acc.single = Some(Expression::List(ListExpression::new(
                vec![],
                ListSeparator::Undecided,
                span,
                true,
            )));
        } else {
            bracket_stack.push(BracketFrame {
                comma_exprs: std::mem::take(&mut acc.comma_exprs),
                space_exprs: std::mem::take(&mut acc.space_exprs),
                single: acc.single.take(),
                operators: std::mem::take(&mut ops.operators),
                operands: std::mem::take(&mut ops.operands),
                allow_slash: acc.allow_slash,
                before: bb,
            });
            acc.allow_slash = true;
        }
    } else {
        acc.single = Some(parse_single(scanner, state)?);
    }

    loop {
        whitespace_impl(
            scanner,
            &mut state.parser_state,
            opts.consume_newlines || !bracket_stack.is_empty(),
        )?;
        // Don't check the until callback when inside a bracketed list.
        // The bracket contents should be parsed as a self-contained
        // expression, and the `until` check at the outer level would
        // otherwise fire on commas inside [...] before `]` is consumed.
        // (Matching Go: _expression never combines `until` with bracket parsing.)
        if bracket_stack.is_empty() {
            if let Some(ref mut f) = opts.until {
                if f(scanner) {
                    break;
                }
            }
        }
        let ch = scanner.peek_char(0);
        if ch < 0 {
            break;
        }

        if ch == ']' as i32 && !bracket_stack.is_empty() {
            scanner.read_char()?;
            let frame = bracket_stack.pop().unwrap();
            let result = build_bracket_result(scanner, state, &mut ops, &mut acc, frame.before)?;
            if bracket_stack.is_empty() && was_bracket_list {
                state.parser_state.in_expression = was_in_expr;
                return Ok(result);
            }
            acc.comma_exprs = frame.comma_exprs;
            acc.space_exprs = frame.space_exprs;
            acc.single = frame.single;
            ops.operators = frame.operators;
            ops.operands = frame.operands;
            acc.allow_slash = frame.allow_slash;
            add_single(scanner, state, &mut ops, &mut acc, result, start)?;
            continue;
        }

        match ch {
            // Unary-first chars (when no current expression)
            _ if ch == '+' as i32 && acc.single.is_none() => {
                let u = unary(scanner, state)?;
                add_single(
                    scanner,
                    state,
                    &mut ops,
                    &mut acc,
                    Expression::UnaryOperation(u),
                    start,
                )?;
            }
            _ if ch == '/' as i32 && acc.single.is_none() => {
                let u = unary(scanner, state)?;
                add_single(
                    scanner,
                    state,
                    &mut ops,
                    &mut acc,
                    Expression::UnaryOperation(u),
                    start,
                )?;
            }
            _ if ch == '-' as i32 && acc.single.is_none() => {
                let n = scanner.peek_char(1);
                if character::is_digit(n as u8 as char) || n == '.' as i32 {
                    let nn = Expression::Number(number_impl(scanner, &state.parser_state)?);
                    add_single(scanner, state, &mut ops, &mut acc, nn, start)?;
                } else if looking_at_interpolated_identifier_impl(scanner) {
                    let expr = ident_like(scanner, state)?;
                    add_single(scanner, state, &mut ops, &mut acc, expr, start)?;
                } else {
                    let u = unary(scanner, state)?;
                    add_single(
                        scanner,
                        state,
                        &mut ops,
                        &mut acc,
                        Expression::UnaryOperation(u),
                        start,
                    )?;
                }
            }
            // Binary operators
            _ if ch == '+' as i32 => {
                scanner.read_char()?;
                let op_start = scanner.pos() - 1;
                add_op(
                    scanner,
                    state,
                    &mut ops,
                    &mut acc,
                    BinaryOperator::Plus,
                    op_start,
                )?;
            }
            _ if ch == '-' as i32 => {
                let next = scanner.peek_char(1);
                let prev = scanner.peek_char(-1);
                if (character::is_digit(next as u8 as char) || next == '.' as i32)
                    && prev >= 0
                    && character::is_whitespace(prev as u8 as char)
                {
                    let nn = Expression::Number(number_impl(scanner, &state.parser_state)?);
                    add_single(scanner, state, &mut ops, &mut acc, nn, start)?;
                } else if looking_at_interpolated_identifier_impl(scanner) {
                    let expr = ident_like(scanner, state)?;
                    add_single(scanner, state, &mut ops, &mut acc, expr, start)?;
                } else {
                    scanner.read_char()?;
                    let op_start = scanner.pos() - 1;
                    add_op(
                        scanner,
                        state,
                        &mut ops,
                        &mut acc,
                        BinaryOperator::Minus,
                        op_start,
                    )?;
                }
            }
            _ if ch == '*' as i32 => {
                scanner.read_char()?;
                let op_start = scanner.pos() - 1;
                add_op(
                    scanner,
                    state,
                    &mut ops,
                    &mut acc,
                    BinaryOperator::Times,
                    op_start,
                )?;
            }
            _ if ch == '/' as i32 => {
                scanner.read_char()?;
                let op_start = scanner.pos() - 1;
                add_op(
                    scanner,
                    state,
                    &mut ops,
                    &mut acc,
                    BinaryOperator::DividedBy,
                    op_start,
                )?;
            }
            _ if ch == '%' as i32 => {
                scanner.read_char()?;
                let op_start = scanner.pos() - 1;
                whitespace_impl(scanner, &mut state.parser_state, true)?;
                if !looking_at_expression_impl(scanner) {
                    let span = scanner.span_from_pos(op_start);
                    let expr = Expression::String(StringExpression::plain("%", span, false));
                    add_single(scanner, state, &mut ops, &mut acc, expr, start)?;
                } else {
                    add_op(
                        scanner,
                        state,
                        &mut ops,
                        &mut acc,
                        BinaryOperator::Modulo,
                        op_start,
                    )?;
                }
            }
            _ if ch == '=' as i32 => {
                scanner.read_char()?;
                let op_start = scanner.pos() - 1;
                let op = if opts.single_equals && scanner.peek_char(0) != '=' as i32 {
                    BinaryOperator::SingleEquals
                } else {
                    scanner.expect_char('=')?;
                    BinaryOperator::Equals
                };
                add_op(scanner, state, &mut ops, &mut acc, op, op_start)?;
            }
            _ if ch == '!' as i32 && scanner.peek_char(1) == '=' as i32 => {
                scanner.read_char()?;
                let op_start = scanner.pos() - 1;
                scanner.read_char()?;
                add_op(
                    scanner,
                    state,
                    &mut ops,
                    &mut acc,
                    BinaryOperator::NotEquals,
                    op_start,
                )?;
            }
            _ if ch == '<' as i32 => {
                scanner.read_char()?;
                let op_start = scanner.pos() - 1;
                let op = if scanner.scan_char('=') {
                    BinaryOperator::LessThanOrEquals
                } else {
                    BinaryOperator::LessThan
                };
                add_op(scanner, state, &mut ops, &mut acc, op, op_start)?;
            }
            _ if ch == '>' as i32 => {
                scanner.read_char()?;
                let op_start = scanner.pos() - 1;
                let op = if scanner.scan_char('=') {
                    BinaryOperator::GreaterThanOrEquals
                } else {
                    BinaryOperator::GreaterThan
                };
                add_op(scanner, state, &mut ops, &mut acc, op, op_start)?;
            }
            // Sub-expressions (push to acc via add_single)
            _ if ch == '(' as i32 => {
                let e = parentheses_impl(scanner, state)?;
                add_single(scanner, state, &mut ops, &mut acc, e, start)?;
            }
            _ if ch == '[' as i32 => {
                let bb = scanner.state();
                scanner.read_char()?;
                whitespace_impl(scanner, &mut state.parser_state, true)?;
                if scanner.scan_char(']') {
                    let span = span_from_impl(scanner, &state.parser_state, bb)?.file_span()?;
                    add_single(
                        scanner,
                        state,
                        &mut ops,
                        &mut acc,
                        Expression::List(ListExpression::new(
                            vec![],
                            ListSeparator::Undecided,
                            span,
                            true,
                        )),
                        start,
                    )?;
                } else {
                    bracket_stack.push(BracketFrame {
                        comma_exprs: std::mem::take(&mut acc.comma_exprs),
                        space_exprs: std::mem::take(&mut acc.space_exprs),
                        single: acc.single.take(),
                        operators: std::mem::take(&mut ops.operators),
                        operands: std::mem::take(&mut ops.operands),
                        allow_slash: acc.allow_slash,
                        before: bb,
                    });
                    acc.allow_slash = true;
                }
            }
            _ if ch == '$' as i32 => {
                let v = variable_impl(scanner, state)?;
                add_single(
                    scanner,
                    state,
                    &mut ops,
                    &mut acc,
                    Expression::Variable(v),
                    start,
                )?;
            }
            _ if ch == '&' as i32 => {
                let s = selector_expr_impl(scanner, state)?;
                add_single(
                    scanner,
                    state,
                    &mut ops,
                    &mut acc,
                    Expression::Selector(s),
                    start,
                )?;
            }
            _ if ch == '\'' as i32 || ch == '"' as i32 => {
                let s = interpolated_string_impl(scanner, state)?;
                add_single(
                    scanner,
                    state,
                    &mut ops,
                    &mut acc,
                    Expression::String(s),
                    start,
                )?;
            }
            _ if ch == '#' as i32 => {
                let e = hash_expression_impl(scanner, state)?;
                add_single(scanner, state, &mut ops, &mut acc, e, start)?;
            }
            _ if ch == '!' as i32 => {
                let n = scanner.peek_char(1);
                if n == '=' as i32 {
                    scanner.read_char()?;
                    let op_start = scanner.pos() - 1;
                    scanner.read_char()?;
                    add_op(
                        scanner,
                        state,
                        &mut ops,
                        &mut acc,
                        BinaryOperator::NotEquals,
                        op_start,
                    )?;
                } else if n == 'i' as i32
                    || n == 'I' as i32
                    || n < 0
                    || character::is_whitespace(n as u8 as char)
                {
                    let bang = scanner.state();
                    scanner.read_char()?;
                    whitespace_impl(scanner, &mut state.parser_state, true)?;
                    expect_identifier_impl(scanner, "important", "", true)?;
                    // Matches Dart's loop `!` arm: span starts at `!`.
                    let span = scanner.span_from(bang);
                    let e = Expression::String(StringExpression::plain("!important", span, false));
                    add_single(scanner, state, &mut ops, &mut acc, e, start)?;
                } else {
                    break;
                }
            }
            _ if character::is_digit(ch as u8 as char) => {
                let nn = Expression::Number(number_impl(scanner, &state.parser_state)?);
                add_single(scanner, state, &mut ops, &mut acc, nn, start)?;
            }
            _ if ch == '.' as i32 && scanner.peek_char(1) == '.' as i32 => break,
            _ if ch == '.' as i32 => {
                let nn = Expression::Number(number_impl(scanner, &state.parser_state)?);
                add_single(scanner, state, &mut ops, &mut acc, nn, start)?;
            }
            _ if (ch == 'u' as i32 || ch == 'U' as i32) && scanner.peek_char(1) == '+' as i32 => {
                let u = unicode_range_impl(scanner)?;
                add_single(
                    scanner,
                    state,
                    &mut ops,
                    &mut acc,
                    Expression::String(u),
                    start,
                )?;
            }
            _ if ch == 'a' as i32 && !is_plain_css(state) => {
                if scan_identifier_impl(scanner, "and", false)? {
                    let op_start = scanner.pos() - 3;
                    add_op(
                        scanner,
                        state,
                        &mut ops,
                        &mut acc,
                        BinaryOperator::And,
                        op_start,
                    )?;
                } else {
                    let e = ident_like(scanner, state)?;
                    add_single(scanner, state, &mut ops, &mut acc, e, start)?;
                }
            }
            _ if ch == 'o' as i32 && !is_plain_css(state) => {
                if scan_identifier_impl(scanner, "or", false)? {
                    let op_start = scanner.pos() - 2;
                    add_op(
                        scanner,
                        state,
                        &mut ops,
                        &mut acc,
                        BinaryOperator::Or,
                        op_start,
                    )?;
                } else {
                    let e = ident_like(scanner, state)?;
                    add_single(scanner, state, &mut ops, &mut acc, e, start)?;
                }
            }
            _ if character::is_name_start(ch as u8 as char) || ch == '\\' as i32 || ch >= 0x80 => {
                let e = ident_like(scanner, state)?;
                add_single(scanner, state, &mut ops, &mut acc, e, start)?;
            }
            // Comma — list separator
            _ if ch == ',' as i32 => {
                if acc.single.is_none() {
                    return Err(Box::new(
                        scanner.error("Expected expression.", None, 0).into(),
                    ));
                }
                resolve_all(scanner, state, &mut ops, &mut acc)?;
                resolve_space(scanner, &mut acc)?;
                acc.comma_exprs.push(acc.single.take().unwrap());
                scanner.read_char()?;
                acc.allow_slash = true;
            }
            _ => break,
        }
    }

    if !bracket_stack.is_empty() {
        if acc.single.is_some() || !acc.space_exprs.is_empty() || !acc.comma_exprs.is_empty() {
            scanner.expect_char(']')?;
        }
        return Err(Box::new(
            scanner.error("Expected expression.", None, 0).into(),
        ));
    }
    resolve_all(scanner, state, &mut ops, &mut acc)?;

    // Comma list resolution
    if !acc.comma_exprs.is_empty() {
        resolve_space(scanner, &mut acc)?;
        let mut comma = acc.comma_exprs.clone();
        if let Some(se) = acc.single.take() {
            comma.push(se);
        }
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        state.parser_state.in_expression = was_in_expr;
        return Ok(Expression::List(ListExpression::new(
            comma,
            ListSeparator::Comma,
            span,
            was_bracket_list,
        )));
    }

    resolve_space(scanner, &mut acc)?;

    state.parser_state.in_expression = was_in_expr;
    Ok(acc.single.unwrap_or(Expression::List(ListExpression::new(
        vec![],
        ListSeparator::Undecided,
        scanner.empty_span(),
        was_bracket_list,
    ))))
}

// ======================================================================
// dynamic_url_impl + interpolated_string_token_impl
// ======================================================================

/// Consumes a `url` token that may contain SassScript.
///
/// Dart: `dynamicUrl`. Uses case-insensitive `url` matching; raw URL contents
/// win when they parse, otherwise this is an interpolated `url(...)` function
/// call.
pub(crate) fn dynamic_url_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, Expression<'parse>> {
    let start = scanner.state();
    // Matches Dart: dynamicUrl uses expectIdentifier("url") with the
    // default caseSensitive=false, so `URL(`/`Url(` parse as url calls.
    expect_identifier_impl(scanner, "url", "", false)?;
    let contents = try_url_contents(scanner, state, start, "url", false)?;
    if let Some(c) = contents {
        return Ok(Expression::String(StringExpression::new(c, false)));
    }
    let invocation = argument_invocation(scanner, state, false, false)?;
    let dyn_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(Expression::InterpolatedFunction(
        InterpolatedFunctionExpression::new(
            Interpolation::plain("url".to_string(), scanner.span_from(start)),
            invocation,
            dyn_span,
        ),
    ))
}

/// Like URL-contents parsing, but returns `None` when the URL does not parse.
///
/// Dart: `_tryUrlContents`, with `start` before the name. Parses a raw
/// `name(...)` URL when possible — Ruby Sass behavior — and backtracks to
/// `None` (re-parse as a function expression) otherwise. A
/// vendor-prefixed call whose argument is not SassScript warns under the
/// function-name deprecation. Largely duplicated with `Parser::try_url`;
/// changes should be mirrored there.
pub(crate) fn try_url_contents<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
    start: LineScannerState,
    name: &str,
    vendored: bool,
) -> ParseResult<'parse, Option<Interpolation<'parse>>> {
    let beginning = scanner.state();
    if !scanner.scan_char('(') {
        return Ok(None);
    }

    let mut invalid_sass_script = false;
    if vendored {
        let before_arg = scanner.state();
        if _expression_impl(
            scanner,
            state,
            ExpressionOpts {
                bracket_list: false,
                single_equals: false,
                consume_newlines: false,
                until: None,
            },
        )
        .is_err()
        {
            invalid_sass_script = true;
        }
        scanner.set_state(before_arg);
    }

    whitespace_without_comments_impl(scanner, &state.parser_state, true)?;
    let mut buffer = InterpolationBuffer::new();
    let n = if name.is_empty() { "url" } else { name };
    buffer.write(n);
    buffer.write_char_code('(');
    loop {
        let ch = scanner.peek_char(0);
        if ch < 0 {
            scanner.set_state(beginning);
            return Ok(None);
        }
        if ch == '\\' as i32 {
            let s = escape_impl(scanner, false)?;
            buffer.write(&s);
        } else if ch == '#' as i32 && scanner.peek_char(1) == '{' as i32 {
            let (expr, span) = single_interpolation_impl(scanner, state)?;
            buffer.add(expr, span);
        } else if ch == '!' as i32
            || ch == '%' as i32
            || ch == '&' as i32
            || ch == '#' as i32
            || (ch >= '*' as i32 && ch <= '~' as i32)
            || ch >= 0x80
        {
            let c = scanner.read_char()?;
            buffer.write_char_code(c);
        } else if character::is_whitespace(ch as u8 as char) {
            whitespace_without_comments_impl(scanner, &state.parser_state, true)?;
            if scanner.peek_char(0) != ')' as i32 {
                scanner.set_state(beginning);
                return Ok(None);
            }
        } else if ch == ')' as i32 {
            let c = scanner.read_char()?;
            buffer.write_char_code(c);
            let url_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
            let result = buffer
                .interpolation(span_from_impl(scanner, &state.parser_state, start)?)
                .map_err(|e| error_impl(&e.to_string(), &url_span))?;

            if vendored && invalid_sass_script {
                let suggest_expr = StringExpression::new(result.clone(), true);
                let suggest_interp = suggest_expr
                    .as_interpolation(false, None)
                    .map_err(|e| error_impl(&e.to_string(), &scanner.span_from(start)))?;
                let suggestion = suggest_interp
                    .as_plain()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                state.warnings.push(ParseTimeWarning {
                    deprecation: Some(&FUNCTION_NAME),
                    message: format!(
                        "Vendor-prefixed url() functions will no longer have special parsing in a future release of Dart Sass. Once that happens, this argument will be parsed as SassScript. To preserve current behavior:\n\nnull(#{{{}}})\n\nMore info: https://sass-lang.com/d/function-name",
                        suggestion
                    ),
                    span: url_span,
                    primary_label: None,
                    secondary: vec![],
                });
            }

            return Ok(Some(result));
        } else {
            scanner.set_state(beginning);
            return Ok(None);
        }
    }
}

/// Consumes a quoted string without interpreting escapes semantically.
///
/// Dart: `interpolatedStringToken`. Keeps the quotes and raw escape text
/// (unlike [`interpolated_string_impl`], which resolves escapes), still
/// processing `#{...}` interpolation. Largely duplicated with
/// `Parser::string` and [`interpolated_string_impl`]; changes should be
/// mirrored there.
pub(crate) fn interpolated_string_token_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut StylesheetState<'parse>,
) -> ParseResult<'parse, Interpolation<'parse>> {
    let start = scanner.state();
    let quote = scanner.read_char()?;
    if quote != '\'' && quote != '"' {
        return Err(scanner
            .error("Expected string.", Some(start.position), 0)
            .into());
    }
    let mut buffer = InterpolationBuffer::new();
    buffer.write_char_code(quote);
    loop {
        let next = scanner.peek_char(0);
        if next == quote as i32 {
            let ch = scanner.read_char()?;
            buffer.write_char_code(ch);
            let tok_span = span_from_impl(scanner, &state.parser_state, start)?;
            return Ok(buffer.interpolation(tok_span).map_err(|e| {
                let sp = span_from_impl(scanner, &state.parser_state, start).unwrap();
                error_impl(&e.to_string(), &sp.file_span().unwrap())
            })?);
        }
        if next < 0 || character::is_newline(next as u8 as char) {
            return Err(Box::new(
                scanner.error(&format!("Expected {quote}."), None, 0).into(),
            ));
        }
        if next == '\\' as i32 {
            let second = scanner.peek_char(1);
            if character::is_newline(second as u8 as char) {
                let ch1 = scanner.read_char()?;
                buffer.write_char_code(ch1);
                let ch2 = scanner.read_char()?;
                buffer.write_char_code(ch2);
                if second == '\r' as i32 && scanner.scan_char('\n') {
                    buffer.write_char_code('\n');
                }
            } else {
                let text = raw_text_impl(scanner, &mut state.parser_state, |sc, _st| {
                    escape_character_impl(sc)?;
                    Ok(())
                })?
                .1;
                buffer.write(&text);
            }
        } else if next == '#' as i32 && scanner.peek_char(1) == '{' as i32 {
            let (expr, span) = single_interpolation_impl(scanner, state)?;
            buffer.add(expr, span);
        } else {
            let ch = scanner.read_char()?;
            buffer.write_char_code(ch);
        }
    }
}

// ======================================================================
// Thin wrappers on StylesheetParser
// ======================================================================

impl<'parse> StylesheetParser<'parse> {
    pub fn _expression(
        &mut self,
        bracket_list: bool,
        single_equals: bool,
        consume_newlines: bool,
    ) -> ParseResult<'parse, Expression<'parse>> {
        _expression_impl(
            &mut self.scanner,
            &mut self.state,
            ExpressionOpts {
                bracket_list,
                single_equals,
                consume_newlines,
                until: None,
            },
        )
    }

    pub fn single_expression(&mut self) -> ParseResult<'parse, Expression<'parse>> {
        parse_single(&mut self.scanner, &mut self.state)
    }

    pub fn expression_until_comma(
        &mut self,
        single_equals: bool,
    ) -> ParseResult<'parse, Expression<'parse>> {
        expression_until_comma(&mut self.scanner, &mut self.state, single_equals)
    }

    pub fn argument_invocation(
        &mut self,
        mixin: bool,
        allow_empty_second_arg: bool,
    ) -> ParseResult<'parse, ArgumentList<'parse>> {
        argument_invocation(
            &mut self.scanner,
            &mut self.state,
            mixin,
            allow_empty_second_arg,
        )
    }

    pub fn expression_until_comparison(&mut self) -> ParseResult<'parse, Expression<'parse>> {
        expression_until_comparison_impl(&mut self.scanner, &mut self.state)
    }

    pub fn dynamic_url(&mut self) -> ParseResult<'parse, Expression<'parse>> {
        dynamic_url_impl(&mut self.scanner, &mut self.state)
    }

    pub fn interpolated_string_token(&mut self) -> ParseResult<'parse, Interpolation<'parse>> {
        interpolated_string_token_impl(&mut self.scanner, &mut self.state)
    }
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::super::parser::ParseError;
    use super::*;
    use crate::common::source_span_file_source::FileSource;
    use crate::parse::parser::ParserState;
    use crate::parse::stylesheet::{CssState, Syntax};
    use bumpalo::Bump;
    use std::collections::{HashMap, HashSet};

    fn make_state<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
    ) -> (SpanScanner<'parse>, StylesheetState<'parse>)
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        let scanner = SpanScanner::new(fs);
        let state = StylesheetState {
            parser_state: ParserState {
                syntax: Syntax::Scss,
                interpolation_map: None,
                in_expression: false,
            },
            parse_selectors: false,
            is_use_allowed: true,
            in_mixin: false,
            in_content_block: false,
            in_control_directive: false,
            in_unknown_at_rule: false,
            in_plain_css_function: false,
            in_style_rule: false,
            in_parentheses: false,
            global_variables: HashMap::new(),
            warnings: vec![],
            last_silent_comment: None,
        };
        (scanner, state)
    }

    #[test]
    fn test_number() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "42");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Number(n) => {
                assert_eq!(n.value, 42.0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_neg_number() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "-42");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Number(n) => {
                assert_eq!(n.value, -42.0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_variable() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "$x");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Variable(v) => {
                assert_eq!(v.name, "x");
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_plus() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 + 2");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::Plus);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_and_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "true and false");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::And);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_comma_list() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1, 2, 3");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::List(le) => {
                assert_eq!(le.separator, ListSeparator::Comma);
                assert_eq!(le.contents.len(), 3);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_parens() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(42)");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Parenthesized(pe) => match &*pe.expression {
                Expression::Number(n) => {
                    assert_eq!(n.value, 42.0);
                }
                _ => {
                    panic!();
                }
            },
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_err_empty() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match *_expression_impl(&mut s, &mut st, o).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "Expected expression.");
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_number_42() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "42", None);
        assert_eq!(
            number_impl(
                &mut SpanScanner::new(fs),
                &ParserState {
                    syntax: Syntax::Scss,
                    interpolation_map: None,
                    in_expression: false
                }
            )
            .unwrap()
            .value,
            42.0
        );
    }

    #[test]
    // 3.14 is the parse subject here, not an approximation of π.
    #[allow(clippy::approx_constant)]
    fn test_number_decimal() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "3.14", None);
        assert_eq!(
            number_impl(
                &mut SpanScanner::new(fs),
                &ParserState {
                    syntax: Syntax::Scss,
                    interpolation_map: None,
                    in_expression: false
                }
            )
            .unwrap()
            .value,
            3.14
        );
    }

    #[test]
    fn test_consume_nat_ok() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "123", None);
        let mut sc = SpanScanner::new(fs);
        consume_natural_number(&mut sc).unwrap();
        assert!(sc.is_done());
    }

    #[test]
    fn test_consume_nat_err() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "abc", None);
        let mut sc = SpanScanner::new(fs);
        match *consume_natural_number(&mut sc).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "Expected digit.");
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_arg_empty() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "()");
        let args = argument_invocation(&mut s, &mut st, false, false).unwrap();
        assert!(args.positional.is_empty());
        assert!(args.named.is_empty());
    }

    #[test]
    fn test_arg_pos() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(1, 2)");
        let args = argument_invocation(&mut s, &mut st, false, false).unwrap();
        assert_eq!(args.positional.len(), 2);
    }

    #[test]
    fn test_string_double_quote() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, r#""hello""#);
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::String(s_str) => {
                assert!(s_str.has_quotes);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_bool_true() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "true");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Boolean(b) => {
                assert!(b.value);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_bool_false() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "false");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Boolean(b) => {
                assert!(!b.value);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_null() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "null");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Null(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_color() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "red");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Color(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_selector() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "&");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Selector(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_important() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "!important");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::String(s_str) => {
                assert_eq!(s_str.text.as_plain(), Some("!important"));
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_percent() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "%");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::String(s_str) => {
                assert_eq!(s_str.text.as_plain(), Some("%"));
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_minus_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 - 2");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::Minus);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_times_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 * 2");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::Times);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_div_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 / 2");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::DividedBy);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_mod_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "3 % 2");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::Modulo);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_eq_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 == 2");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::Equals);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_neq_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 != 2");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::NotEquals);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_lt_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 < 2");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::LessThan);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_lte_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 <= 2");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::LessThanOrEquals);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_gt_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 > 2");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::GreaterThan);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_gte_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 >= 2");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::GreaterThanOrEquals);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_single_eq_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "a = b");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: true,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::SingleEquals);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_or_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "true or false");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::Or);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_unary_div() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "/ 10px");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::UnaryOperation(ue) => {
                assert_eq!(ue.operator, UnaryOperator::Divide);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_unary_plus() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "+ 10");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::UnaryOperation(ue) => {
                assert_eq!(ue.operator, UnaryOperator::Plus);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_unary_minus() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "- 10px");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::UnaryOperation(ue) => {
                assert_eq!(ue.operator, UnaryOperator::Minus);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_not_op() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "not true");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::UnaryOperation(ue) => {
                assert_eq!(ue.operator, UnaryOperator::Not);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_precedence() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 + 2 * 3");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::Plus);
                match &*be.right {
                    Expression::BinaryOperation(right) => {
                        assert_eq!(right.operator, BinaryOperator::Times);
                    }
                    _ => {
                        panic!();
                    }
                };
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_prec_parens() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(1 + 2) * 3");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::Times);
                match &*be.left {
                    Expression::Parenthesized(_) => {}
                    _ => {
                        panic!();
                    }
                };
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_space_list() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 2 3");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::List(le) => {
                assert_eq!(le.separator, ListSeparator::Space);
                assert_eq!(le.contents.len(), 3);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_bracketed_list() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "[1, 2, 3]");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::List(le) => {
                assert!(le.has_brackets);
                assert_eq!(le.contents.len(), 3);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_empty_brackets() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "[]");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::List(le) => {
                assert!(le.has_brackets);
                assert_eq!(le.contents.len(), 0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_map() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(a: 1, b: 2)");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Map(me) => {
                assert_eq!(me.pairs.len(), 2);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_empty_parens() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "()");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::List(le) => {
                assert!(!le.has_brackets);
                assert_eq!(le.contents.len(), 0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_slash_ambiguity() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1/2");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::BinaryOperation(be) => {
                assert!(be.allows_slash());
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_err_missing_operand() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 +");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        assert!(_expression_impl(&mut s, &mut st, o).is_err());
    }

    #[test]
    fn test_err_missing_paren() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(1");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        assert!(_expression_impl(&mut s, &mut st, o).is_err());
    }

    #[test]
    fn test_err_missing_bracket() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "[1");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        assert!(_expression_impl(&mut s, &mut st, o).is_err());
    }

    #[test]
    fn test_err_unclosed_string() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, r#""hello"#);
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match *_expression_impl(&mut s, &mut st, o).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, r#"Expected "."#);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_plain_css_ops() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 < 3");
        st.parser_state.syntax = Syntax::Css(CssState {
            disallowed_function_names: HashSet::new(),
        });
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        assert!(_expression_impl(&mut s, &mut st, o).is_err());
    }

    #[test]
    fn test_plain_css_unary_guard() {
        // Matches Dart: _unaryOperation errors `Operators aren't allowed in
        // plain CSS.` for +/- (divide is exempt).
        for (src, ok) in [("+x", false), ("+ 1", false), ("- 1", false), ("/x", true)] {
            let a = Bump::new();
            let (mut s, mut st) = make_state(&a, src);
            st.parser_state.syntax = Syntax::Css(CssState {
                disallowed_function_names: HashSet::new(),
            });
            let o = ExpressionOpts {
                bracket_list: false,
                single_equals: false,
                consume_newlines: false,
                until: None,
            };
            let res = _expression_impl(&mut s, &mut st, o);
            assert_eq!(res.is_ok(), ok, "src {src}");
            if !ok {
                let err = res.unwrap_err();
                assert!(
                    err.to_string()
                        .contains("Operators aren't allowed in plain CSS."),
                    "src {src}: {err}"
                );
            }
        }
    }

    #[test]
    fn test_important_percent_spans() {
        // Matches Dart: _importantExpression spanFrom(start) covers
        // `!important`; _percentExpression covers just `%`.
        for (src, want) in [("!important", "0-10"), ("%", "0-1")] {
            let a = Bump::new();
            let (mut s, mut st) = make_state(&a, src);
            let o = ExpressionOpts {
                bracket_list: false,
                single_equals: false,
                consume_newlines: false,
                until: None,
            };
            let expr = _expression_impl(&mut s, &mut st, o).unwrap();
            match expr {
                Expression::String(str_expr) => {
                    let span = str_expr.span().unwrap();
                    let got = format!(
                        "{}-{}",
                        span.start_location().offset,
                        span.end_location().offset
                    );
                    assert_eq!(got, want, "src {src}");
                }
                other => panic!("src {src}: expected String, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_plain_css_var() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "$x");
        st.parser_state.syntax = Syntax::Css(CssState {
            disallowed_function_names: HashSet::new(),
        });
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        assert!(_expression_impl(&mut s, &mut st, o).is_err());
    }

    #[test]
    fn test_plain_css_sel() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "&");
        st.parser_state.syntax = Syntax::Css(CssState {
            disallowed_function_names: HashSet::new(),
        });
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        assert!(_expression_impl(&mut s, &mut st, o).is_err());
    }

    #[test]
    fn test_arg_multi_pos() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(1, 2, 3)");
        let args = argument_invocation(&mut s, &mut st, false, false).unwrap();
        assert_eq!(args.positional.len(), 3);
    }

    #[test]
    fn test_arg_named() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "($x: 1)");
        let args = argument_invocation(&mut s, &mut st, false, false).unwrap();
        assert!(args.named.contains_key("x"));
    }

    #[test]
    fn test_arg_mixed() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(1, $x: 2)");
        let args = argument_invocation(&mut s, &mut st, false, false).unwrap();
        assert_eq!(args.positional.len(), 1);
        assert!(args.named.contains_key("x"));
    }

    #[test]
    fn test_arg_rest() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "($list...)");
        let args = argument_invocation(&mut s, &mut st, false, false).unwrap();
        assert!(args.rest.is_some());
    }

    #[test]
    fn test_arg_kwrest() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "($map...)");
        let args = argument_invocation(&mut s, &mut st, false, false).unwrap();
        assert!(args.rest.is_some());
    }

    #[test]
    fn test_arg_dup_err() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "($x: 1, $x: 2)");
        match *argument_invocation(&mut s, &mut st, false, false).unwrap_err() {
            ParseError::Format(f) => {
                assert_eq!(f.message, "Duplicate argument.");
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_arg_pos_after_named() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "($x: 1, 2)");
        match *argument_invocation(&mut s, &mut st, false, false).unwrap_err() {
            ParseError::Format(f) => {
                assert_eq!(
                    f.message,
                    "Positional arguments must come before keyword arguments."
                );
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_arg_empty_second() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(a,)");
        let args = argument_invocation(&mut s, &mut st, false, true).unwrap();
        assert_eq!(args.positional.len(), 2);
    }

    #[test]
    fn test_arg_no_paren() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1");
        assert!(argument_invocation(&mut s, &mut st, false, false).is_err());
    }

    #[test]
    fn test_single_number() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "42");
        match parse_single(&mut s, &mut st).unwrap() {
            Expression::Number(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_single_var() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "$x");
        match parse_single(&mut s, &mut st).unwrap() {
            Expression::Variable(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_single_str() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, r#""hello""#);
        match parse_single(&mut s, &mut st).unwrap() {
            Expression::String(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_single_parens() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(1)");
        match parse_single(&mut s, &mut st).unwrap() {
            Expression::Parenthesized(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_single_empty() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "");
        assert!(parse_single(&mut s, &mut st).is_err());
    }

    #[test]
    fn test_number_neg() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "-42", None);
        assert_eq!(
            number_impl(
                &mut SpanScanner::new(fs),
                &ParserState {
                    syntax: Syntax::Scss,
                    interpolation_map: None,
                    in_expression: false
                }
            )
            .unwrap()
            .value,
            -42.0
        );
    }

    #[test]
    fn test_number_pos() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "+42", None);
        assert_eq!(
            number_impl(
                &mut SpanScanner::new(fs),
                &ParserState {
                    syntax: Syntax::Scss,
                    interpolation_map: None,
                    in_expression: false
                }
            )
            .unwrap()
            .value,
            42.0
        );
    }

    #[test]
    fn test_number_leading_dot() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, ".5", None);
        assert_eq!(
            number_impl(
                &mut SpanScanner::new(fs),
                &ParserState {
                    syntax: Syntax::Scss,
                    interpolation_map: None,
                    in_expression: false
                }
            )
            .unwrap()
            .value,
            0.5
        );
    }

    #[test]
    fn test_number_exp() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "1e3", None);
        assert_eq!(
            number_impl(
                &mut SpanScanner::new(fs),
                &ParserState {
                    syntax: Syntax::Scss,
                    interpolation_map: None,
                    in_expression: false
                }
            )
            .unwrap()
            .value,
            1000.0
        );
    }

    #[test]
    fn test_number_exp_neg() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "1e-3", None);
        assert_eq!(
            number_impl(
                &mut SpanScanner::new(fs),
                &ParserState {
                    syntax: Syntax::Scss,
                    interpolation_map: None,
                    in_expression: false
                }
            )
            .unwrap()
            .value,
            0.001
        );
    }

    #[test]
    fn test_number_exp_pos() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "1e+3", None);
        assert_eq!(
            number_impl(
                &mut SpanScanner::new(fs),
                &ParserState {
                    syntax: Syntax::Scss,
                    interpolation_map: None,
                    in_expression: false
                }
            )
            .unwrap()
            .value,
            1000.0
        );
    }

    #[test]
    fn test_number_pct_unit() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "42%");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Number(n) => {
                assert_eq!(n.value, 42.0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_number_ident_unit() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "42px");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Number(n) => {
                assert_eq!(n.value, 42.0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_number_dot_err() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, ".a", None);
        assert!(number_impl(
            &mut SpanScanner::new(fs),
            &ParserState {
                syntax: Syntax::Scss,
                interpolation_map: None,
                in_expression: false
            }
        )
        .is_err());
    }

    #[test]
    fn test_number_exp_err() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "1e-", None);
        assert!(number_impl(
            &mut SpanScanner::new(fs),
            &ParserState {
                syntax: Syntax::Scss,
                interpolation_map: None,
                in_expression: false
            }
        )
        .is_err());
    }

    #[test]
    fn test_unicode_simple() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "u+1", None);
        let mut sc = SpanScanner::new(fs);
        let se = unicode_range_impl(&mut sc).unwrap();
        assert_eq!(se.text.as_plain(), Some("u+1"));
    }

    #[test]
    fn test_unicode_long() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "u+10ffff", None);
        let mut sc = SpanScanner::new(fs);
        let se = unicode_range_impl(&mut sc).unwrap();
        assert_eq!(se.text.as_plain(), Some("u+10ffff"));
    }

    #[test]
    fn test_unicode_qq() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "u+??", None);
        let mut sc = SpanScanner::new(fs);
        let se = unicode_range_impl(&mut sc).unwrap();
        assert_eq!(se.text.as_plain(), Some("u+??"));
    }

    #[test]
    fn test_unicode_range() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "u+0020-007e", None);
        let mut sc = SpanScanner::new(fs);
        let se = unicode_range_impl(&mut sc).unwrap();
        assert_eq!(se.text.as_plain(), Some("u+0020-007e"));
    }

    #[test]
    fn test_unicode_cap() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "U+1", None);
        let mut sc = SpanScanner::new(fs);
        let se = unicode_range_impl(&mut sc).unwrap();
        assert_eq!(se.text.as_plain(), Some("U+1"));
    }

    #[test]
    fn test_unicode_err_empty() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "u+", None);
        let mut sc = SpanScanner::new(fs);
        match *unicode_range_impl(&mut sc).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, r#"Expected hex digit or "?"."#);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_unicode_too_long() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "u+1234567", None);
        let mut sc = SpanScanner::new(fs);
        assert!(unicode_range_impl(&mut sc).is_err());
    }

    #[test]
    fn test_var_simple() {
        let a = Bump::new();
        let (mut s, st) = make_state(&a, "$x");
        let v = variable_impl(&mut s, &st).unwrap();
        assert_eq!(v.name, "x");
    }

    #[test]
    fn test_var_complex() {
        let a = Bump::new();
        let (mut s, st) = make_state(&a, "$foo-bar");
        let v = variable_impl(&mut s, &st).unwrap();
        assert_eq!(v.name, "foo-bar");
    }

    #[test]
    fn test_var_plain_css() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "$x");
        st.parser_state.syntax = Syntax::Css(CssState {
            disallowed_function_names: HashSet::new(),
        });
        assert!(variable_impl(&mut s, &st).is_err());
    }

    #[test]
    fn test_sel_simple() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "&");
        assert!(selector_expr_impl(&mut s, &mut st).is_ok());
    }

    #[test]
    fn test_sel_double() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "&&");
        let _ = selector_expr_impl(&mut s, &mut st);
        assert!(!st.warnings.is_empty());
    }

    #[test]
    fn test_sel_plain_css() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "&");
        st.parser_state.syntax = Syntax::Css(CssState {
            disallowed_function_names: HashSet::new(),
        });
        assert!(selector_expr_impl(&mut s, &mut st).is_err());
    }

    #[test]
    fn test_str_simple() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, r#""hello""#);
        let s_str = interpolated_string_impl(&mut s, &mut st).unwrap();
        assert!(s_str.has_quotes);
        assert_eq!(s_str.text.as_plain(), Some("hello"));
    }

    #[test]
    fn test_str_single() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "'hello'");
        let s_str = interpolated_string_impl(&mut s, &mut st).unwrap();
        assert_eq!(s_str.text.as_plain(), Some("hello"));
    }

    #[test]
    fn test_str_empty() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, r#""""#);
        let s_str = interpolated_string_impl(&mut s, &mut st).unwrap();
        assert_eq!(s_str.text.as_plain(), Some(""));
    }

    #[test]
    fn test_str_unclosed() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, r#""hello"#);
        assert!(interpolated_string_impl(&mut s, &mut st).is_err());
    }

    #[test]
    fn test_str_escape() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, r#""hello\aworld""#);
        let s_str = interpolated_string_impl(&mut s, &mut st).unwrap();
        assert_eq!(s_str.text.as_plain(), Some("hello\nworld"));
    }

    #[test]
    fn test_str_nl_escape() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "\"hello\\\nworld\"");
        let s_str = interpolated_string_impl(&mut s, &mut st).unwrap();
        assert_eq!(s_str.text.as_plain(), Some("helloworld"));
    }

    #[test]
    fn test_str_tok_simple() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, r#""hello""#);
        let interp = interpolated_string_token_impl(&mut s, &mut st).unwrap();
        assert_eq!(interp.as_plain(), Some(r#""hello""#));
    }

    #[test]
    fn test_str_tok_empty() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, r#""""#);
        let interp = interpolated_string_token_impl(&mut s, &mut st).unwrap();
        assert_eq!(interp.as_plain(), Some(r#""""#));
    }

    #[test]
    fn test_hash_short() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#fff");
        match hash_expression_impl(&mut s, &mut st).unwrap() {
            Expression::Color(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_hash_long() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#ff00ff");
        match hash_expression_impl(&mut s, &mut st).unwrap() {
            Expression::Color(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_hash_alpha() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#ff00ff80");
        match hash_expression_impl(&mut s, &mut st).unwrap() {
            Expression::Color(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_hash_not_color() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#xyz");
        match hash_expression_impl(&mut s, &mut st).unwrap() {
            Expression::String(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_hash_invalid() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#0");
        assert!(hash_expression_impl(&mut s, &mut st).is_err());
    }

    #[test]
    fn test_hash_digit() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#0a0");
        match hash_expression_impl(&mut s, &mut st).unwrap() {
            Expression::Color(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_hex_digit_ok() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "f", None);
        let mut sc = SpanScanner::new(fs);
        assert_eq!(hex_digit(&mut sc).unwrap(), 15);
    }

    #[test]
    fn test_hex_digit_err() {
        let a = Bump::new();
        let fs = FileSource::new_in(&a, "g", None);
        let mut sc = SpanScanner::new(fs);
        match *hex_digit(&mut sc).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "Expected hex digit.");
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_is_hex_3() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#abc");
        s.read_char().unwrap();
        let id = interpolated_identifier_impl(&mut s, &mut st).unwrap();
        assert!(is_hex_color(&id));
    }

    #[test]
    fn test_is_hex_4() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#abcd");
        s.read_char().unwrap();
        let id = interpolated_identifier_impl(&mut s, &mut st).unwrap();
        assert!(is_hex_color(&id));
    }

    #[test]
    fn test_is_hex_6() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#abc123");
        s.read_char().unwrap();
        let id = interpolated_identifier_impl(&mut s, &mut st).unwrap();
        assert!(is_hex_color(&id));
    }

    #[test]
    fn test_is_hex_8() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#abcd1234");
        s.read_char().unwrap();
        let id = interpolated_identifier_impl(&mut s, &mut st).unwrap();
        assert!(is_hex_color(&id));
    }

    #[test]
    fn test_is_hex_bad_len() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#abc12");
        s.read_char().unwrap();
        let id = interpolated_identifier_impl(&mut s, &mut st).unwrap();
        assert!(!is_hex_color(&id));
    }

    #[test]
    fn test_is_hex_non_hex() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#abcz");
        s.read_char().unwrap();
        let id = interpolated_identifier_impl(&mut s, &mut st).unwrap();
        assert!(!is_hex_color(&id));
    }

    #[test]
    fn test_plus_num() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "+10");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Number(n) => {
                assert_eq!(n.value, 10.0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_plus_unary() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "+ $x");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::UnaryOperation(ue) => {
                assert_eq!(ue.operator, UnaryOperator::Plus);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_minus_num() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "-10");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Number(n) => {
                assert_eq!(n.value, -10.0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_minus_ident() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "-foo");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        assert!(_expression_impl(&mut s, &mut st, o).is_ok());
    }

    #[test]
    fn test_unary_op_div() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "/ $x");
        let ue = unary(&mut s, &mut st).unwrap();
        assert_eq!(ue.operator, UnaryOperator::Divide);
    }

    #[test]
    fn test_unary_op_plus() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "+ $x");
        let ue = unary(&mut s, &mut st).unwrap();
        assert_eq!(ue.operator, UnaryOperator::Plus);
    }

    #[test]
    fn test_unary_op_minus() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "- $x");
        let ue = unary(&mut s, &mut st).unwrap();
        assert_eq!(ue.operator, UnaryOperator::Minus);
    }

    #[test]
    fn test_unary_plain_css() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "+ $x");
        st.parser_state.syntax = Syntax::Css(CssState {
            disallowed_function_names: HashSet::new(),
        });
        assert!(unary(&mut s, &mut st).is_err());
    }

    #[test]
    fn test_until_comma() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1, 2");
        match expression_until_comma(&mut s, &mut st, false).unwrap() {
            Expression::Number(n) => {
                assert_eq!(n.value, 1.0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_slash_op_num() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        let expr = _expression_impl(&mut s, &mut st, o).unwrap();
        match &expr {
            Expression::Number(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_slash_op_str() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, r#""hello""#);
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        let expr = _expression_impl(&mut s, &mut st, o).unwrap();
        match &expr {
            Expression::String(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_ident_true() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "true");
        match ident_like(&mut s, &mut st).unwrap() {
            Expression::Boolean(b) => {
                assert!(b.value);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_ident_false() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "false");
        match ident_like(&mut s, &mut st).unwrap() {
            Expression::Boolean(b) => {
                assert!(!b.value);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_ident_null() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "null");
        match ident_like(&mut s, &mut st).unwrap() {
            Expression::Null(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_ident_color() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "red");
        match ident_like(&mut s, &mut st).unwrap() {
            Expression::Color(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_ident_not() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "not x");
        match ident_like(&mut s, &mut st).unwrap() {
            Expression::UnaryOperation(ue) => {
                assert_eq!(ue.operator, UnaryOperator::Not);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_ident_func() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "rgb(1, 2, 3)");
        match ident_like(&mut s, &mut st).unwrap() {
            Expression::Function(fe) => {
                assert_eq!(fe.original_name, "rgb");
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_ident_str() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "foo");
        match ident_like(&mut s, &mut st).unwrap() {
            Expression::String(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_ns_var() {
        let a = Bump::new();
        let input = "ns.$x";
        let (mut s, mut st) = make_state(&a, input);
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Variable(v) => {
                assert_eq!(v.name, "x");
                assert_eq!(v.namespace.as_deref(), Some("ns"));
                assert_eq!(v.span.text(), "ns.$x");
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_ns_func() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "ns.func()");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Function(fe) => {
                assert_eq!(fe.original_name, "func");
                assert_eq!(fe.namespace.as_deref(), Some("ns"));
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_ns_private() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "ns._private");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match *_expression_impl(&mut s, &mut st, o).unwrap_err() {
            ParseError::Format(f) => {
                assert_eq!(
                    f.message,
                    "Private members can't be accessed from outside their modules."
                );
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_try_url_simple() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(foo)");
        let start = s.state();
        let result = try_url_contents(&mut s, &mut st, start, "url", false).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_try_url_empty() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "()");
        let start = s.state();
        let result = try_url_contents(&mut s, &mut st, start, "url", false).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_try_url_no_paren() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "foo");
        let start = s.state();
        let result = try_url_contents(&mut s, &mut st, start, "url", false).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_str_tok_single() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "'world'");
        let interp = interpolated_string_token_impl(&mut s, &mut st).unwrap();
        assert_eq!(interp.as_plain(), Some("'world'"));
    }

    #[test]
    fn test_str_tok_unclosed() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, r#""hello"#);
        match *interpolated_string_token_impl(&mut s, &mut st).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, r#"Expected "."#);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_dyn_url_simple() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "url(foo)");
        match dynamic_url_impl(&mut s, &mut st).unwrap() {
            Expression::String(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_dyn_url_empty() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "url()");
        match dynamic_url_impl(&mut s, &mut st).unwrap() {
            Expression::String(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_dyn_url_args() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "url($a, $b)");
        match dynamic_url_impl(&mut s, &mut st).unwrap() {
            Expression::InterpolatedFunction(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_dyn_url_err() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "url");
        assert!(dynamic_url_impl(&mut s, &mut st).is_err());
    }

    #[test]
    fn test_if_legacy() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "if(1, 2, 3)");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::LegacyIf(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_if_legacy_dep() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "if(1, 2, 3)");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::LegacyIf(_) => {}
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_until_comparison() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 < 2");
        match expression_until_comparison_impl(&mut s, &mut st).unwrap() {
            Expression::Number(n) => {
                assert_eq!(n.value, 1.0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_bracket_list_opts() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "[1, 2]");
        let o = ExpressionOpts {
            bracket_list: true,
            single_equals: false,
            consume_newlines: false,
            until: None,
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::List(le) => {
                assert!(le.has_brackets);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_until_comma_practice() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1, 2, 3");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: Some(Box::new(|s: &mut SpanScanner<'_>| {
                s.peek_char(0) == ',' as i32
            })),
        };
        match _expression_impl(&mut s, &mut st, o).unwrap() {
            Expression::Number(n) => {
                assert_eq!(n.value, 1.0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_until_start_err() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "");
        let o = ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: Some(Box::new(|_: &mut SpanScanner<'_>| true)),
        };
        match *_expression_impl(&mut s, &mut st, o).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "Expected expression.");
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_until_cmp_lt() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 < 2");
        match expression_until_comparison_impl(&mut s, &mut st).unwrap() {
            Expression::Number(n) => {
                assert_eq!(n.value, 1.0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_until_cmp_gt() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 > 2");
        match expression_until_comparison_impl(&mut s, &mut st).unwrap() {
            Expression::Number(n) => {
                assert_eq!(n.value, 1.0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_until_cmp_eq() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 = 2");
        match expression_until_comparison_impl(&mut s, &mut st).unwrap() {
            Expression::Number(n) => {
                assert_eq!(n.value, 1.0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_until_cmp_deq() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "1 == 2");
        match expression_until_comparison_impl(&mut s, &mut st).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::Equals);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_until_cmp_none() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "42");
        match expression_until_comparison_impl(&mut s, &mut st).unwrap() {
            Expression::Number(n) => {
                assert_eq!(n.value, 42.0);
            }
            _ => {
                panic!();
            }
        };
    }

    #[test]
    fn test_until_cmp_ident() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "screen and (color)");
        match expression_until_comparison_impl(&mut s, &mut st).unwrap() {
            Expression::BinaryOperation(be) => {
                assert_eq!(be.operator, BinaryOperator::And);
            }
            _ => {
                panic!();
            }
        };
    }

    // ======================================================================
    // if_group tests
    // ======================================================================

    #[test]
    fn test_if_group_function_env() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "env(--foo)");
        match if_group(&mut s, &mut st).unwrap() {
            IfConditionExpression::Function(f) => {
                assert_eq!(f.name.as_plain().unwrap(), "env");
            }
            _ => panic!("expected IfConditionExpression::Function"),
        };
    }

    #[test]
    fn test_if_group_function_empty() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "foo()");
        match if_group(&mut s, &mut st).unwrap() {
            IfConditionExpression::Function(f) => {
                assert_eq!(f.name.as_plain().unwrap(), "foo");
            }
            _ => panic!("expected IfConditionExpression::Function"),
        };
    }

    #[test]
    fn test_if_group_raw_interpolation() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#{$var}");
        match if_group(&mut s, &mut st).unwrap() {
            IfConditionExpression::Raw(_) => {}
            _ => panic!("expected IfConditionExpression::Raw"),
        };
    }

    #[test]
    fn test_if_group_error_whitespace_required() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "and(x)");
        match *if_group(&mut s, &mut st).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(
                    e.message,
                    "Whitespace is required between \"and\" and \"(\""
                );
            }
            _ => panic!("expected ParseError::Scan"),
        };
    }

    #[test]
    fn test_if_group_error_missing_open_paren() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "foo");
        match *if_group(&mut s, &mut st).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "expected \"(\".");
            }
            _ => panic!("expected ParseError::Scan"),
        };
    }

    #[test]
    fn test_if_group_error_missing_close_paren() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "foo(bar");
        match *if_group(&mut s, &mut st).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "expected \")\".");
            }
            _ => panic!("expected ParseError::Scan"),
        };
    }

    // ======================================================================
    // try_special_function tests
    // ======================================================================

    #[test]
    fn test_try_special_function_type() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "type(text/plain)");
        let start = s.state();
        s.read_char().unwrap();
        s.read_char().unwrap();
        s.read_char().unwrap();
        s.read_char().unwrap();
        let result = try_special_function_impl(&mut s, &mut st, "type", start)
            .unwrap()
            .unwrap();
        match result {
            Expression::String(se) => {
                assert!(!se.has_quotes);
            }
            _ => panic!("expected StringExpression"),
        };
    }

    #[test]
    fn test_try_special_function_type_no_paren() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "type not-a-call");
        let start = s.state();
        s.read_char().unwrap();
        s.read_char().unwrap();
        s.read_char().unwrap();
        s.read_char().unwrap();
        let result = try_special_function_impl(&mut s, &mut st, "type", start).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_try_special_function_calc_vendored() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "-moz-calc(1px + 2px)");
        let start = s.state();
        // consume 9 chars: - m o z - c a l c
        for _ in 0..9 {
            s.read_char().unwrap();
        }
        let result = try_special_function_impl(&mut s, &mut st, "-moz-calc", start)
            .unwrap()
            .unwrap();
        match result {
            Expression::String(se) => {
                assert!(!se.has_quotes);
            }
            _ => panic!("expected StringExpression"),
        };
    }

    #[test]
    fn test_try_special_function_calc_non_vendored() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "calc(1px + 2px)");
        let start = s.state();
        for _ in 0..4 {
            s.read_char().unwrap();
        }
        let result = try_special_function_impl(&mut s, &mut st, "calc", start).unwrap();
        assert!(result.is_none());
    }

    // The vendor-prefixed `expression()` deprecation states the correct
    // future (#2148, via refactor 548e6604): an argument that isn't valid
    // SassScript "will no longer be valid syntax", one that parses but
    // isn't plain CSS "will be parsed as SassScript".
    #[test]
    fn test_try_special_function_expression_invalid_arg() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "-c-expression(@#$)");
        let start = s.state();
        for _ in 0..13 {
            s.read_char().unwrap();
        }
        try_special_function_impl(&mut s, &mut st, "-c-expression", start)
            .unwrap()
            .unwrap();
        assert_eq!(st.warnings.len(), 1);
        assert!(
            st.warnings[0]
                .message
                .contains("this argument will no longer be valid syntax"),
            "got: {}",
            st.warnings[0].message
        );
    }

    #[test]
    fn test_try_special_function_expression_script_like_arg() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "-c-expression($d)");
        let start = s.state();
        for _ in 0..13 {
            s.read_char().unwrap();
        }
        try_special_function_impl(&mut s, &mut st, "-c-expression", start)
            .unwrap()
            .unwrap();
        assert_eq!(st.warnings.len(), 1);
        assert!(
            st.warnings[0]
                .message
                .contains("this argument will be parsed as SassScript"),
            "got: {}",
            st.warnings[0].message
        );
    }

    // Empty `()` skips the probe entirely: no warning (#2148).
    #[test]
    fn test_try_special_function_expression_empty_no_warning() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "-a-expression()");
        let start = s.state();
        for _ in 0..13 {
            s.read_char().unwrap();
        }
        try_special_function_impl(&mut s, &mut st, "-a-expression", start)
            .unwrap()
            .unwrap();
        assert!(st.warnings.is_empty());
    }

    #[test]
    fn test_try_special_function_element() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "element(#foo)");
        let start = s.state();
        for _ in 0..7 {
            s.read_char().unwrap();
        }
        let result = try_special_function_impl(&mut s, &mut st, "element", start)
            .unwrap()
            .unwrap();
        match result {
            Expression::String(se) => {
                assert!(!se.has_quotes);
            }
            _ => panic!("expected StringExpression"),
        };
    }

    #[test]
    fn test_try_special_function_progid() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "progid:DXImageTransform.Microsoft.Alpha(opacity=50)");
        let start = s.state();
        for _ in 0..6 {
            s.read_char().unwrap();
        }
        let result = try_special_function_impl(&mut s, &mut st, "progid", start)
            .unwrap()
            .unwrap();
        match result {
            Expression::String(se) => {
                assert!(!se.has_quotes);
            }
            _ => panic!("expected StringExpression"),
        };
    }

    #[test]
    fn test_try_special_function_url() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "url(http://example.com)");
        let start = s.state();
        for _ in 0..3 {
            s.read_char().unwrap();
        }
        let result = try_special_function_impl(&mut s, &mut st, "url", start)
            .unwrap()
            .unwrap();
        match result {
            Expression::String(se) => {
                assert!(!se.has_quotes);
            }
            _ => panic!("expected StringExpression"),
        };
    }

    #[test]
    fn test_try_special_function_unknown() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "rgb(1, 2, 3)");
        let start = s.state();
        for _ in 0..3 {
            s.read_char().unwrap();
        }
        let result = try_special_function_impl(&mut s, &mut st, "rgb", start).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_try_special_function_type_error_missing_close_paren() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "type(text/plain");
        let start = s.state();
        for _ in 0..4 {
            s.read_char().unwrap();
        }
        match *try_special_function_impl(&mut s, &mut st, "type", start).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "expected \")\".");
            }
            _ => panic!("expected ParseError::Scan"),
        };
    }

    #[test]
    fn test_try_special_function_progid_error_missing_open_paren() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "progid:Foo");
        let start = s.state();
        for _ in 0..6 {
            s.read_char().unwrap();
        }
        match *try_special_function_impl(&mut s, &mut st, "progid", start).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "expected \"(\".");
            }
            _ => panic!("expected ParseError::Scan"),
        };
    }
}
