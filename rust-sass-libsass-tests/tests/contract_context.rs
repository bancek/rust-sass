// dart-source: N/A (C-ABI contract tests — no Dart counterpart; see
//   docs/plans/libsass.md §8/D11 and §13 TDD policy).
//
// Context contract, first increment (plan §6 step 4, vertical slice):
// make/delete × file/data, get_context upcasts, get/set_options plumbing,
// and a trivial compile round-trip. Error-JSON fidelity, source-map options,
// and take_* transfer arrive in later increments — this pins lifecycle and
// the happy path first.
//
// These assertions run IDENTICALLY against our library (default features)
// and upstream libsass (`--no-default-features --features upstream`).
//
// Written FIRST (TDD): validated green on the upstream leg before the adapter
// implemented them.

use rust_sass_libsass_tests::bindings::*;
use std::ffi::{c_char, CStr, CString};

/// Reads a borrowed NUL-terminated C string into bytes (NULL → None).
unsafe fn read_opt(ptr: *const c_char) -> Option<Vec<u8>> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: test-only; non-null pointers here are valid strings produced by
    // the function under test, borrowed (not freed) for the read.
    Some(unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec())
}

/// Makes a data context from a Rust string. The C API takes ownership of a
/// malloc'd buffer (sassc `compile_stdin` hands over `malloc`'d input), so
/// the test allocates via `sass_copy_c_string` and never frees it afterwards.
unsafe fn make_data(source: &str) -> *mut Sass_Data_Context {
    let owned = CString::new(source).unwrap();
    // SAFETY: test-only; the copy is a fresh allocation whose ownership
    // transfers to the context.
    let buf = unsafe { sass_copy_c_string(owned.as_ptr()) };
    unsafe { sass_make_data_context(buf) }
}

#[test]
fn make_delete_file_context_contract() {
    unsafe {
        let path = CString::new("in.scss").unwrap();
        let ctx = sass_make_file_context(path.as_ptr());
        assert!(!ctx.is_null());
        // The context carries its options inline (get_context upcast +
        // get_options return the embedded options, not copies).
        let base = sass_file_context_get_context(ctx);
        assert!(!base.is_null());
        let opts = sass_file_context_get_options(ctx);
        assert!(!opts.is_null());
        assert_eq!(sass_context_get_options(base), opts);
        // input_path was copied in at make time.
        assert_eq!(
            read_opt(sass_option_get_input_path(opts)),
            Some(b"in.scss".to_vec())
        );
        // Fresh contexts report no error and no output.
        assert_eq!(sass_context_get_error_status(base), 0);
        assert!(sass_context_get_output_string(base).is_null());
        sass_delete_file_context(ctx);
        // NULL and empty paths are maker errors (non-null context with
        // error_status set — the later compile early-returns it).
        let null_ctx = sass_make_file_context(std::ptr::null());
        assert!(!null_ctx.is_null());
        assert_ne!(
            sass_context_get_error_status(sass_file_context_get_context(null_ctx)),
            0
        );
        sass_delete_file_context(null_ctx);
        let empty = CString::new("").unwrap();
        let empty_ctx = sass_make_file_context(empty.as_ptr());
        assert!(!empty_ctx.is_null());
        assert_ne!(
            sass_context_get_error_status(sass_file_context_get_context(empty_ctx)),
            0
        );
        sass_delete_file_context(empty_ctx);
    }
}

#[test]
fn make_delete_data_context_contract() {
    unsafe {
        let ctx = make_data("a { b: c; }");
        assert!(!ctx.is_null());
        let base = sass_data_context_get_context(ctx);
        assert!(!base.is_null());
        let opts = sass_data_context_get_options(ctx);
        assert!(!opts.is_null());
        assert_eq!(sass_context_get_options(base), opts);
        assert_eq!(sass_context_get_error_status(base), 0);
        assert!(sass_context_get_output_string(base).is_null());
        sass_delete_data_context(ctx);
        // NULL source is a maker error, like the file side.
        let null_ctx = sass_make_data_context(std::ptr::null_mut());
        assert!(!null_ctx.is_null());
        assert_ne!(
            sass_context_get_error_status(sass_data_context_get_context(null_ctx)),
            0
        );
        sass_delete_data_context(null_ctx);
    }
}

#[test]
fn set_options_move_contract() {
    // `*_set_options` moves the caller's options into the context (plan G2):
    // the context observes the moved values afterwards.
    unsafe {
        let ctx = make_data("a { b: c; }");
        let opts = sass_make_options();
        sass_option_set_precision(opts, 7);
        sass_option_set_output_style(opts, Sass_Output_Style_SASS_STYLE_COMPRESSED);
        sass_data_context_set_options(ctx, opts);
        let ctx_opts = sass_data_context_get_options(ctx);
        assert_eq!(sass_option_get_precision(ctx_opts), 7);
        assert_eq!(
            sass_option_get_output_style(ctx_opts),
            Sass_Output_Style_SASS_STYLE_COMPRESSED
        );
        // The emptied shell must survive deletion (move/steal, not aliasing).
        sass_delete_options(opts);
        sass_delete_data_context(ctx);

        let fpath = CString::new("x.scss").unwrap();
        let fctx = sass_make_file_context(fpath.as_ptr());
        let fopts = sass_make_options();
        sass_option_set_precision(fopts, 3);
        sass_file_context_set_options(fctx, fopts);
        assert_eq!(
            sass_option_get_precision(sass_file_context_get_options(fctx)),
            3
        );
        sass_delete_options(fopts);
        sass_delete_file_context(fctx);
    }
}

#[test]
fn data_compile_roundtrip_contract() {
    unsafe {
        let ctx = make_data("a { b: c; }");
        let status = sass_compile_data_context(ctx);
        let base = sass_data_context_get_context(ctx);
        assert_eq!(status, 0);
        assert_eq!(sass_context_get_error_status(base), 0);
        let css = read_opt(sass_context_get_output_string(base)).unwrap();
        let text = String::from_utf8(css).unwrap();
        assert!(text.contains("a {"), "got: {text}");
        assert!(text.contains("b: c"), "got: {text}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn nested_style_contract() {
    // Explicit NESTED renders libsass nested (`a {\n  b: c; }`) on both
    // legs byte-identically (upstream's native default; ours via
    // `OutputStyle::Nested` with evaluator-stamped tabs).
    unsafe {
        let ctx = make_data("a { b: c; }");
        let opts = sass_data_context_get_options(ctx);
        sass_option_set_output_style(opts, Sass_Output_Style_SASS_STYLE_NESTED);
        assert_eq!(sass_compile_data_context(ctx), 0);
        let base = sass_data_context_get_context(ctx);
        let css = read_opt(sass_context_get_output_string(base)).unwrap();
        assert_eq!(String::from_utf8(css).unwrap(), "a {\n  b: c; }\n");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn linefeed_lfcr_contract() {
    // Non-LF linefeeds flow verbatim (node-sass `lfcr`): both legs emit
    // `\n\r` separators. The buffer must outlive the context (borrowed
    // setter, plan G19 — freed only after delete).
    unsafe {
        let ctx = make_data("a { b: c; }");
        let opts = sass_data_context_get_options(ctx);
        let lf = CString::new("\n\r").unwrap();
        sass_option_set_linefeed(opts, lf.as_ptr());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let base = sass_data_context_get_context(ctx);
        let css = read_opt(sass_context_get_output_string(base)).unwrap();
        let text = String::from_utf8(css).unwrap();
        assert!(text.contains("{\n\r"), "got: {text:?}");
        sass_delete_data_context(ctx);
        drop(lf);
    }
}

#[test]
fn compact_style_contract() {
    // Explicit COMPACT renders one block per line (`a { b: c; }`) on both
    // legs byte-identically (upstream's native style; ours via
    // `OutputStyle::Compact`).
    unsafe {
        let ctx = make_data("a { b: c; d: e; }");
        let opts = sass_data_context_get_options(ctx);
        sass_option_set_output_style(opts, Sass_Output_Style_SASS_STYLE_COMPACT);
        assert_eq!(sass_compile_data_context(ctx), 0);
        let base = sass_data_context_get_context(ctx);
        let css = read_opt(sass_context_get_output_string(base)).unwrap();
        assert_eq!(String::from_utf8(css).unwrap(), "a { b: c; d: e; }\n");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn data_compile_error_contract() {
    unsafe {
        let ctx = make_data("a { b: ; }");
        let status = sass_compile_data_context(ctx);
        let base = sass_data_context_get_context(ctx);
        assert_ne!(status, 0);
        assert_ne!(sass_context_get_error_status(base), 0);
        // Error text is present; output is forced NULL on failure.
        assert!(read_opt(sass_context_get_error_message(base)).is_some());
        assert!(sass_context_get_output_string(base).is_null());
        sass_delete_data_context(ctx);
    }
}

#[test]
fn compile_null_context_contract() {
    unsafe {
        assert_eq!(sass_compile_data_context(std::ptr::null_mut()), 1);
        assert_eq!(sass_compile_file_context(std::ptr::null_mut()), 1);
    }
}
