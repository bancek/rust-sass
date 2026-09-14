// Copyright 2012-2016 Sass Open Source Foundation. Use of this source code
// is governed by an MIT-style license that can be found in the LICENSE
// file or at https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// libsass-source: src/sass_functions.cpp (entries/lists) + src/context.cpp (registration) + src/fn_utils.cpp (signature parse) + src/eval.cpp (C-function invocation) + include/sass/functions.h (function half)

//! Custom Sass functions: `Sass_Function` entries/lists, signature parsing,
//! C↔core value conversion, and the callback bridge.
//!
//! Mirrors `sass/functions.h` (function half): `sass_make_function_list`,
//! `sass_make_function`, `sass_delete_function`, `sass_delete_function_list`,
//! `sass_function_{get,set}_list_entry`, `sass_function_get_signature`,
//! `sass_function_get_function`, `sass_function_get_cookie`.
//! (The importer half lives in [`crate::importers`].)
//!
//! # Custom-function protocol (mirrors upstream, plan §A.5)
//!
//! 1. The consumer builds entries (`sass_make_function`, which COPIES the
//!    signature) into a NULL-terminated list and hands it to
//!    `sass_option_set_c_functions` — ownership transfers (upstream frees
//!    lists+entries on clear; the adapter frees on options drop, overwrite,
//!    and move-out; never the cookie, which stays consumer-owned like
//!    upstream's `sass_delete_function`).
//! 2. At compile time each signature is parsed (strict, embedded-style —
//!    see [`parse_c_signature`]) and wrapped in a core `Callable` whose
//!    callback marshals core `Vec<Value>` argv into a C comma list, invokes
//!    the consumer callback, converts the result back, and deletes both
//!    temporaries (alias-guarded like upstream's `if (c_val != c_args)`).
//! 3. C error/warning returns raise `"error in C function {name}: {msg}"` /
//!    `"warning in C function {name}: {msg}"` (upstream `eval.cpp`
//!    call-site format) as `Script` errors for the core to span-wrap.
//!    A NULL return hardens to a `Script` error (upstream crashes).
//!
//! # Value conversion (lossy by design, plan §A.7)
//!
//! C→core: all 7 data kinds convert (single unit string, quote flag, rgba,
//! separators, recursive lists/maps); C error/warning values become
//! `Script` errors (there is no such `ValueKind`).
//! Core→C: booleans, null, strings, single-unit/unitless numbers, RGB
//! colors, lists (bracketed preserved; `Slash`/`Undecided` degrade to
//! `SPACE`), maps, and rest-arg packs (`foo(...)` variadics arrive as one
//! `ArgumentList`, converted to a comma list like upstream's
//! `Arguments`→list; keywords swallowed) convert; calculations, functions,
//! mixins, compound/denominator units, slash-pairs, and non-RGB colors
//! become `"unknown sass value type"`-style `Script` errors (upstream's
//! conversion error text, values.cpp).

use crate::env::free_callee_entry;
use crate::env::free_env_box;
use crate::env::snapshot_callees;
use crate::env::CalleeSnapshot;
use crate::env::SassCalleeBox;
use crate::env::SassEnvBox;
use crate::importers::free_import_entry;
use crate::importers::ImportEntry;
use sass_value_get_tag as tag_of;
use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;
use std::rc::Rc;

use bumpalo::Bump;

use rust_sass::ast::sass::parameter_list::ParameterList;
use rust_sass::callable::{BuiltInCallable, Callable, CallableKind, SyncBuiltInCallback};
use rust_sass::common::exception::{SassError, SassResult};
use rust_sass::common::file_span::FileSpan;
use rust_sass::common::source_span_file_source::FileSource;
use rust_sass::common::source_span_span_with_context::SourceSpanWithContext;
use rust_sass::environment::Environment;
use rust_sass::eval::EvalState;
use rust_sass::parse::stylesheet_parse::parse_parameter_list;
use rust_sass::value::color::ColorSpace;
use rust_sass::value::{
    ListSeparator, SassBoolean, SassColor, SassList, SassMap, SassNumber, SassString, Value,
    ValueKind,
};

use crate::base::{copy_bytes_nul, guard, malloc_or_abort};
use crate::values::{
    sass_boolean_get_value, sass_color_get_a, sass_color_get_b, sass_color_get_g, sass_color_get_r,
    sass_delete_value, sass_error_get_message, sass_list_get_is_bracketed, sass_list_get_length,
    sass_list_get_separator, sass_list_get_value, sass_list_set_value, sass_make_boolean,
    sass_make_color, sass_make_list, sass_make_map, sass_make_null, sass_make_number,
    sass_make_qstring, sass_make_string, sass_map_get_key, sass_map_get_length, sass_map_get_value,
    sass_map_set_key, sass_map_set_value, sass_number_get_unit, sass_number_get_value,
    sass_string_get_value, sass_string_is_quoted, sass_value_get_tag, sass_warning_get_message,
    SassValue, SASS_BOOLEAN, SASS_COLOR, SASS_COMMA, SASS_ERROR, SASS_LIST, SASS_MAP, SASS_NULL,
    SASS_NUMBER, SASS_SPACE, SASS_STRING, SASS_WARNING,
};

/// The consumer callback: `argv` is a borrowed C comma list (read, never
/// delete); the returned value transfers to the compiler (never delete it
/// either). Mirrors `Sass_Function_Fn` (functions.h:40-41).
pub type SassFunctionFn = Option<
    unsafe extern "C" fn(
        args: *const SassValue,
        entry: *mut FunctionEntry,
        compiler: *mut CompilerBox,
    ) -> *mut SassValue,
>;

/// One custom-function registration. Mirrors `Sass_Function`
/// (sass_functions.hpp: `signature` copied at make, `function` + `cookie`
/// stored raw).
#[repr(C)]
pub struct FunctionEntry {
    signature: *mut c_char,
    callback: SassFunctionFn,
    cookie: *mut c_void,
}

/// Per-invocation compiler token passed to C callbacks (`Sass_Compiler*`).
/// Carries the import-stack snapshot (importer callbacks) and the
/// callee-chain snapshot plus live env (function callbacks) at callback
/// entry — all strings copied (G3), all pointers borrowed for the call.
/// Opaque to C either way.
#[repr(C)]
pub struct CompilerBox {
    _unused: u8,
    /// Import-stack snapshot at callback entry (oldest first; empty when the
    /// callback fires outside any import, e.g. plain function calls).
    pub(crate) import_stack: Vec<ImportStackEntry>,
    /// Materialized C entries backing the `sass_compiler_get_*_import`
    /// getters (owned here, borrowed by C for the call — G3; freed on drop).
    pub(crate) import_entries: Vec<*mut ImportEntry>,
    /// The compile arena, lifetime-erased for env writes (see below).
    pub(crate) arena: Option<&'static Bump>,
    /// The live callback env, lifetime-erased (see below).
    pub(crate) live_env: Option<Environment<'static, 'static>>,
    /// Callee-chain snapshot at callback entry, outermost-first with the
    /// running C function synthesized on top (plan §7 step 8).
    pub(crate) callees: Vec<CalleeSnapshot>,
    /// Materialized C entries backing the `sass_compiler_get_*_callee`
    /// getters (owned here, borrowed by C — G3; freed on drop).
    pub(crate) callee_entries: Vec<*mut SassCalleeBox>,
    /// Env boxes backing `sass_callee_get_env` (owned here; freed on drop).
    pub(crate) env_boxes: Vec<*mut SassEnvBox>,
}

/// One import-stack frame: the upstream `Sass_Import` fields the compiler
/// getters expose (`sass_functions.hpp`; `sass_context.cpp:616-622`).
#[derive(Clone, Default, Debug)]
pub(crate) struct ImportStackEntry {
    pub(crate) imp_path: String,
    pub(crate) abs_path: String,
}

impl CompilerBox {
    /// Token for plain function calls (no import context).
    pub(crate) fn new() -> Self {
        CompilerBox {
            _unused: 0,
            import_stack: Vec::new(),
            import_entries: Vec::new(),
            arena: None,
            live_env: None,
            callees: Vec::new(),
            callee_entries: Vec::new(),
            env_boxes: Vec::new(),
        }
    }

    /// Token for one importer invocation: the stack is the entry frame plus
    /// the containing stylesheet when it differs (mirrors upstream, whose
    /// file/data entry is pushed at parse and never popped —
    /// context.cpp:576,618 — so `get_last_import` is never NULL during an
    /// importer callback; node-sass dereferences it unconditionally).
    pub(crate) fn for_import(containing_url: Option<&str>, entry: &ImportStackEntry) -> Self {
        let mut compiler = Self::new();
        compiler.import_stack.push(entry.clone());
        if let Some(url) = containing_url {
            if url != entry.abs_path && url != entry.imp_path {
                compiler.import_stack.push(ImportStackEntry {
                    imp_path: url.to_string(),
                    abs_path: url.to_string(),
                });
            }
        }
        compiler
    }
}

impl Drop for CompilerBox {
    fn drop(&mut self) {
        for &entry in &self.import_entries {
            // SAFETY: entries materialized by the import getters, owned here,
            // freed exactly once.
            unsafe {
                free_import_entry(entry);
            }
        }
        for &entry in &self.callee_entries {
            // SAFETY: entries materialized by the callee getters, owned here,
            // freed exactly once.
            unsafe {
                free_callee_entry(entry);
            }
        }
        for &entry in &self.env_boxes {
            // SAFETY: env boxes materialized with the callee entries, owned
            // here, freed exactly once.
            unsafe {
                free_env_box(entry);
            }
        }
    }
}

/// Allocates a NULL-terminated entry list with `length` slots (mirrors
/// `calloc(length + 1, …)`, sass_functions.cpp:17-19).
///
/// # Safety
///
/// Always safe to call; the result is owned by the caller (frees with
/// [`sass_delete_function_list`], or transfers to options).
#[no_mangle]
pub unsafe extern "C" fn sass_make_function_list(length: usize) -> *mut *mut FunctionEntry {
    guard(ptr::null_mut(), || {
        // SAFETY: fresh zeroed array of exactly `length + 1` slots (the
        // terminator included, so the list is valid even before any set).
        let list = malloc_or_abort(
            length
                .wrapping_add(1)
                .wrapping_mul(size_of::<*mut FunctionEntry>()),
        ) as *mut *mut FunctionEntry;
        unsafe {
            ptr::write_bytes(list, 0, length + 1);
        }
        list
    })
}

/// Creates one entry, COPYING `signature` (node-sass frees its buffer right
/// after). A NULL signature stores NULL (mirrors upstream, which copies
/// NULL→NULL); the compile-time parse then fails cleanly.
///
/// Mirrors `sass_make_function` (sass_functions.cpp:20-28).
///
/// # Safety
///
/// `signature` must be NULL or a valid NUL-terminated string for the call.
/// `callback` may be NULL (a NULL callback fails the compile with a clear
/// message instead of crashing at invocation). The result is owned by the
/// caller.
#[no_mangle]
pub unsafe extern "C" fn sass_make_function(
    signature: *const c_char,
    callback: SassFunctionFn,
    cookie: *mut c_void,
) -> *mut FunctionEntry {
    guard(ptr::null_mut(), || {
        let owned = if signature.is_null() {
            ptr::null_mut()
        } else {
            // SAFETY: non-null NUL-terminated string per contract above.
            let bytes = unsafe { CStr::from_ptr(signature) }.to_bytes();
            copy_bytes_nul(bytes)
        };
        // SAFETY: fresh box; ownership moves to the caller.
        Box::into_raw(Box::new(FunctionEntry {
            signature: owned,
            callback,
            cookie,
        }))
    })
}

/// Frees one entry (signature + struct; never the cookie — consumer-owned,
/// mirroring `sass_delete_function`, sass_functions.cpp:30-34). NULL-safe.
///
/// # Safety
///
/// `entry` must be NULL or a live entry, freed exactly once here.
#[no_mangle]
pub unsafe extern "C" fn sass_delete_function(entry: *mut FunctionEntry) {
    guard((), || {
        if entry.is_null() {
            return;
        }
        // SAFETY: live entry per contract; Drop glue below frees the string.
        unsafe {
            drop(Box::from_raw(entry));
        }
    });
}

impl Drop for FunctionEntry {
    fn drop(&mut self) {
        if !self.signature.is_null() {
            // SAFETY: owned allocation (copied at make), freed exactly once.
            unsafe {
                libc::free(self.signature as *mut libc::c_void);
            }
        }
    }
}

/// Frees a NULL-terminated list and every entry in it (NULL-list safe).
/// A NULL *entry* terminates the walk (mirrors the `while (*list)` loop,
/// sass_functions.cpp:37-46) — sparse lists after a hole are NOT freed
/// (same as upstream; holes only arise from consumer misuse).
///
/// # Safety
///
/// `list` must be NULL or a live NULL-terminated list whose entries are all
/// live and freed exactly once here.
#[no_mangle]
pub unsafe extern "C" fn sass_delete_function_list(list: *mut *mut FunctionEntry) {
    guard((), || {
        // SAFETY: NULL/live per contract above.
        unsafe { free_function_list(list) };
    });
}

/// The list-free primitive shared by the explicit delete, the options Drop,
/// and set-overwrite (all free lists+entries, never cookies).
///
/// # Safety
///
/// See [`sass_delete_function_list`].
pub(crate) unsafe fn free_function_list(list: *mut *mut FunctionEntry) {
    if list.is_null() {
        return;
    }
    // SAFETY: live NULL-terminated list per contract; entries live.
    unsafe {
        let mut cur = list;
        while !(*cur).is_null() {
            drop(Box::from_raw(*cur));
            cur = cur.add(1);
        }
        libc::free(list as *mut libc::c_void);
    }
}

/// Reads the entry at `pos` (borrowed). Out-of-range dereference is consumer
/// UB like upstream (`list[pos]`, sass_functions.cpp:49) — but NULL lists
/// harden to NULL (upstream would crash on those too).
///
/// # Safety
///
/// `list` must be NULL or a live list; `pos` must be in range for a non-NULL
/// list. Borrowed; freed with the list.
#[no_mangle]
pub unsafe extern "C" fn sass_function_get_list_entry(
    list: *mut *mut FunctionEntry,
    pos: usize,
) -> *mut FunctionEntry {
    guard(ptr::null_mut(), || {
        if list.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live list + in-range per contract above.
        unsafe { *list.add(pos) }
    })
}

/// Stores `entry` at `pos` (no ownership change — the list owns it either
/// way). Same NULL/range contract as the getter.
///
/// Mirrors `sass_function_set_list_entry` (sass_functions.cpp:50).
///
/// # Safety
///
/// `list` must be NULL or a live list (NULL-safe no-op); `pos` must be in
/// range; `entry` must be NULL or a live entry whose ownership the list
/// takes.
#[no_mangle]
pub unsafe extern "C" fn sass_function_set_list_entry(
    list: *mut *mut FunctionEntry,
    pos: usize,
    entry: *mut FunctionEntry,
) {
    guard((), || {
        if list.is_null() {
            return;
        }
        // SAFETY: live list + in-range per contract above.
        unsafe {
            *list.add(pos) = entry;
        }
    })
}

/// Reads the entry's signature (borrowed; NULL entry hardens to NULL).
///
/// Mirrors `sass_function_get_signature` (sass_functions.cpp:52).
///
/// # Safety
///
/// `entry` must be NULL or a live entry for the call. Borrowed; freed with
/// the entry.
#[no_mangle]
pub unsafe extern "C" fn sass_function_get_signature(entry: *mut FunctionEntry) -> *const c_char {
    guard(ptr::null(), || {
        if entry.is_null() {
            return ptr::null();
        }
        // SAFETY: live entry per contract above.
        unsafe { (*entry).signature as *const c_char }
    })
}

/// Reads the entry's callback (NULL entry hardens to None).
///
/// Mirrors `sass_function_get_function` (sass_functions.cpp:53).
///
/// # Safety
///
/// `entry` must be NULL or a live entry for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_function_get_function(entry: *mut FunctionEntry) -> SassFunctionFn {
    guard(None, || {
        if entry.is_null() {
            return None;
        }
        // SAFETY: live entry per contract above.
        unsafe { (*entry).callback }
    })
}

/// Reads the entry's cookie (NULL entry hardens to NULL).
///
/// Mirrors `sass_function_get_cookie` (sass_functions.cpp:54).
///
/// # Safety
///
/// `entry` must be NULL or a live entry for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_function_get_cookie(entry: *mut FunctionEntry) -> *mut c_void {
    guard(ptr::null_mut(), || {
        if entry.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live entry per contract above.
        unsafe { (*entry).cookie }
    })
}

/// Parses a host signature (`name(params)`) into a name and `ParameterList`.
///
/// Port of the embedded bridge's `parse_host_signature`
/// (rust-sass-embedded/src/function.rs:70-136), which itself matches Dart's
/// `ScssParser.parseSignature`: `identifier()` then balanced-paren params
/// then `expectDone()` (no whitespace adjacent to the name or parens, no
/// trailing input), with Dart's exact `Invalid signature "{sig}": {detail}`
/// errors. Deliberately strict (plan step-6 record): upstream registers the
/// leading identifier with best-effort params and ignores trailing garbage,
/// but no working consumer produces such signatures and fail-fast beats a
/// latent arity error; the message matches our embedded bridge.
fn parse_c_signature<'compile, 'parse>(
    sig: &str,
    arena: &'compile Bump,
) -> SassResult<(String, ParameterList<'parse>)>
where
    'compile: 'parse,
{
    let bytes = sig.as_bytes();
    let first = *bytes
        .first()
        .ok_or_else(|| signature_error(sig, "Expected identifier.", 0, arena))?;
    if !is_name_start(first) {
        return Err(signature_error(sig, "Expected identifier.", 0, arena));
    }
    let mut i = 1;
    while i < bytes.len() && is_name(bytes[i]) {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'(' {
        return Err(signature_error(sig, "expected \"(\".", i, arena));
    }
    let name = &sig[..i];
    let open = i;
    let mut close = None;
    let mut depth = 1usize;
    for (j, &b) in bytes.iter().enumerate().skip(open + 1) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(j);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close.ok_or_else(|| signature_error(sig, "expected \")\".", sig.len(), arena))?;
    if close + 1 != sig.len() {
        return Err(signature_error(
            sig,
            "expected no more input.",
            close + 1,
            arena,
        ));
    }
    let params_str = &sig[open + 1..close];
    // Bare `...` (no `$`) is what node-sass generates for signature-less
    // functions (`foo` → `foo(...)`; lib/index.js `normalizeFunctionSignature`,
    // which also unwraps the rest pack back in JS-land). It is not valid
    // Sass, so route it through the real parser as an equivalent rest
    // parameter (a name the parser accepts but no consumer can collide
    // with). Anything else shaped oddly stays strict.
    let params_src = if params_str.trim() == "..." {
        format!("@function {name}($__rest...) {{")
    } else {
        format!("@function {name}({params_str}) {{")
    };
    let params = parse_parameter_list(&params_src, "", arena)
        .map_err(|_| signature_error(sig, "expected \")\".", open + 1, arena))?;
    // Rebase the declaration span onto `"{name}({params})"` (offset 0) like
    // the embedded bridge (Dart parses the raw signature string).
    let signature_text = format!("{name}({params_str})");
    let source = FileSource::new_in(arena, &signature_text, None);
    let prefix = "@function ".len();
    let rebased = FileSpan::new(
        Some(source),
        params.span.start_location().offset.saturating_sub(prefix),
        params.span.end_location().offset.saturating_sub(prefix),
    );
    Ok((
        name.to_string(),
        ParameterList::new(params.parameters, rebased, params.rest_parameter),
    ))
}

/// Dart `isNameStart`: `_`, `-`, ASCII alpha, or non-ASCII.
fn is_name_start(b: u8) -> bool {
    b == b'_' || b == b'-' || b.is_ascii_alphabetic() || b >= 0x80
}

fn is_name(b: u8) -> bool {
    is_name_start(b) || b.is_ascii_digit()
}

/// Builds the signature parse error: a spanned `Sass` error (surfaces as a
/// compilation failure, like the embedded bridge's `CompileFailure`).
fn signature_error(sig: &str, detail: &str, offset: usize, arena: &Bump) -> Box<SassError> {
    let source = FileSource::new_in(arena, sig, None);
    let span = FileSpan::new(Some(source), offset, offset);
    match SourceSpanWithContext::from_file_span(&span) {
        Ok(span) => Box::new(SassError::Sass {
            message: format!("Invalid signature \"{sig}\": {detail}"),
            span,
            cause: None,
            loaded_urls: vec![],
        }),
        // Unreachable for constructed spans (offsets are in range by
        // construction); a `Script` error still fails the compile.
        Err(_) => Box::new(SassError::Script {
            message: format!("Invalid signature \"{sig}\": {detail}"),
            argument_name: None,
        }),
    }
}

/// Converts a C value to a core value (result direction).
///
/// All 7 data kinds convert; C error/warning values become `Script` errors
/// (there is no such `ValueKind` — the bridge formats call-site errors
/// itself before reaching here, so this arm is defensive).
/// Strings use lossy UTF-8 (C buffers are arbitrary bytes; Sass needs `str`).
/// `HASH` separators (a pre-evaluation sentinel, never produced by eval)
/// map to `Comma`, the function-argument default.
///
/// Shared with the env bridge (`crate::env`), which converts through the
/// token arena.
pub(crate) fn c_to_core<'compile, 'parse>(
    v: *const SassValue,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    // SAFETY: the bridge passes live values (or NULL, which hardens below —
    // a NULL result becomes a call-site error, never UB).
    let tag = unsafe {
        if v.is_null() {
            return Err(Box::new(SassError::Script {
                message: "custom function returned NULL".to_string(),
                argument_name: None,
            }));
        }
        tag_of(v)
    };
    // SAFETY: non-null live value per the check above; arms read only their
    // own fields, and every string payload is copied into the arena.
    unsafe {
        match tag {
            SASS_NULL => Ok(Value::new_with_arena(arena, ValueKind::Null)),
            SASS_BOOLEAN => Ok(Value::new_with_arena(
                arena,
                ValueKind::Boolean(SassBoolean::new(sass_boolean_get_value(v))),
            )),
            SASS_NUMBER => {
                let unit = CStr::from_ptr(sass_number_get_unit(v)).to_bytes();
                let unit = String::from_utf8_lossy(unit).into_owned();
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Number(SassNumber::new(
                        sass_number_get_value(v),
                        if unit.is_empty() { None } else { Some(&unit) },
                    )),
                ))
            }
            SASS_STRING => {
                let text = CStr::from_ptr(sass_string_get_value(v)).to_bytes();
                let text = String::from_utf8_lossy(text).into_owned();
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::String(SassString::new(
                        arena.alloc_str(&text),
                        sass_string_is_quoted(v),
                    )),
                ))
            }
            SASS_COLOR => Ok(Value::new_with_arena(
                arena,
                ValueKind::Color(SassColor::rgb(
                    sass_color_get_r(v),
                    sass_color_get_g(v),
                    sass_color_get_b(v),
                    sass_color_get_a(v),
                )),
            )),
            SASS_LIST => {
                let len = sass_list_get_length(v);
                // COMMA stays comma; SPACE stays space; HASH (a
                // pre-evaluation sentinel, never produced by eval) falls to
                // Comma, the function-argument default.
                let sep = match sass_list_get_separator(v) {
                    SASS_COMMA => ListSeparator::Comma,
                    SASS_SPACE => ListSeparator::Space,
                    _ => ListSeparator::Comma,
                };
                let mut contents = Vec::with_capacity(len);
                for i in 0..len {
                    contents.push(c_to_core(sass_list_get_value(v, i), arena)?);
                }
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::List(SassList::new(contents, sep, sass_list_get_is_bracketed(v))),
                ))
            }
            SASS_MAP => {
                let len = sass_map_get_length(v);
                let mut entries = Vec::with_capacity(len);
                for i in 0..len {
                    entries.push((
                        c_to_core(sass_map_get_key(v, i), arena)?,
                        c_to_core(sass_map_get_value(v, i), arena)?,
                    ));
                }
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::Map(SassMap::from_entries(entries)),
                ))
            }
            SASS_ERROR => Err(Box::new(SassError::Script {
                message: string_payload(sass_error_get_message(v)),
                argument_name: None,
            })),
            SASS_WARNING => Err(Box::new(SassError::Script {
                message: string_payload(sass_warning_get_message(v)),
                argument_name: None,
            })),
            _ => Err(Box::new(SassError::Script {
                message: "unknown sass value type".to_string(),
                argument_name: None,
            })),
        }
    }
}

/// Reads a borrowed C string payload lossily (NULL → empty).
///
/// # Safety
///
/// `s` must be NULL or a valid NUL-terminated string.
unsafe fn string_payload(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    // SAFETY: per contract above.
    String::from_utf8_lossy(unsafe { CStr::from_ptr(s) }.to_bytes()).into_owned()
}

/// Converts a core value to a fresh C value (argument direction), or fails
/// with a `Script` error for kinds with no C representation (upstream's
/// `"unknown sass value type"`, values.cpp:72).
///
/// Separators: `Space`→`SPACE`, `Comma`→`COMMA`; `Slash`/`Undecided` degrade
/// to `SPACE` (no C counterpart — documented). Numbers need exactly one
/// numerator unit and no denominators or slash-pair (else the compound
/// value is unrepresentable); non-`Rgb` colors likewise (a conversion pass
/// can land later if a consumer demands it).
///
/// Shared with the env bridge (`crate::env`).
pub(crate) fn core_to_c(v: Value<'_>) -> SassResult<*mut SassValue> {
    // SAFETY: every maker below receives valid inputs; all results are fresh
    // malloc'd values owned by the caller.
    unsafe {
        match v.kind() {
            ValueKind::Boolean(b) => Ok(sass_make_boolean(b.value)),
            ValueKind::Null => Ok(sass_make_null()),
            ValueKind::String(s) => {
                let text = CString::new(s.text).map_err(|_| {
                    Box::new(SassError::Script {
                        message: "strings with NUL bytes have no C representation".to_string(),
                        argument_name: None,
                    })
                })?;
                Ok(if s.has_quotes {
                    sass_make_qstring(text.as_ptr())
                } else {
                    sass_make_string(text.as_ptr())
                })
            }
            ValueKind::Number(n) => {
                if !n.denominator_units.is_empty()
                    || n.numerator_units.len() > 1
                    || n.as_slash.is_some()
                {
                    return Err(Box::new(SassError::Script {
                        message: "compound number units have no C representation".to_string(),
                        argument_name: None,
                    }));
                }
                let unit = n.numerator_units.first().cloned().unwrap_or_default();
                let unit = CString::new(unit).map_err(|_| {
                    Box::new(SassError::Script {
                        message: "units with NUL bytes have no C representation".to_string(),
                        argument_name: None,
                    })
                })?;
                Ok(sass_make_number(n.value, unit.as_ptr()))
            }
            ValueKind::Color(c) => {
                if c.space != ColorSpace::Rgb {
                    return Err(Box::new(SassError::Script {
                        message: "non-RGB colors have no C representation".to_string(),
                        argument_name: None,
                    }));
                }
                Ok(sass_make_color(c.channel0, c.channel1, c.channel2, c.alpha))
            }
            ValueKind::List(l) => {
                let sep = match l.separator {
                    ListSeparator::Comma => SASS_COMMA,
                    _ => SASS_SPACE,
                };
                let out = sass_make_list(l.contents.len(), sep, l.has_brackets);
                if out.is_null() {
                    return Err(Box::new(SassError::Script {
                        message: "out of memory".to_string(),
                        argument_name: None,
                    }));
                }
                for (i, item) in l.contents.iter().enumerate() {
                    sass_list_set_value(out, i, core_to_c(*item)?);
                }
                Ok(out)
            }
            ValueKind::Map(m) => {
                let out = sass_make_map(m.entries.len());
                if out.is_null() {
                    return Err(Box::new(SassError::Script {
                        message: "out of memory".to_string(),
                        argument_name: None,
                    }));
                }
                for (i, (k, val)) in m.entries.iter().enumerate() {
                    sass_map_set_key(out, i, core_to_c(*k)?);
                    sass_map_set_value(out, i, core_to_c(*val)?);
                }
                Ok(out)
            }
            // Rest-arg packs (`foo(...)` variadics): upstream converts
            // `Arguments` to a comma list the same way (`ast2c.cpp`).
            // Keywords are swallowed (marked accessed so the post-call
            // unused-keyword check stays quiet) — documented.
            ValueKind::ArgumentList(l) => {
                l.were_keywords_accessed.set(true);
                let sep = match l.list.separator {
                    ListSeparator::Comma => SASS_COMMA,
                    _ => SASS_SPACE,
                };
                let out = sass_make_list(l.list.contents.len(), sep, l.list.has_brackets);
                if out.is_null() {
                    return Err(Box::new(SassError::Script {
                        message: "out of memory".to_string(),
                        argument_name: None,
                    }));
                }
                for (i, item) in l.list.contents.iter().enumerate() {
                    sass_list_set_value(out, i, core_to_c(*item)?);
                }
                Ok(out)
            }
            _ => Err(Box::new(SassError::Script {
                message: "unknown sass value type".to_string(),
                argument_name: None,
            })),
        }
    }
}

/// Builds core callables from a NULL-terminated C function list (NULL list
/// → no functions). Signature failures fail the whole compile with the
/// `Invalid signature` error (like the embedded bridge's `CompileFailure`);
///
/// NULL callbacks fail the compile too (upstream would crash at invocation).
pub(crate) fn build_custom_callables<'compile, 'parse>(
    list: *mut *mut FunctionEntry,
    arena: &'compile Bump,
) -> SassResult<Vec<Callable<'compile, 'parse>>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let mut out = Vec::new();
    if list.is_null() {
        return Ok(out);
    }
    // SAFETY: live NULL-terminated list per contract (entries live until the
    // owning options/context is deleted — strictly longer than this compile).
    let mut count = 0usize;
    unsafe {
        while !(*list.add(count)).is_null() {
            count += 1;
        }
    }
    for i in 0..count {
        // SAFETY: index in range per the count above.
        let entry = unsafe { *list.add(i) };
        if entry.is_null() {
            continue;
        }
        // SAFETY: live entry per contract. The cookie flows to the consumer
        // through `entry` itself (`sass_function_get_cookie`, as in
        // node-sass) — the bridge passes the entry through untouched.
        let (sig, callback) = unsafe {
            (
                sass_function_get_signature(entry),
                sass_function_get_function(entry),
            )
        };
        let sig_text = if sig.is_null() {
            String::new()
        } else {
            // SAFETY: non-null NUL-terminated signature per maker contract.
            unsafe { CStr::from_ptr(sig) }
                .to_string_lossy()
                .into_owned()
        };
        let (name, params) = parse_c_signature(&sig_text, arena)?;
        let Some(callback) = callback else {
            return Err(Box::new(SassError::Script {
                message: format!("custom function '{sig_text}' has no callback"),
                argument_name: None,
            }));
        };
        let fn_name = name.clone();
        let cb: SyncBuiltInCallback<'compile, 'parse> = Rc::new(move |_, state, args, arena| {
            guard(
                Err(Box::new(SassError::Script {
                    message: format!("custom function '{fn_name}' failed"),
                    argument_name: None,
                })),
                || invoke_c_function(fn_name.clone(), entry, callback, args, state, arena),
            )
        });
        out.push(Callable::new(
            arena,
            CallableKind::BuiltIn(BuiltInCallable::new(name, params, cb)),
        ));
    }
    Ok(out)
}

/// Invokes one C callback: core argv → C comma list → call → convert back.
/// Cleans up both temporaries (alias-guarded like upstream's
/// `if (c_val != c_args)` — extended to entries, which upstream misses and
/// would double-free).
///
/// The per-call token snapshots the callee chain and captures the live env
/// (plan §7 step 8): `get_last_callee` sees the running C function on top
/// with its call-site span, and the env getters read the caller's frame
/// (upstream exposes the caller env directly — params are argv-only).
fn invoke_c_function<'compile, 'parse>(
    name: String,
    entry: *mut FunctionEntry,
    callback: unsafe extern "C" fn(
        *const SassValue,
        *mut FunctionEntry,
        *mut CompilerBox,
    ) -> *mut SassValue,
    args: Vec<Value<'parse>>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    // Marshal argv (core → C). A conversion failure aborts before the call,
    // deleting whatever was already built.
    let mut converted: Vec<*mut SassValue> = Vec::with_capacity(args.len());
    for arg in &args {
        match core_to_c(*arg) {
            Ok(v) => converted.push(v),
            Err(e) => {
                for v in converted {
                    // SAFETY: fresh values from this function.
                    unsafe { sass_delete_value(v) };
                }
                return Err(e);
            }
        }
    }
    // SAFETY: fresh list; slots start NULL and each set below takes ownership
    // of one `converted` entry.
    let c_args = unsafe { sass_make_list(converted.len(), SASS_COMMA, false) };
    if c_args.is_null() {
        for v in converted {
            // SAFETY: fresh values from this function.
            unsafe { sass_delete_value(v) };
        }
        return Err(Box::new(SassError::Script {
            message: "out of memory".to_string(),
            argument_name: None,
        }));
    }
    for (i, &v) in converted.iter().enumerate() {
        // SAFETY: index in range by construction; slot was NULL.
        unsafe { sass_list_set_value(c_args, i, v) };
    }
    // Per-call token: callee snapshot (strings copied — G3) plus the live
    // env and compile arena, lifetime-erased. Sound: the compile arena
    // outlives the whole compile (so frame-0 writes stay valid), and the
    // token — the only owner of the erased handles — is dropped at the end
    // of this call, confining every use to the callback duration (the same
    // confinement as upstream's pop-dangled pointers, minus the dangle).
    let mut compiler = CompilerBox::new();
    compiler.callees = snapshot_callees(state, &name);
    compiler.live_env = Some(unsafe {
        std::mem::transmute::<Environment<'_, '_>, Environment<'static, 'static>>(state.env)
    });
    compiler.arena = Some(unsafe { std::mem::transmute::<&Bump, &'static Bump>(arena) });
    // SAFETY: `c_args` is a live list; `entry` outlives the compile per
    // contract; `compiler` is stack-local for the call. The consumer must
    // not retain pointers or unwind across the C boundary.
    let c_val = unsafe { callback(c_args as *const SassValue, entry, &mut compiler) };
    // Cleanup helper: the callback may return the args list or one of its
    // entries (degenerate but must not double-free). `converted` still holds
    // the entry addresses (Copy) for the check.
    let cleanup = |c_val: *mut SassValue| {
        // SAFETY: `c_args` and every `converted` entry are fresh temporaries
        // from this function.
        unsafe {
            sass_delete_value(c_args);
            if c_val != c_args && !converted.contains(&c_val) {
                sass_delete_value(c_val);
            }
        }
    };
    if c_val.is_null() {
        cleanup(c_val);
        return Err(Box::new(SassError::Script {
            message: format!("custom function '{name}' returned NULL"),
            argument_name: None,
        }));
    }
    // SAFETY: non-null live value per the NULL check above.
    let tag = unsafe { sass_value_get_tag(c_val) };
    // Error/warning returns raise call-site errors (upstream `eval.cpp`
    // format); the core span-wraps the `Script` error.
    if tag == SASS_ERROR || tag == SASS_WARNING {
        // SAFETY: live value of the matching kind per tag check above.
        let (message, kind) = unsafe {
            if tag == SASS_ERROR {
                (string_payload(sass_error_get_message(c_val)), "error")
            } else {
                (string_payload(sass_warning_get_message(c_val)), "warning")
            }
        };
        cleanup(c_val);
        return Err(Box::new(SassError::Script {
            message: format!("{kind} in C function {name}: {message}"),
            argument_name: None,
        }));
    }
    // Convert (deep copy out), then clean up (conversion failure still frees
    // both temporaries — no leak on the error path).
    let out = c_to_core(c_val, arena);
    cleanup(c_val);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::values::{
        sass_boolean_get_value, sass_delete_value, sass_list_get_length, sass_list_get_value,
        sass_make_boolean, sass_make_error, sass_make_list, sass_make_number, sass_make_qstring,
        sass_number_get_unit, sass_number_get_value, sass_string_get_value, sass_string_is_quoted,
        sass_value_get_tag,
    };

    fn test_arena() -> Bump {
        Bump::new()
    }

    unsafe fn read_opt(ptr: *const c_char) -> Option<Vec<u8>> {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: test-only; non-null pointers are valid strings produced by
        // the functions under test.
        Some(unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec())
    }

    #[test]
    fn signature_vectors() {
        let arena = test_arena();
        // Valid: name + params survive; invalid: strict errors.
        let (name, params) = parse_c_signature("foo($x)", &arena).unwrap();
        assert_eq!(name, "foo");
        assert_eq!(params.parameters.len(), 1);
        let (name, params) = parse_c_signature("foo()", &arena).unwrap();
        assert_eq!(name, "foo");
        assert_eq!(params.parameters.len(), 0);
        let (name, params) = parse_c_signature("foo($x, $y: 1, $rest...)", &arena).unwrap();
        assert_eq!(name, "foo");
        assert_eq!(params.parameters.len(), 2);
        assert!(params.rest_parameter.is_some());
        // Bare `...` (node-sass signature-less shape) parses as rest-only.
        let (name, params) = parse_c_signature("foo(...)", &arena).unwrap();
        assert_eq!(name, "foo");
        assert!(params.parameters.is_empty());
        assert!(params.rest_parameter.is_some());
        for bad in [
            "",
            " foo()",
            "foo() ",
            "foo ()",
            "foo(arg)",
            "!!!",
            "foo($x",
            "foo($x))x",
        ] {
            assert!(
                parse_c_signature(bad, &arena).is_err(),
                "should reject: {bad:?}"
            );
        }
        // Error shape matches the embedded bridge: spanned `Sass` with the
        // `Invalid signature` message.
        let err = parse_c_signature("foo ()", &arena).unwrap_err();
        assert!(
            err.message().contains("Invalid signature"),
            "got: {}",
            err.message()
        );
        assert!(err.span().is_some());
    }

    #[test]
    fn conversion_roundtrips() {
        let arena = test_arena();
        // Number (unit + unitless).
        unsafe {
            let px = CString::new("px").unwrap();
            let c = sass_make_number(1.5, px.as_ptr());
            let core = c_to_core(c, &arena).unwrap();
            assert!(matches!(*core, ValueKind::Number(_)));
            let back = core_to_c(core).unwrap();
            assert_eq!(sass_value_get_tag(back), SASS_NUMBER);
            assert_eq!(sass_number_get_value(back), 1.5);
            assert_eq!(read_opt(sass_number_get_unit(back)), Some(b"px".to_vec()));
            sass_delete_value(c);
            sass_delete_value(back);
            let u = sass_make_number(2.0, CString::new("").unwrap().as_ptr());
            let core = c_to_core(u, &arena).unwrap();
            let back = core_to_c(core).unwrap();
            assert_eq!(read_opt(sass_number_get_unit(back)), Some(b"".to_vec()));
            sass_delete_value(u);
            sass_delete_value(back);
        }
        // String quotedness both directions.
        unsafe {
            let q = sass_make_qstring(CString::new("a").unwrap().as_ptr());
            let core = c_to_core(q, &arena).unwrap();
            let back = core_to_c(core).unwrap();
            assert!(sass_string_is_quoted(back));
            assert_eq!(read_opt(sass_string_get_value(back)), Some(b"a".to_vec()));
            sass_delete_value(q);
            sass_delete_value(back);
        }
        // Color channels.
        unsafe {
            let c = sass_make_color(1.0, 2.0, 3.0, 0.5);
            let core = c_to_core(c, &arena).unwrap();
            let back = core_to_c(core).unwrap();
            assert_eq!(sass_color_get_r(back), 1.0);
            assert_eq!(sass_color_get_a(back), 0.5);
            sass_delete_value(c);
            sass_delete_value(back);
        }
        // Nested list + map.
        unsafe {
            let inner = sass_make_list(1, SASS_COMMA, false);
            sass_list_set_value(inner, 0, sass_make_boolean(true));
            let core = c_to_core(inner, &arena).unwrap();
            let back = core_to_c(core).unwrap();
            assert_eq!(sass_list_get_length(back), 1);
            assert!(sass_boolean_get_value(sass_list_get_value(back, 0)));
            sass_delete_value(inner);
            sass_delete_value(back);
            // Separators map; HASH (never from eval) degrades to Comma here
            // only in the sense that non-COMMA/SPACE fall to Space — HASH
            // lands on Space per the documented rule.
            let sp = sass_make_list(0, SASS_SPACE, false);
            let core = c_to_core(sp, &arena).unwrap();
            assert!(matches!(*core, ValueKind::List(_)));
            sass_delete_value(sp);
        }
    }

    #[test]
    fn conversion_degrades() {
        let arena = test_arena();
        // C error/warning values become Script errors (no such ValueKind).
        unsafe {
            let e = sass_make_error(CString::new("x").unwrap().as_ptr());
            assert!(c_to_core(e, &arena).is_err());
            sass_delete_value(e);
        }
        // Core-only kinds fail core→C with the upstream conversion text.
        let calc = Value::new_with_arena(
            &arena,
            ValueKind::Calculation(Box::new(
                rust_sass::value::SassCalculation::new_unsimplified(
                    "calc",
                    vec![rust_sass::value::CalcArgument::Number(SassNumber::new(
                        1.0, None,
                    ))],
                ),
            )),
        );
        let err = core_to_c(calc).unwrap_err();
        assert!(
            err.message().contains("unknown sass value type"),
            "got: {}",
            err.message()
        );
        // Complex units fail.
        let complex = Value::new_with_arena(
            &arena,
            ValueKind::Number(SassNumber::with_units(
                1.0,
                vec!["px".to_string(), "em".to_string()],
                vec![],
            )),
        );
        assert!(core_to_c(complex).is_err());
        // Non-RGB colors fail.
        let hsl = Value::new_with_arena(
            &arena,
            ValueKind::Color(SassColor::hsl(10.0, 0.5, 0.5, 1.0).unwrap()),
        );
        assert!(core_to_c(hsl).is_err());
        // RGB passes.
        let rgb =
            Value::new_with_arena(&arena, ValueKind::Color(SassColor::rgb(1.0, 2.0, 3.0, 1.0)));
        assert!(core_to_c(rgb).is_ok());
    }

    #[test]
    fn entry_null_hardening() {
        unsafe {
            assert!(sass_function_get_signature(ptr::null_mut()).is_null());
            assert!(sass_function_get_function(ptr::null_mut()).is_none());
            assert!(sass_function_get_cookie(ptr::null_mut()).is_null());
            assert!(sass_function_get_list_entry(ptr::null_mut(), 0).is_null());
            sass_delete_function(ptr::null_mut());
            sass_delete_function_list(ptr::null_mut());
            sass_function_set_list_entry(ptr::null_mut(), 0, ptr::null_mut());
        }
    }

    #[test]
    fn build_empty_and_invalid() {
        let arena = test_arena();
        // NULL list → no functions, no error.
        let empty = build_custom_callables(ptr::null_mut(), &arena).unwrap();
        assert!(empty.is_empty());
        drop(empty);
        // NULL callback fails the compile (upstream would crash at call).
        unsafe {
            let list = sass_make_function_list(1);
            let sig = CString::new("foo()").unwrap();
            let entry = sass_make_function(sig.as_ptr(), None, ptr::null_mut());
            sass_function_set_list_entry(list, 0, entry);
            assert!(build_custom_callables(list, &arena).is_err());
            sass_delete_function_list(list);
        }
    }
}
