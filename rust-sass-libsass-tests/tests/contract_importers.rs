// dart-source: N/A (C-ABI contract tests — no Dart counterpart; see
//   docs/plans/libsass.md §8/D11 and §13 TDD policy).
//
// Custom-importer contract: list/entry lifecycle, `{file,contents,map}`
// subsets, priority order, `Error` abort, NULL fallthrough, and multi-entry
// fan-out. Assertions run IDENTICALLY on both legs (default = ours,
// `upstream` = upstream libsass) except where a per-leg split is justified
// inline.
//
// Written FIRST (TDD): validated green on the upstream leg before the adapter
// implemented them. Ownership rules pinned here (verified against upstream
// sass_functions.cpp + context.cpp call_loader + node-sass
// custom_importer_bridge.cpp):
// - `sass_make_importer` stores fn/priority/cookie raw (no copy); delete frees
//   the struct only, never the cookie.
// - `sass_make_importer_list(n)` / `sass_make_import_list(n)` are
//   NULL-terminated (n+1 slots).
// - `sass_make_import_entry(path, source, srcmap)` copies `path` (for both
//   imp and abs), ADOPTS `source`/`srcmap` (must be malloc'd, e.g.
//   `sass_copy_c_string` — never pass stack buffers, never free after).
// - `sass_import_set_error` copies the message; falsy line/col (0) map to -1
//   (as `usize::MAX`); line 0 is inexpressible.
// - The returned `Sass_Import_List` transfers to the compiler (never delete
//   it in the callback); NULL return = fallthrough to next importer /
//   filesystem.
// - `set_c_importers` transfers the list (context owns it afterwards — never
//   delete a list that was set).
// - Dispatch order is customs-first per `@import` (upstream `call_loader`):
//   for a file entry with a filesystem-resolvable relative, the C importer
//   still fires before the filesystem; NULL falls back to disk.

use rust_sass_libsass_tests::bindings::*;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
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

/// Registers ONE C importer on a data context's options (list owned by the
/// context afterwards — never delete it).
unsafe fn register_one(
    ctx: *mut Sass_Data_Context,
    cb: Sass_Importer_Fn,
    priority: f64,
    cookie: *mut c_void,
) {
    let opts = unsafe { sass_data_context_get_options(ctx) };
    let list = unsafe { sass_make_importer_list(1) };
    assert!(!list.is_null());
    let entry = unsafe { sass_make_importer(cb, priority, cookie) };
    assert!(!entry.is_null());
    unsafe {
        sass_importer_set_list_entry(list, 0, entry);
        sass_option_set_c_importers(opts, list);
    }
}

/// Builds a single-entry import list transferring `source`/`srcmap` (both
/// must be malloc'd or NULL — the entry adopts them).
unsafe fn single_import(
    path: *const c_char,
    source: *mut c_char,
    srcmap: *mut c_char,
) -> Sass_Import_List {
    let list = unsafe { sass_make_import_list(1) };
    assert!(!list.is_null());
    let entry = unsafe { sass_make_import_entry(path, source, srcmap) };
    assert!(!entry.is_null());
    unsafe {
        sass_import_set_list_entry(list, 0, entry);
    }
    list
}

/// `contents` responder: serves `$color: red`-style virtual file `virtual`
/// with the given SCSS body, ignoring the URL.
unsafe extern "C" fn contents_importer(
    _url: *const c_char,
    _cb: Sass_Importer_Entry,
    _compiler: *mut Sass_Compiler,
) -> Sass_Import_List {
    unsafe {
        let path = CString::new("virtual").unwrap();
        let body = CString::new("$virt: red;").unwrap();
        single_import(
            path.as_ptr(),
            sass_copy_c_string(body.as_ptr()),
            std::ptr::null_mut(),
        )
    }
}

/// Fallthrough responder: always returns NULL (next importer / filesystem).
unsafe extern "C" fn null_importer(
    _url: *const c_char,
    _cb: Sass_Importer_Entry,
    _compiler: *mut Sass_Compiler,
) -> Sass_Import_List {
    std::ptr::null_mut()
}

/// Error responder: single error entry (`set_error(msg, -1, -1)` idiom —
/// `-1` as `usize::MAX`).
unsafe extern "C" fn error_importer(
    _url: *const c_char,
    _cb: Sass_Importer_Entry,
    _compiler: *mut Sass_Compiler,
) -> Sass_Import_List {
    unsafe {
        let list = sass_make_import_list(1);
        let entry =
            sass_make_import_entry(std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut());
        let msg = CString::new("import boom").unwrap();
        sass_import_set_error(entry, msg.as_ptr(), usize::MAX, usize::MAX);
        sass_import_set_list_entry(list, 0, entry);
        list
    }
}

/// Priority probe responders: each serves a distinct variable value.
unsafe extern "C" fn prio_low_importer(
    _url: *const c_char,
    _cb: Sass_Importer_Entry,
    _compiler: *mut Sass_Compiler,
) -> Sass_Import_List {
    unsafe {
        let path = CString::new("prio").unwrap();
        let body = CString::new("$prio: low;").unwrap();
        single_import(
            path.as_ptr(),
            sass_copy_c_string(body.as_ptr()),
            std::ptr::null_mut(),
        )
    }
}

/// Priority probe responders: each serves a distinct variable value.
unsafe extern "C" fn prio_high_importer(
    _url: *const c_char,
    _cb: Sass_Importer_Entry,
    _compiler: *mut Sass_Compiler,
) -> Sass_Import_List {
    unsafe {
        let path = CString::new("prio").unwrap();
        let body = CString::new("$prio: high;").unwrap();
        single_import(
            path.as_ptr(),
            sass_copy_c_string(body.as_ptr()),
            std::ptr::null_mut(),
        )
    }
}

/// Fan-out responder: returns TWO entries (upstream registers both; our
/// bridge fails fast per plan — pinned per-leg).
unsafe extern "C" fn fanout_importer(
    _url: *const c_char,
    _cb: Sass_Importer_Entry,
    _compiler: *mut Sass_Compiler,
) -> Sass_Import_List {
    unsafe {
        let list = sass_make_import_list(2);
        let p1 = CString::new("fan1").unwrap();
        let b1 = CString::new("$fan: one;").unwrap();
        let e1 = sass_make_import_entry(
            p1.as_ptr(),
            sass_copy_c_string(b1.as_ptr()),
            std::ptr::null_mut(),
        );
        let p2 = CString::new("fan2").unwrap();
        let b2 = CString::new("$fan: two;").unwrap();
        let e2 = sass_make_import_entry(
            p2.as_ptr(),
            sass_copy_c_string(b2.as_ptr()),
            std::ptr::null_mut(),
        );
        sass_import_set_list_entry(list, 0, e1);
        sass_import_set_list_entry(list, 1, e2);
        list
    }
}

/// Cookie responder: serves the `f64` behind the cookie as `$ck`.
unsafe extern "C" fn cookie_importer(
    _url: *const c_char,
    cb: Sass_Importer_Entry,
    _compiler: *mut Sass_Compiler,
) -> Sass_Import_List {
    unsafe {
        let cookie = sass_importer_get_cookie(cb) as *const f64;
        let body = format!("$ck: {};", *cookie);
        let path = CString::new("ck").unwrap();
        let body_c = CString::new(body).unwrap();
        single_import(
            path.as_ptr(),
            sass_copy_c_string(body_c.as_ptr()),
            std::ptr::null_mut(),
        )
    }
}

/// Last-import observer: records `abs_path` of `get_last_import` (if any)
/// into the cookie buffer, then falls through (NULL) so the compile proceeds
/// without this importer shadowing anything.
unsafe extern "C" fn last_import_observer(
    _url: *const c_char,
    cb: Sass_Importer_Entry,
    compiler: *mut Sass_Compiler,
) -> Sass_Import_List {
    unsafe {
        let cookie = sass_importer_get_cookie(cb) as *mut LastImportCookie;
        if !cookie.is_null() && !compiler.is_null() {
            let last = sass_compiler_get_last_import(compiler);
            let text = if last.is_null() {
                String::new()
            } else {
                let abs = sass_import_get_abs_path(last);
                if abs.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(abs).to_string_lossy().into_owned()
                }
            };
            (*cookie).seen = true;
            (*cookie).abs_len = text.len();
        }
        std::ptr::null_mut()
    }
}

struct LastImportCookie {
    seen: bool,
    abs_len: usize,
}

/// Chained-import observer: serves `outer` (which nested-imports `inner`)
/// and records the `prev` (`get_last_import` abs) seen for `inner`.
/// `prev` must be the plain path the importer returned — never a `file:`
/// URL (node-sass compares it verbatim; a mismatch there once hung its
/// async bridge, since a throwing importer never calls `done`).
static CHAIN_PREV: Mutex<String> = Mutex::new(String::new());

/// Serves `{path, contents}` for one import (buffers malloc'd — adopted).
unsafe fn serve_import(path: &str, contents: &str) -> Sass_Import_List {
    let list = unsafe { sass_make_import_list(1) };
    assert!(!list.is_null());
    let path_c = CString::new(path).unwrap();
    let body_c = CString::new(contents).unwrap();
    let entry = unsafe {
        sass_make_import_entry(
            path_c.as_ptr(),
            sass_copy_c_string(body_c.as_ptr()),
            std::ptr::null_mut(),
        )
    };
    assert!(!entry.is_null());
    unsafe {
        sass_import_set_list_entry(list, 0, entry);
    }
    list
}

unsafe extern "C" fn chain_importer(
    url: *const c_char,
    _cb: Sass_Importer_Entry,
    compiler: *mut Sass_Compiler,
) -> Sass_Import_List {
    unsafe {
        let url_text = CStr::from_ptr(url).to_bytes().to_vec();
        if url_text == b"outer" {
            return serve_import("/chain/outer.scss", "@import \"inner\";");
        }
        let last = sass_compiler_get_last_import(compiler);
        let prev = if last.is_null() {
            String::new()
        } else {
            let abs = sass_import_get_abs_path(last);
            if abs.is_null() {
                String::new()
            } else {
                CStr::from_ptr(abs).to_string_lossy().into_owned()
            }
        };
        *CHAIN_PREV.lock().unwrap() = prev;
        serve_import("/chain/inner.scss", "a { x: 1; }")
    }
}

#[test]
fn importer_list_entry_lifecycle_contract() {
    unsafe {
        let list = sass_make_importer_list(2);
        assert!(!list.is_null());
        let e1 = sass_make_importer(Some(contents_importer), 1.0, std::ptr::null_mut());
        let e2 = sass_make_importer(Some(null_importer), 2.0, std::ptr::null_mut());
        assert!(!e1.is_null());
        assert!(!e2.is_null());
        assert!(sass_importer_get_function(e1).is_some());
        assert_eq!(sass_importer_get_priority(e1), 1.0);
        assert!(sass_importer_get_cookie(e1).is_null());
        sass_importer_set_list_entry(list, 0, e1);
        sass_importer_set_list_entry(list, 1, e2);
        assert_eq!(sass_importer_get_list_entry(list, 0), e1);
        assert_eq!(sass_importer_get_list_entry(list, 1), e2);
        // Separate paths: single delete vs list delete (never both).
        let e3 = sass_make_importer(Some(null_importer), 0.0, std::ptr::null_mut());
        sass_delete_importer(e3);
        sass_delete_importer_list(list); // frees e1, e2 + array
    }
}

#[test]
fn import_entry_make_error_contract() {
    unsafe {
        // make_import_entry copies paths, adopts malloc'd buffers.
        let path = CString::new("p").unwrap();
        let src = CString::new("a { x: 1; }").unwrap();
        let entry = sass_make_import_entry(
            path.as_ptr(),
            sass_copy_c_string(src.as_ptr()),
            std::ptr::null_mut(),
        );
        assert!(!entry.is_null());
        assert_eq!(
            read_opt(sass_import_get_imp_path(entry)),
            Some(b"p".to_vec())
        );
        assert_eq!(
            read_opt(sass_import_get_source(entry)),
            Some(b"a { x: 1; }".to_vec())
        );
        // take_source detaches (getter goes NULL afterwards).
        let taken = sass_import_take_source(entry);
        assert!(!taken.is_null());
        assert!(sass_import_get_source(entry).is_null());
        sass_free_memory(taken as *mut c_void);
        sass_delete_import(entry);

        // set_error maps falsy 0 → -1 (usize::MAX); message is copied.
        let entry =
            sass_make_import_entry(std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut());
        let msg = CString::new("bad").unwrap();
        sass_import_set_error(entry, msg.as_ptr(), 0, 0);
        assert_eq!(sass_import_get_error_line(entry), usize::MAX);
        assert_eq!(sass_import_get_error_column(entry), usize::MAX);
        assert_eq!(
            read_opt(sass_import_get_error_message(entry)),
            Some(b"bad".to_vec())
        );
        sass_delete_import(entry);
        sass_delete_import_list(std::ptr::null_mut());
        sass_delete_importer_list(std::ptr::null_mut());
    }
}

#[test]
fn contents_import_contract() {
    unsafe {
        let ctx = make_data("@import \"virtual\"; a { x: $virt; }");
        register_one(ctx, Some(contents_importer), 0.0, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("x: red;"), "got: {css}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn error_import_contract() {
    unsafe {
        let ctx = make_data("@import \"anything\"; a { x: 1; }");
        register_one(ctx, Some(error_importer), 0.0, std::ptr::null_mut());
        assert_ne!(sass_compile_data_context(ctx), 0);
        let msg = err_of(ctx);
        assert!(msg.contains("import boom"), "got: {msg}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn fallthrough_null_contract() {
    unsafe {
        // NULL return falls through: unknown import still errors, but the
        // importer itself does not shadow anything; a plain compile passes.
        let ctx = make_data("a { x: 1; }");
        register_one(ctx, Some(null_importer), 0.0, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("x: 1;"), "got: {css}");
        sass_delete_data_context(ctx);

        // Unknown @import with only a fallthrough importer errors on both legs.
        let ctx = make_data("@import \"definitely-missing-xyz\"; a { x: 1; }");
        register_one(ctx, Some(null_importer), 0.0, std::ptr::null_mut());
        assert_ne!(sass_compile_data_context(ctx), 0);
        sass_delete_data_context(ctx);
    }
}

#[test]
fn priority_order_contract() {
    unsafe {
        // Higher priority wins regardless of registration order (D16:
        // descending sort mirrors upstream `sort_importers`).
        let ctx = make_data("@import \"prio\"; a { x: $prio; }");
        let opts = sass_data_context_get_options(ctx);
        let list = sass_make_importer_list(2);
        let low = sass_make_importer(Some(prio_low_importer), 1.0, std::ptr::null_mut());
        let high = sass_make_importer(Some(prio_high_importer), 10.0, std::ptr::null_mut());
        // Register low first, high second — sort must still pick high.
        sass_importer_set_list_entry(list, 0, low);
        sass_importer_set_list_entry(list, 1, high);
        sass_option_set_c_importers(opts, list);
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("x: high;"), "got: {css}");
        sass_delete_data_context(ctx);
    }
}

#[test]
fn cookie_and_last_import_contract() {
    unsafe {
        static COOKIE_VAL: f64 = 9.0;
        let ctx = make_data("@import \"ck\"; a { x: $ck; }");
        register_one(
            ctx,
            Some(cookie_importer),
            0.0,
            &COOKIE_VAL as *const f64 as *mut c_void,
        );
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("x: 9;"), "got: {css}");
        sass_delete_data_context(ctx);

        // get_last_import is callable during the callback on both legs
        // (must not crash; upstream returns the import stack top).
        let mut cookie = LastImportCookie {
            seen: false,
            abs_len: usize::MAX,
        };
        let ctx = make_data("a { x: 1; }");
        register_one(
            ctx,
            Some(last_import_observer),
            0.0,
            &mut cookie as *mut LastImportCookie as *mut c_void,
        );
        // No @import means the observer never fires; compile passes and the
        // cookie stays untouched (proves registration alone is harmless).
        assert_eq!(sass_compile_data_context(ctx), 0);
        assert!(!cookie.seen);
        sass_delete_data_context(ctx);
    }
}

#[test]
fn nested_prev_path_contract() {
    // `prev` for a nested import is the plain path the outer import
    // returned — identical on both legs (upstream passes `abs_path`
    // through verbatim; our bridge converts back from the canonical
    // `file:` URL).
    unsafe {
        *CHAIN_PREV.lock().unwrap() = String::new();
        let ctx = make_data("@import \"outer\";");
        register_one(ctx, Some(chain_importer), 0.0, std::ptr::null_mut());
        assert_eq!(sass_compile_data_context(ctx), 0);
        let css = css_of(ctx);
        assert!(css.contains("x: 1;"), "got: {css}");
        assert_eq!(CHAIN_PREV.lock().unwrap().as_str(), "/chain/outer.scss");
        sass_delete_data_context(ctx);
    }
}

/// Writes `name` with `contents` under a fresh tempdir unique to `tag`
/// (tests run in parallel threads of one process), returning the dir path
/// (caller removes it).
fn write_temp_fixture(tag: &str, name: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rust-sass-libsass-dispatch-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), contents).unwrap();
    dir
}

static DISPATCH_FIRED: Mutex<bool> = Mutex::new(false);

/// Serves `a { x: dispatched; }` for any import (winning over disk).
unsafe extern "C" fn dispatch_winner(
    _url: *const c_char,
    _cb: Sass_Importer_Entry,
    _compiler: *mut Sass_Compiler,
) -> Sass_Import_List {
    unsafe {
        *DISPATCH_FIRED.lock().unwrap() = true;
        serve_import("winner", "a { x: dispatched; }")
    }
}

unsafe fn css_of_file(ctx: *mut Sass_File_Context) -> String {
    let base = unsafe { sass_file_context_get_context(ctx) };
    String::from_utf8(unsafe { read_opt(sass_context_get_output_string(base)) }.unwrap()).unwrap()
}

#[test]
fn file_entry_customs_first_contract() {
    // File entry with a filesystem-resolvable relative: the C importer
    // fires FIRST and its contents win over disk (upstream `call_loader`
    // order). Identical on both legs.
    unsafe {
        *DISPATCH_FIRED.lock().unwrap() = false;
        let dir = write_temp_fixture("customs-first", "disk.scss", "a { x: disk; }");
        std::fs::write(dir.join("main.scss"), "@import \"disk\";").unwrap();
        let main = CString::new(dir.join("main.scss").to_string_lossy().into_owned()).unwrap();
        let ctx = sass_make_file_context(main.as_ptr());
        assert!(!ctx.is_null());
        register_one_file(ctx, Some(dispatch_winner), 0.0, std::ptr::null_mut());
        assert_eq!(sass_compile_file_context(ctx), 0);
        let css = css_of_file(ctx);
        assert!(*DISPATCH_FIRED.lock().unwrap());
        assert!(css.contains("dispatched"), "got: {css}");
        sass_delete_file_context(ctx);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn file_entry_null_falls_back_contract() {
    // Same shape, but the importer returns NULL: the filesystem resolves
    // `disk.scss` from disk on both legs.
    unsafe {
        let dir = write_temp_fixture("null-fallback", "disk.scss", "a { x: disk; }");
        std::fs::write(dir.join("main.scss"), "@import \"disk\";").unwrap();
        let main = CString::new(dir.join("main.scss").to_string_lossy().into_owned()).unwrap();
        let ctx = sass_make_file_context(main.as_ptr());
        assert!(!ctx.is_null());
        register_one_file(ctx, Some(null_importer), 0.0, std::ptr::null_mut());
        assert_eq!(sass_compile_file_context(ctx), 0);
        let css = css_of_file(ctx);
        assert!(css.contains("disk"), "got: {css}");
        sass_delete_file_context(ctx);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Registers ONE C importer on a file context's options (list owned by the
/// context afterwards — never delete it).
unsafe fn register_one_file(
    ctx: *mut Sass_File_Context,
    cb: Sass_Importer_Fn,
    priority: f64,
    cookie: *mut c_void,
) {
    let opts = unsafe { sass_file_context_get_options(ctx) };
    let list = unsafe { sass_make_importer_list(1) };
    assert!(!list.is_null());
    let entry = unsafe { sass_make_importer(cb, priority, cookie) };
    assert!(!entry.is_null());
    unsafe {
        sass_importer_set_list_entry(list, 0, entry);
        sass_option_set_c_importers(opts, list);
    }
}

#[test]
fn fanout_contract() {
    // A single callback returning N>1 entries diverges by engine: upstream
    // registers every entry (fan-out); our bridge fails fast with a clear
    // message (plan decision: silent first-only drop would hide imports).
    unsafe {
        let ctx = make_data("@import \"fan\"; a { x: 1; }");
        register_one(ctx, Some(fanout_importer), 0.0, std::ptr::null_mut());
        #[cfg(feature = "upstream")]
        {
            assert_eq!(sass_compile_data_context(ctx), 0);
        }
        #[cfg(not(feature = "upstream"))]
        {
            assert_ne!(sass_compile_data_context(ctx), 0);
            let msg = err_of(ctx);
            assert!(msg.contains("single-import"), "got: {msg}");
        }
        sass_delete_data_context(ctx);
    }
}
