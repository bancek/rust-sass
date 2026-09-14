// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/embedded/protofier.dart
// go-source: go/embedded/protofier.go

use crate::embedded_sass::inbound_message::FunctionCallResponse;
use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexMap;

use rust_sass::common::exception::SassResult;
use rust_sass::value::color::ColorSpace;
use rust_sass::value::{
    CalcArgument, CalculationOperator, ListSeparator, ListValue, SassArgumentList, SassBoolean,
    SassCalculation, SassColor, SassFunction, SassList, SassMap, SassMixin, SassNumber, SassString,
    Value as SassValue, ValueKind, ValueVisitor, SASS_FALSE, SASS_TRUE,
};

use crate::compilation::CompilationContext;
use crate::embedded_sass::inbound_message::function_call_response;
use crate::embedded_sass::value::calculation::calculation_value;
use crate::embedded_sass::value::calculation::{self, CalculationValue as ProtoCalculationValue};
use crate::embedded_sass::{
    value as pv, CalculationOperator as ProtoCalcOperator, ListSeparator as ProtoListSeparator,
    SingletonValue, Value as ProtoValue,
};
use crate::error::script;
use crate::function::host_callable;

/// Converts Sass `Value`s to/from protocol-buffer `Value`s, valid within the
/// scope of a single custom function call.
///
/// Matches Go: Protofier (protofier.go).
pub struct Protofier<'compile, 'parse> {
    pub ctx: Rc<CompilationContext<'compile, 'parse>>,
    /// Argument lists protofied so far, in protofy order (1-based IDs).
    argument_lists: Vec<SassValue<'parse>>,
}

impl<'compile: 'parse, 'parse: 'compile> Protofier<'compile, 'parse> {
    pub fn new(ctx: Rc<CompilationContext<'compile, 'parse>>) -> Self {
        Protofier {
            ctx,
            argument_lists: Vec::new(),
        }
    }

    /// Converts a Sass value to its proto representation.
    pub fn protofy(&mut self, v: &SassValue<'parse>) -> SassResult<ProtoValue> {
        Ok(ProtoValue {
            value: Some(v.accept(self)?),
        })
    }

    fn protofy_values(&mut self, values: &[SassValue<'parse>]) -> SassResult<Vec<ProtoValue>> {
        values.iter().map(|v| self.protofy_owned(v)).collect()
    }

    fn protofy_owned(&mut self, v: &SassValue<'parse>) -> SassResult<ProtoValue> {
        Ok(ProtoValue {
            value: Some(v.accept(self)?),
        })
    }

    /// Converts a proto value to its Sass representation.
    pub fn deprotofy(&self, v: &ProtoValue) -> SassResult<SassValue<'parse>> {
        let inner = v
            .value
            .as_ref()
            .ok_or_else(|| script("Missing mandatory field value"))?;
        self.deprotofy_inner(inner)
    }

    /// Converts a `FunctionCallResponse`'s return value to a Sass value,
    /// marking the referenced argument lists' keywords as accessed.
    ///
    /// Matches Go: DeprotofyResponse.
    pub fn deprotofy_response(
        &mut self,
        resp: &FunctionCallResponse,
    ) -> SassResult<SassValue<'parse>> {
        for &id in &resp.accessed_argument_lists {
            let arg_list = self.argument_list_for_id(id)?;
            if let ValueKind::ArgumentList(a) = arg_list.kind() {
                a.keywords(); // Mark keywords as accessed
            }
        }
        let success = match resp.result.as_ref() {
            Some(function_call_response::Result::Success(v)) => v,
            _ => return Err(script("Missing mandatory field result")),
        };
        self.deprotofy(success)
    }

    fn argument_list_for_id(&self, id: u32) -> SassResult<&SassValue<'parse>> {
        if id < 1 {
            return Err(script(&format!(
                "Value.ArgumentList.id {id} can't be marked as accessed"
            )));
        }
        self.argument_lists.get((id - 1) as usize).ok_or_else(|| {
            script(&format!(
                "Value.ArgumentList.id {id} doesn't match any known argument lists"
            ))
        })
    }

    fn deprotofy_inner(&self, v: &pv::Value) -> SassResult<SassValue<'parse>> {
        let out: SassValue<'parse> = match v {
            pv::Value::String(s) => SassValue::new_with_arena(
                self.ctx.arena,
                ValueKind::String(SassString::new(self.ctx.arena.alloc_str(&s.text), s.quoted)),
            ),
            pv::Value::Number(n) => SassValue::new_with_arena(
                self.ctx.arena,
                ValueKind::Number(SassNumber::with_units(
                    n.value,
                    n.numerators.clone(),
                    n.denominators.clone(),
                )),
            ),
            pv::Value::Color(c) => {
                let space =
                    ColorSpace::from_name(&c.space, None).map_err(|e| script(&e.to_string()))?;
                let channels = [
                    c.channel1.unwrap_or(0.0),
                    c.channel2.unwrap_or(0.0),
                    c.channel3.unwrap_or(0.0),
                ];
                let missing = [
                    c.channel1.is_none(),
                    c.channel2.is_none(),
                    c.channel3.is_none(),
                    c.alpha.is_none(),
                ];
                SassValue::new_with_arena(
                    self.ctx.arena,
                    ValueKind::Color(SassColor::for_space(
                        space,
                        channels,
                        c.alpha.unwrap_or(0.0),
                        missing,
                    )),
                )
            }
            pv::Value::List(l) => {
                let separator = deprotofy_separator(l.separator)?;
                let contents = self.deprotofy_values(&l.contents)?;
                if contents.is_empty() {
                    SassValue::new_with_arena(
                        self.ctx.arena,
                        ValueKind::List(SassList::empty(separator, l.has_brackets)),
                    )
                } else {
                    if separator == ListSeparator::Undecided && contents.len() > 1 {
                        return Err(script(&format!(
                            "List can't have an undecided separator because it has {} elements",
                            contents.len()
                        )));
                    }
                    SassValue::new_with_arena(
                        self.ctx.arena,
                        ValueKind::List(SassList::new(contents, separator, l.has_brackets)),
                    )
                }
            }
            pv::Value::Map(m) => {
                let mut entries = Vec::new();
                for e in &m.entries {
                    let key = e
                        .key
                        .as_ref()
                        .ok_or_else(|| script("Missing mandatory field key"))?;
                    let value = e
                        .value
                        .as_ref()
                        .ok_or_else(|| script("Missing mandatory field value"))?;
                    entries.push((self.deprotofy(key)?, self.deprotofy(value)?));
                }
                SassValue::new_with_arena(
                    self.ctx.arena,
                    ValueKind::Map(SassMap::from_entries(entries)),
                )
            }
            pv::Value::Singleton(s) => match *s {
                0 => SassValue::new_with_arena(self.ctx.arena, ValueKind::Boolean(SASS_TRUE)),
                1 => SassValue::new_with_arena(self.ctx.arena, ValueKind::Boolean(SASS_FALSE)),
                _ => SassValue::new_with_arena(self.ctx.arena, ValueKind::Null),
            },
            pv::Value::CompilerFunction(f) => {
                // Return the stored value as-is so its compile context (the
                // eval's) survives the host round-trip. Matches Dart:
                // `if (_functions[id] case var function?) return function;`.
                let function = self
                    .ctx
                    .functions
                    .borrow()
                    .get(f.id)
                    .cloned()
                    .ok_or_else(|| {
                        script(&format!(
                            "CompilerFunction.id {} doesn't match any known functions",
                            f.id
                        ))
                    })?;
                SassValue::new_with_arena(self.ctx.arena, ValueKind::Function(function))
            }
            pv::Value::HostFunction(h) => {
                // Host-defined functions have no compilation ownership, so they
                // use the plain constructor (compile context = None), matching
                // Dart's `SassFunction(hostCallable(...))`.
                let callable =
                    host_callable(self.ctx.clone(), &h.signature, Some(h.id), self.ctx.arena)?;
                SassValue::new_with_arena(
                    self.ctx.arena,
                    ValueKind::Function(SassFunction::new(callable)),
                )
            }
            pv::Value::CompilerMixin(m) => {
                let mixin = self.ctx.mixins.borrow().get(m.id).cloned().ok_or_else(|| {
                    script(&format!(
                        "CompilerMixin.id {} doesn't match any known mixins",
                        m.id
                    ))
                })?;
                SassValue::new_with_arena(self.ctx.arena, ValueKind::Mixin(mixin))
            }
            pv::Value::ArgumentList(a) => {
                if a.id != 0 {
                    let arg_list = self.argument_list_for_id(a.id)?;
                    return Ok(*arg_list);
                }
                let separator = deprotofy_separator(a.separator)?;
                let contents = self.deprotofy_values(&a.contents)?;
                let mut keywords = IndexMap::new();
                for (k, v) in &a.keywords {
                    keywords.insert(k.clone(), self.deprotofy(v)?);
                }
                SassValue::new_with_arena(
                    self.ctx.arena,
                    ValueKind::ArgumentList(SassArgumentList::new(
                        self.ctx.arena,
                        contents,
                        keywords,
                        separator,
                    )),
                )
            }
            pv::Value::Calculation(c) => self.deprotofy_calculation(c)?,
        };
        Ok(out)
    }

    fn deprotofy_values(&self, values: &[ProtoValue]) -> SassResult<Vec<SassValue<'parse>>> {
        values
            .iter()
            .map(|v| {
                let inner = v
                    .value
                    .as_ref()
                    .ok_or_else(|| script("Missing mandatory field value"))?;
                self.deprotofy_inner(inner)
            })
            .collect()
    }

    fn protofy_calculation(&mut self, c: &SassCalculation) -> SassResult<pv::Calculation> {
        let mut arguments = Vec::with_capacity(c.arguments.len());
        for arg in &c.arguments {
            arguments.push(self.protofy_calculation_value(arg)?);
        }
        Ok(pv::Calculation {
            name: c.name.clone(),
            arguments,
        })
    }

    fn protofy_calculation_value(
        &mut self,
        arg: &CalcArgument,
    ) -> SassResult<ProtoCalculationValue> {
        let oneof = match arg {
            CalcArgument::Number(n) => calculation_value::Value::Number(pv::Number {
                value: n.value,
                numerators: n.numerator_units.clone(),
                denominators: n.denominator_units.clone(),
            }),
            CalcArgument::Calculation(c) => {
                calculation_value::Value::Calculation(self.protofy_calculation(c)?)
            }
            CalcArgument::String(s, _quoted) => calculation_value::Value::String(s.clone()),
            CalcArgument::Interpolation(s) => calculation_value::Value::Interpolation(s.clone()),
            CalcArgument::Operation(op) => {
                calculation_value::Value::Operation(Box::new(calculation::CalculationOperation {
                    operator: protofy_calculation_operator(op.operator)?,
                    left: Some(Box::new(self.protofy_calculation_value(&op.left)?)),
                    right: Some(Box::new(self.protofy_calculation_value(&op.right)?)),
                }))
            }
        };
        Ok(ProtoCalculationValue { value: Some(oneof) })
    }

    fn deprotofy_calculation(&self, c: &pv::Calculation) -> SassResult<SassValue<'parse>> {
        let mut args = Vec::with_capacity(c.arguments.len());
        for a in &c.arguments {
            args.push(self.deprotofy_calculation_value(a)?);
        }
        let static_value = match c.name.as_str() {
            "calc" => {
                if args.len() != 1 {
                    return Err(script(
                        "Value.Calculation.arguments must have exactly one argument for calc().",
                    ));
                }
                rust_sass::value::calculation::new_calc(self.ctx.arena, args[0].clone())?
            }
            "clamp" => {
                if args.is_empty() || args.len() > 3 {
                    return Err(script(
                        "Value.Calculation.arguments must have 1 to 3 arguments for clamp().",
                    ));
                }
                rust_sass::value::calculation::new_clamp(
                    self.ctx.arena,
                    args[0].clone(),
                    args.get(1).cloned(),
                    args.get(2).cloned(),
                )?
            }
            "min" => {
                if args.is_empty() {
                    return Err(script(
                        "Value.Calculation.arguments must have at least 1 argument for min().",
                    ));
                }
                rust_sass::value::calculation::new_min(self.ctx.arena, args)?
            }
            "max" => {
                if args.is_empty() {
                    return Err(script(
                        "Value.Calculation.arguments must have at least 1 argument for max().",
                    ));
                }
                rust_sass::value::calculation::new_max(self.ctx.arena, args)?
            }
            name => {
                return Err(script(&format!(
                    "Value.Calculation.name \"{name}\" is not a recognized calculation type."
                )))
            }
        };
        Ok(static_value)
    }

    fn deprotofy_calculation_value(&self, cv: &ProtoCalculationValue) -> SassResult<CalcArgument> {
        match cv
            .value
            .as_ref()
            .ok_or_else(|| script("Missing mandatory field value"))?
        {
            calculation_value::Value::Number(n) => Ok(CalcArgument::Number(
                SassNumber::with_units(n.value, n.numerators.clone(), n.denominators.clone()),
            )),
            calculation_value::Value::String(s) => Ok(CalcArgument::String(s.clone(), false)),
            calculation_value::Value::Interpolation(s) => {
                Ok(CalcArgument::String(format!("({s})"), false))
            }
            calculation_value::Value::Operation(op) => {
                let operator = deprotofy_calculation_operator(op.operator)?;
                let left = self.deprotofy_calculation_value(
                    op.left
                        .as_ref()
                        .ok_or_else(|| script("Missing mandatory field left"))?,
                )?;
                let right = self.deprotofy_calculation_value(
                    op.right
                        .as_ref()
                        .ok_or_else(|| script("Missing mandatory field right"))?,
                )?;
                // Matches Dart `_deprotofyCalculationValue`'s operation arm,
                // which calls `SassCalculation.operate` (protofier.dart): this
                // simplifies numeric operands (`1 + 2` -> `3`).
                rust_sass::value::calculation::operate(operator, left, right)
            }
            calculation_value::Value::Calculation(c) => {
                // Matches Dart: `_deprotofyCalculation` may simplify to a
                // number (e.g. `min(3, 4)` -> `3`); the result is used directly
                // as a calculation operand.
                match self.deprotofy_calculation(c)?.kind() {
                    ValueKind::Calculation(b) => Ok(CalcArgument::Calculation(b.clone())),
                    ValueKind::Number(n) => Ok(CalcArgument::Number(n.clone())),
                    _ => Err(script("Internal error: expected a calculation")),
                }
            }
        }
    }

    fn protofy_argument_list(&mut self, a: &SassArgumentList<'parse>) -> SassResult<pv::Value> {
        let id = (self.argument_lists.len() + 1) as u32;
        let contents = self.protofy_values(&a.list.contents)?;
        let mut keywords = HashMap::new();
        for (k, v) in a.keywords_without_marking() {
            keywords.insert(k.clone(), self.protofy_owned(v)?);
        }
        self.argument_lists.push(SassValue::new_with_arena(
            self.ctx.arena,
            ValueKind::ArgumentList(a.clone()),
        ));
        Ok(pv::Value::ArgumentList(pv::ArgumentList {
            id,
            separator: protofy_separator(a.list.separator)?,
            contents,
            keywords,
        }))
    }
}

impl<'compile: 'parse, 'parse: 'compile> ValueVisitor<'parse> for Protofier<'compile, 'parse> {
    type Output = pv::Value;

    fn visit_boolean(&mut self, value: &SassBoolean) -> SassResult<pv::Value> {
        Ok(pv::Value::Singleton(if value.value {
            SingletonValue::True as i32
        } else {
            SingletonValue::False as i32
        }))
    }

    fn visit_null(&mut self) -> SassResult<pv::Value> {
        Ok(pv::Value::Singleton(SingletonValue::Null as i32))
    }

    fn visit_string(&mut self, value: &SassString<'parse>) -> SassResult<pv::Value> {
        Ok(pv::Value::String(pv::String {
            text: value.text.to_string(),
            quoted: value.has_quotes,
        }))
    }

    fn visit_number(&mut self, value: &SassNumber) -> SassResult<pv::Value> {
        Ok(pv::Value::Number(pv::Number {
            value: value.value,
            numerators: value.numerator_units.clone(),
            denominators: value.denominator_units.clone(),
        }))
    }

    fn visit_color(&mut self, value: &SassColor) -> SassResult<pv::Value> {
        Ok(pv::Value::Color(pv::Color {
            space: value.space.name().to_string(),
            channel1: value.channel0_or_nil(),
            channel2: value.channel1_or_nil(),
            channel3: value.channel2_or_nil(),
            alpha: value.alpha_or_nil(),
        }))
    }

    fn visit_list(&mut self, value: &ListValue<'_, 'parse>) -> SassResult<pv::Value> {
        match value {
            ListValue::List(l) => {
                let contents = self.protofy_values(&l.contents)?;
                Ok(pv::Value::List(pv::List {
                    separator: protofy_separator(l.separator)?,
                    has_brackets: l.has_brackets,
                    contents,
                }))
            }
            ListValue::ArgumentList(a) => self.protofy_argument_list(a),
        }
    }

    fn visit_map(&mut self, value: &SassMap<'parse>) -> SassResult<pv::Value> {
        let mut entries = Vec::with_capacity(value.entries.len());
        for (k, v) in &value.entries {
            entries.push(pv::map::Entry {
                key: Some(self.protofy_owned(k)?),
                value: Some(self.protofy_owned(v)?),
            });
        }
        Ok(pv::Value::Map(pv::Map { entries }))
    }

    fn visit_calculation(&mut self, value: &SassCalculation) -> SassResult<pv::Value> {
        Ok(pv::Value::Calculation(self.protofy_calculation(value)?))
    }

    fn visit_function(&mut self, value: &SassFunction<'parse>) -> SassResult<pv::Value> {
        let id = self.ctx.functions.borrow_mut().get_id(value);
        Ok(pv::Value::CompilerFunction(pv::CompilerFunction { id }))
    }

    fn visit_mixin(&mut self, value: &SassMixin<'parse>) -> SassResult<pv::Value> {
        let id = self.ctx.mixins.borrow_mut().get_id(value);
        Ok(pv::Value::CompilerMixin(pv::CompilerMixin { id }))
    }
}

fn protofy_separator(sep: ListSeparator) -> SassResult<i32> {
    Ok(match sep {
        ListSeparator::Comma => ProtoListSeparator::Comma as i32,
        ListSeparator::Space => ProtoListSeparator::Space as i32,
        ListSeparator::Slash => ProtoListSeparator::Slash as i32,
        ListSeparator::Undecided => ProtoListSeparator::Undecided as i32,
    })
}

fn deprotofy_separator(sep: i32) -> SassResult<ListSeparator> {
    Ok(match sep {
        x if x == ProtoListSeparator::Comma as i32 => ListSeparator::Comma,
        x if x == ProtoListSeparator::Space as i32 => ListSeparator::Space,
        x if x == ProtoListSeparator::Slash as i32 => ListSeparator::Slash,
        x if x == ProtoListSeparator::Undecided as i32 => ListSeparator::Undecided,
        _ => return Err(script(&format!("Unknown ListSeparator {sep}"))),
    })
}

fn protofy_calculation_operator(op: CalculationOperator) -> SassResult<i32> {
    Ok(match op {
        CalculationOperator::Plus => ProtoCalcOperator::Plus as i32,
        CalculationOperator::Minus => ProtoCalcOperator::Minus as i32,
        CalculationOperator::Times => ProtoCalcOperator::Times as i32,
        CalculationOperator::DividedBy => ProtoCalcOperator::Divide as i32,
    })
}

fn deprotofy_calculation_operator(op: i32) -> SassResult<CalculationOperator> {
    Ok(match op {
        x if x == ProtoCalcOperator::Plus as i32 => CalculationOperator::Plus,
        x if x == ProtoCalcOperator::Minus as i32 => CalculationOperator::Minus,
        x if x == ProtoCalcOperator::Times as i32 => CalculationOperator::Times,
        x if x == ProtoCalcOperator::Divide as i32 => CalculationOperator::DividedBy,
        _ => return Err(script(&format!("Unknown CalculationOperator {op}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation::test_context;

    use rust_sass::callable::{Callable, CallableKind, PlainCssCallable};
    use rust_sass::Bump;

    fn test_ctx<'compile, 'parse>(arena: &'compile Bump) -> Rc<CompilationContext<'compile, 'parse>>
    where
        'compile: 'parse,
    {
        #[cfg(feature = "async")]
        let (_tx, rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        #[cfg(not(feature = "async"))]
        let (_tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        test_context(rx, arena)
    }

    fn dummy<'compile, 'parse>(arena: &'compile Bump, name: &str) -> Callable<'compile, 'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        Callable::new(
            arena,
            CallableKind::PlainCss(PlainCssCallable { name: name.into() }),
        )
    }

    fn roundtrip<'compile, 'parse>(
        ctx: &Rc<CompilationContext<'compile, 'parse>>,
        v: SassValue<'parse>,
    ) where
        'compile: 'parse,
        'parse: 'compile,
    {
        let back = roundtrip_deprotofy(ctx, &v);
        assert!(v.equals(&back), "roundtrip mismatch:\n  {v:?}\n  {back:?}");
    }

    /// Protofies `v` and deprotofies the result, returning the value.
    fn roundtrip_deprotofy<'compile, 'parse>(
        ctx: &Rc<CompilationContext<'compile, 'parse>>,
        v: &SassValue<'parse>,
    ) -> SassValue<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let mut protofier = Protofier::new(ctx.clone());
        let proto = protofier.protofy(v).unwrap();
        protofier.deprotofy(&proto).unwrap()
    }

    fn rgb(r: f64, g: f64, b: f64, a: f64) -> SassColor {
        SassColor::for_space(
            ColorSpace::from_name("rgb", None).unwrap(),
            [r, g, b],
            a,
            [false, false, false, false],
        )
    }

    #[test]
    fn roundtrip_boolean() {
        let arena = Bump::new();
        let ctx = test_ctx(&arena);
        roundtrip(
            &ctx,
            SassValue::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE)),
        );
        roundtrip(
            &ctx,
            SassValue::new_with_arena(&arena, ValueKind::Boolean(SASS_FALSE)),
        );
    }

    #[test]
    fn roundtrip_null() {
        let arena = Bump::new();
        let ctx = test_ctx(&arena);
        roundtrip(&ctx, SassValue::new_with_arena(&arena, ValueKind::Null));
    }

    #[test]
    fn roundtrip_string() {
        let arena = Bump::new();
        let ctx = test_ctx(&arena);
        roundtrip(
            &ctx,
            SassValue::new_with_arena(
                &arena,
                ValueKind::String(SassString::new(arena.alloc_str("hello"), true)),
            ),
        );
        roundtrip(
            &ctx,
            SassValue::new_with_arena(
                &arena,
                ValueKind::String(SassString::new(arena.alloc_str("foo"), false)),
            ),
        );
    }

    #[test]
    fn roundtrip_number() {
        let arena = Bump::new();
        let ctx = test_ctx(&arena);
        roundtrip(
            &ctx,
            SassValue::new_with_arena(
                &arena,
                ValueKind::Number(SassNumber::with_units(
                    3.0,
                    vec!["px".into()],
                    vec!["s".into()],
                )),
            ),
        );
        roundtrip(
            &ctx,
            SassValue::new_with_arena(&arena, ValueKind::Number(SassNumber::new(42.0, None))),
        );
    }

    #[test]
    fn roundtrip_color() {
        let arena = Bump::new();
        let ctx = test_ctx(&arena);
        roundtrip(
            &ctx,
            SassValue::new_with_arena(&arena, ValueKind::Color(rgb(1.0, 2.0, 3.0, 0.5))),
        );
        // Missing channels round-trip as missing.
        roundtrip(
            &ctx,
            SassValue::new_with_arena(
                &arena,
                ValueKind::Color(SassColor::for_space(
                    ColorSpace::from_name("rgb", None).unwrap(),
                    [0.0, 2.0, 0.0],
                    0.0,
                    [true, false, true, true],
                )),
            ),
        );
    }

    #[test]
    fn roundtrip_list() {
        let arena = Bump::new();
        let ctx = test_ctx(&arena);
        roundtrip(
            &ctx,
            SassValue::new_with_arena(
                &arena,
                ValueKind::List(SassList::new(
                    vec![
                        SassValue::new_with_arena(
                            &arena,
                            ValueKind::Number(SassNumber::new(1.0, None)),
                        ),
                        SassValue::new_with_arena(
                            &arena,
                            ValueKind::String(SassString::new(arena.alloc_str("a"), false)),
                        ),
                    ],
                    ListSeparator::Comma,
                    true,
                )),
            ),
        );
        roundtrip(
            &ctx,
            SassValue::new_with_arena(
                &arena,
                ValueKind::List(SassList::empty(ListSeparator::Undecided, false)),
            ),
        );
    }

    #[test]
    fn roundtrip_map() {
        let arena = Bump::new();
        let ctx = test_ctx(&arena);
        roundtrip(
            &ctx,
            SassValue::new_with_arena(
                &arena,
                ValueKind::Map(SassMap::from_entries(vec![
                    (
                        SassValue::new_with_arena(
                            &arena,
                            ValueKind::String(SassString::new(arena.alloc_str("key"), true)),
                        ),
                        SassValue::new_with_arena(
                            &arena,
                            ValueKind::Number(SassNumber::new(1.0, None)),
                        ),
                    ),
                    (
                        SassValue::new_with_arena(
                            &arena,
                            ValueKind::Number(SassNumber::new(2.0, None)),
                        ),
                        SassValue::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE)),
                    ),
                ])),
            ),
        );
    }

    #[test]
    fn roundtrip_argument_list() {
        let arena = Bump::new();
        let ctx = test_ctx(&arena);
        let mut keywords = IndexMap::new();
        keywords.insert(
            "kw".to_string(),
            SassValue::new_with_arena(
                &arena,
                ValueKind::String(SassString::new(arena.alloc_str("v"), true)),
            ),
        );
        roundtrip(
            &ctx,
            SassValue::new_with_arena(
                &arena,
                ValueKind::ArgumentList(SassArgumentList::new(
                    &arena,
                    vec![SassValue::new_with_arena(
                        &arena,
                        ValueKind::Number(SassNumber::new(1.0, None)),
                    )],
                    keywords,
                    ListSeparator::Comma,
                )),
            ),
        );
    }

    #[test]
    fn roundtrip_calculation() {
        let arena = Bump::new();
        let ctx = test_ctx(&arena);
        roundtrip(
            &ctx,
            SassValue::new_with_arena(
                &arena,
                ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                    "calc",
                    vec![CalcArgument::String("var(--x)".into(), false)],
                ))),
            ),
        );
        roundtrip(
            &ctx,
            SassValue::new_with_arena(
                &arena,
                ValueKind::Calculation(Box::new(SassCalculation::new_unsimplified(
                    "min",
                    vec![
                        CalcArgument::Number(SassNumber::new(1.0, Some("px"))),
                        CalcArgument::String("var(--x)".into(), false),
                    ],
                ))),
            ),
        );
    }

    #[test]
    fn roundtrip_function_and_mixin() {
        let arena = Bump::new();
        let ctx = test_ctx(&arena);
        let fn_value = SassFunction::new(dummy(&arena, "f"));
        let mixin_value = SassMixin::new(dummy(&arena, "m"));
        let fn_id = ctx.functions.borrow_mut().get_id(&fn_value);
        let mixin_id = ctx.mixins.borrow_mut().get_id(&mixin_value);
        roundtrip(
            &ctx,
            SassValue::new_with_arena(&arena, ValueKind::Function(fn_value.clone())),
        );
        roundtrip(
            &ctx,
            SassValue::new_with_arena(&arena, ValueKind::Mixin(mixin_value.clone())),
        );
        assert_eq!(fn_id, 0);
        assert_eq!(mixin_id, 0); // separate registry, also starts at 0
                                 // The round-trip returns the stored value (with its compile context).
        let back = roundtrip_deprotofy(
            &ctx,
            &SassValue::new_with_arena(&arena, ValueKind::Function(fn_value.clone())),
        );
        assert!(back.equals(&SassValue::new_with_arena(
            &arena,
            ValueKind::Function(fn_value)
        )));
    }

    #[test]
    fn argument_list_referenced_by_id_returns_same_value() {
        let arena = Bump::new();
        let ctx = test_ctx(&arena);
        let mut protofier = Protofier::new(ctx.clone());
        let arg_list = SassValue::new_with_arena(
            &arena,
            ValueKind::ArgumentList(SassArgumentList::new(
                &arena,
                vec![SassValue::new_with_arena(
                    &arena,
                    ValueKind::Number(SassNumber::new(7.0, None)),
                )],
                IndexMap::new(),
                ListSeparator::Undecided,
            )),
        );
        let proto = protofier.protofy(&arg_list).unwrap();
        // id 0 means the host builds a fresh list; id != 0 refers back.
        assert!(matches!(
            proto.value,
            Some(pv::Value::ArgumentList(ref al)) if al.id == 1
        ));
        let back = protofier.deprotofy(&proto).unwrap();
        // The deprotofied value is the clone the protofier stored; it equals
        // the original (and shares the keywords-accessed flag via Rc<Cell>).
        assert!(arg_list.equals(&back));
    }
}
