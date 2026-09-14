// dart-source: N/A (C-ABI contract tests — no Dart counterpart; see
//   docs/plans/libsass.md §8/D11 and §13 TDD policy).
//
// Callee-stack + environment contract: `sass_compiler_get_callee_*` chain
// (names, types, spans), `sass_callee_get_*` accessors, and the six
// `sass_env_get/set_*` accessors through live C callbacks.
//
// Written FIRST (TDD): validated green on the upstream leg before the adapter
// implemented them. Ownership rules pinned here (verified against upstream
// sass_functions.cpp + eval.cpp:1049-1121 + expand.cpp:793-838):
// - Callee/import pointers die on pop (plan G3): entries are borrowed for
//   the callback (never delete them); strings are snapshots.
// - `sass_callee_get_env` borrows the frame env (valid only during the
//   callback, like upstream's `&entry->env`).
// - Env getters return fresh values (caller deletes); setters never steal
//   the caller's value (plan §A.6).
// - Variable names carry the `$` on both legs (upstream `lex_variable`
//   keeps it; our adapter strips one leading `$` for the core's bare keys).
// - `set_lexical` inside a C callback writes through to the caller env
//   (probed upstream: the callee env aliases the caller frame — params are
//   argv-only and never env-bound, so `get_lexical("$param")` misses).

use rust_sass_libsass_tests::bindings::*;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::Mutex;

/// Reads a borrowed NUL-terminated C string into bytes (NULL → None).
unsafe fn read_opt(ptr: *const c_char) -> Option<Vec<u8>> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: test-only; non-null pointers here are valid strings produced by
    // the function under test, borrowed (not freed) for the read.
    Some(unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec())
}

unsafe fn read_str(ptr: *const c_char) -> String {
    String::from_utf8(unsafe { read_opt(ptr) }.unwrap_or_default()).unwrap_or_default()
}

/// Makes a data context from a Rust string (ownership transfers like
/// sassc's stdin path).
unsafe fn make_data(source: &str) -> *mut Sass_Data_Context {
    let owned = CString::new(source).unwrap();
    // SAFETY: test-only; fresh allocation, ownership transfers to the context.
    let buf = unsafe { sass_copy_c_string(owned.as_ptr()) };
    unsafe { sass_make_data_context(buf) }
}

unsafe fn css_of(ctx: *mut Sass_Data_Context) -> String {
    let base = unsafe { sass_data_context_get_context(ctx) };
    String::from_utf8(unsafe { read_opt(sass_context_get_output_string(base)) }.unwrap()).unwrap()
}

unsafe fn err_of(ctx: *mut Sass_Data_Context) -> String {
    let base = unsafe { sass_data_context_get_context(ctx) };
    String::from_utf8(unsafe { read_opt(sass_context_get_error_message(base)) }.unwrap()).unwrap()
}

/// Registers ONE C function on a data context's options (list owned by the
/// context afterwards — never delete it). The signature buffer is freed
/// before returning, pinning the maker's copy semantics on both legs.
unsafe fn register_one(
    ctx: *mut Sass_Data_Context,
    sig: &str,
    cb: unsafe extern "C" fn(
        *const Sass_Value,
        Sass_Function_Entry,
        *mut Sass_Compiler,
    ) -> *mut Sass_Value,
    cookie: *mut c_void,
) {
    let opts = unsafe { sass_data_context_get_options(ctx) };
    let list = unsafe { sass_make_function_list(1) };
    assert!(!list.is_null());
    let sig_buf = CString::new(sig).unwrap();
    let entry = unsafe { sass_make_function(sig_buf.as_ptr(), Some(cb), cookie) };
    assert!(!entry.is_null());
    drop(sig_buf);
    unsafe {
        sass_function_set_list_entry(list, 0, entry);
        sass_option_set_c_functions(opts, list);
    }
}

#[derive(Default)]
struct CalleeProbe {
    size: usize,
    last_name: String,
    last_type: u32,
    last_env_null: bool,
    parent_name: String,
    parent_type: u32,
}

/// One probe static + callback per callee test: tests run in parallel
/// threads, so sharing a single static races (callbacks interleave across
/// concurrent compiles).
macro_rules! probe_for {
    ($static:ident, $cb:ident) => {
        static $static: Mutex<CalleeProbe> = Mutex::new(CalleeProbe {
            size: 0,
            last_name: String::new(),
            last_type: 0,
            last_env_null: true,
            parent_name: String::new(),
            parent_type: 0,
        });

        /// Records the callee chain into its static, returns 0.
        unsafe extern "C" fn $cb(
            _args: *const Sass_Value,
            _cb: Sass_Function_Entry,
            compiler: *mut Sass_Compiler,
        ) -> *mut Sass_Value {
            unsafe {
                let mut probe = $static.lock().unwrap();
                *probe = CalleeProbe::default();
                probe.size = sass_compiler_get_callee_stack_size(compiler);
                let last = sass_compiler_get_last_callee(compiler);
                if !last.is_null() {
                    probe.last_name = read_str(sass_callee_get_name(last));
                    probe.last_type = sass_callee_get_type(last);
                    probe.last_env_null = sass_callee_get_env(last).is_null();
                }
                if probe.size >= 2 {
                    let parent = sass_compiler_get_callee_entry(compiler, probe.size - 2);
                    if !parent.is_null() {
                        probe.parent_name = read_str(sass_callee_get_name(parent));
                        probe.parent_type = sass_callee_get_type(parent);
                    }
                }
                let empty = CString::new("").unwrap();
                sass_make_number(0.0, empty.as_ptr())
            }
        }
    };
}

probe_for!(PROBE_SELF, callee_probe_self);
probe_for!(PROBE_FN, callee_probe_fn);
probe_for!(PROBE_MIXIN, callee_probe_mixin);

fn take_probe(probe: &Mutex<CalleeProbe>) -> (usize, String, u32, bool, String, u32) {
    let probe = probe.lock().unwrap();
    (
        probe.size,
        probe.last_name.clone(),
        probe.last_type,
        probe.last_env_null,
        probe.parent_name.clone(),
        probe.parent_type,
    )
}

/// Returns `sass_env_get_lexical(env, "$g")` (cloned) or an error value.
unsafe extern "C" fn env_echo_global(
    _args: *const Sass_Value,
    cb: Sass_Function_Entry,
    compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe {
        let _ = (cb, compiler);
        // Env comes from our own callee (innermost frame).
        let callee = sass_compiler_get_last_callee(compiler);
        if callee.is_null() {
            return sass_make_error(CString::new("no callee").unwrap().as_ptr());
        }
        let env = sass_callee_get_env(callee);
        let name = CString::new("$g").unwrap();
        let got = sass_env_get_lexical(env, name.as_ptr());
        if got.is_null() {
            return sass_make_error(CString::new("missing g").unwrap().as_ptr());
        }
        got
    }
}

/// Returns `sass_env_get_lexical(env, "$x")` (the bound param).
unsafe extern "C" fn env_echo_param(
    _args: *const Sass_Value,
    _cb: Sass_Function_Entry,
    compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe {
        let callee = sass_compiler_get_last_callee(compiler);
        let env = sass_callee_get_env(callee);
        let name = CString::new("$x").unwrap();
        let got = sass_env_get_lexical(env, name.as_ptr());
        if got.is_null() {
            return sass_make_error(CString::new("missing x").unwrap().as_ptr());
        }
        got
    }
}

/// Returns `sass_env_get_lexical(env, "$loc")` (nested lexical read).
unsafe extern "C" fn env_echo_nested(
    _args: *const Sass_Value,
    _cb: Sass_Function_Entry,
    compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe {
        let callee = sass_compiler_get_last_callee(compiler);
        let env = sass_callee_get_env(callee);
        let name = CString::new("$loc").unwrap();
        let got = sass_env_get_lexical(env, name.as_ptr());
        if got.is_null() {
            return sass_make_error(CString::new("missing loc").unwrap().as_ptr());
        }
        got
    }
}

/// Returns `sass_env_get_local(env, "$lx")` (misses from nested sites).
unsafe extern "C" fn env_echo_local_global(
    _args: *const Sass_Value,
    _cb: Sass_Function_Entry,
    compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe {
        let callee = sass_compiler_get_last_callee(compiler);
        let env = sass_callee_get_env(callee);
        let name = CString::new("$lx").unwrap();
        let got = sass_env_get_local(env, name.as_ptr());
        if got.is_null() {
            return sass_make_error(CString::new("missing local lx").unwrap().as_ptr());
        }
        got
    }
}

/// Returns `sass_env_get_local(env, "$loc")`.
unsafe extern "C" fn env_echo_local_nested(
    _args: *const Sass_Value,
    _cb: Sass_Function_Entry,
    compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe {
        let callee = sass_compiler_get_last_callee(compiler);
        let env = sass_callee_get_env(callee);
        let name = CString::new("$loc").unwrap();
        let got = sass_env_get_local(env, name.as_ptr());
        if got.is_null() {
            return sass_make_error(CString::new("missing local loc").unwrap().as_ptr());
        }
        got
    }
}

/// Returns `sass_env_get_global(env, "$g")`.
unsafe extern "C" fn env_echo_global_frame(
    _args: *const Sass_Value,
    _cb: Sass_Function_Entry,
    compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe {
        let callee = sass_compiler_get_last_callee(compiler);
        let env = sass_callee_get_env(callee);
        let name = CString::new("$g").unwrap();
        let got = sass_env_get_global(env, name.as_ptr());
        if got.is_null() {
            return sass_make_error(CString::new("missing global g").unwrap().as_ptr());
        }
        got
    }
}

/// `set_lexical("$tmp", 99)`, returns 1.
unsafe extern "C" fn env_set_tmp(
    _args: *const Sass_Value,
    _cb: Sass_Function_Entry,
    compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe {
        let callee = sass_compiler_get_last_callee(compiler);
        let env = sass_callee_get_env(callee);
        let name = CString::new("$tmp").unwrap();
        let ninety_nine = sass_make_number(99.0, CString::new("").unwrap().as_ptr());
        sass_env_set_lexical(env, name.as_ptr(), ninety_nine);
        sass_delete_value(ninety_nine);
        sass_make_number(1.0, CString::new("").unwrap().as_ptr())
    }
}

/// `set_global("$gg", 7)`, returns 1.
unsafe extern "C" fn env_set_global(
    _args: *const Sass_Value,
    _cb: Sass_Function_Entry,
    compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe {
        let callee = sass_compiler_get_last_callee(compiler);
        let env = sass_callee_get_env(callee);
        let name = CString::new("$gg").unwrap();
        let seven = sass_make_number(7.0, CString::new("").unwrap().as_ptr());
        sass_env_set_global(env, name.as_ptr(), seven);
        sass_delete_value(seven);
        sass_make_number(1.0, CString::new("").unwrap().as_ptr())
    }
}

#[test]
fn callee_self_contract() {
    unsafe {
        let ctx = make_data("a { x: probe(); }");
        register_one(ctx, "probe()", callee_probe_self, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let (size, name, ctype, env_null, _, _) = take_probe(&PROBE_SELF);
        assert!(size >= 1, "size: {size}");
        assert!(name.contains("probe"), "name: {name}");
        assert_eq!(ctype, Sass_Callee_Type_SASS_CALLEE_C_FUNCTION);
        assert!(!env_null);
        sass_delete_data_context(ctx);
    }
}

#[test]
fn callee_chain_function_contract() {
    unsafe {
        let ctx = make_data("@function outer() { @return probe2(1); } a { x: outer(); }");
        register_one(ctx, "probe2($x)", callee_probe_fn, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let (size, name, ctype, _, parent, ptype) = take_probe(&PROBE_FN);
        assert!(size >= 2, "size: {size}");
        assert!(name.contains("probe2"), "name: {name}");
        assert_eq!(ctype, Sass_Callee_Type_SASS_CALLEE_C_FUNCTION);
        assert!(parent.contains("outer"), "parent: {parent}");
        assert_eq!(ptype, Sass_Callee_Type_SASS_CALLEE_FUNCTION);
        sass_delete_data_context(ctx);
    }
}

#[test]
fn callee_chain_mixin_contract() {
    unsafe {
        let ctx = make_data("@mixin m { a { x: probe3(); } } @include m;");
        register_one(ctx, "probe3()", callee_probe_mixin, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let (size, name, ctype, _, parent, ptype) = take_probe(&PROBE_MIXIN);
        assert!(size >= 2, "size: {size}");
        assert!(name.contains("probe3"), "name: {name}");
        assert_eq!(ctype, Sass_Callee_Type_SASS_CALLEE_C_FUNCTION);
        assert!(parent.contains("m"), "parent: {parent}");
        assert_eq!(ptype, Sass_Callee_Type_SASS_CALLEE_MIXIN);
        sass_delete_data_context(ctx);
    }
}

#[test]
fn env_lexical_contract() {
    unsafe {
        let ctx = make_data("$g: 42; a { x: getg(); }");
        register_one(ctx, "getg()", env_echo_global, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("x: 42;"), "got: {css}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn env_param_invisible_contract() {
    // Params are argv-only, never env-bound (probed upstream: the callee env
    // is the caller's frame, while params live in the popped `fn_env`).
    // `get_lexical("$x")` misses, the error value raises at the call site.
    unsafe {
        let ctx = make_data("a { x: getx(7); }");
        register_one(ctx, "getx($x)", env_echo_param, std::ptr::null_mut());
        assert_ne!(sass_compile_data_context(ctx), 0);
        let msg = err_of(ctx);
        assert!(msg.contains("missing x"), "got: {msg}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn env_nested_lexical_contract() {
    unsafe {
        let ctx = make_data("@function outer() { $loc: 11; @return getloc(); } a { x: outer(); }");
        register_one(ctx, "getloc()", env_echo_nested, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("x: 11;"), "got: {css}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn env_local_contract() {
    // `get_local` reads the current frame only (probed upstream: a global
    // is invisible from a nested call site). Inside a function body the
    // local is in the current frame.
    unsafe {
        let ctx = make_data("@function outer() { $loc: 12; @return getl2(); } a { x: outer(); }");
        register_one(ctx, "getl2()", env_echo_local_nested, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("x: 12;"), "got: {css}");
        sass_delete_data_context(ctx);

        // ...while a global is NOT visible to `get_local` from a nested site.
        let ctx = make_data("$lx: 3; @function outer() { @return getl3(); } a { x: outer(); }");
        register_one(ctx, "getl3()", env_echo_local_global, std::ptr::null_mut());
        assert_ne!(sass_compile_data_context(ctx), 0);
        let msg = err_of(ctx);
        assert!(msg.contains("missing local lx"), "got: {msg}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn env_global_read_contract() {
    unsafe {
        let ctx = make_data("$g: 5; a { x: getgg(); }");
        register_one(ctx, "getgg()", env_echo_global_frame, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("x: 5;"), "got: {css}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn env_set_writes_through_contract() {
    // `set_lexical` inside a C callback writes the caller's frame (probed
    // upstream: the callee env aliases the caller env, no fresh scope) —
    // `$tmp` is visible after the call.
    unsafe {
        let ctx = make_data("a { x: settmp(1); y: $tmp; }");
        register_one(ctx, "settmp($x)", env_set_tmp, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("y: 99;"), "got: {css}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn env_set_global_contract() {
    unsafe {
        let ctx = make_data("$gg: 0; a { x: setgg(); } b { y: $gg; }");
        register_one(ctx, "setgg()", env_set_global, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("y: 7;"), "got: {css}");
        sass_delete_data_context(ctx);
    }
}
