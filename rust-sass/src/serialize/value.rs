// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

//! Value serialization: one `visit_*` per [`ValueKind`] plus the inspect-mode
//! error surface (Dart's `visitBoolean`…`visitMixin`).
//!
//! Thin `ValueVisitor` wrappers delegate to free `visit_*_impl` functions so
//! [`visit_value_impl`] can dispatch by `match` where `accept()` is
//! unavailable (inside buffer/state-only callbacks).

// dart-source: lib/src/visitor/serialize.dart (visitBoolean, visitCalculation, visitColor, visitFunction, visitMixin, visitList, _separatorString, _elementNeedsParens, visitMap, _writeMapElement, visitNull, visitNumber, visitString)
// go-source: go/value/visitor_value.go

use crate::common::SassError;
use crate::serialize::calc::write_calculation_value;
use crate::serialize::color::visit_color_impl;
use crate::serialize::list::element_needs_parens;
use crate::serialize::list::list_sep_str;
use crate::serialize::list::write_map_element;
use crate::serialize::string::visit_quoted_string;
use crate::serialize::string::visit_unquoted_string;
use crate::value::CalcArgument;
use crate::value::ListSeparator;
use std::fmt::Write;

use crate::common::SassResult;
use crate::source_map_buffer::SourceMapBuffer;
use crate::value::boolean::SassBoolean;
use crate::value::{
    ListValue, SassCalculation, SassColor, SassFunction, SassMap, SassMixin, SassNumber,
    SassString, Value, ValueKind, ValueVisitor,
};

use crate::serialize::{write_between, SerializeState, SerializeVisitor};

impl<'parse> ValueVisitor<'parse> for SerializeVisitor<'parse> {
    type Output = ();

    fn visit_boolean(&mut self, value: &SassBoolean) -> SassResult<()> {
        visit_boolean_impl(&mut self.buffer, value)
    }
    fn visit_null(&mut self) -> SassResult<()> {
        visit_null_impl(&mut self.buffer, &self.inner)
    }
    fn visit_string(&mut self, value: &SassString<'parse>) -> SassResult<()> {
        visit_string_impl(&mut self.buffer, &self.inner, value)
    }
    fn visit_number(&mut self, value: &SassNumber) -> SassResult<()> {
        visit_number_impl(&mut self.buffer, &self.inner, value)
    }
    fn visit_color(&mut self, value: &SassColor) -> SassResult<()> {
        visit_color_impl(&mut self.buffer, &self.inner, value)
    }
    fn visit_calculation(&mut self, value: &SassCalculation) -> SassResult<()> {
        visit_calculation_impl(&mut self.buffer, &self.inner, value)
    }
    fn visit_list(&mut self, value: &ListValue<'_, 'parse>) -> SassResult<()> {
        visit_list_value_impl(&mut self.buffer, &self.inner, value)
    }
    fn visit_map(&mut self, value: &SassMap<'parse>) -> SassResult<()> {
        visit_map_impl(&mut self.buffer, &self.inner, value)
    }
    fn visit_function(&mut self, value: &SassFunction<'parse>) -> SassResult<()> {
        visit_function_impl(&mut self.buffer, &self.inner, value)
    }
    fn visit_mixin(&mut self, value: &SassMixin<'parse>) -> SassResult<()> {
        visit_mixin_impl(&mut self.buffer, &self.inner, value)
    }
}

/// Writes `true`/`false` (Dart's `visitBoolean`).
pub(crate) fn visit_boolean_impl(
    buf: &mut SourceMapBuffer<'_>,
    value: &SassBoolean,
) -> SassResult<()> {
    if value.value {
        write!(buf, "true").unwrap();
    } else {
        write!(buf, "false").unwrap();
    }
    Ok(())
}

/// Writes `null` in inspect mode, nothing otherwise (Dart's `visitNull`).
pub(crate) fn visit_null_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
) -> SassResult<()> {
    if state.inspect {
        write!(buf, "null").unwrap();
    }
    Ok(())
}

/// Writes a quoted or raw string per `state.quote` (Dart's `visitString`).
pub(crate) fn visit_string_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    value: &SassString<'_>,
) -> SassResult<()> {
    if state.quote && value.has_quotes {
        visit_quoted_string(buf, state, value.text);
    } else {
        visit_unquoted_string(buf, state, value.text);
    }
    Ok(())
}

/// Writes a number: `a/b` pairs recurse around a literal `/`; non-finite and
/// complex-unit values go through `calc()`; otherwise number plus first
/// numerator unit (Dart's `visitNumber`).
pub(crate) fn visit_number_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    value: &SassNumber,
) -> SassResult<()> {
    if let Some(slash) = &value.as_slash {
        visit_number_impl(buf, state, &slash.0)?;
        buf.write_char('/').unwrap();
        visit_number_impl(buf, state, &slash.1)?;
        return Ok(());
    }
    if value.value.is_infinite() || value.value.is_nan() {
        let calc =
            SassCalculation::new_unsimplified("calc", vec![CalcArgument::Number(value.clone())]);
        return visit_calculation_impl(buf, state, &calc);
    }
    if value.has_complex_units() {
        let calc =
            SassCalculation::new_unsimplified("calc", vec![CalcArgument::Number(value.clone())]);
        return visit_calculation_impl(buf, state, &calc);
    }
    state.write_number(buf, value.value);
    if let Some(first) = value.numerator_units.first() {
        write!(buf, "{}", first).unwrap();
    }
    Ok(())
}

/// Writes `name(args)` with comma separators (Dart's `visitCalculation`).
pub(crate) fn visit_calculation_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    value: &SassCalculation,
) -> SassResult<()> {
    let sep = state.comma_sep();
    write!(buf, "{}", value.name).unwrap();
    buf.write_char('(').unwrap();
    write_between(buf, &value.arguments, sep, |buf, arg| {
        write_calculation_value(buf, state, arg)
    })?;
    buf.write_char(')').unwrap();
    Ok(())
}

/// Unwraps plain vs argument lists onto the shared `visit_list_impl`
/// (list-shape logic lives in `list.rs`).
pub(crate) fn visit_list_value_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    value: &ListValue<'_, '_>,
) -> SassResult<()> {
    match value {
        ListValue::List(l) => visit_list_impl(buf, state, l.as_list(), l.separator, l.has_brackets),
        ListValue::ArgumentList(a) => {
            visit_list_impl(buf, state, a.as_list(), a.list.separator, a.has_brackets())
        }
    }
}

/// Writes `(k: v, ...)` in inspect mode; otherwise errors with the
/// inspected map quoted (`<map> isn't a valid CSS value.`, Dart's `visitMap`).
pub(crate) fn visit_map_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    value: &SassMap<'_>,
) -> SassResult<()> {
    if !state.inspect {
        return Err(Box::new(SassError::Script {
            message: {
                let mut visitor = SerializeVisitor::new_plain(true, true);
                match visitor.visit_map(value) {
                    Ok(_) => format!("{} isn't a valid CSS value.", visitor.into_string()),
                    Err(_) => "(map) isn't a valid CSS value.".into(),
                }
            },
            argument_name: None,
        }));
    }
    buf.write_char('(').unwrap();
    let mut first = true;
    for (k, v) in &value.entries {
        if first {
            first = false;
        } else {
            write!(buf, ", ").unwrap();
        }
        write_map_element(buf, state, k)?;
        write!(buf, ": ").unwrap();
        write_map_element(buf, state, v)?;
    }
    buf.write_char(')').unwrap();
    Ok(())
}

/// Writes `get-function(name)` in inspect mode; otherwise errors with the
/// inspected function quoted (Dart's `visitFunction`).
pub(crate) fn visit_function_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    value: &SassFunction<'_>,
) -> SassResult<()> {
    if !state.inspect {
        return Err(Box::new(SassError::Script {
            message: {
                let mut visitor = SerializeVisitor::new_plain(true, true);
                match visitor.visit_function(value) {
                    Ok(_) => format!("{} isn't a valid CSS value.", visitor.into_string()),
                    Err(_) => "(function) isn't a valid CSS value.".into(),
                }
            },
            argument_name: None,
        }));
    }
    write!(buf, "get-function(").unwrap();
    visit_quoted_string(buf, state, value.callable.name());
    buf.write_char(')').unwrap();
    Ok(())
}

/// Writes `get-mixin(name)` in inspect mode; otherwise errors with the
/// inspected mixin quoted (Dart's `visitMixin`).
pub(crate) fn visit_mixin_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    value: &SassMixin<'_>,
) -> SassResult<()> {
    if !state.inspect {
        return Err(Box::new(SassError::Script {
            message: {
                let mut visitor = SerializeVisitor::new_plain(true, true);
                match visitor.visit_mixin(value) {
                    Ok(_) => format!("{} isn't a valid CSS value.", visitor.into_string()),
                    Err(_) => "(mixin) isn't a valid CSS value.".into(),
                }
            },
            argument_name: None,
        }));
    }
    write!(buf, "get-mixin(").unwrap();
    visit_quoted_string(buf, state, value.callable.name());
    buf.write_char(')').unwrap();
    Ok(())
}

/// Manual `match` dispatch over [`ValueKind`] for buffer/state-only contexts
/// (Dart dispatches via `value.accept`; see the module docs).
pub(crate) fn visit_value_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    value: &Value<'_>,
) -> SassResult<()> {
    match &**value {
        ValueKind::Boolean(b) => visit_boolean_impl(buf, b),
        ValueKind::Null => visit_null_impl(buf, state),
        ValueKind::String(s) => visit_string_impl(buf, state, s),
        ValueKind::Number(n) => visit_number_impl(buf, state, n),
        ValueKind::Color(c) => visit_color_impl(buf, state, c),
        ValueKind::List(l) => visit_list_impl(buf, state, l.as_list(), l.separator, l.has_brackets),
        ValueKind::ArgumentList(a) => {
            visit_list_impl(buf, state, a.as_list(), a.list.separator, a.has_brackets())
        }
        ValueKind::Map(m) => visit_map_impl(buf, state, m),
        ValueKind::Calculation(c) => visit_calculation_impl(buf, state, c),
        ValueKind::Function(f) => visit_function_impl(buf, state, f),
        ValueKind::Mixin(m) => visit_mixin_impl(buf, state, m),
    }
}

// Brackets, the inspect-only `()` spelling, singleton trailing separators,
// blank-element filtering in CSS mode, and inspect-mode disambiguation
// parens (Dart's `visitList`).
fn visit_list_impl(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    contents: &[Value<'_>],
    separator: ListSeparator,
    has_brackets: bool,
) -> SassResult<()> {
    if has_brackets {
        buf.write_char('[').unwrap();
    } else if contents.is_empty() {
        if !state.inspect {
            return Err(Box::new(SassError::Script {
                message: "() isn't a valid CSS value.".into(),
                argument_name: None,
            }));
        }
        write!(buf, "()").unwrap();
        return Ok(());
    }

    let singleton = state.inspect
        && contents.len() == 1
        && (separator == ListSeparator::Comma || separator == ListSeparator::Slash);
    if singleton && !has_brackets {
        buf.write_char('(').unwrap();
    }

    let elems: Vec<&Value<'_>> = if state.inspect {
        contents.iter().collect()
    } else {
        contents.iter().filter(|e| !e.is_blank()).collect()
    };

    let sep = list_sep_str(state, separator);
    for (i, elem) in elems.iter().enumerate() {
        if i > 0 {
            write!(buf, "{}", sep).unwrap();
        }
        if state.inspect && element_needs_parens(separator, elem) {
            buf.write_char('(').unwrap();
        }
        visit_value_impl(buf, state, elem)?;
        if state.inspect && element_needs_parens(separator, elem) {
            buf.write_char(')').unwrap();
        }
    }

    if singleton {
        match separator {
            ListSeparator::Comma => write!(buf, ",").unwrap(),
            ListSeparator::Slash => write!(buf, "/").unwrap(),
            _ => {}
        }
        if !has_brackets {
            buf.write_char(')').unwrap();
        }
    }
    if has_brackets {
        buf.write_char(']').unwrap();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_map_buffer::SourceMapBuffer;
    use crate::value::boolean::SASS_TRUE;

    #[test]
    fn test_visit_boolean_true() {
        let mut buf = SourceMapBuffer::new_plain();
        visit_boolean_impl(&mut buf, &SASS_TRUE).unwrap();
        assert_eq!(buf.into_string(), "true");
    }
}
