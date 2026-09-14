// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/stylesheet.dart (supports condition sections)
// go-source: go/value/parse_stylesheet_supports.go

//! Stylesheet-level `@supports` condition parsing.
//!
//! Operators (`and`/`or`/`not`) match case-insensitively per the CSS spec.
//! The core grammar is ambiguous (`Expression ":" Expression` vs an
//! interpolated identifier plus optional value), so the parenthesized pass
//! parses the declaration form first and backtracks to the operation/anything
//! fallbacks on failure.

use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression_if::BooleanOperator;
use crate::ast::sass::expression_string::StringExpression;
use crate::ast::sass::interpolation::{Interpolation, InterpolationPart};
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::ast::sass::supports_condition::anything::SupportsAnything;
use crate::ast::sass::supports_condition::declaration::SupportsDeclaration;
use crate::ast::sass::supports_condition::function::SupportsFunction;
use crate::ast::sass::supports_condition::interpolation::SupportsInterpolation;
use crate::ast::sass::supports_condition::negation::SupportsNegation;
use crate::ast::sass::supports_condition::operation::SupportsOperation;
use crate::ast::sass::supports_condition::SupportsCondition;
use crate::common::ast_node::AstNode;
use crate::common::span_scanner::{LineScannerState, SpanScanner};

use crate::parse::anyvalue::{interpolated_declaration_value_impl, DeclarationValueOpts};
use crate::parse::expression::{ExpressionOpts, _expression_impl};
use crate::parse::identifier::interpolated_identifier_impl;
use crate::parse::parser::{
    error_impl, expect_identifier_impl, looking_at_identifier, scan_identifier_impl,
    span_from_impl, whitespace_impl, ParseError, ParseResult,
};
use crate::parse::stylesheet::StylesheetState;
use crate::parse::util::looking_at_interpolated_identifier_impl;

// ======================================================================
// supports_condition_in_parens_impl
// ======================================================================

// Consumes a parenthesized supports condition, or an interpolation.
//
// An interpolated-identifier lead is a `name(args)` function, a lone `#{…}`
// interpolation, or an error (`"not"` is rejected outright, anything else
// that is neither is `"Expected @supports condition."`). A `(` lead is a
// `not`, a nested condition (re-spanned to the outer parens), or a
// declaration/operation/anything condition parsed with backtracking (see
// the grammar note on the module docs).
pub(crate) fn supports_condition_in_parens_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, SupportsCondition<'b>> {
    let start = scanner.state();

    // Branch A: Interpolated identifier lead
    if looking_at_interpolated_identifier_impl(scanner) {
        let identifier = interpolated_identifier_impl(scanner, state)?;
        if let Some(plain) = identifier.as_plain() {
            if plain.eq_ignore_ascii_case("not") {
                let id_span = identifier
                    .span()
                    .map_err(|e| error_impl(&e.to_string(), &scanner.span_from(start)))?;
                return Err(Box::new(error_impl(
                    "\"not\" is not a valid identifier here.",
                    &id_span,
                )));
            }
        }
        if scanner.scan_char('(') {
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
            let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
            return Ok(SupportsCondition::Function(SupportsFunction::new(
                identifier, arguments, span,
            )));
        } else if identifier.contents.len() == 1 {
            if let InterpolationPart::Expression(expr) = &identifier.contents[0] {
                let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                return Ok(SupportsCondition::Interpolation(
                    SupportsInterpolation::new((**expr).clone(), span),
                ));
            }
        }
        let id_span = identifier
            .span()
            .map_err(|e| error_impl(&e.to_string(), &scanner.span_from(start)))?;
        return Err(Box::new(error_impl(
            "Expected @supports condition.",
            &id_span,
        )));
    }

    // Branch B: Parenthesized condition
    scanner.expect_char('(')?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    if scan_identifier_impl(scanner, "not", false)? {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let condition = supports_condition_in_parens_impl(scanner, state)?;
        scanner.expect_char(')')?;
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(SupportsCondition::Negation(SupportsNegation::new(
            condition, span,
        )));
    } else if scanner.peek_char(0) == '(' as i32 {
        let condition = supports_condition_impl(scanner, state, true)?;
        scanner.expect_char(')')?;
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(condition.with_span(span));
    }

    // Branch C: Expression → Declaration or Unified Fallback
    let name_start = scanner.state();
    let was_in_parentheses = state.in_parentheses;

    let name_err: Box<ParseError<'b>> = match _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: true,
            until: None,
        },
    ) {
        Ok(name) => match scanner.expect_char(':') {
            Ok(()) => {
                let value = supports_declaration_value_impl(scanner, state, &name)?;
                scanner.expect_char(')')?;
                let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                return Ok(SupportsCondition::Declaration(SupportsDeclaration::new(
                    name, value, span,
                )));
            }
            Err(col_err) => col_err.into(),
        },
        Err(expr_err) => expr_err,
    };

    // Unified fallback — matches Dart's `on FormatException catch (_)` block
    scanner.set_state(name_start);
    state.in_parentheses = was_in_parentheses;

    let identifier = interpolated_identifier_impl(scanner, state)?;
    if let Some(op) = try_supports_operation_impl(scanner, state, &identifier, name_start)? {
        scanner.expect_char(')')?;
        let op_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(SupportsCondition::Operation(op.with_span(op_span)));
    }

    let mut contents = InterpolationBuffer::new();
    contents.add_interpolation(&identifier);
    let dec_value = interpolated_declaration_value_impl(
        scanner,
        state,
        DeclarationValueOpts {
            allow_empty: true,
            allow_semicolon: true,
            allow_colon: false,
            allow_open_brace: true,
            end_after_of: false,
            silent_comments: true,
            consume_newlines: true,
        },
    )?;
    contents.add_interpolation(&dec_value);

    if scanner.peek_char(0) == ':' as i32 {
        return Err(name_err);
    }

    scanner.expect_char(')')?;
    let contents_span = span_from_impl(scanner, &state.parser_state, name_start)?.file_span()?;
    let contents_interp = contents
        .interpolation(contents_span)
        .map_err(|e| error_impl(&e.to_string(), &scanner.span_from(name_start)))?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(SupportsCondition::Anything(SupportsAnything::new(
        contents_interp,
        span,
    )))
}

// ======================================================================
// try_supports_operation_impl
// ======================================================================

// If `interpolation` is a lone expression followed by `"and"`/`"or"`,
// parses it as a supports operation (left-associative chain). Otherwise
// returns `None` without moving the scanner. A mismatched operator after the
// first (`and … or …`) is a hard error, not a silent stop.
fn try_supports_operation_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    interpolation: &Interpolation<'b>,
    start: LineScannerState,
) -> ParseResult<'b, Option<SupportsOperation<'b>>> {
    if interpolation.contents.len() != 1 {
        return Ok(None);
    }
    let expr = match &interpolation.contents[0] {
        InterpolationPart::Expression(e) => (**e).clone(),
        _ => return Ok(None),
    };

    let before_whitespace = scanner.state();
    whitespace_impl(scanner, &mut state.parser_state, true)?;

    let mut operation: Option<SupportsOperation<'b>> = None;
    let mut operator: Option<BooleanOperator> = None;
    while looking_at_identifier(scanner, None) {
        if let Some(ref op) = operator {
            expect_identifier_impl(scanner, &op.to_display_string().unwrap(), "", false)?;
        } else if scan_identifier_impl(scanner, "and", false)? {
            operator = Some(BooleanOperator::And);
        } else if scan_identifier_impl(scanner, "or", false)? {
            operator = Some(BooleanOperator::Or);
        } else {
            scanner.set_state(before_whitespace);
            return Ok(None);
        }

        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let right = supports_condition_in_parens_impl(scanner, state)?;
        let interp_span = interpolation
            .span()
            .map_err(|e| error_impl(&e.to_string(), &scanner.span_from(start)))?;
        let op_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;

        let side =
            SupportsCondition::Interpolation(SupportsInterpolation::new(expr.clone(), interp_span));
        operation = Some(if let Some(prev) = operation {
            SupportsOperation::new(
                SupportsCondition::Operation(prev),
                right,
                operator.unwrap(),
                op_span,
            )
        } else {
            SupportsOperation::new(side, right, operator.unwrap(), op_span)
        });
        whitespace_impl(scanner, &mut state.parser_state, true)?;
    }

    Ok(operation)
}

// ======================================================================
// supports_condition_impl
// ======================================================================

// Consumes a `@supports` condition: an optional leading `not`, then a
// left-associative `and`/`or` chain (a repeated operator must match the
// first).
//
// If `in_parentheses` is set, the indented syntax consumes newlines where a
// statement would otherwise end.
pub(crate) fn supports_condition_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    in_parentheses: bool,
) -> ParseResult<'b, SupportsCondition<'b>> {
    let start = scanner.state();
    if scan_identifier_impl(scanner, "not", false)? {
        whitespace_impl(scanner, &mut state.parser_state, in_parentheses)?;
        let condition = supports_condition_in_parens_impl(scanner, state)?;
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(SupportsCondition::Negation(SupportsNegation::new(
            condition, span,
        )));
    }

    let mut condition = supports_condition_in_parens_impl(scanner, state)?;
    whitespace_impl(scanner, &mut state.parser_state, in_parentheses)?;
    let mut operator: Option<BooleanOperator> = None;
    while looking_at_identifier(scanner, None) {
        if let Some(ref op) = operator {
            expect_identifier_impl(scanner, &op.to_display_string().unwrap(), "", false)?;
        } else if scan_identifier_impl(scanner, "or", false)? {
            operator = Some(BooleanOperator::Or);
        } else {
            expect_identifier_impl(scanner, "and", "", false)?;
            operator = Some(BooleanOperator::And);
        }

        whitespace_impl(scanner, &mut state.parser_state, in_parentheses)?;
        let right = supports_condition_in_parens_impl(scanner, state)?;
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        condition = SupportsCondition::Operation(SupportsOperation::new(
            condition,
            right,
            operator.unwrap(),
            span,
        ));
        whitespace_impl(scanner, &mut state.parser_state, in_parentheses)?;
    }
    Ok(condition)
}

// ======================================================================
// import_supports_query_impl
// ======================================================================

// Consumes the contents of a `supports()` function after an `@import` rule
// (but not the function name or parentheses): a leading `not`, a
// parenthesized condition, an interpolated function call, or a bare
// `name: value` declaration.
pub(crate) fn import_supports_query_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, SupportsCondition<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    if scan_identifier_impl(scanner, "not", false)? {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let start = scanner.state();
        let condition = supports_condition_in_parens_impl(scanner, state)?;
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(SupportsCondition::Negation(SupportsNegation::new(
            condition, span,
        )));
    } else if scanner.peek_char(0) == '(' as i32 {
        return supports_condition_impl(scanner, state, true);
    }
    if let Some(fn_cond) = try_import_supports_function_impl(scanner, state)? {
        return Ok(fn_cond);
    }
    let start = scanner.state();
    let name = _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: true,
            until: None,
        },
    )?;
    scanner.expect_char(':')?;
    let value = supports_declaration_value_impl(scanner, state, &name)?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(SupportsCondition::Declaration(SupportsDeclaration::new(
        name, value, span,
    )))
}

// ======================================================================
// try_import_supports_function_impl
// ======================================================================

// Consumes a function call within a `supports()` function after an `@import`
// if available: an interpolated name plus `(...)`. Returns `None` without
// moving the scanner when the input is not a call (restores the start
// position, since the name was already consumed).
fn try_import_supports_function_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Option<SupportsCondition<'b>>> {
    if !looking_at_interpolated_identifier_impl(scanner) {
        return Ok(None);
    }
    let start = scanner.state();
    let name = interpolated_identifier_impl(scanner, state)?;
    if !scanner.scan_char('(') {
        scanner.set_state(start);
        return Ok(None);
    }
    let value = interpolated_declaration_value_impl(
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
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(Some(SupportsCondition::Function(SupportsFunction::new(
        name, value, span,
    ))))
}

// ======================================================================
// supports_declaration_value_impl
// ======================================================================

// Parses and returns the right-hand side of a declaration in a supports
// query.
//
// Unquoted custom-property names (`--*`) take a raw interpolated
// declaration value; anything else parses as a full Sass expression.
fn supports_declaration_value_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    name: &Expression<'b>,
) -> ParseResult<'b, Expression<'b>> {
    if let Expression::String(se) = name {
        if !se.has_quotes && se.text.initial_plain().starts_with("--") {
            let interp = interpolated_declaration_value_impl(
                scanner,
                state,
                DeclarationValueOpts::default(),
            )?;
            return Ok(Expression::String(StringExpression::new(interp, false)));
        }
    }
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: true,
            until: None,
        },
    )
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::source_span_file_source::FileSource;
    use crate::parse::parser::ParserState;
    use crate::parse::stylesheet::Syntax;
    use bumpalo::Bump;
    use std::collections::HashMap;

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
    fn test_import_supports_query_not() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "not (color)");
        match import_supports_query_impl(&mut s, &mut st).unwrap() {
            SupportsCondition::Negation(_) => {}
            _ => panic!("expected SupportsNegation"),
        };
    }

    #[test]
    fn test_import_supports_query_parens_decl() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(display: flex)");
        match import_supports_query_impl(&mut s, &mut st).unwrap() {
            SupportsCondition::Declaration(_) => {}
            _ => panic!("expected SupportsDeclaration"),
        };
    }

    #[test]
    fn test_import_supports_query_function() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "selector(foo)");
        match import_supports_query_impl(&mut s, &mut st).unwrap() {
            SupportsCondition::Function(f) => {
                assert_eq!(f.name.as_plain().unwrap(), "selector");
            }
            _ => panic!("expected SupportsFunction"),
        };
    }

    #[test]
    fn test_import_supports_query_declaration() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "display: flex");
        match import_supports_query_impl(&mut s, &mut st).unwrap() {
            SupportsCondition::Declaration(_) => {}
            _ => panic!("expected SupportsDeclaration"),
        };
    }

    #[test]
    fn test_supports_condition_not() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "not (color)");
        match supports_condition_impl(&mut s, &mut st, false).unwrap() {
            SupportsCondition::Negation(_) => {}
            _ => panic!("expected SupportsNegation"),
        };
    }

    #[test]
    fn test_supports_condition_and() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(color) and (width > 0)");
        match supports_condition_impl(&mut s, &mut st, false).unwrap() {
            SupportsCondition::Operation(op) => {
                assert!(matches!(op.operator, BooleanOperator::And));
            }
            _ => panic!("expected SupportsOperation"),
        };
    }

    #[test]
    fn test_supports_condition_or() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(color) or (monochrome)");
        match supports_condition_impl(&mut s, &mut st, false).unwrap() {
            SupportsCondition::Operation(op) => {
                assert!(matches!(op.operator, BooleanOperator::Or));
            }
            _ => panic!("expected SupportsOperation"),
        };
    }

    #[test]
    fn test_supports_condition_multi_and() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(a) and (b) and (c)");
        match supports_condition_impl(&mut s, &mut st, false).unwrap() {
            SupportsCondition::Operation(_) => {}
            _ => panic!("expected nested SupportsOperation"),
        };
    }

    #[test]
    fn test_supports_function() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "func(args)");
        match supports_condition_impl(&mut s, &mut st, false).unwrap() {
            SupportsCondition::Function(f) => {
                assert_eq!(f.name.as_plain().unwrap(), "func");
            }
            _ => panic!("expected SupportsFunction"),
        };
    }

    #[test]
    fn test_supports_interpolation() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "#{$var}");
        match supports_condition_impl(&mut s, &mut st, false).unwrap() {
            SupportsCondition::Interpolation(_) => {}
            _ => panic!("expected SupportsInterpolation"),
        };
    }

    #[test]
    fn test_supports_parens_not_inside() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(not (color))");
        match supports_condition_impl(&mut s, &mut st, false).unwrap() {
            SupportsCondition::Negation(_) => {}
            _ => panic!("expected SupportsNegation inside parens"),
        };
    }

    #[test]
    fn test_supports_parens_nested() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "((display: flex))");
        match supports_condition_impl(&mut s, &mut st, false).unwrap() {
            SupportsCondition::Declaration(_) => {}
            _ => panic!("expected SupportsDeclaration in nested parens"),
        };
    }

    #[test]
    fn test_supports_declaration() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(display: flex)");
        match supports_condition_impl(&mut s, &mut st, false).unwrap() {
            SupportsCondition::Declaration(_) => {}
            _ => panic!("expected SupportsDeclaration"),
        };
    }

    #[test]
    fn test_supports_declaration_custom_property() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(--custom: value)");
        match supports_condition_impl(&mut s, &mut st, false).unwrap() {
            SupportsCondition::Declaration(_) => {}
            _ => panic!("expected SupportsDeclaration (custom property)"),
        };
    }

    #[test]
    fn test_supports_fallback_anything() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(--foo)");
        match supports_condition_impl(&mut s, &mut st, false).unwrap() {
            SupportsCondition::Anything(_) => {}
            _ => panic!("expected SupportsAnything"),
        };
    }

    #[test]
    fn test_supports_error_not_as_identifier() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "not(args)");
        match *supports_condition_in_parens_impl(&mut s, &mut st).unwrap_err() {
            ParseError::Format(f) => {
                assert_eq!(f.message, "\"not\" is not a valid identifier here.");
            }
            _ => {
                let e = supports_condition_in_parens_impl(&mut s, &mut st).unwrap_err();
                panic!("expected ParseError::Format, got {e:?}");
            }
        };
    }

    #[test]
    fn test_supports_error_missing_close_paren() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "(color");
        match *supports_condition_impl(&mut s, &mut st, false).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "expected \")\".");
            }
            _ => panic!("expected ParseError::Scan"),
        };
    }

    #[test]
    fn test_supports_error_missing_open_paren() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "}");
        match *supports_condition_impl(&mut s, &mut st, false).unwrap_err() {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "expected \"(\".");
            }
            _ => panic!("expected ParseError::Scan"),
        };
    }

    #[test]
    fn test_supports_error_expected_condition() {
        let a = Bump::new();
        let (mut s, mut st) = make_state(&a, "a b");
        match *supports_condition_impl(&mut s, &mut st, false).unwrap_err() {
            ParseError::Format(f) => {
                assert_eq!(f.message, "Expected @supports condition.");
            }
            _ => panic!("expected ParseError::Format"),
        };
    }
}
