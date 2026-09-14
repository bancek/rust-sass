// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/functions/selector.dart
// dart-source: lib/src/value.dart (Value._selectorString, _selectorStringOrNull, SassApiValue.assertSelector, assertCompoundSelector)
// go-source: go/functions/selector.go

use crate::common::source_span_highlighter::HighlightColor;
use crate::common::source_span_highlighter::HighlightOptions;
use crate::eval::warn::WarnLoggerAdapter;
use crate::extend::extend_static;
use crate::extend::replace_static;
use crate::termglyph::GlyphSet;
use std::rc::Rc;

use bumpalo::Bump;

use crate::callable::{BuiltInCallable, Callable, CallableKind};
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::{FileSpan, BOGUS_SPAN};
use crate::common::source_span_file_source::FileSource;
use crate::eval::warn::flush_buffered_warnings;
use crate::eval::{EvalConfig, EvalState};
use crate::logger::BufferedWarnLogger;
use crate::module::BuiltInModule;
use crate::parse::selector_parse::SelectorParser;
use crate::selector::complex::ComplexSelector;
use crate::selector::complex_component::ComplexSelectorComponent;
use crate::selector::compound::CompoundSelector;
use crate::selector::list::SelectorList;
use crate::selector::parent::ParentSelector;
use crate::selector::{SimpleSelector, WarnLogger};
use crate::value::{ListSeparator, SassList, SassString, Value, ValueKind, SASS_FALSE, SASS_TRUE};

/// Returns all globally-available selector functions.
///
/// Matches Dart: `global` in selector.dart — the same function objects as the
/// module, with the `selector-*` globals renamed and a `selector`
/// deprecation warning attached.
/// Matches Go: GlobalSelectorFunctions.
pub fn global_selector_functions<'compile, 'parse>(
    arena: &'compile Bump,
) -> Vec<Callable<'compile, 'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    macro_rules! c {
        ($arena:expr, $f:expr) => {
            Callable::new($arena, CallableKind::BuiltIn($f))
        };
    }
    vec![
        c!(
            arena,
            is_superselector_function(arena).with_deprecation_warning("selector", None)
        ),
        c!(
            arena,
            simple_selectors_function(arena).with_deprecation_warning("selector", None)
        ),
        c!(
            arena,
            selector_parse_function(arena).with_deprecation_warning("selector", Some("parse"))
        ),
        c!(
            arena,
            selector_nest_function(arena).with_deprecation_warning("selector", Some("nest"))
        ),
        c!(
            arena,
            selector_append_function(arena).with_deprecation_warning("selector", Some("append"))
        ),
        c!(
            arena,
            selector_extend_function(arena).with_deprecation_warning("selector", Some("extend"))
        ),
        c!(
            arena,
            selector_replace_function(arena).with_deprecation_warning("selector", Some("replace"))
        ),
        c!(
            arena,
            selector_unify_function(arena).with_deprecation_warning("selector", Some("unify"))
        ),
    ]
}

/// Returns the sass:selector built-in module.
///
/// Matches Dart: `module` in selector.dart (`BuiltInModule("selector", ...)`
/// with the eight unrenamed callables).
/// Matches Go: SelectorModule.
pub fn selector_module<'compile, 'parse>(arena: &'compile Bump) -> BuiltInModule<'compile, 'parse>
where
    'compile: 'parse,
{
    macro_rules! c {
        ($arena:expr, $f:expr) => {
            Callable::new($arena, CallableKind::BuiltIn($f))
        };
    }
    let fns: Vec<Callable<'compile, 'parse>> = vec![
        c!(arena, is_superselector_function(arena)),
        c!(arena, simple_selectors_function(arena)),
        c!(arena, selector_parse_module_function(arena)),
        c!(arena, selector_nest_module_function(arena)),
        c!(arena, selector_append_module_function(arena)),
        c!(arena, selector_extend_module_function(arena)),
        c!(arena, selector_replace_module_function(arena)),
        c!(arena, selector_unify_module_function(arena)),
    ];
    BuiltInModule::new(
        arena,
        "selector".into(),
        &fns,
        &[],
        indexmap::IndexMap::new(),
    )
}

// --- Helpers ---

/// Converts a SassScript value to a selector string.
///
/// Returns `None` if the value isn't a valid selector structure.
///
/// Matches Dart: `Value._selectorStringOrNull` (value.dart) — strings pass
/// through, commas join with `", "`, other separators with `" "`, slash
/// separators bail out, and non-strings bail out (recursing only into
/// space-separated nested lists).
/// Matches Go: selectorStringOrNil.
///
/// Both `Value::List` and `Value::ArgumentList` are accepted — in Dart,
/// SassArgumentList extends SassList, so argument lists pass the
/// `self is! SassList` check too.
fn selector_string_or_nil<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    v: &Value<'parse>,
) -> SassResult<Option<String>> {
    if let ValueKind::String(s) = &**v {
        return Ok(Some(s.text.to_string()));
    }
    if !matches!(&**v, ValueKind::List(_) | ValueKind::ArgumentList(_)) {
        return Ok(None);
    }
    let items = v.as_list(arena)?;
    if items.is_empty() {
        return Ok(None);
    }
    let mut parts: Vec<String> = Vec::new();
    match v.separator() {
        ListSeparator::Comma => {
            for item in &items {
                if let ValueKind::String(s) = &**item {
                    parts.push(s.text.to_string());
                } else if matches!(&**item, ValueKind::List(_) | ValueKind::ArgumentList(_)) {
                    if item.separator() != ListSeparator::Space {
                        return Ok(None);
                    }
                    match selector_string_or_nil(arena, item)? {
                        Some(s) => parts.push(s),
                        None => return Ok(None),
                    }
                } else {
                    return Ok(None);
                }
            }
        }
        ListSeparator::Slash => return Ok(None),
        _ => {
            for item in &items {
                if let ValueKind::String(s) = &**item {
                    parts.push(s.text.to_string());
                } else {
                    return Ok(None);
                }
            }
        }
    }
    let sep = if v.separator() == ListSeparator::Comma {
        ", "
    } else {
        " "
    };
    Ok(Some(parts.join(sep)))
}

/// Converts a SassScript value to a selector string that can be parsed.
///
/// Returns an error if the value isn't a valid selector structure (string,
/// list of strings, or list of lists of strings).
///
/// Matches Dart: `Value._selectorString` (value.dart) — the throwing wrapper
/// around `_selectorStringOrNull`.
/// Matches Go: selectorString.
fn selector_string<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    v: &Value<'parse>,
    name: &str,
) -> SassResult<String> {
    if let Some(s) = selector_string_or_nil(arena, v)? {
        return Ok(s);
    }
    let v_str = v.to_display_string()?;
    Err(Box::new(SassError::Script {
        message: format!(
            "{} is not a valid selector: it must be a string,\na list of strings, or a list of lists of strings.",
            v_str
        ),
        argument_name: arg_name(name),
    }))
}

/// Maps Go's empty-string ArgumentName to Rust's `None`.
fn arg_name(name: &str) -> Option<String> {
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Converts a SassScript value to a SelectorList by parsing the selector
/// string. If `allow_parent` is true, parent selectors (&) are permitted.
///
/// Matches Dart: `SassApiValue.assertSelector` (value.dart) — stringifies the
/// value, then parses as a [`SelectorList`], wrapping parse failures in a
/// script error that embeds the formatted parse error (minus the `Error: `
/// prefix). If the value came from a function argument, `name` is the
/// argument name (without the `$`) used for error reporting.
/// Matches Go: assertSelector.
fn assert_selector<'compile, 'parse>(
    v: &Value<'parse>,
    name: &str,
    allow_parent: bool,
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
) -> SassResult<SelectorList<'parse>>
where
    'compile: 'parse,
{
    let s = selector_string(arena, v, name)?;
    let source = FileSource::new_in(arena, &s, None);
    let warn_buf = BufferedWarnLogger::new(arena);
    let mut parser = SelectorParser::new_with_options(
        arena,
        source,
        allow_parent,
        false,
        None,
        Some(&warn_buf),
        None,
    );
    let result = match parser.parse() {
        Ok(r) => r,
        Err(err) => {
            // Mirrors Dart's `assertSelector` (value.dart:457-470): the
            // formatted parse error (message + selector-text highlight + trace,
            // without the leading "Error: ") is embedded in the Script error's
            // message, which the eval call site then renders above the
            // function-call highlight + trace.
            let msg = err
                .to_error_string_with_options(
                    &HighlightOptions {
                        color: if config.alert_color {
                            HighlightColor::Default
                        } else {
                            HighlightColor::None
                        },
                        glyphs: if config.alert_ascii || !config.unicode {
                            GlyphSet::Ascii
                        } else {
                            GlyphSet::default()
                        },
                        ..Default::default()
                    },
                    config.io.as_ref(),
                )
                .strip_prefix("Error: ")
                .unwrap_or("")
                .trim_end()
                .to_string();
            return Err(Box::new(SassError::Script {
                message: msg,
                argument_name: arg_name(name),
            }));
        }
    };
    flush_buffered_warnings(&warn_buf, config, state)?;
    Ok(result)
}

/// Converts a SassScript value to a CompoundSelector.
///
/// Matches Dart: `Value.assertCompoundSelector` (value.dart:510-527) — parses
/// at the COMPOUND level (`CompoundSelector.parse`), so `"c, d"` fails with
/// `expected selector.` instead of the Go-style `must be a compound selector`.
fn assert_compound_selector<'compile, 'parse>(
    v: &Value<'parse>,
    name: &str,
    config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
) -> SassResult<CompoundSelector<'parse>>
where
    'compile: 'parse,
{
    let s = selector_string(arena, v, name)?;
    let source = FileSource::new_in(arena, &s, None);
    let warn_buf = BufferedWarnLogger::new(arena);
    let mut parser =
        SelectorParser::new_with_options(arena, source, false, false, None, Some(&warn_buf), None);
    match parser.parse_compound_selector() {
        Ok(compound) => Ok(compound),
        Err(err) => {
            // Mirrors Dart's `assertCompoundSelector` (value.dart:518-525):
            // the formatted parse error without the leading "Error: " becomes
            // the Script error's message.
            let msg = err
                .to_error_string_with_options(
                    &HighlightOptions {
                        color: if config.alert_color {
                            HighlightColor::Default
                        } else {
                            HighlightColor::None
                        },
                        glyphs: if config.alert_ascii || !config.unicode {
                            GlyphSet::Ascii
                        } else {
                            GlyphSet::default()
                        },
                        ..Default::default()
                    },
                    config.io.as_ref(),
                )
                .strip_prefix("Error: ")
                .unwrap_or("")
                .trim_end()
                .to_string();
            Err(Box::new(SassError::Script {
                message: msg,
                argument_name: arg_name(name),
            }))
        }
    }
}

/// Renders a [`SelectorList`] as a comma-separated string for the
/// `Can't append ... to ...` error messages.
///
/// Dart interpolates the parent list inline at each throw site; extracted here
/// since both failure sites need the same rendering.
/// Matches Go: selectorListToString.
fn selector_list_to_string(sl: &SelectorList<'_>) -> SassResult<String> {
    let mut parts = Vec::with_capacity(sl.0.components.len());
    for c in &sl.0.components {
        parts.push(c.to_css_string(true)?);
    }
    Ok(parts.join(", "))
}

/// Adds a [`ParentSelector`] to the beginning of `compound`, or returns
/// `None` if that wouldn't produce a valid selector.
///
/// A leading universal selector or a namespaced type selector can't take a
/// parent (returns `None`); a plain type selector becomes the parent's suffix,
/// anything else gets a bare parent prepended. `span` covers the new nodes
/// and comes from the current callable span.
///
/// Matches Dart: `_prependParent` (selector.dart:168-184).
/// Matches Go: prependParent.
fn prepend_parent<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    compound: &CompoundSelector<'parse>,
    span: FileSpan<'parse>,
) -> SassResult<Option<CompoundSelector<'parse>>> {
    let first = &compound.components[0];
    match first {
        SimpleSelector::Universal(_) => Ok(None),
        SimpleSelector::Type(t) => {
            if t.name.namespace.is_some() {
                return Ok(None);
            }
            let rest = &compound.components[1..];
            let mut comps: Vec<SimpleSelector<'parse>> = Vec::with_capacity(1 + rest.len());
            comps.push(SimpleSelector::Parent(ParentSelector::new(
                arena,
                span,
                Some(t.name.name.clone()),
            )));
            comps.extend_from_slice(rest);
            Ok(Some(CompoundSelector::new(comps, span)?))
        }
        _ => {
            let mut comps: Vec<SimpleSelector<'parse>> =
                Vec::with_capacity(1 + compound.components.len());
            comps.push(SimpleSelector::Parent(ParentSelector::new(
                arena, span, None,
            )));
            comps.extend_from_slice(&compound.components);
            Ok(Some(CompoundSelector::new(comps, span)?))
        }
    }
}

// --- Shared implementation functions ---
//
// Dart shares the same function objects between module and global (renaming
// via .withName() for the global versions). Go mirrors that with shared
// closure vars; Rust uses free functions with the callback signature.
// Each `*_function` below passes `"sass:selector"` as the URL, like Dart's
// `_function` helper (selector.dart:186-193), inlined at each call site here.

/// Returns whether `$super` is a superselector of `$sub`.
///
/// Matches Dart: `_isSuperselector` (selector.dart:138-147) — asserts both
/// arguments as selector lists (rejecting bogus combinators) and returns the
/// [`SelectorList::is_superselector`] verdict as a boolean.
fn is_superselector_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let sel1 = assert_selector(&args[0], "super", false, config, state, arena)?;
    sel1.assert_not_bogus(
        Some("super"),
        Some(&mut WarnLoggerAdapter { config, state } as &mut dyn WarnLogger),
    )?;
    let sel2 = assert_selector(&args[1], "sub", false, config, state, arena)?;
    sel2.assert_not_bogus(
        Some("sub"),
        Some(&mut WarnLoggerAdapter { config, state } as &mut dyn WarnLogger),
    )?;
    let ok = sel1.is_superselector(&sel2)?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Boolean(if ok { SASS_TRUE } else { SASS_FALSE }),
    ))
}

/// Creates a `sass:selector` callable named `is-superselector` with signature
/// `$super, $sub`.
///
/// Shared by the module and the deprecated global of the same name.
fn is_superselector_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "is-superselector",
        "$super, $sub",
        "sass:selector",
        arena,
        Rc::new(is_superselector_impl),
    )
}

/// Returns the simple selectors of `$selector` as a comma-separated list of
/// unquoted strings.
///
/// Matches Dart: `_simpleSelectors` (selector.dart:149-160) — parses at the
/// compound level (see [`assert_compound_selector`]) and renders each
/// component with `to_css_string`.
fn simple_selectors_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let compound = assert_compound_selector(&args[0], "selector", config, state, arena)?;
    let mut items: Vec<Value<'parse>> = Vec::with_capacity(compound.components.len());
    for s in &compound.components {
        items.push(Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(
                arena.alloc_str(&s.to_css_string(true)?),
                false,
            )),
        ));
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::List(SassList::new(items, ListSeparator::Comma, false)),
    ))
}

/// Creates a `sass:selector` callable named `simple-selectors` with signature
/// `$selector`.
///
/// Shared by the module and the deprecated global of the same name.
fn simple_selectors_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "simple-selectors",
        "$selector",
        "sass:selector",
        arena,
        Rc::new(simple_selectors_impl),
    )
}

/// Parses `$selector` into a Sass list of selectors.
///
/// Matches Dart: `_parse` (selector.dart:162-166) — asserts the argument as a
/// selector list and returns [`SelectorList::as_sass_list`].
fn parse_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let sel = assert_selector(&args[0], "selector", false, config, state, arena)?;
    sel.as_sass_list(arena)
}

/// Creates a `sass:selector` callable named `selector-parse` with signature
/// `$selector`.
///
/// The deprecated global spelling of the module's `parse`; Dart renames via
/// `.withName("selector-parse")`.
fn selector_parse_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "selector-parse",
        "$selector",
        "sass:selector",
        arena,
        Rc::new(parse_impl),
    )
}

/// Creates a `sass:selector` callable named `parse` with signature
/// `$selector`.
///
/// The `sass:selector` module spelling.
fn selector_parse_module_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "parse",
        "$selector",
        "sass:selector",
        arena,
        Rc::new(parse_impl),
    )
}

/// Nests `$selectors...` via [`SelectorList::nest_within`], allowing parent
/// selectors.
///
/// Matches Dart: `_nest` (selector.dart:44-56) — asserts each selector with
/// `allow_parent: true` and folds with `nestWithin`, throwing
/// `$selectors: At least one selector must be passed.` on empty input (the
/// `None` fold result).
fn nest_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let selectors = args[0].as_list(arena)?;
    if selectors.is_empty() {
        return Err(Box::new(SassError::Script {
            message: "$selectors: At least one selector must be passed.".into(),
            argument_name: None,
        }));
    }
    let mut result: Option<SelectorList<'_>> = None;
    for v in &selectors {
        let sel = assert_selector(v, "", true, config, state, arena)?;
        result = Some(sel.nest_within(arena, result.as_ref(), true, false)?);
    }
    result.unwrap().as_sass_list(arena)
}

/// Creates a `sass:selector` callable named `selector-nest` with signature
/// `$selectors...`.
///
/// The deprecated global spelling of the module's `nest`.
fn selector_nest_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "selector-nest",
        "$selectors...",
        "sass:selector",
        arena,
        Rc::new(nest_impl),
    )
}

/// Creates a `sass:selector` callable named `nest` with signature
/// `$selectors...`.
///
/// The `sass:selector` module spelling.
fn selector_nest_module_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "nest",
        "$selectors...",
        "sass:selector",
        arena,
        Rc::new(nest_impl),
    )
}

/// Appends `$selectors...` right-to-left, prepending a parent to each child
/// complex before nesting it within the accumulated parent.
///
/// Matches Dart: `_append` (selector.dart:58-91) — a complex with leading
/// combinators, or whose first compound can't take a parent (see
/// [`prepend_parent`]), fails with `Can't append <child> to <parent>.`
/// Selectors parse lazily during the fold, so an early append failure fires
/// before a later parse error.
fn append_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let selectors = args[0].as_list(arena)?;
    if selectors.is_empty() {
        return Err(Box::new(SassError::Script {
            message: "$selectors: At least one selector must be passed.".into(),
            argument_name: None,
        }));
    }
    // Dart parses LAZILY during the fold (`map.parse.reduce`, selector.dart
    // :67-90): an early append failure fires before a later parse error.
    // Pre-parsing all selectors would report the parse error first.
    let span = state.callable_span.unwrap_or(BOGUS_SPAN);
    let mut result = assert_selector(&selectors[0], "", false, config, state, arena)?;
    for child_v in &selectors[1..] {
        let child = assert_selector(child_v, "", false, config, state, arena)?;
        let mut new_components: Vec<ComplexSelector<'_>> = Vec::new();
        for complex in &child.0.components {
            if !complex.leading_combinators.is_empty() {
                let s = selector_list_to_string(&result)?;
                let complex_str = complex.to_css_string(true)?;
                return Err(Box::new(SassError::Script {
                    message: format!("Can't append {} to {}.", complex_str, s),
                    argument_name: None,
                }));
            }
            let first_comp = &complex.components[0];
            let rest = &complex.components[1..];
            let new_compound = match prepend_parent(arena, &first_comp.selector, span)? {
                Some(c) => c,
                None => {
                    let s = selector_list_to_string(&result)?;
                    let complex_str = complex.to_css_string(true)?;
                    return Err(Box::new(SassError::Script {
                        message: format!("Can't append {} to {}.", complex_str, s),
                        argument_name: None,
                    }));
                }
            };
            let mut new_comps: Vec<ComplexSelectorComponent<'_>> =
                Vec::with_capacity(1 + rest.len());
            new_comps.push(ComplexSelectorComponent::new(
                Box::new(new_compound),
                first_comp.combinators.clone(),
                span,
            ));
            new_comps.extend_from_slice(rest);
            let cs = ComplexSelector::new(vec![], new_comps, span, false)?;
            new_components.push(cs);
        }
        let child_list = SelectorList::new(arena, new_components, span)?;
        result = child_list.nest_within(arena, Some(&result), true, false)?;
    }
    result.as_sass_list(arena)
}

/// Creates a `sass:selector` callable named `selector-append` with signature
/// `$selectors...`.
///
/// The deprecated global spelling of the module's `append`.
fn selector_append_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "selector-append",
        "$selectors...",
        "sass:selector",
        arena,
        Rc::new(append_impl),
    )
}

/// Creates a `sass:selector` callable named `append` with signature
/// `$selectors...`.
///
/// The `sass:selector` module spelling.
fn selector_append_module_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "append",
        "$selectors...",
        "sass:selector",
        arena,
        Rc::new(append_impl),
    )
}

/// Extends `$selector` by replacing `$extendee` with `$extender`.
///
/// Matches Dart: `_extend` (selector.dart:93-109) — asserts all three
/// arguments as selector lists (rejecting bogus combinators) and runs the
/// extension store at the current callable span.
fn extend_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let sel = assert_selector(&args[0], "selector", false, config, state, arena)?;
    sel.assert_not_bogus(
        Some("selector"),
        Some(&mut WarnLoggerAdapter { config, state } as &mut dyn WarnLogger),
    )?;
    let target = assert_selector(&args[1], "extendee", false, config, state, arena)?;
    target.assert_not_bogus(
        Some("extendee"),
        Some(&mut WarnLoggerAdapter { config, state } as &mut dyn WarnLogger),
    )?;
    let source = assert_selector(&args[2], "extender", false, config, state, arena)?;
    source.assert_not_bogus(
        Some("extender"),
        Some(&mut WarnLoggerAdapter { config, state } as &mut dyn WarnLogger),
    )?;
    let current_callable_span = state.callable_span.unwrap_or(BOGUS_SPAN);
    let result = extend_static(
        arena,
        &sel,
        &source,
        &target,
        current_callable_span,
        config.io.clone(),
    )?;
    result.as_sass_list(arena)
}

/// Creates a `sass:selector` callable named `selector-extend` with signature
/// `$selector, $extendee, $extender`.
///
/// The deprecated global spelling of the module's `extend`.
fn selector_extend_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "selector-extend",
        "$selector, $extendee, $extender",
        "sass:selector",
        arena,
        Rc::new(extend_impl),
    )
}

/// Creates a `sass:selector` callable named `extend` with signature
/// `$selector, $extendee, $extender`.
///
/// The `sass:selector` module spelling.
fn selector_extend_module_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "extend",
        "$selector, $extendee, $extender",
        "sass:selector",
        arena,
        Rc::new(extend_impl),
    )
}

/// Replaces `$original` with `$replacement` within `$selector`.
///
/// Matches Dart: `_replace` (selector.dart:111-127) — asserts all three
/// arguments as selector lists (rejecting bogus combinators) and runs the
/// extension-store replace at the current callable span.
fn replace_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let sel = assert_selector(&args[0], "selector", false, config, state, arena)?;
    sel.assert_not_bogus(
        Some("selector"),
        Some(&mut WarnLoggerAdapter { config, state } as &mut dyn WarnLogger),
    )?;
    let target = assert_selector(&args[1], "original", false, config, state, arena)?;
    target.assert_not_bogus(
        Some("original"),
        Some(&mut WarnLoggerAdapter { config, state } as &mut dyn WarnLogger),
    )?;
    let source = assert_selector(&args[2], "replacement", false, config, state, arena)?;
    source.assert_not_bogus(
        Some("replacement"),
        Some(&mut WarnLoggerAdapter { config, state } as &mut dyn WarnLogger),
    )?;
    let current_callable_span = state.callable_span.unwrap_or(BOGUS_SPAN);
    let result = replace_static(
        arena,
        &sel,
        &source,
        &target,
        current_callable_span,
        config.io.clone(),
    )?;
    result.as_sass_list(arena)
}

/// Creates a `sass:selector` callable named `selector-replace` with signature
/// `$selector, $original, $replacement`.
///
/// The deprecated global spelling of the module's `replace`.
fn selector_replace_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "selector-replace",
        "$selector, $original, $replacement",
        "sass:selector",
        arena,
        Rc::new(replace_impl),
    )
}

/// Creates a `sass:selector` callable named `replace` with signature
/// `$selector, $original, $replacement`.
///
/// The `sass:selector` module spelling.
fn selector_replace_module_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "replace",
        "$selector, $original, $replacement",
        "sass:selector",
        arena,
        Rc::new(replace_impl),
    )
}

/// Unifies `$selector1` and `$selector2`, returning null when they can't
/// match the same element.
///
/// Matches Dart: `_unify` (selector.dart:129-136) — asserts both arguments as
/// selector lists (rejecting bogus combinators); [`SelectorList::unify`] maps
/// onto a Sass list, `None` onto null.
fn unify_impl<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let sel1 = assert_selector(&args[0], "selector1", false, config, state, arena)?;
    sel1.assert_not_bogus(
        Some("selector1"),
        Some(&mut WarnLoggerAdapter { config, state } as &mut dyn WarnLogger),
    )?;
    let sel2 = assert_selector(&args[1], "selector2", false, config, state, arena)?;
    sel2.assert_not_bogus(
        Some("selector2"),
        Some(&mut WarnLoggerAdapter { config, state } as &mut dyn WarnLogger),
    )?;
    match sel1.unify(arena, &sel2)? {
        None => Ok(Value::new_with_arena(arena, ValueKind::Null)),
        Some(result) => Ok(result.as_sass_list(arena)?),
    }
}

/// Creates a `sass:selector` callable named `selector-unify` with signature
/// `$selector1, $selector2`.
///
/// The deprecated global spelling of the module's `unify`.
fn selector_unify_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "selector-unify",
        "$selector1, $selector2",
        "sass:selector",
        arena,
        Rc::new(unify_impl),
    )
}

/// Creates a `sass:selector` callable named `unify` with signature
/// `$selector1, $selector2`.
///
/// The `sass:selector` module spelling.
fn selector_unify_module_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "unify",
        "$selector1, $selector2",
        "sass:selector",
        arena,
        Rc::new(unify_impl),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::serialize_value_inspect;
    use crate::value::SassArgumentList;
    use crate::value::SassMap;

    use crate::functions::test_utils::{
        assert_is_false, assert_is_true, assert_no_warnings, eval, eval_recorded,
    };
    use crate::logger::test_utils::RecordLogger;
    use crate::value::SassNumber;

    // --- helpers ---

    /// Mirrors Go: value.SerializeValueInspect (assertInspect in selector_test.go).
    fn inspect(v: &Value<'_>) -> String {
        serialize_value_inspect(v).unwrap()
    }

    fn assert_inspect(got: &Value<'_>, want: &str) {
        assert_eq!(inspect(got), want);
    }

    fn str_val<'compile: 'parse, 'parse>(arena: &'compile Bump, s: &str) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(s), false)),
        )
    }

    fn quoted_val<'compile: 'parse, 'parse>(arena: &'compile Bump, s: &str) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(s), true)),
        )
    }

    fn num_val<'compile: 'parse, 'parse>(arena: &'compile Bump, v: f64) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(v, None)))
    }

    fn list_of<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        sep: ListSeparator,
        items: &[Value<'parse>],
    ) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::List(SassList::new(items.to_vec(), sep, false)),
        )
    }

    fn comma_list<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        items: &[Value<'parse>],
    ) -> Value<'parse> {
        list_of(arena, ListSeparator::Comma, items)
    }

    fn space_list<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        items: &[Value<'parse>],
    ) -> Value<'parse> {
        list_of(arena, ListSeparator::Space, items)
    }

    fn slash_list<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        items: &[Value<'parse>],
    ) -> Value<'parse> {
        list_of(arena, ListSeparator::Slash, items)
    }

    fn arg_list<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        sep: ListSeparator,
        items: &[Value<'parse>],
    ) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::ArgumentList(SassArgumentList::new(
                arena,
                items.to_vec(),
                Default::default(),
                sep,
            )),
        )
    }

    fn map_val<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        pairs: &[(Value<'parse>, Value<'parse>)],
    ) -> Value<'parse> {
        let mut m = SassMap::empty();
        for (k, v) in pairs {
            m.set(*k, *v);
        }
        Value::new_with_arena(arena, ValueKind::Map(m))
    }

    /// Asserts err is a Script error with the given message prefix and argument
    /// name. Mirrors Go: assertScriptErr (Message/ArgumentName field split).
    /// Parse-error messages embed the formatted selector-text highlight, so the
    /// message is checked as a prefix.
    fn assert_script_err(err: Box<SassError>, want_msg: &str, want_arg: Option<&str>) {
        match *err {
            SassError::Script {
                message,
                argument_name,
            } => {
                assert!(
                    message.starts_with(want_msg),
                    "expected message to start with {want_msg:?}, got {message:?}"
                );
                assert_eq!(argument_name.as_deref(), want_arg);
            }
            other => panic!("expected Script error, got {other:?}"),
        }
    }

    fn not_valid_selector_msg(v: &str) -> String {
        format!(
            "{} is not a valid selector: it must be a string,\na list of strings, or a list of lists of strings.",
            v
        )
    }

    fn bogus_warning_msg(prefix: &str, selector: &str) -> String {
        [
            &format!("{prefix}{selector} is not valid CSS."),
            "This will be an error in Dart Sass 2.0.0.",
            "",
            "More info: https://sass-lang.com/d/bogus-combinators",
        ]
        .join("\n")
    }

    fn global_builtin_warning_msg(name: &str) -> String {
        [
            "Global built-in functions are deprecated and will be removed in Dart Sass 3.0.0.",
            &format!("Use selector.{name} instead."),
            "",
            "More info and automated migrator: https://sass-lang.com/d/import",
        ]
        .join("\n")
    }

    fn assert_single_warning(logger: &RecordLogger, want: &str, dep_id: &str) {
        let messages = logger.messages();
        assert_eq!(messages.len(), 1, "warnings = {messages:?}, want 1");
        assert_eq!(messages[0], want);
        assert_eq!(logger.deprecation_ids()[0].as_deref(), Some(dep_id));
    }

    fn global_bic<'compile, 'parse>(
        arena: &'compile Bump,
        index: usize,
    ) -> BuiltInCallable<'compile, 'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fns = global_selector_functions(arena);
        match fns[index].kind() {
            CallableKind::BuiltIn(b) => b.clone(),
            _ => panic!("expected BuiltIn callable"),
        }
    }

    #[rust_sass_macros::maybe_async]
    async fn eval_ok<'compile, 'parse>(
        arena: &'compile Bump,
        fn_: &BuiltInCallable<'compile, 'parse>,
        args: &[Value<'parse>],
    ) -> Value<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        eval(arena, fn_, args).await.unwrap()
    }

    // --- global_selector_functions ---

    #[rust_sass_macros::maybe_test]
    async fn test_global_selector_functions_names() {
        let arena = Bump::new();
        let fns = global_selector_functions(&arena);
        let want = [
            "is-superselector",
            "simple-selectors",
            "selector-parse",
            "selector-nest",
            "selector-append",
            "selector-extend",
            "selector-replace",
            "selector-unify",
        ];
        assert_eq!(fns.len(), want.len());
        for (i, f) in fns.iter().enumerate() {
            assert_eq!(f.name(), want[i], "fns[{i}]");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_selector_functions_deprecation_metadata() {
        let arena = Bump::new();
        let fns = global_selector_functions(&arena);
        let want = [
            ("is-superselector", "is-superselector"),
            ("simple-selectors", "simple-selectors"),
            ("selector-parse", "parse"),
            ("selector-nest", "nest"),
            ("selector-append", "append"),
            ("selector-extend", "extend"),
            ("selector-replace", "replace"),
            ("selector-unify", "unify"),
        ];
        for (i, f) in fns.iter().enumerate() {
            match f.kind() {
                CallableKind::BuiltIn(b) => {
                    assert_eq!(b.name(), want[i].0);
                    let dw = b
                        .deprecation_warning()
                        .expect("deprecation warning should be set");
                    assert_eq!(dw.0, "selector");
                    assert_eq!(dw.1, want[i].1);
                }
                _ => panic!("expected BuiltIn callable"),
            }
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_selector_parse_emits_deprecation_warning() {
        let arena = Bump::new();
        let parse = global_bic(&arena, 2);
        let (result, logger) = eval_recorded(&arena, &parse, &[str_val(&arena, "c")]).await;
        assert_inspect(&result.unwrap(), "(c,)");
        assert_single_warning(
            &logger,
            &global_builtin_warning_msg("parse"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_is_superselector_emits_deprecation_warning() {
        let arena = Bump::new();
        let is_super = global_bic(&arena, 0);
        let (result, logger) = eval_recorded(
            &arena,
            &is_super,
            &[str_val(&arena, ".c"), str_val(&arena, ".c.d")],
        )
        .await;
        assert_is_true(&result.unwrap());
        assert_single_warning(
            &logger,
            &global_builtin_warning_msg("is-superselector"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_simple_selectors_emits_deprecation_warning() {
        let arena = Bump::new();
        let simple = global_bic(&arena, 1);
        let (result, logger) = eval_recorded(&arena, &simple, &[str_val(&arena, ".foo.bar")]).await;
        assert_inspect(&result.unwrap(), ".foo, .bar");
        assert_single_warning(
            &logger,
            &global_builtin_warning_msg("simple-selectors"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_selector_nest_emits_deprecation_warning() {
        let arena = Bump::new();
        let nest = global_bic(&arena, 3);
        let (result, logger) = eval_recorded(
            &arena,
            &nest,
            &[comma_list(
                &arena,
                &[str_val(&arena, "c"), str_val(&arena, "d")],
            )],
        )
        .await;
        assert_inspect(&result.unwrap(), "(c d,)");
        assert_single_warning(
            &logger,
            &global_builtin_warning_msg("nest"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_selector_append_emits_deprecation_warning() {
        let arena = Bump::new();
        let append = global_bic(&arena, 4);
        let (result, logger) = eval_recorded(
            &arena,
            &append,
            &[comma_list(
                &arena,
                &[str_val(&arena, ".c"), str_val(&arena, ".d")],
            )],
        )
        .await;
        assert_inspect(&result.unwrap(), "(.c.d,)");
        assert_single_warning(
            &logger,
            &global_builtin_warning_msg("append"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_selector_extend_emits_deprecation_warning() {
        let arena = Bump::new();
        let extend = global_bic(&arena, 5);
        let (result, logger) = eval_recorded(
            &arena,
            &extend,
            &[
                str_val(&arena, "c"),
                str_val(&arena, "c"),
                str_val(&arena, "e"),
            ],
        )
        .await;
        assert_inspect(&result.unwrap(), "c, e");
        assert_single_warning(
            &logger,
            &global_builtin_warning_msg("extend"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_selector_replace_emits_deprecation_warning() {
        let arena = Bump::new();
        let replace = global_bic(&arena, 6);
        let (result, logger) = eval_recorded(
            &arena,
            &replace,
            &[
                str_val(&arena, "c"),
                str_val(&arena, "c"),
                str_val(&arena, "d"),
            ],
        )
        .await;
        assert_inspect(&result.unwrap(), "(d,)");
        assert_single_warning(
            &logger,
            &global_builtin_warning_msg("replace"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_selector_unify_emits_deprecation_warning() {
        let arena = Bump::new();
        let unify = global_bic(&arena, 7);
        let (result, logger) = eval_recorded(
            &arena,
            &unify,
            &[str_val(&arena, ".c"), str_val(&arena, ".d")],
        )
        .await;
        assert_inspect(&result.unwrap(), "(.c.d,)");
        assert_single_warning(
            &logger,
            &global_builtin_warning_msg("unify"),
            "global-builtin",
        );
    }

    // --- selector_module ---

    #[rust_sass_macros::maybe_test]
    async fn test_selector_module_url() {
        let arena = Bump::new();
        let m = selector_module(&arena);
        assert_eq!(m.url, "sass:selector");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_module_functions() {
        let arena = Bump::new();
        let m = selector_module(&arena);
        let want = [
            "is-superselector",
            "simple-selectors",
            "parse",
            "nest",
            "append",
            "extend",
            "replace",
            "unify",
        ];
        assert_eq!(m.functions.len(), want.len());
        for (i, name) in m.functions.keys().enumerate() {
            assert_eq!(name, want[i], "functions[{i}]");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_module_no_mixins_no_variables() {
        let arena = Bump::new();
        let m = selector_module(&arena);
        assert_eq!(m.mixins.len(), 0);
        assert_eq!(m.variables.len(), 0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_module_parse_emits_no_warnings() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &selector_parse_module_function(&arena),
            &[str_val(&arena, "c")],
        )
        .await;
        assert_inspect(&result.unwrap(), "(c,)");
        assert_no_warnings(&logger);
    }

    // --- parse ---

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_string() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_parse_module_function(&arena),
            &[str_val(&arena, "c d, e f")],
        )
        .await;
        assert_inspect(&got, "c d, e f");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_single() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_parse_module_function(&arena),
            &[str_val(&arena, "c")],
        )
        .await;
        assert_inspect(&got, "(c,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_quoted_string() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_parse_module_function(&arena),
            &[quoted_val(&arena, "c d")],
        )
        .await;
        assert_inspect(&got, "(c d,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_comma_list() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_parse_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c"), str_val(&arena, "d e")],
            )],
        )
        .await;
        assert_inspect(&got, "c, d e");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_space_list() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_parse_module_function(&arena),
            &[space_list(
                &arena,
                &[str_val(&arena, ".c"), str_val(&arena, ".d")],
            )],
        )
        .await;
        assert_inspect(&got, "(.c .d,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_nested_list() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_parse_module_function(&arena),
            &[comma_list(
                &arena,
                &[
                    space_list(&arena, &[str_val(&arena, "c"), str_val(&arena, "d")]),
                    str_val(&arena, "e"),
                ],
            )],
        )
        .await;
        assert_inspect(&got, "c d, e");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_argument_list() {
        let arena = Bump::new();
        // Matches Dart: SassArgumentList extends SassList, so an argument
        // list is a valid selector structure.
        let got = eval_ok(
            &arena,
            &selector_parse_module_function(&arena),
            &[arg_list(
                &arena,
                ListSeparator::Comma,
                &[str_val(&arena, "c"), str_val(&arena, "d")],
            )],
        )
        .await;
        assert_inspect(&got, "c, d");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_nested_argument_list() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_parse_module_function(&arena),
            &[comma_list(
                &arena,
                &[
                    arg_list(
                        &arena,
                        ListSeparator::Space,
                        &[str_val(&arena, "c"), str_val(&arena, "d")],
                    ),
                    str_val(&arena, "e"),
                ],
            )],
        )
        .await;
        assert_inspect(&got, "c d, e");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_structure() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_parse_module_function(&arena),
            &[str_val(&arena, "c d, e")],
        )
        .await;
        let ValueKind::List(outer) = &*got else {
            panic!("expected List, got {got:?}");
        };
        assert_eq!(outer.separator, ListSeparator::Comma);
        assert!(!outer.has_brackets);
        assert_eq!(outer.contents.len(), 2);
        let ValueKind::List(inner) = &*outer.contents[0] else {
            panic!("expected inner List, got {:?}", outer.contents[0]);
        };
        assert_eq!(inner.separator, ListSeparator::Space);
        assert_eq!(inner.contents.len(), 2);
        let ValueKind::String(s) = &*inner.contents[0] else {
            panic!("expected String, got {:?}", inner.contents[0]);
        };
        assert_eq!(s.text, "c");
        assert!(!s.has_quotes);
    }

    // --- parse errors (selector_string) ---

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_number_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[num_val(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_null_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[Value::new_with_arena(&arena, ValueKind::Null)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("null"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_boolean_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE))],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("true"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_map_error() {
        let arena = Bump::new();
        let m = map_val(&arena, &[(str_val(&arena, "c"), str_val(&arena, "d"))]);
        let err = eval(&arena, &selector_parse_module_function(&arena), &[m])
            .await
            .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("(c: d)"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_empty_list_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[comma_list(&arena, &[])],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("()"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_slash_list_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[slash_list(
                &arena,
                &[str_val(&arena, "c"), str_val(&arena, "d")],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("(c / d)"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_nested_comma_list_error() {
        let arena = Bump::new();
        // Matches sass-spec: core_functions/selector/parse/error inner_comma.
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[comma_list(
                &arena,
                &[comma_list(&arena, &[str_val(&arena, "c")])],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("((c,),)"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_too_nested_error() {
        let arena = Bump::new();
        // Matches sass-spec: core_functions/selector/parse/error too_nested.
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[comma_list(
                &arena,
                &[space_list(
                    &arena,
                    &[space_list(&arena, &[str_val(&arena, "c")])],
                )],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("(c,)"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_outer_space_error() {
        let arena = Bump::new();
        // Matches sass-spec: core_functions/selector/parse/error outer_space.
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[space_list(
                &arena,
                &[space_list(&arena, &[str_val(&arena, "c")])],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("(c)"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_comma_list_number_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c"), num_val(&arena, 1.0)],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("(c, 1)"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_space_list_number_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[space_list(
                &arena,
                &[str_val(&arena, "c"), num_val(&arena, 1.0)],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("(c 1)"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_nested_slash_list_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[comma_list(
                &arena,
                &[
                    str_val(&arena, "c"),
                    slash_list(&arena, &[str_val(&arena, "d"), str_val(&arena, "e")]),
                ],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("(c, d / e)"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_parent_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[str_val(&arena, "&")],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "Parent selectors aren't allowed here.",
            Some("selector"),
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_pseudo_parent_error() {
        let arena = Bump::new();
        // allow_parent=false also applies inside selector pseudos.
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[str_val(&arena, ":is(&)")],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "Parent selectors aren't allowed here.",
            Some("selector"),
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_invalid_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[str_val(&arena, "[c")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "expected more input.", Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_selector_parse_extra_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_parse_module_function(&arena),
            &[str_val(&arena, "c {")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "expected selector.", Some("selector"));
    }

    // --- is-superselector ---

    #[rust_sass_macros::maybe_test]
    async fn test_is_superselector_true() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &is_superselector_function(&arena),
            &[str_val(&arena, ".c"), str_val(&arena, ".c.d")],
        )
        .await;
        assert_is_true(&result.unwrap());
        assert_no_warnings(&logger);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_superselector_false() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &is_superselector_function(&arena),
            &[str_val(&arena, ".c.d"), str_val(&arena, ".c")],
        )
        .await;
        assert_is_false(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_superselector_equal() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &is_superselector_function(&arena),
            &[str_val(&arena, "c"), str_val(&arena, "c")],
        )
        .await;
        assert_is_true(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_superselector_list() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &is_superselector_function(&arena),
            &[str_val(&arena, "c, d"), str_val(&arena, "c")],
        )
        .await;
        assert_is_true(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_superselector_bogus_super() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &is_superselector_function(&arena),
            &[str_val(&arena, "> c"), str_val(&arena, "c")],
        )
        .await;
        assert_is_false(&result.unwrap());
        assert_single_warning(
            &logger,
            &bogus_warning_msg("$super: ", "> c"),
            "bogus-combinators",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_superselector_bogus_sub() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &is_superselector_function(&arena),
            &[str_val(&arena, "c"), str_val(&arena, "d + ~ c")],
        )
        .await;
        assert_is_false(&result.unwrap());
        assert_single_warning(
            &logger,
            &bogus_warning_msg("$sub: ", "d + ~ c"),
            "bogus-combinators",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_superselector_super_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &is_superselector_function(&arena),
            &[num_val(&arena, 1.0), str_val(&arena, "c")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), Some("super"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_superselector_sub_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &is_superselector_function(&arena),
            &[str_val(&arena, "c"), num_val(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), Some("sub"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_superselector_super_parse_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &is_superselector_function(&arena),
            &[str_val(&arena, "[c"), str_val(&arena, "d")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "expected more input.", Some("super"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_superselector_sub_parent_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &is_superselector_function(&arena),
            &[str_val(&arena, "c"), str_val(&arena, "&")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Parent selectors aren't allowed here.", Some("sub"));
    }

    // --- simple-selectors ---

    #[rust_sass_macros::maybe_test]
    async fn test_simple_selectors() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &simple_selectors_function(&arena),
            &[str_val(&arena, ".foo.bar")],
        )
        .await;
        assert_inspect(&result.unwrap(), ".foo, .bar");
        assert_no_warnings(&logger);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_simple_selectors_triple() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &simple_selectors_function(&arena),
            &[str_val(&arena, ".foo.bar.baz")],
        )
        .await;
        assert_inspect(&got, ".foo, .bar, .baz");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_simple_selectors_pseudo() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &simple_selectors_function(&arena),
            &[str_val(&arena, "c:hover")],
        )
        .await;
        assert_inspect(&got, "c, :hover");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_simple_selectors_single() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &simple_selectors_function(&arena),
            &[str_val(&arena, "c")],
        )
        .await;
        assert_inspect(&got, "(c,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_simple_selectors_separator() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &simple_selectors_function(&arena),
            &[str_val(&arena, ".foo.bar")],
        )
        .await;
        assert_eq!(got.separator(), ListSeparator::Comma);
        assert!(!got.has_brackets());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_simple_selectors_multiple_complex_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &simple_selectors_function(&arena),
            &[str_val(&arena, "c, d")],
        )
        .await
        .unwrap_err();
        // Dart `CompoundSelector.parse` (value.dart:516): `expected selector.`
        assert_script_err(err, "expected selector.", Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_simple_selectors_combinator_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &simple_selectors_function(&arena),
            &[str_val(&arena, "c > d")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "expected selector.", Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_simple_selectors_descendant_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &simple_selectors_function(&arena),
            &[str_val(&arena, "c d")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "expected selector.", Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_simple_selectors_parse_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &simple_selectors_function(&arena),
            &[str_val(&arena, "[c")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "expected more input.", Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_simple_selectors_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &simple_selectors_function(&arena),
            &[num_val(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_simple_selectors_parent_error() {
        let arena = Bump::new();
        // assert_compound_selector always disallows parent selectors.
        let err = eval(
            &arena,
            &simple_selectors_function(&arena),
            &[str_val(&arena, "&.c")],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "Parent selectors aren't allowed here.",
            Some("selector"),
        );
    }

    // --- nest ---

    #[rust_sass_macros::maybe_test]
    async fn test_nest_single() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(&arena, &[str_val(&arena, "c")])],
        )
        .await;
        assert_inspect(&result.unwrap(), "(c,)");
        assert_no_warnings(&logger);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_many() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(
                &arena,
                &[
                    str_val(&arena, "c"),
                    str_val(&arena, "d"),
                    str_val(&arena, "e"),
                    str_val(&arena, "f"),
                    str_val(&arena, "g"),
                ],
            )],
        )
        .await;
        assert_inspect(&got, "(c d e f g,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_list() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c, d"), str_val(&arena, "e")],
            )],
        )
        .await;
        assert_inspect(&got, "c e, d e");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_parent_alone() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(&arena, &[str_val(&arena, "&")])],
        )
        .await;
        assert_inspect(&got, "(&,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_parent_alone_second() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c"), str_val(&arena, "&")],
            )],
        )
        .await;
        assert_inspect(&got, "(c,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_parent_compound() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c"), str_val(&arena, "&.d")],
            )],
        )
        .await;
        assert_inspect(&got, "(c.d,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_parent_suffix() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c"), str_val(&arena, "&d")],
            )],
        )
        .await;
        assert_inspect(&got, "(cd,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_complex_parent() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c d"), str_val(&arena, "e &.f")],
            )],
        )
        .await;
        assert_inspect(&got, "(e c d.f,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_selector_pseudo() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c"), str_val(&arena, ":is(&)")],
            )],
        )
        .await;
        assert_inspect(&got, "(:is(c),)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_selector_pseudo_alone() {
        let arena = Bump::new();
        // A parent selector inside a selector pseudo survives a nil parent.
        let got = eval_ok(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(&arena, &[str_val(&arena, ":is(&)")])],
        )
        .await;
        assert_inspect(&got, "(:is(&),)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_selector_pseudo_suffix_error() {
        let arena = Bump::new();
        // The parent-with-suffix check recurses into selector pseudos.
        let err = eval(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(&arena, &[str_val(&arena, ":is(&c)")])],
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Sass { message, .. } => assert_eq!(
                message,
                "A top-level selector may not contain a parent selector with a suffix."
            ),
            other => panic!("expected Sass error, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_selector_pseudo_non_initial_simple() {
        let arena = Bump::new();
        // The pseudo is not the first simple in its compound; the
        // parser-level allow_parent still applies to the pseudo's inner
        // selector list.
        let got = eval_ok(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c"), str_val(&arena, "d:is(&)")],
            )],
        )
        .await;
        assert_inspect(&got, "(d:is(c),)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_leading_combinator() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "> c"), str_val(&arena, "d")],
            )],
        )
        .await;
        assert_inspect(&got, "(> c d,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_trailing_combinator() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c ~"), str_val(&arena, "d")],
            )],
        )
        .await;
        assert_inspect(&got, "(c ~ d,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_empty_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(&arena, &[])],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "$selectors: At least one selector must be passed.",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_suffix_parent_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(&arena, &[str_val(&arena, "&c")])],
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Sass { message, .. } => assert_eq!(
                message,
                "A top-level selector may not contain a parent selector with a suffix."
            ),
            other => panic!("expected Sass error, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_non_initial_parent_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c"), str_val(&arena, "[d]&")],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "\"&\" may only used at the beginning of a compound selector.",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(&arena, &[num_val(&arena, 1.0)])],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_type_error_later() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c"), num_val(&arena, 1.0)],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nest_invalid_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_nest_module_function(&arena),
            &[comma_list(&arena, &[str_val(&arena, "[c")])],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "expected more input.", None);
    }

    // --- append ---

    #[rust_sass_macros::maybe_test]
    async fn test_append_classes() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, ".c"), str_val(&arena, ".d")],
            )],
        )
        .await;
        assert_inspect(&result.unwrap(), "(.c.d,)");
        assert_no_warnings(&logger);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_classes_double() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, ".c, .d"), str_val(&arena, ".e, .f")],
            )],
        )
        .await;
        assert_inspect(&got, ".c.e, .c.f, .d.e, .d.f");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_suffix() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, ".c"), str_val(&arena, "d")],
            )],
        )
        .await;
        assert_inspect(&got, "(.cd,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_suffix_multiple() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, ".c, .d"), str_val(&arena, "e, f")],
            )],
        )
        .await;
        assert_inspect(&got, ".ce, .cf, .de, .df");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_descendant() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c d"), str_val(&arena, "e f")],
            )],
        )
        .await;
        assert_inspect(&got, "(c de f,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_one_arg() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(&arena, &[str_val(&arena, ".c.d")])],
        )
        .await;
        assert_inspect(&got, "(.c.d,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_many_args() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[
                    str_val(&arena, ".c"),
                    str_val(&arena, ".d"),
                    str_val(&arena, ".e"),
                ],
            )],
        )
        .await;
        assert_inspect(&got, "(.c.d.e,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_list_format() {
        let arena = Bump::new();
        // Matches sass-spec: format/input/initial — parsed-format input.
        let got = eval_ok(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[
                    comma_list(&arena, &[str_val(&arena, "c"), str_val(&arena, "d e")]),
                    str_val(&arena, "f"),
                ],
            )],
        )
        .await;
        assert_inspect(&got, "cf, d ef");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_leading_combinator_parent() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "> c"), str_val(&arena, "d")],
            )],
        )
        .await;
        assert_inspect(&got, "(> cd,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_trailing_combinator_child() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c"), str_val(&arena, "d ~")],
            )],
        )
        .await;
        assert_inspect(&got, "(cd ~,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_middle_combinators() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c > > d"), str_val(&arena, "e")],
            )],
        )
        .await;
        assert_inspect(&got, "(c > > de,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_universal_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, ".c"), str_val(&arena, "*")],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Can't append * to .c.", None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_leading_combinator_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, ".c"), str_val(&arena, "> .d")],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Can't append > .d to .c.", None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_combinator_only_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[
                    str_val(&arena, ".c"),
                    str_val(&arena, ">"),
                    str_val(&arena, ".d"),
                ],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Can't append > to .c.", None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_namespace_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "c"), str_val(&arena, "|d")],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Can't append |d to c.", None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_multiple_targets_error() {
        let arena = Bump::new();
        // The result string in the error lists all components of the
        // accumulated selector.
        let err = eval(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, ".c, .d"), str_val(&arena, "*")],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Can't append * to .c, .d.", None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_trailing_combinator_parent_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, ".c ~"), str_val(&arena, ".d")],
            )],
        )
        .await
        .unwrap_err();
        match *err {
            SassError::MultiSpan {
                message,
                primary_label,
                trace: _,
                ..
            } => {
                assert_eq!(
                    message,
                    "Selector \".c ~\" can't be used as a parent in a compound selector."
                );
                assert_eq!(primary_label.as_deref(), Some("outer selector"));
            }
            other => panic!("expected MultiSpan error, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_empty_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(&arena, &[])],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "$selectors: At least one selector must be passed.",
            None,
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(&arena, &[num_val(&arena, 1.0)])],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_parent_error() {
        let arena = Bump::new();
        // append parses with allow_parent=false.
        let err = eval(
            &arena,
            &selector_append_module_function(&arena),
            &[comma_list(
                &arena,
                &[str_val(&arena, "&"), str_val(&arena, "c")],
            )],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Parent selectors aren't allowed here.", None);
    }

    // --- extend ---

    #[rust_sass_macros::maybe_test]
    async fn test_extend_equal() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &selector_extend_module_function(&arena),
            &[
                str_val(&arena, "c"),
                str_val(&arena, "c"),
                str_val(&arena, "e"),
            ],
        )
        .await;
        assert_inspect(&result.unwrap(), "c, e");
        assert_no_warnings(&logger);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_extend_unequal() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_extend_module_function(&arena),
            &[
                str_val(&arena, "c"),
                str_val(&arena, "d"),
                str_val(&arena, "e"),
            ],
        )
        .await;
        assert_inspect(&got, "(c,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_extend_compound() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_extend_module_function(&arena),
            &[
                str_val(&arena, ".c.d"),
                str_val(&arena, ".c"),
                str_val(&arena, ".e"),
            ],
        )
        .await;
        assert_inspect(&got, ".c.d, .d.e");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_extend_parent() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_extend_module_function(&arena),
            &[
                str_val(&arena, ".c .d"),
                str_val(&arena, ".c"),
                str_val(&arena, ".e"),
            ],
        )
        .await;
        assert_inspect(&got, ".c .d, .e .d");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_extend_list_extender() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_extend_module_function(&arena),
            &[
                str_val(&arena, ".c .d"),
                str_val(&arena, ".c"),
                str_val(&arena, ".e, .f"),
            ],
        )
        .await;
        assert_inspect(&got, ".c .d, .e .d, .f .d");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_extend_bogus_selector_warning() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &selector_extend_module_function(&arena),
            &[
                str_val(&arena, "> .c"),
                str_val(&arena, ".c"),
                str_val(&arena, ".d"),
            ],
        )
        .await;
        assert_inspect(&result.unwrap(), "> .c, > .d");
        assert_single_warning(
            &logger,
            &bogus_warning_msg("$selector: ", "> .c"),
            "bogus-combinators",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_extend_selector_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_extend_module_function(&arena),
            &[
                num_val(&arena, 1.0),
                str_val(&arena, "c"),
                str_val(&arena, "d"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_extend_extendee_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_extend_module_function(&arena),
            &[
                str_val(&arena, "c"),
                num_val(&arena, 1.0),
                str_val(&arena, "d"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), Some("extendee"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_extend_extender_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_extend_module_function(&arena),
            &[
                str_val(&arena, "c"),
                str_val(&arena, "d"),
                num_val(&arena, 1.0),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), Some("extender"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_extend_parent_selector_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_extend_module_function(&arena),
            &[
                str_val(&arena, "&"),
                str_val(&arena, "c"),
                str_val(&arena, "d"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "Parent selectors aren't allowed here.",
            Some("selector"),
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_extend_extender_parse_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_extend_module_function(&arena),
            &[
                str_val(&arena, "c"),
                str_val(&arena, "d"),
                str_val(&arena, "[e"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "expected more input.", Some("extender"));
    }

    // --- replace ---

    #[rust_sass_macros::maybe_test]
    async fn test_replace_simple() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &selector_replace_module_function(&arena),
            &[
                str_val(&arena, "c"),
                str_val(&arena, "c"),
                str_val(&arena, "d"),
            ],
        )
        .await;
        assert_inspect(&result.unwrap(), "(d,)");
        assert_no_warnings(&logger);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_replace_compound() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_replace_module_function(&arena),
            &[
                str_val(&arena, "c.d"),
                str_val(&arena, "c"),
                str_val(&arena, "e"),
            ],
        )
        .await;
        assert_inspect(&got, "(e.d,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_replace_complex() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_replace_module_function(&arena),
            &[
                str_val(&arena, "c d"),
                str_val(&arena, "d"),
                str_val(&arena, "e f"),
            ],
        )
        .await;
        assert_inspect(&got, "c e f, e c f");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_replace_selector_pseudo() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_replace_module_function(&arena),
            &[
                str_val(&arena, ":is(c)"),
                str_val(&arena, "c"),
                str_val(&arena, "d"),
            ],
        )
        .await;
        assert_inspect(&got, "(:is(d),)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_replace_no_op() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_replace_module_function(&arena),
            &[
                str_val(&arena, "c"),
                str_val(&arena, "d"),
                str_val(&arena, "e"),
            ],
        )
        .await;
        assert_inspect(&got, "(c,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_replace_selector_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_replace_module_function(&arena),
            &[
                num_val(&arena, 1.0),
                str_val(&arena, "c"),
                str_val(&arena, "d"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), Some("selector"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_replace_original_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_replace_module_function(&arena),
            &[
                str_val(&arena, "c"),
                num_val(&arena, 1.0),
                str_val(&arena, "d"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), Some("original"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_replace_replacement_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_replace_module_function(&arena),
            &[
                str_val(&arena, "c"),
                str_val(&arena, "d"),
                num_val(&arena, 1.0),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), Some("replacement"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_replace_parent_selector_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_replace_module_function(&arena),
            &[
                str_val(&arena, "&"),
                str_val(&arena, "c"),
                str_val(&arena, "d"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "Parent selectors aren't allowed here.",
            Some("selector"),
        );
    }

    // --- unify ---

    #[rust_sass_macros::maybe_test]
    async fn test_unify_same() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &selector_unify_module_function(&arena),
            &[str_val(&arena, ".c"), str_val(&arena, ".c")],
        )
        .await;
        assert_inspect(&result.unwrap(), "(.c,)");
        assert_no_warnings(&logger);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_unify_different_classes() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_unify_module_function(&arena),
            &[str_val(&arena, ".c"), str_val(&arena, ".d")],
        )
        .await;
        assert_inspect(&got, "(.c.d,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_unify_ids_null() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_unify_module_function(&arena),
            &[str_val(&arena, "#c"), str_val(&arena, "#d")],
        )
        .await;
        assert!(matches!(&*got, ValueKind::Null), "want Null, got {got:?}");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_unify_descendant_null() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_unify_module_function(&arena),
            &[str_val(&arena, "c d"), str_val(&arena, "e f")],
        )
        .await;
        assert!(matches!(&*got, ValueKind::Null), "want Null, got {got:?}");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_unify_descendant_same() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &selector_unify_module_function(&arena),
            &[str_val(&arena, "c d"), str_val(&arena, "c d")],
        )
        .await;
        assert_inspect(&got, "(c d,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_unify_bogus_selector1_warning() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &selector_unify_module_function(&arena),
            &[str_val(&arena, "c ~"), str_val(&arena, "c")],
        )
        .await;
        assert_inspect(&result.unwrap(), "(c ~,)");
        assert_single_warning(
            &logger,
            &bogus_warning_msg("$selector1: ", "c ~"),
            "bogus-combinators",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_unify_selector1_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_unify_module_function(&arena),
            &[num_val(&arena, 1.0), str_val(&arena, "c")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), Some("selector1"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_unify_selector2_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_unify_module_function(&arena),
            &[str_val(&arena, "c"), num_val(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, &not_valid_selector_msg("1"), Some("selector2"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_unify_selector1_parent_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &selector_unify_module_function(&arena),
            &[str_val(&arena, "&"), str_val(&arena, "c")],
        )
        .await
        .unwrap_err();
        assert_script_err(
            err,
            "Parent selectors aren't allowed here.",
            Some("selector1"),
        );
    }

    // Dart parses LAZILY during the fold (selector.dart:67-90): an early
    // append failure fires before a later parse error. Append takes
    // `$selectors...`, so the harness must wrap the values in an argument
    // list (the `arg_list` helper).
    #[rust_sass_macros::maybe_test]
    async fn test_append_error_precedence() {
        let arena = Bump::new();
        let rest = arg_list(
            &arena,
            ListSeparator::Comma,
            &[
                str_val(&arena, ".a"),
                str_val(&arena, "> .b"),
                str_val(&arena, "[c"),
            ],
        );
        let err = eval(&arena, &selector_append_module_function(&arena), &[rest])
            .await
            .unwrap_err();
        assert_script_err(err, "Can't append > .b to .a.", None);
    }
}
