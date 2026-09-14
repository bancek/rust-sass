// dart-source: N/A (C-ABI contract tests — no Dart counterpart; see
//   docs/plans/libsass.md §8/D11 and §13 TDD policy).
//
// Options contract: every setter/getter pair round-trips, path lists behave,
// NULL inputs are safe, defaults match. These assertions run IDENTICALLY
// against our library (default features) and upstream libsass
// (`--no-default-features --features upstream`) — they pin the ABI contract,
// never implementation output.
//
// Written FIRST (TDD): validated green on the upstream leg before the adapter
// implemented them. Where upstream exhibits behavior we deliberately won't
// mirror, the decision is recorded inline (see the index-getter test).

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

/// Sets a `*const c_char` string option from `value` (None = NULL).
unsafe fn set_str(
    set: unsafe extern "C" fn(*mut Sass_Options, *const c_char),
    opts: *mut Sass_Options,
    value: Option<&str>,
) {
    match value {
        Some(v) => {
            let owned = CString::new(v).unwrap();
            // SAFETY: test-only; setter copies the string (plan G1).
            unsafe { set(opts, owned.as_ptr()) };
        }
        None => unsafe { set(opts, std::ptr::null()) },
    }
}

unsafe fn make_options() -> *mut Sass_Options {
    // SAFETY: test-only; fresh heap options, deleted at the end of each test.
    unsafe { sass_make_options() }
}

unsafe fn delete_options(opts: *mut Sass_Options) {
    // SAFETY: test-only; `opts` is a live heap allocation, freed once here.
    unsafe { sass_delete_options(opts) };
}

#[test]
fn defaults_contract() {
    unsafe {
        let opts = make_options();
        assert_eq!(sass_option_get_precision(opts), 10);
        assert_eq!(
            sass_option_get_output_style(opts),
            Sass_Output_Style_SASS_STYLE_NESTED
        );
        assert!(!sass_option_get_source_comments(opts));
        assert!(!sass_option_get_source_map_embed(opts));
        assert!(!sass_option_get_source_map_contents(opts));
        assert!(!sass_option_get_source_map_file_urls(opts));
        assert!(!sass_option_get_omit_source_map_url(opts));
        assert!(!sass_option_get_is_indented_syntax_src(opts));
        assert_eq!(read_opt(sass_option_get_indent(opts)), Some(b"  ".to_vec()));
        assert_eq!(
            read_opt(sass_option_get_linefeed(opts)),
            Some(b"\n".to_vec())
        );
        assert_eq!(read_opt(sass_option_get_input_path(opts)), None);
        assert_eq!(read_opt(sass_option_get_output_path(opts)), None);
        assert_eq!(read_opt(sass_option_get_source_map_file(opts)), None);
        assert_eq!(read_opt(sass_option_get_source_map_root(opts)), None);
        assert_eq!(sass_option_get_include_path_size(opts), 0);
        assert_eq!(sass_option_get_plugin_path_size(opts), 0);
        delete_options(opts);
    }
}

#[test]
fn scalar_roundtrip_contract() {
    unsafe {
        let opts = make_options();
        sass_option_set_precision(opts, 5);
        assert_eq!(sass_option_get_precision(opts), 5);
        sass_option_set_precision(opts, 0);
        assert_eq!(sass_option_get_precision(opts), 0);
        for style in [
            Sass_Output_Style_SASS_STYLE_NESTED,
            Sass_Output_Style_SASS_STYLE_EXPANDED,
            Sass_Output_Style_SASS_STYLE_COMPACT,
            Sass_Output_Style_SASS_STYLE_COMPRESSED,
        ] {
            sass_option_set_output_style(opts, style);
            assert_eq!(sass_option_get_output_style(opts), style);
        }
        for (set, get) in [
            (
                sass_option_set_source_comments as unsafe extern "C" fn(*mut Sass_Options, bool),
                sass_option_get_source_comments as unsafe extern "C" fn(*mut Sass_Options) -> bool,
            ),
            (
                sass_option_set_source_map_embed,
                sass_option_get_source_map_embed,
            ),
            (
                sass_option_set_source_map_contents,
                sass_option_get_source_map_contents,
            ),
            (
                sass_option_set_source_map_file_urls,
                sass_option_get_source_map_file_urls,
            ),
            (
                sass_option_set_omit_source_map_url,
                sass_option_get_omit_source_map_url,
            ),
            (
                sass_option_set_is_indented_syntax_src,
                sass_option_get_is_indented_syntax_src,
            ),
        ] {
            set(opts, true);
            assert!(get(opts));
            set(opts, false);
            assert!(!get(opts));
        }
        delete_options(opts);
    }
}

#[test]
fn string_roundtrip_contract() {
    unsafe {
        let opts = make_options();
        // indent/linefeed/input/output/source_map_file/root round-trip values.
        // NOTE: indent/linefeed setters BORROW (plan G1 — plain assignment,
        // `IMPLEMENT_SASS_OPTION_ACCESSOR` at sass_context.cpp:640-641), so
        // the getter reflects the caller's buffer, which must outlive the
        // read. All other string setters COPY (`_STRING_SETTER`, :642-647).
        let indent = CString::new("\t\t").unwrap();
        sass_option_set_indent(opts, indent.as_ptr());
        assert_eq!(
            read_opt(sass_option_get_indent(opts)),
            Some(b"\t\t".to_vec())
        );
        let linefeed = CString::new("\r\n").unwrap();
        sass_option_set_linefeed(opts, linefeed.as_ptr());
        assert_eq!(
            read_opt(sass_option_get_linefeed(opts)),
            Some(b"\r\n".to_vec())
        );
        set_str(sass_option_set_input_path, opts, Some("in.scss"));
        assert_eq!(
            read_opt(sass_option_get_input_path(opts)),
            Some(b"in.scss".to_vec())
        );
        set_str(sass_option_set_output_path, opts, Some("out.css"));
        assert_eq!(
            read_opt(sass_option_get_output_path(opts)),
            Some(b"out.css".to_vec())
        );
        set_str(sass_option_set_source_map_file, opts, Some("out.css.map"));
        assert_eq!(
            read_opt(sass_option_get_source_map_file(opts)),
            Some(b"out.css.map".to_vec())
        );
        set_str(sass_option_set_source_map_root, opts, Some("http://x/"));
        assert_eq!(
            read_opt(sass_option_get_source_map_root(opts)),
            Some(b"http://x/".to_vec())
        );
        // Overwrite replaces (no leak observable here, but ASan/valgrind on
        // the C side would catch it; both legs must at least return latest).
        set_str(sass_option_set_input_path, opts, Some("second.scss"));
        assert_eq!(
            read_opt(sass_option_get_input_path(opts)),
            Some(b"second.scss".to_vec())
        );
        delete_options(opts);
    }
}

#[test]
fn string_null_contract() {
    unsafe {
        let opts = make_options();
        // NULL clears to NULL (upstream: free + store 0). sassc passes NULL
        // straight from getenv("SASS_PATH"), so this is load-bearing.
        set_str(sass_option_set_input_path, opts, Some("x.scss"));
        set_str(sass_option_set_input_path, opts, None);
        assert_eq!(read_opt(sass_option_get_input_path(opts)), None);
        set_str(sass_option_set_include_path, opts, None);
        assert_eq!(sass_option_get_include_path_size(opts), 0);
        set_str(sass_option_set_plugin_path, opts, None);
        assert_eq!(sass_option_get_plugin_path_size(opts), 0);
        set_str(sass_option_set_indent, opts, None);
        // NOTE (deliberate divergence surface, not asserted): upstream's
        // indent/linefeed getters are raw pointers, so setting NULL makes the
        // getter return NULL; our adapter hardens NULL to the default instead.
        // The contract pins only that the getter is non-crashing and the
        // compile path behaves. See plan G1.
        let _ = sass_option_get_indent(opts);
        delete_options(opts);
    }
}

#[test]
fn path_list_contract() {
    unsafe {
        let opts = make_options();
        let a = CString::new("/a").unwrap();
        let b = CString::new("/b").unwrap();
        sass_option_push_include_path(opts, a.as_ptr());
        sass_option_push_include_path(opts, b.as_ptr());
        assert_eq!(sass_option_get_include_path_size(opts), 2);
        assert_eq!(
            read_opt(sass_option_get_include_path(opts, 0)),
            Some(b"/a".to_vec())
        );
        assert_eq!(
            read_opt(sass_option_get_include_path(opts, 1)),
            Some(b"/b".to_vec())
        );
        // Pushed strings are copied: mutating the caller's buffer afterwards
        // must not affect the stored value.
        let mut owned = b"/c".to_vec();
        owned.push(0);
        sass_option_push_include_path(opts, owned.as_ptr() as *const c_char);
        owned[0] = b'X';
        assert_eq!(
            read_opt(sass_option_get_include_path(opts, 2)),
            Some(b"/c".to_vec())
        );
        assert_eq!(sass_option_get_include_path_size(opts), 3);
        // Plugin list is independent and behaves the same.
        let p = CString::new("/p").unwrap();
        sass_option_push_plugin_path(opts, p.as_ptr());
        assert_eq!(sass_option_get_plugin_path_size(opts), 1);
        assert_eq!(
            read_opt(sass_option_get_plugin_path(opts, 0)),
            Some(b"/p".to_vec())
        );
        assert_eq!(sass_option_get_include_path_size(opts), 3);
        delete_options(opts);
    }
}

#[test]
fn index_getter_safety_contract() {
    // Upstream's indexed getters walk unchecked (`cur->next` / `cur->string`
    // without bounds checks — plan §A.8): out-of-range reads are UB there.
    // Our adapter hardens them (returns NULL). The contract pins ONLY the
    // in-range behavior identically on both legs; the out-of-range case runs
    // on our leg alone.
    unsafe {
        let opts = make_options();
        let a = CString::new("/a").unwrap();
        sass_option_push_include_path(opts, a.as_ptr());
        assert_eq!(
            read_opt(sass_option_get_include_path(opts, 0)),
            Some(b"/a".to_vec())
        );
        #[cfg(not(feature = "upstream"))]
        {
            assert_eq!(read_opt(sass_option_get_include_path(opts, 7)), None);
            assert_eq!(read_opt(sass_option_get_plugin_path(opts, 0)), None);
        }
        delete_options(opts);
    }
}

#[test]
fn make_delete_options_contract() {
    unsafe {
        // Fresh options are usable and deletable; double-lifecycle works.
        for _ in 0..2 {
            let opts = make_options();
            sass_option_set_precision(opts, 3);
            assert_eq!(sass_option_get_precision(opts), 3);
            delete_options(opts);
        }
    }
}
