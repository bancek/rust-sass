// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/stylesheet.dart (selector sections)
// go-source: go/value/parse_stylesheet_selector.go

//! Stylesheet-level interpolated selector parsing.
//!
//! Consumes selectors that may still contain `#{}` interpolation, producing
//! the `Interpolated*` AST. Resolution against the selector extension system
//! happens later; see `selector_parse.rs` for the interpolation-free pass.
//
// What follows is largely duplicated between here and `selector_parse.rs`.
// Most changes here should be mirrored there and vice versa.

use crate::ast::sass::interpolated_selector::{
    InterpolatedAttributeSelector, InterpolatedClassSelector, InterpolatedComplexSelector,
    InterpolatedComplexSelectorComponent, InterpolatedCompoundSelector, InterpolatedIDSelector,
    InterpolatedParentSelector, InterpolatedPlaceholderSelector, InterpolatedPseudoSelector,
    InterpolatedQualifiedName, InterpolatedSelectorList, InterpolatedSimpleSelector,
    InterpolatedTypeSelector, InterpolatedUniversalSelector,
};
use crate::ast::sass::interpolation::Interpolation;
use crate::common::ast_css_value::CssValue;
use crate::common::span_scanner::SpanScanner;
use crate::common::SassError;
use crate::selector::attribute::AttributeOperator;
use crate::selector::combinator::Combinator;
use crate::unvendor::unvendor;

use crate::parse::anyvalue::{interpolated_declaration_value_impl, DeclarationValueOpts};
use crate::parse::expression::interpolated_string_token_impl;
use crate::parse::identifier::{interpolated_identifier_body_impl, interpolated_identifier_impl};
use crate::parse::parser::{
    error_impl, span_from_impl, span_from_to_impl, whitespace_impl, ParseError, ParseResult,
};
use crate::parse::selector_parse::{SELECTOR_PSEUDO_CLASSES, SELECTOR_PSEUDO_ELEMENTS};
use crate::parse::stylesheet::{StylesheetParser, StylesheetState, Syntax};
use crate::parse::util::looking_at_interpolated_identifier_body_impl;
use crate::parse::util::looking_at_interpolated_identifier_impl;

// ======================================================================
// selector_list_impl — consumes a comma-separated selector list.
// ======================================================================

// Consumes a comma-separated selector list.
//
// Doubled commas are skipped and a trailing comma ends the list, matching
// Dart's `_selectorList`.
pub(crate) fn selector_list_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, InterpolatedSelectorList<'b>> {
    let _ = scanner.line();
    let first = complex_selector_impl(scanner, state, false, true, true)?;
    let mut components = vec![first];
    let mut previous_line = scanner.line();

    whitespace_impl(scanner, &mut state.parser_state, false)?;
    while scanner.scan_char(',') {
        whitespace_impl(scanner, &mut state.parser_state, true)?;
        if scanner.peek_char(0) == ',' as i32 {
            continue;
        }
        if scanner.is_done() {
            break;
        }

        let line_break = scanner.line() != previous_line;
        if line_break {
            previous_line = scanner.line();
        }
        let sel = complex_selector_impl(scanner, state, line_break, true, true)?;
        components.push(sel);
    }

    InterpolatedSelectorList::new(components).map_err(|e| {
        Box::new(ParseError::Sass(Box::new(SassError::Script {
            message: e.to_string(),
            argument_name: None,
        })))
    })
}

// ======================================================================
// complex_selector_impl — consumes a complex selector.
// ======================================================================

// Consumes a complex selector.
//
// If `line_break` is set, there was a line break before this selector.
// A leading combinator is kept as [`InterpolatedComplexSelector::leading_combinator`];
// a trailing combinator is an `"expected selector."` error when
// `allow_trailing_combinator` is false (or always in plain CSS).
pub(crate) fn complex_selector_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    _line_break: bool,
    allow_leading_combinator: bool,
    allow_trailing_combinator: bool,
) -> ParseResult<'b, InterpolatedComplexSelector<'b>> {
    let start = scanner.state();
    let mut component_start = scanner.state();
    let mut last_compound: Option<InterpolatedCompoundSelector<'b>> = None;
    let mut combinator: Option<CssValue<'b, Combinator>> = None;
    let mut leading_combinator: Option<CssValue<'b, Combinator>> = None;
    let mut components: Vec<InterpolatedComplexSelectorComponent<'b>> = Vec::new();

    loop {
        whitespace_impl(scanner, &mut state.parser_state, false)?;

        let allow_combinator =
            combinator.is_none() && (allow_leading_combinator || last_compound.is_some());

        let ch = scanner.peek_char(0);
        match ch {
            c if c == '+' as i32 && allow_combinator => {
                let cstart = scanner.state();
                scanner.read_char()?;
                let file_span =
                    span_from_impl(scanner, &state.parser_state, cstart)?.file_span()?;
                combinator = Some(CssValue::new(Combinator::NextSibling, file_span));
            }
            c if c == '>' as i32 && allow_combinator => {
                let cstart = scanner.state();
                scanner.read_char()?;
                let file_span =
                    span_from_impl(scanner, &state.parser_state, cstart)?.file_span()?;
                combinator = Some(CssValue::new(Combinator::Child, file_span));
            }
            c if c == '~' as i32 && allow_combinator => {
                let cstart = scanner.state();
                scanner.read_char()?;
                let file_span =
                    span_from_impl(scanner, &state.parser_state, cstart)?.file_span()?;
                combinator = Some(CssValue::new(Combinator::FollowingSibling, file_span));
            }
            c if c < 0 => break,

            c if c == '[' as i32
                || c == '.' as i32
                || c == '#' as i32
                || c == '%' as i32
                || c == ':' as i32
                || c == '&' as i32
                || c == '*' as i32
                || c == '|' as i32 =>
            {
                if let Some(ref lc) = last_compound {
                    let file_span = span_from_impl(scanner, &state.parser_state, component_start)?
                        .file_span()?;
                    components.push(InterpolatedComplexSelectorComponent::new(
                        lc.clone(),
                        file_span,
                        combinator.take(),
                    ));
                } else if combinator.is_some() {
                    leading_combinator = combinator.take();
                    component_start = scanner.state();
                }

                last_compound = Some(compound_selector_impl(scanner, state)?);
                combinator = None;
                if scanner.peek_char(0) == '&' as i32 {
                    return Err(scanner
                        .error(
                            "\"&\" may only used at the beginning of a compound selector.",
                            None,
                            0,
                        )
                        .into());
                }
            }

            _ if looking_at_interpolated_identifier_impl(scanner) => {
                if let Some(ref lc) = last_compound {
                    let file_span = span_from_impl(scanner, &state.parser_state, component_start)?
                        .file_span()?;
                    components.push(InterpolatedComplexSelectorComponent::new(
                        lc.clone(),
                        file_span,
                        combinator.take(),
                    ));
                } else if combinator.is_some() {
                    leading_combinator = combinator.take();
                    component_start = scanner.state();
                }

                last_compound = Some(compound_selector_impl(scanner, state)?);
                combinator = None;
                if scanner.peek_char(0) == '&' as i32 {
                    return Err(scanner
                        .error(
                            "\"&\" may only used at the beginning of a compound selector.",
                            None,
                            0,
                        )
                        .into());
                }
            }

            _ => break,
        }
    }

    let is_plain_css = matches!(state.parser_state.syntax, Syntax::Css(_));
    if combinator.is_some() && (is_plain_css || !allow_trailing_combinator) {
        return Err(Box::new(
            scanner.error("expected selector.", None, 0).into(),
        ));
    } else if last_compound.is_some() {
        let file_span =
            span_from_impl(scanner, &state.parser_state, component_start)?.file_span()?;
        components.push(InterpolatedComplexSelectorComponent::new(
            last_compound.unwrap(),
            file_span,
            combinator.take(),
        ));
    } else if combinator.is_some() {
        leading_combinator = combinator.take();
    } else {
        return Err(Box::new(
            scanner.error("expected selector.", None, 0).into(),
        ));
    }

    let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    InterpolatedComplexSelector::new(components, file_span, leading_combinator).map_err(|e| {
        Box::new(ParseError::Sass(Box::new(SassError::Script {
            message: e.to_string(),
            argument_name: None,
        })))
    })
}

// compound_selector_impl — consumes a compound selector: one simple selector
// followed by zero or more simple selectors with no combinator between them.
// Non-initial `&` continuations follow the plain-CSS rule (`allow_parent` is
// the stylesheet's plain-CSS flag).

pub(crate) fn compound_selector_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, InterpolatedCompoundSelector<'b>> {
    let first = simple_selector_impl(scanner, state, true)?;
    let mut components = vec![first];

    let is_plain_css = matches!(state.parser_state.syntax, Syntax::Css(_));
    while is_simple_selector_start(scanner.peek_char(0), is_plain_css) {
        let sel = simple_selector_impl(scanner, state, is_plain_css)?;
        components.push(sel);
    }

    InterpolatedCompoundSelector::new(components).map_err(|e| {
        Box::new(ParseError::Sass(Box::new(SassError::Script {
            message: e.to_string(),
            argument_name: None,
        })))
    })
}

// is_simple_selector_start — whether `ch` can start a simple selector in the
// middle of a compound selector. `&` only continues a compound in plain CSS
// (in Sass it is only valid at the start, enforced by the `&` error below).

fn is_simple_selector_start(ch: i32, is_plain_css: bool) -> bool {
    match ch as u8 as char {
        '*' | '[' | '.' | '#' | '%' | ':' => true,
        '&' => is_plain_css,
        _ => false,
    }
}

// simple_selector_impl — consumes a simple selector.
//
// If `allow_parent` is set, the parent selector `&` is allowed here;
// otherwise it is a `"Parent selectors aren't allowed here."` error.
// `%` placeholders are rejected in plain CSS. `#` followed by `#{`
// falls through to the type/universal branch so the interpolation parses
// as a type selector.

pub(crate) fn simple_selector_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
    allow_parent: bool,
) -> ParseResult<'b, InterpolatedSimpleSelector<'b>> {
    let start = scanner.state();
    let is_plain_css = matches!(state.parser_state.syntax, Syntax::Css(_));

    match scanner.peek_char(0) {
        c if c == '[' as i32 => Ok(InterpolatedSimpleSelector::Attribute(
            attribute_selector_impl(scanner, state)?,
        )),
        c if c == '.' as i32 => Ok(InterpolatedSimpleSelector::Class(class_selector_impl(
            scanner, state,
        )?)),
        c if c == '#' as i32 && scanner.peek_char(1) != '{' as i32 => Ok(
            InterpolatedSimpleSelector::ID(id_selector_impl(scanner, state)?),
        ),
        c if c == '#' as i32 => Ok(type_or_universal_selector_impl(scanner, state)?),
        c if c == '%' as i32 => {
            let sel = placeholder_selector_impl(scanner, state)?;
            if is_plain_css {
                let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                return Err(Box::new(error_impl(
                    "Placeholder selectors aren't allowed in plain CSS.",
                    &file_span,
                )));
            }
            Ok(InterpolatedSimpleSelector::Placeholder(sel))
        }
        c if c == ':' as i32 => Ok(InterpolatedSimpleSelector::Pseudo(pseudo_selector_impl(
            scanner, state,
        )?)),
        c if c == '&' as i32 => {
            let sel = parent_selector_impl(scanner, state)?;
            if !allow_parent {
                let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
                return Err(Box::new(error_impl(
                    "Parent selectors aren't allowed here.",
                    &file_span,
                )));
            }
            Ok(InterpolatedSimpleSelector::Parent(sel))
        }
        _ => Ok(type_or_universal_selector_impl(scanner, state)?),
    }
}

// attribute_selector_impl — consumes `[name]`, `[name op value]`, and the
// optional case modifier (`[href=val i]`).

pub(crate) fn attribute_selector_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, InterpolatedAttributeSelector<'b>> {
    let start = scanner.state();
    scanner.expect_char('[')?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;

    let name = attribute_name_impl(scanner, state)?;

    whitespace_impl(scanner, &mut state.parser_state, true)?;
    if scanner.scan_char(']') {
        let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(InterpolatedAttributeSelector::new(name, file_span));
    }

    let op = attribute_operator_impl(scanner, state)?;
    whitespace_impl(scanner, &mut state.parser_state, true)?;

    let next = scanner.peek_char(0);
    let value = if next == '\'' as i32 || next == '"' as i32 {
        interpolated_string_token_impl(scanner, state)?
    } else {
        interpolated_identifier_impl(scanner, state)?
    };
    whitespace_impl(scanner, &mut state.parser_state, true)?;

    let modifier = if looking_at_interpolated_identifier_impl(scanner) {
        Some(interpolated_identifier_impl(scanner, state)?)
    } else {
        None
    };
    whitespace_impl(scanner, &mut state.parser_state, true)?;

    scanner.expect_char(']')?;
    let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(InterpolatedAttributeSelector::with_operator(
        name, op, value, file_span, modifier,
    ))
}

// attribute_name_impl — consumes a qualified name as part of an attribute
// selector: `*|name`, `|name`, `ns|name`, or a bare name. A `|` followed by
// `=` is an operator, not a namespace separator.

pub(crate) fn attribute_name_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, InterpolatedQualifiedName<'b>> {
    let start = scanner.state();
    if scanner.scan_char('*') {
        let ns_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        let namespace = Interpolation::plain("*".to_string(), ns_span);
        scanner.expect_char('|')?;
        let name = interpolated_identifier_impl(scanner, state)?;
        let full_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(InterpolatedQualifiedName::new(
            name,
            full_span,
            Some(namespace),
        ));
    }

    if scanner.scan_char('|') {
        let ns_span = span_from_to_impl(scanner, &state.parser_state, start, Some(&start))?;
        let namespace = Interpolation::plain(String::new(), ns_span);
        let name = interpolated_identifier_impl(scanner, state)?;
        let full_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(InterpolatedQualifiedName::new(
            name,
            full_span,
            Some(namespace),
        ));
    }

    let name_or_namespace = interpolated_identifier_impl(scanner, state)?;
    if scanner.peek_char(0) != '|' as i32 || scanner.peek_char(1) == '=' as i32 {
        let full_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(InterpolatedQualifiedName::new(
            name_or_namespace,
            full_span,
            None,
        ));
    }

    scanner.read_char()?; // consume '|'
    let name = interpolated_identifier_impl(scanner, state)?;
    let full_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(InterpolatedQualifiedName::new(
        name,
        full_span,
        Some(name_or_namespace),
    ))
}

// attribute_operator_impl — consumes an attribute selector's operator
// (`=`, `~=`, `|=`, `^=`, `$=`, `*=`), including its span.

pub(crate) fn attribute_operator_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, CssValue<'b, AttributeOperator>> {
    let start = scanner.state();
    let ch = scanner.read_char()?;
    let op = match ch {
        '=' => AttributeOperator::Equal,
        '~' => {
            scanner.expect_char('=')?;
            AttributeOperator::Include
        }
        '|' => {
            scanner.expect_char('=')?;
            AttributeOperator::Dash
        }
        '^' => {
            scanner.expect_char('=')?;
            AttributeOperator::Prefix
        }
        '$' => {
            scanner.expect_char('=')?;
            AttributeOperator::Suffix
        }
        '*' => {
            scanner.expect_char('=')?;
            AttributeOperator::Substring
        }
        _ => {
            // Matches Dart: scanner.error('Expected "]".', position: start)
            // — zero-length span at the offending char.
            return Err(scanner
                .error("Expected \"]\".", Some(start.position), 0)
                .into());
        }
    };
    let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(CssValue::new(op, file_span))
}

// class_selector_impl — consumes a class selector (`.` + name).

pub(crate) fn class_selector_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, InterpolatedClassSelector<'b>> {
    scanner.expect_char('.')?;
    let name = interpolated_identifier_impl(scanner, state)?;
    Ok(InterpolatedClassSelector::new(name))
}

// id_selector_impl — consumes an ID selector (`#` + name).

pub(crate) fn id_selector_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, InterpolatedIDSelector<'b>> {
    scanner.expect_char('#')?;
    let name = interpolated_identifier_impl(scanner, state)?;
    Ok(InterpolatedIDSelector::new(name))
}

// placeholder_selector_impl — consumes a placeholder selector (`%` + name).

pub(crate) fn placeholder_selector_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, InterpolatedPlaceholderSelector<'b>> {
    scanner.expect_char('%')?;
    let name = interpolated_identifier_impl(scanner, state)?;
    Ok(InterpolatedPlaceholderSelector::new(name))
}

// parent_selector_impl — consumes `&` plus an optional interpolated
// identifier-body suffix (`&-suffix`). Suffixes are rejected in plain CSS.

pub(crate) fn parent_selector_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, InterpolatedParentSelector<'b>> {
    let start = scanner.state();
    scanner.expect_char('&')?;
    let suffix = if looking_at_interpolated_identifier_body_impl(scanner) {
        Some(interpolated_identifier_body_impl(scanner, state)?)
    } else {
        None
    };
    let is_plain_css = matches!(state.parser_state.syntax, Syntax::Css(_));
    if is_plain_css && suffix.is_some() {
        return Err(scanner
            .error(
                "Parent selectors can't have suffixes in plain CSS.",
                Some(start.position),
                (scanner.pos() - start.position) as isize,
            )
            .into());
    }
    let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(InterpolatedParentSelector::new(file_span, suffix))
}

// pseudo_selector_impl — consumes a pseudo-class or pseudo-element.
//
// A parenthesized argument parses as a nested selector list for the known
// selector-taking pseudos (`:not`/`:is`/… and `::slotted`), as an
// `of`-terminated `An+B` + selector pair for `:nth-child`/
// `:nth-last-child`, and as an interpolated declaration value otherwise
// (vendor prefixes are stripped before the lookup).

pub(crate) fn pseudo_selector_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, InterpolatedPseudoSelector<'b>> {
    let start = scanner.state();
    scanner.expect_char(':')?;
    let element = scanner.scan_char(':');
    let name = interpolated_identifier_impl(scanner, state)?;

    if !scanner.scan_char('(') {
        let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(InterpolatedPseudoSelector::new(
            name, file_span, element, None, None,
        ));
    }
    whitespace_impl(scanner, &mut state.parser_state, true)?;

    let unvendored = if let Some(plain) = name.as_plain() {
        unvendor(plain)
    } else {
        String::new()
    };

    let mut argument: Option<Interpolation<'b>> = None;
    let mut selector_list: Option<InterpolatedSelectorList<'b>> = None;

    if element {
        if SELECTOR_PSEUDO_ELEMENTS.contains(&unvendored.as_str()) {
            selector_list = Some(selector_list_impl(scanner, state)?);
        } else {
            argument = Some(interpolated_declaration_value_impl(
                scanner,
                state,
                DeclarationValueOpts {
                    allow_empty: true,
                    ..Default::default()
                },
            )?);
        }
    } else if SELECTOR_PSEUDO_CLASSES.contains(&unvendored.as_str()) {
        selector_list = Some(selector_list_impl(scanner, state)?);
    } else if unvendored == "nth-child" || unvendored == "nth-last-child" {
        argument = Some(interpolated_declaration_value_impl(
            scanner,
            state,
            DeclarationValueOpts {
                end_after_of: true,
                consume_newlines: true,
                ..Default::default()
            },
        )?);
        if scanner.peek_char(0) != ')' as i32 {
            selector_list = Some(selector_list_impl(scanner, state)?);
        }
    } else {
        argument = Some(interpolated_declaration_value_impl(
            scanner,
            state,
            DeclarationValueOpts {
                allow_empty: true,
                ..Default::default()
            },
        )?);
    }
    scanner.expect_char(')')?;

    let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
    Ok(InterpolatedPseudoSelector::new(
        name,
        file_span,
        element,
        argument,
        selector_list,
    ))
}

// type_or_universal_selector_impl — consumes a type or universal selector.
//
// The two are combined because either one can start with `*`
// (`*`, `*|*`, `*|name`, `|*`, `|name`, `ns|*`, `ns|name`).

pub(crate) fn type_or_universal_selector_impl<'b>(
    scanner: &mut SpanScanner<'b>,
    state: &mut StylesheetState<'b>,
) -> ParseResult<'b, InterpolatedSimpleSelector<'b>> {
    let start = scanner.state();
    if scanner.scan_char('*') {
        let after_asterisk = scanner.state();
        if !scanner.scan_char('|') {
            let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
            return Ok(InterpolatedSimpleSelector::Universal(
                InterpolatedUniversalSelector::new(file_span, None),
            ));
        }
        let ns_span =
            span_from_to_impl(scanner, &state.parser_state, start, Some(&after_asterisk))?;
        let namespace = Interpolation::plain("*".to_string(), ns_span);
        if scanner.scan_char('*') {
            let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
            return Ok(InterpolatedSimpleSelector::Universal(
                InterpolatedUniversalSelector::new(file_span, Some(namespace)),
            ));
        }
        let name = interpolated_identifier_impl(scanner, state)?;
        let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(InterpolatedSimpleSelector::Type(
            InterpolatedTypeSelector::new(InterpolatedQualifiedName::new(
                name,
                file_span,
                Some(namespace),
            )),
        ));
    } else if scanner.scan_char('|') {
        let ns_span = span_from_to_impl(scanner, &state.parser_state, start, Some(&start))?;
        let namespace = Interpolation::plain(String::new(), ns_span);
        if scanner.scan_char('*') {
            let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
            return Ok(InterpolatedSimpleSelector::Universal(
                InterpolatedUniversalSelector::new(file_span, Some(namespace)),
            ));
        }
        let name = interpolated_identifier_impl(scanner, state)?;
        let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        return Ok(InterpolatedSimpleSelector::Type(
            InterpolatedTypeSelector::new(InterpolatedQualifiedName::new(
                name,
                file_span,
                Some(namespace),
            )),
        ));
    }

    let name_or_namespace = interpolated_identifier_impl(scanner, state)?;
    if !scanner.scan_char('|') {
        let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        Ok(InterpolatedSimpleSelector::Type(
            InterpolatedTypeSelector::new(InterpolatedQualifiedName::new(
                name_or_namespace,
                file_span,
                None,
            )),
        ))
    } else if scanner.scan_char('*') {
        let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        Ok(InterpolatedSimpleSelector::Universal(
            InterpolatedUniversalSelector::new(file_span, Some(name_or_namespace)),
        ))
    } else {
        let name = interpolated_identifier_impl(scanner, state)?;
        let file_span = span_from_impl(scanner, &state.parser_state, start)?.file_span()?;
        Ok(InterpolatedSimpleSelector::Type(
            InterpolatedTypeSelector::new(InterpolatedQualifiedName::new(
                name,
                file_span,
                Some(name_or_namespace),
            )),
        ))
    }
}

// ======================================================================
// Thin wrappers on StylesheetParser
// ======================================================================

impl<'parse> StylesheetParser<'parse> {
    pub fn selector_list(&mut self) -> ParseResult<'parse, InterpolatedSelectorList<'parse>> {
        selector_list_impl(&mut self.scanner, &mut self.state)
    }

    pub fn complex_selector(
        &mut self,
        line_break: bool,
        allow_leading_combinator: bool,
        allow_trailing_combinator: bool,
    ) -> ParseResult<'parse, InterpolatedComplexSelector<'parse>> {
        complex_selector_impl(
            &mut self.scanner,
            &mut self.state,
            line_break,
            allow_leading_combinator,
            allow_trailing_combinator,
        )
    }

    pub fn compound_selector(
        &mut self,
    ) -> ParseResult<'parse, InterpolatedCompoundSelector<'parse>> {
        compound_selector_impl(&mut self.scanner, &mut self.state)
    }

    pub fn is_simple_selector_start(&self, ch: i32) -> bool {
        let is_plain_css = matches!(self.state.parser_state.syntax, Syntax::Css(_));
        is_simple_selector_start(ch, is_plain_css)
    }

    pub fn simple_selector(
        &mut self,
        allow_parent: bool,
    ) -> ParseResult<'parse, InterpolatedSimpleSelector<'parse>> {
        simple_selector_impl(&mut self.scanner, &mut self.state, allow_parent)
    }

    pub fn attribute_selector(
        &mut self,
    ) -> ParseResult<'parse, InterpolatedAttributeSelector<'parse>> {
        attribute_selector_impl(&mut self.scanner, &mut self.state)
    }

    pub fn attribute_name(&mut self) -> ParseResult<'parse, InterpolatedQualifiedName<'parse>> {
        attribute_name_impl(&mut self.scanner, &mut self.state)
    }

    pub fn attribute_operator(
        &mut self,
    ) -> ParseResult<'parse, CssValue<'parse, AttributeOperator>> {
        attribute_operator_impl(&mut self.scanner, &mut self.state)
    }

    pub fn class_selector(&mut self) -> ParseResult<'parse, InterpolatedClassSelector<'parse>> {
        class_selector_impl(&mut self.scanner, &mut self.state)
    }

    pub fn id_selector(&mut self) -> ParseResult<'parse, InterpolatedIDSelector<'parse>> {
        id_selector_impl(&mut self.scanner, &mut self.state)
    }

    pub fn placeholder_selector(
        &mut self,
    ) -> ParseResult<'parse, InterpolatedPlaceholderSelector<'parse>> {
        placeholder_selector_impl(&mut self.scanner, &mut self.state)
    }

    pub fn parent_selector(&mut self) -> ParseResult<'parse, InterpolatedParentSelector<'parse>> {
        parent_selector_impl(&mut self.scanner, &mut self.state)
    }

    pub fn pseudo_selector(&mut self) -> ParseResult<'parse, InterpolatedPseudoSelector<'parse>> {
        pseudo_selector_impl(&mut self.scanner, &mut self.state)
    }

    pub fn type_or_universal_selector(
        &mut self,
    ) -> ParseResult<'parse, InterpolatedSimpleSelector<'parse>> {
        type_or_universal_selector_impl(&mut self.scanner, &mut self.state)
    }
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::source_span_file_source::FileSource;
    use crate::parse::parser::ParserState;
    use crate::parse::stylesheet::CssState;
    use bumpalo::Bump;
    use std::collections::HashMap;
    use std::collections::HashSet;

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
            warnings: Vec::new(),
            last_silent_comment: None,
        };
        (scanner, state)
    }

    fn make_css_state<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
    ) -> (SpanScanner<'parse>, StylesheetState<'parse>)
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let (scanner, mut state) = make_state(arena, text);
        state.parser_state.syntax = Syntax::Css(CssState {
            disallowed_function_names: HashSet::new(),
        });
        (scanner, state)
    }

    // =========================================================================
    // selector_list tests
    // =========================================================================

    #[test]
    fn test_selector_list_single() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a");
        let list = selector_list_impl(&mut s, &mut st).unwrap();
        assert_eq!(list.components.len(), 1);
        let text = list.to_display_string().unwrap();
        assert_eq!(text, "a");
    }

    #[test]
    fn test_selector_list_multiple() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a, b, c");
        let list = selector_list_impl(&mut s, &mut st).unwrap();
        assert_eq!(list.components.len(), 3);
        let text = list.to_display_string().unwrap();
        assert_eq!(text, "a, b, c");
    }

    #[test]
    fn test_selector_list_empty_comma() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a,,b");
        let list = selector_list_impl(&mut s, &mut st).unwrap();
        assert_eq!(list.components.len(), 2);
    }

    #[test]
    fn test_selector_list_trailing_comma() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a,");
        let list = selector_list_impl(&mut s, &mut st).unwrap();
        assert_eq!(list.components.len(), 1);
    }

    // =========================================================================
    // complex_selector tests
    // =========================================================================

    #[test]
    fn test_complex_selector_single_compound() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "div");
        let complex = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap();
        let text = complex.to_display_string().unwrap();
        assert_eq!(text, "div");
    }

    #[test]
    fn test_complex_selector_child() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a > b");
        let complex = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap();
        let text = complex.to_display_string().unwrap();
        assert_eq!(text, "a > b");
    }

    #[test]
    fn test_complex_selector_next_sibling() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a + b");
        let complex = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap();
        let text = complex.to_display_string().unwrap();
        assert_eq!(text, "a + b");
    }

    #[test]
    fn test_complex_selector_following_sibling() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a ~ b");
        let complex = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap();
        let text = complex.to_display_string().unwrap();
        assert_eq!(text, "a ~ b");
    }

    #[test]
    fn test_complex_selector_descendant() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a b");
        let complex = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap();
        let text = complex.to_display_string().unwrap();
        assert_eq!(text, "a b");
    }

    #[test]
    fn test_complex_selector_leading_combinator() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "> a");
        let complex = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap();
        assert!(complex.leading_combinator.is_some());
    }

    #[test]
    fn test_complex_selector_trailing_combinator_error() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a >");
        let err = complex_selector_impl(&mut s, &mut st, false, true, false).unwrap_err();
        match &*err {
            ParseError::Scan(e) => assert_eq!(e.message, "expected selector."),
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    #[test]
    fn test_complex_selector_empty_error() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "");
        let err = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap_err();
        match &*err {
            ParseError::Scan(e) => assert_eq!(e.message, "expected selector."),
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    #[test]
    fn test_complex_selector_multiple_combinators() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a > b + c ~ d");
        let complex = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap();
        let text = complex.to_display_string().unwrap();
        assert_eq!(text, "a > b + c ~ d");
    }

    #[test]
    fn test_complex_selector_ampersand_mid_compound_error() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a&b");
        let err = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap_err();
        match &*err {
            ParseError::Scan(e) => assert_eq!(
                e.message,
                "\"&\" may only used at the beginning of a compound selector."
            ),
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    // =========================================================================
    // compound_selector tests
    // =========================================================================

    #[test]
    fn test_compound_selector_single() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "div");
        let comp = compound_selector_impl(&mut s, &mut st).unwrap();
        assert!(!comp.components.is_empty());
    }

    #[test]
    fn test_compound_selector_multiple_simples() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "div.foo#bar");
        let comp = compound_selector_impl(&mut s, &mut st).unwrap();
        let text = comp.to_display_string().unwrap();
        assert_eq!(text, "div.foo#bar");
    }

    // =========================================================================
    // is_simple_selector_start tests
    // =========================================================================

    #[test]
    fn test_is_simple_selector_start() {
        let cases = vec![
            ('*' as i32, true),
            ('[' as i32, true),
            ('.' as i32, true),
            ('#' as i32, true),
            ('%' as i32, true),
            (':' as i32, true),
            ('a' as i32, false),
            ('-' as i32, false),
        ];
        for (ch, want) in cases {
            assert_eq!(
                is_simple_selector_start(ch, false),
                want,
                "is_simple_selector_start('{}') should be {want}",
                ch as u8 as char
            );
        }
    }

    #[test]
    fn test_is_simple_selector_start_ampersand_plain_css() {
        assert!(is_simple_selector_start('&' as i32, true));
    }

    #[test]
    fn test_is_simple_selector_start_ampersand_not_plain_css() {
        assert!(!is_simple_selector_start('&' as i32, false));
    }

    // =========================================================================
    // simple_selector tests
    // =========================================================================

    #[test]
    fn test_simple_selector_class() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ".foo");
        let sel = simple_selector_impl(&mut s, &mut st, true).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, ".foo");
    }

    #[test]
    fn test_simple_selector_id() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "#bar");
        let sel = simple_selector_impl(&mut s, &mut st, true).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "#bar");
    }

    #[test]
    fn test_simple_selector_type() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "div");
        let sel = simple_selector_impl(&mut s, &mut st, true).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "div");
    }

    #[test]
    fn test_simple_selector_universal() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "*");
        let sel = simple_selector_impl(&mut s, &mut st, true).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "*");
    }

    #[test]
    fn test_simple_selector_parent_allowed() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "&");
        let sel = simple_selector_impl(&mut s, &mut st, true).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "&");
    }

    #[test]
    fn test_simple_selector_parent_not_allowed_error() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "&");
        let err = simple_selector_impl(&mut s, &mut st, false).unwrap_err();
        match &*err {
            ParseError::Format(f) => {
                assert_eq!(f.message, "Parent selectors aren't allowed here.");
            }
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    #[test]
    fn test_simple_selector_placeholder() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "%x");
        let sel = simple_selector_impl(&mut s, &mut st, true).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "%x");
    }

    #[test]
    fn test_simple_selector_pseudo_class() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ":hover");
        let sel = simple_selector_impl(&mut s, &mut st, true).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, ":hover");
    }

    #[test]
    fn test_simple_selector_pseudo_element() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "::before");
        let sel = simple_selector_impl(&mut s, &mut st, true).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "::before");
    }

    // =========================================================================
    // attribute_selector tests
    // =========================================================================

    #[test]
    fn test_attribute_selector_bare() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "[href]");
    }

    #[test]
    fn test_attribute_selector_with_value() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href=val]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "[href=val]");
    }

    #[test]
    fn test_attribute_selector_with_operator() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[data-value~=x]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "[data-value~=x]");
    }

    #[test]
    fn test_expected_bracket_zero_length() {
        // Matches Dart: scanner.error('Expected "]".', position: start) —
        // zero-length span at the offending char (verified end-to-end via
        // CLI differential on `a[b?="c"]`).
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[b?=\"c\"]");
        let err = attribute_selector_impl(&mut s, &mut st).unwrap_err();
        assert!(err.to_string().contains("Expected \"]\"."), "{err}");
    }

    #[test]
    fn test_attribute_selector_with_quoted_value() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href=\"val\"]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "[href=\"val\"]");
    }

    #[test]
    fn test_attribute_selector_with_modifier() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href=val i]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "[href=val i]");
    }

    #[test]
    fn test_attribute_selector_all_operators() {
        let tests = [
            ("[a=b]", "[a=b]"),
            ("[a~=b]", "[a~=b]"),
            ("[a|=b]", "[a|=b]"),
            ("[a^=b]", "[a^=b]"),
            ("[a$=b]", "[a$=b]"),
            ("[a*=b]", "[a*=b]"),
        ];
        for (input, want) in tests {
            let arena = Bump::new();
            let (mut s, mut st) = make_state(&arena, input);
            let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
            let text = sel.to_display_string().unwrap();
            assert_eq!(text, want, "input: {input}");
        }
    }

    #[test]
    fn test_attribute_selector_wildcard_namespace() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[*|href]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "[*|href]");
    }

    #[test]
    fn test_attribute_selector_explicit_namespace() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[ns|href]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "[ns|href]");
    }

    #[test]
    fn test_attribute_selector_no_namespace() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[|href]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "[|href]");
    }

    // =========================================================================
    // class_selector tests
    // =========================================================================

    #[test]
    fn test_class_selector() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ".foo");
        let sel = class_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, ".foo");
    }

    // =========================================================================
    // id_selector tests
    // =========================================================================

    #[test]
    fn test_id_selector() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "#bar");
        let sel = id_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "#bar");
    }

    // =========================================================================
    // placeholder_selector tests
    // =========================================================================

    #[test]
    fn test_placeholder_selector() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "%foo");
        let sel = placeholder_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "%foo");
    }

    // =========================================================================
    // parent_selector tests
    // =========================================================================

    #[test]
    fn test_parent_selector() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "&");
        let sel = parent_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "&");
    }

    #[test]
    fn test_parent_selector_suffix() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "&-suffix");
        let sel = parent_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "&-suffix");
    }

    // =========================================================================
    // pseudo_selector tests
    // =========================================================================

    #[test]
    fn test_pseudo_selector_class() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ":hover");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, ":hover");
    }

    #[test]
    fn test_pseudo_selector_element() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "::before");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "::before");
    }

    #[test]
    fn test_pseudo_selector_with_selector_arg() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ":not(.foo)");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, ":not(.foo)");
    }

    #[test]
    fn test_pseudo_selector_with_declaration_arg() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ":lang(en)");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, ":lang(en)");
    }

    #[test]
    fn test_pseudo_selector_nth_child() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ":nth-child(2n+1)");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, ":nth-child(2n+1)");
    }

    #[test]
    fn test_pseudo_selector_nth_child_with_selector() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ":nth-child(1 of .foo)");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, ":nth-child(1 of .foo)");
    }

    // =========================================================================
    // type_or_universal_selector tests
    // =========================================================================

    #[test]
    fn test_type_or_universal_wildcard() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "*");
        let sel = type_or_universal_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "*");
    }

    #[test]
    fn test_type_or_universal_wildcard_namespace() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "*|*");
        let sel = type_or_universal_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "*|*");
    }

    #[test]
    fn test_type_or_universal_wildcard_namespace_type() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "*|div");
        let sel = type_or_universal_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "*|div");
    }

    #[test]
    fn test_type_or_universal_empty_namespace_wildcard() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "|*");
        let sel = type_or_universal_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "|*");
    }

    #[test]
    fn test_type_or_universal_empty_namespace_type() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "|div");
        let sel = type_or_universal_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "|div");
    }

    #[test]
    fn test_type_or_universal_namespaced_type() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "ns|div");
        let sel = type_or_universal_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "ns|div");
    }

    #[test]
    fn test_type_or_universal_namespaced_wildcard() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "ns|*");
        let sel = type_or_universal_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "ns|*");
    }

    #[test]
    fn test_type_or_universal_bare_type() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "div");
        let sel = type_or_universal_selector_impl(&mut s, &mut st).unwrap();
        let text = sel.to_display_string().unwrap();
        assert_eq!(text, "div");
    }

    // =========================================================================
    // Additional tests: Error paths
    // =========================================================================

    #[test]
    fn test_simple_selector_placeholder_plain_css_error() {
        let arena = Bump::new();
        let (mut s, mut st) = make_css_state(&arena, "%x");
        let err = simple_selector_impl(&mut s, &mut st, true).unwrap_err();
        match &*err {
            ParseError::Format(f) => {
                assert_eq!(
                    f.message,
                    "Placeholder selectors aren't allowed in plain CSS."
                );
            }
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    #[test]
    fn test_simple_selector_placeholder_plain_css_error_with_prefix() {
        let arena = Bump::new();
        let (mut s, mut st) = make_css_state(&arena, "a %x");
        let err = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap_err();
        match &*err {
            ParseError::Format(f) => {
                assert_eq!(
                    f.message,
                    "Placeholder selectors aren't allowed in plain CSS."
                );
            }
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    #[test]
    fn test_parent_selector_suffix_plain_css_error() {
        let arena = Bump::new();
        let (mut s, mut st) = make_css_state(&arena, "&-suffix");
        let err = parent_selector_impl(&mut s, &mut st).unwrap_err();
        match &*err {
            ParseError::Scan(e) => {
                assert_eq!(
                    e.message,
                    "Parent selectors can't have suffixes in plain CSS."
                );
            }
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    #[test]
    fn test_attribute_selector_invalid_operator_error() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href!val]");
        let err = attribute_selector_impl(&mut s, &mut st).unwrap_err();
        match &*err {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "Expected \"]\".");
            }
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    #[test]
    fn test_complex_selector_leading_combinator_denied_error() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "> a");
        let err = complex_selector_impl(&mut s, &mut st, false, false, true).unwrap_err();
        match &*err {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "expected selector.");
            }
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    #[test]
    fn test_complex_selector_trailing_combinator_plain_css_error() {
        let arena = Bump::new();
        let (mut s, mut st) = make_css_state(&arena, "a >");
        let err = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap_err();
        match &*err {
            ParseError::Scan(e) => {
                assert_eq!(e.message, "expected selector.");
            }
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    // =========================================================================
    // Additional tests: complex_selector structural assertions
    // =========================================================================

    #[test]
    fn test_complex_selector_leading_combinator_child() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "> a");
        let complex = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap();
        let lc = complex.leading_combinator.as_ref().unwrap();
        assert_eq!(lc.value, Combinator::Child);
    }

    #[test]
    fn test_complex_selector_leading_combinator_next_sibling() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "+ a");
        let complex = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap();
        let lc = complex.leading_combinator.as_ref().unwrap();
        assert_eq!(lc.value, Combinator::NextSibling);
    }

    #[test]
    fn test_complex_selector_leading_combinator_following_sibling() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "~ a");
        let complex = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap();
        let lc = complex.leading_combinator.as_ref().unwrap();
        assert_eq!(lc.value, Combinator::FollowingSibling);
    }

    #[test]
    fn test_complex_selector_combinator_types_in_order() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a > b + c ~ d");
        let complex = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap();
        assert_eq!(complex.components.len(), 4);
        let expected = [
            Combinator::Child,
            Combinator::NextSibling,
            Combinator::FollowingSibling,
        ];
        for (i, exp) in expected.iter().enumerate() {
            let comp = &complex.components[i];
            assert!(
                comp.combinator.is_some(),
                "component {i}: expected combinator"
            );
            assert_eq!(
                comp.combinator.as_ref().unwrap().value,
                *exp,
                "component {i}"
            );
        }
    }

    #[test]
    fn test_complex_selector_descendant_chain() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a b c");
        let complex = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap();
        assert_eq!(complex.components.len(), 3);
        for i in 0..complex.components.len() - 1 {
            assert!(
                complex.components[i].combinator.is_none(),
                "component {i}: expected nil combinator"
            );
        }
    }

    #[test]
    fn test_complex_selector_child_combinator_component() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a > b");
        let complex = complex_selector_impl(&mut s, &mut st, false, true, true).unwrap();
        assert_eq!(complex.components.len(), 2);
        assert!(complex.components[0].combinator.is_some());
        assert_eq!(
            complex.components[0].combinator.as_ref().unwrap().value,
            Combinator::Child
        );
    }

    // =========================================================================
    // Additional tests: simple_selector dispatch
    // =========================================================================

    #[test]
    fn test_simple_selector_hash_with_brace_goes_to_type_or_universal() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "#{$x}");
        let sel = simple_selector_impl(&mut s, &mut st, true).unwrap();
        if let InterpolatedSimpleSelector::ID(_) = &sel {
            panic!("expected not ID selector")
        }
    }

    #[test]
    fn test_simple_selector_placeholder_not_plain_css() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "%x");
        let sel = simple_selector_impl(&mut s, &mut st, true).unwrap();
        match &sel {
            InterpolatedSimpleSelector::Placeholder(_) => {}
            _ => panic!("expected Placeholder variant"),
        }
    }

    #[test]
    fn test_simple_selector_attribute_dispatch() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href]");
        let sel = simple_selector_impl(&mut s, &mut st, true).unwrap();
        match &sel {
            InterpolatedSimpleSelector::Attribute(_) => {}
            _ => panic!("expected Attribute variant"),
        }
    }

    #[test]
    fn test_simple_selector_parent() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "&");
        let sel = simple_selector_impl(&mut s, &mut st, true).unwrap();
        match &sel {
            InterpolatedSimpleSelector::Parent(_) => {}
            _ => panic!("expected Parent variant"),
        }
    }

    #[test]
    fn test_simple_selector_parent_plain_css_disallowed() {
        let arena = Bump::new();
        let (mut s, mut st) = make_css_state(&arena, "&");
        let err = simple_selector_impl(&mut s, &mut st, false).unwrap_err();
        match &*err {
            ParseError::Format(f) => {
                assert_eq!(f.message, "Parent selectors aren't allowed here.");
            }
            _ => panic!("unexpected error variant: {err:?}"),
        };
    }

    // =========================================================================
    // Additional tests: attribute_selector structural assertions
    // =========================================================================

    #[test]
    fn test_attribute_selector_modifier_present() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href=val i]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        let modifier = sel.modifier.as_ref().unwrap();
        assert_eq!(modifier.to_display_string().unwrap(), "i");
    }

    #[test]
    fn test_attribute_selector_bare_no_op_no_value() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        assert!(sel.op.is_none());
        assert!(sel.value.is_none());
    }

    #[test]
    fn test_attribute_selector_quoted_value() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href=\"val\"]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        let value = sel.value.as_ref().unwrap();
        assert_eq!(value.to_display_string().unwrap(), "\"val\"");
    }

    #[test]
    fn test_attribute_selector_operator_include() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href~=val]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(sel.op.as_ref().unwrap().value, AttributeOperator::Include);
    }

    #[test]
    fn test_attribute_selector_operator_dash() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href|=val]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(sel.op.as_ref().unwrap().value, AttributeOperator::Dash);
    }

    #[test]
    fn test_attribute_selector_operator_prefix() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href^=val]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(sel.op.as_ref().unwrap().value, AttributeOperator::Prefix);
    }

    #[test]
    fn test_attribute_selector_operator_suffix() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href$=val]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(sel.op.as_ref().unwrap().value, AttributeOperator::Suffix);
    }

    #[test]
    fn test_attribute_selector_operator_substring() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href*=val]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(sel.op.as_ref().unwrap().value, AttributeOperator::Substring);
    }

    // =========================================================================
    // Additional tests: attribute_name logic
    // =========================================================================

    #[test]
    fn test_attribute_name_wildcard_ns() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[*|href]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        let ns = sel.name.namespace.as_ref().unwrap();
        assert_eq!(ns.to_display_string().unwrap(), "*");
        assert_eq!(sel.name.name.to_display_string().unwrap(), "href");
    }

    #[test]
    fn test_attribute_name_empty_ns() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[|href]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        let ns = sel.name.namespace.as_ref().unwrap();
        assert_eq!(ns.to_display_string().unwrap(), "");
    }

    #[test]
    fn test_attribute_name_explicit_ns() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[ns|href]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        let ns = sel.name.namespace.as_ref().unwrap();
        assert_eq!(ns.to_display_string().unwrap(), "ns");
        assert_eq!(sel.name.name.to_display_string().unwrap(), "href");
    }

    #[test]
    fn test_attribute_name_no_ns() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        assert!(sel.name.namespace.is_none());
    }

    #[test]
    fn test_attribute_name_pipe_equals_disambig() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "[href|=val]");
        let sel = attribute_selector_impl(&mut s, &mut st).unwrap();
        assert!(
            sel.name.namespace.is_none(),
            "|= is operator, not namespace"
        );
        assert_eq!(sel.name.name.to_display_string().unwrap(), "href");
    }

    // =========================================================================
    // Additional tests: pseudo_selector expanded
    // =========================================================================

    #[test]
    fn test_pseudo_selector_nth_last_child() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ":nth-last-child(2n)");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(sel.to_display_string().unwrap(), ":nth-last-child(2n)");
    }

    #[test]
    fn test_pseudo_selector_is_list() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ":is(.a, .b)");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(sel.to_display_string().unwrap(), ":is(.a, .b)");
    }

    #[test]
    fn test_pseudo_selector_where() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ":where(.x)");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(sel.to_display_string().unwrap(), ":where(.x)");
    }

    #[test]
    fn test_pseudo_selector_has_combinator() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ":has(div > a)");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(sel.to_display_string().unwrap(), ":has(div > a)");
    }

    #[test]
    fn test_pseudo_selector_slotted() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "::slotted(.x)");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(sel.to_display_string().unwrap(), "::slotted(.x)");
    }

    #[test]
    fn test_pseudo_selector_empty_arg() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ":lang()");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(sel.to_display_string().unwrap(), ":lang()");
    }

    #[test]
    fn test_pseudo_selector_vendor_prefixed() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ":-moz-any(.x)");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(sel.to_display_string().unwrap(), ":-moz-any(.x)");
    }

    #[test]
    fn test_pseudo_selector_structural_class_vs_element() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, ":hover");
        let sel = pseudo_selector_impl(&mut s, &mut st).unwrap();
        assert!(sel.is_syntactic_class);
        assert!(!sel.is_syntactic_element());

        let (mut s2, mut st2) = make_state(&arena, "::before");
        let sel2 = pseudo_selector_impl(&mut s2, &mut st2).unwrap();
        assert!(!sel2.is_syntactic_class);
        assert!(sel2.is_syntactic_element());
    }

    // =========================================================================
    // Additional tests: type_or_universal structural assertions
    // =========================================================================

    #[test]
    fn test_type_or_universal_wildcard_no_ns() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "*");
        let sel = type_or_universal_selector_impl(&mut s, &mut st).unwrap();
        match &sel {
            InterpolatedSimpleSelector::Universal(u) => {
                assert!(u.namespace.is_none());
            }
            _ => panic!("expected Universal variant"),
        }
    }

    #[test]
    fn test_type_or_universal_namespaced_type_as_type() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "ns|div");
        let sel = type_or_universal_selector_impl(&mut s, &mut st).unwrap();
        match &sel {
            InterpolatedSimpleSelector::Type(t) => {
                let ns = t.name.namespace.as_ref().unwrap();
                assert_eq!(ns.to_display_string().unwrap(), "ns");
            }
            _ => panic!("expected Type variant"),
        }
    }

    #[test]
    fn test_type_or_universal_wildcard_namespace_star() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "*|*");
        let sel = type_or_universal_selector_impl(&mut s, &mut st).unwrap();
        match &sel {
            InterpolatedSimpleSelector::Universal(u) => {
                let ns = u.namespace.as_ref().unwrap();
                assert_eq!(ns.to_display_string().unwrap(), "*");
            }
            _ => panic!("expected Universal variant"),
        }
    }

    #[test]
    fn test_type_or_universal_empty_ns_star() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "|*");
        let sel = type_or_universal_selector_impl(&mut s, &mut st).unwrap();
        match &sel {
            InterpolatedSimpleSelector::Universal(u) => {
                let ns = u.namespace.as_ref().unwrap();
                assert_eq!(ns.to_display_string().unwrap(), "");
            }
            _ => panic!("expected Universal variant"),
        }
    }

    #[test]
    fn test_type_or_universal_bare_type_as_type() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "div");
        let sel = type_or_universal_selector_impl(&mut s, &mut st).unwrap();
        match &sel {
            InterpolatedSimpleSelector::Type(t) => {
                assert!(t.name.namespace.is_none());
                assert_eq!(t.name.name.to_display_string().unwrap(), "div");
            }
            _ => panic!("expected Type variant"),
        }
    }

    // =========================================================================
    // Additional tests: selector_list
    // =========================================================================

    #[test]
    fn test_selector_list_line_break() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a,\nb");
        let list = selector_list_impl(&mut s, &mut st).unwrap();
        assert_eq!(list.components.len(), 2);
    }

    #[test]
    fn test_selector_list_complex_with_combinators() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "a > b, c + d");
        let list = selector_list_impl(&mut s, &mut st).unwrap();
        assert_eq!(list.components.len(), 2);
        assert_eq!(list.to_display_string().unwrap(), "a > b, c + d");
    }

    // =========================================================================
    // Additional tests: compound_selector
    // =========================================================================

    #[test]
    fn test_compound_selector_all_types() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "div.foo#bar:hover");
        let comp = compound_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(comp.to_display_string().unwrap(), "div.foo#bar:hover");
    }

    #[test]
    fn test_compound_selector_namespaced_type() {
        let arena = Bump::new();
        let (mut s, mut st) = make_state(&arena, "*|div.foo");
        let comp = compound_selector_impl(&mut s, &mut st).unwrap();
        assert_eq!(comp.to_display_string().unwrap(), "*|div.foo");
    }
}
