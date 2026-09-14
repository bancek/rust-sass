// Value <-> JS marshalling for the wasm bindings.
//
// This mirrors rust-sass-embedded/src/protofier.rs case-for-case (same wire
// semantics: color spaces + missing channels, calculation trees, argument-list
// keyword-access tracking, opaque function/mixin ids) but crosses the boundary
// as tagged plain JS objects (serde-wasm-bindgen) instead of protobufs.
//
// dart-source: lib/src/embedded/protofier.dart
// go-source: go/embedded/protofier.go  (via rust-sass-embedded/src/protofier.rs)

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use js_sys::{Array, Object, Reflect};
use rust_sass::callable::Callable;
use rust_sass::common::exception::{SassError, SassResult};
use rust_sass::value::color::ColorSpace;
use rust_sass::value::{
    CalcArgument, CalculationOperator, ListSeparator, SassArgumentList, SassBoolean, SassColor,
    SassFunction, SassList, SassMap, SassMixin, SassNumber, SassString, Value as SassValue,
    ValueKind,
};
#[cfg(test)]
use rust_sass::value::{SassCalculation, SASS_FALSE, SASS_TRUE};
use rust_sass::Bump;
use serde::Serialize;
use wasm_bindgen::{JsCast, JsValue};

pub(crate) fn script(message: impl Into<String>) -> Box<SassError> {
    Box::new(SassError::Script {
        message: message.into(),
        argument_name: None,
    })
}

/// Per-compilation opaque registries used while marshalling values across the
/// JS boundary: argument lists (1-based ids) and function/mixin references
/// (0-based ids). Shared (Rc<RefCell>) so function callbacks and importers can
/// borrow it. Lives for exactly one compilation (dropped with the compile).
/// `Clone` shares the registries (Rc clone) — used to move into callbacks.
#[derive(Default, Clone)]
pub struct MarshallingContext<'parse> {
    argument_lists: Rc<RefCell<Vec<SassValue<'parse>>>>,
    functions: Rc<RefCell<Vec<SassFunction<'parse>>>>,
    mixins: Rc<RefCell<Vec<SassMixin<'parse>>>>,
}

impl<'parse> MarshallingContext<'parse> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the keywords of the argument lists with the given 1-based ids as
    /// accessed (mirrors protofier.rs `deprotofy_response`).
    pub fn mark_accessed(&self, ids: &[u32]) -> SassResult<()> {
        for &id in ids {
            let list = self.argument_list_for_id(id)?;
            if let ValueKind::ArgumentList(a) = list.kind() {
                a.keywords(); // mark accessed
            }
        }
        Ok(())
    }

    fn argument_list_for_id(&self, id: u32) -> SassResult<SassValue<'parse>> {
        if id < 1 {
            return Err(script(format!(
                "Value.ArgumentList.id {id} can't be marked as accessed"
            )));
        }
        self.argument_lists
            .borrow()
            .get((id - 1) as usize)
            .cloned()
            .ok_or_else(|| {
                script(format!(
                    "Value.ArgumentList.id {id} doesn't match any known argument lists"
                ))
            })
    }
}

// ==== Outgoing: SassValue -> JS object (serde) ===============================

#[derive(Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SerializableValue<'parse> {
    Boolean {
        value: bool,
    },
    Null,
    String {
        text: &'parse str,
        quoted: bool,
    },
    Number {
        value: f64,
        numerator_units: Vec<String>,
        denominator_units: Vec<String>,
    },
    Color {
        space: &'static str,
        channel1: Option<f64>,
        channel2: Option<f64>,
        channel3: Option<f64>,
        alpha: Option<f64>,
        missing: [bool; 4],
    },
    List {
        separator: &'static str,
        has_brackets: bool,
        contents: Vec<SerializableValue<'parse>>,
    },
    ArgumentList {
        id: u32,
        separator: &'static str,
        has_brackets: bool,
        contents: Vec<SerializableValue<'parse>>,
        keywords: IndexMap<String, SerializableValue<'parse>>,
    },
    Map {
        entries: Vec<SerializableMapEntry<'parse>>,
    },
    Calculation {
        name: String,
        arguments: Vec<SerializableCalcArg>,
    },
    Function {
        id: u32,
    },
    Mixin {
        id: u32,
    },
}

#[derive(Serialize)]
pub struct SerializableMapEntry<'parse> {
    key: SerializableValue<'parse>,
    value: SerializableValue<'parse>,
}

#[derive(Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SerializableCalcArg {
    Number {
        value: f64,
        numerator_units: Vec<String>,
        denominator_units: Vec<String>,
    },
    String {
        value: String,
        quoted: bool,
    },
    Interpolation {
        value: String,
    },
    Operation {
        operator: &'static str,
        left: Box<SerializableCalcArg>,
        right: Box<SerializableCalcArg>,
    },
    Calculation {
        name: String,
        arguments: Vec<SerializableCalcArg>,
    },
}

/// Converts a Sass value to its JS representation, registering opaque ids in
/// `ctx` as needed. Mirrors protofier.rs `protofy`.
pub fn value_to_js<'parse>(
    ctx: &MarshallingContext<'parse>,
    v: &SassValue<'parse>,
) -> SassResult<JsValue> {
    let serializable = to_serializable(ctx, v)?;
    serde_wasm_bindgen::to_value(&serializable)
        .map_err(|e| script(format!("Failed to serialize value to JS: {e}")))
}

fn to_serializable<'parse>(
    ctx: &MarshallingContext<'parse>,
    v: &SassValue<'parse>,
) -> SassResult<SerializableValue<'parse>> {
    Ok(match v.kind() {
        ValueKind::Boolean(b) => SerializableValue::Boolean { value: b.value },
        ValueKind::Null => SerializableValue::Null,
        ValueKind::String(s) => SerializableValue::String {
            text: s.text,
            quoted: s.has_quotes,
        },
        ValueKind::Number(n) => SerializableValue::Number {
            value: n.value,
            numerator_units: n.numerator_units.clone(),
            denominator_units: n.denominator_units.clone(),
        },
        ValueKind::Color(c) => SerializableValue::Color {
            space: c.space.name(),
            channel1: c.channel0_or_nil(),
            channel2: c.channel1_or_nil(),
            channel3: c.channel2_or_nil(),
            alpha: c.alpha_or_nil(),
            missing: [
                c.channel0_or_nil().is_none(),
                c.channel1_or_nil().is_none(),
                c.channel2_or_nil().is_none(),
                c.alpha_or_nil().is_none(),
            ],
        },
        ValueKind::List(l) => SerializableValue::List {
            separator: separator_str(l.separator),
            has_brackets: l.has_brackets,
            contents: to_serializable_values(ctx, &l.contents)?,
        },
        ValueKind::ArgumentList(a) => {
            let id = {
                let mut lists = ctx.argument_lists.borrow_mut();
                lists.push(*v);
                lists.len() as u32
            };
            let keywords = a
                .keywords_without_marking()
                .iter()
                .map(|(k, val)| Ok((k.clone(), to_serializable(ctx, val)?)))
                .collect::<SassResult<IndexMap<_, _>>>()?;
            SerializableValue::ArgumentList {
                id,
                separator: separator_str(a.list.separator),
                has_brackets: a.list.has_brackets,
                contents: to_serializable_values(ctx, &a.list.contents)?,
                keywords,
            }
        }
        ValueKind::Map(m) => {
            let mut entries = Vec::with_capacity(m.entries.len());
            for (k, val) in m.entries.iter() {
                entries.push(SerializableMapEntry {
                    key: to_serializable(ctx, k)?,
                    value: to_serializable(ctx, val)?,
                });
            }
            SerializableValue::Map { entries }
        }
        ValueKind::Calculation(c) => SerializableValue::Calculation {
            name: c.name.clone(),
            arguments: c
                .arguments
                .iter()
                .map(calc_arg_to_serializable)
                .collect::<SassResult<_>>()?,
        },
        ValueKind::Function(f) => {
            let mut funcs = ctx.functions.borrow_mut();
            let id = match funcs
                .iter()
                .position(|x| x.callable.identity_eq(&f.callable))
            {
                Some(id) => id,
                None => {
                    funcs.push(f.clone());
                    funcs.len() - 1
                }
            };
            SerializableValue::Function { id: id as u32 }
        }
        ValueKind::Mixin(m) => {
            let mut mixins = ctx.mixins.borrow_mut();
            let id = match mixins
                .iter()
                .position(|x| x.callable.identity_eq(&m.callable))
            {
                Some(id) => id,
                None => {
                    mixins.push(m.clone());
                    mixins.len() - 1
                }
            };
            SerializableValue::Mixin { id: id as u32 }
        }
    })
}

fn to_serializable_values<'parse>(
    ctx: &MarshallingContext<'parse>,
    values: &[SassValue<'parse>],
) -> SassResult<Vec<SerializableValue<'parse>>> {
    values.iter().map(|v| to_serializable(ctx, v)).collect()
}

fn calc_arg_to_serializable(arg: &CalcArgument) -> SassResult<SerializableCalcArg> {
    Ok(match arg {
        CalcArgument::Number(n) => SerializableCalcArg::Number {
            value: n.value,
            numerator_units: n.numerator_units.clone(),
            denominator_units: n.denominator_units.clone(),
        },
        CalcArgument::Calculation(c) => SerializableCalcArg::Calculation {
            name: c.name.clone(),
            arguments: c
                .arguments
                .iter()
                .map(calc_arg_to_serializable)
                .collect::<SassResult<_>>()?,
        },
        CalcArgument::String(s, quoted) => SerializableCalcArg::String {
            value: s.clone(),
            quoted: *quoted,
        },
        CalcArgument::Interpolation(s) => SerializableCalcArg::Interpolation { value: s.clone() },
        CalcArgument::Operation(op) => SerializableCalcArg::Operation {
            operator: operator_str(op.operator),
            left: Box::new(calc_arg_to_serializable(&op.left)?),
            right: Box::new(calc_arg_to_serializable(&op.right)?),
        },
    })
}

fn separator_str(sep: ListSeparator) -> &'static str {
    match sep {
        ListSeparator::Comma => "comma",
        ListSeparator::Space => "space",
        ListSeparator::Slash => "slash",
        ListSeparator::Undecided => "undecided",
    }
}

fn operator_str(op: CalculationOperator) -> &'static str {
    match op {
        CalculationOperator::Plus => "plus",
        CalculationOperator::Minus => "minus",
        CalculationOperator::Times => "times",
        CalculationOperator::DividedBy => "dividedBy",
    }
}

// ==== Incoming: JS object -> SassValue (js_sys) ==============================

/// Builds a host-defined `Callable` (JS `SassFunction(signature, callback)`)
/// so JS-created function values can be used inside Sass.
pub type HostFunctionBuilder<'compile, 'parse> =
    dyn Fn(&str, &JsValue, &'compile Bump) -> SassResult<Callable<'compile, 'parse>> + 'parse;

/// Converts a JS value object to a Sass value, allocating into the compile
/// arena. Mirrors protofier.rs `deprotofy`.
pub fn js_to_value<'compile: 'parse, 'parse: 'compile>(
    ctx: &MarshallingContext<'parse>,
    arena: &'compile Bump,
    value: &JsValue,
    host_function: &HostFunctionBuilder<'compile, 'parse>,
) -> SassResult<SassValue<'parse>> {
    let ty = get_str(value, "type")?;
    let out: SassValue<'parse> = match ty.as_str() {
        "boolean" => SassValue::new_with_arena(
            arena,
            ValueKind::Boolean(SassBoolean::new(get_bool(value, "value")?)),
        ),
        "null" => SassValue::new_with_arena(arena, ValueKind::Null),
        "string" => {
            let text = get_str(value, "text")?;
            SassValue::new_with_arena(
                arena,
                ValueKind::String(SassString::new(
                    arena.alloc_str(&text),
                    get_bool(value, "quoted")?,
                )),
            )
        }
        "number" => {
            let num = get_number(value)?;
            SassValue::new_with_arena(arena, ValueKind::Number(num))
        }
        "color" => {
            let space = ColorSpace::from_name(&get_str(value, "space")?, None)?;
            let c1 = get_f64(value, "channel1")?;
            let c2 = get_f64(value, "channel2")?;
            let c3 = get_f64(value, "channel3")?;
            let alpha = get_f64(value, "alpha")?;
            let missing = get_missing(
                value,
                c1.is_none(),
                c2.is_none(),
                c3.is_none(),
                alpha.is_none(),
            )?;
            SassValue::new_with_arena(
                arena,
                ValueKind::Color(SassColor::for_space(
                    space,
                    [c1.unwrap_or(0.0), c2.unwrap_or(0.0), c3.unwrap_or(0.0)],
                    alpha.unwrap_or(0.0),
                    missing,
                )),
            )
        }
        "list" => {
            let separator = de_separator(&get_str(value, "separator")?)?;
            let has_brackets = get_bool(value, "hasBrackets")?;
            let contents = get_value_array(ctx, arena, value, "contents", host_function)?;
            if contents.is_empty() {
                SassValue::new_with_arena(
                    arena,
                    ValueKind::List(SassList::empty(separator, has_brackets)),
                )
            } else {
                if separator == ListSeparator::Undecided && contents.len() > 1 {
                    return Err(script(format!(
                        "List can't have an undecided separator because it has {} elements",
                        contents.len()
                    )));
                }
                SassValue::new_with_arena(
                    arena,
                    ValueKind::List(SassList::new(contents, separator, has_brackets)),
                )
            }
        }
        "argumentList" => {
            let id = get_f64(value, "id")?.unwrap_or(0.0) as u32;
            if id != 0 {
                ctx.argument_list_for_id(id)? // clone of the stored value
            } else {
                let separator = de_separator(&get_str(value, "separator")?)?;
                let contents = get_value_array(ctx, arena, value, "contents", host_function)?;
                let keywords = get_keywords(ctx, arena, value, host_function)?;
                SassValue::new_with_arena(
                    arena,
                    ValueKind::ArgumentList(SassArgumentList::new(
                        arena, contents, keywords, separator,
                    )),
                )
            }
        }
        "map" => {
            let mut entries = Vec::new();
            let arr = Array::from(&get(value, "entries")?);
            for i in 0..arr.length() {
                let entry = arr.get(i);
                let key = js_to_value(ctx, arena, &get(&entry, "key")?, host_function)?;
                let val = js_to_value(ctx, arena, &get(&entry, "value")?, host_function)?;
                entries.push((key, val));
            }
            SassValue::new_with_arena(arena, ValueKind::Map(SassMap::from_entries(entries)))
        }
        "calculation" => {
            let name = get_str(value, "name")?;
            let args = get_calc_args(ctx, arena, value, host_function)?;
            let calc = de_calculation(arena, &name, args)?;
            calc
        }
        "function" => {
            let id = get_f64(value, "id")?.unwrap_or(0.0) as u32;
            let funcs = ctx.functions.borrow();
            let f = funcs.get(id as usize).cloned().ok_or_else(|| {
                script(format!(
                    "Value.Function.id {id} doesn't match any known functions"
                ))
            })?;
            SassValue::new_with_arena(arena, ValueKind::Function(f))
        }
        "mixin" => {
            let id = get_f64(value, "id")?.unwrap_or(0.0) as u32;
            let mixins = ctx.mixins.borrow();
            let m = mixins.get(id as usize).cloned().ok_or_else(|| {
                script(format!(
                    "Value.Mixin.id {id} doesn't match any known mixins"
                ))
            })?;
            SassValue::new_with_arena(arena, ValueKind::Mixin(m))
        }
        "hostFunction" => {
            let signature = get_str(value, "signature")?;
            let callback = get(value, "callback")?;
            let callable = host_function(&signature, &callback, arena)?;
            SassValue::new_with_arena(arena, ValueKind::Function(SassFunction::new(callable)))
        }
        other => return Err(script(format!("Unknown value type {other}"))),
    };
    Ok(out)
}

/// Converts a JS value + the accessed-argument-list ids returned from a custom
/// function call. Mirrors protofier.rs `deprotofy_response`.
pub fn value_from_js_with_accessed<'compile: 'parse, 'parse: 'compile>(
    ctx: &MarshallingContext<'parse>,
    arena: &'compile Bump,
    value: &JsValue,
    accessed: &[u32],
    host_function: &HostFunctionBuilder<'compile, 'parse>,
) -> SassResult<SassValue<'parse>> {
    ctx.mark_accessed(accessed)?;
    js_to_value(ctx, arena, value, host_function)
}

fn de_separator(sep: &str) -> SassResult<ListSeparator> {
    Ok(match sep {
        "comma" => ListSeparator::Comma,
        "space" => ListSeparator::Space,
        "slash" => ListSeparator::Slash,
        "undecided" => ListSeparator::Undecided,
        _ => return Err(script(format!("Unknown ListSeparator {sep}"))),
    })
}

fn de_operator(op: &str) -> SassResult<CalculationOperator> {
    Ok(match op {
        "plus" => CalculationOperator::Plus,
        "minus" => CalculationOperator::Minus,
        "times" => CalculationOperator::Times,
        "dividedBy" => CalculationOperator::DividedBy,
        _ => return Err(script(format!("Unknown CalculationOperator {op}"))),
    })
}

fn get_number(value: &JsValue) -> SassResult<SassNumber> {
    Ok(SassNumber::with_units(
        get_f64_required(value, "value")?,
        get_string_array(value, "numeratorUnits")?,
        get_string_array(value, "denominatorUnits")?,
    ))
}

fn get_value_array<'compile: 'parse, 'parse: 'compile>(
    ctx: &MarshallingContext<'parse>,
    arena: &'compile Bump,
    obj: &JsValue,
    key: &str,
    host_function: &HostFunctionBuilder<'compile, 'parse>,
) -> SassResult<Vec<SassValue<'parse>>> {
    let arr = Array::from(&get(obj, key)?);
    let mut out = Vec::with_capacity(arr.length() as usize);
    for i in 0..arr.length() {
        out.push(js_to_value(ctx, arena, &arr.get(i), host_function)?);
    }
    Ok(out)
}

fn get_keywords<'compile: 'parse, 'parse: 'compile>(
    ctx: &MarshallingContext<'parse>,
    arena: &'compile Bump,
    obj: &JsValue,
    host_function: &HostFunctionBuilder<'compile, 'parse>,
) -> SassResult<IndexMap<String, SassValue<'parse>>> {
    let keywords_obj = get(obj, "keywords")?;
    if keywords_obj.is_undefined() || keywords_obj.is_null() {
        return Ok(IndexMap::new());
    }
    let keywords_obj: Object = keywords_obj
        .dyn_into()
        .map_err(|_| script("Expected \"keywords\" to be an object"))?;
    let mut out = IndexMap::new();
    let keys = Object::keys(&keywords_obj);
    for i in 0..keys.length() {
        let key = keys.get(i);
        let name = key
            .as_string()
            .ok_or_else(|| script("keyword name not a string"))?;
        let val = Reflect::get(&keywords_obj, &key)
            .map_err(|e| script(format!("bad keyword value: {e:?}")))?;
        out.insert(name, js_to_value(ctx, arena, &val, host_function)?);
    }
    Ok(out)
}

fn get_calc_args<'compile: 'parse, 'parse: 'compile>(
    ctx: &MarshallingContext<'parse>,
    arena: &'compile Bump,
    obj: &JsValue,
    host_function: &HostFunctionBuilder<'compile, 'parse>,
) -> SassResult<Vec<CalcArgument>> {
    let arr = Array::from(&get(obj, "arguments")?);
    let mut out = Vec::with_capacity(arr.length() as usize);
    for i in 0..arr.length() {
        out.push(js_calc_arg_to_rust(ctx, arena, &arr.get(i), host_function)?);
    }
    Ok(out)
}

fn js_calc_arg_to_rust<'compile: 'parse, 'parse: 'compile>(
    ctx: &MarshallingContext<'parse>,
    arena: &'compile Bump,
    cv: &JsValue,
    host_function: &HostFunctionBuilder<'compile, 'parse>,
) -> SassResult<CalcArgument> {
    let ty = get_str(cv, "type")?;
    Ok(match ty.as_str() {
        "number" => CalcArgument::Number(get_number(cv)?),
        "string" => CalcArgument::String(get_str(cv, "value")?, get_bool(cv, "quoted")?),
        "interpolation" => CalcArgument::String(format!("({})", get_str(cv, "value")?), false),
        "operation" => {
            let operator = de_operator(&get_str(cv, "operator")?)?;
            let left = js_calc_arg_to_rust(ctx, arena, &get(cv, "left")?, host_function)?;
            let right = js_calc_arg_to_rust(ctx, arena, &get(cv, "right")?, host_function)?;
            rust_sass::value::calculation::operate(operator, left, right)?
        }
        "calculation" => {
            let name = get_str(cv, "name")?;
            let args = get_calc_args(ctx, arena, cv, host_function)?;
            match de_calculation(arena, &name, args)?.kind() {
                ValueKind::Calculation(b) => CalcArgument::Calculation(b.clone()),
                ValueKind::Number(n) => CalcArgument::Number(n.clone()),
                _ => return Err(script("Internal error: expected a calculation")),
            }
        }
        other => return Err(script(format!("Unknown calculation value type {other}"))),
    })
}

fn de_calculation<'compile: 'parse, 'parse: 'compile>(
    arena: &'compile Bump,
    name: &str,
    args: Vec<CalcArgument>,
) -> SassResult<SassValue<'parse>> {
    match name {
        "calc" => {
            if args.len() != 1 {
                return Err(script(
                    "Value.Calculation.arguments must have exactly one argument for calc().",
                ));
            }
            rust_sass::value::calculation::new_calc(arena, args[0].clone())
        }
        "clamp" => {
            if args.is_empty() || args.len() > 3 {
                return Err(script(
                    "Value.Calculation.arguments must have 1 to 3 arguments for clamp().",
                ));
            }
            rust_sass::value::calculation::new_clamp(
                arena,
                args[0].clone(),
                args.get(1).cloned(),
                args.get(2).cloned(),
            )
        }
        "min" => {
            if args.is_empty() {
                return Err(script(
                    "Value.Calculation.arguments must have at least 1 argument for min().",
                ));
            }
            rust_sass::value::calculation::new_min(arena, args)
        }
        "max" => {
            if args.is_empty() {
                return Err(script(
                    "Value.Calculation.arguments must have at least 1 argument for max().",
                ));
            }
            rust_sass::value::calculation::new_max(arena, args)
        }
        name => Err(script(format!(
            "Value.Calculation.name \"{name}\" is not a recognized calculation type."
        ))),
    }
}

// ==== js_sys field readers ===================================================

fn get(obj: &JsValue, key: &str) -> SassResult<JsValue> {
    Reflect::get(obj, &JsValue::from_str(key)).map_err(|e| {
        script(format!(
            "Invalid value object: failed to read \"{key}\": {e:?}"
        ))
    })
}

fn get_str(obj: &JsValue, key: &str) -> SassResult<String> {
    get(obj, key)?
        .as_string()
        .ok_or_else(|| script(format!("Expected \"{key}\" to be a string")))
}

fn get_f64(obj: &JsValue, key: &str) -> SassResult<Option<f64>> {
    let v = get(obj, key)?;
    if v.is_undefined() || v.is_null() {
        Ok(None)
    } else {
        v.as_f64()
            .ok_or_else(|| script(format!("Expected \"{key}\" to be a number")))
            .map(Some)
    }
}

fn get_f64_required(obj: &JsValue, key: &str) -> SassResult<f64> {
    get_f64(obj, key)?.ok_or_else(|| script(format!("Missing mandatory field {key}")))
}

fn get_bool(obj: &JsValue, key: &str) -> SassResult<bool> {
    let v = get(obj, key)?;
    if v.is_undefined() || v.is_null() {
        Ok(false)
    } else {
        v.as_bool()
            .ok_or_else(|| script(format!("Expected \"{key}\" to be a boolean")))
    }
}

fn get_string_array(obj: &JsValue, key: &str) -> SassResult<Vec<String>> {
    let arr = Array::from(&get(obj, key)?);
    let mut out = Vec::with_capacity(arr.length() as usize);
    for i in 0..arr.length() {
        out.push(
            arr.get(i)
                .as_string()
                .ok_or_else(|| script(format!("Expected \"{key}\" to contain strings")))?,
        );
    }
    Ok(out)
}

fn get_missing(obj: &JsValue, c1: bool, c2: bool, c3: bool, alpha: bool) -> SassResult<[bool; 4]> {
    let v = get(obj, "missing")?;
    if v.is_undefined() || v.is_null() {
        return Ok([c1, c2, c3, alpha]);
    }
    let arr = Array::from(&v);
    let get = |i: u32| -> SassResult<bool> {
        arr.get(i)
            .as_bool()
            .ok_or_else(|| script("Expected \"missing\" to contain booleans"))
    };
    Ok([
        c1 || get(0)?,
        c2 || get(1)?,
        c3 || get(2)?,
        alpha || get(3)?,
    ])
}

// ==== tests (run via wasm-pack test --node; js_sys is wasm-only) ==============

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx<'p>() -> MarshallingContext<'p> {
        MarshallingContext::new()
    }

    fn roundtrip<'compile: 'parse, 'parse: 'compile>(
        ctx: &MarshallingContext<'parse>,
        arena: &'compile Bump,
        v: SassValue<'parse>,
    ) {
        let back = roundtrip_value(ctx, arena, &v);
        assert!(
            v.equals(&back),
            "{}",
            format!("roundtrip mismatch:\n  {v:?}\n  {back:?}")
        );
    }

    fn roundtrip_value<'compile: 'parse, 'parse: 'compile>(
        ctx: &MarshallingContext<'parse>,
        arena: &'compile Bump,
        v: &SassValue<'parse>,
    ) -> SassValue<'parse> {
        let js = value_to_js(ctx, v).unwrap();
        js_to_value(ctx, arena, &js, &|_sig, _cb, _arena| {
            Err(script("unexpected hostFunction"))
        })
        .unwrap()
    }

    fn rgb(r: f64, g: f64, b: f64, a: f64) -> SassColor {
        SassColor::for_space(
            ColorSpace::from_name("rgb", None).unwrap(),
            [r, g, b],
            a,
            [false, false, false, false],
        )
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn roundtrip_boolean() {
        let arena = Bump::new();
        let ctx = test_ctx();
        roundtrip(
            &ctx,
            &arena,
            SassValue::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE)),
        );
        roundtrip(
            &ctx,
            &arena,
            SassValue::new_with_arena(&arena, ValueKind::Boolean(SASS_FALSE)),
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn roundtrip_null() {
        let arena = Bump::new();
        let ctx = test_ctx();
        roundtrip(
            &ctx,
            &arena,
            SassValue::new_with_arena(&arena, ValueKind::Null),
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn roundtrip_string() {
        let arena = Bump::new();
        let ctx = test_ctx();
        roundtrip(
            &ctx,
            &arena,
            SassValue::new_with_arena(
                &arena,
                ValueKind::String(SassString::new(arena.alloc_str("hello"), true)),
            ),
        );
        roundtrip(
            &ctx,
            &arena,
            SassValue::new_with_arena(
                &arena,
                ValueKind::String(SassString::new(arena.alloc_str("foo"), false)),
            ),
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn roundtrip_number() {
        let arena = Bump::new();
        let ctx = test_ctx();
        roundtrip(
            &ctx,
            &arena,
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
            &arena,
            SassValue::new_with_arena(&arena, ValueKind::Number(SassNumber::new(42.0, None))),
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn roundtrip_color() {
        let arena = Bump::new();
        let ctx = test_ctx();
        roundtrip(
            &ctx,
            &arena,
            SassValue::new_with_arena(&arena, ValueKind::Color(rgb(1.0, 2.0, 3.0, 0.5))),
        );
        roundtrip(
            &ctx,
            &arena,
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn roundtrip_list() {
        let arena = Bump::new();
        let ctx = test_ctx();
        roundtrip(
            &ctx,
            &arena,
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
            &arena,
            SassValue::new_with_arena(
                &arena,
                ValueKind::List(SassList::empty(ListSeparator::Undecided, false)),
            ),
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn roundtrip_map() {
        let arena = Bump::new();
        let ctx = test_ctx();
        roundtrip(
            &ctx,
            &arena,
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn roundtrip_argument_list() {
        let arena = Bump::new();
        let ctx = test_ctx();
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
            &arena,
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn roundtrip_calculation() {
        let arena = Bump::new();
        let ctx = test_ctx();
        roundtrip(
            &ctx,
            &arena,
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
            &arena,
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn argument_list_referenced_by_id_returns_same_value() {
        let arena = Bump::new();
        let ctx = test_ctx();
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
        let js = value_to_js(&ctx, &arg_list).unwrap();
        // id 1 means the host builds a fresh list; id != 0 refers back.
        let id = Reflect::get(&js, &JsValue::from_str("id"))
            .unwrap()
            .as_f64()
            .unwrap();
        assert_eq!(id, 1.0);
        let back = js_to_value(&ctx, &arena, &js, &|_s, _c, _a| Err(script("unexpected"))).unwrap();
        assert!(arg_list.equals(&back));
        assert_eq!(
            Reflect::get(&js, &JsValue::from_str("type"))
                .unwrap()
                .as_string()
                .as_deref(),
            Some("argumentList")
        );
    }
}
