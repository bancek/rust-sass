// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/selector.dart
// go-source: go/value/parse_selector.go

//! Resolution-level selector parser: plain selectors with no interpolation.
//!
//! Used to re-parse selector text produced by interpolation and by built-in
//! functions. What follows is largely duplicated between here and
//! `selector.rs`. Most changes here should be mirrored there and vice versa.

use crate::ast::sass::interpolation_map::InterpolationMap;
use crate::common::span_scanner::SpanScanner;
use crate::logger::WarnLogger;
use bumpalo::Bump;
use std::rc::Rc;

use crate::common::ast_css_value::CssValue;
use crate::common::exception::SassResult;
use crate::common::source_span_file_source::FileSource;
use crate::common::span::Span;
use crate::deprecation::ADJACENT_COMPOUNDS;
use crate::logger::Logger;
use crate::selector::attribute::{AttributeOperator, AttributeSelector};
use crate::selector::class::ClassSelector;
use crate::selector::combinator::Combinator;
use crate::selector::complex::ComplexSelector;
use crate::selector::complex_component::ComplexSelectorComponent;
use crate::selector::compound::CompoundSelector;
use crate::selector::id::IdSelector;
use crate::selector::list::SelectorList;
use crate::selector::parent::ParentSelector;
use crate::selector::placeholder::PlaceholderSelector;
use crate::selector::pseudo::PseudoSelector;
use crate::selector::qualified_name::QualifiedName;
use crate::selector::ty::TypeSelector;
use crate::selector::universal::UniversalSelector;
use crate::selector::{Selector, SimpleSelector};
use crate::unvendor::unvendor;
use crate::util::character;

use crate::parse::parser::{
    declaration_value_impl, error_impl, expect_ident_char_impl, expect_identifier_impl,
    identifier_body_impl, identifier_impl, looking_at_identifier, looking_at_identifier_body,
    scan_ident_char_impl, span_from_impl, string_impl, whitespace_impl,
    wrap_span_format_exception_impl, ParseResult, Parser, ParserState,
};
use crate::parse::stylesheet::Syntax;

// Pseudo-class selectors that take unadorned selectors as arguments.
//
// Internal (Dart marks this `@internal`): kept as `pub(crate)` for the
// stylesheet-level selector parser, which shares the pseudo-argument rules.
// Mostly duplicated between `selector.rs` and here.
pub(crate) const SELECTOR_PSEUDO_CLASSES: &[&str] = &[
    "not",
    "is",
    "matches",
    "where",
    "current",
    "any",
    "has",
    "host",
    "host-context",
];

// Pseudo-element selectors that take unadorned selectors as arguments.
//
// Internal, shared with `selector.rs` like the class set above.
pub(crate) const SELECTOR_PSEUDO_ELEMENTS: &[&str] = &["slotted"];

// ======================================================================
// SelectorParser — a parser for plain (interpolation-free) selectors.
// ======================================================================

/// A parser for plain (interpolation-free) selectors.
///
/// What follows is largely duplicated between here and `selector.rs`.
/// Most changes here should be mirrored there and vice versa.
pub struct SelectorParser<'compile, 'parse, 'warn> {
    pub arena: &'compile Bump,
    pub parser: Parser<'parse>,
    /// Whether this parser allows the parent selector `&`.
    pub allow_parent: bool,
    /// Whether to parse the selector as plain CSS.
    pub plain_css: bool,
    /// The logger used to report deprecation warnings.
    pub logger: Option<Rc<dyn Logger>>,
    /// Buffered warning sink used during evaluation so the adjacent-compounds
    /// deprecation resolves to the call-site span on flush (threading the
    /// inner parse span would attribute it to the parsed string's file).
    pub warn_logger: Option<&'warn dyn WarnLogger<'parse>>,
}

impl<'compile: 'parse, 'parse, 'warn> SelectorParser<'compile, 'parse, 'warn> {
    pub fn new(arena: &'compile Bump, source: &'parse FileSource<'parse>) -> Self {
        SelectorParser {
            arena,
            parser: Parser::new(source, Syntax::Scss, None),
            allow_parent: true,
            plain_css: false,
            logger: None,
            warn_logger: None,
        }
    }

    /// Creates a parser that parses CSS selectors.
    ///
    /// If `allow_parent` is `false`, selectors containing the parent
    /// selector `&` fail with a `SassFormatException`-equivalent
    /// (`"Parent selectors aren't allowed here."`).
    ///
    /// If `plain_css` is `true`, the selector parses as plain CSS rather
    /// than Sass. `logger` reports deprecation warnings, defaulting to the
    /// default logger when absent.
    pub fn new_with_options(
        arena: &'compile Bump,
        source: &'parse FileSource<'parse>,
        allow_parent: bool,
        plain_css: bool,
        logger: Option<Rc<dyn Logger>>,
        warn_logger: Option<&'warn dyn WarnLogger<'parse>>,
        interpolation_map: Option<&'parse InterpolationMap<'parse>>,
    ) -> Self {
        SelectorParser {
            arena,
            parser: Parser::new(source, Syntax::Scss, interpolation_map),
            allow_parent,
            plain_css,
            logger,
            warn_logger,
        }
    }

    // ==================================================================
    // Public API
    // ==================================================================

    /// Consumes a selector list, requiring the whole input to be consumed.
    pub fn parse(&mut self) -> SassResult<SelectorList<'parse>> {
        let arena = self.arena;
        let logger_ref = self.logger.as_ref().map(|l| l.as_ref());
        let warn_ref = self.warn_logger;
        let allow_parent = self.allow_parent;
        let plain = self.plain_css;
        let result = wrap_span_format_exception_impl(
            &mut self.parser.scanner,
            &mut self.parser.state,
            |scanner, state| {
                let list = selector_list_impl(
                    arena,
                    scanner,
                    state,
                    allow_parent,
                    plain,
                    logger_ref,
                    warn_ref,
                )?;
                if !scanner.is_done() {
                    return Err(Box::new(
                        scanner.error("expected selector.", None, 0).into(),
                    ));
                }
                Ok(list)
            },
        );
        match result {
            Ok(v) => Ok(v),
            Err(e) => Err(e.into()),
        }
    }

    /// Consumes a complex selector, requiring the whole input to be consumed.
    pub fn parse_complex_selector(&mut self) -> SassResult<ComplexSelector<'parse>> {
        let arena = self.arena;
        let logger_ref = self.logger.as_ref().map(|l| l.as_ref());
        let warn_ref = self.warn_logger;
        let allow_parent = self.allow_parent;
        let plain = self.plain_css;
        let result = wrap_span_format_exception_impl(
            &mut self.parser.scanner,
            &mut self.parser.state,
            |scanner, state| {
                let cs = complex_selector_impl(
                    arena,
                    scanner,
                    state,
                    false,
                    allow_parent,
                    plain,
                    logger_ref,
                    warn_ref,
                )?;
                if !scanner.is_done() {
                    return Err(Box::new(
                        scanner.error("expected selector.", None, 0).into(),
                    ));
                }
                Ok(cs)
            },
        );
        match result {
            Ok(v) => Ok(v),
            Err(e) => Err(e.into()),
        }
    }

    /// Consumes a compound selector, requiring the whole input to be consumed.
    pub fn parse_compound_selector(&mut self) -> SassResult<CompoundSelector<'parse>> {
        let arena = self.arena;
        let result = wrap_span_format_exception_impl(
            &mut self.parser.scanner,
            &mut self.parser.state,
            |scanner, state| {
                let cs = compound_selector_impl(
                    arena,
                    scanner,
                    state,
                    self.allow_parent,
                    self.plain_css,
                )?;
                if !scanner.is_done() {
                    return Err(Box::new(
                        scanner.error("expected selector.", None, 0).into(),
                    ));
                }
                Ok(cs)
            },
        );
        match result {
            Ok(v) => Ok(v),
            Err(e) => Err(e.into()),
        }
    }

    /// Consumes a simple selector, requiring the whole input to be consumed
    /// (trailing input is an `"unexpected token."` error, unlike the
    /// `"expected selector."` used by the other entry points).
    pub fn parse_simple_selector(&mut self) -> SassResult<SimpleSelector<'parse>> {
        let arena = self.arena;
        let result = wrap_span_format_exception_impl(
            &mut self.parser.scanner,
            &mut self.parser.state,
            |scanner, state| {
                let sel = simple_selector_impl(
                    arena,
                    scanner,
                    state,
                    self.allow_parent,
                    self.allow_parent,
                    self.plain_css,
                )?;
                if !scanner.is_done() {
                    return Err(Box::new(scanner.error("unexpected token.", None, 0).into()));
                }
                Ok(sel)
            },
        );
        match result {
            Ok(v) => Ok(v),
            Err(e) => Err(e.into()),
        }
    }
}

// Free functions: the logic behind each parser method, taking the scanner
// and state as separate references. `consume_newlines` is always true here,
// so this level has no `StylesheetState` newline discipline to document.

// Consumes a selector list.
//
// Doubled commas are skipped and a trailing comma ends the list. Unlike the
// stylesheet-level list, components track whether a line break preceded them.
pub(crate) fn selector_list_impl<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
    allow_parent: bool,
    plain_css: bool,
    logger: Option<&dyn Logger>,
    warn_logger: Option<&dyn WarnLogger<'parse>>,
) -> ParseResult<'parse, SelectorList<'parse>> {
    let start = scanner.state();
    let mut previous_line = scanner.line();
    let cs = complex_selector_impl(
        arena,
        scanner,
        state,
        false,
        allow_parent,
        plain_css,
        logger,
        warn_logger,
    )?;
    let mut components = vec![cs];

    whitespace_impl(scanner, state, true)?;
    while scanner.scan_char(',') {
        whitespace_impl(scanner, state, true)?;
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
        let cs = complex_selector_impl(
            arena,
            scanner,
            state,
            line_break,
            allow_parent,
            plain_css,
            logger,
            warn_logger,
        )?;
        components.push(cs);
    }

    let span = span_from_impl(scanner, state, start)?;
    let file_span = span.file_span()?;
    Ok(SelectorList::new(arena, components, file_span)?)
}

// Consumes a complex selector.
//
// If `line_break` is `true`, that indicates that there was a line break
// before this selector. Adjacent compounds with no whitespace between them
// warn for the `adjacent-compounds` deprecation. A `&` anywhere but the
// start of a compound is an error.
// Threaded parser state; a params struct would just rename the list.
#[allow(clippy::too_many_arguments)]
fn complex_selector_impl<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
    line_break: bool,
    allow_parent: bool,
    plain_css: bool,
    logger: Option<&dyn Logger>,
    warn_logger: Option<&dyn WarnLogger<'parse>>,
) -> ParseResult<'parse, ComplexSelector<'parse>> {
    let start = scanner.state();
    let mut component_start = scanner.state();
    let mut last_compound: Option<CompoundSelector<'parse>> = None;
    let mut combinators: Vec<CssValue<'parse, Combinator>> = Vec::new();
    let mut initial_combinators: Vec<CssValue<'parse, Combinator>> = Vec::new();
    let mut components: Vec<ComplexSelectorComponent<'parse>> = Vec::new();

    loop {
        let before_whitespace = scanner.pos();
        whitespace_impl(scanner, state, true)?;
        let consumed_whitespace = scanner.pos() != before_whitespace;

        let ch = scanner.peek_char(0);

        match ch {
            _ if ch == '+' as i32 => {
                let comb_start = scanner.state();
                scanner.read_char()?;
                let comb_span = span_from_impl(scanner, state, comb_start)?;
                combinators.push(CssValue::new(
                    Combinator::NextSibling,
                    comb_span.file_span()?,
                ));
            }
            _ if ch == '>' as i32 => {
                let comb_start = scanner.state();
                scanner.read_char()?;
                let comb_span = span_from_impl(scanner, state, comb_start)?;
                combinators.push(CssValue::new(Combinator::Child, comb_span.file_span()?));
            }
            _ if ch == '~' as i32 => {
                let comb_start = scanner.state();
                scanner.read_char()?;
                let comb_span = span_from_impl(scanner, state, comb_start)?;
                combinators.push(CssValue::new(
                    Combinator::FollowingSibling,
                    comb_span.file_span()?,
                ));
            }
            _ if ch < 0 => break,

            _ => {
                let is_simple_start = ch == '[' as i32
                    || ch == '.' as i32
                    || ch == '#' as i32
                    || ch == '%' as i32
                    || ch == ':' as i32
                    || ch == '&' as i32
                    || ch == '*' as i32
                    || ch == '|' as i32;
                if !is_simple_start && !looking_at_identifier(scanner, None) {
                    break;
                }

                // Go: save adjacent compounds check result BEFORE combinators are taken
                let adjacent_check =
                    last_compound.is_some() && combinators.is_empty() && !consumed_whitespace;

                if let Some(ref lc) = last_compound {
                    let comp_span = span_from_impl(scanner, state, component_start)?.file_span()?;
                    let comp = ComplexSelectorComponent::new(
                        Box::new(lc.clone()),
                        std::mem::take(&mut combinators),
                        comp_span,
                    );
                    components.push(comp);
                } else if !combinators.is_empty() {
                    initial_combinators = std::mem::take(&mut combinators);
                    component_start = scanner.state();
                }

                let next_compound =
                    compound_selector_impl(arena, scanner, state, allow_parent, plain_css)?;

                // Dart line 191-203: adjacent compounds deprecation
                if adjacent_check {
                    let last_span = last_compound.as_ref().unwrap().span;
                    let next_span = next_compound.span;
                    let warn_span = Span::from(last_span.expand(&Span::from(next_span))?);
                    let msg = format!(
                        "Adjacent compound selectors must be separated by whitespace. \
                         This will be an error in Dart Sass 2.0.0. Suggestion:\n\n\
                         {} {}\n\n\
                         More info: https://sass-lang.com/d/adjacent-compounds",
                        last_span.text(),
                        next_span.text(),
                    );
                    if let Some(w) = warn_logger {
                        w.warn_deprecation(&msg, &ADJACENT_COMPOUNDS, None);
                    } else if let Some(l) = logger {
                        l.warn_deprecation(&msg, Some(&warn_span), &ADJACENT_COMPOUNDS, None)?;
                    }
                }

                last_compound = Some(next_compound);
                combinators.clear();

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
        }
    }

    if !combinators.is_empty() && plain_css {
        return Err(Box::new(
            scanner.error("expected selector.", None, 0).into(),
        ));
    } else if let Some(lc) = last_compound {
        let comp_span = span_from_impl(scanner, state, component_start)?.file_span()?;
        let comp = ComplexSelectorComponent::new(
            Box::new(lc),
            std::mem::take(&mut combinators),
            comp_span,
        );
        components.push(comp);
    } else if !combinators.is_empty() {
        initial_combinators = std::mem::take(&mut combinators);
    } else {
        return Err(Box::new(
            scanner.error("expected selector.", None, 0).into(),
        ));
    }

    let span = span_from_impl(scanner, state, start)?.file_span()?;
    Ok(ComplexSelector::new(
        initial_combinators,
        components,
        span,
        line_break,
    )?)
}

// Consumes a compound selector: one simple selector plus continuations.
//
// Non-initial `&` occurrences parse with the parser-level `plain_css` flag
// rather than the compound-level `allow_parent`, so `&` still resolves
// inside pseudo arguments such as `:is(&)` when the compound itself would
// forbid it. Nested selector pseudos keep the parser-level `allow_parent`.
fn compound_selector_impl<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
    allow_parent: bool,
    plain_css: bool,
) -> ParseResult<'parse, CompoundSelector<'parse>> {
    let start = scanner.state();
    let sel = simple_selector_impl(arena, scanner, state, allow_parent, allow_parent, plain_css)?;
    let mut comps = vec![sel];

    while is_simple_selector_start(scanner.peek_char(0), plain_css) {
        // Dart: _simpleSelector(allowParent: _plainCss) — the local override
        // only affects `&`; nested selector pseudos still use the parser-level
        // allowParent.
        let sel = simple_selector_impl(arena, scanner, state, plain_css, allow_parent, plain_css)?;
        comps.push(sel);
    }

    let span = span_from_impl(scanner, state, start)?.file_span()?;
    Ok(CompoundSelector::new(comps, span)?)
}

// Consumes a simple selector.
//
// If `allow_parent` is set, the parent selector `&` is allowed here.
// `parser_allow_parent` is the parser-level flag threaded into nested
// pseudo-selector arguments; `plain_css` rejects `%` placeholders and `&`
// suffixes. Defaults to the parser's `allow_parent` when not overridden.
fn simple_selector_impl<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
    allow_parent: bool,
    parser_allow_parent: bool,
    plain_css: bool,
) -> ParseResult<'parse, SimpleSelector<'parse>> {
    let start = scanner.state();

    match scanner.peek_char(0) {
        _ if scanner.peek_char(0) == '[' as i32 => attribute_selector_impl(scanner, state),
        _ if scanner.peek_char(0) == '.' as i32 => class_selector_impl(scanner, state),
        _ if scanner.peek_char(0) == '#' as i32 => id_selector_impl(scanner, state),
        _ if scanner.peek_char(0) == '%' as i32 => {
            let sel = placeholder_selector_impl(scanner, state)?;
            if plain_css {
                let span = span_from_impl(scanner, state, start)?.file_span()?;
                return Err(Box::new(error_impl(
                    "Placeholder selectors aren't allowed in plain CSS.",
                    &span,
                )));
            }
            Ok(sel)
        }
        _ if scanner.peek_char(0) == ':' as i32 => {
            pseudo_selector_impl(arena, scanner, state, parser_allow_parent, plain_css)
        }
        _ if scanner.peek_char(0) == '&' as i32 => {
            let sel = parent_selector_impl(arena, scanner, state, plain_css)?;
            if !allow_parent {
                let span = span_from_impl(scanner, state, start)?.file_span()?;
                return Err(Box::new(error_impl(
                    "Parent selectors aren't allowed here.",
                    &span,
                )));
            }
            Ok(sel)
        }
        _ => type_or_universal_selector_impl(scanner, state),
    }
}

// Consumes an attribute selector: `[name]`, `[name op value]`, and the
// optional single-letter case modifier (`[foo=bar i]`).
fn attribute_selector_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
) -> ParseResult<'parse, SimpleSelector<'parse>> {
    let start = scanner.state();
    scanner.expect_char('[')?;
    whitespace_impl(scanner, state, true)?;

    let name = attribute_name_impl(scanner, state)?;

    whitespace_impl(scanner, state, true)?;
    if scanner.scan_char(']') {
        let span = span_from_impl(scanner, state, start)?.file_span()?;
        return Ok(SimpleSelector::Attribute(AttributeSelector::new(
            name, span,
        )));
    }

    let op = attribute_operator_impl(scanner)?;
    whitespace_impl(scanner, state, true)?;

    let next = scanner.peek_char(0);
    let value = if next == '\'' as i32 || next == '"' as i32 {
        string_impl(scanner)?
    } else {
        identifier_impl(scanner, state, false, false)?
    };
    whitespace_impl(scanner, state, true)?;

    let next = scanner.peek_char(0);
    let modifier = if next >= 0 && character::is_alphabetic(next as u8 as char) {
        let ch = scanner.read_char()?;
        Some(ch.to_string())
    } else {
        None
    };

    scanner.expect_char(']')?;
    let span = span_from_impl(scanner, state, start)?.file_span()?;
    Ok(SimpleSelector::Attribute(
        AttributeSelector::new_with_operator(name, op, value, span, modifier),
    ))
}

// Consumes a qualified name as part of an attribute selector: `*|name`,
// `|name`, `ns|name`, or a bare name. A `|` followed by `=` is an
// operator, not a namespace separator.
fn attribute_name_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
) -> ParseResult<'parse, QualifiedName> {
    if scanner.scan_char('*') {
        scanner.expect_char('|')?;
        let name = identifier_impl(scanner, state, false, false)?;
        return Ok(QualifiedName::new_with_namespace(
            name,
            Some("*".to_string()),
        ));
    }

    if scanner.scan_char('|') {
        let name = identifier_impl(scanner, state, false, false)?;
        return Ok(QualifiedName::new_with_namespace(
            name,
            Some("".to_string()),
        ));
    }

    let name_or_namespace = identifier_impl(scanner, state, false, false)?;
    if scanner.peek_char(0) != '|' as i32 || scanner.peek_char(1) == '=' as i32 {
        return Ok(QualifiedName::new(name_or_namespace));
    }

    scanner.read_char()?; // consume '|'
    let name = identifier_impl(scanner, state, false, false)?;
    Ok(QualifiedName::new_with_namespace(
        name,
        Some(name_or_namespace),
    ))
}

// Consumes an attribute selector's operator (`=`, `~=`, `|=`, `^=`,
// `$=`, `*=`).
//
// Anything else is an `Expected "]".` error whose span covers the
// offending character.
fn attribute_operator_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
) -> ParseResult<'parse, AttributeOperator> {
    let start = scanner.pos();
    let ch = scanner.read_char()?;
    match ch {
        '=' => Ok(AttributeOperator::Equal),
        '~' => {
            scanner.expect_char('=')?;
            Ok(AttributeOperator::Include)
        }
        '|' => {
            scanner.expect_char('=')?;
            Ok(AttributeOperator::Dash)
        }
        '^' => {
            scanner.expect_char('=')?;
            Ok(AttributeOperator::Prefix)
        }
        '$' => {
            scanner.expect_char('=')?;
            Ok(AttributeOperator::Suffix)
        }
        '*' => {
            scanner.expect_char('=')?;
            Ok(AttributeOperator::Substring)
        }
        _ => Err(Box::new(
            scanner.error("Expected \"]\".", Some(start), 1).into(),
        )),
    }
}

// Consumes a class selector (`.` + name, with its span).
fn class_selector_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
) -> ParseResult<'parse, SimpleSelector<'parse>> {
    let start = scanner.state();
    scanner.expect_char('.')?;
    let name = identifier_impl(scanner, state, false, false)?;
    let span = span_from_impl(scanner, state, start)?.file_span()?;
    Ok(SimpleSelector::Class(ClassSelector::new(name, span)))
}

// Consumes an ID selector (`#` + name, with its span).
fn id_selector_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
) -> ParseResult<'parse, SimpleSelector<'parse>> {
    let start = scanner.state();
    scanner.expect_char('#')?;
    let name = identifier_impl(scanner, state, false, false)?;
    let span = span_from_impl(scanner, state, start)?.file_span()?;
    Ok(SimpleSelector::Id(IdSelector::new(name, span)))
}

// Consumes a placeholder selector (`%` + name, with its span).
fn placeholder_selector_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
) -> ParseResult<'parse, SimpleSelector<'parse>> {
    let start = scanner.state();
    scanner.expect_char('%')?;
    let name = identifier_impl(scanner, state, false, false)?;
    let span = span_from_impl(scanner, state, start)?.file_span()?;
    Ok(SimpleSelector::Placeholder(PlaceholderSelector::new(
        name, span,
    )))
}

// Consumes a parent selector: `&` plus an optional identifier-body suffix
// (`&-suffix`), which is rejected in plain CSS.
fn parent_selector_impl<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
    plain_css: bool,
) -> ParseResult<'parse, SimpleSelector<'parse>> {
    let start = scanner.state();
    scanner.expect_char('&')?;
    let suffix = if looking_at_identifier_body(scanner) {
        Some(identifier_body_impl(scanner)?)
    } else {
        None
    };

    // Dart line 413: plainCss check for suffix
    if plain_css && suffix.is_some() {
        return Err(scanner
            .error(
                "Parent selectors can't have suffixes in plain CSS.",
                Some(start.position),
                (scanner.pos().saturating_sub(start.position)) as isize,
            )
            .into());
    }

    let span = span_from_impl(scanner, state, start)?.file_span()?;
    Ok(SimpleSelector::Parent(ParentSelector::new(
        arena, span, suffix,
    )))
}

// Consumes a pseudo selector, including its parenthesized argument.
//
// Selector-taking pseudos (`:not`/`:is`/… and `::slotted`, matched after
// unvendoring the name) parse a nested selector list with the parser-level
// `allow_parent`; `:nth-child`/`:nth-last-child` parse `An+B` plus an
// optional `of <selector>` tail; other functional pseudos take a raw
// declaration value (trimmed of trailing whitespace).
fn pseudo_selector_impl<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
    allow_parent: bool,
    plain_css: bool,
) -> ParseResult<'parse, SimpleSelector<'parse>> {
    let start = scanner.state();
    scanner.expect_char(':')?;
    let element = scanner.scan_char(':');
    let name = identifier_impl(scanner, state, false, false)?;

    if !scanner.scan_char('(') {
        let span = span_from_impl(scanner, state, start)?.file_span()?;
        return Ok(SimpleSelector::Pseudo(PseudoSelector::new(
            name, span, element, None, None,
        )));
    }
    whitespace_impl(scanner, state, true)?;

    let unvendored = unvendor(&name);
    let argument: Option<String>;
    let selector: Option<Selector<'parse>>;

    if element {
        if SELECTOR_PSEUDO_ELEMENTS.contains(&unvendored.as_str()) {
            let list =
                selector_list_impl(arena, scanner, state, allow_parent, plain_css, None, None)?;
            selector = Some(Selector::List(list));
            argument = None;
        } else {
            argument = Some(declaration_value_impl(scanner, state, true)?);
            selector = None;
        }
    } else if SELECTOR_PSEUDO_CLASSES.contains(&unvendored.as_str()) {
        let list = selector_list_impl(arena, scanner, state, allow_parent, plain_css, None, None)?;
        selector = Some(Selector::List(list));
        argument = None;
    } else if unvendored == "nth-child" || unvendored == "nth-last-child" {
        let mut arg = a_n_plus_b_impl(scanner, state)?;
        whitespace_impl(scanner, state, true)?;
        if scanner.peek_char(-1) >= 0
            && character::is_whitespace(scanner.peek_char(-1) as u8 as char)
            && scanner.peek_char(0) != ')' as i32
        {
            expect_identifier_impl(scanner, "of", "", false)?;
            arg.push_str(" of");
            whitespace_impl(scanner, state, true)?;
            let list =
                selector_list_impl(arena, scanner, state, allow_parent, plain_css, None, None)?;
            selector = Some(Selector::List(list));
        } else {
            selector = None;
        }
        argument = Some(arg);
    } else {
        let val = declaration_value_impl(scanner, state, true)?;
        let trimmed = val.trim_end().to_string();
        argument = Some(trimmed);
        selector = None;
    }

    scanner.expect_char(')')?;
    let span = span_from_impl(scanner, state, start)?.file_span()?;
    Ok(SimpleSelector::Pseudo(PseudoSelector::new(
        name, span, element, argument, selector,
    )))
}

// Consumes an [`An+B` production][An+B] and returns its text.
//
// [An+B]: https://drafts.csswg.org/css-syntax-3/#anb-microsyntax
fn a_n_plus_b_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
) -> ParseResult<'parse, String> {
    let mut buf = String::new();

    match scanner.peek_char(0) {
        ch if ch == 'e' as i32 || ch == 'E' as i32 => {
            expect_identifier_impl(scanner, "even", "", false)?;
            return Ok("even".to_string());
        }
        ch if ch == 'o' as i32 || ch == 'O' as i32 => {
            expect_identifier_impl(scanner, "odd", "", false)?;
            return Ok("odd".to_string());
        }
        ch if ch == '+' as i32 || ch == '-' as i32 => {
            let c = scanner.read_char()?;
            buf.push(c);
        }
        _ => {}
    }

    if character::is_digit(scanner.peek_char(0) as u8 as char) {
        while character::is_digit(scanner.peek_char(0) as u8 as char) {
            let c = scanner.read_char()?;
            buf.push(c);
        }
        whitespace_impl(scanner, state, true)?;
        if !scan_ident_char_impl(scanner, 'n' as i32, false)? {
            return Ok(buf);
        }
    } else {
        expect_ident_char_impl(scanner, 'n' as i32, false)?;
    }
    buf.push('n');
    whitespace_impl(scanner, state, true)?;

    let next = scanner.peek_char(0);
    if next != '+' as i32 && next != '-' as i32 {
        return Ok(buf);
    }
    let c = scanner.read_char()?;
    buf.push(c);
    whitespace_impl(scanner, state, true)?;

    if !character::is_digit(scanner.peek_char(0) as u8 as char) {
        return Err(Box::new(
            scanner.error("Expected a number.", None, 0).into(),
        ));
    }
    while character::is_digit(scanner.peek_char(0) as u8 as char) {
        let c = scanner.read_char()?;
        buf.push(c);
    }
    Ok(buf)
}

// Consumes a type selector or a universal selector.
//
// These are combined because either one could start with `*`
// (`*`, `*|*`, `*|name`, `|*`, `|name`, `ns|*`, `ns|name`).
fn type_or_universal_selector_impl<'parse>(
    scanner: &mut SpanScanner<'parse>,
    state: &mut ParserState<'parse>,
) -> ParseResult<'parse, SimpleSelector<'parse>> {
    let start = scanner.state();

    if scanner.scan_char('*') {
        if !scanner.scan_char('|') {
            let span = span_from_impl(scanner, state, start)?.file_span()?;
            return Ok(SimpleSelector::Universal(UniversalSelector::new(
                span, None,
            )));
        }
        let wildcard = Some("*".to_string());
        if scanner.scan_char('*') {
            let span = span_from_impl(scanner, state, start)?.file_span()?;
            return Ok(SimpleSelector::Universal(UniversalSelector::new(
                span, wildcard,
            )));
        }
        let name = identifier_impl(scanner, state, false, false)?;
        let span = span_from_impl(scanner, state, start)?.file_span()?;
        return Ok(SimpleSelector::Type(TypeSelector::new(
            QualifiedName::new_with_namespace(name, wildcard),
            span,
        )));
    }

    if scanner.scan_char('|') {
        let empty = Some("".to_string());
        if scanner.scan_char('*') {
            let span = span_from_impl(scanner, state, start)?.file_span()?;
            return Ok(SimpleSelector::Universal(UniversalSelector::new(
                span, empty,
            )));
        }
        let name = identifier_impl(scanner, state, false, false)?;
        let span = span_from_impl(scanner, state, start)?.file_span()?;
        return Ok(SimpleSelector::Type(TypeSelector::new(
            QualifiedName::new_with_namespace(name, empty),
            span,
        )));
    }

    let name_or_namespace = identifier_impl(scanner, state, false, false)?;
    if !scanner.scan_char('|') {
        let span = span_from_impl(scanner, state, start)?.file_span()?;
        Ok(SimpleSelector::Type(TypeSelector::new(
            QualifiedName::new(name_or_namespace),
            span,
        )))
    } else if scanner.scan_char('*') {
        let span = span_from_impl(scanner, state, start)?.file_span()?;
        Ok(SimpleSelector::Universal(UniversalSelector::new(
            span,
            Some(name_or_namespace),
        )))
    } else {
        let name = identifier_impl(scanner, state, false, false)?;
        let span = span_from_impl(scanner, state, start)?.file_span()?;
        Ok(SimpleSelector::Type(TypeSelector::new(
            QualifiedName::new_with_namespace(name, Some(name_or_namespace)),
            span,
        )))
    }
}

// Whether `character` can start a simple selector in the middle of a
// compound selector. `&` only continues a compound in plain CSS.
fn is_simple_selector_start(ch: i32, plain_css: bool) -> bool {
    match ch {
        c if c == '*' as i32
            || c == '[' as i32
            || c == '.' as i32
            || c == '#' as i32
            || c == '%' as i32
            || c == ':' as i32 =>
        {
            true
        }
        c if c == '&' as i32 => plain_css,
        _ => false,
    }
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::exception::SassError;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    fn make_parser<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
    ) -> SelectorParser<'compile, 'parse, 'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        SelectorParser::new(arena, fs)
    }

    fn make_parser_plain_css<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
    ) -> SelectorParser<'compile, 'parse, 'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        SelectorParser::new_with_options(arena, fs, true, true, None, None, None)
    }

    fn make_parser_no_parent<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
    ) -> SelectorParser<'compile, 'parse, 'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        SelectorParser::new_with_options(arena, fs, false, false, None, None, None)
    }

    // ================================================================
    // Constructor
    // ================================================================

    #[test]
    fn test_constructor_defaults() {
        let arena = Bump::new();
        let parser = make_parser(&arena, "");
        assert!(parser.allow_parent);
        assert!(!parser.plain_css);
    }

    #[test]
    fn test_constructor_no_parent() {
        let arena = Bump::new();
        let parser = make_parser_no_parent(&arena, "");
        assert!(!parser.allow_parent);
    }

    #[test]
    fn test_constructor_plain_css() {
        let arena = Bump::new();
        let parser = make_parser_plain_css(&arena, "");
        assert!(parser.plain_css);
    }

    // ================================================================
    // Parse (public API)
    // ================================================================

    #[test]
    fn test_parse_valid_simple() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo");
        let list = parser.parse().unwrap();
        assert_eq!(list.0.components.len(), 1);
    }

    #[test]
    fn test_parse_valid_multi() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo, .bar");
        let list = parser.parse().unwrap();
        assert_eq!(list.0.components.len(), 2);
    }

    #[test]
    fn test_parse_error_empty() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "");
        assert!(parser.parse().is_err());
    }

    #[test]
    fn test_parse_error_trailing() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo {");
        assert!(parser.parse().is_err());
    }

    // ================================================================
    // ParseComplexSelector
    // ================================================================

    #[test]
    fn test_parse_complex_valid() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo");
        let cs = parser.parse_complex_selector().unwrap();
        assert_eq!(cs.components.len(), 1);
    }

    #[test]
    fn test_parse_complex_error_empty() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "");
        assert!(parser.parse_complex_selector().is_err());
    }

    #[test]
    fn test_parse_complex_error_trailing() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo {");
        assert!(parser.parse_complex_selector().is_err());
    }

    // ================================================================
    // ParseCompoundSelector
    // ================================================================

    #[test]
    fn test_parse_compound_valid() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo");
        let cs = parser.parse_compound_selector().unwrap();
        assert_eq!(cs.components.len(), 1);
    }

    #[test]
    fn test_parse_compound_error_empty() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "");
        assert!(parser.parse_compound_selector().is_err());
    }

    // ================================================================
    // ParseSimpleSelector
    // ================================================================

    #[test]
    fn test_parse_simple_class() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Class(c) => assert_eq!(c.name, "foo"),
            _ => panic!("expected ClassSelector"),
        }
    }

    #[test]
    fn test_parse_simple_error_trailing() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo bar");
        assert!(parser.parse_simple_selector().is_err());
    }

    // ================================================================
    // class_selector
    // ================================================================

    #[test]
    fn test_class_valid() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Class(c) => assert_eq!(c.name, "foo"),
            _ => panic!("expected ClassSelector"),
        }
    }

    #[test]
    fn test_class_error_invalid_ident() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".1foo");
        assert!(parser.parse_simple_selector().is_err());
    }

    // ================================================================
    // id_selector
    // ================================================================

    #[test]
    fn test_id_valid() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "#foo");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Id(i) => assert_eq!(i.name, "foo"),
            _ => panic!("expected IdSelector"),
        }
    }

    // ================================================================
    // placeholder_selector
    // ================================================================

    #[test]
    fn test_placeholder_valid() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "%foo");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Placeholder(p) => assert_eq!(p.name, "foo"),
            _ => panic!("expected PlaceholderSelector"),
        }
    }

    #[test]
    fn test_placeholder_plain_css_error() {
        let arena = Bump::new();
        let mut parser = make_parser_plain_css(&arena, "%foo");
        assert!(parser.parse_simple_selector().is_err());
    }

    // ================================================================
    // parent_selector
    // ================================================================

    #[test]
    fn test_parent_bare() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "&");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Parent(p) => assert!(p.suffix().is_none()),
            _ => panic!("expected ParentSelector"),
        }
    }

    #[test]
    fn test_parent_with_suffix() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "&-suffix");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Parent(p) => assert_eq!(p.suffix(), Some("-suffix")),
            _ => panic!("expected ParentSelector"),
        }
    }

    #[test]
    fn test_parent_suffix_plain_css_error() {
        let arena = Bump::new();
        let mut parser = make_parser_plain_css(&arena, "&-suffix");
        assert!(parser.parse_simple_selector().is_err());
    }

    #[test]
    fn test_parent_not_allowed() {
        let arena = Bump::new();
        let mut parser = make_parser_no_parent(&arena, "&");
        assert!(parser.parse_simple_selector().is_err());
    }

    #[test]
    fn test_parse_parent_not_allowed() {
        // allow_parent=false flows through parse() into the compound chain.
        let arena = Bump::new();
        let mut parser = make_parser_no_parent(&arena, "&");
        match *parser.parse().unwrap_err() {
            SassError::Format { message, .. } => {
                assert_eq!(message, "Parent selectors aren't allowed here.")
            }
            other => panic!("expected Format error, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_pseudo_parent_not_allowed() {
        // allow_parent=false also applies inside selector pseudo arguments.
        let arena = Bump::new();
        let mut parser = make_parser_no_parent(&arena, ":is(&)");
        match *parser.parse().unwrap_err() {
            SassError::Format { message, .. } => {
                assert_eq!(message, "Parent selectors aren't allowed here.")
            }
            other => panic!("expected Format error, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_pseudo_parent_allowed_non_initial_simple() {
        // The parser-level allow_parent applies to pseudo arguments even when
        // the pseudo is not the first simple selector in its compound (where
        // the local `&` override is plain_css=false).
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "d:is(&)");
        let list = parser.parse().unwrap();
        assert_eq!(list.to_css_string(true).unwrap(), "d:is(&)");
    }

    // ================================================================
    // attribute_selector
    // ================================================================

    #[test]
    fn test_attribute_bare() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "[foo]");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Attribute(a) => {
                assert_eq!(a.name.name, "foo");
                assert!(a.op.is_none());
            }
            _ => panic!("expected AttributeSelector"),
        }
    }

    #[test]
    fn test_attribute_equal_string() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "[foo=\"bar\"]");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Attribute(a) => {
                assert_eq!(a.name.name, "foo");
                assert_eq!(a.value.as_deref(), Some("bar"));
            }
            _ => panic!("expected AttributeSelector"),
        }
    }

    #[test]
    fn test_attribute_equal_ident() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "[foo=bar]");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Attribute(a) => {
                assert_eq!(a.value.as_deref(), Some("bar"));
            }
            _ => panic!("expected AttributeSelector"),
        }
    }

    #[test]
    fn test_attribute_operators() {
        let arena = Bump::new();
        let cases = [
            ("[foo~=bar]", AttributeOperator::Include),
            ("[foo|=bar]", AttributeOperator::Dash),
            ("[foo^=bar]", AttributeOperator::Prefix),
            ("[foo$=bar]", AttributeOperator::Suffix),
            ("[foo*=bar]", AttributeOperator::Substring),
        ];
        for (input, expected) in cases {
            let mut parser = make_parser(&arena, input);
            let sel = parser.parse_simple_selector().unwrap();
            match sel {
                SimpleSelector::Attribute(a) => {
                    assert_eq!(a.op.unwrap(), expected, "for input {input}");
                }
                _ => panic!("expected AttributeSelector for {input}"),
            }
        }
    }

    #[test]
    fn test_attribute_with_modifier() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "[foo=bar i]");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Attribute(a) => {
                assert_eq!(a.modifier.as_deref(), Some("i"));
            }
            _ => panic!("expected AttributeSelector"),
        }
    }

    #[test]
    fn test_attribute_error_missing_close() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "[foo");
        assert!(parser.parse_simple_selector().is_err());
    }

    // ================================================================
    // attribute_name
    // ================================================================

    #[test]
    fn test_attribute_name_plain() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "div");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Type(t) => {
                assert_eq!(t.name.name, "div");
                assert!(t.name.namespace.is_none());
            }
            _ => panic!("expected TypeSelector"),
        }
    }

    #[test]
    fn test_attribute_name_wildcard_ns() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "*|div");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Type(t) => {
                assert_eq!(t.name.name, "div");
                assert_eq!(t.name.namespace.as_deref(), Some("*"));
            }
            _ => panic!("expected TypeSelector"),
        }
    }

    #[test]
    fn test_attribute_name_empty_ns() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "|div");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Type(t) => {
                assert_eq!(t.name.name, "div");
                assert_eq!(t.name.namespace.as_deref(), Some(""));
            }
            _ => panic!("expected TypeSelector"),
        }
    }

    #[test]
    fn test_attribute_name_explicit_ns() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "ns|div");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Type(t) => {
                assert_eq!(t.name.name, "div");
                assert_eq!(t.name.namespace.as_deref(), Some("ns"));
            }
            _ => panic!("expected TypeSelector"),
        }
    }

    #[test]
    fn test_attribute_name_pipe_equals_disambig() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "[ns|=bar]");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Attribute(a) => {
                assert_eq!(a.op.unwrap(), AttributeOperator::Dash);
            }
            _ => panic!("expected AttributeSelector"),
        }
    }

    // ================================================================
    // attribute_operator
    // ================================================================

    #[test]
    fn test_attribute_operator_equal() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "[foo=bar]");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Attribute(a) => {
                assert_eq!(a.op.unwrap(), AttributeOperator::Equal);
            }
            _ => panic!("expected AttributeSelector"),
        }
    }

    #[test]
    fn test_attribute_operator_invalid() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "[foo!bar]");
        assert!(parser.parse_simple_selector().is_err());
    }

    // ================================================================
    // type_or_universal
    // ================================================================

    #[test]
    fn test_universal_star() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "*");
        let sel = parser.parse_simple_selector().unwrap();
        assert!(matches!(sel, SimpleSelector::Universal(_)));
    }

    #[test]
    fn test_universal_wildcard_ns() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "*|*");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Universal(u) => {
                assert_eq!(u.namespace.as_deref(), Some("*"));
            }
            _ => panic!("expected UniversalSelector"),
        }
    }

    #[test]
    fn test_type_wildcard_ns() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "*|div");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Type(t) => {
                assert_eq!(t.name.namespace.as_deref(), Some("*"));
            }
            _ => panic!("expected TypeSelector"),
        }
    }

    #[test]
    fn test_universal_empty_ns() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "|*");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Universal(u) => {
                assert_eq!(u.namespace.as_deref(), Some(""));
            }
            _ => panic!("expected UniversalSelector"),
        }
    }

    #[test]
    fn test_type_empty_ns() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "|div");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Type(t) => {
                assert_eq!(t.name.namespace.as_deref(), Some(""));
            }
            _ => panic!("expected TypeSelector"),
        }
    }

    #[test]
    fn test_type_plain() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "div");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Type(t) => {
                assert_eq!(t.name.name, "div");
                assert!(t.name.namespace.is_none());
            }
            _ => panic!("expected TypeSelector"),
        }
    }

    // ================================================================
    // is_simple_selector_start
    // ================================================================

    #[test]
    fn test_is_simple_start_star() {
        assert!(is_simple_selector_start('*' as i32, false));
    }

    #[test]
    fn test_is_simple_start_brackets() {
        for ch in ['[' as i32, '.' as i32, '#' as i32, '%' as i32, ':' as i32] {
            assert!(is_simple_selector_start(ch, false), "ch={ch}");
        }
    }

    #[test]
    fn test_is_simple_start_amp_plain() {
        assert!(is_simple_selector_start('&' as i32, true));
    }

    #[test]
    fn test_is_simple_start_amp_non_plain() {
        assert!(!is_simple_selector_start('&' as i32, false));
    }

    #[test]
    fn test_is_simple_start_non_match() {
        assert!(!is_simple_selector_start('a' as i32, false));
    }

    // ================================================================
    // a_n_plus_b
    // ================================================================

    #[test]
    fn test_anp_even() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":nth-child(even)");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert_eq!(p.argument.as_deref(), Some("even"));
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_anp_odd() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":nth-child(odd)");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert_eq!(p.argument.as_deref(), Some("odd"));
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_anp_pos_n() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":nth-child(+n)");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert_eq!(p.argument.as_deref(), Some("+n"));
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_anp_neg_n() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":nth-child(-n)");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert_eq!(p.argument.as_deref(), Some("-n"));
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_anp_two_n() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":nth-child(2n)");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert_eq!(p.argument.as_deref(), Some("2n"));
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_anp_two_n_one() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":nth-child(2n+1)");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert_eq!(p.argument.as_deref(), Some("2n+1"));
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_anp_bare_number() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":nth-child(5)");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert_eq!(p.argument.as_deref(), Some("5"));
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_anp_n_plus_three() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":nth-child(n+3)");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert_eq!(p.argument.as_deref(), Some("n+3"));
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_anp_error_n_plus_x() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":nth-child(n+x)");
        assert!(parser.parse_simple_selector().is_err());
    }

    // ================================================================
    // pseudo_selector
    // ================================================================

    #[test]
    fn test_pseudo_hover() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":hover");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert_eq!(p.name, "hover");
                assert!(p.argument.is_none());
                assert!(p.selector.is_none());
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_pseudo_element_before() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "::before");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert!(!p.is_class); // it's a pseudo-element
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_pseudo_not() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":not(.foo)");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert!(p.selector.is_some());
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_pseudo_is() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":is(.foo)");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert!(p.selector.is_some());
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_pseudo_slotted() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "::slotted(.foo)");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert!(!p.is_class);
                assert!(p.selector.is_some());
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_pseudo_nth_child_with_of() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":nth-child(2n+1 of .foo)");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert!(p.selector.is_some());
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_pseudo_lang() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":lang(en-us)");
        let sel = parser.parse_simple_selector().unwrap();
        match sel {
            SimpleSelector::Pseudo(p) => {
                assert_eq!(p.argument.as_deref(), Some("en-us"));
                assert!(p.selector.is_none());
            }
            _ => panic!("expected PseudoSelector"),
        }
    }

    #[test]
    fn test_pseudo_error_missing_close() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ":hover(");
        assert!(parser.parse_simple_selector().is_err());
    }

    // ================================================================
    // compound_selector
    // ================================================================

    #[test]
    fn test_compound_single() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo");
        let cs = parser.parse_compound_selector().unwrap();
        assert_eq!(cs.components.len(), 1);
    }

    #[test]
    fn test_compound_multi() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "a.b#c");
        let cs = parser.parse_compound_selector().unwrap();
        assert_eq!(cs.components.len(), 3);
    }

    #[test]
    fn test_compound_allow_parent() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "&.foo");
        let cs = parser.parse_compound_selector().unwrap();
        assert_eq!(cs.components.len(), 2);
        assert!(matches!(cs.components[0], SimpleSelector::Parent(_)));
    }

    #[test]
    fn test_compound_no_parent() {
        let arena = Bump::new();
        let mut parser = make_parser_no_parent(&arena, "&.foo");
        assert!(parser.parse_compound_selector().is_err());
    }

    // ================================================================
    // complex_selector
    // ================================================================

    #[test]
    fn test_complex_single() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo");
        let cs = parser.parse_complex_selector().unwrap();
        assert_eq!(cs.components.len(), 1);
    }

    #[test]
    fn test_complex_child() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo > .bar");
        let cs = parser.parse_complex_selector().unwrap();
        assert_eq!(cs.components.len(), 2);
    }

    #[test]
    fn test_complex_next_sibling() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo + .bar");
        let cs = parser.parse_complex_selector().unwrap();
        assert_eq!(cs.components.len(), 2);
    }

    #[test]
    fn test_complex_following_sibling() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo ~ .bar");
        let cs = parser.parse_complex_selector().unwrap();
        assert_eq!(cs.components.len(), 2);
    }

    #[test]
    fn test_complex_descendant() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo .bar");
        let cs = parser.parse_complex_selector().unwrap();
        assert_eq!(cs.components.len(), 2);
    }

    #[test]
    fn test_complex_leading_combinator() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "> .foo");
        let cs = parser.parse_complex_selector().unwrap();
        assert!(!cs.leading_combinators.is_empty());
    }

    #[test]
    fn test_complex_leading_only() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ">");
        let cs = parser.parse_complex_selector().unwrap();
        assert!(!cs.leading_combinators.is_empty());
        assert!(cs.components.is_empty());
    }

    #[test]
    fn test_complex_trailing_plain_css() {
        let arena = Bump::new();
        let mut parser = make_parser_plain_css(&arena, ".foo >");
        assert!(parser.parse_complex_selector().is_err());
    }

    #[test]
    fn test_complex_trailing_non_plain() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo >");
        let cs = parser.parse_complex_selector().unwrap();
        assert_eq!(cs.components.len(), 1);
    }

    #[test]
    fn test_complex_ampersand_trailing() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "a b &");
        let cs = parser.parse_complex_selector().unwrap();
        assert_eq!(cs.components.len(), 3);
    }

    #[test]
    fn test_complex_multi() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "a > b + c");
        let cs = parser.parse_complex_selector().unwrap();
        assert_eq!(cs.components.len(), 3);
    }

    // ================================================================
    // selector_list
    // ================================================================

    #[test]
    fn test_selector_list_single() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo");
        let list = parser.parse().unwrap();
        assert_eq!(list.0.components.len(), 1);
    }

    #[test]
    fn test_selector_list_comma() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo, .bar");
        let list = parser.parse().unwrap();
        assert_eq!(list.0.components.len(), 2);
    }

    #[test]
    fn test_selector_list_consecutive_commas() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo,, .bar");
        let list = parser.parse().unwrap();
        assert_eq!(list.0.components.len(), 2);
    }

    #[test]
    fn test_selector_list_line_break() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, ".foo,\n.bar");
        let list = parser.parse().unwrap();
        assert_eq!(list.0.components.len(), 2);
        assert!(list.0.components[1].line_break);
    }

    #[test]
    fn test_selector_list_error_empty() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "");
        assert!(parser.parse().is_err());
    }

    // ================================================================
    // Error propagation
    // ================================================================

    #[test]
    fn test_read_char_error_mid_selector() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "[foo=\"unterminated");
        assert!(parser.parse_simple_selector().is_err());
    }

    #[test]
    fn test_scanner_error_propagation() {
        let arena = Bump::new();
        let mut parser = make_parser(&arena, "div.");
        assert!(parser.parse_compound_selector().is_err());
    }
}
