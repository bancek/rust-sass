// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/css.dart
// go-source: go/value/parse_css.go

//! Plain-CSS parsing: the restricted at-rule set and CSS-only expression
//! shapes.
//!
//! Covers Dart's `CssParser`, which extends the SCSS parser and narrows it:
//! Sass-only at-rules and interpolation are rejected, `@import` disallows
//! interpolation, and function calls are checked against the disallowed-name
//! set below.

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression_function::FunctionExpression;
use crate::ast::sass::expression_parenthesized::ParenthesizedExpression;
use crate::ast::sass::expression_string::StringExpression;
use crate::ast::sass::import::Import;
use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation::InterpolationPart;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::ast::sass::statement::import_rule::ImportRule;
use crate::ast::sass::statement::Statement;
use crate::ast::sass::static_import::StaticImport;
use crate::common::ast_node::AstNode;
use crate::common::span::Span;
use crate::common::span_scanner::{LineScannerState, SpanScanner};
use crate::common::SassError;
use crate::parse::atrule::media_rule_impl;
use crate::parse::atrule::moz_document_rule_impl;
use crate::parse::atrule::supports_rule_impl;
use crate::parse::atrule::try_import_modifiers_impl;
use crate::parse::atrule::unknown_at_rule_impl;
use crate::parse::expression::if_expression;
use crate::parse::expression::interpolated_string_token_impl;
use crate::parse::expression::namespaced;
use crate::parse::expression::try_special_function_impl;
use crate::parse::stylesheet::expect_statement_separator_impl;
use indexmap::IndexMap;

use crate::parse::anyvalue::almost_any_value_impl;
use crate::parse::expression::{dynamic_url_impl, expression_until_comma};
use crate::parse::identifier::interpolated_identifier_impl;
use crate::parse::parser::{error_impl, span_from_impl, whitespace_impl, ParseError, ParseResult};
use crate::parse::stylesheet::{StylesheetState, Syntax};

// ======================================================================
// CSS atRule (restricted set)
// ======================================================================

/// Consumes an at-rule in plain CSS: the restricted set.
///
/// Sass-only names (`at-root`, `content`, `debug`, `each`, `error`, `extend`,
/// `for`, `if`, `include`, `mixin`, `return`, `warn`, `while`, plus any
/// interpolated name) are rejected; `import`/`function`/`media`/`supports`/
/// `-moz-document` get CSS-specific handling and anything else falls through
/// to the unknown-at-rule parser. Matches Dart's `CssParser.atRule`, whose
/// logic is largely duplicated in `StylesheetParser.atRule` — changes here
/// should be mirrored there.
pub(crate) fn css_at_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    _child: &mut dyn FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Statement<'b>>,
    _root: bool,
) -> ParseResult<'b, Statement<'b>> {
    let start = scanner.state();
    scanner.expect_char('@')?;
    let name = interpolated_identifier_impl(scanner, state)?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    match name.as_plain() {
        None => css_forbidden_at_rule_impl(scanner, state, start),
        Some(plain) => match plain {
            "at-root" | "content" | "debug" | "each" | "error" | "extend" | "for" | "if"
            | "include" | "mixin" | "return" | "warn" | "while" => {
                css_forbidden_at_rule_impl(scanner, state, start)
            }
            "import" => css_import_rule_impl(scanner, state, start).map(Statement::ImportRule),
            "function" => css_function_rule_impl(scanner, state, start, &name),
            "media" => media_rule_impl(scanner, state, start).map(Statement::MediaRule),
            "supports" => supports_rule_impl(scanner, state, start).map(Statement::SupportsRule),
            "-moz-document" => {
                moz_document_rule_impl(scanner, state, start, &name).map(Statement::AtRule)
            }
            _ => unknown_at_rule_impl(scanner, state, start, &name),
        },
    }
}

// ======================================================================
// CSS forbidden atRule
// ======================================================================

// Throws an error for a forbidden at-rule. Consumes the remainder with
// almost_any_value_impl first so the scanner advances past it. Matches Dart's
// `CssParser._forbiddenAtRule`.
fn css_forbidden_at_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, Statement<'b>> {
    let _ = almost_any_value_impl(scanner, state, false)?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Err(Box::new(error_impl(
        "This at-rule isn't allowed in plain CSS.",
        &span,
    )))
}

// ======================================================================
// CSS import rule
// ======================================================================

// Consumes a plain-CSS `@import` rule that disallows interpolation.
// `start` should point before the `@`. A `u`/`U`-leading URL goes through the
// dynamic-url path (interpolated function forms are re-wrapped as
// interpolation; anything else is "Unsupported plain CSS import."); otherwise
// the URL is a static interpolated string. Matches Dart's
// `CssParser._cssImportRule`.
fn css_import_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, ImportRule<'b>> {
    let url_start = scanner.state();
    let ch = scanner.peek_char(0);
    let url = if ch == 'u' as i32 || ch == 'U' as i32 {
        let url_expr = dynamic_url_impl(scanner, state)?;
        if let Expression::String(ref se) = url_expr {
            se.text.clone()
        } else {
            // Try to extract from InterpolatedFunctionExpression
            if let Expression::InterpolatedFunction(ref ife) = url_expr {
                if ife.arguments.positional.len() == 1
                    && ife.arguments.named.is_empty()
                    && ife.arguments.rest.is_none()
                    && ife.arguments.keyword_rest.is_none()
                {
                    if let Expression::String(ref se) = ife.arguments.positional[0] {
                        let mut buf = InterpolationBuffer::new();
                        buf.write(ife.name.as_plain().unwrap_or("url"));
                        buf.write_char_code('(');
                        let interp = se.as_interpolation(false, None).map_err(|e| {
                            let err_span = scanner.span_from(url_start);
                            error_impl(&e.to_string(), &err_span)
                        })?;
                        buf.add_interpolation(&interp);
                        buf.write_char_code(')');
                        let span =
                            span_from_impl(scanner, &state.parser_state, url_start)?.file_span()?;
                        buf.interpolation(span)
                            .map_err(|e| Box::new(ParseError::Sass(e)))?
                    } else {
                        let span = url_expr.span()?;
                        return Err(Box::new(error_impl("Unsupported plain CSS import.", &span)));
                    }
                } else {
                    let span = url_expr.span()?;
                    return Err(Box::new(error_impl("Unsupported plain CSS import.", &span)));
                }
            } else {
                // Wrap as interpolation
                let expr_span = url_expr.span()?;
                Interpolation::new(
                    vec![InterpolationPart::Expression(Box::new(url_expr))],
                    vec![Some(expr_span)],
                    Span::File(expr_span),
                )
                .map_err(|e| Box::new(ParseError::Sass(Box::new(SassError::from(e)))))?
            }
        }
    } else {
        let ist = interpolated_string_token_impl(scanner, state)?;
        ist
    };
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let modifiers = try_import_modifiers_impl(scanner, state)?;
    expect_statement_separator_impl(scanner, state, "@import rule")?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    let import_span = span_from_impl(scanner, &state.parser_state, url_start)?.file_span()?;
    Ok(ImportRule::new(
        vec![Import::Static(StaticImport::new(
            url,
            import_span,
            modifiers,
        ))],
        span,
    ))
}

// ======================================================================
// CSS function rule (-- prefix only)
// ======================================================================

// Consumes a plain CSS function declaration. `start` should point before the
// `@`. A `--`-prefixed name delegates to the unknown-at-rule parser;
// anything else is consumed and rejected as forbidden in plain CSS. Matches
// Dart's `CssParser._cssFunctionRule`.
fn css_function_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
    _at_rule_name: &Interpolation<'b>,
) -> ParseResult<'b, Statement<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    if scanner.peek_char(0) == '-' as i32 && scanner.peek_char(1) == '-' as i32 {
        unknown_at_rule_impl(scanner, state, start, _at_rule_name)
    } else {
        let _ = almost_any_value_impl(scanner, state, false)?;
        css_forbidden_at_rule_impl(scanner, state, start)
    }
}

// ======================================================================
// CSS parentheses (single expression only)
// ======================================================================

/// Parses a parenthesized expression: a single comma-free expression in
/// parens.
///
/// Expressions are only allowed within calculations, but that is verified at
/// evaluation time, not here. Matches Dart's `CssParser.parentheses`.
pub(crate) fn css_parentheses_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, ParenthesizedExpression<'b>> {
    let start = scanner.state();
    scanner.expect_char('(')?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let expression = expression_until_comma(scanner, state, false)?;
    scanner.expect_char(')')?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(ParenthesizedExpression::new(expression, span))
}

// ======================================================================
// CSS identifierLike
// ======================================================================

/// Parses an identifier-like expression in plain CSS.
///
/// The identifier must be plain — interpolation fails with "Interpolation
/// isn't allowed in CSS identifiers." Special functions (`calc`, `url`, …)
/// take priority; `namespace.name` is parsed only to throw the clearer
/// "Module namespaces aren't allowed in plain CSS." error; `if(...)` parses
/// as a CSS `if()`; otherwise a bare identifier becomes an unquoted string
/// and `name(args)` a function call with comma-separated arguments (`var()`
/// allows an empty second argument). Names in the disallowed set fail with
/// "This function isn't allowed in plain CSS." Matches Dart's
/// `CssParser.identifierLike`.
pub(crate) fn css_identifier_like_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Expression<'b>> {
    let start = scanner.state();
    let identifier = interpolated_identifier_impl(scanner, state)?;
    let plain = identifier.as_plain().ok_or_else(|| {
        let span = span_from_impl(scanner, &state.parser_state, start)
            .unwrap_or_else(|_| Span::File(scanner.empty_span()));
        error_impl(
            "Interpolation isn't allowed in CSS identifiers.",
            &span.file_span().unwrap_or(scanner.empty_span()),
        )
    })?;
    let lower = plain.to_lowercase();

    // Try special functions (calc, url, expression, etc.)
    if let Some(special) = try_special_function_impl(scanner, state, &lower, start)? {
        return Ok(special);
    }

    // Namespaced expression check — Dart (css.dart:211) parses the whole
    // namespaced expression (`c.d()`) via `super.namespacedExpression` and then
    // errors on its full span.
    if scanner.peek_char(0) == '.' as i32 {
        scanner.read_char()?;
        let expr = namespaced(scanner, state, &lower, start)?;
        let span = expr.span()?;
        return Err(Box::new(error_impl(
            "Module namespaces aren't allowed in plain CSS.",
            &span,
        )));
    }

    // CSS if()
    if lower == "if" && scanner.peek_char(0) == '(' as i32 {
        return if_expression(scanner, state, start).map(Expression::If);
    }

    // Bare identifier (no function call)
    if scanner.peek_char(0) != '(' as i32 {
        return Ok(Expression::String(StringExpression::new(identifier, false)));
    }

    // Matches Dart: `beforeArguments` is captured after the `(` is scanned.
    let before_arguments = {
        scanner.read_char()?; // consume (
        scanner.state()
    };
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let mut args: Vec<Expression<'b>> = Vec::new();
    let allow_empty_second_arg = lower == "var";
    if scanner.peek_char(0) == ')' as i32 {
        scanner.read_char()?;
    } else {
        loop {
            whitespace_impl(scanner, &mut state.parser_state, true)?;
            if allow_empty_second_arg && args.len() == 1 && scanner.peek_char(0) == ')' as i32 {
                args.push(Expression::String(StringExpression::new(
                    Interpolation::plain(String::new(), scanner.empty_span()),
                    false,
                )));
                break;
            }
            let expr = expression_until_comma(scanner, state, true)?;
            args.push(expr);
            whitespace_impl(scanner, &mut state.parser_state, true)?;
            if !scanner.scan_char(',') {
                break;
            }
        }
        scanner.expect_char(')')?;
    }

    // Check disallowed function names
    if let Syntax::Css(ref css_state) = state.parser_state.syntax {
        if css_state.disallowed_function_names.contains(plain) {
            let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
            return Err(Box::new(error_impl(
                "This function isn't allowed in plain CSS.",
                &span,
            )));
        }
    }

    // Matches Dart: spanFrom(beforeArguments) — the args span starts after
    // the `(` (at the `beforeArguments` state), not at the function name.
    let arg_span = scanner.span_from(before_arguments);
    let arg_list = ArgumentList::new(args, IndexMap::new(), IndexMap::new(), arg_span, None, None);
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(Expression::Function(FunctionExpression::new(
        plain.to_string(),
        arg_list,
        span,
        None,
    )))
}
