// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/stylesheet.dart (At Rules section: atRule,
//   _declarationAtRule, _functionChild, each @-rule production,
//   _configuration, _parameterList; import-URL helpers live in import_url.rs,
//   _importSupportsQuery family in supports.rs)
// go-source: go/value/parse_stylesheet_atrule.go
//
// Matches Dart: the dispatch in `atRule` (stylesheet.dart:669) largely
// duplicates `CssParser.atRule` — most changes here should be mirrored in
// css.rs.

use crate::common::SassError;
use crate::parse::css::css_at_rule_impl;
use crate::parse::expression::interpolated_string_token_impl;
use crate::parse::parser::raw_text_impl;
use crate::parse::parser::string_impl;
use crate::parse::sass::sass_import_argument_impl;
use crate::parse::stylesheet::StylesheetParser;
use std::collections::{HashMap, HashSet};

use percent_encoding::percent_decode_str;

use crate::url::SassUrl;

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::configured_variable::ConfiguredVariable;
use crate::ast::sass::dynamic_import::DynamicImport;
use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression_supports::SupportsExpression;
use crate::ast::sass::import::Import;
use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation::InterpolationPart;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::ast::sass::parameter::Parameter;
use crate::ast::sass::parameter_list::ParameterList;
use crate::ast::sass::statement::at_root_rule::AtRootRule;
use crate::ast::sass::statement::at_rule::AtRule;
use crate::ast::sass::statement::content_block::ContentBlock;
use crate::ast::sass::statement::content_rule::ContentRule;
use crate::ast::sass::statement::debug_rule::DebugRule;
use crate::ast::sass::statement::each_rule::EachRule;
use crate::ast::sass::statement::error_rule::ErrorRule;
use crate::ast::sass::statement::extend_rule::ExtendRule;
use crate::ast::sass::statement::for_rule::ForRule;
use crate::ast::sass::statement::forward_rule::ForwardRule;
use crate::ast::sass::statement::function_rule::FunctionRule;
use crate::ast::sass::statement::if_rule::{IfClause, IfRule};
use crate::ast::sass::statement::import_rule::ImportRule;
use crate::ast::sass::statement::include_rule::IncludeRule;
use crate::ast::sass::statement::media_rule::MediaRule;
use crate::ast::sass::statement::mixin_rule::MixinRule;
use crate::ast::sass::statement::return_rule::ReturnRule;
use crate::ast::sass::statement::stylesheet::ParseTimeWarning;
use crate::ast::sass::statement::supports_rule::SupportsRule;
use crate::ast::sass::statement::use_rule::UseRule;
use crate::ast::sass::statement::warn_rule::WarnRule;
use crate::ast::sass::statement::while_rule::WhileRule;
use crate::ast::sass::statement::Statement;
use crate::ast::sass::static_import::StaticImport;
use crate::ast::sass::supports_condition::SupportsCondition;
use crate::common::ast_node::AstNode;
use crate::common::span::Span;
use crate::common::span_scanner::{LineScannerState, SpanScanner};
use crate::deprecation;
use crate::unvendor::unvendor;

use crate::parse::anyvalue::{
    almost_any_value_impl, interpolated_declaration_value_impl, DeclarationValueOpts,
};
use crate::parse::expression::{
    argument_invocation, dynamic_url_impl, expression_until_comma, try_url_contents,
    ExpressionOpts, _expression_impl,
};
use crate::parse::identifier::{interpolated_identifier_impl, single_interpolation_impl};
use crate::parse::import_url::{is_plain_import_url, parse_import_url};
use crate::parse::media_style::media_query_list_impl;
use crate::parse::parser::{
    error_impl, expect_identifier_impl, identifier_impl, looking_at_identifier, parse_identifier,
    scan_identifier_impl, span_from_impl, span_from_to_impl, variable_name_impl, whitespace_impl,
    whitespace_without_comments_impl, with_error_message_impl, ParseError, ParseResult,
};
use crate::parse::stylesheet::{
    at_end_of_statement_impl, children_impl, current_indentation_impl,
    expect_statement_separator_impl, looking_at_children_impl, scan_else_impl, StylesheetState,
    Syntax,
};
use crate::parse::stylesheet_parse::{
    declaration_child_impl, declaration_or_style_rule_impl, statement_impl, style_rule_impl,
    variable_declaration_with_namespace_impl, with_children_impl,
};
use crate::parse::supports::{import_supports_query_impl, supports_condition_impl};
use crate::parse::util::{
    add_or_inject_impl, looking_at_expression_impl, looking_at_interpolated_identifier_impl,
    public_identifier_impl, url_string_impl,
};

// ======================================================================
// Dispatch: atRule, declarationAtRule, functionChild
// ======================================================================

/// Consumes an at-rule.
///
/// Matches Dart: `atRule` (stylesheet.dart:669). Consumes at-rules allowed
/// at all levels of the document; `child` consumes any rules allowed
/// specifically in the caller's context. When `root` is `true`, parses
/// at-rules allowed only at the stylesheet root (`@forward`, `@use`).
pub(crate) fn at_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    child: &mut dyn FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Statement<'b>>,
    root: bool,
) -> ParseResult<'b, Statement<'b>> {
    if let Syntax::Css(_) = state.parser_state.syntax {
        return css_at_rule_impl(scanner, state, child, root);
    }
    let start = scanner.state();
    scanner.expect_char('@')?;
    let name = interpolated_identifier_impl(scanner, state)?;
    let was_use_allowed = state.is_use_allowed;
    state.is_use_allowed = false;
    match name.as_plain() {
        None => unknown_at_rule_impl(scanner, state, start, &name),
        Some(plain) => match plain {
            "at-root" => at_root_rule_impl(scanner, state, start).map(Statement::AtRootRule),
            "content" => content_rule_impl(scanner, state, start).map(Statement::ContentRule),
            "debug" => debug_rule_impl(scanner, state, start).map(Statement::DebugRule),
            "each" => each_rule_impl(scanner, state, start, child).map(Statement::EachRule),
            "else" => disallowed_at_rule_impl(scanner, state, start),
            "error" => error_rule_impl(scanner, state, start).map(Statement::ErrorRule),
            "extend" => extend_rule_impl(scanner, state, start).map(Statement::ExtendRule),
            "for" => for_rule_impl(scanner, state, start, child).map(Statement::ForRule),
            "forward" => {
                state.is_use_allowed = was_use_allowed;
                if !root {
                    return disallowed_at_rule_impl(scanner, state, start);
                }
                forward_rule_impl(scanner, state, start).map(Statement::ForwardRule)
            }
            "function" => function_rule_impl(scanner, state, start, &name),
            "if" => if_rule_impl(scanner, state, start, child).map(Statement::IfRule),
            "import" => import_rule_impl(scanner, state, start).map(Statement::ImportRule),
            "include" => include_rule_impl(scanner, state, start).map(Statement::IncludeRule),
            "media" => media_rule_impl(scanner, state, start).map(Statement::MediaRule),
            "mixin" => mixin_rule_impl(scanner, state, start).map(Statement::MixinRule),
            "-moz-document" => {
                moz_document_rule_impl(scanner, state, start, &name).map(Statement::AtRule)
            }
            "return" => disallowed_at_rule_impl(scanner, state, start),
            "supports" => supports_rule_impl(scanner, state, start).map(Statement::SupportsRule),
            "use" => {
                state.is_use_allowed = was_use_allowed;
                if !root {
                    return disallowed_at_rule_impl(scanner, state, start);
                }
                use_rule_impl(scanner, state, start).map(Statement::UseRule)
            }
            "warn" => warn_rule_impl(scanner, state, start).map(Statement::WarnRule),
            "while" => while_rule_impl(scanner, state, start, child).map(Statement::WhileRule),
            _ => unknown_at_rule_impl(scanner, state, start, &name),
        },
    }
}

/// Consumes an at-rule allowed within a property declaration.
///
/// Matches Dart: `_declarationAtRule` (stylesheet.dart:737).
pub(crate) fn declaration_at_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Statement<'b>> {
    let start = scanner.state();
    let name = plain_at_rule_name_impl(scanner, state)?;
    match name.as_str() {
        "content" => content_rule_impl(scanner, state, start).map(Statement::ContentRule),
        "debug" => debug_rule_impl(scanner, state, start).map(Statement::DebugRule),
        "each" => each_rule_impl(scanner, state, start, &mut |s, st| {
            declaration_child_impl(s, st)
        })
        .map(Statement::EachRule),
        "else" => disallowed_at_rule_impl(scanner, state, start),
        "error" => error_rule_impl(scanner, state, start).map(Statement::ErrorRule),
        "for" => for_rule_impl(scanner, state, start, &mut |s, st| {
            declaration_child_impl(s, st)
        })
        .map(Statement::ForRule),
        "if" => if_rule_impl(scanner, state, start, &mut |s, st| {
            declaration_child_impl(s, st)
        })
        .map(Statement::IfRule),
        "include" => include_rule_impl(scanner, state, start).map(Statement::IncludeRule),
        "warn" => warn_rule_impl(scanner, state, start).map(Statement::WarnRule),
        "while" => while_rule_impl(scanner, state, start, &mut |s, st| {
            declaration_child_impl(s, st)
        })
        .map(Statement::WhileRule),
        _ => disallowed_at_rule_impl(scanner, state, start),
    }
}

/// Consumes a statement allowed within a function.
///
/// Matches Dart: `_functionChild` (stylesheet.dart:755). A failed variable
/// declaration falls back to declaration-or-style-rule parsing so that a
/// misplaced style rule or declaration reports "@function rules may not
/// contain …" instead of the raw declaration error.
pub(crate) fn function_child_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Statement<'b>> {
    if scanner.peek_char(0) != '@' as i32 {
        let saved_state = scanner.state();
        match variable_declaration_with_namespace_impl(scanner, state) {
            Ok(var_decl) => return Ok(Statement::VariableDeclaration(var_decl)),
            Err(var_decl_err) => {
                scanner.set_state(saved_state);
                let statement = match declaration_or_style_rule_impl(scanner, state) {
                    Ok(s) => s,
                    Err(_) => return Err(var_decl_err),
                };
                let span = statement.span().unwrap_or_else(|_| scanner.empty_span());
                let kind = if matches!(statement, Statement::StyleRule(_)) {
                    "style rules"
                } else {
                    "declarations"
                };
                return Err(Box::new(error_impl(
                    &format!("@function rules may not contain {kind}."),
                    &span,
                )));
            }
        }
    }
    let start = scanner.state();
    let name = plain_at_rule_name_impl(scanner, state)?;
    match name.as_str() {
        "debug" => debug_rule_impl(scanner, state, start).map(Statement::DebugRule),
        "each" => {
            each_rule_impl(scanner, state, start, &mut function_child_impl).map(Statement::EachRule)
        }
        "else" => disallowed_at_rule_impl(scanner, state, start),
        "error" => error_rule_impl(scanner, state, start).map(Statement::ErrorRule),
        "for" => {
            for_rule_impl(scanner, state, start, &mut function_child_impl).map(Statement::ForRule)
        }
        "if" => {
            if_rule_impl(scanner, state, start, &mut function_child_impl).map(Statement::IfRule)
        }
        "return" => return_rule_impl(scanner, state, start).map(Statement::ReturnRule),
        "warn" => warn_rule_impl(scanner, state, start).map(Statement::WarnRule),
        "while" => while_rule_impl(scanner, state, start, &mut function_child_impl)
            .map(Statement::WhileRule),
        _ => disallowed_at_rule_impl(scanner, state, start),
    }
}

// Matches Dart: `_plainAtRuleName` (stylesheet.dart:798). Consumes an
// at-rule name with interpolation disallowed.
fn plain_at_rule_name_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, String> {
    scanner.expect_char('@')?;
    identifier_impl(scanner, &state.parser_state, true, false)
}

// ======================================================================
// atRootRule + atRootQuery
// ======================================================================

// Matches Dart: `_atRootRule` (stylesheet.dart:807). `start` points
// before the `@`.
fn at_root_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, AtRootRule<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    if scanner.peek_char(0) == '(' as i32 {
        let query = at_root_query_impl(scanner, state)?;
        return with_children_impl(
            scanner,
            state,
            &mut |s, st| statement_impl(s, st, false).map(Some),
            start,
            |_, children, span| Ok(AtRootRule::new(children, span, Some(query))),
        );
    }
    let has_children = looking_at_children_impl(scanner, state)?;
    if has_children
        || (matches!(state.parser_state.syntax, Syntax::Sass(_))
            && at_end_of_statement_impl(scanner, state))
    {
        return with_children_impl(
            scanner,
            state,
            &mut |s, st| statement_impl(s, st, false).map(Some),
            start,
            |_, children, span| Ok(AtRootRule::new(children, span, None)),
        );
    }
    let child = style_rule_impl(scanner, state, None, None)?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(AtRootRule::new(
        vec![Statement::StyleRule(child)],
        span,
        None,
    ))
}

// Matches Dart: `_atRootQuery` (stylesheet.dart:829). Consumes a query
// expression of the form `(foo: bar)`.
fn at_root_query_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Interpolation<'b>> {
    let start = scanner.state();
    let mut buffer = InterpolationBuffer::new();
    scanner.expect_char('(')?;
    buffer.write_char_code('(');
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let expr1 = _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: true,
            until: None,
        },
    )?;
    add_or_inject_impl(&mut buffer, &expr1)?;
    if scanner.scan_char(':') {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        buffer.write_char_code(':');
        buffer.write_char_code(' ');
        let expr2 = _expression_impl(
            scanner,
            state,
            ExpressionOpts {
                bracket_list: false,
                single_equals: false,
                consume_newlines: true,
                until: None,
            },
        )?;
        add_or_inject_impl(&mut buffer, &expr2)?;
    }
    scanner.expect_char(')')?;
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    buffer.write_char_code(')');
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    buffer
        .interpolation(span)
        .map_err(|e| Box::new(ParseError::Sass(e)))
}

// ======================================================================
// contentRule
// ======================================================================

// Matches Dart: `_contentRule` (stylesheet.dart:854). `start` points
// before the `@`. Only allowed within mixin declarations.
fn content_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, ContentRule<'b>> {
    if !state.in_mixin {
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Err(Box::new(error_impl(
            "@content is only allowed within mixin declarations.",
            &span,
        )));
    }
    let before_whitespace = scanner.state();
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let arguments = if scanner.peek_char(0) == '(' as i32 {
        let args = argument_invocation(scanner, state, true, false)?;
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        args
    } else {
        let span = scanner.span_from_to(before_whitespace.position, before_whitespace.position);
        ArgumentList::empty(span)
    };
    expect_statement_separator_impl(scanner, state, "@content rule")?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(ContentRule::new(arguments, span))
}

// ======================================================================
// debugRule, errorRule, warnRule
// ======================================================================

// Matches Dart: `_debugRule` (stylesheet.dart:879). `start` points before
// the `@`. The span ends at the expression end, not the separator.
fn debug_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, DebugRule<'b>> {
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
    let expression_end = scanner.state();
    expect_statement_separator_impl(scanner, state, "@debug rule")?;
    let span = span_from_to_impl(scanner, &state.parser_state, start, Some(&expression_end))?
        .file_span()?;
    Ok(DebugRule::new(value, span))
}

// Matches Dart: `_errorRule` (stylesheet.dart:918). `start` points before
// the `@`. The span ends at the expression end, not the separator.
fn error_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, ErrorRule<'b>> {
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
    let expression_end = scanner.state();
    expect_statement_separator_impl(scanner, state, "@error rule")?;
    let span = span_from_to_impl(scanner, &state.parser_state, start, Some(&expression_end))?
        .file_span()?;
    Ok(ErrorRule::new(value, span))
}

// Matches Dart: `_warnRule` (stylesheet.dart:1732). `start` points before
// the `@`. The span ends at the expression end, not the separator.
fn warn_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, WarnRule<'b>> {
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
    let expression_end = scanner.state();
    expect_statement_separator_impl(scanner, state, "@warn rule")?;
    let span = span_from_to_impl(scanner, &state.parser_state, start, Some(&expression_end))?
        .file_span()?;
    Ok(WarnRule::new(value, span))
}

// ======================================================================
// extendRule
// ======================================================================

// Matches Dart: `_extendRule` (stylesheet.dart:929). `start` points
// before the `@`. Only valid within style rules, mixins, and content
// blocks; the optional flag is a separate `!optional` scan.
fn extend_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, ExtendRule<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    if !state.in_style_rule && !state.in_mixin && !state.in_content_block {
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Err(Box::new(error_impl(
            "@extend may only be used within style rules.",
            &span,
        )));
    }
    let value = almost_any_value_impl(scanner, state, false)?;
    let optional = scanner.scan_char('!');
    if optional {
        expect_identifier_impl(scanner, "optional", "", true)?;
        whitespace_impl(scanner, &mut state.parser_state, false)?;
    }
    expect_statement_separator_impl(scanner, state, "@extend rule")?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(ExtendRule::new(value, span, optional))
}

// ======================================================================
// returnRule
// ======================================================================

// Matches Dart: `_returnRule` (stylesheet.dart:1593). `start` points
// before the `@`.
fn return_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, ReturnRule<'b>> {
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
    expect_statement_separator_impl(scanner, state, "@return rule")?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(ReturnRule::new(value, span))
}

// ======================================================================
// useNamespace
// ======================================================================

// Matches Dart: `_useNamespace` (stylesheet.dart:1641). Parses the `as`
// clause of a `@use` rule, or derives the default namespace from the URL's
// last path segment (leading `_` stripped, extension dropped). Returns
// `None` for `@use … as *`.
fn use_namespace_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    url: &SassUrl,
    start: LineScannerState,
) -> ParseResult<'b, Option<String>> {
    let ok = scan_identifier_impl(scanner, "as", false)?;
    if ok {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        if scanner.scan_char('*') {
            return Ok(None);
        }
        let s = identifier_impl(scanner, &state.parser_state, false, false)?;
        return Ok(Some(s));
    }
    let path = url.path();
    let raw_basename = match path.rfind('/') {
        Some(idx) => &path[idx + 1..],
        None => path,
    };
    // Dart: the namespace comes from `url.pathSegments.last`, which is
    // percent-decoded (so `pkg:%66oo` derives the namespace `foo`).
    let decoded = percent_decode_str(raw_basename).decode_utf8_lossy();
    let basename: &str = decoded.strip_prefix('_').unwrap_or(decoded.as_ref());
    let basename = match basename.find('.') {
        Some(dot) => &basename[..dot],
        None => basename,
    };
    let result = basename.to_string();
    if parse_identifier(&result).is_err() {
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Err(Box::new(error_impl(
            &format!("The default namespace {:?} is not a valid Sass identifier.\n\nRecommendation: add an \"as\" clause to define an explicit namespace.", result),
            &span,
        )));
    }
    Ok(Some(result))
}

// ======================================================================
// useRule
// ======================================================================

/// Consumes a `@use` rule.
///
/// Matches Dart: `_useRule` (stylesheet.dart:1618). `start` points before
/// the `@`. Errors when a prior rule already closed the `@use` window.
pub(crate) fn use_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, UseRule<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let url = url_string_impl(scanner, &state.parser_state)?;
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let namespace = use_namespace_impl(scanner, state, &url, start)?;
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let configuration = configuration_impl(scanner, state, false)?;
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    if !state.is_use_allowed {
        return Err(Box::new(error_impl(
            "@use rules must be written before any other rules.",
            &span,
        )));
    }
    expect_statement_separator_impl(scanner, state, "@use rule")?;
    UseRule::new(url, namespace, span, configuration).map_err(|e| Box::new(ParseError::Sass(e)))
}

// ======================================================================
// forwardRule + memberList
// ======================================================================

// Matches Dart: `_forwardRule` (stylesheet.dart:1064). `start` points
// before the `@`. The `show` member order is preserved (Dart returns plain
// sets; Rust keeps ordered vecs alongside the sets for deterministic
// forwarding).
fn forward_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, ForwardRule<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let url = url_string_impl(scanner, &state.parser_state)?;
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let mut prefix: Option<String> = None;
    if scan_identifier_impl(scanner, "as", false)? {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let s = identifier_impl(scanner, &state.parser_state, true, false)?;
        prefix = Some(s);
        scanner.expect_char('*')?;
        whitespace_impl(scanner, &mut state.parser_state, false)?;
    }
    let (mut shown_mf, mut shown_vars) = (None, None);
    let (mut shown_order_mf, mut shown_order_vars): (Option<Vec<String>>, Option<Vec<String>>) =
        (None, None);
    let (mut hidden_mf, mut hidden_vars) = (None, None);
    if scan_identifier_impl(scanner, "show", false)? {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let (order_mf, order_vars, idents, vars) = member_list_impl(scanner, state)?;
        shown_mf = Some(idents);
        shown_vars = Some(vars);
        shown_order_mf = Some(order_mf);
        shown_order_vars = Some(order_vars);
    } else if scan_identifier_impl(scanner, "hide", false)? {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let (_, _, idents, vars) = member_list_impl(scanner, state)?;
        hidden_mf = Some(idents);
        hidden_vars = Some(vars);
    }
    let configuration = configuration_impl(scanner, state, true)?;
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    expect_statement_separator_impl(scanner, state, "@forward rule")?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    if !state.is_use_allowed {
        return Err(Box::new(error_impl(
            "@forward rules must be written before any other rules.",
            &span,
        )));
    }
    if let Some(sm) = shown_mf {
        return Ok(ForwardRule::show_ordered(
            url,
            sm,
            shown_vars.unwrap_or_default(),
            shown_order_mf.unwrap_or_default(),
            shown_order_vars.unwrap_or_default(),
            span,
            prefix,
            configuration,
        ));
    }
    if let Some(hm) = hidden_mf {
        return Ok(ForwardRule::hide(
            url,
            hm,
            hidden_vars.unwrap_or_default(),
            span,
            prefix,
            configuration,
        ));
    }
    Ok(ForwardRule::new(url, span, prefix, configuration))
}

// Matches Dart: `_memberList` (stylesheet.dart:1131). Consumes the `show`
// / `hide` member list of `@forward`: plain identifiers first in the tuple,
// variable names second. Duplicates collapse; `show` additionally keeps
// source order alongside the sets (see `forward_rule_impl`).
// Dart `_memberList` 4-tuple, destructured once at the call site; a named
// alias would be single-use indirection.
#[allow(clippy::type_complexity)]
fn member_list_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, (Vec<String>, Vec<String>, HashSet<String>, HashSet<String>)> {
    let mut idents = Vec::new();
    let mut idents_set = HashSet::new();
    let mut vars = Vec::new();
    let mut vars_set = HashSet::new();
    loop {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        with_error_message_impl(
            scanner,
            &mut state.parser_state,
            "Expected variable, mixin, or function name",
            |scanner, parser_state| {
                if scanner.peek_char(0) == '$' as i32 {
                    let name = variable_name_impl(scanner, parser_state)?;
                    if !vars_set.contains(&name) {
                        vars.push(name.clone());
                        vars_set.insert(name);
                    }
                } else {
                    let name = identifier_impl(scanner, parser_state, true, false)?;
                    if !idents_set.contains(&name) {
                        idents.push(name.clone());
                        idents_set.insert(name);
                    }
                }
                Ok(())
            },
        )?;
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        if !scanner.scan_char(',') {
            break;
        }
    }
    Ok((
        idents.clone(),
        vars.clone(),
        idents.into_iter().collect::<HashSet<String>>(),
        vars.into_iter().collect::<HashSet<String>>(),
    ))
}

// ======================================================================
// configuration
// ======================================================================

// Matches Dart: `_configuration` (stylesheet.dart:1672). Returns the
// configured variables from a `@use`/`@forward` `with` clause, or an empty
// vec when there is none (Dart returns `null`; Rust uses an empty vec).
// When `allow_guarded` is set, `!default`-flagged entries are accepted.
fn configuration_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    allow_guarded: bool,
) -> ParseResult<'b, Vec<ConfiguredVariable<'b>>> {
    if !scan_identifier_impl(scanner, "with", false)? {
        return Ok(Vec::new());
    }
    let mut variable_names = HashMap::new();
    let mut configuration = Vec::new();
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    scanner.expect_char('(')?;
    loop {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let variable_start = scanner.state();
        let name = variable_name_impl(scanner, &state.parser_state)?;
        if name.starts_with('-') {
            let span = span_from_impl(scanner, &state.parser_state, variable_start)?.file_span()?;
            state.warnings.push(ParseTimeWarning {
                deprecation: Some(&deprecation::WITH_PRIVATE),
                message: "Configuring private variables is deprecated.\nThis will be an error in Dart Sass 2.0.0.".into(),
                span,
                primary_label: None,
                secondary: vec![],
            });
        }
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        scanner.expect_char(':')?;
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let expression = expression_until_comma(scanner, state, false)?;
        let mut guarded = false;
        if allow_guarded && scanner.scan_char('!') {
            let flag_name = identifier_impl(scanner, &state.parser_state, true, false)?;
            if flag_name == "default" {
                guarded = true;
                whitespace_impl(scanner, &mut state.parser_state, true)?;
            } else {
                let flag_start = scanner.state();
                let flag_span =
                    span_from_impl(scanner, &state.parser_state, flag_start)?.file_span()?;
                return Err(Box::new(error_impl("Invalid flag name.", &flag_span)));
            }
        }
        let span = span_from_impl(scanner, &state.parser_state, variable_start)?.file_span()?;
        if variable_names.contains_key(&name) {
            return Err(Box::new(error_impl(
                "The same variable may only be configured once.",
                &span,
            )));
        }
        variable_names.insert(name.clone(), ());
        configuration.push(ConfiguredVariable::new(name, expression, span, guarded));
        if !scanner.scan_char(',') {
            break;
        }
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        if !looking_at_expression_impl(scanner) {
            break;
        }
    }
    scanner.expect_char(')')?;
    Ok(configuration)
}

// ======================================================================
// parameterList
// ======================================================================

/// Consumes a parameter list.
///
/// Matches Dart: `_parameterList` (stylesheet.dart:1804). Duplicate names
/// error against the new parameter's span.
pub(crate) fn parameter_list_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, ParameterList<'b>> {
    let start = scanner.state();
    scanner.expect_char('(')?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let mut parameters: Vec<Parameter<'b>> = Vec::new();
    let mut named: HashMap<String, ()> = HashMap::new();
    let mut rest_parameter: Option<String> = None;
    while scanner.peek_char(0) == '$' as i32 {
        let variable_start = scanner.state();
        let name = variable_name_impl(scanner, &state.parser_state)?;
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        let default_value;
        if scanner.scan_char(':') {
            whitespace_impl(scanner, &mut state.parser_state, true)?;
            default_value = Some(expression_until_comma(scanner, state, false)?);
        } else if scanner.scan_char('.') {
            scanner.expect_char('.')?;
            scanner.expect_char('.')?;
            whitespace_impl(scanner, &mut state.parser_state, true)?;
            if scanner.scan_char(',') {
                whitespace_impl(scanner, &mut state.parser_state, true)?;
            }
            rest_parameter = Some(name);
            break;
        } else {
            default_value = None;
        }
        let param_span =
            span_from_impl(scanner, &state.parser_state, variable_start)?.file_span()?;
        parameters.push(Parameter::new(name.clone(), param_span, default_value));
        if named.contains_key(&name) {
            let dup_span = parameters.last().unwrap().span()?;
            return Err(Box::new(error_impl("Duplicate parameter.", &dup_span)));
        }
        named.insert(name, ());
        if !scanner.scan_char(',') {
            break;
        }
        whitespace_impl(scanner, &mut state.parser_state, true)?;
    }
    scanner.expect_char(')')?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(ParameterList::new(parameters, span, rest_parameter))
}

// ======================================================================
// importRule + importArgument + tryImportModifiers
// ======================================================================

// Matches Dart: `_importRule` (stylesheet.dart:1185). `start` points
// before the `@`. Dynamic imports warn (deprecated) and are rejected inside
// control directives and mixins.
fn import_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, ImportRule<'b>> {
    let mut imports: Vec<Import<'b>> = Vec::new();
    loop {
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        let argument = if let Syntax::Sass(_) = state.parser_state.syntax {
            sass_import_argument_impl(scanner, state)?
        } else {
            import_argument_impl(scanner, state)?
        };
        if let Import::Dynamic(ref di) = argument {
            let dynamic_span = di.span()?;
            state.warnings.push(ParseTimeWarning {
                deprecation: Some(&deprecation::IMPORT),
                message: "Sass @import rules are deprecated and will be removed in Dart Sass 3.0.0.\n\nMore info and automated migrator: https://sass-lang.com/d/import".into(),
                span: dynamic_span,
                primary_label: None,
                secondary: vec![],
            });
            if state.in_control_directive || state.in_mixin {
                let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                return Err(Box::new(error_impl(
                    "This at-rule is not allowed here.",
                    &span,
                )));
            }
        }
        imports.push(argument);
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        if !scanner.scan_char(',') {
            break;
        }
    }
    expect_statement_separator_impl(scanner, state, "@import rule")?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(ImportRule::new(imports, span))
}

/// Consumes an argument to an `@import` rule.
///
/// Matches Dart: `importArgument` (stylesheet.dart:1217). `url(…)` (any
/// case) and plain-CSS URLs / modifier-carrying imports become static
/// imports; anything else is validated and kept as a dynamic import.
pub(crate) fn import_argument_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Import<'b>> {
    let start = scanner.state();
    let ch = scanner.peek_char(0);
    if ch == 'u' as i32 || ch == 'U' as i32 {
        let url_expr = dynamic_url_impl(scanner, state)?;
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        let modifiers = try_import_modifiers_impl(scanner, state)?;
        let interp = if let Expression::String(ref se) = url_expr {
            se.text.clone()
        } else {
            let expr_span = url_expr.span()?;
            Interpolation::new(
                vec![InterpolationPart::Expression(Box::new(url_expr.clone()))],
                vec![Some(expr_span)],
                Span::File(expr_span),
            )
            .map_err(|e| Box::new(ParseError::Sass(Box::new(SassError::from(e)))))?
        };
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(Import::Static(StaticImport::new(interp, span, modifiers)));
    }
    let url_str = string_impl(scanner)?;
    let url_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let modifiers = try_import_modifiers_impl(scanner, state)?;
    if is_plain_import_url(&url_str) || modifiers.is_some() {
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        let url_span_text = url_span.text().to_string();
        return Ok(Import::Static(StaticImport::new(
            Interpolation::plain(url_span_text, url_span),
            span,
            modifiers,
        )));
    }
    let parsed_str = parse_import_url(&url_str);
    // Matches Dart: DynamicImport(parseImportUrl(url), urlSpan) — the URL is
    // passed through unchanged; Uri.parse(url) only validates.
    SassUrl::parse(&parsed_str).map_err(|e| error_impl(&format!("Invalid URL: {e}"), &url_span))?;
    Ok(Import::Dynamic(DynamicImport::new(parsed_str, url_span)))
}

/// Consumes modifiers (media or supports queries) after an import argument.
///
/// Matches Dart: `tryImportModifiers` (stylesheet.dart:1282). Returns `None`
/// when no modifiers follow — checked before allocating, since that is the
/// common case.
pub(crate) fn try_import_modifiers_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, Option<Interpolation<'b>>> {
    if !looking_at_interpolated_identifier_impl(scanner) && scanner.peek_char(0) != '(' as i32 {
        return Ok(None);
    }
    let start = scanner.state();
    let mut buffer = InterpolationBuffer::new();
    loop {
        if looking_at_interpolated_identifier_impl(scanner) {
            if !buffer.is_empty() {
                buffer.write_char_code(' ');
            }
            let identifier = interpolated_identifier_impl(scanner, state)?;
            buffer.add_interpolation(&identifier);
            let name = identifier.as_plain();
            let lower = name.map(|n| n.to_lowercase());
            if lower.as_deref() != Some("and") && scanner.scan_char('(') {
                if lower.as_deref() == Some("supports") {
                    let query = import_supports_query_impl(scanner, state)?;
                    let is_decl = matches!(query, SupportsCondition::Declaration(_));
                    if !is_decl {
                        buffer.write_char_code('(');
                    }
                    let query_span = query.span()?;
                    buffer.add(
                        Expression::Supports(SupportsExpression::new(query)),
                        query_span,
                    );
                    if !is_decl {
                        buffer.write_char_code(')');
                    }
                } else {
                    buffer.write_char_code('(');
                    let idecl = interpolated_declaration_value_impl(
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
                    buffer.add_interpolation(&idecl);
                    buffer.write_char_code(')');
                }
                scanner.expect_char(')')?;
                whitespace_impl(scanner, &mut state.parser_state, false)?;
            } else {
                whitespace_impl(scanner, &mut state.parser_state, false)?;
                if scanner.scan_char(',') {
                    buffer.write(", ");
                    let mql = media_query_list_impl(scanner, state)?;
                    buffer.add_interpolation(&mql);
                    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                    return Ok(Some(
                        buffer
                            .interpolation(span)
                            .map_err(|e| Box::new(ParseError::Sass(e)))?,
                    ));
                }
            }
        } else if scanner.peek_char(0) == '(' as i32 {
            if !buffer.is_empty() {
                buffer.write_char_code(' ');
            }
            let mql = media_query_list_impl(scanner, state)?;
            buffer.add_interpolation(&mql);
            let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
            return Ok(Some(
                buffer
                    .interpolation(span)
                    .map_err(|e| Box::new(ParseError::Sass(e)))?,
            ));
        } else {
            let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
            return Ok(Some(
                buffer
                    .interpolation(span)
                    .map_err(|e| Box::new(ParseError::Sass(e)))?,
            ));
        }
    }
}

// ======================================================================
// mixinRule
// ======================================================================

/// Consumes a mixin declaration.
///
/// Matches Dart: `_mixinRule` (stylesheet.dart:1455). `start` points before
/// the `@`. `@mixin` names beginning with `--` are rejected for
/// forward-compatibility with plain CSS mixins; nested mixin declarations
/// and declarations inside control directives are rejected.
pub(crate) fn mixin_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, MixinRule<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let preceding_comment = state.last_silent_comment.take();
    let before_name = scanner.state();
    let name = identifier_impl(scanner, &state.parser_state, false, false)?;
    if name.starts_with("--") {
        let span = span_from_impl(scanner, &state.parser_state, before_name)?.file_span()?;
        let msg = concat!(
            "Sass @mixin names beginning with -- are forbidden for forward-compatibility with plain CSS mixins.\n",
            "\n",
            "For details, see https://sass-lang.com/d/css-function-mixin",
        );
        return Err(Box::new(error_impl(msg, &span)));
    }
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let parameters = if scanner.peek_char(0) == '(' as i32 {
        parameter_list_impl(scanner, state)?
    } else {
        ParameterList::empty(scanner.empty_span())
    };
    if state.in_mixin || state.in_content_block {
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Err(Box::new(error_impl(
            "Mixins may not contain mixin declarations.",
            &span,
        )));
    } else if state.in_control_directive {
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Err(Box::new(error_impl(
            "Mixins may not be declared in control directives.",
            &span,
        )));
    }
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    state.in_mixin = true;
    let n = name.clone();
    let p = parameters.clone();
    let comment = preceding_comment.map(|b| *b);
    let result = with_children_impl(
        scanner,
        state,
        &mut |s, st| statement_impl(s, st, false).map(Some),
        start,
        |_, children, span| Ok(MixinRule::new(n, p, children, span, comment)),
    )?;
    state.in_mixin = false;
    Ok(result)
}

// ======================================================================
// functionRule
// ======================================================================

// Matches Dart: `_functionRule` (stylesheet.dart:951). Consumes a function
// declaration; `at_rule_name` is the already-parsed `@…` name, used to
// route a `--`-prefixed name to the unknown-at-rule path. `type` and the
// CSS-special names (`expression`, `url`, `and`/`or`/`not`, vendor `element`)
// are rejected; lowercase `expression`/`url`/vendor-`element` warn
// (deprecated) instead. Children parse via `function_child_impl`, and the
// preceding silent comment is captured for the node.
fn function_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
    at_rule_name: &Interpolation<'b>,
) -> ParseResult<'b, Statement<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let preceding_comment = state.last_silent_comment.take();
    let before_name = scanner.state();
    if scanner.peek_char(0) == '-' as i32 && scanner.peek_char(1) == '-' as i32 {
        return unknown_at_rule_impl(scanner, state, start, at_rule_name);
    }
    let name = identifier_impl(scanner, &state.parser_state, false, false)?;
    if name.eq_ignore_ascii_case("type") {
        let span = span_from_impl(scanner, &state.parser_state, before_name)?.file_span()?;
        return Err(Box::new(error_impl(
            "This name is reserved for the plain-CSS function.",
            &span,
        )));
    }
    match name.as_str() {
        "expression" | "url" | "and" | "or" | "not" => {
            let span = span_from_impl(scanner, &state.parser_state, before_name)?.file_span()?;
            return Err(Box::new(error_impl("Invalid function name.", &span)));
        }
        _ => {
            if unvendor(&name) == "element" {
                let span =
                    span_from_impl(scanner, &state.parser_state, before_name)?.file_span()?;
                return Err(Box::new(error_impl("Invalid function name.", &span)));
            }
        }
    }
    let lower = name.to_lowercase();
    let warn = match lower.as_str() {
        "expression" | "url" => true,
        _ => unvendor(&lower) == "element",
    };
    if warn {
        let span = span_from_impl(scanner, &state.parser_state, before_name)?.file_span()?;
        state.warnings.push(ParseTimeWarning {
            deprecation: Some(&deprecation::FUNCTION_NAME),
            message: "Custom functions with this name are deprecated and will be removed in a future\nrelease. Please choose a different name.\nMore info: https://sass-lang.com/d/function-name".into(),
            span,
            primary_label: None,
            secondary: vec![],
        });
    }
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let parameters = parameter_list_impl(scanner, state)?;
    if state.in_mixin || state.in_content_block {
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Err(Box::new(error_impl(
            "Mixins may not contain function declarations.",
            &span,
        )));
    } else if state.in_control_directive {
        let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Err(Box::new(error_impl(
            "Functions may not be declared in control directives.",
            &span,
        )));
    }
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let n = name.clone();
    let p = parameters.clone();
    let comment = preceding_comment.map(|b| *b);
    with_children_impl(
        scanner,
        state,
        &mut |s, st| function_child_impl(s, st).map(Some),
        start,
        |_, children, span| {
            Ok(Statement::FunctionRule(FunctionRule::new(
                n, p, children, span, comment,
            )))
        },
    )
}

// ======================================================================
// eachRule
// ======================================================================

// Matches Dart: `_eachRule` (stylesheet.dart:891). `start` points before
// the `@`; `child` consumes context-specific children. Sets
// `in_control_directive` while parsing the header and body.
fn each_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
    child: &mut dyn FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Statement<'b>>,
) -> ParseResult<'b, EachRule<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let was_in_control_directive = state.in_control_directive;
    state.in_control_directive = true;
    let mut variables = vec![variable_name_impl(scanner, &state.parser_state)?];
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    while scanner.scan_char(',') {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        variables.push(variable_name_impl(scanner, &state.parser_state)?);
        whitespace_impl(scanner, &mut state.parser_state, true)?;
    }
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    expect_identifier_impl(scanner, "in", "", true)?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let list = _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        },
    )?;
    let vars = variables.clone();
    let l = list.clone();
    let result = with_children_impl(
        scanner,
        state,
        &mut |s, st| child(s, st).map(Some),
        start,
        |_, children, span| Ok(EachRule::new(vars, l, children, span)),
    )?;
    state.in_control_directive = was_in_control_directive;
    Ok(result)
}

// ======================================================================
// forRule
// ======================================================================

/// Consumes a `@for` rule.
///
/// Matches Dart: `_forRule` (stylesheet.dart:1017). `start` points before
/// the `@`; `child` consumes context-specific children. The `to`/`through`
/// keyword is scanned via the expression's `until` hook; a missing keyword
/// errors with `Expected "to" or "through".`.
pub(crate) fn for_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
    child: &mut dyn FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Statement<'b>>,
) -> ParseResult<'b, ForRule<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let was_in_control_directive = state.in_control_directive;
    state.in_control_directive = true;
    let variable = variable_name_impl(scanner, &state.parser_state)?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    expect_identifier_impl(scanner, "from", "", true)?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let mut exclusive: Option<bool> = None;
    let mut scan_err: Option<Box<ParseError<'b>>> = None;
    let from = _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: true,
            until: Some(Box::new(|s: &mut SpanScanner<'b>| {
                if !looking_at_identifier(s, None) {
                    return false;
                }
                match scan_identifier_impl(s, "to", false) {
                    Ok(true) => {
                        exclusive = Some(true);
                        return true;
                    }
                    Err(e) => {
                        scan_err = Some(e);
                        return true;
                    }
                    _ => {}
                }
                match scan_identifier_impl(s, "through", false) {
                    Ok(true) => {
                        exclusive = Some(false);
                        return true;
                    }
                    Err(e) => {
                        scan_err = Some(e);
                        return true;
                    }
                    _ => {}
                }
                false
            })),
        },
    )?;
    if let Some(e) = scan_err {
        return Err(e);
    }
    let exclusive = exclusive
        .ok_or_else(|| error_impl("Expected \"to\" or \"through\".", &scanner.empty_span()))?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let to = _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        },
    )?;
    let v = variable.clone();
    let f = from;
    let t = to;
    let exc = exclusive;
    let result = with_children_impl(
        scanner,
        state,
        &mut |s, st| child(s, st).map(Some),
        start,
        |_, children, span| Ok(ForRule::new(v, f, t, children, span, exc)),
    )?;
    state.in_control_directive = was_in_control_directive;
    Ok(result)
}

// ======================================================================
// ifRule
// ======================================================================

// Matches Dart: `_ifRule` (stylesheet.dart:1153). `start` points before
// the `@`; `child` consumes context-specific children. `@else`/`@else if`
// continuation is scanned with `scan_else` using the pre-`@if` indentation;
// the span covers through the final clause plus trailing comment-free
// whitespace.
fn if_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
    child: &mut dyn FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Statement<'b>>,
) -> ParseResult<'b, IfRule<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let _if_indentation = current_indentation_impl(state);
    let was_in_control_directive = state.in_control_directive;
    state.in_control_directive = true;
    let condition = _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        },
    )?;
    let children = children_impl(scanner, state, &mut |s, st| child(s, st).map(Some))?;
    whitespace_without_comments_impl(scanner, &state.parser_state, false)?;
    let mut clauses = vec![IfClause::new(condition, children)];
    let mut last_clause: Option<Vec<Statement<'b>>> = None;
    loop {
        if !scan_else_impl(scanner, state, _if_indentation)? {
            break;
        }
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        if scan_identifier_impl(scanner, "if", false)? {
            whitespace_impl(scanner, &mut state.parser_state, true)?;
            let cond = _expression_impl(
                scanner,
                state,
                ExpressionOpts {
                    bracket_list: false,
                    single_equals: false,
                    consume_newlines: false,
                    until: None,
                },
            )?;
            let ch = children_impl(scanner, state, &mut |s, st| child(s, st).map(Some))?;
            clauses.push(IfClause::new(cond, ch));
        } else {
            let ch = children_impl(scanner, state, &mut |s, st| child(s, st).map(Some))?;
            last_clause = Some(ch);
            break;
        }
    }
    state.in_control_directive = was_in_control_directive;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    whitespace_without_comments_impl(scanner, &state.parser_state, false)?;
    Ok(IfRule::new(clauses, span, last_clause))
}

// ======================================================================
// whileRule
// ======================================================================

// Matches Dart: `_whileRule` (stylesheet.dart:1744). `start` points before
// the `@`; `child` consumes context-specific children. Sets
// `in_control_directive` while parsing.
fn while_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
    child: &mut dyn FnMut(
        &mut SpanScanner<'b>,
        &mut StylesheetState<'b>,
    ) -> ParseResult<'b, Statement<'b>>,
) -> ParseResult<'b, WhileRule<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let was_in_control_directive = state.in_control_directive;
    state.in_control_directive = true;
    let condition = _expression_impl(
        scanner,
        state,
        ExpressionOpts {
            bracket_list: false,
            single_equals: false,
            consume_newlines: false,
            until: None,
        },
    )?;
    let c = condition;
    let result = with_children_impl(
        scanner,
        state,
        &mut |s, st| child(s, st).map(Some),
        start,
        |_, children, span| Ok(WhileRule::new(c, children, span)),
    )?;
    state.in_control_directive = was_in_control_directive;
    Ok(result)
}

// ======================================================================
// mediaRule + supportsRule
// ======================================================================

/// Consumes a `@media` rule.
///
/// Matches Dart: `mediaRule` (stylesheet.dart:1442). `start` points before
/// the `@`.
pub(crate) fn media_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, MediaRule<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let query = media_query_list_impl(scanner, state)?;
    let q = query;
    with_children_impl(
        scanner,
        state,
        &mut |s, st| statement_impl(s, st, false).map(Some),
        start,
        |_, children, span| Ok(MediaRule::new(q, children, span)),
    )
}

/// Consumes a `@supports` rule.
///
/// Matches Dart: `supportsRule` (stylesheet.dart:1604). `start` points
/// before the `@`.
pub(crate) fn supports_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, SupportsRule<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let condition = supports_condition_impl(scanner, state, false)?;
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let cond = condition;
    with_children_impl(
        scanner,
        state,
        &mut |s, st| statement_impl(s, st, false).map(Some),
        start,
        |_, children, span| Ok(SupportsRule::new(cond, children, span)),
    )
}

// ======================================================================
// includeRule
// ======================================================================

/// Consumes an `@include` rule.
///
/// Matches Dart: `_includeRule` (stylesheet.dart:1390). `start` points
/// before the `@`. A `using` parameter list and/or a child block becomes a
/// [`ContentBlock`] (parsed with `in_content_block` set); otherwise a
/// statement separator is expected. The span stretches from `start` through
/// the content block or argument list.
pub(crate) fn include_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, IncludeRule<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, true)?;
    let mut namespace = String::new();
    let mut name = identifier_impl(scanner, &state.parser_state, false, false)?;
    if scanner.scan_char('.') {
        namespace = name.clone();
        name = public_identifier_impl(scanner, &state.parser_state)?;
    }
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let arguments = if scanner.peek_char(0) == '(' as i32 {
        argument_invocation(scanner, state, true, false)?
    } else {
        ArgumentList::empty(scanner.empty_span())
    };
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let mut content_parameters: Option<ParameterList<'b>> = None;
    if scan_identifier_impl(scanner, "using", false)? {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        content_parameters = Some(parameter_list_impl(scanner, state)?);
        whitespace_impl(scanner, &mut state.parser_state, false)?;
    }
    let mut content: Option<ContentBlock<'b>> = None;
    let has_content_children = looking_at_children_impl(scanner, state)?;
    if content_parameters.is_some() || has_content_children {
        let cp = content_parameters
            .take()
            .unwrap_or_else(|| ParameterList::empty(scanner.empty_span()));
        let was_in_content_block = state.in_content_block;
        state.in_content_block = true;
        let cb = with_children_impl(
            scanner,
            state,
            &mut |s, st| statement_impl(s, st, false).map(Some),
            start,
            |_, children, span| Ok(ContentBlock::new(cp, children, span)),
        )?;
        state.in_content_block = was_in_content_block;
        content = Some(cb);
    } else {
        expect_statement_separator_impl(scanner, state, "")?;
    }
    let ns = if namespace.is_empty() {
        None
    } else {
        Some(namespace)
    };
    let span = if let Some(ref content) = content {
        let content_span = content.span()?;
        let span_from =
            span_from_to_impl(scanner, &state.parser_state, start, Some(&start))?.file_span()?;
        span_from.expand(&Span::File(content_span))?
    } else {
        let arguments_span = arguments.span()?;
        let span_from =
            span_from_to_impl(scanner, &state.parser_state, start, Some(&start))?.file_span()?;
        span_from.expand(&Span::File(arguments_span))?
    };
    Ok(IncludeRule::new(name, arguments, span, ns, content))
}

// ======================================================================
// mozDocumentRule
// ======================================================================

/// Consumes a `@-moz-document` rule.
///
/// Matches Dart: `mozDocumentRule` (stylesheet.dart:1512). Gecko's rule
/// diverges from the spec by letting `url-prefix`/`domain` omit quotes;
/// anything but `url`/`url-prefix`/`domain`/`regexp` errors with `Invalid
/// function name.` A bare or empty-string `url-prefix()` is not (yet)
/// deprecated; every other form records a deprecation warning on the rule
/// span.
pub(crate) fn moz_document_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
    name: &Interpolation<'b>,
) -> ParseResult<'b, AtRule<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let value_start = scanner.state();
    let mut buffer = InterpolationBuffer::new();
    let mut needs_deprecation_warning = false;
    loop {
        if scanner.peek_char(0) == '#' as i32 {
            let (expr, span) = single_interpolation_impl(scanner, state)?;
            buffer.add(expr, span);
            needs_deprecation_warning = true;
        } else {
            let identifier_start = scanner.state();
            let identifier = identifier_impl(scanner, &state.parser_state, true, false)?;
            match identifier.as_str() {
                "url" | "url-prefix" | "domain" => {
                    let contents =
                        try_url_contents(scanner, state, identifier_start, &identifier, false)?;
                    if let Some(c) = contents {
                        buffer.add_interpolation(&c);
                    } else {
                        scanner.expect_char('(')?;
                        whitespace_impl(scanner, &mut state.parser_state, false)?;
                        let argument = interpolated_string_token_impl(scanner, state)?;
                        scanner.expect_char(')')?;
                        buffer.write(&identifier);
                        buffer.write_char_code('(');
                        buffer.add_interpolation(&argument);
                        buffer.write_char_code(')');
                    }
                    let trailing = buffer.trailing_string();
                    if !trailing.ends_with("url-prefix()")
                        && !trailing.ends_with("url-prefix('')")
                        && !trailing.ends_with("url-prefix(\"\")")
                    {
                        needs_deprecation_warning = true;
                    }
                }
                "regexp" => {
                    buffer.write("regexp(");
                    scanner.expect_char('(')?;
                    let ist = interpolated_string_token_impl(scanner, state)?;
                    buffer.add_interpolation(&ist);
                    scanner.expect_char(')')?;
                    buffer.write_char_code(')');
                    needs_deprecation_warning = true;
                }
                _ => {
                    let span = span_from_impl(scanner, &state.parser_state, identifier_start)?
                        .file_span()?;
                    return Err(Box::new(error_impl("Invalid function name.", &span)));
                }
            }
        }
        whitespace_impl(scanner, &mut state.parser_state, false)?;
        if !scanner.scan_char(',') {
            break;
        }
        buffer.write_char_code(',');
        let text = raw_text_impl(scanner, &mut state.parser_state, |s, st| {
            whitespace_impl(s, st, false)
        })?
        .1;
        buffer.write(&text);
    }
    let value_span = span_from_impl(scanner, &state.parser_state, value_start)?.file_span()?;
    let value = Some(
        buffer
            .interpolation(value_span)
            .map_err(|e| Box::new(ParseError::Sass(e)))?,
    );
    let val = value;
    let name2 = name.clone();
    with_children_impl(
        scanner,
        state,
        &mut |s, st| statement_impl(s, st, false).map(Some),
        start,
        |st, children, span| {
            if needs_deprecation_warning {
                st.warnings.push(ParseTimeWarning {
                deprecation: Some(&deprecation::MOZ_DOCUMENT),
                message: "@-moz-document is deprecated and support will be removed in Dart Sass 2.0.0.\n\nFor details, see https://sass-lang.com/d/moz-document.".into(),
                span,
                primary_label: None,
                secondary: vec![],
            });
            }
            Ok(AtRule::new(name2, span, val, Some(children)))
        },
    )
}

// ======================================================================
// unknownAtRule + disallowedAtRule
// ======================================================================

/// Consumes an at-rule not explicitly supported by Sass.
///
/// Matches Dart: `unknownAtRule` (stylesheet.dart:1759). `start` points
/// before the `@`; `name` is the rule name. The value is skipped (not
/// parsed) when it starts with `!` or sits at a statement end; a
/// case-insensitive `function` name flips `in_plain_css_function` for the
/// body. Flags are restored after both the children and separator paths.
pub(crate) fn unknown_at_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
    name: &Interpolation<'b>,
) -> ParseResult<'b, Statement<'b>> {
    let was_in_unknown_at_rule = state.in_unknown_at_rule;
    state.in_unknown_at_rule = true;
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let value = if scanner.peek_char(0) != '!' as i32 && !at_end_of_statement_impl(scanner, state) {
        Some(interpolated_declaration_value_impl(
            scanner,
            state,
            DeclarationValueOpts {
                allow_empty: false,
                allow_semicolon: false,
                allow_colon: true,
                allow_open_brace: false,
                end_after_of: false,
                silent_comments: true,
                consume_newlines: false,
            },
        )?)
    } else {
        None
    };
    let was_in_plain_css_function = state.in_plain_css_function;
    if let Some(plain) = name.as_plain() {
        if plain.eq_ignore_ascii_case("function") {
            state.in_plain_css_function = true;
        }
    }
    let has_children = looking_at_children_impl(scanner, state)?;
    if has_children {
        let n2 = name.clone();
        let v2 = value.clone();
        let result = with_children_impl(
            scanner,
            state,
            &mut |s, st| statement_impl(s, st, false).map(Some),
            start,
            |_, children, span| Ok(Statement::AtRule(AtRule::new(n2, span, v2, Some(children)))),
        )?;
        state.in_unknown_at_rule = was_in_unknown_at_rule;
        state.in_plain_css_function = was_in_plain_css_function;
        return Ok(result);
    }
    expect_statement_separator_impl(scanner, state, "")?;
    state.in_unknown_at_rule = was_in_unknown_at_rule;
    state.in_plain_css_function = was_in_plain_css_function;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(Statement::AtRule(AtRule::new(
        name.clone(),
        span,
        value,
        None,
    )))
}

// Matches Dart: `_disallowedAtRule` (stylesheet.dart:1797). Swallows the
// rule's value text, then errors `This at-rule is not allowed here.` over
// the whole rule span. Returns `Statement` only so it fits in dispatch
// position.
fn disallowed_at_rule_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    start: LineScannerState,
) -> ParseResult<'b, Statement<'b>> {
    whitespace_impl(scanner, &mut state.parser_state, false)?;
    let _ = interpolated_declaration_value_impl(
        scanner,
        state,
        DeclarationValueOpts {
            allow_empty: true,
            allow_semicolon: false,
            allow_colon: true,
            allow_open_brace: false,
            end_after_of: false,
            silent_comments: true,
            consume_newlines: false,
        },
    )?;
    let span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Err(Box::new(error_impl(
        "This at-rule is not allowed here.",
        &span,
    )))
}

// ======================================================================
// Thin wrappers on StylesheetParser
// ======================================================================

impl<'parse> StylesheetParser<'parse> {
    /// Parses one at-rule; see [`at_rule_impl`].
    pub fn at_rule(
        &mut self,
        child: &mut dyn FnMut(
            &mut SpanScanner<'parse>,
            &mut StylesheetState<'parse>,
        ) -> ParseResult<'parse, Statement<'parse>>,
        root: bool,
    ) -> ParseResult<'parse, Statement<'parse>> {
        at_rule_impl(&mut self.scanner, &mut self.state, child, root)
    }

    /// Parses a parameter list; see [`parameter_list_impl`].
    pub fn parameter_list(&mut self) -> ParseResult<'parse, ParameterList<'parse>> {
        parameter_list_impl(&mut self.scanner, &mut self.state)
    }

    /// Parses a `@use`/`@forward` `with` clause; see [`configuration_impl`].
    pub fn configuration(
        &mut self,
        allow_guarded: bool,
    ) -> ParseResult<'parse, Vec<ConfiguredVariable<'parse>>> {
        configuration_impl(&mut self.scanner, &mut self.state, allow_guarded)
    }

    /// Parses a `@forward` rule; see `forward_rule_impl`.
    pub fn forward_rule(
        &mut self,
        start: LineScannerState,
    ) -> ParseResult<'parse, ForwardRule<'parse>> {
        forward_rule_impl(&mut self.scanner, &mut self.state, start)
    }

    /// Parses a `@use` rule; see [`use_rule_impl`].
    pub fn use_rule(&mut self, start: LineScannerState) -> ParseResult<'parse, UseRule<'parse>> {
        use_rule_impl(&mut self.scanner, &mut self.state, start)
    }
}
