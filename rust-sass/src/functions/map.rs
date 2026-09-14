// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/functions/map.dart
// go-source: go/functions/map.go

use std::rc::Rc;

use bumpalo::Bump;

use crate::callable::{BuiltInCallable, Callable, CallableKind};
use crate::common::exception::{SassError, SassResult};
use crate::eval::{EvalConfig, EvalState};
use crate::module::BuiltInModule;
use crate::value::{
    assert_map, ListSeparator, SassList, SassMap, Value, ValueKind, SASS_FALSE, SASS_TRUE,
};

/// The global definitions of Sass map functions.
///
/// Deprecated aliases for the `sass:map` module functions. Each wraps the
/// module callable so the call still runs module behavior but emits a
/// `global-builtin` deprecation warning. Only the six functions with global
/// spells are listed here; `set`, `deep-merge`, and `deep-remove` are
/// module-only. Wrapping order is load-bearing: `with_deprecation_warning`
/// runs before `with_name`, so the warning names the original module function.
///
/// Matches Dart: `global` (map.dart:17).
pub fn global_map_functions<'compile, 'parse>(
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
            map_get_function(arena)
                .with_deprecation_warning("map", None)
                .with_name("map-get".into())
        ),
        c!(
            arena,
            map_merge_function(arena)
                .with_deprecation_warning("map", None)
                .with_name("map-merge".into())
        ),
        c!(
            arena,
            map_remove_function(arena)
                .with_deprecation_warning("map", None)
                .with_name("map-remove".into())
        ),
        c!(
            arena,
            map_keys_function(arena)
                .with_deprecation_warning("map", None)
                .with_name("map-keys".into())
        ),
        c!(
            arena,
            map_values_function(arena)
                .with_deprecation_warning("map", None)
                .with_name("map-values".into())
        ),
        c!(
            arena,
            map_has_key_function(arena)
                .with_deprecation_warning("map", None)
                .with_name("map-has-key".into())
        ),
    ]
}

/// The Sass `sass:map` built-in module.
///
/// Registers `get`, `set`, `merge`, `remove`, `keys`, `values`, `has-key`,
/// `deep-merge`, and `deep-remove` in declaration order.
///
/// Matches Dart: `module` (map.dart:26).
pub fn map_module<'compile, 'parse>(arena: &'compile Bump) -> BuiltInModule<'compile, 'parse>
where
    'compile: 'parse,
{
    macro_rules! c {
        ($arena:expr, $f:expr) => {
            Callable::new($arena, CallableKind::BuiltIn($f))
        };
    }
    let fns: Vec<Callable<'compile, 'parse>> = vec![
        c!(arena, map_get_function(arena)),
        c!(arena, set_function(arena)),
        c!(arena, map_merge_function(arena)),
        c!(arena, map_remove_function(arena)),
        c!(arena, map_keys_function(arena)),
        c!(arena, map_values_function(arena)),
        c!(arena, map_has_key_function(arena)),
        c!(arena, deep_merge_function(arena)),
        c!(arena, deep_remove_function(arena)),
    ];
    BuiltInModule::new(arena, "map".into(), &fns, &[], indexmap::IndexMap::new())
}

/// Creates the `map.get($map, $key, $keys...)` callable.
///
/// Walks `$keys` through nested maps; returns `null` when an intermediate
/// key is missing or holds a non-map, or when the final key is absent.
///
/// Matches Dart: `_get` (map.dart:42).
fn map_get_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
// Each `*_function` below passes `"sass:map"` as the URL, like
// `BuiltInCallable::function` with the URL fixed (Dart's `_function` helper
// in functions/map.dart:249, inlined at each call site here).
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "get",
        "$map, $key, $keys...",
        "sass:map",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  _arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let mut m = assert_map(&args[0], Some("map"))?;
                let mut keys: Vec<Value<'parse>> = vec![args[1]];
                if args.len() > 2 {
                    let kl = args[2].as_list(arena)?;
                    keys.extend(kl);
                }
                let last = keys.len() - 1;
                for (i, key) in keys.iter().enumerate() {
                    let v = m.get(key).cloned();
                    if i == last {
                        return match v {
                            None => Ok(Value::new_with_arena(arena, ValueKind::Null)),
                            Some(v) => Ok(v),
                        };
                    }
                    let Some(v) = v else {
                        return Ok(Value::new_with_arena(arena, ValueKind::Null));
                    };
                    let ValueKind::Map(next) = &*v else {
                        return Ok(Value::new_with_arena(arena, ValueKind::Null));
                    };
                    m = next.clone();
                }
                Ok(Value::new_with_arena(arena, ValueKind::Null))
            },
        ),
    )
}

/// Creates the `map.set` callable (module-only, no global alias).
///
/// Overloads: `$map, $key, $value` replaces one entry; `$map, $args...`
/// splits the rest args into a key path plus the trailing value, creating
/// missing nesting via [`modify_map`]. Empty `$args` reports
/// `Expected $args to contain a key.`; a lone key reports
/// `Expected $args to contain a value.` — both plain `//` script errors with
/// no argument name, matching Dart's bare `SassScriptException`.
///
/// Matches Dart: `_set` (map.dart:53).
fn set_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::overloaded_function(
        "set",
        "",
        vec![
            (
                "$map, $key, $value",
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        let m = assert_map(&args[0], Some("map"))?;
                        let key = args[1];
                        let val = args[2];
                        let modify = move |_old: Value<'parse>| -> Value<'parse> { val };
                        Ok(modify_map(arena, &m, &[key], &modify, true))
                    },
                ),
            ),
            (
                "$map, $args...",
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        let m = assert_map(&args[0], Some("map"))?;
                        let rest_items = args[1].as_list(arena)?;
                        match rest_items.len() {
                            0 => Err(Box::new(SassError::Script {
                                message: "Expected $args to contain a key.".into(),
                                argument_name: None,
                            })),
                            1 => Err(Box::new(SassError::Script {
                                message: "Expected $args to contain a value.".into(),
                                argument_name: None,
                            })),
                            _ => {
                                let keys = &rest_items[..rest_items.len() - 1];
                                let set_val = rest_items[rest_items.len() - 1];
                                let modify =
                                    move |_old: Value<'parse>| -> Value<'parse> { set_val };
                                Ok(modify_map(arena, &m, keys, &modify, true))
                            }
                        }
                    },
                ),
            ),
        ],
        arena,
    )
}

/// Creates the `map.merge` callable.
///
/// Overloads: `$map1, $map2` does a shallow merge with `$map2` winning;
/// `$map1, $args...` merges `$map2` (the trailing rest arg, asserted as a
/// map named `map2`) into the nested map at the leading key path, creating
/// missing nesting. Errors mirror `set`: empty `$args` wants a key, a lone
/// key wants a map. Unlike `get`/`has-key`, a non-map intermediate is
/// replaced by the merged map rather than returned as `null`.
///
/// Matches Dart: `_merge` (map.dart:81).
fn map_merge_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::overloaded_function(
        "merge",
        "",
        vec![
            (
                "$map1, $map2",
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        let map1 = assert_map(&args[0], Some("map1"))?;
                        let map2 = assert_map(&args[1], Some("map2"))?;
                        let mut merged = map1.copy();
                        for (k, v) in map2.entries.iter() {
                            merged.set(*k, *v);
                        }
                        Ok(Value::new_with_arena(arena, ValueKind::Map(merged)))
                    },
                ),
            ),
            (
                "$map1, $args...",
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        let map1 = assert_map(&args[0], Some("map1"))?;
                        let rest_items = args[1].as_list(arena)?;
                        match rest_items.len() {
                            0 => Err(Box::new(SassError::Script {
                                message: "Expected $args to contain a key.".into(),
                                argument_name: None,
                            })),
                            1 => Err(Box::new(SassError::Script {
                                message: "Expected $args to contain a map.".into(),
                                argument_name: None,
                            })),
                            _ => {
                                let keys = &rest_items[..rest_items.len() - 1];
                                let last = &rest_items[rest_items.len() - 1];
                                let map2 = assert_map(last, Some("map2"))?;
                                let modify = move |old: Value<'parse>| -> Value<'parse> {
                                    let nested_map = old.try_map();
                                    match nested_map {
                                        None => Value::new_with_arena(
                                            arena,
                                            ValueKind::Map(map2.clone()),
                                        ),
                                        Some(nested_map) => {
                                            let mut merged = nested_map.copy();
                                            for (k, v) in map2.entries.iter() {
                                                merged.set(*k, *v);
                                            }
                                            Value::new_with_arena(arena, ValueKind::Map(merged))
                                        }
                                    }
                                };
                                Ok(modify_map(arena, &map1, keys, &modify, true))
                            }
                        }
                    },
                ),
            ),
        ],
        arena,
    )
}

/// Creates the `map.remove` callable.
///
/// Overloads: bare `$map` returns the map unchanged (a lone `$map` signature
/// cannot accept zero keys, so this overload covers that case);
/// `$map, $key, $keys...` deletes each top-level key present, ignoring
/// missing keys. Removal is shallow — nested keys need `deep-remove`.
///
/// Matches Dart: `_remove` (map.dart:135).
fn map_remove_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::overloaded_function(
        "remove",
        "",
        vec![
            (
                // Because the signature below has an explicit `$key` argument, it
                // doesn't allow zero keys to be passed. We want to allow that case,
                // so we add an explicit overload for it.
                "$map",
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        Ok(Value::new_with_arena(
                            arena,
                            ValueKind::Map(assert_map(&args[0], Some("map"))?),
                        ))
                    },
                ),
            ),
            (
                // The first argument has special handling so that the $key
                // parameter can be passed by name.
                "$map, $key, $keys...",
                Rc::new(
                    move |_config: &EvalConfig<'compile, 'parse>,
                          _state: &mut EvalState<'compile, 'parse>,
                          args: Vec<Value<'parse>>,
                          _arena: &'compile Bump|
                          -> SassResult<Value<'parse>> {
                        let m = assert_map(&args[0], Some("map"))?;
                        let mut keys: Vec<Value<'parse>> = vec![args[1]];
                        if args.len() > 2 {
                            let kl = args[2].as_list(arena)?;
                            keys.extend(kl);
                        }
                        let mut result = m.copy();
                        for key in &keys {
                            result.delete(key);
                        }
                        Ok(Value::new_with_arena(arena, ValueKind::Map(result)))
                    },
                ),
            ),
        ],
        arena,
    )
}

/// Creates the `map.keys($map)` callable.
///
/// Returns the map's keys as a comma-separated list, in insertion order.
///
/// Matches Dart: `_keys` (map.dart:154).
fn map_keys_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "keys",
        "$map",
        "sass:map",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  _arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let m = assert_map(&args[0], Some("map"))?;
                let mut keys: Vec<Value<'parse>> = Vec::with_capacity(m.len());
                for (k, _) in m.entries.iter() {
                    keys.push(*k);
                }
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::List(SassList::new(keys, ListSeparator::Comma, false)),
                ))
            },
        ),
    )
}

/// Creates the `map.values($map)` callable.
///
/// Returns the map's values as a comma-separated list, in insertion order.
///
/// Matches Dart: `_values` (map.dart:163).
fn map_values_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "values",
        "$map",
        "sass:map",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  _arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let m = assert_map(&args[0], Some("map"))?;
                let mut vals: Vec<Value<'parse>> = Vec::with_capacity(m.len());
                for (_, v) in m.entries.iter() {
                    vals.push(*v);
                }
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::List(SassList::new(vals, ListSeparator::Comma, false)),
                ))
            },
        ),
    )
}

/// Creates the `map.has-key($map, $key, $keys...)` callable.
///
/// Same nested-key walk as `get`, but returns a boolean: `false` when an
/// intermediate key is missing or holds a non-map, otherwise whether the
/// final key is present.
///
/// Matches Dart: `_hasKey` (map.dart:172).
fn map_has_key_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "has-key",
        "$map, $key, $keys...",
        "sass:map",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  _arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let mut m = assert_map(&args[0], Some("map"))?;
                let mut keys: Vec<Value<'parse>> = vec![args[1]];
                if args.len() > 2 {
                    let kl = args[2].as_list(arena)?;
                    keys.extend(kl);
                }
                let last = keys.len() - 1;
                for (i, key) in keys.iter().enumerate() {
                    let v = m.get(key).cloned();
                    if i == last {
                        if v.is_some() {
                            return Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_TRUE)));
                        }
                        return Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)));
                    }
                    let Some(v) = v else {
                        return Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)));
                    };
                    let ValueKind::Map(next) = &*v else {
                        return Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)));
                    };
                    m = next.clone();
                }
                Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)))
            },
        ),
    )
}

/// Creates the `map.deep-merge($map1, $map2)` callable (module-only).
///
/// Returns [`deep_merge_impl`] of the two asserted maps.
///
/// Matches Dart: `_deepMerge` (map.dart:115).
fn deep_merge_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "deep-merge",
        "$map1, $map2",
        "sass:map",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  _arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let map1 = assert_map(&args[0], Some("map1"))?;
                let map2 = assert_map(&args[1], Some("map2"))?;
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Map(deep_merge_impl(arena, &map1, &map2)),
                ))
            },
        ),
    )
}

/// Creates the `map.deep-remove($map, $key, $keys...)` callable (module-only).
///
/// Descends to the parent of the final key via [`modify_map`] with
/// `add_nesting: false` (no-op when the path is missing or non-map), then
/// deletes the final key from that nested map only when present.
///
/// Matches Dart: `_deepRemove` (map.dart:121).
fn deep_remove_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "deep-remove",
        "$map, $key, $keys...",
        "sass:map",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  args: Vec<Value<'parse>>,
                  _arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let m = assert_map(&args[0], Some("map"))?;
                let mut keys: Vec<Value<'parse>> = vec![args[1]];
                if args.len() > 2 {
                    let kl = args[2].as_list(arena)?;
                    keys.extend(kl);
                }
                let last_key = keys[keys.len() - 1];
                let modify = move |val: Value<'parse>| -> Value<'parse> {
                    if let Some(nested_map) = val.try_map() {
                        if nested_map.get(&last_key).is_some() {
                            let mut result = nested_map.copy();
                            result.delete(&last_key);
                            return Value::new_with_arena(arena, ValueKind::Map(result));
                        }
                    }
                    val
                };
                Ok(modify_map(
                    arena,
                    &m,
                    &keys[..keys.len() - 1],
                    &modify,
                    false,
                ))
            },
        ),
    )
}

/// Merges `map1` and `map2`, with values in `map2` taking precedence.
///
/// If both `map1` and `map2` have a map value associated with the same key,
/// this recursively merges those maps as well.
///
/// Matches Go: deepMergeImpl / Dart: _deepMergeImpl
fn deep_merge_impl<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    map1: &SassMap<'parse>,
    map2: &SassMap<'parse>,
) -> SassMap<'parse> {
    if map1.is_empty() {
        return map2.copy();
    }
    if map2.is_empty() {
        return map1.copy();
    }
    let mut result = map1.copy();
    for (key, val) in map2.entries.iter() {
        let existing = result.get(key).cloned();
        match existing {
            Some(existing) => {
                let existing_map = existing.try_map();
                let val_map = val.try_map();
                if let (Some(existing_map), Some(val_map)) = (existing_map, val_map) {
                    let merged = deep_merge_impl(arena, &existing_map, &val_map);
                    // Go compares object identity here (merged == existingMap), as
                    // does Dart via identical(). With owned maps we compare by
                    // value, which is observationally equivalent and matches
                    // Dart's const-canonicalized empty-map semantics.
                    if merged.equals(&existing_map) {
                        continue;
                    }
                    result.set(*key, Value::new_with_arena(arena, ValueKind::Map(merged)));
                } else {
                    result.set(*key, *val);
                }
            }
            None => {
                result.set(*key, *val);
            }
        }
    }
    result
}

/// Updates the specified value in `m` by applying the `modify` callback to
/// it, then returns the resulting map.
///
/// If more than one key is provided, this means the map targeted for update
/// is nested within `m`. The multiple `keys` form a path of nested maps that
/// leads to the targeted value, which is passed to `modify`.
///
/// If any value along the path (other than the last one) is not a map and
/// `add_nesting` is true, this creates nested maps to match `keys` and passes
/// `Value::Null` to `modify`. Otherwise, this fails and returns `m` with no
/// changes.
///
/// If no keys are provided, this passes `m` directly to `modify` and returns
/// the result.
///
/// Matches Go: modifyMap / Dart: _modify
fn modify_map<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    m: &SassMap<'parse>,
    keys: &[Value<'parse>],
    modify: &dyn Fn(Value<'parse>) -> Value<'parse>,
    add_nesting: bool,
) -> Value<'parse> {
    if keys.is_empty() {
        return modify(Value::new_with_arena(arena, ValueKind::Map(m.clone())));
    }
    Value::new_with_arena(
        arena,
        ValueKind::Map(modify_nested_map(arena, m, keys, 0, modify, add_nesting)),
    )
}

/// Matches Go: modifyNestedMap / Dart: _modify.modifyNestedMap
fn modify_nested_map<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    m: &SassMap<'parse>,
    keys: &[Value<'parse>],
    depth: usize,
    modify: &dyn Fn(Value<'parse>) -> Value<'parse>,
    add_nesting: bool,
) -> SassMap<'parse> {
    let mut result = m.copy();
    let key = &keys[depth];
    if depth == keys.len() - 1 {
        let old = match result.get(key) {
            Some(v) => *v,
            None => Value::new_with_arena(arena, ValueKind::Null),
        };
        result.set(*key, modify(old));
        return result;
    }
    let nested_map = result.get(key).and_then(|next| next.try_map());
    if nested_map.is_none() && !add_nesting {
        return result;
    }
    let nested_map = nested_map.unwrap_or_else(SassMap::empty);
    result.set(
        *key,
        Value::new_with_arena(
            arena,
            ValueKind::Map(modify_nested_map(
                arena,
                &nested_map,
                keys,
                depth + 1,
                modify,
                add_nesting,
            )),
        ),
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile_context::new_compile_context;
    use crate::functions::test_utils::invoke_callback;
    use crate::io::VirtualIo;
    use crate::serialize::serialize_value_inspect;
    use crate::value::SassNumber;
    use std::cell::Cell;
    use std::collections::HashSet;

    use crate::common::file_span::BOGUS_SPAN;
    use crate::logger::test_utils::RecordLogger;
    use crate::value::{SassArgumentList, SassString};

    // --- helpers ---

    fn mk_str<'compile: 'parse, 'parse>(arena: &'compile Bump, s: &str) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(s), false)),
        )
    }

    fn mk_num<'compile: 'parse, 'parse>(arena: &'compile Bump, v: f64) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(v, None)))
    }

    fn mk_list<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        sep: ListSeparator,
        contents: Vec<Value<'parse>>,
    ) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::List(SassList::new(contents, sep, false)))
    }

    fn empty_rest<'compile: 'parse, 'parse>(arena: &'compile Bump) -> Value<'parse> {
        mk_list(arena, ListSeparator::Comma, vec![])
    }

    fn mk_map<'parse>(pairs: Vec<(Value<'parse>, Value<'parse>)>) -> SassMap<'parse> {
        let mut m = SassMap::empty();
        for (k, v) in pairs {
            m.set(k, v);
        }
        m
    }

    fn map_val<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        pairs: Vec<(Value<'parse>, Value<'parse>)>,
    ) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::Map(mk_map(pairs)))
    }

    fn inspect(v: &Value<'_>) -> String {
        // Mirrors Go: value.SerializeValueInspect (map_test.go inspect helper).
        serialize_value_inspect(v).unwrap()
    }

    #[rust_sass_macros::maybe_async]
    async fn call_fn<'compile, 'parse>(
        fn_: &BuiltInCallable<'compile, 'parse>,
        arena: &'compile Bump,
        positional: usize,
        names: &[&str],
        args: Vec<Value<'parse>>,
    ) -> SassResult<Value<'parse>>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        call_fn_recorded(fn_, arena, positional, names, args)
            .await
            .0
    }

    /// Like `call_fn`, but returns the RecordLogger so tests can assert
    /// emitted warnings.
    #[rust_sass_macros::maybe_async]
    async fn call_fn_recorded<'compile, 'parse>(
        fn_: &BuiltInCallable<'compile, 'parse>,
        arena: &'compile Bump,
        positional: usize,
        names: &[&str],
        args: Vec<Value<'parse>>,
    ) -> (SassResult<Value<'parse>>, Rc<RecordLogger>)
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let logger = Rc::new(RecordLogger::new());
        let name_set: HashSet<&str> = names.iter().copied().collect();
        let overload = match fn_.callback_for(positional, &name_set) {
            Ok(o) => o,
            Err(e) => return (Err(e), logger),
        };
        let mut state = EvalState::new(arena, BOGUS_SPAN);
        let config = EvalConfig::new(
            arena,
            logger.clone(),
            new_compile_context(),
            Rc::new(VirtualIo::new()),
        );
        let result = invoke_callback(&overload.callback, &config, &mut state, args, arena).await;
        (result, logger)
    }

    #[rust_sass_macros::maybe_async]
    async fn call_fn_ok<'compile, 'parse>(
        fn_: &BuiltInCallable<'compile, 'parse>,
        arena: &'compile Bump,
        positional: usize,
        names: &[&str],
        args: Vec<Value<'parse>>,
    ) -> Value<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        call_fn(fn_, arena, positional, names, args).await.unwrap()
    }

    fn assert_script_err(err: Box<SassError>, want_msg: &str, want_arg: Option<&str>) {
        match *err {
            SassError::Script {
                message,
                argument_name,
            } => {
                assert_eq!(message, want_msg);
                assert_eq!(argument_name.as_deref(), want_arg);
            }
            other => panic!("expected Script error, got {other:?}"),
        }
    }

    fn assert_is_null(got: &Value<'_>) {
        assert!(matches!(&**got, ValueKind::Null), "want Null, got {got:?}");
    }

    fn assert_is_true(got: &Value<'_>) {
        match &**got {
            ValueKind::Boolean(b) => assert!(b.value, "want true, got false"),
            other => panic!("want SassTrue, got {other:?}"),
        }
    }

    fn assert_is_false(got: &Value<'_>) {
        match &**got {
            ValueKind::Boolean(b) => assert!(!b.value, "want false, got true"),
            other => panic!("want SassFalse, got {other:?}"),
        }
    }

    // --- map.get ---

    #[rust_sass_macros::maybe_test]
    async fn test_map_get_top_level() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![
                (mk_str(&arena, "a"), mk_num(&arena, 1.0)),
                (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
            ],
        );
        let got = call_fn_ok(
            &map_get_function(&arena),
            &arena,
            2,
            &[],
            vec![m, mk_str(&arena, "a"), empty_rest(&arena)],
        )
        .await;
        assert_eq!(inspect(&got), "1");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_get_missing_key() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &map_get_function(&arena),
            &arena,
            2,
            &[],
            vec![m, mk_str(&arena, "x"), empty_rest(&arena)],
        )
        .await;
        assert_is_null(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_get_nested() {
        let arena = Bump::new();
        let inner2 = map_val(&arena, vec![(mk_str(&arena, "c"), mk_num(&arena, 3.0))]);
        let inner1 = map_val(&arena, vec![(mk_str(&arena, "b"), inner2)]);
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), inner1)]);
        let got = call_fn_ok(
            &map_get_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![mk_str(&arena, "b"), mk_str(&arena, "c")],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "3");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_get_nested_intermediate_missing() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 1.0))]),
            )],
        );
        let got = call_fn_ok(
            &map_get_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "x"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "b")]),
            ],
        )
        .await;
        assert_is_null(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_get_nested_intermediate_not_a_map() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &map_get_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "b")]),
            ],
        )
        .await;
        assert_is_null(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_get_nested_intermediate_empty_list() {
        let arena = Bump::new();
        // An empty list is NOT a Value::Map for get's type check, so lookup stops.
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Space, vec![]),
            )],
        );
        let got = call_fn_ok(
            &map_get_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "b")]),
            ],
        )
        .await;
        assert_is_null(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_get_nested_last_missing() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 1.0))]),
            )],
        );
        let got = call_fn_ok(
            &map_get_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "x")]),
            ],
        )
        .await;
        assert_is_null(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_get_returns_map_value() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 1.0))]),
            )],
        );
        let got = call_fn_ok(
            &map_get_function(&arena),
            &arena,
            2,
            &[],
            vec![m, mk_str(&arena, "a"), empty_rest(&arena)],
        )
        .await;
        assert!(matches!(&*got, ValueKind::Map(_)), "result should be a map");
        assert_eq!(inspect(&got), "(b: 1)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_get_two_args_only() {
        let arena = Bump::new();
        // Defensive path: len(args) == 2, no rest argument at all.
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &map_get_function(&arena),
            &arena,
            2,
            &[],
            vec![m, mk_str(&arena, "a")],
        )
        .await;
        assert_eq!(inspect(&got), "1");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_get_rest_argument_list() {
        let arena = Bump::new();
        // The rest argument is a SassArgumentList during real evaluation.
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 4.0))]),
            )],
        );
        let rest = Value::new_with_arena(
            &arena,
            ValueKind::ArgumentList(SassArgumentList::new(
                &arena,
                vec![mk_str(&arena, "b")],
                indexmap::IndexMap::new(),
                ListSeparator::Comma,
            )),
        );
        let got = call_fn_ok(
            &map_get_function(&arena),
            &arena,
            3,
            &[],
            vec![m, mk_str(&arena, "a"), rest],
        )
        .await;
        assert_eq!(inspect(&got), "4");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_get_rest_scalar_as_list() {
        let arena = Bump::new();
        // as_list on a scalar returns [self], so the scalar becomes a single key.
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &map_get_function(&arena),
            &arena,
            3,
            &[],
            vec![m, mk_str(&arena, "a"), mk_num(&arena, 5.0)],
        )
        .await;
        assert_is_null(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_get_empty_list_as_map() {
        let arena = Bump::new();
        let got = call_fn_ok(
            &map_get_function(&arena),
            &arena,
            2,
            &[],
            vec![
                mk_list(&arena, ListSeparator::Space, vec![]),
                mk_str(&arena, "a"),
                empty_rest(&arena),
            ],
        )
        .await;
        assert_is_null(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_get_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &map_get_function(&arena),
            &arena,
            2,
            &[],
            vec![mk_num(&arena, 3.0), mk_str(&arena, "a"), empty_rest(&arena)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "3 is not a map.", Some("map"));
    }

    // --- map.set ---

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_three_args_new_key() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &set_function(&arena),
            &arena,
            3,
            &[],
            vec![m, mk_str(&arena, "b"), mk_num(&arena, 2.0)],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 1, b: 2)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_three_args_existing_key() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![
                (mk_str(&arena, "a"), mk_num(&arena, 1.0)),
                (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
            ],
        );
        let got = call_fn_ok(
            &set_function(&arena),
            &arena,
            3,
            &[],
            vec![m, mk_str(&arena, "a"), mk_num(&arena, 3.0)],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 3, b: 2)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_three_args_does_not_mutate_original() {
        let arena = Bump::new();
        let m = mk_map(vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        call_fn_ok(
            &set_function(&arena),
            &arena,
            3,
            &[],
            vec![
                Value::new_with_arena(&arena, ValueKind::Map(m.clone())),
                mk_str(&arena, "a"),
                mk_num(&arena, 9.0),
            ],
        )
        .await;
        assert_eq!(
            inspect(&Value::new_with_arena(&arena, ValueKind::Map(m))),
            "(a: 1)"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_three_args_on_empty_list_map() {
        let arena = Bump::new();
        let got = call_fn_ok(
            &set_function(&arena),
            &arena,
            3,
            &[],
            vec![
                mk_list(&arena, ListSeparator::Space, vec![]),
                mk_str(&arena, "a"),
                mk_num(&arena, 1.0),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 1)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_three_args_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &set_function(&arena),
            &arena,
            3,
            &[],
            vec![
                mk_num(&arena, 3.0),
                mk_str(&arena, "a"),
                mk_num(&arena, 1.0),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "3 is not a map.", Some("map"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_rest_empty() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let err = call_fn(
            &set_function(&arena),
            &arena,
            1,
            &[],
            vec![m, empty_rest(&arena)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Expected $args to contain a key.", None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_rest_one_item() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let err = call_fn(
            &set_function(&arena),
            &arena,
            2,
            &[],
            vec![
                m,
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "a")]),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Expected $args to contain a value.", None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_rest_two_items() {
        // Two rest items: one key plus the value, so a top-level set.
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let fn_ = set_function(&arena);
        let overload = fn_.callback_for(4, &HashSet::new()).unwrap();
        let logger = Rc::new(RecordLogger::new());
        let mut state = EvalState::new(&arena, BOGUS_SPAN);
        let config = EvalConfig::new(
            &arena,
            logger,
            new_compile_context(),
            Rc::new(VirtualIo::new()),
        );
        let got = invoke_callback(
            &overload.callback,
            &config,
            &mut state,
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![mk_str(&arena, "b"), mk_num(&arena, 2.0)],
                ),
            ],
            &arena,
        )
        .await
        .unwrap();
        assert_eq!(inspect(&got), "(a: 1, b: 2)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_rest_nested_create() {
        let arena = Bump::new();
        let m = Value::new_with_arena(&arena, ValueKind::Map(SassMap::empty()));
        let got = call_fn_ok(
            &set_function(&arena),
            &arena,
            4,
            &[],
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        mk_str(&arena, "b"),
                        mk_num(&arena, 1.0),
                    ],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (b: 1))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_rest_nested_existing() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![
                (
                    mk_str(&arena, "a"),
                    map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 1.0))]),
                ),
                (mk_str(&arena, "c"), mk_num(&arena, 2.0)),
            ],
        );
        let got = call_fn_ok(
            &set_function(&arena),
            &arena,
            4,
            &[],
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        mk_str(&arena, "b"),
                        mk_num(&arena, 9.0),
                    ],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (b: 9), c: 2)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_rest_overwrites_non_map_intermediate() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &set_function(&arena),
            &arena,
            4,
            &[],
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        mk_str(&arena, "b"),
                        mk_num(&arena, 2.0),
                    ],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (b: 2))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_rest_deep_create() {
        let arena = Bump::new();
        let m = Value::new_with_arena(&arena, ValueKind::Map(SassMap::empty()));
        let got = call_fn_ok(
            &set_function(&arena),
            &arena,
            5,
            &[],
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        mk_str(&arena, "b"),
                        mk_str(&arena, "c"),
                        mk_num(&arena, 1.0),
                    ],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (b: (c: 1)))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_rest_empty_list_intermediate() {
        let arena = Bump::new();
        // try_map converts an empty list intermediate into an empty map.
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Space, vec![]),
            )],
        );
        let got = call_fn_ok(
            &set_function(&arena),
            &arena,
            4,
            &[],
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        mk_str(&arena, "b"),
                        mk_num(&arena, 1.0),
                    ],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (b: 1))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_set_rest_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &set_function(&arena),
            &arena,
            4,
            &[],
            vec![
                mk_num(&arena, 3.0),
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        mk_str(&arena, "b"),
                        mk_num(&arena, 1.0),
                    ],
                ),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "3 is not a map.", Some("map"));
    }

    // --- map.merge ---

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_basic() {
        let arena = Bump::new();
        let map1 = map_val(
            &arena,
            vec![
                (mk_str(&arena, "a"), mk_num(&arena, 1.0)),
                (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
            ],
        );
        let map2 = map_val(&arena, vec![(mk_str(&arena, "c"), mk_num(&arena, 3.0))]);
        let got = call_fn_ok(
            &map_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![map1, map2],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 1, b: 2, c: 3)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_overlap_keeps_position() {
        let arena = Bump::new();
        let map1 = map_val(
            &arena,
            vec![
                (mk_str(&arena, "a"), mk_num(&arena, 1.0)),
                (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
            ],
        );
        let map2 = map_val(
            &arena,
            vec![
                (mk_str(&arena, "b"), mk_num(&arena, 9.0)),
                (mk_str(&arena, "c"), mk_num(&arena, 3.0)),
            ],
        );
        let got = call_fn_ok(
            &map_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![map1, map2],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 1, b: 9, c: 3)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_empty_first() {
        let arena = Bump::new();
        let got = call_fn_ok(
            &map_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![
                mk_list(&arena, ListSeparator::Space, vec![]),
                map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 1)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_empty_second() {
        let arena = Bump::new();
        let got = call_fn_ok(
            &map_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![
                map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]),
                mk_list(&arena, ListSeparator::Space, vec![]),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 1)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_does_not_mutate_originals() {
        let arena = Bump::new();
        let map1 = mk_map(vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let map2 = mk_map(vec![(mk_str(&arena, "b"), mk_num(&arena, 2.0))]);
        call_fn_ok(
            &map_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![
                Value::new_with_arena(&arena, ValueKind::Map(map1.clone())),
                Value::new_with_arena(&arena, ValueKind::Map(map2.clone())),
            ],
        )
        .await;
        assert_eq!(
            inspect(&Value::new_with_arena(&arena, ValueKind::Map(map1))),
            "(a: 1)"
        );
        assert_eq!(
            inspect(&Value::new_with_arena(&arena, ValueKind::Map(map2))),
            "(b: 2)"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_map1_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &map_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![
                mk_num(&arena, 1.0),
                map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a map.", Some("map1"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_map2_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &map_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![
                map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]),
                mk_num(&arena, 1.0),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a map.", Some("map2"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_rest_empty() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let err = call_fn(
            &map_merge_function(&arena),
            &arena,
            1,
            &[],
            vec![m, empty_rest(&arena)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Expected $args to contain a key.", None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_rest_one_item() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let fn_ = map_merge_function(&arena);
        let overload = fn_.callback_for(3, &HashSet::new()).unwrap();
        let logger = Rc::new(RecordLogger::new());
        let mut state = EvalState::new(&arena, BOGUS_SPAN);
        let config = EvalConfig::new(
            &arena,
            logger,
            new_compile_context(),
            Rc::new(VirtualIo::new()),
        );
        let err = invoke_callback(
            &overload.callback,
            &config,
            &mut state,
            vec![
                m,
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "a")]),
            ],
            &arena,
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Expected $args to contain a map.", None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_rest_nested() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![
                (
                    mk_str(&arena, "a"),
                    map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 1.0))]),
                ),
                (mk_str(&arena, "c"), mk_num(&arena, 2.0)),
            ],
        );
        let got = call_fn_ok(
            &map_merge_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        map_val(&arena, vec![(mk_str(&arena, "b2"), mk_num(&arena, 9.0))]),
                    ],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (b: 1, b2: 9), c: 2)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_rest_nested_overlap() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(
                    &arena,
                    vec![
                        (mk_str(&arena, "x"), mk_num(&arena, 1.0)),
                        (mk_str(&arena, "y"), mk_num(&arena, 2.0)),
                    ],
                ),
            )],
        );
        let got = call_fn_ok(
            &map_merge_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        map_val(&arena, vec![(mk_str(&arena, "y"), mk_num(&arena, 9.0))]),
                    ],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (x: 1, y: 9))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_rest_old_value_not_a_map() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &map_merge_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        map_val(&arena, vec![(mk_str(&arena, "x"), mk_num(&arena, 2.0))]),
                    ],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (x: 2))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_rest_missing_key() {
        let arena = Bump::new();
        let m = Value::new_with_arena(&arena, ValueKind::Map(SassMap::empty()));
        let got = call_fn_ok(
            &map_merge_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        map_val(&arena, vec![(mk_str(&arena, "x"), mk_num(&arena, 2.0))]),
                    ],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (x: 2))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_rest_empty_list_old_value() {
        let arena = Bump::new();
        // try_map converts the empty-list old value to an empty map, which is
        // then merged.
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Space, vec![]),
            )],
        );
        let got = call_fn_ok(
            &map_merge_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        map_val(&arena, vec![(mk_str(&arena, "x"), mk_num(&arena, 2.0))]),
                    ],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (x: 2))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_rest_deep_path() {
        let arena = Bump::new();
        let inner = map_val(
            &arena,
            vec![(
                mk_str(&arena, "b"),
                map_val(&arena, vec![(mk_str(&arena, "x"), mk_num(&arena, 1.0))]),
            )],
        );
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), inner)]);
        let got = call_fn_ok(
            &map_merge_function(&arena),
            &arena,
            4,
            &[],
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        mk_str(&arena, "b"),
                        map_val(&arena, vec![(mk_str(&arena, "y"), mk_num(&arena, 2.0))]),
                    ],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (b: (x: 1, y: 2)))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_rest_last_not_a_map() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let err = call_fn(
            &map_merge_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![mk_str(&arena, "a"), mk_num(&arena, 5.0)],
                ),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "5 is not a map.", Some("map2"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_merge_rest_map1_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &map_merge_function(&arena),
            &arena,
            3,
            &[],
            vec![
                mk_num(&arena, 3.0),
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        Value::new_with_arena(&arena, ValueKind::Map(SassMap::empty())),
                    ],
                ),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "3 is not a map.", Some("map1"));
    }

    // --- map.remove ---

    #[rust_sass_macros::maybe_test]
    async fn test_map_remove_no_keys() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(&map_remove_function(&arena), &arena, 1, &[], vec![m]).await;
        assert_eq!(inspect(&got), "(a: 1)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_remove_no_keys_empty_list() {
        let arena = Bump::new();
        let got = call_fn_ok(
            &map_remove_function(&arena),
            &arena,
            1,
            &[],
            vec![mk_list(&arena, ListSeparator::Space, vec![])],
        )
        .await;
        assert!(matches!(&*got, ValueKind::Map(_)), "result should be a map");
        assert_eq!(inspect(&got), "()");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_remove_one_key() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![
                (mk_str(&arena, "a"), mk_num(&arena, 1.0)),
                (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
            ],
        );
        let got = call_fn_ok(
            &map_remove_function(&arena),
            &arena,
            2,
            &[],
            vec![m, mk_str(&arena, "a"), empty_rest(&arena)],
        )
        .await;
        assert_eq!(inspect(&got), "(b: 2)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_remove_multiple_keys() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![
                (mk_str(&arena, "a"), mk_num(&arena, 1.0)),
                (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
                (mk_str(&arena, "c"), mk_num(&arena, 3.0)),
            ],
        );
        let got = call_fn_ok(
            &map_remove_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "b")]),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(c: 3)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_remove_missing_keys_ignored() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &map_remove_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "x"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "y")]),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 1)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_remove_does_not_mutate_original() {
        let arena = Bump::new();
        let m = mk_map(vec![
            (mk_str(&arena, "a"), mk_num(&arena, 1.0)),
            (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
        ]);
        call_fn_ok(
            &map_remove_function(&arena),
            &arena,
            2,
            &[],
            vec![
                Value::new_with_arena(&arena, ValueKind::Map(m.clone())),
                mk_str(&arena, "a"),
                empty_rest(&arena),
            ],
        )
        .await;
        assert_eq!(
            inspect(&Value::new_with_arena(&arena, ValueKind::Map(m))),
            "(a: 1, b: 2)"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_remove_key_passed_by_name() {
        let arena = Bump::new();
        // The second overload exists so $key can be passed by name.
        let m = map_val(
            &arena,
            vec![
                (mk_str(&arena, "a"), mk_num(&arena, 1.0)),
                (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
            ],
        );
        let got = call_fn_ok(
            &map_remove_function(&arena),
            &arena,
            1,
            &["key"],
            vec![m, mk_str(&arena, "a"), empty_rest(&arena)],
        )
        .await;
        assert_eq!(inspect(&got), "(b: 2)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_remove_no_keys_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &map_remove_function(&arena),
            &arena,
            1,
            &[],
            vec![mk_num(&arena, 3.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "3 is not a map.", Some("map"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_remove_with_keys_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &map_remove_function(&arena),
            &arena,
            2,
            &[],
            vec![mk_num(&arena, 3.0), mk_str(&arena, "a"), empty_rest(&arena)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "3 is not a map.", Some("map"));
    }

    // --- map.keys ---

    #[rust_sass_macros::maybe_test]
    async fn test_map_keys() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![
                (mk_str(&arena, "a"), mk_num(&arena, 1.0)),
                (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
            ],
        );
        let got = call_fn_ok(&map_keys_function(&arena), &arena, 1, &[], vec![m]).await;
        match &*got {
            ValueKind::List(l) => {
                assert_eq!(l.separator, ListSeparator::Comma);
                assert!(!l.has_brackets, "keys list should not have brackets");
            }
            other => panic!("keys should return a list, got {other:?}"),
        }
        assert_eq!(inspect(&got), "a, b");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_keys_single() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(&map_keys_function(&arena), &arena, 1, &[], vec![m]).await;
        assert_eq!(inspect(&got), "(a,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_keys_empty() {
        let arena = Bump::new();
        let got = call_fn_ok(
            &map_keys_function(&arena),
            &arena,
            1,
            &[],
            vec![Value::new_with_arena(
                &arena,
                ValueKind::Map(SassMap::empty()),
            )],
        )
        .await;
        match &*got {
            ValueKind::List(l) => {
                assert_eq!(l.length_as_list(), 0);
                assert_eq!(l.separator, ListSeparator::Comma);
            }
            other => panic!("keys should return a list, got {other:?}"),
        }
        assert_eq!(inspect(&got), "()");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_keys_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &map_keys_function(&arena),
            &arena,
            1,
            &[],
            vec![mk_num(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a map.", Some("map"));
    }

    // --- map.values ---

    #[rust_sass_macros::maybe_test]
    async fn test_map_values() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![
                (mk_str(&arena, "a"), mk_num(&arena, 1.0)),
                (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
            ],
        );
        let got = call_fn_ok(&map_values_function(&arena), &arena, 1, &[], vec![m]).await;
        match &*got {
            ValueKind::List(l) => {
                assert_eq!(l.separator, ListSeparator::Comma);
                assert!(!l.has_brackets, "values list should not have brackets");
            }
            other => panic!("values should return a list, got {other:?}"),
        }
        assert_eq!(inspect(&got), "1, 2");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_values_single() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(&map_values_function(&arena), &arena, 1, &[], vec![m]).await;
        assert_eq!(inspect(&got), "(1,)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_values_empty() {
        let arena = Bump::new();
        let got = call_fn_ok(
            &map_values_function(&arena),
            &arena,
            1,
            &[],
            vec![Value::new_with_arena(
                &arena,
                ValueKind::Map(SassMap::empty()),
            )],
        )
        .await;
        match &*got {
            ValueKind::List(l) => assert_eq!(l.length_as_list(), 0),
            other => panic!("values should return a list, got {other:?}"),
        }
        assert_eq!(inspect(&got), "()");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_values_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &map_values_function(&arena),
            &arena,
            1,
            &[],
            vec![mk_num(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a map.", Some("map"));
    }

    // --- map.has-key ---

    #[rust_sass_macros::maybe_test]
    async fn test_map_has_key_true() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &map_has_key_function(&arena),
            &arena,
            2,
            &[],
            vec![m, mk_str(&arena, "a"), empty_rest(&arena)],
        )
        .await;
        assert_is_true(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_has_key_false() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &map_has_key_function(&arena),
            &arena,
            2,
            &[],
            vec![m, mk_str(&arena, "x"), empty_rest(&arena)],
        )
        .await;
        assert_is_false(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_has_key_null_value() {
        let arena = Bump::new();
        // A key whose value is null still counts as present.
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                Value::new_with_arena(&arena, ValueKind::Null),
            )],
        );
        let got = call_fn_ok(
            &map_has_key_function(&arena),
            &arena,
            2,
            &[],
            vec![m, mk_str(&arena, "a"), empty_rest(&arena)],
        )
        .await;
        assert_is_true(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_has_key_nested_true() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 1.0))]),
            )],
        );
        let got = call_fn_ok(
            &map_has_key_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "b")]),
            ],
        )
        .await;
        assert_is_true(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_has_key_nested_last_missing() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 1.0))]),
            )],
        );
        let got = call_fn_ok(
            &map_has_key_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "x")]),
            ],
        )
        .await;
        assert_is_false(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_has_key_nested_intermediate_missing() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 1.0))]),
            )],
        );
        let got = call_fn_ok(
            &map_has_key_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "x"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "b")]),
            ],
        )
        .await;
        assert_is_false(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_has_key_nested_intermediate_not_a_map() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &map_has_key_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "b")]),
            ],
        )
        .await;
        assert_is_false(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_has_key_nested_intermediate_empty_list() {
        let arena = Bump::new();
        // An empty list is NOT a Value::Map for has-key's type check.
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Space, vec![]),
            )],
        );
        let got = call_fn_ok(
            &map_has_key_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "b")]),
            ],
        )
        .await;
        assert_is_false(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_has_key_two_args_only() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &map_has_key_function(&arena),
            &arena,
            2,
            &[],
            vec![m, mk_str(&arena, "a")],
        )
        .await;
        assert_is_true(&got);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_has_key_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &map_has_key_function(&arena),
            &arena,
            2,
            &[],
            vec![mk_num(&arena, 3.0), mk_str(&arena, "a"), empty_rest(&arena)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "3 is not a map.", Some("map"));
    }

    // --- map.deep-merge ---

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_flat() {
        let arena = Bump::new();
        let map1 = map_val(
            &arena,
            vec![
                (mk_str(&arena, "a"), mk_num(&arena, 1.0)),
                (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
            ],
        );
        let map2 = map_val(
            &arena,
            vec![
                (mk_str(&arena, "b"), mk_num(&arena, 9.0)),
                (mk_str(&arena, "c"), mk_num(&arena, 3.0)),
            ],
        );
        let got = call_fn_ok(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![map1, map2],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 1, b: 9, c: 3)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_empty_map1() {
        let arena = Bump::new();
        let map2 = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![
                Value::new_with_arena(&arena, ValueKind::Map(SassMap::empty())),
                map2,
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 1)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_empty_map2() {
        let arena = Bump::new();
        let map1 = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![
                map1,
                Value::new_with_arena(&arena, ValueKind::Map(SassMap::empty())),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 1)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_both_empty() {
        let arena = Bump::new();
        let got = call_fn_ok(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![
                Value::new_with_arena(&arena, ValueKind::Map(SassMap::empty())),
                Value::new_with_arena(&arena, ValueKind::Map(SassMap::empty())),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "()");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_nested_maps() {
        let arena = Bump::new();
        let map1 = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(
                    &arena,
                    vec![
                        (mk_str(&arena, "x"), mk_num(&arena, 1.0)),
                        (mk_str(&arena, "y"), mk_num(&arena, 2.0)),
                    ],
                ),
            )],
        );
        let map2 = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(
                    &arena,
                    vec![
                        (mk_str(&arena, "y"), mk_num(&arena, 9.0)),
                        (mk_str(&arena, "z"), mk_num(&arena, 3.0)),
                    ],
                ),
            )],
        );
        let got = call_fn_ok(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![map1, map2],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (x: 1, y: 9, z: 3))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_two_levels() {
        let arena = Bump::new();
        let map1 = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(
                    &arena,
                    vec![(
                        mk_str(&arena, "b"),
                        map_val(&arena, vec![(mk_str(&arena, "x"), mk_num(&arena, 1.0))]),
                    )],
                ),
            )],
        );
        let map2 = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(
                    &arena,
                    vec![(
                        mk_str(&arena, "b"),
                        map_val(&arena, vec![(mk_str(&arena, "y"), mk_num(&arena, 2.0))]),
                    )],
                ),
            )],
        );
        let got = call_fn_ok(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![map1, map2],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (b: (x: 1, y: 2)))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_val_empty_map_keeps_existing() {
        let arena = Bump::new();
        // Merging a nested empty map into an existing nested map leaves it
        // unchanged.
        let map1 = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "x"), mk_num(&arena, 1.0))]),
            )],
        );
        let map2 = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                Value::new_with_arena(&arena, ValueKind::Map(SassMap::empty())),
            )],
        );
        let got = call_fn_ok(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![map1, map2],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (x: 1))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_val_empty_list_keeps_existing() {
        let arena = Bump::new();
        // An empty list value converts to an empty map via try_map and merges
        // as one.
        let map1 = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "x"), mk_num(&arena, 1.0))]),
            )],
        );
        let map2 = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Space, vec![]),
            )],
        );
        let got = call_fn_ok(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![map1, map2],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (x: 1))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_existing_not_a_map() {
        let arena = Bump::new();
        let map1 = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let map2 = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "x"), mk_num(&arena, 2.0))]),
            )],
        );
        let got = call_fn_ok(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![map1, map2],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (x: 2))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_val_not_a_map() {
        let arena = Bump::new();
        let map1 = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "x"), mk_num(&arena, 1.0))]),
            )],
        );
        let map2 = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 5.0))]);
        let got = call_fn_ok(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![map1, map2],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 5)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_new_keys_appended() {
        let arena = Bump::new();
        let map1 = map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 2.0))]);
        let map2 = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![map1, map2],
        )
        .await;
        assert_eq!(inspect(&got), "(b: 2, a: 1)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_does_not_mutate_originals() {
        let arena = Bump::new();
        let map1 = mk_map(vec![(
            mk_str(&arena, "a"),
            map_val(&arena, vec![(mk_str(&arena, "x"), mk_num(&arena, 1.0))]),
        )]);
        let map2 = mk_map(vec![(
            mk_str(&arena, "a"),
            map_val(&arena, vec![(mk_str(&arena, "y"), mk_num(&arena, 2.0))]),
        )]);
        call_fn_ok(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![
                Value::new_with_arena(&arena, ValueKind::Map(map1.clone())),
                Value::new_with_arena(&arena, ValueKind::Map(map2.clone())),
            ],
        )
        .await;
        assert_eq!(
            inspect(&Value::new_with_arena(&arena, ValueKind::Map(map1))),
            "(a: (x: 1))"
        );
        assert_eq!(
            inspect(&Value::new_with_arena(&arena, ValueKind::Map(map2))),
            "(a: (y: 2))"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_map1_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![
                mk_num(&arena, 1.0),
                Value::new_with_arena(&arena, ValueKind::Map(SassMap::empty())),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a map.", Some("map1"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_map2_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &deep_merge_function(&arena),
            &arena,
            2,
            &[],
            vec![
                Value::new_with_arena(&arena, ValueKind::Map(SassMap::empty())),
                mk_num(&arena, 1.0),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a map.", Some("map2"));
    }

    // --- deep_merge_impl (internal) ---

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_impl_empty_map1_returns_map2() {
        let arena = Bump::new();
        let map2 = mk_map(vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = deep_merge_impl(&arena, &SassMap::empty(), &map2);
        assert!(
            got.equals(&map2),
            "deep_merge_impl with empty map1 should return map2"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_impl_empty_map2_returns_map1() {
        let arena = Bump::new();
        let map1 = mk_map(vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = deep_merge_impl(&arena, &map1, &SassMap::empty());
        assert!(
            got.equals(&map1),
            "deep_merge_impl with empty map2 should return map1"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_merge_impl_recursive() {
        let arena = Bump::new();
        let map1 = mk_map(vec![
            (
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "x"), mk_num(&arena, 1.0))]),
            ),
            (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
        ]);
        let map2 = mk_map(vec![(
            mk_str(&arena, "a"),
            map_val(&arena, vec![(mk_str(&arena, "y"), mk_num(&arena, 3.0))]),
        )]);
        let got = deep_merge_impl(&arena, &map1, &map2);
        assert_eq!(
            inspect(&Value::new_with_arena(&arena, ValueKind::Map(got))),
            "(a: (x: 1, y: 3), b: 2)"
        );
    }

    // --- map.deep-remove ---

    #[rust_sass_macros::maybe_test]
    async fn test_deep_remove_top_level() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![
                (mk_str(&arena, "a"), mk_num(&arena, 1.0)),
                (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
            ],
        );
        let got = call_fn_ok(
            &deep_remove_function(&arena),
            &arena,
            2,
            &[],
            vec![m, mk_str(&arena, "a"), empty_rest(&arena)],
        )
        .await;
        assert_eq!(inspect(&got), "(b: 2)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_remove_top_level_missing() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &deep_remove_function(&arena),
            &arena,
            2,
            &[],
            vec![m, mk_str(&arena, "x"), empty_rest(&arena)],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 1)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_remove_nested() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(
                    &arena,
                    vec![
                        (mk_str(&arena, "b"), mk_num(&arena, 1.0)),
                        (mk_str(&arena, "c"), mk_num(&arena, 2.0)),
                    ],
                ),
            )],
        );
        let got = call_fn_ok(
            &deep_remove_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "b")]),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (c: 2))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_remove_nested_last_missing() {
        let arena = Bump::new();
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 1.0))]),
            )],
        );
        let got = call_fn_ok(
            &deep_remove_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "x")]),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (b: 1))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_remove_intermediate_not_a_map() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &deep_remove_function(&arena),
            &arena,
            3,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "b")]),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: 1)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_remove_intermediate_path_missing() {
        let arena = Bump::new();
        let m = map_val(&arena, vec![(mk_str(&arena, "x"), mk_num(&arena, 1.0))]);
        let got = call_fn_ok(
            &deep_remove_function(&arena),
            &arena,
            4,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![mk_str(&arena, "b"), mk_str(&arena, "c")],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(x: 1)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_remove_empty_list_intermediate() {
        let arena = Bump::new();
        // try_map turns the empty-list intermediate into an empty map, so the
        // traversal continues and materializes (b: null) along the way.
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Space, vec![]),
            )],
        );
        let got = call_fn_ok(
            &deep_remove_function(&arena),
            &arena,
            4,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![mk_str(&arena, "b"), mk_str(&arena, "c")],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (b: null))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_remove_three_levels() {
        let arena = Bump::new();
        let inner = map_val(
            &arena,
            vec![
                (mk_str(&arena, "c"), mk_num(&arena, 1.0)),
                (mk_str(&arena, "d"), mk_num(&arena, 2.0)),
            ],
        );
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "b"), inner)]),
            )],
        );
        let got = call_fn_ok(
            &deep_remove_function(&arena),
            &arena,
            4,
            &[],
            vec![
                m,
                mk_str(&arena, "a"),
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![mk_str(&arena, "b"), mk_str(&arena, "c")],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&got), "(a: (b: (d: 2)))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_remove_does_not_mutate_original() {
        let arena = Bump::new();
        let m = mk_map(vec![(
            mk_str(&arena, "a"),
            map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 1.0))]),
        )]);
        call_fn_ok(
            &deep_remove_function(&arena),
            &arena,
            3,
            &[],
            vec![
                Value::new_with_arena(&arena, ValueKind::Map(m.clone())),
                mk_str(&arena, "a"),
                mk_list(&arena, ListSeparator::Comma, vec![mk_str(&arena, "b")]),
            ],
        )
        .await;
        assert_eq!(
            inspect(&Value::new_with_arena(&arena, ValueKind::Map(m))),
            "(a: (b: 1))"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_deep_remove_not_a_map() {
        let arena = Bump::new();
        let err = call_fn(
            &deep_remove_function(&arena),
            &arena,
            2,
            &[],
            vec![mk_num(&arena, 3.0), mk_str(&arena, "a"), empty_rest(&arena)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "3 is not a map.", Some("map"));
    }

    // --- modify_map (internal) ---

    #[rust_sass_macros::maybe_test]
    async fn test_modify_map_no_keys() {
        let arena = Bump::new();
        let m = mk_map(vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let modify = |old: Value<'_>| -> Value<'_> {
            assert!(
                matches!(&*old, ValueKind::Map(_)),
                "modify should receive a map, got {old:?}"
            );
            assert_eq!(inspect(&old), "(a: 1)");
            mk_num(&arena, 42.0)
        };
        let got = modify_map(&arena, &m, &[], &modify, true);
        assert_eq!(inspect(&got), "42");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_modify_map_single_key_existing() {
        let arena = Bump::new();
        let m = mk_map(vec![
            (mk_str(&arena, "a"), mk_num(&arena, 1.0)),
            (mk_str(&arena, "b"), mk_num(&arena, 2.0)),
        ]);
        let modify = |old: Value<'_>| -> Value<'_> {
            assert_eq!(inspect(&old), "1");
            mk_num(&arena, 9.0)
        };
        let got = modify_map(&arena, &m, &[mk_str(&arena, "a")], &modify, true);
        assert_eq!(inspect(&got), "(a: 9, b: 2)");
        assert_eq!(
            inspect(&Value::new_with_arena(&arena, ValueKind::Map(m))),
            "(a: 1, b: 2)"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_modify_map_single_key_missing() {
        let arena = Bump::new();
        let m = mk_map(vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let modify = |old: Value<'_>| -> Value<'_> {
            assert!(
                matches!(&*old, ValueKind::Null),
                "modify should receive Null for a missing key"
            );
            mk_num(&arena, 9.0)
        };
        let got = modify_map(&arena, &m, &[mk_str(&arena, "x")], &modify, true);
        assert_eq!(inspect(&got), "(a: 1, x: 9)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_modify_map_nested_add_nesting() {
        let arena = Bump::new();
        let m = SassMap::empty();
        let modify = |old: Value<'_>| -> Value<'_> {
            assert!(
                matches!(&*old, ValueKind::Null),
                "modify should receive Null"
            );
            mk_num(&arena, 1.0)
        };
        let got = modify_map(
            &arena,
            &m,
            &[mk_str(&arena, "a"), mk_str(&arena, "b")],
            &modify,
            true,
        );
        assert_eq!(inspect(&got), "(a: (b: 1))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_modify_map_nested_existing_path() {
        let arena = Bump::new();
        let m = mk_map(vec![(
            mk_str(&arena, "a"),
            map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 1.0))]),
        )]);
        let modify = |old: Value<'_>| -> Value<'_> {
            assert_eq!(inspect(&old), "1");
            mk_num(&arena, 9.0)
        };
        let got = modify_map(
            &arena,
            &m,
            &[mk_str(&arena, "a"), mk_str(&arena, "b")],
            &modify,
            true,
        );
        assert_eq!(inspect(&got), "(a: (b: 9))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_modify_map_nested_non_map_intermediate_add_nesting() {
        let arena = Bump::new();
        let m = mk_map(vec![(mk_str(&arena, "a"), mk_num(&arena, 5.0))]);
        let modify = |_old: Value<'_>| -> Value<'_> { mk_num(&arena, 9.0) };
        let got = modify_map(
            &arena,
            &m,
            &[mk_str(&arena, "a"), mk_str(&arena, "b")],
            &modify,
            true,
        );
        assert_eq!(inspect(&got), "(a: (b: 9))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_modify_map_nested_missing_no_nesting() {
        let arena = Bump::new();
        let m = mk_map(vec![(mk_str(&arena, "x"), mk_num(&arena, 1.0))]);
        let called = Cell::new(false);
        let modify = |_old: Value<'_>| -> Value<'_> {
            called.set(true);
            mk_num(&arena, 9.0)
        };
        let got = modify_map(
            &arena,
            &m,
            &[mk_str(&arena, "a"), mk_str(&arena, "b")],
            &modify,
            false,
        );
        assert!(
            !called.get(),
            "modify should not be called when nesting is missing and add_nesting is false"
        );
        assert_eq!(inspect(&got), "(x: 1)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_modify_map_nested_non_map_intermediate_no_nesting() {
        let arena = Bump::new();
        let m = mk_map(vec![(mk_str(&arena, "a"), mk_num(&arena, 5.0))]);
        let called = Cell::new(false);
        let modify = |_old: Value<'_>| -> Value<'_> {
            called.set(true);
            mk_num(&arena, 9.0)
        };
        let got = modify_map(
            &arena,
            &m,
            &[mk_str(&arena, "a"), mk_str(&arena, "b")],
            &modify,
            false,
        );
        assert!(
            !called.get(),
            "modify should not be called for a non-map intermediate with add_nesting false"
        );
        assert_eq!(inspect(&got), "(a: 5)");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_modify_map_empty_list_intermediate_no_nesting() {
        let arena = Bump::new();
        // try_map converts an empty list to an empty map even when add_nesting
        // is false, so traversal continues.
        let m = mk_map(vec![(
            mk_str(&arena, "a"),
            mk_list(&arena, ListSeparator::Space, vec![]),
        )]);
        let modify = |_old: Value<'_>| -> Value<'_> { mk_num(&arena, 9.0) };
        let got = modify_map(
            &arena,
            &m,
            &[mk_str(&arena, "a"), mk_str(&arena, "b")],
            &modify,
            false,
        );
        assert_eq!(inspect(&got), "(a: (b: 9))");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_modify_map_deep_nesting() {
        let arena = Bump::new();
        let m = SassMap::empty();
        let modify = |_old: Value<'_>| -> Value<'_> { mk_num(&arena, 9.0) };
        let got = modify_map(
            &arena,
            &m,
            &[
                mk_str(&arena, "a"),
                mk_str(&arena, "b"),
                mk_str(&arena, "c"),
            ],
            &modify,
            true,
        );
        assert_eq!(inspect(&got), "(a: (b: (c: 9)))");
    }

    // --- global_map_functions ---

    #[rust_sass_macros::maybe_test]
    async fn test_global_map_functions_names() {
        let arena = Bump::new();
        let fns = global_map_functions(&arena);
        let want = [
            "map-get",
            "map-merge",
            "map-remove",
            "map-keys",
            "map-values",
            "map-has-key",
        ];
        assert_eq!(fns.len(), want.len());
        for (i, f) in fns.iter().enumerate() {
            assert_eq!(f.name(), want[i], "fns[{i}]");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_map_functions_deprecation_metadata() {
        let arena = Bump::new();
        // Each wrapped global stores (module, name) metadata for
        // introspection; the warning itself is emitted by the wrapped
        // callback (see the emission tests above).
        let fns = global_map_functions(&arena);
        let want = [
            ("map-get", "get"),
            ("map-merge", "merge"),
            ("map-remove", "remove"),
            ("map-keys", "keys"),
            ("map-values", "values"),
            ("map-has-key", "has-key"),
        ];
        for (i, f) in fns.iter().enumerate() {
            match f.kind() {
                CallableKind::BuiltIn(b) => {
                    assert_eq!(b.name(), want[i].0);
                    let dw = b
                        .deprecation_warning()
                        .expect("deprecation warning should be set");
                    assert_eq!(dw.0, "map");
                    assert_eq!(dw.1, want[i].1);
                }
                _ => panic!("expected BuiltIn callable"),
            }
        }
    }

    fn assert_global_builtin_warning(logger: &RecordLogger, module_fn: &str) {
        let messages = logger.messages();
        assert_eq!(messages.len(), 1, "warnings = {messages:?}, want 1");
        let want = [
            "Global built-in functions are deprecated and will be removed in Dart Sass 3.0.0.",
            &format!("Use {module_fn} instead."),
            "",
            "More info and automated migrator: https://sass-lang.com/d/import",
        ]
        .join("\n");
        assert_eq!(messages[0], want);
        assert_eq!(
            logger.deprecation_ids()[0].as_deref(),
            Some("global-builtin")
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_map_get_emits_deprecation_warning() {
        let arena = Bump::new();
        // Mirrors Go: TestGlobalMapGetEmitsDeprecationWarning.
        let fns = global_map_functions(&arena);
        let CallableKind::BuiltIn(map_get) = fns[0].kind() else {
            panic!("expected BuiltIn callable");
        };
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let (result, logger) =
            call_fn_recorded(map_get, &arena, 2, &[], vec![m, mk_str(&arena, "a")]).await;
        assert_eq!(inspect(&result.unwrap()), "1");
        assert_global_builtin_warning(&logger, "map.get");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_map_has_key_emits_deprecation_warning() {
        let arena = Bump::new();
        // Mirrors Go: TestGlobalMapHasKeyEmitsDeprecationWarning.
        let fns = global_map_functions(&arena);
        let CallableKind::BuiltIn(has_key) = fns[5].kind() else {
            panic!("expected BuiltIn callable");
        };
        let m = map_val(&arena, vec![(mk_str(&arena, "a"), mk_num(&arena, 1.0))]);
        let (result, logger) =
            call_fn_recorded(has_key, &arena, 2, &[], vec![m, mk_str(&arena, "a")]).await;
        assert_is_true(&result.unwrap());
        assert_global_builtin_warning(&logger, "map.has-key");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_map_merge_rest_overload_emits_deprecation_warning() {
        let arena = Bump::new();
        // Mirrors Go: TestGlobalMapMergeRestOverloadEmitsDeprecationWarning —
        // the wrapper wraps every overload, including the rest overload.
        let fns = global_map_functions(&arena);
        let CallableKind::BuiltIn(map_merge) = fns[1].kind() else {
            panic!("expected BuiltIn callable");
        };
        let m = map_val(
            &arena,
            vec![(
                mk_str(&arena, "a"),
                map_val(&arena, vec![(mk_str(&arena, "b"), mk_num(&arena, 1.0))]),
            )],
        );
        let (result, logger) = call_fn_recorded(
            map_merge,
            &arena,
            3,
            &[],
            vec![
                m,
                mk_list(
                    &arena,
                    ListSeparator::Comma,
                    vec![
                        mk_str(&arena, "a"),
                        map_val(&arena, vec![(mk_str(&arena, "c"), mk_num(&arena, 2.0))]),
                    ],
                ),
            ],
        )
        .await;
        assert_eq!(inspect(&result.unwrap()), "(a: (b: 1, c: 2))");
        assert_global_builtin_warning(&logger, "map.merge");
    }

    // --- map_module ---

    #[rust_sass_macros::maybe_test]
    async fn test_map_module_url() {
        let arena = Bump::new();
        let m = map_module(&arena);
        assert_eq!(m.url, "sass:map");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_module_functions() {
        let arena = Bump::new();
        let m = map_module(&arena);
        let want = [
            "get",
            "set",
            "merge",
            "remove",
            "keys",
            "values",
            "has-key",
            "deep-merge",
            "deep-remove",
        ];
        assert_eq!(m.functions.len(), want.len());
        for (i, name) in m.functions.keys().enumerate() {
            assert_eq!(name.as_str(), want[i], "functions[{i}]");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_module_no_mixins_no_variables() {
        let arena = Bump::new();
        let m = map_module(&arena);
        assert_eq!(m.mixins.len(), 0);
        assert_eq!(m.variables.len(), 0);
    }
}
