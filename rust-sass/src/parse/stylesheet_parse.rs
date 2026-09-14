// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/stylesheet.dart (Statements section: parse,
//   _statement, variable/style-rule/declaration disambiguation, _styleRule,
//   _withChildren, _withStyleRuleChildren, _parseSingleProduction;
//   at-rule productions live in atrule.rs)
// go-source: go/value/parse_stylesheet_parse.go

use crate::common::exception::ScanError;
use crate::common::file_span::SourceLocation;
use crate::parse::atrule::parameter_list_impl;
use crate::parse::atrule::use_rule_impl;
use crate::parse::selector::selector_list_impl;
use crate::value::SassNumber;
use std::collections::HashMap;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression_string::StringExpression;
use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::ast::sass::parameter_list::ParameterList;
use crate::ast::sass::statement::declaration::Declaration;
use crate::ast::sass::statement::style_rule::StyleRule;
use crate::ast::sass::statement::stylesheet::ParseTimeWarning;
use crate::ast::sass::statement::stylesheet::Stylesheet;
use crate::ast::sass::statement::use_rule::UseRule;
use crate::ast::sass::statement::variable_declaration::VariableDeclaration;
use crate::ast::sass::statement::Statement;
use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::common::source_span_file_source::FileSource;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::common::span::Span;
use crate::common::span_scanner::{LineScannerState, SpanScanner};
use crate::deprecation;
use crate::url::SassUrl;

use crate::parse::anyvalue::{
    almost_any_value_impl, interpolated_declaration_value_impl, DeclarationValueOpts,
};
use crate::parse::atrule::{
    at_rule_impl, declaration_at_rule_impl, include_rule_impl, mixin_rule_impl,
};
use crate::parse::expression::{ExpressionOpts, _expression_impl};
use crate::parse::identifier::interpolated_identifier_impl;
use crate::parse::parser::{
    adjust_exception_span_impl, error_impl, expect_identifier_impl, has_prefix_ignore_case,
    identifier_impl, looking_at_identifier, loud_comment_impl, raw_text_impl, span_from_impl,
    span_from_to_impl, string_impl, variable_name_impl, whitespace_impl,
    whitespace_without_comments_impl, wrap_span_format_exception_impl, ParseError,
    ParseFormatError, ParseResult, ParserState,
};
use crate::parse::stylesheet::{
    at_end_of_statement_impl, children_impl, expect_statement_separator_impl,
    looking_at_children_impl, statements_impl, style_rule_selector_impl, StylesheetParser,
    StylesheetState, Syntax,
};
use crate::parse::util::{
    assert_public_impl, looking_at_interpolated_identifier_body_impl,
    looking_at_interpolated_identifier_impl, looking_at_potential_property_hack_impl,
};

// ======================================================================
// Type helpers
// ======================================================================

// Rust-only split of Dart's `dynamic` return from `_declarationOrBuffer`
// (stylesheet.dart:390): a consumed declaration vs. selector-parsing fallback
// text. A `Buffer` feeds back into `_styleRule` with the original `start`.
// Rust-only split, but kept unboxed: transient helper on the hot
// declaration/style-rule disambiguation path; boxing would add alloc churn.
#[allow(clippy::large_enum_variant)]
enum DeclarationOrStyleRule<'b> {
    Statement(Statement<'b>),
    Buffer(InterpolationBuffer<'b>),
}

// Rust-only split of Dart's `dynamic` return from
// `_variableDeclarationOrInterpolation` (stylesheet.dart:503): a consumed
// namespaced declaration vs. selector-parsing fallback interpolation.
// Same unboxed rationale as `DeclarationOrStyleRule` above.
#[allow(clippy::large_enum_variant)]
enum DeclarationOrInterpolation<'b> {
    Declaration(VariableDeclaration<'b>),
    Interpolation(Interpolation<'b>),
}

// ======================================================================
// parseSingleProduction
// ======================================================================

// Matches Dart: `_parseSingleProduction` (stylesheet.dart:168). Parses
// `production` as the entire scanner contents, then expects EOF, all inside
// span-format-exception wrapping.
fn parse_single_production_impl<'b, T>(
    scanner: &mut SpanScanner<'b>,
    state: &mut ParserState<'b>,
    production: impl FnOnce(&mut SpanScanner<'b>, &mut ParserState<'b>) -> ParseResult<'b, T>,
) -> ParseResult<'b, T> {
    wrap_span_format_exception_impl(scanner, state, |scanner, state| {
        let result = production(scanner, state)?;
        scanner.expect_done()?;
        Ok(result)
    })
}

// ======================================================================
// statement_impl
// ======================================================================

/// Consumes a statement allowed at the top level or within nested style
/// and at rules.
///
/// Matches Dart: `_statement({root})` (stylesheet.dart:196). If `root` is
/// `true`, at-rules allowed only at the stylesheet root (`@forward`, `@use`)
/// are parsed; otherwise they error.
pub(crate) fn statement_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    root: bool,
) -> ParseResult<'b, Statement<'b>> {
    let ch = scanner.peek_char(0);
    match ch {
        c if c == '@' as i32 => at_rule_impl(
            scanner,
            state,
            &mut |s, st| statement_impl(s, st, false),
            root,
        ),
        c if c == '+' as i32 => {
            let indented = matches!(state.parser_state.syntax, Syntax::Sass(_));
            if !indented || !looking_at_identifier(scanner, Some(1)) {
                style_rule_impl(scanner, state, None, None).map(Statement::StyleRule)
            } else {
                state.is_use_allowed = false;
                let start = scanner.state();
                scanner.read_char()?;
                include_rule_impl(scanner, state, start).map(Statement::IncludeRule)
            }
        }
        c if c == '=' as i32 => {
            let indented = matches!(state.parser_state.syntax, Syntax::Sass(_));
            if !indented {
                style_rule_impl(scanner, state, None, None).map(Statement::StyleRule)
            } else {
                state.is_use_allowed = false;
                let start = scanner.state();
                scanner.read_char()?;
                whitespace_impl(scanner, &mut state.parser_state, true)?;
                mixin_rule_impl(scanner, state, start).map(Statement::MixinRule)
            }
        }
        c if c == '}' as i32 => {
            let msg = "unmatched \"}\".";
            // Matches Dart: scanner.error(..., length: 1).
            let span = scanner.span_from_to(scanner.pos(), scanner.pos() + 1);
            Err(Box::new(error_impl(msg, &span)))
        }
        _ => {
            if state.in_style_rule
                || state.in_unknown_at_rule
                || state.in_mixin
                || state.in_content_block
            {
                declaration_or_style_rule_impl(scanner, state)
            } else {
                variable_declaration_or_style_rule_impl(scanner, state)
            }
        }
    }
}

// ======================================================================
// variable declarations
// ======================================================================

/// Consumes a namespaced variable declaration.
///
/// Matches Dart: `_variableDeclarationWithNamespace` (stylesheet.dart:227).
pub(crate) fn variable_declaration_with_namespace_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, VariableDeclaration<'b>> {
    let start = scanner.state();
    // Matches Dart: _variableDeclarationWithNamespace uses identifier() with
    // the default normalize=false, so `_bar` stays `_bar` (normalize=true
    // would rewrite it to `-bar` and break module lookup).
    let namespace = identifier_impl(scanner, &state.parser_state, false, false)?;
    scanner.expect_char('.')?;
    variable_declaration_without_namespace_impl(scanner, state, Some(namespace), Some(start))
}

/// Consumes a variable declaration.
///
/// Matches Dart: `variableDeclarationWithoutNamespace` (stylesheet.dart:239).
/// Never *consumes* a namespace, but uses `namespace` for the declaration
/// when passed. Records `!global` names in `global_variables` so the
/// generated module exposes the same variables no matter how it evaluates.
pub(crate) fn variable_declaration_without_namespace_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    namespace: Option<String>,
    start: Option<LineScannerState>,
) -> ParseResult<'b, VariableDeclaration<'b>> {
    let preceding_comment = state.last_silent_comment.take();
    let start = start.unwrap_or_else(|| scanner.state());
    let name = variable_name_impl(scanner, &state.parser_state)?;
    if namespace.is_some() {
        let st_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        assert_public_impl(&name, || Some(st_span))?;
    }
    if matches!(state.parser_state.syntax, Syntax::Css(_)) {
        let err_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Err(Box::new(error_impl(
            "Sass variables aren't allowed in plain CSS.",
            &err_span,
        )));
    }
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    scanner.expect_char(':')?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let value = _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        },
    )?;
    let mut guarded = false;
    let mut global = false;
    let mut flag_start = scanner.state();
    while scanner.scan_char('!') {
        let id = identifier_impl(scanner, &state.parser_state, true, false)?;
        let flag_span = span_from_impl(scanner, &state.parser_state, flag_start)?.file_span()?;
        match id.as_str() {
            "default" => {
                if guarded {
                    state.warnings.push(ParseTimeWarning {
                        deprecation: Some(&deprecation::DUPLICATE_VAR_FLAGS),
                        message: "!default should only be written once for each variable.\nThis will be an error in Dart Sass 2.0.0.".into(),
                        span: flag_span,
                        primary_label: None,
                        secondary: vec![],
                    });
                }
                guarded = true;
            }
            "global" => {
                if namespace.is_some() {
                    return Err(Box::new(error_impl(
                        "!global isn't allowed for variables in other modules.",
                        &flag_span,
                    )));
                } else if global {
                    state.warnings.push(ParseTimeWarning {
                        deprecation: Some(&deprecation::DUPLICATE_VAR_FLAGS),
                        message: "!global should only be written once for each variable.\nThis will be an error in Dart Sass 2.0.0.".into(),
                        span: flag_span,
                        primary_label: None,
                        secondary: vec![],
                    });
                }
                global = true;
            }
            _ => {
                return Err(Box::new(error_impl("Invalid flag name.", &flag_span)));
            }
        }
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        flag_start = scanner.state();
    }
    expect_statement_separator_impl(scanner, state, "variable declaration")?;
    let decl_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    let decl = VariableDeclaration::new(
        name.clone(),
        value,
        decl_span,
        namespace.clone(),
        guarded,
        global,
        preceding_comment.map(|b| *b),
    )
    .map_err(|e| Box::new(ParseError::Sass(e)))?;
    if global {
        state.global_variables.entry(name).or_insert(decl_span);
    }
    Ok(decl)
}

// ======================================================================
// variableDeclarationOrStyleRule
// ======================================================================

// Matches Dart: `_variableDeclarationOrStyleRule` (stylesheet.dart:320).
// The indented syntax allows one leading backslash to force a style rule
// (legacy property-syntax escape hatch); a non-identifier start is always
// a style rule.
fn variable_declaration_or_style_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Statement<'b>> {
    if matches!(state.parser_state.syntax, Syntax::Css(_)) {
        return style_rule_impl(scanner, state, None, None).map(Statement::StyleRule);
    }
    let indented = matches!(state.parser_state.syntax, Syntax::Sass(_));
    if indented && scanner.scan_char('\\') {
        return style_rule_impl(scanner, state, None, None).map(Statement::StyleRule);
    }
    if !looking_at_identifier(scanner, None) {
        return style_rule_impl(scanner, state, None, None).map(Statement::StyleRule);
    }
    let start = scanner.state();
    let result = variable_declaration_or_interpolation_impl(scanner, state)?;
    match result {
        DeclarationOrInterpolation::Declaration(vd) => Ok(Statement::VariableDeclaration(vd)),
        DeclarationOrInterpolation::Interpolation(interp) => {
            let mut buf = InterpolationBuffer::new();
            buf.add_interpolation(&interp);
            style_rule_impl(scanner, state, Some(buf), Some(start)).map(Statement::StyleRule)
        }
    }
}

// ======================================================================
// declarationOrStyleRule
// ======================================================================

/// Consumes a [`VariableDeclaration`], a [`Declaration`], or a
/// [`StyleRule`].
///
/// Matches Dart: `_declarationOrStyleRule` (stylesheet.dart:370) plus the
/// disambiguation criteria in its doc comment: `ns.$var` is always a
/// variable declaration; without an identifier-colon shape it is a
/// selector; `::` after the colon is a selector; otherwise a declaration
/// is attempted and, when the colon is followed by interpolation or an
/// identifier start, a failed declaration parse backtracks into selector
/// parsing. A declaration value followed by `{` is likewise reparsed as a
/// selector so `.foo:bar {` never becomes a nested property.
pub(crate) fn declaration_or_style_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Statement<'b>> {
    let indented = matches!(state.parser_state.syntax, Syntax::Sass(_));
    if indented && scanner.scan_char('\\') {
        return style_rule_impl(scanner, state, None, None).map(Statement::StyleRule);
    }
    let start = scanner.state();
    let result = declaration_or_buffer_impl(scanner, state)?;
    match result {
        DeclarationOrStyleRule::Statement(stmt) => Ok(stmt),
        DeclarationOrStyleRule::Buffer(buf) => {
            style_rule_impl(scanner, state, Some(buf), Some(start)).map(Statement::StyleRule)
        }
    }
}

// ======================================================================
// variableDeclarationOrInterpolation
// ======================================================================

// Matches Dart: `_variableDeclarationOrInterpolation`
// (stylesheet.dart:503). `ident.$` starts a namespaced declaration; anything
// else accumulates into an interpolation, consuming the rest of an
// interpolated identifier so callers don't have to.
fn variable_declaration_or_interpolation_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, DeclarationOrInterpolation<'b>> {
    if !looking_at_identifier(scanner, None) {
        let interp = interpolated_identifier_impl(scanner, state)?;
        return Ok(DeclarationOrInterpolation::Interpolation(interp));
    }
    let start = scanner.state();
    let ident = identifier_impl(scanner, &state.parser_state, false, false)?;
    if scanner.peek_char(0) == '.' as i32 && scanner.peek_char(1) == '$' as i32 {
        scanner.read_char()?;
        let decl =
            variable_declaration_without_namespace_impl(scanner, state, Some(ident), Some(start))?;
        return Ok(DeclarationOrInterpolation::Declaration(decl));
    }
    let mut buf = InterpolationBuffer::new();
    buf.write(&ident);
    if looking_at_interpolated_identifier_body_impl(scanner) {
        let interp = interpolated_identifier_impl(scanner, state)?;
        buf.add_interpolation(&interp);
    }
    let interp_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    let interp = buf
        .interpolation(interp_span)
        .map_err(|e| Box::new(ParseError::Sass(e)))?;
    Ok(DeclarationOrInterpolation::Interpolation(interp))
}

// ======================================================================
// declarationOrBuffer — the most complex method
// ======================================================================

// Matches Dart: `_declarationOrBuffer` (stylesheet.dart:390). Tries to
// parse a variable or property declaration and returns the text parsed so
// far when it fails: an [`InterpolationBuffer`] means "try selector
// parsing", while a `Statement` means a declaration was consumed. Custom
// properties (`--*`) and the plain-CSS `@function` `result` property are
// always parsed as declarations.
fn declaration_or_buffer_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, DeclarationOrStyleRule<'b>> {
    let start = scanner.state();
    let mut name_buffer = InterpolationBuffer::new();
    let mut starts_with_punctuation = false;
    if looking_at_potential_property_hack_impl(scanner) {
        starts_with_punctuation = true;
        let ch = scanner.read_char()?;
        name_buffer.write_char_code(ch);
        let text = raw_text_impl(scanner, &mut state.parser_state, |s, st| {
            whitespace_impl(s, st, false)
        })?
        .1;
        name_buffer.write(&text);
    }
    if !looking_at_interpolated_identifier_impl(scanner) {
        return Ok(DeclarationOrStyleRule::Buffer(name_buffer));
    }
    let variable_or_interpolation = if starts_with_punctuation {
        DeclarationOrInterpolation::Interpolation(interpolated_identifier_impl(scanner, state)?)
    } else {
        variable_declaration_or_interpolation_impl(scanner, state)?
    };
    match variable_or_interpolation {
        DeclarationOrInterpolation::Declaration(vd) => {
            return Ok(DeclarationOrStyleRule::Statement(
                Statement::VariableDeclaration(vd),
            ));
        }
        DeclarationOrInterpolation::Interpolation(interp) => {
            name_buffer.add_interpolation(&interp);
        }
    }
    state.is_use_allowed = false;
    if scanner.peek_char(0) == '/' as i32 && scanner.peek_char(1) == '*' as i32 {
        let text = raw_text_impl(scanner, &mut state.parser_state, |s, _st| {
            loud_comment_impl(s)
        })?
        .1;
        name_buffer.write(&text);
    }
    let mut mid_buffer = String::new();
    let mid_text = raw_text_impl(scanner, &mut state.parser_state, |s, st| {
        whitespace_impl(s, st, false)
    })?
    .1;
    mid_buffer.push_str(&mid_text);
    let before_colon = scanner.state();
    if !scanner.scan_char(':') {
        if !mid_buffer.is_empty() {
            name_buffer.write_char_code(' ');
        }
        return Ok(DeclarationOrStyleRule::Buffer(name_buffer));
    }
    mid_buffer.push(':');
    let interp_span =
        span_from_to_impl(scanner, &state.parser_state, start, Some(&before_colon))?.file_span()?;
    let name = name_buffer.interpolation(interp_span).map_err(|e| {
        let err_span = span_from_impl(scanner, &state.parser_state, start)
            .unwrap_or_else(|_| Span::File(scanner.empty_span()));
        error_impl(
            &e.to_string(),
            &err_span.file_span().unwrap_or(scanner.empty_span()),
        )
    })?;
    let is_custom_property = name.initial_plain().starts_with("--");
    if is_custom_property
        || (state.in_plain_css_function
            && name
                .as_plain()
                .is_some_and(|p| p.eq_ignore_ascii_case("result")))
    {
        let value_expr = if at_end_of_statement_impl(scanner, state) {
            StringExpression::new(
                Interpolation::plain(String::new(), scanner.empty_span()),
                false,
            )
        } else {
            let val = interpolated_declaration_value_impl(
                scanner,
                state,
                DeclarationValueOpts {
                    allow_empty: false,
                    allow_semicolon: false,
                    allow_colon: true,
                    allow_open_brace: true,
                    end_after_of: false,
                    silent_comments: false,
                    consume_newlines: false,
                },
            )?;
            StringExpression::new(val, false)
        };
        let sep_name = if is_custom_property {
            "custom property"
        } else {
            "@function result"
        };
        expect_statement_separator_impl(scanner, state, sep_name)?;
        let ret_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(DeclarationOrStyleRule::Statement(Statement::Declaration(
            Declaration::not_sass_script(name, value_expr, ret_span),
        )));
    }
    if scanner.scan_char(':') {
        name_buffer.write(&mid_buffer);
        name_buffer.write_char_code(':');
        return Ok(DeclarationOrStyleRule::Buffer(name_buffer));
    } else {
        let indented = matches!(state.parser_state.syntax, Syntax::Sass(_));
        if indented && looking_at_interpolated_identifier_impl(scanner) {
            name_buffer.write(&mid_buffer);
            return Ok(DeclarationOrStyleRule::Buffer(name_buffer));
        }
    }
    let post_colon_whitespace = raw_text_impl(scanner, &mut state.parser_state, |s, st| {
        whitespace_impl(s, st, false)
    })?
    .1;
    let nested = try_declaration_children_impl(scanner, state, &name, start, None)?;
    if let Some(n) = nested {
        return Ok(DeclarationOrStyleRule::Statement(Statement::Declaration(n)));
    }
    mid_buffer.push_str(&post_colon_whitespace);
    let could_be_selector =
        post_colon_whitespace.is_empty() && looking_at_interpolated_identifier_impl(scanner);
    let before_declaration = scanner.state();
    let value_result = (|| -> ParseResult<'b, Expression<'b>> {
        let val = _expression_impl(
            scanner,
            state,
            ExpressionOpts {
                bracket_list: false,
                single_equals: false,
                consume_newlines: false,
                until: None,
            },
        )?;
        let has_children = looking_at_children_impl(scanner, state)?;
        if has_children {
            if could_be_selector {
                expect_statement_separator_impl(scanner, state, "")?;
            }
        } else if !at_end_of_statement_impl(scanner, state) {
            expect_statement_separator_impl(scanner, state, "")?;
        }
        Ok(val)
    })();
    match value_result {
        Ok(value) => {
            let nested =
                try_declaration_children_impl(scanner, state, &name, start, Some(value.clone()))?;
            if let Some(n) = nested {
                return Ok(DeclarationOrStyleRule::Statement(Statement::Declaration(n)));
            }
            expect_statement_separator_impl(scanner, state, "variable declaration")?;
            let ret_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
            Ok(DeclarationOrStyleRule::Statement(Statement::Declaration(
                Declaration::new(name, value, ret_span),
            )))
        }
        Err(value_err) => {
            if !could_be_selector {
                return Err(value_err);
            }
            scanner.set_state(before_declaration);
            let additional = almost_any_value_impl(scanner, state, false)?;
            let indented = matches!(state.parser_state.syntax, Syntax::Sass(_));
            if !indented && scanner.peek_char(0) == ';' as i32 {
                return Err(value_err);
            }
            name_buffer.write(&mid_buffer);
            name_buffer.add_interpolation(&additional);
            Ok(DeclarationOrStyleRule::Buffer(name_buffer))
        }
    }
}

// ======================================================================
// styleRule
// ======================================================================

/// Consumes a [`StyleRule`], optionally seeded with `buffer` text that was
/// already parsed while disambiguating a declaration from a selector.
///
/// Matches Dart: `_styleRule` (stylesheet.dart:526).
pub(crate) fn style_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    buffer: Option<InterpolationBuffer<'b>>,
    start_: Option<LineScannerState>,
) -> ParseResult<'b, StyleRule<'b>> {
    state.is_use_allowed = false;
    let start = start_.unwrap_or_else(|| scanner.state());
    if state.parse_selectors {
        if let Some(ref st) = start_ {
            scanner.set_state(*st);
        }
        let selector_list = selector_list_impl(scanner, state)?;
        let was_in_style_rule = state.in_style_rule;
        state.in_style_rule = true;
        let node_span = selector_list.span()?;
        let result = with_children_impl(
            scanner,
            state,
            &mut |s, st| statement_impl(s, st, false).map(Some),
            start,
            |st, children, span| {
                let indented = matches!(st.parser_state.syntax, Syntax::Sass(_));
                if indented && children.is_empty() {
                    st.warnings.push(ParseTimeWarning {
                        deprecation: None,
                        message: "This selector doesn't have any properties and won't be rendered."
                            .into(),
                        span: node_span,
                        primary_label: None,
                        secondary: vec![],
                    });
                }
                Ok(StyleRule::with_parsed_selector(
                    selector_list,
                    children,
                    span,
                ))
            },
        )?;
        state.in_style_rule = was_in_style_rule;
        return Ok(result);
    }
    let mut interpolation = style_rule_selector_impl(scanner, state)?;
    if let Some(mut buf) = buffer {
        buf.add_interpolation(&interpolation);
        let interp_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        interpolation = buf.interpolation(interp_span).map_err(|e| {
            let err_span = span_from_impl(scanner, &state.parser_state, start)
                .unwrap_or_else(|_| Span::File(scanner.empty_span()));
            error_impl(
                &e.to_string(),
                &err_span.file_span().unwrap_or(scanner.empty_span()),
            )
        })?;
    }
    if interpolation.contents.is_empty() {
        return Err(Box::new(error_impl(
            "expected \"}\".",
            &scanner.empty_span(),
        )));
    }
    with_style_rule_children_impl(scanner, state, start, interpolation)
}

// ======================================================================
// withChildren — Dart's _withChildren wrapper
// Dart: lib/src/parse/stylesheet.dart line 4770
// Go: withChildren in go/value/parse_stylesheet_util.go
// ======================================================================

/// Consumes a block of `child` statements and passes them, plus the span
/// from `start` to the end of the child block, to `create`.
///
/// Matches Dart: `_withChildren` (stylesheet.dart:4770).
pub(crate) fn with_children_impl<'b, T>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    child: &mut dyn FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Option<Statement<'b>>>,
    start: LineScannerState,
    create: impl FnOnce(
        &mut StylesheetState<'b>,
        Vec<Statement<'b>>,
        FileSpan<'b>,
    ) -> ParseResult<'b, T>,
) -> ParseResult<'b, T> {
    let children = children_impl(scanner, state, child)?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    let result = create(state, children, span)?;
    whitespace_without_comments_impl(scanner, &state.parser_state, false)?;
    Ok(result)
}

// ======================================================================
// withStyleRuleChildren — Dart's _withStyleRuleChildren wrapper
// Dart: lib/src/parse/stylesheet.dart line 557
// ======================================================================

// Matches Dart: `_withStyleRuleChildren` (stylesheet.dart:559). Consumes
// a style rule's children via [`with_children_impl`], warning in the
// indented syntax when the selector has no properties and won't render.
fn with_style_rule_children_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
    selector: Interpolation<'b>,
) -> ParseResult<'b, StyleRule<'b>> {
    let was_in_style_rule = state.in_style_rule;
    state.in_style_rule = true;
    let result = with_children_impl(
        scanner,
        state,
        &mut |s, st| statement_impl(s, st, false).map(Some),
        start,
        |st, children, span| {
            let indented = matches!(st.parser_state.syntax, Syntax::Sass(_));
            if indented && children.is_empty() {
                let node_span = selector.span()?;
                st.warnings.push(ParseTimeWarning {
                    deprecation: None,
                    message: "This selector doesn't have any properties and won't be rendered."
                        .into(),
                    span: node_span,
                    primary_label: None,
                    secondary: vec![],
                });
            }
            Ok(StyleRule::new(selector, children, span))
        },
    )?;
    state.in_style_rule = was_in_style_rule;
    Ok(result)
}

// ======================================================================
// propertyOrVariableDeclaration
// ======================================================================

// Matches Dart: `_propertyOrVariableDeclaration` (stylesheet.dart:590).
// Only used nested beneath other declarations; `_declarationOrStyleRule`
// handles the general case. Custom-property names (`--*`) may not nest.
fn property_or_variable_declaration_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Statement<'b>> {
    let start = scanner.state();
    let name: Interpolation<'b>;
    if looking_at_potential_property_hack_impl(scanner) {
        let mut name_buffer = InterpolationBuffer::new();
        let ch = scanner.read_char()?;
        name_buffer.write_char_code(ch);
        let text = raw_text_impl(scanner, &mut state.parser_state, |s, st| {
            whitespace_impl(s, st, false)
        })?
        .1;
        name_buffer.write(&text);
        let interp = interpolated_identifier_impl(scanner, state)?;
        name_buffer.add_interpolation(&interp);
        let interp_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        name = name_buffer.interpolation(interp_span).map_err(|e| {
            let err_span = span_from_impl(scanner, &state.parser_state, start)
                .unwrap_or_else(|_| Span::File(scanner.empty_span()));
            error_impl(
                &e.to_string(),
                &err_span.file_span().unwrap_or(scanner.empty_span()),
            )
        })?;
    } else if !matches!(state.parser_state.syntax, Syntax::Css(_)) {
        let result = variable_declaration_or_interpolation_impl(scanner, state)?;
        match result {
            DeclarationOrInterpolation::Declaration(vd) => {
                return Ok(Statement::VariableDeclaration(vd));
            }
            DeclarationOrInterpolation::Interpolation(interp) => {
                name = interp;
            }
        }
    } else {
        name = interpolated_identifier_impl(scanner, state)?;
    }
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    scanner.expect_char(':')?;
    if name.initial_plain().starts_with("--") {
        let name_span = name.span()?;
        return Err(Box::new(error_impl(
            "Declarations whose names begin with \"--\" may not be nested.",
            &name_span,
        )));
    }
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let nested = try_declaration_children_impl(scanner, state, &name, start, None)?;
    if let Some(n) = nested {
        return Ok(Statement::Declaration(n));
    }
    let value = _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        },
    )?;
    let nested = try_declaration_children_impl(scanner, state, &name, start, Some(value.clone()))?;
    if let Some(n) = nested {
        return Ok(Statement::Declaration(n));
    }
    expect_statement_separator_impl(scanner, state, "")?;
    let ret_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(Statement::Declaration(Declaration::new(
        name, value, ret_span,
    )))
}

// ======================================================================
// tryDeclarationChildren + declarationChild
// ======================================================================

// Matches Dart: `_tryDeclarationChildren` (stylesheet.dart:636). Tries
// parsing nested children of an already-parsed declaration `name`,
// returning `None` when there are none. `value` becomes the property value
// when the declaration also carries one.
fn try_declaration_children_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    _name: &Interpolation<'b>,
    _start: LineScannerState,
    _value: Option<Expression<'b>>,
) -> ParseResult<'b, Option<Declaration<'b>>> {
    let has_children = looking_at_children_impl(scanner, state)?;
    if !has_children {
        return Ok(None);
    }
    if matches!(state.parser_state.syntax, Syntax::Css(_)) {
        return Err(Box::new(error_impl(
            "Nested declarations aren't allowed in plain CSS.",
            &scanner.empty_span(),
        )));
    }
    let name = _name.clone();
    let value = _value.clone();
    with_children_impl(
        scanner,
        state,
        &mut |s, st| declaration_child_impl(s, st).map(Some),
        _start,
        |_, children, span| Ok(Some(Declaration::nested(name, children, span, value))),
    )
}

/// Consumes a statement allowed within a declaration.
///
/// Matches Dart: `_declarationChild` (stylesheet.dart:654): `@`-rules go
/// to `_declarationAtRule`, everything else to
/// `_propertyOrVariableDeclaration`.
pub(crate) fn declaration_child_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Statement<'b>> {
    if scanner.peek_char(0) == '@' as i32 {
        declaration_at_rule_impl(scanner, state)
    } else {
        property_or_variable_declaration_impl(scanner, state)
    }
}

// ======================================================================
// parse (entry point)
// ======================================================================

/// Parses a full stylesheet: optional BOM, top-level statements, EOF.
///
/// Matches Dart: `parse` (stylesheet.dart:98). `@charset` is consumed and
/// discarded here so [`statement_impl`] always returns a statement.
pub(crate) fn parse_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Stylesheet<'b>> {
    let start = scanner.state();
    scanner.scan_char('\u{FEFF}');
    let stmts = statements_impl(
        scanner,
        state,
        &mut |s: &mut SpanScanner<'b>, st: &mut StylesheetState<'b>| {
            if s.scan("@charset") {
                whitespace_impl(s, &mut st.parser_state, false)?;
                string_impl(s)?;
                return Ok(None);
            }
            statement_impl(s, st, true).map(Some)
        },
    )
    // Dart `parse()` wraps the whole stylesheet in `wrapSpanFormatException`,
    // which adjusts "expected"-prefixed errors' spans to the last newline.
    // Without this, statement-level errors (e.g. "Expected newline." from the
    // indented syntax) point past the offending line.
    .map_err(|e| adjust_stylesheet_error_span(*e))?;
    scanner.expect_done()?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    let warnings: Vec<ParseTimeWarning<'b>> = state.warnings.drain(..).collect();
    let plain_css = matches!(state.parser_state.syntax, Syntax::Css(_));
    let global_variables: indexmap::IndexMap<String, FileSpan<'b>> =
        state.global_variables.drain().collect();
    Ok(Stylesheet::detailed(
        stmts,
        span,
        warnings,
        plain_css,
        global_variables,
    ))
}

/// Applies the "expected"-prefix span adjustment to a stylesheet-level parse
/// error. Mirrors Dart's `wrapSpanFormatException` (parser.dart:734) for the
/// statement/child parsing of a whole stylesheet.
fn adjust_stylesheet_error_span(err: ParseError<'_>) -> Box<ParseError<'_>> {
    match err {
        ParseError::Format(fmt) => {
            if has_prefix_ignore_case(&fmt.message, "expected") {
                match adjust_exception_span_impl(fmt.span) {
                    Ok(adjusted) => Box::new(ParseError::Format(ParseFormatError {
                        span: adjusted,
                        ..fmt
                    })),
                    Err(_) => Box::new(ParseError::Format(fmt)),
                }
            } else {
                Box::new(ParseError::Format(fmt))
            }
        }
        ParseError::Scan(scan) => {
            if has_prefix_ignore_case(&scan.message, "expected") {
                match adjust_exception_span_impl(scan.span) {
                    Ok(adjusted) => Box::new(ParseError::Scan(ScanError {
                        span: adjusted,
                        ..scan
                    })),
                    Err(_) => Box::new(ParseError::Scan(scan)),
                }
            } else {
                Box::new(ParseError::Scan(scan))
            }
        }
        other => Box::new(other),
    }
}

// ======================================================================
// Thin production wrappers
// ======================================================================

// Rust-only helper: builds a throwaway stylesheet state over an existing
// parser state for the single-production entry points below.
fn make_temp_stylesheet<'b>(state: &ParserState<'b>) -> StylesheetState<'b> {
    StylesheetState {
        parser_state: ParserState {
            syntax: state.syntax.clone(),
            interpolation_map: state.interpolation_map,
            in_expression: state.in_expression,
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
        warnings: Vec::new(),
        last_silent_comment: None,
    }
}

// TODO(eval): entry point for the evaluator — remove #[allow] once eval is ported.
// Matches Dart: `parseParameterList` (stylesheet.dart:126).
#[allow(dead_code)]
pub(crate) fn parse_parameter_list_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut ParserState<'b>,
) -> ParseResult<'b, ParameterList<'b>> {
    let mut st = make_temp_stylesheet(state);
    parse_single_production_impl(scanner, state, |scanner, _state| {
        scanner.expect_char('@')?;
        identifier_impl(scanner, _state, true, false)?;
        whitespace_impl(scanner, _state, true)?;
        identifier_impl(scanner, _state, true, false)?;
        let params = parameter_list_impl(scanner, &mut st)?;
        whitespace_impl(scanner, _state, true)?;
        scanner.expect_char('{')?;
        Ok(params)
    })
}

// TODO(eval): entry point for the evaluator — remove #[allow] once eval is ported.
// Matches Dart: `parseExpression` (stylesheet.dart:137).
#[allow(dead_code)]
pub(crate) fn parse_expression_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut ParserState<'b>,
) -> ParseResult<'b, Expression<'b>> {
    let mut st = make_temp_stylesheet(state);
    parse_single_production_impl(scanner, state, |scanner, _state| {
        _expression_impl(
            scanner,
            &mut st,
            ExpressionOpts {
                bracket_list: false,
                single_equals: false,
                consume_newlines: false,
                until: None,
            },
        )
    })
}

// TODO(eval): entry point for the evaluator — remove #[allow] once eval is ported.
// Matches Dart: `parseNumber` (stylesheet.dart:142).
#[allow(dead_code)]
pub(crate) fn parse_number_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut ParserState<'b>,
) -> ParseResult<'b, SassNumber> {
    let mut st = make_temp_stylesheet(state);
    parse_single_production_impl(scanner, state, |scanner, _state| {
        let expr = _expression_impl(
            scanner,
            &mut st,
            ExpressionOpts {
                bracket_list: false,
                single_equals: false,
                consume_newlines: false,
                until: None,
            },
        )?;
        match expr {
            Expression::Number(n) => Ok(SassNumber::new(n.value, n.unit.as_deref())),
            _ => Err(Box::new(error_impl(
                "Expected number.",
                &scanner.empty_span(),
            ))),
        }
    })
}

// TODO(eval): entry point for the evaluator — remove #[allow] once eval is ported.
// Matches Dart: `parseVariableDeclaration` (stylesheet.dart:147).
#[allow(dead_code)]
pub(crate) fn parse_variable_declaration_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut ParserState<'b>,
) -> ParseResult<'b, (VariableDeclaration<'b>, Vec<ParseTimeWarning<'b>>)> {
    let mut st = make_temp_stylesheet(state);
    parse_single_production_impl(scanner, state, |scanner, _state| {
        let decl = if looking_at_identifier(scanner, None) {
            variable_declaration_with_namespace_impl(scanner, &mut st)?
        } else {
            variable_declaration_without_namespace_impl(scanner, &mut st, None, None)?
        };
        Ok((decl, st.warnings))
    })
}

// TODO(eval): entry point for the evaluator — remove #[allow] once eval is ported.
// Matches Dart: `parseUseRule` (stylesheet.dart:156).
#[allow(dead_code)]
pub(crate) fn parse_use_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut ParserState<'b>,
) -> ParseResult<'b, (UseRule<'b>, Vec<ParseTimeWarning<'b>>)> {
    let mut st = make_temp_stylesheet(state);
    parse_single_production_impl(scanner, state, |scanner, _state| {
        let start = scanner.state();
        scanner.expect_char('@')?;
        expect_identifier_impl(scanner, "use", "", true)?;
        whitespace_impl(scanner, _state, true)?;
        let rule = use_rule_impl(scanner, &mut st, start)?;
        Ok((rule, st.warnings))
    })
}

/// Parses a function signature in Node Sass's `functions` option format and
/// returns its name and declaration.
///
/// Matches Dart: `parseSignature` in utils.dart (backed by
// `StylesheetParser.parseSignature`, stylesheet.dart:180). When
// `require_parens` is `false`, parentheses may be omitted.
pub fn parse_signature(
    signature: &str,
    require_parens: bool,
) -> Result<SignatureResult, Box<SassError>> {
    let arena = bumpalo::Bump::new();
    let source = FileSource::new_in(&arena, signature, None);
    let mut scanner = SpanScanner::new(source);
    let mut state = ParserState {
        syntax: Syntax::Scss,
        interpolation_map: None,
        in_expression: false,
    };
    let result =
        wrap_span_format_exception_impl(&mut scanner, &mut state, |scanner, parser_state| {
            let name = identifier_impl(scanner, parser_state, false, false)?;
            let mut st = StylesheetState {
                parser_state: ParserState {
                    syntax: Syntax::Scss,
                    interpolation_map: parser_state.interpolation_map,
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
                warnings: Vec::new(),
                last_silent_comment: None,
            };
            let parameters = if require_parens || scanner.peek_char(0) == '(' as i32 {
                parameter_list_impl(scanner, &mut st)?
            } else {
                ParameterList::empty(scanner.empty_span())
            };
            scanner.expect_done()?;
            Ok((name, parameters))
        });
    result
        .map_err(|e| {
            let msg = format!("Invalid signature {:?}: {}", signature, e);
            Box::new(SassError::Format {
                message: msg,
                span: SourceSpanWithContext::new(
                    SourceLocation {
                        offset: 0,
                        line: 0,
                        column: 0,
                    },
                    SourceLocation {
                        offset: 0,
                        line: 0,
                        column: 0,
                    },
                    String::new(),
                    String::new(),
                    None,
                )
                .unwrap_or_else(|_| panic!("bogus span")),
                original_source: Some(signature.to_string()),
                cause: None,
                loaded_urls: Vec::new(),
            })
        })
        .and_then(|(name, pl)| -> Result<SignatureResult, Box<SassError>> {
            let mut params = Vec::with_capacity(pl.parameters.len());
            for p in &pl.parameters {
                let default = match &p.default_value {
                    Some(e) => Some(Expression::to_display_string(e)?),
                    None => None,
                };
                params.push((p.name.clone(), default));
            }
            Ok(SignatureResult {
                name,
                params,
                rest_param: pl.rest_parameter.clone(),
            })
        })
}

/// Owned result type for `parse_signature`. No arena borrows.
#[derive(Debug, Clone)]
pub struct SignatureResult {
    pub name: String,
    pub params: Vec<(String, Option<String>)>,
    pub rest_param: Option<String>,
}

// ======================================================================
// Thin wrappers on StylesheetParser
// ======================================================================

impl<'parse> StylesheetParser<'parse> {
    /// Parses a full stylesheet; see [`parse_impl`].
    pub fn parse(&mut self) -> ParseResult<'parse, Stylesheet<'parse>> {
        parse_impl(&mut self.scanner, &mut self.state)
    }

    /// Parses one statement; see [`statement_impl`].
    pub fn statement(&mut self, root: bool) -> ParseResult<'parse, Statement<'parse>> {
        statement_impl(&mut self.scanner, &mut self.state, root)
    }
}

/// Parse a parameter list from a string like `@function name($a, $b: default) {`.
/// Returns an error if parsing fails — do NOT use for hardcoded strings.
///
/// Matches Go: `value.ParseParameterList`
pub fn parse_parameter_list<'compile, 'parse>(
    contents: &str,
    url: &str,
    arena: &'compile bumpalo::Bump,
) -> SassResult<ParameterList<'parse>>
where
    'compile: 'parse,
{
    let parsed_url = if url.is_empty() {
        None
    } else {
        Some(SassUrl::parse(url).map_err(|_| SassError::Script {
            message: format!("Invalid URL: {url}"),
            argument_name: None,
        })?)
    };
    let source = FileSource::new_in(arena, contents, parsed_url);
    let mut scanner = SpanScanner::new(source);
    let mut state = ParserState {
        syntax: Syntax::Scss,
        interpolation_map: None,
        in_expression: false,
    };
    let params = parse_parameter_list_impl(&mut scanner, &mut state)?;
    scanner.expect_done().map_err(SassError::from)?;
    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_scss<'compile, 'parse>(
        arena: &'compile bumpalo::Bump,
        text: &str,
    ) -> ParseResult<'parse, Stylesheet<'parse>>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        let mut parser = StylesheetParser::new(fs, Syntax::Scss, None);
        parser.parse()
    }

    #[test]
    fn test_unmatched_brace_span_len_1() {
        // Matches Dart: scanner.error('unmatched "}".', length: 1).
        for src in ["}", "a {b: c}}"] {
            let arena = bumpalo::Bump::new();
            let err = parse_scss(&arena, src).unwrap_err();
            let (start, end) = match *err {
                ParseError::Scan(scan) => (
                    scan.span.start_location().offset,
                    scan.span.end_location().offset,
                ),
                ParseError::Format(fmt) => (
                    fmt.span.start_location().offset,
                    fmt.span.end_location().offset,
                ),
                other => panic!("src {src}: expected span error, got {other:?}"),
            };
            assert_eq!(end - start, 1, "src {src}");
        }
    }

    #[test]
    fn test_use_namespace_no_normalize() {
        // Matches Dart: _useNamespace uses identifier() with the default
        // normalize=false, so `_bar` stays `_bar` (normalize would give
        // `-bar` and break module lookup).
        for (src, want_ns) in [
            ("@use \"foo\" as _bar;", Some("_bar")),
            ("@use \"foo\" as _Foo;", Some("_Foo")),
            ("@use \"foo\" as -bar;", Some("-bar")),
        ] {
            let arena = bumpalo::Bump::new();
            let sheet = parse_scss(&arena, src).unwrap();
            match &sheet.children[0] {
                Statement::UseRule(rule) => {
                    assert_eq!(rule.namespace.as_deref(), want_ns, "src {src}");
                }
                other => panic!("src {src}: expected UseRule, got {other:?}"),
            }
        }
    }
}
