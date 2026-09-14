// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/functions/list.dart
// go-source: go/functions/list.go

use crate::value::SassNumber;
use std::rc::Rc;

use crate::callable::{BuiltInCallable, Callable, CallableKind};
use crate::common::exception::{SassError, SassResult};
use crate::eval::warn::flush_buffered_warnings;
use crate::logger::BufferedWarnLogger;
use crate::module::BuiltInModule;
use crate::value::{
    assert_string, sass_index_to_list_index, ListSeparator, SassList, SassString, Value, ValueKind,
    SASS_FALSE, SASS_TRUE,
};
use bumpalo::Bump;

/// The global definitions of Sass list functions.
pub fn global_list_functions<'compile, 'parse>(
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
            length_function(arena).with_deprecation_warning("list", None)
        ),
        c!(
            arena,
            nth_function(arena).with_deprecation_warning("list", None)
        ),
        c!(
            arena,
            set_nth_function(arena).with_deprecation_warning("list", None)
        ),
        c!(
            arena,
            join_function(arena).with_deprecation_warning("list", None)
        ),
        c!(
            arena,
            append_function(arena).with_deprecation_warning("list", None)
        ),
        c!(
            arena,
            zip_function(arena).with_deprecation_warning("list", None)
        ),
        c!(
            arena,
            index_function(arena).with_deprecation_warning("list", None)
        ),
        c!(
            arena,
            is_bracketed_function(arena).with_deprecation_warning("list", None)
        ),
        c!(
            arena,
            separator_function(arena)
                .with_deprecation_warning("list", None)
                .with_name("list-separator".into())
        ),
    ]
}

/// The Sass list module.
pub fn list_module<'compile, 'parse>(arena: &'compile Bump) -> BuiltInModule<'compile, 'parse>
where
    'compile: 'parse,
{
    macro_rules! c {
        ($arena:expr, $f:expr) => {
            Callable::new($arena, CallableKind::BuiltIn($f))
        };
    }
    let fns: Vec<Callable<'compile, 'parse>> = vec![
        c!(arena, length_function(arena)),
        c!(arena, nth_function(arena)),
        c!(arena, set_nth_function(arena)),
        c!(arena, join_function(arena)),
        c!(arena, append_function(arena)),
        c!(arena, zip_function(arena)),
        c!(arena, index_function(arena)),
        c!(arena, is_bracketed_function(arena)),
        c!(arena, separator_function(arena)),
        c!(arena, slash_function(arena)),
    ];
    BuiltInModule::new(arena, "list".into(), &fns, &[], indexmap::IndexMap::new())
}

fn length_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
// Each `*_function` below passes `"sass:list"` as the URL, like
// `BuiltInCallable::function` with the URL fixed (Dart's `_function` helper
// in functions/list.dart, inlined at each call site here).
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "length",
        "$list",
        "sass:list",
        arena,
        Rc::new(move |_config, _state, args, arena: &'compile Bump| {
            Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(SassNumber::new(args[0].length_as_list() as f64, None)),
            ))
        }),
    )
}

fn nth_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "nth",
        "$list, $n",
        "sass:list",
        arena,
        Rc::new(move |config, state, args, arena: &'compile Bump| {
            let a = args[0];
            let warn_buf = BufferedWarnLogger::new(arena);
            let index = sass_index_to_list_index(&a, &args[1], "n", &warn_buf)?;
            flush_buffered_warnings(&warn_buf, config, state)?;
            let list = a.as_list(arena)?;
            Ok(list[index])
        }),
    )
}

fn set_nth_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "set-nth",
        "$list, $n, $value",
        "sass:list",
        arena,
        Rc::new(
            move |config, state, args: Vec<Value<'parse>>, arena: &'compile Bump| {
                let a = args[0];
                let warn_buf = BufferedWarnLogger::new(arena);
                let index = sass_index_to_list_index(&a, &args[1], "n", &warn_buf)?;
                flush_buffered_warnings(&warn_buf, config, state)?;
                let list = a.as_list(arena)?;
                let mut v: Vec<Value<'_>> = list.to_vec();
                v[index] = args[2];
                let sep = a.separator();
                let bracketed = a.has_brackets();
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::List(SassList::new(v, sep, bracketed)),
                ))
            },
        ),
    )
}

fn join_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "join",
        "$list1, $list2, $separator: auto, $bracketed: auto",
        "sass:list",
        arena,
        Rc::new(
            move |_config,
                  _state,
                  args: Vec<Value<'parse>>,
                  arena: &'compile Bump|
                  -> SassResult<Value<'parse>> {
                let a0 = args[0];
                let a1 = args[1];
                let l1 = a0.as_list(arena)?;
                let l2 = a1.as_list(arena)?;
                let sep = join_separator(&a0, &a1, args.get(2))?;
                let bracketed = join_bracketed(&a0, args.get(3));
                let mut v = Vec::with_capacity(l1.len() + l2.len());
                v.extend(l1.iter().cloned());
                v.extend(l2.iter().cloned());
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::List(SassList::new(v, sep, bracketed)),
                ))
            },
        ),
    )
}

fn join_separator(
    a: &Value<'_>,
    b: &Value<'_>,
    sep: Option<&Value<'_>>,
) -> SassResult<ListSeparator> {
    // Dart `assertString` throws on explicit null (`$separator: null is not
    // a string`) — only an OMITTED separator falls back to auto. Through the
    // real call path the omitted arg is default-filled to `auto` before the
    // callback runs, so `None`/missing here also means auto; only
    // `Some(Null)` is the explicit-null form and it throws.
    if let Some(s) = sep {
        // Explicit null throws via the shared assert (Dart `assertString`):
        // `$separator: null is not a string.` — only omitted (None) is auto.
        let t: &str = assert_string(s, Some("separator"))?.text;
        return match t {
            "auto" => {
                let (s1, s2) = (a.separator(), b.separator());
                Ok(
                    if s1 == ListSeparator::Undecided && s2 == ListSeparator::Undecided {
                        ListSeparator::Space
                    } else if s1 == ListSeparator::Undecided {
                        s2
                    } else {
                        s1
                    },
                )
            }
            "space" => Ok(ListSeparator::Space),
            "comma" => Ok(ListSeparator::Comma),
            "slash" => Ok(ListSeparator::Slash),
            _ => Err(Box::new(SassError::Script {
                message: r#"Must be "space", "comma", "slash", or "auto"."#.into(),
                argument_name: Some("separator".into()),
            })),
        };
    }
    let (s1, s2) = (a.separator(), b.separator());
    Ok(
        if s1 == ListSeparator::Undecided && s2 == ListSeparator::Undecided {
            ListSeparator::Space
        } else if s1 == ListSeparator::Undecided {
            s2
        } else {
            s1
        },
    )
}

fn join_bracketed(a: &Value<'_>, arg: Option<&Value<'_>>) -> bool {
    if let Some(v) = arg {
        if let Ok(s) = assert_string(v, None) {
            if s.text == "auto" {
                return a.has_brackets();
            }
        }
        v.is_truthy()
    } else {
        a.has_brackets()
    }
}

fn append_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "append",
        "$list, $val, $separator: auto",
        "sass:list",
        arena,
        Rc::new(
            move |_config, _state, args: Vec<Value<'parse>>, arena: &'compile Bump| {
                let a = args[0];
                let mut v: Vec<Value<'_>> = a.as_list(arena)?;
                let sep = append_separator(&a, args.get(2))?;
                v.push(args[1]);
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::List(SassList::new(v, sep, a.has_brackets())),
                ))
            },
        ),
    )
}

fn append_separator(a: &Value<'_>, sep: Option<&Value<'_>>) -> SassResult<ListSeparator> {
    // Same null discipline as `join_separator`: explicit null throws.
    if let Some(s) = sep {
        // Same null discipline as `join_separator`: explicit null throws
        // via the shared assert; omitted (None) is auto.
        let t: &str = assert_string(s, Some("separator"))?.text;
        return match t {
            "auto" => {
                let s1 = a.separator();
                Ok(if s1 == ListSeparator::Undecided {
                    ListSeparator::Space
                } else {
                    s1
                })
            }
            "space" => Ok(ListSeparator::Space),
            "comma" => Ok(ListSeparator::Comma),
            "slash" => Ok(ListSeparator::Slash),
            _ => Err(Box::new(SassError::Script {
                message: r#"Must be "space", "comma", "slash", or "auto"."#.into(),
                argument_name: Some("separator".into()),
            })),
        };
    }
    let s1 = a.separator();
    Ok(if s1 == ListSeparator::Undecided {
        ListSeparator::Space
    } else {
        s1
    })
}

fn zip_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "zip",
        "$lists...",
        "sass:list",
        arena,
        Rc::new(
            move |_config, _state, args, arena: &'compile Bump| -> SassResult<Value<'parse>> {
                let a = args[0];
                let lists = a.as_list(arena)?;
                if lists.is_empty() {
                    return Ok(Value::new_with_arena(
                        arena,
                        ValueKind::List(SassList::empty(ListSeparator::Comma, false)),
                    ));
                }
                let mut sub_lists: Vec<Vec<Value<'_>>> = Vec::new();
                for v in &lists {
                    sub_lists.push(v.as_list(arena)?);
                }
                let mut results: Vec<Value<'_>> = Vec::new();
                let mut i = 0;
                loop {
                    if !sub_lists.iter().all(|l| i < l.len()) {
                        break;
                    }
                    let entry: Vec<Value<'_>> = sub_lists.iter().map(|l| l[i]).collect();
                    results.push(Value::new_with_arena(
                        arena,
                        ValueKind::List(SassList::new(entry, ListSeparator::Space, false)),
                    ));
                    i += 1;
                }
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::List(SassList::new(results, ListSeparator::Comma, false)),
                ))
            },
        ),
    )
}

fn index_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "index",
        "$list, $value",
        "sass:list",
        arena,
        Rc::new(
            move |_config, _state, args: Vec<Value<'parse>>, arena: &'compile Bump| {
                let a = args[0];
                let list = a.as_list(arena)?;
                for (i, item) in list.iter().enumerate() {
                    if item == &args[1] {
                        return Ok(Value::new_with_arena(
                            arena,
                            ValueKind::Number(SassNumber::new((i + 1) as f64, None)),
                        ));
                    }
                }
                Ok(Value::new_with_arena(arena, ValueKind::Null))
            },
        ),
    )
}

fn is_bracketed_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "is-bracketed",
        "$list",
        "sass:list",
        arena,
        Rc::new(
            move |_config, _state, args: Vec<Value<'parse>>, arena: &'compile Bump| {
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Boolean(if args[0].has_brackets() {
                        SASS_TRUE
                    } else {
                        SASS_FALSE
                    }),
                ))
            },
        ),
    )
}

fn separator_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "separator",
        "$list",
        "sass:list",
        arena,
        Rc::new(
            move |_config, _state, args: Vec<Value<'parse>>, arena: &'compile Bump| {
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::String(SassString::new(
                        arena.alloc_str(match args[0].separator() {
                            ListSeparator::Comma => "comma",
                            ListSeparator::Slash => "slash",
                            _ => "space",
                        }),
                        false,
                    )),
                ))
            },
        ),
    )
}

fn slash_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "slash",
        "$elements...",
        "sass:list",
        arena,
        Rc::new(
            move |_config, _state, args: Vec<Value<'parse>>, arena: &'compile Bump| {
                let a = args[0];
                let list = a.as_list(arena)?;
                if list.len() < 2 {
                    return Err(Box::new(SassError::Script {
                        message: "At least two elements are required.".into(),
                        argument_name: None,
                    }));
                }
                let v: Vec<Value<'_>> = list.to_vec();
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::List(SassList::new(v, ListSeparator::Slash, false)),
                ))
            },
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::SassNumber;

    use crate::functions::test_utils::*;

    fn num<'compile: 'parse, 'parse>(arena: &'compile Bump, v: f64) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(v, None)))
    }

    fn s<'compile: 'parse, 'parse>(arena: &'compile Bump, s: &str) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(s), false)),
        )
    }

    fn comma_list<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        items: &[Value<'parse>],
    ) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::List(SassList::new(items.to_vec(), ListSeparator::Comma, false)),
        )
    }

    fn space_list<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        items: &[Value<'parse>],
    ) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::List(SassList::new(items.to_vec(), ListSeparator::Space, false)),
        )
    }

    fn bracketed_list<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        items: &[Value<'parse>],
    ) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::List(SassList::new(items.to_vec(), ListSeparator::Space, true)),
        )
    }

    fn slash_list<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        items: &[Value<'parse>],
    ) -> Value<'parse> {
        Value::new_with_arena(
            arena,
            ValueKind::List(SassList::new(items.to_vec(), ListSeparator::Slash, false)),
        )
    }

    #[rust_sass_macros::maybe_test]
    async fn test_length() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &length_function(&arena),
            &[comma_list(
                &arena,
                &[num(&arena, 1.0), num(&arena, 2.0), num(&arena, 3.0)],
            )],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Number(n) => assert_eq!(n.value, 3.0),
            _ => panic!("expected number"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_length_empty() {
        let arena = Bump::new();
        let v = eval(&arena, &length_function(&arena), &[comma_list(&arena, &[])])
            .await
            .unwrap();
        match &*v {
            ValueKind::Number(n) => assert_eq!(n.value, 0.0),
            _ => panic!("expected number"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nth() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &nth_function(&arena),
            &[
                comma_list(
                    &arena,
                    &[num(&arena, 10.0), num(&arena, 20.0), num(&arena, 30.0)],
                ),
                num(&arena, 2.0),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Number(n) => assert_eq!(n.value, 20.0),
            _ => panic!("expected number"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nth_negative() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &nth_function(&arena),
            &[
                comma_list(
                    &arena,
                    &[num(&arena, 10.0), num(&arena, 20.0), num(&arena, 30.0)],
                ),
                num(&arena, -1.0),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Number(n) => assert_eq!(n.value, 30.0),
            _ => panic!("expected number"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nth_out_of_bounds() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &nth_function(&arena),
            &[comma_list(&arena, &[num(&arena, 10.0)]), num(&arena, 5.0)],
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Invalid index"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nth_zero() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &nth_function(&arena),
            &[comma_list(&arena, &[num(&arena, 10.0)]), num(&arena, 0.0)],
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("may not be 0"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_set_nth() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &set_nth_function(&arena),
            &[
                comma_list(
                    &arena,
                    &[num(&arena, 1.0), num(&arena, 2.0), num(&arena, 3.0)],
                ),
                num(&arena, 2.0),
                s(&arena, "replaced"),
            ],
        )
        .await
        .unwrap();
        let items = v.as_list(&arena).unwrap();
        match &*items[1] {
            ValueKind::String(s) => assert_eq!(s.text, "replaced"),
            _ => panic!("expected string"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_join_default() {
        let arena = Bump::new();
        let l1 = comma_list(&arena, &[num(&arena, 1.0), num(&arena, 2.0)]);
        let l2 = comma_list(&arena, &[num(&arena, 3.0), num(&arena, 4.0)]);
        let v = eval(&arena, &join_function(&arena), &[l1, l2])
            .await
            .unwrap();
        assert_eq!(v.length_as_list(), 4);
        assert_eq!(v.separator(), ListSeparator::Comma);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_join_comma() {
        let arena = Bump::new();
        let l1 = space_list(&arena, &[num(&arena, 1.0)]);
        let l2 = space_list(&arena, &[num(&arena, 2.0)]);
        let v = eval(
            &arena,
            &join_function(&arena),
            &[l1, l2, s(&arena, "comma")],
        )
        .await
        .unwrap();
        assert_eq!(v.separator(), ListSeparator::Comma);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_join_space() {
        let arena = Bump::new();
        let l1 = comma_list(&arena, &[num(&arena, 1.0)]);
        let l2 = comma_list(&arena, &[num(&arena, 2.0)]);
        let v = eval(
            &arena,
            &join_function(&arena),
            &[l1, l2, s(&arena, "space")],
        )
        .await
        .unwrap();
        assert_eq!(v.separator(), ListSeparator::Space);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_join_bracketed() {
        let arena = Bump::new();
        let l1 = bracketed_list(&arena, &[num(&arena, 1.0)]);
        let l2 = comma_list(&arena, &[num(&arena, 2.0)]);
        let v = eval(
            &arena,
            &join_function(&arena),
            &[l1, l2, s(&arena, "comma"), s(&arena, "auto")],
        )
        .await
        .unwrap();
        assert!(v.has_brackets());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_join_invalid_separator() {
        let arena = Bump::new();
        let l1 = comma_list(&arena, &[num(&arena, 1.0)]);
        let l2 = comma_list(&arena, &[num(&arena, 2.0)]);
        let err = eval(
            &arena,
            &join_function(&arena),
            &[l1, l2, s(&arena, "invalid")],
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Must be \"space\""));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_join_append_null_separator_throws() {
        // Explicit `$separator: null` throws
        // `$separator: null is not a string.` (Dart assertString), in both
        // `join` and `append` — only an omitted separator means auto.
        let arena = Bump::new();
        let null = Value::new_with_arena(&arena, ValueKind::Null);
        let l1 = comma_list(&arena, &[num(&arena, 1.0)]);
        let l2 = comma_list(&arena, &[num(&arena, 2.0)]);
        let err = eval(&arena, &join_function(&arena), &[l1, l2, null])
            .await
            .unwrap_err();
        match *err {
            // The harness calls the callback directly (no eval error-boundary
            // wrap), so assert the raw variant + parts; the CLI shows the
            // wrapped `$separator: null is not a string.` (verified below).
            SassError::Script {
                message,
                argument_name,
            } => {
                assert_eq!(message, "null is not a string.");
                assert_eq!(argument_name.as_deref(), Some("separator"));
            }
            other => panic!("expected Script, got {other:?}"),
        }
        let l = comma_list(&arena, &[num(&arena, 1.0)]);
        let err = eval(
            &arena,
            &append_function(&arena),
            &[l, num(&arena, 2.0), null],
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Script {
                message,
                argument_name,
            } => {
                assert_eq!(message, "null is not a string.");
                assert_eq!(argument_name.as_deref(), Some("separator"));
            }
            other => panic!("expected Script, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &append_function(&arena),
            &[
                comma_list(&arena, &[num(&arena, 1.0), num(&arena, 2.0)]),
                num(&arena, 3.0),
            ],
        )
        .await
        .unwrap();
        assert_eq!(v.length_as_list(), 3);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_append_space() {
        let arena = Bump::new();
        let l = space_list(&arena, &[num(&arena, 1.0), num(&arena, 2.0)]);
        let v = eval(&arena, &append_function(&arena), &[l, num(&arena, 3.0)])
            .await
            .unwrap();
        assert_eq!(v.separator(), ListSeparator::Space);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_zip() {
        let arena = Bump::new();
        let l1 = comma_list(&arena, &[num(&arena, 1.0), num(&arena, 2.0)]);
        let l2 = comma_list(&arena, &[s(&arena, "a"), s(&arena, "b")]);
        let v = eval(
            &arena,
            &zip_function(&arena),
            &[comma_list(&arena, &[l1, l2])],
        )
        .await
        .unwrap();
        assert_eq!(v.length_as_list(), 2);
        assert_eq!(v.separator(), ListSeparator::Comma);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_zip_empty() {
        let arena = Bump::new();
        let v = eval(&arena, &zip_function(&arena), &[comma_list(&arena, &[])])
            .await
            .unwrap();
        assert_eq!(v.length_as_list(), 0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_zip_uneven() {
        let arena = Bump::new();
        let l1 = comma_list(
            &arena,
            &[num(&arena, 1.0), num(&arena, 2.0), num(&arena, 3.0)],
        );
        let l2 = comma_list(&arena, &[s(&arena, "a")]);
        let v = eval(
            &arena,
            &zip_function(&arena),
            &[comma_list(&arena, &[l1, l2])],
        )
        .await
        .unwrap();
        assert_eq!(v.length_as_list(), 1);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_index() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &index_function(&arena),
            &[
                comma_list(
                    &arena,
                    &[num(&arena, 10.0), num(&arena, 20.0), num(&arena, 30.0)],
                ),
                num(&arena, 20.0),
            ],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::Number(n) => assert_eq!(n.value, 2.0),
            _ => panic!("expected number"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_index_not_found() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &index_function(&arena),
            &[comma_list(&arena, &[num(&arena, 10.0)]), num(&arena, 99.0)],
        )
        .await
        .unwrap();
        assert!(matches!(&*v, ValueKind::Null));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_bracketed_true() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_bracketed_function(&arena),
            &[bracketed_list(&arena, &[num(&arena, 1.0)])],
        )
        .await
        .unwrap();
        assert!(matches!(&*v, ValueKind::Boolean(b) if b.value));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_bracketed_false() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &is_bracketed_function(&arena),
            &[space_list(&arena, &[num(&arena, 1.0)])],
        )
        .await
        .unwrap();
        assert!(matches!(&*v, ValueKind::Boolean(b) if !b.value));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_separator_comma() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &separator_function(&arena),
            &[comma_list(&arena, &[num(&arena, 1.0)])],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::String(s) => assert_eq!(s.text, "comma"),
            _ => panic!("expected string"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_separator_space() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &separator_function(&arena),
            &[space_list(&arena, &[num(&arena, 1.0)])],
        )
        .await
        .unwrap();
        match &*v {
            ValueKind::String(s) => assert_eq!(s.text, "space"),
            _ => panic!("expected string"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_separator_slash() {
        let arena = Bump::new();
        let l = slash_list(&arena, &[num(&arena, 1.0), num(&arena, 2.0)]);
        let v = eval(&arena, &separator_function(&arena), &[l])
            .await
            .unwrap();
        match &*v {
            ValueKind::String(s) => assert_eq!(s.text, "slash"),
            _ => panic!("expected string"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_slash() {
        let arena = Bump::new();
        let v = eval(
            &arena,
            &slash_function(&arena),
            &[comma_list(
                &arena,
                &[num(&arena, 1.0), num(&arena, 2.0), num(&arena, 3.0)],
            )],
        )
        .await
        .unwrap();
        assert_eq!(v.length_as_list(), 3);
        assert_eq!(v.separator(), ListSeparator::Slash);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_slash_too_few() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &slash_function(&arena),
            &[comma_list(&arena, &[num(&arena, 1.0)])],
        )
        .await
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("At least two elements are required"));
    }
}
