// dart-source: N/A (C-ABI contract tests — no Dart counterpart; see
//   docs/plans/libsass.md §8/D11 and §13 TDD policy).
//
// Staged-compiler contract: `sass_make_*_compiler`, `sass_compiler_parse`,
// `sass_compiler_execute`, `sass_delete_compiler`, the staged getters
// (`get_state/get_context/get_options`), and the `sass_context_take_*`
// transfer family.
//
// These assertions run IDENTICALLY against our library (default features)
// and upstream libsass (`--no-default-features --features upstream`).
// NULL-compiler probes are limited to what upstream survives: `parse`,
// `execute` (both return 1) and `delete` (safe no-op). The getters
// dereference unconditionally upstream, so NULL-getter hardening is covered
// only by the adapter's in-module tests, never here.
//
// Written FIRST (TDD): validated green on the upstream leg before the adapter
// implemented them.

use rust_sass_libsass_tests::bindings::*;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

/// Reads a borrowed NUL-terminated C string into bytes (NULL → None).
unsafe fn read_opt(ptr: *const c_char) -> Option<Vec<u8>> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: test-only; non-null pointers here are valid strings produced by
    // the function under test, borrowed (not freed) for the read.
    Some(unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec())
}

/// Reads a NULL-terminated C string array into Rust strings (NULL → empty).
unsafe fn read_files(arr: *mut *mut c_char) -> Vec<String> {
    let mut out = Vec::new();
    if arr.is_null() {
        return out;
    }
    // SAFETY: test-only; NULL-terminated array of live strings per the
    // included-files contract, borrowed for the read.
    unsafe {
        let mut cur = arr;
        while !(*cur).is_null() {
            let bytes = read_opt(*cur as *const c_char).unwrap_or_default();
            out.push(String::from_utf8_lossy(&bytes).into_owned());
            cur = cur.add(1);
        }
    }
    out
}

/// Makes a data context from a Rust string. The C API takes ownership of a
/// malloc'd buffer, so the test allocates via `sass_copy_c_string` and never
/// frees it afterwards.
unsafe fn make_data(source: &str) -> *mut Sass_Data_Context {
    let owned = CString::new(source).unwrap();
    // SAFETY: test-only; the copy is a fresh allocation whose ownership
    // transfers to the context.
    let buf = unsafe { sass_copy_c_string(owned.as_ptr()) };
    unsafe { sass_make_data_context(buf) }
}

/// Creates a scratch dir (unique per process and call) with the given files.
/// Returns the dir; the caller removes it when done.
fn scratch_files(files: &[(&str, &str)]) -> PathBuf {
    // Tests share one process and run on threads: the counter keeps
    // identical fixtures from colliding on the same dir name.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "rust-sass-staged-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, contents) in files {
        std::fs::write(dir.join(name), contents).unwrap();
    }
    dir
}

#[test]
fn staged_data_roundtrip_contract() {
    unsafe {
        let ctx = make_data("a { b: c; }");
        let base = sass_data_context_get_context(ctx);
        let compiler = sass_make_data_compiler(ctx);
        assert!(!compiler.is_null());
        assert_eq!(
            sass_compiler_get_state(compiler),
            Sass_Compiler_State_SASS_COMPILER_CREATED
        );
        assert_eq!(sass_compiler_parse(compiler), 0);
        assert_eq!(
            sass_compiler_get_state(compiler),
            Sass_Compiler_State_SASS_COMPILER_PARSED
        );
        // No output until execute (upstream renders only in execute).
        assert!(sass_context_get_output_string(base).is_null());
        assert_eq!(sass_compiler_execute(compiler), 0);
        assert_eq!(
            sass_compiler_get_state(compiler),
            Sass_Compiler_State_SASS_COMPILER_EXECUTED
        );
        assert_eq!(sass_context_get_error_status(base), 0);
        let css = read_opt(sass_context_get_output_string(base)).unwrap();
        let text = String::from_utf8(css).unwrap();
        assert!(text.contains("a {"), "got: {text}");
        assert!(text.contains("b: c"), "got: {text}");
        sass_delete_compiler(compiler);
        sass_delete_data_context(ctx);
    }
}

#[test]
fn staged_file_roundtrip_contract() {
    unsafe {
        let dir = scratch_files(&[("main.scss", "a { b: c; }\n")]);
        let path = CString::new(dir.join("main.scss").to_str().unwrap().to_owned()).unwrap();
        let ctx = sass_make_file_context(path.as_ptr());
        let base = sass_file_context_get_context(ctx);
        let compiler = sass_make_file_compiler(ctx);
        assert!(!compiler.is_null());
        assert_eq!(sass_compiler_parse(compiler), 0);
        assert_eq!(sass_compiler_execute(compiler), 0);
        assert_eq!(sass_context_get_error_status(base), 0);
        let css = read_opt(sass_context_get_output_string(base)).unwrap();
        let text = String::from_utf8(css).unwrap();
        assert!(text.contains("b: c"), "got: {text}");
        sass_delete_compiler(compiler);
        sass_delete_file_context(ctx);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn staged_state_machine_contract() {
    unsafe {
        let ctx = make_data("a { b: c; }");
        let compiler = sass_make_data_compiler(ctx);
        assert!(!compiler.is_null());
        // Execute before parse is refused.
        assert_eq!(sass_compiler_execute(compiler), -1);
        assert_eq!(
            sass_compiler_get_state(compiler),
            Sass_Compiler_State_SASS_COMPILER_CREATED
        );
        // Parse is idempotent.
        assert_eq!(sass_compiler_parse(compiler), 0);
        assert_eq!(sass_compiler_parse(compiler), 0);
        // Execute is idempotent.
        assert_eq!(sass_compiler_execute(compiler), 0);
        assert_eq!(sass_compiler_execute(compiler), 0);
        // Parse after execute is refused.
        assert_eq!(sass_compiler_parse(compiler), -1);
        sass_delete_compiler(compiler);
        sass_delete_data_context(ctx);

        // NULL makers return NULL; NULL parse/execute fail; NULL delete is safe.
        assert!(sass_make_data_compiler(std::ptr::null_mut()).is_null());
        assert!(sass_make_file_compiler(std::ptr::null_mut()).is_null());
        assert_eq!(sass_compiler_parse(std::ptr::null_mut()), 1);
        assert_eq!(sass_compiler_execute(std::ptr::null_mut()), 1);
        sass_delete_compiler(std::ptr::null_mut());
    }
}

#[test]
fn staged_parse_error_returns_zero_contract() {
    unsafe {
        // Unmatched brace: a syntax error in both engines. Upstream's parse
        // still returns 0 — the failure surfaces via the context fields.
        let ctx = make_data("}");
        let base = sass_data_context_get_context(ctx);
        let compiler = sass_make_data_compiler(ctx);
        assert_eq!(sass_compiler_parse(compiler), 0);
        assert_ne!(sass_context_get_error_status(base), 0);
        assert!(sass_context_get_output_string(base).is_null());
        assert!(read_opt(sass_context_get_error_message(base)).is_some());
        // Execute on a failed parse refuses (nonzero).
        assert_ne!(sass_compiler_execute(compiler), 0);
        sass_delete_compiler(compiler);
        sass_delete_data_context(ctx);
    }
}

#[test]
fn staged_eval_error_contract() {
    unsafe {
        // Undefined variable: upstream's parser evaluates eagerly, so the
        // error is already set after parse; ours surfaces it at execute
        // (documented error-timing divergence). The common contract both
        // legs honor: parse returns 0, execute fails, and the final state
        // carries the error with NULL output.
        let ctx = make_data("a { b: $undefined-var; }");
        let base = sass_data_context_get_context(ctx);
        let compiler = sass_make_data_compiler(ctx);
        assert_eq!(sass_compiler_parse(compiler), 0);
        assert_ne!(sass_compiler_execute(compiler), 0);
        assert_ne!(sass_context_get_error_status(base), 0);
        assert!(sass_context_get_output_string(base).is_null());
        assert!(read_opt(sass_context_get_error_message(base)).is_some());
        sass_delete_compiler(compiler);
        sass_delete_data_context(ctx);
    }
}

#[test]
fn staged_included_files_after_parse_contract() {
    unsafe {
        let dir = scratch_files(&[
            ("main.scss", "@import \"partial\";\n"),
            ("_partial.scss", "x { y: z; }\n"),
        ]);
        let path = CString::new(dir.join("main.scss").to_str().unwrap().to_owned()).unwrap();
        let ctx = sass_make_file_context(path.as_ptr());
        let base = sass_file_context_get_context(ctx);
        let compiler = sass_make_file_compiler(ctx);
        assert_eq!(sass_compiler_parse(compiler), 0);
        // The header's documented use case: files are queryable after parse,
        // before execute — and still no output yet.
        let files = read_files(sass_context_get_included_files(base));
        assert_eq!(sass_context_get_included_files_size(base), files.len());
        assert_eq!(files.len(), 2, "got: {files:?}");
        assert!(
            files.iter().any(|f| f.ends_with("main.scss")),
            "got: {files:?}"
        );
        assert!(
            files.iter().any(|f| f.ends_with("_partial.scss")),
            "got: {files:?}"
        );
        assert!(sass_context_get_output_string(base).is_null());
        assert_eq!(sass_compiler_execute(compiler), 0);
        let css = read_opt(sass_context_get_output_string(base)).unwrap();
        let text = String::from_utf8(css).unwrap();
        assert!(text.contains("y: z"), "got: {text}");
        sass_delete_compiler(compiler);
        sass_delete_file_context(ctx);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn staged_getters_contract() {
    unsafe {
        let ctx = make_data("a { b: c; }");
        let base = sass_data_context_get_context(ctx);
        let opts = sass_data_context_get_options(ctx);
        let compiler = sass_make_data_compiler(ctx);
        // The staged getters expose the same context/options the context
        // getters return.
        assert_eq!(sass_compiler_get_context(compiler), base);
        assert_eq!(sass_compiler_get_options(compiler), opts);
        sass_delete_compiler(compiler);
        sass_delete_data_context(ctx);

        let fpath = CString::new("x.scss").unwrap();
        let fctx = sass_make_file_context(fpath.as_ptr());
        let fbase = sass_file_context_get_context(fctx);
        let fopts = sass_file_context_get_options(fctx);
        let fcompiler = sass_make_file_compiler(fctx);
        assert_eq!(sass_compiler_get_context(fcompiler), fbase);
        assert_eq!(sass_compiler_get_options(fcompiler), fopts);
        sass_delete_compiler(fcompiler);
        sass_delete_file_context(fctx);
    }
}

#[test]
fn take_output_transfer_contract() {
    unsafe {
        let ctx = make_data("a { b: c; }");
        assert_eq!(sass_compile_data_context(ctx), 0);
        let base = sass_data_context_get_context(ctx);
        // Take transfers ownership: content matches, slot reads NULL after,
        // second take is NULL, and deleting the context afterwards is safe
        // (no double free — a violation aborts the test binary).
        let taken = sass_context_take_output_string(base);
        assert!(!taken.is_null());
        let text =
            String::from_utf8(CStr::from_ptr(taken as *const c_char).to_bytes().to_vec()).unwrap();
        assert!(text.contains("b: c"), "got: {text}");
        sass_free_memory(taken as *mut c_void);
        assert!(sass_context_get_output_string(base).is_null());
        assert!(sass_context_take_output_string(base).is_null());
        sass_delete_data_context(ctx);
    }
}

#[test]
fn take_error_transfer_contract() {
    unsafe {
        let ctx = make_data("}");
        assert_ne!(sass_compile_data_context(ctx), 0);
        let base = sass_data_context_get_context(ctx);
        for take in [
            sass_context_take_error_json as unsafe extern "C" fn(*mut Sass_Context) -> *mut c_char,
            sass_context_take_error_text,
            sass_context_take_error_message,
        ] {
            let taken = take(base);
            assert!(!taken.is_null());
            sass_free_memory(taken as *mut c_void);
        }
        assert!(sass_context_get_error_json(base).is_null());
        assert!(sass_context_get_error_text(base).is_null());
        assert!(sass_context_get_error_message(base).is_null());
        // Location takes are present when the error carries a span.
        let file = sass_context_take_error_file(base);
        assert!(!file.is_null());
        sass_free_memory(file as *mut c_void);
        let src = sass_context_take_error_src(base);
        assert!(!src.is_null());
        sass_free_memory(src as *mut c_void);
        sass_delete_data_context(ctx);
    }
}

#[test]
fn take_included_files_transfer_contract() {
    unsafe {
        let dir = scratch_files(&[
            ("main.scss", "@import \"partial\";\n"),
            ("_partial.scss", "x { y: z; }\n"),
        ]);
        let path = CString::new(dir.join("main.scss").to_str().unwrap().to_owned()).unwrap();
        let ctx = sass_make_file_context(path.as_ptr());
        assert_eq!(sass_compile_file_context(ctx), 0);
        let base = sass_file_context_get_context(ctx);
        let taken = sass_context_take_included_files(base);
        assert!(!taken.is_null());
        let files = read_files(taken);
        assert_eq!(files.len(), 2, "got: {files:?}");
        // Free the transferred array (strings + array) the way a C consumer
        // would; the context must remain deletable afterwards.
        let mut cur = taken;
        while !(*cur).is_null() {
            sass_free_memory(*cur as *mut c_void);
            cur = cur.add(1);
        }
        sass_free_memory(taken as *mut c_void);
        assert_eq!(sass_context_get_included_files_size(base), 0);
        sass_delete_file_context(ctx);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
