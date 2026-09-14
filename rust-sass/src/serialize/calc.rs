// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

//! Calculation argument emission: numbers, operations, and units
//! (Dart's `_writeCalculationValue`, `_writeCalculationUnits`,
//! `_parenthesizeCalculationRhs`).
//!
//! Plain values recurse through `accept()`; operations parenthesize by
//! precedence (division always parenthesizes its right side).

// dart-source: lib/src/visitor/serialize.dart (_writeCalculationValue, _writeCalculationUnits, _parenthesizeCalculationRhs)
// go-source: go/value/visitor_calc.go

use crate::serialize::value::visit_calculation_impl;
use crate::value::CalculationOperator;
use std::fmt::Write;

use crate::common::SassResult;
use crate::source_map_buffer::SourceMapBuffer;
use crate::value::calculation::CalcArgument;

use crate::serialize::{OutputStyle, SerializeState};

/// Writes one calculation argument: infinities by name, complex units with
/// the first unit inline and the rest as `* 1unit`/`/ 1unit`, operations
/// with precedence parens (Dart's `_writeCalculationValue`).
pub(crate) fn write_calculation_value(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    arg: &CalcArgument,
) -> SassResult<()> {
    match arg {
        CalcArgument::Number(n) => {
            if n.value.is_infinite() || n.value.is_nan() {
                if n.value.is_infinite() && n.value.is_sign_positive() {
                    write!(buf, "infinity").unwrap();
                } else if n.value.is_infinite() {
                    write!(buf, "-infinity").unwrap();
                } else {
                    write!(buf, "NaN").unwrap();
                }
                write_calc_units(buf, state, &n.numerator_units, &n.denominator_units, false);
            } else if n.has_complex_units() {
                state.write_number(buf, n.value);
                if let Some((first, rest)) = n.numerator_units.split_first() {
                    write!(buf, "{}", first).unwrap();
                    write_calc_units(buf, state, rest, &n.denominator_units, false);
                } else {
                    write_calc_units(buf, state, &[], &n.denominator_units, false);
                }
            } else {
                state.write_number(buf, n.value);
                if let Some(first) = n.numerator_units.first() {
                    write!(buf, "{}", first).unwrap();
                }
            }
        }
        CalcArgument::Operation(op) => {
            let prec = op.operator.precedence();
            let left_parens =
                matches!(&op.left, CalcArgument::Operation(o) if o.operator.precedence() < prec);
            if left_parens {
                buf.write_char('(').unwrap();
            }
            write_calculation_value(buf, state, &op.left)?;
            if left_parens {
                buf.write_char(')').unwrap();
            }
            let ws = matches!(
                state.style,
                OutputStyle::Expanded | OutputStyle::Nested | OutputStyle::Compact
            ) || prec == 1;
            if ws {
                buf.write_char(' ').unwrap();
            }
            write!(buf, "{}", op.operator.operator_str()).unwrap();
            if ws {
                buf.write_char(' ').unwrap();
            }
            let right_parens = parenthesize_calc_rhs(op.operator, &op.right);
            if right_parens {
                buf.write_char('(').unwrap();
            }
            write_calculation_value(buf, state, &op.right)?;
            if right_parens {
                buf.write_char(')').unwrap();
            }
        }
        CalcArgument::String(s, _) | CalcArgument::Interpolation(s) => {
            write!(buf, "{}", s).unwrap();
        }
        CalcArgument::Calculation(c) => {
            visit_calculation_impl(buf, state, c)?;
        }
    }
    Ok(())
}

/// Writes complex numerator/denominator units beyond the first numerator
/// unit as `* 1<unit>` / `/ 1<unit>` (Dart's `_writeCalculationUnits`).
///
/// `negative` prefixes the next unit expression with a minus sign (Dart's
/// `negative` parameter; no caller sets it yet — kept for parity).
pub(crate) fn write_calc_units(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    num_units: &[String],
    den_units: &[String],
    negative: bool,
) {
    let mut negative = negative;
    for unit in num_units {
        state.write_optional_space(buf);
        buf.write_char('*').unwrap();
        state.write_optional_space(buf);
        if negative {
            buf.write_char('-').unwrap();
            negative = false;
        }
        write!(buf, "1{}", unit).unwrap();
    }
    for unit in den_units {
        state.write_optional_space(buf);
        buf.write_char('/').unwrap();
        state.write_optional_space(buf);
        if negative {
            buf.write_char('-').unwrap();
            negative = false;
        }
        write!(buf, "1{}", unit).unwrap();
    }
}

// In `a ? (b # c)`, `outer` is `?` and `right` is `#`: division always
// parenthesizes, plus never does, others parenthesize `+`/`-` (Dart's
// `_parenthesizeCalculationRhs`). The number case lives inline in
// `write_calculation_value` (finite → complex units only; infinite/NaN →
// any units).
fn parenthesize_calc_rhs(outer: CalculationOperator, right: &CalcArgument) -> bool {
    match outer {
        CalculationOperator::DividedBy => match right {
            // Matches Go: parenthesizeCalcRHS(DividedBy, right) → true for operations
            // Matches Dart: _parenthesizeCalculationRhs(dividedBy, _) → true
            CalcArgument::Operation(_) => true,
            // Matches Go: parenthesizeCalcRHSRight → n.HasComplexUnits() check
            // Matches Dart: right.value.isFinite ? right.hasComplexUnits : right.hasUnits
            CalcArgument::Number(n) => {
                if n.value.is_infinite() || n.value.is_nan() {
                    n.has_units()
                } else {
                    n.has_complex_units()
                }
            }
            _ => false,
        },
        CalculationOperator::Plus => false,
        _ => match right {
            CalcArgument::Operation(op) => {
                op.operator == CalculationOperator::Plus
                    || op.operator == CalculationOperator::Minus
            }
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::SerializeState;
    use crate::source_map_buffer::SourceMapBuffer;
    use crate::value::calculation::CalcArgument;
    use crate::value::SassNumber;

    #[test]
    fn test_calc_number() {
        let mut buf = SourceMapBuffer::new_plain();
        let state = SerializeState::new(true, false);
        let n = SassNumber::new(42.0, None);
        let arg = CalcArgument::Number(n);
        write_calculation_value(&mut buf, &state, &arg).unwrap();
        assert_eq!(buf.into_string(), "42");
    }
}
