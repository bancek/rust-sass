// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

//! List/map-element separators and paren rules (Dart's `_separatorString`,
//! `_elementNeedsParens`, `_writeMapElement`).
//!
//! The `visitList` body itself lives in `value.rs`; this file holds the
//! helpers both writers share.

// dart-source: lib/src/visitor/serialize.dart (_separatorString, _elementNeedsParens, _writeMapElement)
// go-source: go/value/visitor_list.go

use crate::common::SassResult;
use crate::serialize::value::visit_value_impl;
use crate::source_map_buffer::SourceMapBuffer;
use crate::value::{ListSeparator, Value, ValueKind};

use crate::serialize::{OutputStyle, SerializeState};

/// Separator text for `sep`: comma follows the output style, slash is tight
/// in compressed mode, space is a single blank. Undecided never reaches
/// output with more than one element, so it maps to `""` (Dart's
/// `_separatorString`).
pub(crate) fn list_sep_str(state: &SerializeState, sep: ListSeparator) -> &'static str {
    match sep {
        ListSeparator::Comma => state.comma_sep(),
        ListSeparator::Slash => match state.style {
            OutputStyle::Compressed => "/",
            OutputStyle::Expanded | OutputStyle::Nested | OutputStyle::Compact => " / ",
        },
        ListSeparator::Space => " ",
        _ => "",
    }
}

/// Whether a nested unbracketed multi-element list needs parens inside an
/// outer list with `separator` (Dart's `_elementNeedsParens`).
pub(crate) fn element_needs_parens(separator: ListSeparator, value: &Value<'_>) -> bool {
    match &**value {
        ValueKind::List(l) => {
            if l.contents.len() <= 1 || l.has_brackets {
                return false;
            }
            match separator {
                ListSeparator::Comma => l.separator == ListSeparator::Comma,
                ListSeparator::Slash => {
                    l.separator == ListSeparator::Comma || l.separator == ListSeparator::Slash
                }
                _ => l.separator != ListSeparator::Undecided,
            }
        }
        ValueKind::ArgumentList(a) => {
            if a.list.contents.len() <= 1 || a.list.has_brackets {
                return false;
            }
            match separator {
                ListSeparator::Comma => a.list.separator == ListSeparator::Comma,
                ListSeparator::Slash => {
                    a.list.separator == ListSeparator::Comma
                        || a.list.separator == ListSeparator::Slash
                }
                _ => a.list.separator != ListSeparator::Undecided,
            }
        }
        _ => false,
    }
}

/// Writes a map key/value, parenthesizing unbracketed comma lists
/// (Dart's `_writeMapElement`).
pub(crate) fn write_map_element(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    value: &Value<'_>,
) -> SassResult<()> {
    let needs_parens = match &**value {
        ValueKind::List(l) => l.separator == ListSeparator::Comma && !l.has_brackets,
        ValueKind::ArgumentList(a) => {
            a.list.separator == ListSeparator::Comma && !a.list.has_brackets
        }
        _ => false,
    };
    if needs_parens {
        buf.write_char('(').unwrap();
    }
    visit_value_impl(buf, state, value)?;
    if needs_parens {
        buf.write_char(')').unwrap();
    }
    Ok(())
}
