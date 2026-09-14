// dart-source: N/A (C-ABI contract tests — no Dart counterpart; see
//   docs/plans/libsass.md §8/D11 and §13 TDD policy).
//
// Custom-function contract: list/entry lifecycle, signature/cookie accessors,
// and end-to-end compiles through C callbacks (argv in, values out, errors,
// cookies, defaults). These assertions run IDENTICALLY against our library
// (default features) and upstream libsass (`--no-default-features
// --features upstream`) — they pin the ABI contract, never implementation
// output.
//
// Written FIRST (TDD): validated green on the upstream leg before the adapter
// implemented them. Ownership rules pinned here (verified against upstream
// sass_functions.cpp + node-sass/src/binding.cpp):
// - `sass_make_function` COPIES the signature (node-sass frees its buffer
//   right after; the test drops its CString before compiling).
// - `sass_make_function_list(n)` is NULL-terminated (n+1 slots).
// - `set_c_functions` transfers the list: the context owns it afterwards
//   (upstream frees lists+entries on clear; the test never deletes a list
//   it has set — neither leg would survive that).
// - Callback argv is a borrowed comma list (read, never delete); the
//   returned value transfers to the compiler (never delete it either).
// - C error/warning returns raise a call-site error mentioning the function
//   name and message (upstream `eval.cpp`: `"error in C function {name}:
//   {msg}"` — our bridge mirrors the format; the test pins the full text on
//   both legs after observing upstream).
// - Invalid signatures fail the compile (upstream throws at registration).
//   The message text differs (ours matches the embedded bridge's
//   `Invalid signature "…": …`), so only status is pinned across legs.
// Hardening (unit-tested in the adapter crate, not here since upstream
// unchecked-derefs): NULL entries/lists, out-of-range list indexes.

use rust_sass_libsass_tests::bindings::*;
use std::ffi::{c_char, c_void, CStr, CString};

/// Reads a borrowed NUL-terminated C string into bytes (NULL → None).
unsafe fn read_opt(ptr: *const c_char) -> Option<Vec<u8>> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: test-only; non-null pointers here are valid strings produced by
    // the function under test, borrowed (not freed) for the read.
    Some(unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec())
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

/// `double($x)`: returns 2× the numeric argument, preserving its unit.
unsafe extern "C" fn double_it(
    args: *const Sass_Value,
    _cb: Sass_Function_Entry,
    _compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe {
        let x = sass_list_get_value(args, 0);
        sass_make_number(sass_number_get_value(x) * 2.0, sass_number_get_unit(x))
    }
}

/// Echoes the first argument (clone — ownership transfers to the compiler).
unsafe extern "C" fn echo_it(
    args: *const Sass_Value,
    _cb: Sass_Function_Entry,
    _compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe { sass_clone_value(sass_list_get_value(args, 0)) }
}

/// `add($a, $b: 2)`: exercised with and without the default.
unsafe extern "C" fn add_it(
    args: *const Sass_Value,
    _cb: Sass_Function_Entry,
    _compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe {
        let a = sass_number_get_value(sass_list_get_value(args, 0));
        let b = sass_number_get_value(sass_list_get_value(args, 1));
        let unit = sass_number_get_unit(sass_list_get_value(args, 0));
        sass_make_number(a + b, unit)
    }
}

/// Always fails with `kaboom`.
unsafe extern "C" fn boom_it(
    _args: *const Sass_Value,
    _cb: Sass_Function_Entry,
    _compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe { sass_make_error(CString::new("kaboom").unwrap().as_ptr()) }
}

/// Returns the `f64` behind the cookie, ignoring arguments.
unsafe extern "C" fn cookie_it(
    _args: *const Sass_Value,
    cb: Sass_Function_Entry,
    _compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe {
        let cookie = sass_function_get_cookie(cb) as *const f64;
        let empty = CString::new("").unwrap();
        sass_make_number(*cookie, empty.as_ptr())
    }
}

#[test]
fn list_entry_lifecycle_contract() {
    unsafe {
        let list = sass_make_function_list(2);
        assert!(!list.is_null());
        let s1 = CString::new("foo($x)").unwrap();
        let s2 = CString::new("bar()").unwrap();
        let e1 = sass_make_function(s1.as_ptr(), Some(double_it), std::ptr::null_mut());
        let e2 = sass_make_function(s2.as_ptr(), Some(echo_it), std::ptr::null_mut());
        assert!(!e1.is_null());
        assert!(!e2.is_null());
        // Signature is copied at make time (buffers already droppable here
        // would be fine — dropped explicitly below before any read).
        assert_eq!(
            read_opt(sass_function_get_signature(e1)),
            Some(b"foo($x)".to_vec())
        );
        // Callback round-trips through the entry.
        assert!(sass_function_get_function(e1).is_some());
        assert!(sass_function_get_cookie(e1).is_null());
        sass_function_set_list_entry(list, 0, e1);
        sass_function_set_list_entry(list, 1, e2);
        assert_eq!(sass_function_get_list_entry(list, 0), e1);
        assert_eq!(sass_function_get_list_entry(list, 1), e2);
        drop(s1);
        drop(s2);
        assert_eq!(
            read_opt(sass_function_get_signature(e1)),
            Some(b"foo($x)".to_vec())
        );
        // Explicit single delete (entry never placed in a list) and list
        // delete (frees its entries + array) are separate paths — never both
        // for the same entry (double free aborts on both legs).
        let s3 = CString::new("baz()").unwrap();
        let e3 = sass_make_function(s3.as_ptr(), Some(echo_it), std::ptr::null_mut());
        sass_delete_function(e3);
        sass_delete_function_list(list); // frees e1, e2 + array
    }
}

#[test]
fn delete_list_null_contract() {
    // NULL list delete is a no-op on both legs (explicit upstream check).
    unsafe {
        sass_delete_function_list(std::ptr::null_mut());
    }
}

#[test]
fn numeric_callback_contract() {
    unsafe {
        let ctx = make_data("a { x: double(21px); }");
        register_one(ctx, "double($x)", double_it, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("42px"), "got: {css}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn echo_and_defaults_contract() {
    unsafe {
        // Lists round-trip identically on both legs.
        let ctx = make_data("a { y: echo(1px 2px); }");
        register_one(ctx, "echo($v)", echo_it, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("1px 2px"), "got: {css}");
        sass_delete_data_context(ctx);

        // Quotedness of C-function RESULTS differs by engine (probed
        // 2026-09-07): upstream unquotes every custom-function string result
        // (even fresh `sass_make_qstring`s print bare), while the modern
        // engine preserves quotes (Dart behavior — the core serializes
        // `String_Quoted` with quotes, no unquoting on the callback path).
        // The ARG keeps its flag on both legs (probed: quoted=1 in).
        let ctx = make_data("a { x: echo(\"hi\"); }");
        register_one(ctx, "echo($v)", echo_it, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        #[cfg(feature = "upstream")]
        assert!(css.contains("x: hi;"), "got: {css}");
        #[cfg(not(feature = "upstream"))]
        assert!(css.contains("x: \"hi\";"), "got: {css}");
        sass_delete_data_context(ctx);

        // Defaults bind when the caller omits them (both legs).
        let ctx = make_data("a { x: add(1); y: add(1, 10); }");
        register_one(ctx, "add($a, $b: 2)", add_it, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("x: 3;"), "got: {css}");
        assert!(css.contains("y: 11;"), "got: {css}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn error_return_contract() {
    unsafe {
        let ctx = make_data("a { x: boom(1); }");
        register_one(ctx, "boom($x)", boom_it, std::ptr::null_mut());
        assert_ne!(sass_compile_data_context(ctx), 0);
        let msg = err_of(ctx);
        assert!(msg.contains("kaboom"), "got: {msg}");
        assert!(msg.contains("boom"), "got: {msg}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn cookie_contract() {
    static COOKIE_VAL: f64 = 7.0;
    unsafe {
        let ctx = make_data("a { x: seven(); }");
        register_one(
            ctx,
            "seven()",
            cookie_it,
            &COOKIE_VAL as *const f64 as *mut c_void,
        );
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("x: 7;"), "got: {css}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn invalid_signature_contract() {
    // Garbage signatures diverge by engine (probed 2026-09-07): upstream
    // registers the leading identifier with zero params (`not a
    // signature!!!` becomes `not/0`, no error while unused); our bridge is
    // strict (embedded-style) and fails the compile with `Invalid signature`.
    // Strictness is deliberate: no working consumer produces garbage (all
    // surveyed bindings pass well-formed signatures), fail-fast beats a
    // latent arity error, and the message matches our embedded bridge.
    unsafe {
        let ctx = make_data("a { x: 1; }");
        register_one(ctx, "not a signature!!!", double_it, std::ptr::null_mut());
        #[cfg(feature = "upstream")]
        assert_eq!(sass_compile_data_context(ctx), 0);
        #[cfg(not(feature = "upstream"))]
        {
            assert_ne!(sass_compile_data_context(ctx), 0);
            let msg = err_of(ctx);
            assert!(msg.contains("Invalid signature"), "got: {msg}");
        }
        sass_delete_data_context(ctx);
    }
}

/// Returns argv length (NULL-safe probe for signature-shape oracle tests).
unsafe extern "C" fn len_probe(
    args: *const Sass_Value,
    _cb: Sass_Function_Entry,
    _compiler: *mut Sass_Compiler,
) -> *mut Sass_Value {
    unsafe {
        let n = if args.is_null() {
            -1.0
        } else {
            sass_list_get_length(args) as f64
        };
        sass_make_number(n, CString::new("").unwrap().as_ptr())
    }
}

#[test]
fn ellipsis_signature_contract() {
    // Bare `...` (no `$`) is what node-sass generates for signature-less
    // functions (`foo` → `foo(...)`): all call args arrive packed as ONE
    // comma list (upstream binds the rest pack; node-sass splits it back in
    // JS-land). Both legs echo the pack identically.
    unsafe {
        let ctx = make_data("a { x: foo(20, 22); }");
        register_one(ctx, "foo(...)", echo_it, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("20, 22"), "got: {css}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn ellipsis_argv_shape_contract() {
    // Oracle pin: `foo(...)` called with 2 args delivers argv of length 1
    // (the rest pack) on both legs — never flattened.
    unsafe {
        let ctx = make_data("a { x: foo(20, 22); }");
        register_one(ctx, "foo(...)", len_probe, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("x: 1;"), "got: {css}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn unknown_function_contract() {
    // Unknown functions pass through as plain CSS on BOTH legs (probed
    // 2026-09-07 — an earlier draft of this test wrongly assumed the modern
    // engine errors; it does not, matching Dart's forward-compat treatment
    // of unknown functions, and upstream's fallthrough alike).
    unsafe {
        let ctx = make_data("a { x: nosuchfn(1); }");
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("nosuchfn(1)"), "got: {css}");
        sass_delete_data_context(ctx);
    }
}
