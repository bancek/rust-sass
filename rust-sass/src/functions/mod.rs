// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/functions.dart
// go-source: go/functions/functions.go

use std::rc::Rc;

use bumpalo::Bump;

use crate::callable::{BuiltInCallable, Callable, CallableKind};
use crate::module::BuiltInModule;
use crate::value::Value;

pub mod color;
pub mod color_adjust;
pub mod color_helpers;
pub mod color_manipulation;
pub mod color_spaces;
pub mod color_spaces_shared;
pub mod disallowed;
pub mod helpers;
pub mod list;
pub mod map;
pub mod math;
pub mod meta;
pub mod selector;
pub mod string;

#[cfg(test)]
pub(crate) mod test_utils;

/// Sass core functions that are globally available.
///
/// This excludes a few functions that need access to the evaluation context;
/// those are registered on the evaluator itself (see `eval/meta.rs`).
pub fn global_functions<'compile, 'parse>(arena: &'compile Bump) -> Vec<Callable<'compile, 'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let mut result: Vec<Callable<'compile, 'parse>> = Vec::new();
    result.extend(color::global_color_functions(arena));
    result.extend(list::global_list_functions(arena));
    result.extend(map::global_map_functions(arena));
    result.extend(math::global_math_functions(arena));
    result.extend(selector::global_selector_functions(arena));
    result.extend(string::global_string_functions(arena));
    result.extend(meta::shared_meta_functions(arena));
    // This is only invoked using `call()`. Hand-authored `if()`s are parsed
    // as `LegacyIfExpression`s (see `ast/sass/expression/legacy_if.rs`).
    result.push(if_function(arena));
    result
}

/// Sass's core library modules.
///
/// This doesn't include the `sass:meta` module: the evaluator registers
/// extra members needing runtime state (see [`crate::functions::meta`] and
/// `eval/meta.rs`), so there is no standalone `meta` entry here.
pub fn core_modules<'compile, 'parse>(arena: &'compile Bump) -> Vec<BuiltInModule<'compile, 'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    vec![
        color::color_module(arena),
        list::list_module(arena),
        map::map_module(arena),
        math::math_module(arena),
        selector::selector_module(arena),
        string::string_module(arena),
    ]
}

/// The built-in `if()` function (`$condition, $if-true, $if-false`).
///
/// Only reachable through `call()` (or `meta.get-function("if")`): the parser
/// turns hand-authored `if()`s into `LegacyIfExpression`s handled directly by
/// the evaluator, which can skip evaluating the unused branch. See
/// `functions/README.md` in Dart Sass.
fn if_function<'compile, 'parse>(arena: &'compile Bump) -> Callable<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    Callable::new(
        arena,
        CallableKind::BuiltIn(BuiltInCallable::function(
            "if",
            "$condition, $if-true, $if-false",
            "",
            arena,
            Rc::new(
                |_config, _state, args: Vec<Value<'parse>>, _arena: &'compile Bump| {
                    if args[0].is_truthy() {
                        Ok(args[1])
                    } else {
                        Ok(args[2])
                    }
                },
            ),
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;

    use crate::functions::test_utils::eval;
    use crate::value::{SassNumber, SassString, Value, ValueKind, SASS_FALSE, SASS_TRUE};

    fn str_val<'compile: 'parse, 'parse>(arena: &'compile Bump, s: &str) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(s), false)),
        )
    }

    // --- global_functions ---

    #[rust_sass_macros::maybe_test]
    async fn test_global_functions_contains_all_packages() {
        let arena = Bump::new();
        let names: HashSet<String> = global_functions(&arena)
            .iter()
            .map(|f| f.name().to_string())
            .collect();
        // One representative per source list, plus if().
        for name in [
            "red",            // color
            "nth",            // list
            "map-get",        // map
            "percentage",     // math
            "selector-parse", // selector
            "str-length",     // string
            "inspect",        // meta
            "if",             // hand-authored if()
        ] {
            assert!(
                names.contains(name),
                "global_functions() should contain {name:?}"
            );
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_functions_if_is_last() {
        let arena = Bump::new();
        let fns = global_functions(&arena);
        assert_eq!(fns[fns.len() - 1].name(), "if");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_if_function() {
        let arena = Bump::new();
        let fns = global_functions(&arena);
        let CallableKind::BuiltIn(if_fn) = fns[fns.len() - 1].kind() else {
            panic!("expected BuiltIn callable");
        };
        let if_true = str_val(&arena, "yes");
        let if_false = str_val(&arena, "no");

        let got = eval(
            &arena,
            if_fn,
            &[
                Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE)),
                if_true,
                if_false,
            ],
        )
        .await
        .unwrap();
        assert!(
            std::ptr::eq(&*got as *const _, &*if_true as *const _),
            "if(true, ...) should return $if-true"
        );

        let got = eval(
            &arena,
            if_fn,
            &[
                Value::new_with_arena(&arena, ValueKind::Boolean(SASS_FALSE)),
                if_true,
                if_false,
            ],
        )
        .await
        .unwrap();
        assert!(
            std::ptr::eq(&*got as *const _, &*if_false as *const _),
            "if(false, ...) should return $if-false"
        );

        let got = eval(
            &arena,
            if_fn,
            &[
                Value::new_with_arena(&arena, ValueKind::Null),
                if_true,
                if_false,
            ],
        )
        .await
        .unwrap();
        assert!(
            std::ptr::eq(&*got as *const _, &*if_false as *const _),
            "if(null, ...) should return $if-false"
        );

        // Any non-false, non-null value is truthy.
        let got = eval(
            &arena,
            if_fn,
            &[
                Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(0.0, None))),
                if_true,
                if_false,
            ],
        )
        .await
        .unwrap();
        assert!(
            std::ptr::eq(&*got as *const _, &*if_true as *const _),
            "if(0, ...) should return $if-true"
        );
    }

    // --- core_modules ---

    #[rust_sass_macros::maybe_test]
    async fn test_core_modules() {
        let arena = Bump::new();
        let modules = core_modules(&arena);
        let want = [
            "sass:color",
            "sass:list",
            "sass:map",
            "sass:math",
            "sass:selector",
            "sass:string",
        ];
        assert_eq!(modules.len(), want.len());
        for (i, m) in modules.iter().enumerate() {
            assert_eq!(m.url, want[i], "modules[{i}]");
        }
    }
}
