// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/functions/string.dart (+ lib/src/utils.dart
//   (codepointIndexToCodeUnitIndex, codeUnitIndexToCodepointIndex, folded into
//   the codepoint helpers below); lib/src/util/character.dart (toUpperCase,
//   toLowerCase, ASCII-only); lib/src/value/string.dart (sassLength
//   code-point semantics))
// go-source: go/functions/string.go + go/functions/functions_string_init.go

use std::cell::Cell;
use std::rc::Rc;

use bumpalo::Bump;

use crate::callable::{BuiltInCallable, Callable, CallableKind};
use crate::common::exception::{SassError, SassResult};
use crate::eval::{EvalConfig, EvalState};
use crate::functions::helpers::random_int;
use crate::module::BuiltInModule;
use crate::value::{
    assert_number, assert_string, ListSeparator, SassList, SassNumber, SassString, Value, ValueKind,
};

/// The global definitions of Sass string functions.
///
/// Mirrors Dart's `global` list (`string.dart`): each entry wraps a module
/// function with a deprecation warning for `"string"`. The `str-*` names are
/// renames (`with_name`) of the module `length`/`insert`/`index`/`slice`, so
/// the warning names the original module function.
/// Matches Go: GlobalStringFunctions.
pub fn global_string_functions<'compile, 'parse>(
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
            unquote_function(arena).with_deprecation_warning("string", None)
        ),
        c!(
            arena,
            quote_function(arena).with_deprecation_warning("string", None)
        ),
        c!(
            arena,
            to_upper_case_function(arena).with_deprecation_warning("string", None)
        ),
        c!(
            arena,
            to_lower_case_function(arena).with_deprecation_warning("string", None)
        ),
        c!(
            arena,
            unique_id_function(arena).with_deprecation_warning("string", None)
        ),
        c!(
            arena,
            str_length_function_global(arena).with_deprecation_warning("string", Some("length"))
        ),
        c!(
            arena,
            str_insert_function_global(arena).with_deprecation_warning("string", Some("insert"))
        ),
        c!(
            arena,
            str_index_function_global(arena).with_deprecation_warning("string", Some("index"))
        ),
        c!(
            arena,
            str_slice_function_global(arena).with_deprecation_warning("string", Some("slice"))
        ),
    ]
}

/// The Sass string module (`sass:string`).
///
/// Mirrors Dart's `module`: `unquote`, `quote`, `to-upper-case`,
/// `to-lower-case`, `length`, `insert`, `index`, `slice`, then — out of
/// alphabetical order, matching Dart — `unique-id`, then `split`
/// (module-only, no deprecated global).
/// Matches Go: StringModule.
pub fn string_module<'compile, 'parse>(arena: &'compile Bump) -> BuiltInModule<'compile, 'parse>
where
    'compile: 'parse,
{
    macro_rules! c {
        ($arena:expr, $f:expr) => {
            Callable::new($arena, CallableKind::BuiltIn($f))
        };
    }
    let fns: Vec<Callable<'compile, 'parse>> = vec![
        c!(arena, unquote_function(arena)),
        c!(arena, quote_function(arena)),
        c!(arena, to_upper_case_function(arena)),
        c!(arena, to_lower_case_function(arena)),
        c!(arena, str_length_function(arena)),
        c!(arena, str_insert_function(arena)),
        c!(arena, str_index_function(arena)),
        c!(arena, str_slice_function(arena)),
        // Dart declares `unique-id` 9th (string.dart:40-41).
        c!(arena, unique_id_function(arena)),
        c!(arena, split_function(arena)),
    ];
    BuiltInModule::new(arena, "string".into(), &fns, &[], indexmap::IndexMap::new())
}

// ---- Shared function implementations ----

// Maps each ASCII lowercase letter to uppercase, leaving every other char
// (including non-ASCII letters) untouched.
//
// Matches Dart: `toUpperCase(codeUnit)` per UTF-16 code unit
// (`util/character.dart`) applied in the `_toUpperCase` loop. Since case
// mapping is ASCII-only, per-char and per-code-unit iteration agree; Rust
// iterates `char`s instead of code units.
fn sass_to_upper(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}

// Maps each ASCII uppercase letter to lowercase, leaving every other char
// (including non-ASCII letters) untouched.
//
// Matches Dart: `toLowerCase(codeUnit)` per UTF-16 code unit
// (`util/character.dart`) applied in the `_toLowerCase` loop (see the
// `sass_to_upper` note on ASCII-only equivalence).
fn sass_to_lower(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect()
}

// ---- Global-only functions ----

fn unquote_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
// Matches Dart: `_unquote` (`string.dart:90-94`). Returns the input unchanged
// when already unquoted; otherwise re-wraps the same text without quotes.
where
    'compile: 'parse,
{
    let s = assert_string(&args[0], Some("string"))?;
    if !s.has_quotes {
        return Ok(args[0]);
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(s.text), false)),
    ))
}

fn unquote_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
// Each `*_function` below passes `"sass:string"` as the URL, like
// `BuiltInCallable::function` with the URL fixed (Dart's `_function` helper
// in functions/string.dart, inlined at each call site here).
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "unquote",
        "$string",
        "sass:string",
        arena,
        Rc::new(unquote_impl),
    )
}

fn quote_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
// Matches Dart: `_quote` (`string.dart:96-100`). Returns the input unchanged
// when already quoted; otherwise re-wraps the same text with quotes.
where
    'compile: 'parse,
{
    let s = assert_string(&args[0], Some("string"))?;
    if s.has_quotes {
        return Ok(args[0]);
    }
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(s.text), true)),
    ))
}

fn quote_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "quote",
        "$string",
        "sass:string",
        arena,
        Rc::new(quote_impl),
    )
}

fn to_upper_case_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
// Matches Dart: `_toUpperCase` (`string.dart:188-195`). Case-maps the text
// (ASCII-only, via `sass_to_upper`) and preserves the input's quotes.
where
    'compile: 'parse,
{
    let s = assert_string(&args[0], Some("string"))?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(
            arena.alloc_str(&sass_to_upper(s.text)),
            s.has_quotes,
        )),
    ))
}

fn to_upper_case_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "to-upper-case",
        "$string",
        "sass:string",
        arena,
        Rc::new(to_upper_case_impl),
    )
}

fn to_lower_case_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
// Matches Dart: `_toLowerCase` (`string.dart:197-204`). Case-maps the text
// (ASCII-only, via `sass_to_lower`) and preserves the input's quotes.
where
    'compile: 'parse,
{
    let s = assert_string(&args[0], Some("string"))?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(
            arena.alloc_str(&sass_to_lower(s.text)),
            s.has_quotes,
        )),
    ))
}

fn to_lower_case_function<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "to-lower-case",
        "$string",
        "sass:string",
        arena,
        Rc::new(to_lower_case_impl),
    )
}

// The random state for unique IDs. Mirrors Dart's `_random` +
// `_previousUniqueId` (base-36 over `36^6`, `string.dart:17-21`) — Go's
// package-level `stringRandom` + `prevUniqueID`. Owned by the callable
// closure as an arena-allocated `&'parse Cell<i64>` (shared by reference),
// not a thread-local global.

fn unique_id_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
// Creates the `unique-id` callable. Unusually, the random state (`prev`,
// above) is allocated here — not inside the callback — so every call through
// this callable shares one incrementing counter.
//
// Matches Dart: `_uniqueId` (`string.dart:206-218`): each call bumps the
// previous ID by a random 1..=36, wraps modulo `36^6` past the max, and
// returns `"u" + id.toRadixString(36).padLeft(6, '0')` unquoted, where the
// leading `u` keeps the result a valid identifier.
where
    'compile: 'parse,
{
    let prev: &'parse Cell<i64> = arena.alloc(Cell::new(random_int(36i64.pow(6))));

    BuiltInCallable::function(
        "unique-id",
        "",
        "sass:string",
        arena,
        Rc::new(
            move |_config: &EvalConfig<'compile, 'parse>,
                  _state: &mut EvalState<'compile, 'parse>,
                  _args: Vec<Value<'parse>>,
                  arena: &'compile Bump| {
                let id = {
                    // Make it difficult to guess the next ID by randomizing
                    // the increase.
                    let mut id = prev.get() + random_int(36) + 1;
                    let max_val = 36i64.pow(6);
                    if id > max_val {
                        id %= max_val;
                    }
                    prev.set(id);
                    id
                };
                // Use base-36 encoding and zero-pad to 6 chars, matching Dart's
                // toRadixString(36).padLeft(6, '0'). The leading "u" ensures
                // that the result is a valid identifier.
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::String(SassString::new(
                        arena.alloc_str(&format!("u{}", pad_left(&to_radix_36(id), 6, '0'))),
                        false,
                    )),
                ))
            },
        ),
    )
}

// Formats a non-negative `n` in base 36 (digits + lowercase letters).
fn to_radix_36(n: i64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut n = n;
    let mut buf = Vec::new();
    while n > 0 {
        buf.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    buf.reverse();
    String::from_utf8(buf).unwrap()
}

// Left-pads `s` with `pad_char` to `length`; returns `s` unchanged if it is
// already that long. Used to zero-pad the base-36 ID to 6 chars.
fn pad_left(s: &str, length: usize, pad_char: char) -> String {
    if s.len() >= length {
        return s.to_string();
    }
    let mut result = String::with_capacity(length);
    for _ in 0..length - s.len() {
        result.push(pad_char);
    }
    result.push_str(s);
    result
}

// ---- str-length / length ----

fn str_length_function_global<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "str-length",
        "$string",
        "sass:string",
        arena,
        Rc::new(str_length_impl),
    )
}

fn str_length_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "length",
        "$string",
        "sass:string",
        arena,
        Rc::new(str_length_impl),
    )
}

fn str_length_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
// Matches Dart: `_length` (`string.dart:102-105`). Returns the string's
// code-point length (`sassLength`), not its byte/UTF-16 length.
where
    'compile: 'parse,
{
    let s = assert_string(&args[0], Some("string"))?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Number(SassNumber::new(s.sass_length() as f64, None)),
    ))
}

// ---- str-insert / insert ----

fn str_insert_function_global<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "str-insert",
        "$string, $insert, $index",
        "sass:string",
        arena,
        Rc::new(str_insert_impl),
    )
}

fn str_insert_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "insert",
        "$string, $insert, $index",
        "sass:string",
        arena,
        Rc::new(str_insert_impl),
    )
}

fn str_insert_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
// Matches Dart: `_insert` (`string.dart:107-134`). Asserts `$string`/
// `$insert` as strings and `$index` as a unitless int, re-bases negative
// indexes so `$insert` lands *after* the indexed character, then splices at
// the code-point-derived byte index (`replaceRange` in Dart). Out-of-range
// indexes clamp to the ends; quotes of `$string` are preserved.
where
    'compile: 'parse,
{
    let s = assert_string(&args[0], Some("string"))?;
    let insert = assert_string(&args[1], Some("insert"))?;
    let index = assert_number(&args[2], Some("index"))?;
    index.assert_no_units(Some("index"))?;
    let mut index_int = index.assert_int(Some("index"))?;

    // str-insert has unusual behavior for negative inputs. It guarantees that
    // the `$insert` string is at `$index` in the result, which means that we
    // want to insert before `$index` if it's positive and after if it's
    // negative.
    let length_in_codepoints = s.sass_length() as i64;
    if index_int < 0 {
        // +1 because negative indexes start counting from -1 rather than 0,
        // and another +1 because we want to insert *after* that index.
        index_int = (length_in_codepoints + index_int + 2).max(0);
    }

    let codepoint_idx = codepoint_for_index(index_int, length_in_codepoints);
    let code_unit_idx = codepoint_index_to_code_unit_index(s.text, codepoint_idx);

    let result = format!(
        "{}{}{}",
        &s.text[..code_unit_idx],
        insert.text,
        &s.text[code_unit_idx..]
    );
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(&result), s.has_quotes)),
    ))
}

// ---- str-index / index ----

fn str_index_function_global<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "str-index",
        "$string, $substring",
        "sass:string",
        arena,
        Rc::new(str_index_impl),
    )
}

fn str_index_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "index",
        "$string, $substring",
        "sass:string",
        arena,
        Rc::new(str_index_impl),
    )
}

fn str_index_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
// Matches Dart: `_index` (`string.dart:136-147`). Finds the substring in
// UTF-8 bytes, then converts the byte index to a 1-based code-point index;
// returns null when absent.
where
    'compile: 'parse,
{
    let s = assert_string(&args[0], Some("string"))?;
    let substring = assert_string(&args[1], Some("substring"))?;

    let code_unit_index = match s.text.find(substring.text) {
        None => return Ok(Value::new_with_arena(arena, ValueKind::Null)),
        Some(i) => i,
    };
    let codepoint_idx = code_unit_index_to_codepoint_index(s.text, code_unit_index);
    Ok(Value::new_with_arena(
        arena,
        ValueKind::Number(SassNumber::new((codepoint_idx + 1) as f64, None)),
    ))
}

// ---- str-slice / slice ----

fn str_slice_function_global<'compile, 'parse>(
    arena: &'compile Bump,
) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "str-slice",
        "$string, $start-at, $end-at: -1",
        "sass:string",
        arena,
        Rc::new(str_slice_impl),
    )
}

fn str_slice_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "slice",
        "$string, $start-at, $end-at: -1",
        "sass:string",
        arena,
        Rc::new(str_slice_impl),
    )
}

fn str_slice_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
// Matches Dart: `_slice` (`string.dart:149-186`). Both bounds are 1-based
// Sass indexes into code points; `$end-at: 0` always yields an empty string,
// an overlong end clamps to the last character, and an inverted range yields
// an empty string. Quote-ness of the input is preserved.
where
    'compile: 'parse,
{
    let s = assert_string(&args[0], Some("string"))?;
    let start = assert_number(&args[1], Some("start-at"))?;
    let end = assert_number(&args[2], Some("end-at"))?;
    start.assert_no_units(Some("start-at"))?;
    end.assert_no_units(Some("end-at"))?;

    let length_in_codepoints = s.sass_length() as i64;

    // No matter what the start index is, an end index of 0 will produce an
    // empty string.
    let end_int = end.assert_int(None)?;

    if end_int == 0 {
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(""), s.has_quotes)),
        ));
    }

    let start_int = start.assert_int(None)?;
    let start_codepoint = codepoint_for_index(start_int, length_in_codepoints);
    let end_codepoint = codepoint_for_index_allow_negative(end_int, length_in_codepoints);
    let end_codepoint = if end_codepoint == length_in_codepoints {
        end_codepoint - 1
    } else {
        end_codepoint
    };
    if end_codepoint < start_codepoint {
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(arena.alloc_str(""), s.has_quotes)),
        ));
    }

    let start_byte = codepoint_index_to_code_unit_index(s.text, start_codepoint);
    let end_byte = codepoint_index_to_code_unit_index(s.text, end_codepoint + 1);
    let result = s.text[start_byte..end_byte].to_string();
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(&result), s.has_quotes)),
    ))
}

// ---- split (module only) ----

fn split_impl<'compile, 'parse>(
    _config: &EvalConfig<'compile, 'parse>,
    _state: &mut EvalState<'compile, 'parse>,
    args: Vec<Value<'parse>>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
// Matches Dart's `split` closure (`string.dart:43-86`): asserts `$string`,
// `$separator`, and a real-null `$limit` (explicit null means "no limit");
// rejects `$limit < 1`; splits an empty input to an empty bracketed list,
// an empty separator to one string per code point (`text.runes`), and
// otherwise folds `separator.allMatches`-style matches up to `$limit`.
// Result is always a bracketed comma list preserving the input's quotes.
where
    'compile: 'parse,
{
    let s = assert_string(&args[0], Some("string"))?;
    let separator = assert_string(&args[1], Some("separator"))?;

    let mut limit_val: i64 = 0;
    let mut has_limit = false;
    if !matches!(&*args[2], ValueKind::Null) {
        let limit_num = assert_number(&args[2], Some("limit"))?;
        limit_val = limit_num.assert_int(Some("limit"))?;
        if limit_val < 1 {
            return Err(Box::new(SassError::Script {
                message: format!("Must be 1 or greater, was {}.", limit_val),
                argument_name: Some("limit".into()),
            }));
        }
        has_limit = true;
    }

    if s.text.is_empty() {
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::List(SassList::new(vec![], ListSeparator::Comma, true)),
        ));
    }

    if separator.text.is_empty() {
        let chunks: Vec<Value<'parse>> = s
            .text
            .chars()
            .map(|r| {
                Value::new_with_arena(
                    arena,
                    ValueKind::String(SassString::new(
                        arena.alloc_str(&r.to_string()),
                        s.has_quotes,
                    )),
                )
            })
            .collect();
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::List(SassList::new(chunks, ListSeparator::Comma, true)),
        ));
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut chunk_count: i64 = 0;
    let mut last_end = 0;
    loop {
        let match_index = match s.text[last_end..].find(separator.text) {
            None => break,
            Some(i) => i + last_end,
        };
        chunks.push(s.text[last_end..match_index].to_string());
        last_end = match_index + separator.text.len();
        chunk_count += 1;
        if has_limit && chunk_count == limit_val {
            break;
        }
    }
    chunks.push(s.text[last_end..].to_string());

    let result: Vec<Value<'parse>> = chunks
        .into_iter()
        .map(|chunk| {
            Value::new_with_arena(
                arena,
                ValueKind::String(SassString::new(arena.alloc_str(&chunk), s.has_quotes)),
            )
        })
        .collect();
    Ok(Value::new_with_arena(
        arena,
        ValueKind::List(SassList::new(result, ListSeparator::Comma, true)),
    ))
}

fn split_function<'compile, 'parse>(arena: &'compile Bump) -> BuiltInCallable<'compile, 'parse>
where
    'compile: 'parse,
{
    BuiltInCallable::function(
        "split",
        "$string, $separator, $limit: null",
        "sass:string",
        arena,
        Rc::new(split_impl),
    )
}

// ---- Codepoint helpers ----

// Converts a Sass string index into a codepoint index into a string whose
// codepoint length is `length_in_codepoints`.
//
// A Sass string index is one-based, and uses negative numbers to count
// backwards from the end of the string.
//
// Matches Dart: `_codepointForIndex` with `allowNegative: false`
// (`string.dart:230-240`) — a negative index before the start clamps to `0`.
// Rust-only split of Dart's keyword flag into two functions; the `slice` end
// bound uses [`codepoint_for_index_allow_negative`].
fn codepoint_for_index(index: i64, length_in_codepoints: i64) -> i64 {
    if index == 0 {
        return 0;
    }
    if index > 0 {
        return (index - 1).min(length_in_codepoints);
    }
    let result = length_in_codepoints + index;
    if result < 0 {
        return 0;
    }
    result
}

// Like [`codepoint_for_index`], but lets a negative index before the start
// stay negative (the `slice` end bound compares it against the start, so an
// inverted range still yields an empty string).
//
// Matches Dart: `_codepointForIndex` with `allowNegative: true`
// (`string.dart:230-240`).
fn codepoint_for_index_allow_negative(index: i64, length_in_codepoints: i64) -> i64 {
    if index == 0 {
        return 0;
    }
    if index > 0 {
        return (index - 1).min(length_in_codepoints);
    }
    length_in_codepoints + index
}

// Converts a codepoint index to a byte index into `s`.
//
// Matches Dart: `codepointIndexToCodeUnitIndex` (`utils.dart:163-173`). Dart
// counts UTF-16 code units; Rust `str` is UTF-8, so "code units" here are
// UTF-8 bytes — the same unit `str.find` and slicing use.
fn codepoint_index_to_code_unit_index(s: &str, codepoint_index: i64) -> usize {
    if codepoint_index <= 0 {
        return 0;
    }
    s.chars()
        .take(codepoint_index as usize)
        .map(|c| c.len_utf8())
        .sum()
}

// Converts a byte index into `s` to a codepoint index.
//
// Matches Dart: `codeUnitIndexToCodepointIndex` (`utils.dart:175-186`),
// with the same UTF-16 → UTF-8 unit shift as
// [`codepoint_index_to_code_unit_index`]. Callers pass only indices
// produced by `str.find` or earlier conversions, so the slice is always on a
// `char` boundary.
fn code_unit_index_to_codepoint_index(s: &str, code_unit_index: usize) -> usize {
    if code_unit_index == 0 {
        return 0;
    }
    s[..code_unit_index].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::serialize_value_inspect;

    use crate::functions::test_utils::{assert_no_warnings, eval, eval_recorded};
    use crate::logger::test_utils::RecordLogger;

    // --- helpers ---

    /// Mirrors Go: value.SerializeValueInspect (assertInspect in string_test.go).
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

    fn unit_val<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        v: f64,
        unit: &str,
    ) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(v, Some(unit))))
    }

    /// The evaluated default for `$end-at: -1`. The test harness pads missing
    /// arguments with Null, but the real evaluator always passes the
    /// evaluated default expression.
    fn neg_one<'compile: 'parse, 'parse>(arena: &'compile Bump) -> Value<'parse> {
        num_val(arena, -1.0)
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

    fn assert_unquoted_string(got: &Value<'_>, want: &str) {
        match &**got {
            ValueKind::String(s) => {
                assert_eq!(s.text, want);
                assert!(!s.has_quotes, "expected unquoted string");
            }
            other => panic!("want String, got {other:?}"),
        }
    }

    fn assert_quoted_string(got: &Value<'_>, want: &str) {
        match &**got {
            ValueKind::String(s) => {
                assert_eq!(s.text, want);
                assert!(s.has_quotes, "expected quoted string");
            }
            other => panic!("want String, got {other:?}"),
        }
    }

    fn assert_num(got: &Value<'_>, want: f64) {
        match &**got {
            ValueKind::Number(n) => assert_eq!(n.value, want),
            other => panic!("want Number, got {other:?}"),
        }
    }

    fn string_global_builtin_warning_msg(name: &str) -> String {
        [
            "Global built-in functions are deprecated and will be removed in Dart Sass 3.0.0.",
            &format!("Use string.{name} instead."),
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
        let fns = global_string_functions(arena);
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

    // --- global_string_functions ---

    #[rust_sass_macros::maybe_test]
    async fn test_global_string_functions_names() {
        let arena = Bump::new();
        let fns = global_string_functions(&arena);
        let want = [
            "unquote",
            "quote",
            "to-upper-case",
            "to-lower-case",
            "unique-id",
            "str-length",
            "str-insert",
            "str-index",
            "str-slice",
        ];
        assert_eq!(fns.len(), want.len());
        for (i, f) in fns.iter().enumerate() {
            assert_eq!(f.name(), want[i], "fns[{i}]");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_string_functions_deprecation_metadata() {
        let arena = Bump::new();
        let fns = global_string_functions(&arena);
        let want = [
            ("unquote", "unquote"),
            ("quote", "quote"),
            ("to-upper-case", "to-upper-case"),
            ("to-lower-case", "to-lower-case"),
            ("unique-id", "unique-id"),
            ("str-length", "length"),
            ("str-insert", "insert"),
            ("str-index", "index"),
            ("str-slice", "slice"),
        ];
        for (i, f) in fns.iter().enumerate() {
            match f.kind() {
                CallableKind::BuiltIn(b) => {
                    assert_eq!(b.name(), want[i].0);
                    let dw = b
                        .deprecation_warning()
                        .expect("deprecation warning should be set");
                    assert_eq!(dw.0, "string");
                    assert_eq!(dw.1, want[i].1);
                }
                _ => panic!("expected BuiltIn callable"),
            }
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_unquote_emits_deprecation_warning() {
        let arena = Bump::new();
        let unquote = global_bic(&arena, 0);
        let (result, logger) = eval_recorded(&arena, &unquote, &[quoted_val(&arena, "foo")]).await;
        assert_unquoted_string(&result.unwrap(), "foo");
        assert_single_warning(
            &logger,
            &string_global_builtin_warning_msg("unquote"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_str_length_emits_deprecation_warning() {
        let arena = Bump::new();
        let str_length = global_bic(&arena, 5);
        let (result, logger) = eval_recorded(&arena, &str_length, &[str_val(&arena, "abcd")]).await;
        assert_num(&result.unwrap(), 4.0);
        assert_single_warning(
            &logger,
            &string_global_builtin_warning_msg("length"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_str_insert_emits_deprecation_warning() {
        let arena = Bump::new();
        let str_insert = global_bic(&arena, 6);
        let (result, logger) = eval_recorded(
            &arena,
            &str_insert,
            &[
                str_val(&arena, "abcd"),
                str_val(&arena, "X"),
                num_val(&arena, 1.0),
            ],
        )
        .await;
        assert_unquoted_string(&result.unwrap(), "Xabcd");
        assert_single_warning(
            &logger,
            &string_global_builtin_warning_msg("insert"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_str_index_emits_deprecation_warning() {
        let arena = Bump::new();
        let str_index = global_bic(&arena, 7);
        let (result, logger) = eval_recorded(
            &arena,
            &str_index,
            &[str_val(&arena, "abcd"), str_val(&arena, "bc")],
        )
        .await;
        assert_num(&result.unwrap(), 2.0);
        assert_single_warning(
            &logger,
            &string_global_builtin_warning_msg("index"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_str_slice_emits_deprecation_warning() {
        let arena = Bump::new();
        let str_slice = global_bic(&arena, 8);
        let (result, logger) = eval_recorded(
            &arena,
            &str_slice,
            &[
                str_val(&arena, "abcd"),
                num_val(&arena, 2.0),
                neg_one(&arena),
            ],
        )
        .await;
        assert_unquoted_string(&result.unwrap(), "bcd");
        assert_single_warning(
            &logger,
            &string_global_builtin_warning_msg("slice"),
            "global-builtin",
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_global_unique_id_emits_deprecation_warning() {
        let arena = Bump::new();
        let unique_id = global_bic(&arena, 4);
        let (result, logger) = eval_recorded(&arena, &unique_id, &[]).await;
        result.unwrap();
        assert_single_warning(
            &logger,
            &string_global_builtin_warning_msg("unique-id"),
            "global-builtin",
        );
    }

    // --- string_module ---

    #[rust_sass_macros::maybe_test]
    async fn test_string_module_url() {
        let arena = Bump::new();
        let m = string_module(&arena);
        assert_eq!(m.url, "sass:string");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_string_module_functions() {
        let arena = Bump::new();
        let m = string_module(&arena);
        // Dart declares `unique-id` 9th (string.dart:40-41).
        let want = [
            "unquote",
            "quote",
            "to-upper-case",
            "to-lower-case",
            "length",
            "insert",
            "index",
            "slice",
            "unique-id",
            "split",
        ];
        assert_eq!(m.functions.len(), want.len());
        for (i, name) in m.functions.keys().enumerate() {
            assert_eq!(name, want[i], "functions[{i}]");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_string_module_no_mixins_no_variables() {
        let arena = Bump::new();
        let m = string_module(&arena);
        assert_eq!(m.mixins.len(), 0);
        assert_eq!(m.variables.len(), 0);
    }

    // --- unquote ---

    #[rust_sass_macros::maybe_test]
    async fn test_unquote_quoted() {
        let arena = Bump::new();
        let (result, logger) = eval_recorded(
            &arena,
            &unquote_function(&arena),
            &[quoted_val(&arena, "foo")],
        )
        .await;
        assert_unquoted_string(&result.unwrap(), "foo");
        assert_no_warnings(&logger);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_unquote_unquoted_returns_same_value() {
        let arena = Bump::new();
        let input = str_val(&arena, "foo");
        let got = eval_ok(&arena, &unquote_function(&arena), &[input]).await;
        assert!(
            std::ptr::eq(&*got as *const _, &*input as *const _),
            "unquote of an unquoted string should return the same value"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_unquote_empty() {
        let arena = Bump::new();
        let got = eval_ok(&arena, &unquote_function(&arena), &[quoted_val(&arena, "")]).await;
        assert_unquoted_string(&got, "");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_unquote_type_error() {
        let arena = Bump::new();
        let err = eval(&arena, &unquote_function(&arena), &[num_val(&arena, 1.0)])
            .await
            .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("string"));
    }

    // --- quote ---

    #[rust_sass_macros::maybe_test]
    async fn test_quote_unquoted() {
        let arena = Bump::new();
        let got = eval_ok(&arena, &quote_function(&arena), &[str_val(&arena, "foo")]).await;
        assert_quoted_string(&got, "foo");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_quote_quoted_returns_same_value() {
        let arena = Bump::new();
        let input = quoted_val(&arena, "foo");
        let got = eval_ok(&arena, &quote_function(&arena), &[input]).await;
        assert!(
            std::ptr::eq(&*got as *const _, &*input as *const _),
            "quote of a quoted string should return the same value"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_quote_type_error() {
        let arena = Bump::new();
        let err = eval(&arena, &quote_function(&arena), &[num_val(&arena, 1.0)])
            .await
            .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("string"));
    }

    // --- to-upper-case / to-lower-case ---

    #[rust_sass_macros::maybe_test]
    async fn test_to_upper_case() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &to_upper_case_function(&arena),
            &[str_val(&arena, "aBc123")],
        )
        .await;
        assert_unquoted_string(&got, "ABC123");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_upper_case_ascii_only() {
        let arena = Bump::new();
        // Only ASCII a-z are uppercased, matching Dart's toUpperCase(codeUnit).
        let got = eval_ok(
            &arena,
            &to_upper_case_function(&arena),
            &[str_val(&arena, "aBc123äö")],
        )
        .await;
        assert_unquoted_string(&got, "ABC123äö");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_upper_case_keeps_quotes() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &to_upper_case_function(&arena),
            &[quoted_val(&arena, "abc")],
        )
        .await;
        assert_quoted_string(&got, "ABC");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_upper_case_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_upper_case_function(&arena),
            &[num_val(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("string"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_lower_case() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &to_lower_case_function(&arena),
            &[str_val(&arena, "AbC123")],
        )
        .await;
        assert_unquoted_string(&got, "abc123");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_lower_case_ascii_only() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &to_lower_case_function(&arena),
            &[str_val(&arena, "AbC123ÄÖ")],
        )
        .await;
        assert_unquoted_string(&got, "abc123ÄÖ");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_lower_case_keeps_quotes() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &to_lower_case_function(&arena),
            &[quoted_val(&arena, "ABC")],
        )
        .await;
        assert_quoted_string(&got, "abc");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_lower_case_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &to_lower_case_function(&arena),
            &[num_val(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("string"));
    }

    // --- unique-id ---

    #[rust_sass_macros::maybe_test]
    async fn test_unique_id_format() {
        let arena = Bump::new();
        let got = eval_ok(&arena, &unique_id_function(&arena), &[]).await;
        let ValueKind::String(s) = &*got else {
            panic!("expected String, got {got:?}");
        };
        assert!(!s.has_quotes, "unique-id should be unquoted");
        assert_eq!(s.text.len(), 7, "len({:?})", s.text);
        assert!(s.text.starts_with('u'));
        for c in s.text[1..].chars() {
            assert!(
                c.is_ascii_digit() || c.is_ascii_lowercase(),
                "invalid base-36 char {c:?} in {:?}",
                s.text
            );
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_unique_id_changes() {
        let arena = Bump::new();
        let first = eval_ok(&arena, &unique_id_function(&arena), &[]).await;
        let second = eval_ok(&arena, &unique_id_function(&arena), &[]).await;
        let (ValueKind::String(a), ValueKind::String(b)) = (&*first, &*second) else {
            panic!("expected Strings");
        };
        assert_ne!(
            a.text, b.text,
            "two unique-id calls returned the same value"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_pad_left() {
        assert_eq!(pad_left("ab", 6, '0'), "0000ab");
        assert_eq!(pad_left("abcdef", 6, '0'), "abcdef");
        assert_eq!(pad_left("abcdefg", 6, '0'), "abcdefg");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_to_radix_36() {
        assert_eq!(to_radix_36(0), "0");
        assert_eq!(to_radix_36(35), "z");
        assert_eq!(to_radix_36(36), "10");
        assert_eq!(to_radix_36(36 * 36 - 1), "zz");
    }

    // --- str-length / length ---

    #[rust_sass_macros::maybe_test]
    async fn test_str_length() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &str_length_function(&arena),
            &[str_val(&arena, "abcd")],
        )
        .await;
        assert_num(&got, 4.0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_length_empty() {
        let arena = Bump::new();
        let got = eval_ok(&arena, &str_length_function(&arena), &[str_val(&arena, "")]).await;
        assert_num(&got, 0.0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_length_codepoints() {
        let arena = Bump::new();
        // Length counts codepoints, not bytes.
        let got = eval_ok(
            &arena,
            &str_length_function(&arena),
            &[str_val(&arena, "a\u{1F46D}b")],
        )
        .await;
        assert_num(&got, 3.0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_length_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &str_length_function(&arena),
            &[num_val(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("string"));
    }

    // --- str-insert / insert ---

    #[rust_sass_macros::maybe_test]
    async fn test_str_insert() {
        let arena = Bump::new();
        let cases: [(f64, &str); 9] = [
            (1.0, "Xabcd"),
            (3.0, "abXcd"),
            (5.0, "abcdX"),
            (100.0, "abcdX"),
            (0.0, "Xabcd"),
            (-1.0, "abcdX"),
            (-2.0, "abcXd"),
            (-5.0, "Xabcd"),
            (-100.0, "Xabcd"),
        ];
        for (index, want) in cases {
            let got = eval_ok(
                &arena,
                &str_insert_function(&arena),
                &[
                    str_val(&arena, "abcd"),
                    str_val(&arena, "X"),
                    num_val(&arena, index),
                ],
            )
            .await;
            let ValueKind::String(s) = &*got else {
                panic!("index {index}: expected String, got {got:?}");
            };
            assert_eq!(s.text, want, "insert(abcd, X, {index})");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_insert_keeps_quotes() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &str_insert_function(&arena),
            &[
                quoted_val(&arena, "abcd"),
                str_val(&arena, "X"),
                num_val(&arena, 1.0),
            ],
        )
        .await;
        assert_quoted_string(&got, "Xabcd");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_insert_codepoints() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &str_insert_function(&arena),
            &[
                str_val(&arena, "a\u{1F46D}b"),
                str_val(&arena, "X"),
                num_val(&arena, 3.0),
            ],
        )
        .await;
        assert_unquoted_string(&got, "a\u{1F46D}Xb");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_insert_string_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &str_insert_function(&arena),
            &[
                num_val(&arena, 1.0),
                str_val(&arena, "X"),
                num_val(&arena, 1.0),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("string"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_insert_insert_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &str_insert_function(&arena),
            &[
                str_val(&arena, "ab"),
                num_val(&arena, 1.0),
                num_val(&arena, 1.0),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("insert"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_insert_index_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &str_insert_function(&arena),
            &[
                str_val(&arena, "ab"),
                str_val(&arena, "X"),
                str_val(&arena, "x"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "x is not a number.", Some("index"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_insert_index_units_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &str_insert_function(&arena),
            &[
                str_val(&arena, "ab"),
                str_val(&arena, "X"),
                unit_val(&arena, 1.0, "px"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Expected 1px to have no units.", Some("index"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_insert_index_non_int_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &str_insert_function(&arena),
            &[
                str_val(&arena, "ab"),
                str_val(&arena, "X"),
                num_val(&arena, 1.5),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1.5 is not an int.", Some("index"));
    }

    // --- str-index / index ---

    #[rust_sass_macros::maybe_test]
    async fn test_str_index_found() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &str_index_function(&arena),
            &[str_val(&arena, "abcd"), str_val(&arena, "bc")],
        )
        .await;
        assert_num(&got, 2.0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_index_not_found() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &str_index_function(&arena),
            &[str_val(&arena, "abcd"), str_val(&arena, "x")],
        )
        .await;
        assert!(matches!(&*got, ValueKind::Null), "want Null, got {got:?}");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_index_first() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &str_index_function(&arena),
            &[str_val(&arena, "abcd"), str_val(&arena, "a")],
        )
        .await;
        assert_num(&got, 1.0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_index_codepoints() {
        let arena = Bump::new();
        // The result is a codepoint index, not a byte index.
        let got = eval_ok(
            &arena,
            &str_index_function(&arena),
            &[str_val(&arena, "a\u{1F46D}bc"), str_val(&arena, "b")],
        )
        .await;
        assert_num(&got, 3.0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_index_empty_substring() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &str_index_function(&arena),
            &[str_val(&arena, "abcd"), str_val(&arena, "")],
        )
        .await;
        assert_num(&got, 1.0);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_index_string_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &str_index_function(&arena),
            &[num_val(&arena, 1.0), str_val(&arena, "a")],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("string"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_index_substring_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &str_index_function(&arena),
            &[str_val(&arena, "ab"), num_val(&arena, 1.0)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("substring"));
    }

    // --- str-slice / slice ---

    #[rust_sass_macros::maybe_test]
    async fn test_str_slice() {
        let arena = Bump::new();
        let cases: [(f64, f64, &str); 10] = [
            (2.0, 3.0, "bc"),
            (2.0, -1.0, "bcd"),
            (-3.0, -2.0, "bc"),
            (1.0, 0.0, ""),
            (2.0, 1.0, ""),
            (2.0, 10.0, "bcd"),
            (-100.0, -1.0, "abcd"),
            (-100.0, -100.0, ""),
            (1.0, -1.0, "abcd"),
            (4.0, 4.0, "d"),
        ];
        for (start, end, want) in cases {
            let got = eval_ok(
                &arena,
                &str_slice_function(&arena),
                &[
                    str_val(&arena, "abcd"),
                    num_val(&arena, start),
                    num_val(&arena, end),
                ],
            )
            .await;
            let ValueKind::String(s) = &*got else {
                panic!("slice(abcd, {start}, {end}): expected String, got {got:?}");
            };
            assert_eq!(s.text, want, "slice(abcd, {start}, {end})");
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_slice_keeps_quotes() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &str_slice_function(&arena),
            &[
                quoted_val(&arena, "abcd"),
                num_val(&arena, 2.0),
                num_val(&arena, 3.0),
            ],
        )
        .await;
        assert_quoted_string(&got, "bc");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_slice_empty_result_keeps_quotes() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &str_slice_function(&arena),
            &[
                quoted_val(&arena, "abcd"),
                num_val(&arena, 1.0),
                num_val(&arena, 0.0),
            ],
        )
        .await;
        assert_quoted_string(&got, "");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_slice_codepoints() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &str_slice_function(&arena),
            &[
                str_val(&arena, "a\u{1F46D}bc"),
                num_val(&arena, 2.0),
                num_val(&arena, 3.0),
            ],
        )
        .await;
        assert_unquoted_string(&got, "\u{1F46D}b");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_slice_string_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &str_slice_function(&arena),
            &[num_val(&arena, 1.0), num_val(&arena, 1.0), neg_one(&arena)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("string"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_slice_start_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &str_slice_function(&arena),
            &[str_val(&arena, "ab"), str_val(&arena, "x"), neg_one(&arena)],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "x is not a number.", Some("start-at"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_slice_explicit_null_end_error() {
        let arena = Bump::new();
        // Matches Dart: an explicit null $end-at is a type error — the
        // default -1 only comes from the parameter's default expression.
        let err = eval(
            &arena,
            &str_slice_function(&arena),
            &[
                str_val(&arena, "ab"),
                num_val(&arena, 1.0),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "null is not a number.", Some("end-at"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_slice_end_asserted_before_start_units() {
        let arena = Bump::new();
        // Matches Dart: both numbers are asserted before either no-units
        // check.
        let err = eval(
            &arena,
            &str_slice_function(&arena),
            &[
                str_val(&arena, "ab"),
                unit_val(&arena, 1.0, "px"),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "null is not a number.", Some("end-at"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_slice_start_units_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &str_slice_function(&arena),
            &[
                str_val(&arena, "ab"),
                unit_val(&arena, 1.0, "px"),
                num_val(&arena, 2.0),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Expected 1px to have no units.", Some("start-at"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_slice_end_units_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &str_slice_function(&arena),
            &[
                str_val(&arena, "ab"),
                num_val(&arena, 1.0),
                unit_val(&arena, 2.0, "px"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Expected 2px to have no units.", Some("end-at"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_slice_start_non_int_error() {
        let arena = Bump::new();
        // Matches Dart: start.assertInt() has no argument name...
        let err = eval(
            &arena,
            &str_slice_function(&arena),
            &[
                str_val(&arena, "abcd"),
                num_val(&arena, 1.5),
                neg_one(&arena),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1.5 is not an int.", None);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_str_slice_end_non_int_error() {
        let arena = Bump::new();
        // ...and neither does end.assertInt(), which runs first.
        let err = eval(
            &arena,
            &str_slice_function(&arena),
            &[
                str_val(&arena, "abcd"),
                num_val(&arena, 1.5),
                num_val(&arena, 2.5),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "2.5 is not an int.", None);
    }

    // --- split ---

    #[rust_sass_macros::maybe_test]
    async fn test_split() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &split_function(&arena),
            &[
                quoted_val(&arena, "a,b,c"),
                quoted_val(&arena, ","),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await;
        assert_inspect(&got, "[\"a\", \"b\", \"c\"]");
        assert_eq!(got.separator(), ListSeparator::Comma);
        assert!(got.has_brackets());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_unquoted() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &split_function(&arena),
            &[
                str_val(&arena, "a,b,c"),
                str_val(&arena, ","),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await;
        assert_inspect(&got, "[a, b, c]");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_limit() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &split_function(&arena),
            &[
                quoted_val(&arena, "a,b,c"),
                quoted_val(&arena, ","),
                num_val(&arena, 1.0),
            ],
        )
        .await;
        assert_inspect(&got, "[\"a\", \"b,c\"]");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_limit_larger_than_chunks() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &split_function(&arena),
            &[
                quoted_val(&arena, "a b"),
                quoted_val(&arena, " "),
                num_val(&arena, 5.0),
            ],
        )
        .await;
        assert_inspect(&got, "[\"a\", \"b\"]");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_empty_separator() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &split_function(&arena),
            &[
                quoted_val(&arena, "abc"),
                quoted_val(&arena, ""),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await;
        assert_inspect(&got, "[\"a\", \"b\", \"c\"]");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_empty_separator_codepoints() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &split_function(&arena),
            &[
                str_val(&arena, "a\u{1F46D}b"),
                str_val(&arena, ""),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await;
        assert_eq!(got.length_as_list(), 3);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_empty_string() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &split_function(&arena),
            &[
                quoted_val(&arena, ""),
                quoted_val(&arena, ","),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await;
        assert_inspect(&got, "[]");
        assert!(got.has_brackets());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_trailing_separator() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &split_function(&arena),
            &[
                quoted_val(&arena, "a,b,c,"),
                quoted_val(&arena, ","),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await;
        assert_inspect(&got, "[\"a\", \"b\", \"c\", \"\"]");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_leading_separator() {
        let arena = Bump::new();
        let got = eval_ok(
            &arena,
            &split_function(&arena),
            &[
                quoted_val(&arena, ",a"),
                quoted_val(&arena, ","),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await;
        assert_inspect(&got, "[\"\", \"a\"]");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_separator_not_found() {
        let arena = Bump::new();
        // A singleton comma-separated list keeps the trailing comma even when
        // bracketed (verified against dart-sass).
        let got = eval_ok(
            &arena,
            &split_function(&arena),
            &[
                quoted_val(&arena, "abc"),
                quoted_val(&arena, "x"),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await;
        assert_inspect(&got, "[\"abc\",]");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_string_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &split_function(&arena),
            &[
                num_val(&arena, 1.0),
                str_val(&arena, ","),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("string"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_separator_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &split_function(&arena),
            &[
                str_val(&arena, "ab"),
                num_val(&arena, 1.0),
                Value::new_with_arena(&arena, ValueKind::Null),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1 is not a string.", Some("separator"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_limit_zero_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &split_function(&arena),
            &[
                str_val(&arena, "a,b"),
                str_val(&arena, ","),
                num_val(&arena, 0.0),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Must be 1 or greater, was 0.", Some("limit"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_limit_negative_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &split_function(&arena),
            &[
                str_val(&arena, "a,b"),
                str_val(&arena, ","),
                num_val(&arena, -1.0),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "Must be 1 or greater, was -1.", Some("limit"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_limit_non_int_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &split_function(&arena),
            &[
                str_val(&arena, "a,b"),
                str_val(&arena, ","),
                num_val(&arena, 1.5),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "1.5 is not an int.", Some("limit"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_split_limit_type_error() {
        let arena = Bump::new();
        let err = eval(
            &arena,
            &split_function(&arena),
            &[
                str_val(&arena, "a,b"),
                str_val(&arena, ","),
                str_val(&arena, "x"),
            ],
        )
        .await
        .unwrap_err();
        assert_script_err(err, "x is not a number.", Some("limit"));
    }

    // --- codepoint helpers ---

    #[rust_sass_macros::maybe_test]
    async fn test_codepoint_for_index() {
        let cases: [(i64, i64, i64); 9] = [
            (0, 4, 0),
            (1, 4, 0),
            (4, 4, 3),
            (5, 4, 4),
            (100, 4, 4),
            (-1, 4, 3),
            (-4, 4, 0),
            (-5, 4, 0),
            (-100, 4, 0),
        ];
        for (index, length, want) in cases {
            assert_eq!(
                codepoint_for_index(index, length),
                want,
                "codepoint_for_index({index}, {length})"
            );
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_codepoint_for_index_allow_negative() {
        assert_eq!(codepoint_for_index_allow_negative(-5, 4), -1);
        assert_eq!(codepoint_for_index_allow_negative(-100, 4), -96);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_codepoint_index_to_code_unit_index() {
        let s = "a\u{1F46D}b";
        let cases: [(i64, usize); 4] = [(0, 0), (1, 1), (2, 5), (3, 6)];
        for (codepoint, want) in cases {
            assert_eq!(
                codepoint_index_to_code_unit_index(s, codepoint),
                want,
                "codepoint_index_to_code_unit_index({s:?}, {codepoint})"
            );
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_code_unit_index_to_codepoint_index() {
        let s = "a\u{1F46D}b";
        let cases: [(usize, usize); 4] = [(0, 0), (1, 1), (5, 2), (6, 3)];
        for (code_unit, want) in cases {
            assert_eq!(
                code_unit_index_to_codepoint_index(s, code_unit),
                want,
                "code_unit_index_to_codepoint_index({s:?}, {code_unit})"
            );
        }
    }
}
