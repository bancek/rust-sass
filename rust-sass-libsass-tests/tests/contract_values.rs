// dart-source: N/A (C-ABI contract tests — no Dart counterpart; see
//   docs/plans/libsass.md §8/D11 and §13 TDD policy).
//
// Values contract: make/get/set/clone/delete/tag/predicates for all 9 value
// kinds. These assertions run IDENTICALLY against our library (default
// features) and upstream libsass (`--no-default-features --features
// upstream`) — they pin the ABI contract, never implementation output.
//
// Written FIRST (TDD): validated green on the upstream leg before the adapter
// implemented them. Ownership rules pinned here (verified against upstream +
// node-sass/src/sass_types usage):
// - makers copy input strings (the caller's buffer stays owned by the caller;
//   node-sass `delete`s its temporaries right after `make_*`);
// - `set_unit`/`set_value`/`set_message` TAKE ownership (store raw, no copy;
//   node-sass passes fresh `create_string` buffers with no free);
// - `list/map_set_*` TAKE ownership of the entry (node-sass passes fresh
//   values into empty slots); getters BORROW (no transfer).
// - `clone` is deep; `delete` is recursive and NULL-safe.
// - `make_number(v, NULL)` and `make_string/qstring/error/warning(NULL)`
//   return NULL (unitless is `""`, not NULL).
// Deliberate adapter hardenings (in-range behavior identical; unit-tested in
// the adapter crate, not here since upstream unchecked-derefs): out-of-range
// list/map indexed getters return NULL; setters free the previously owned
// value before storing (upstream overwrites and leaks).

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

/// Makes an owned C string for transfer into a setter (caller = the value).
fn owned(s: &str) -> *mut c_char {
    // SAFETY: test-only; fresh malloc'd buffer, ownership transfers to the
    // value under test (freed by its delete).
    unsafe { sass_copy_c_string(CString::new(s).unwrap().as_ptr()) }
}

unsafe fn tag_of(v: *const Sass_Value) -> Sass_Tag {
    // SAFETY: test-only; `v` is a live value for the call.
    unsafe { sass_value_get_tag(v) }
}

#[test]
fn tags_and_predicates_contract() {
    unsafe {
        let px = CString::new("px").unwrap();
        let s = CString::new("hi").unwrap();
        let msg = CString::new("boom").unwrap();
        let cases: Vec<(*mut Sass_Value, Sass_Tag)> = vec![
            (sass_make_null(), Sass_Tag_SASS_NULL),
            (sass_make_boolean(true), Sass_Tag_SASS_BOOLEAN),
            (sass_make_number(1.5, px.as_ptr()), Sass_Tag_SASS_NUMBER),
            (sass_make_color(1.0, 2.0, 3.0, 0.5), Sass_Tag_SASS_COLOR),
            (sass_make_string(s.as_ptr()), Sass_Tag_SASS_STRING),
            (
                sass_make_list(0, Sass_Separator_SASS_COMMA, false),
                Sass_Tag_SASS_LIST,
            ),
            (sass_make_map(0), Sass_Tag_SASS_MAP),
            (sass_make_error(msg.as_ptr()), Sass_Tag_SASS_ERROR),
            (sass_make_warning(msg.as_ptr()), Sass_Tag_SASS_WARNING),
        ];
        assert_eq!(cases.len(), 9);
        for (v, tag) in &cases {
            assert!(!v.is_null());
            assert_eq!(tag_of(*v), *tag);
        }
        // Predicate matrix: each value is its own kind and no other.
        let preds: Vec<(Sass_Tag, unsafe extern "C" fn(*const Sass_Value) -> bool)> = vec![
            (Sass_Tag_SASS_NULL, sass_value_is_null),
            (Sass_Tag_SASS_NUMBER, sass_value_is_number),
            (Sass_Tag_SASS_STRING, sass_value_is_string),
            (Sass_Tag_SASS_BOOLEAN, sass_value_is_boolean),
            (Sass_Tag_SASS_COLOR, sass_value_is_color),
            (Sass_Tag_SASS_LIST, sass_value_is_list),
            (Sass_Tag_SASS_MAP, sass_value_is_map),
            (Sass_Tag_SASS_ERROR, sass_value_is_error),
            (Sass_Tag_SASS_WARNING, sass_value_is_warning),
        ];
        for (v, tag) in &cases {
            for (p_tag, pred) in &preds {
                assert_eq!(pred(*v), p_tag == tag, "tag {tag}");
            }
        }
        for (v, _) in cases {
            sass_delete_value(v);
        }
    }
}

#[test]
fn boolean_null_contract() {
    unsafe {
        let t = sass_make_boolean(true);
        let f = sass_make_boolean(false);
        assert!(sass_boolean_get_value(t));
        assert!(!sass_boolean_get_value(f));
        sass_boolean_set_value(f, true);
        assert!(sass_boolean_get_value(f));
        sass_boolean_set_value(t, false);
        assert!(!sass_boolean_get_value(t));
        let n = sass_make_null();
        assert_eq!(tag_of(n), Sass_Tag_SASS_NULL);
        sass_delete_value(t);
        sass_delete_value(f);
        sass_delete_value(n);
        // NULL delete is a no-op.
        sass_delete_value(std::ptr::null_mut());
    }
}

#[test]
fn number_contract() {
    unsafe {
        let px = CString::new("px").unwrap();
        let n = sass_make_number(1.5, px.as_ptr());
        assert_eq!(sass_number_get_value(n), 1.5);
        assert_eq!(read_opt(sass_number_get_unit(n)), Some(b"px".to_vec()));
        // Maker copies the unit: mutating the caller's buffer is invisible.
        let mut unit_buf = b"em".to_vec();
        unit_buf.push(0);
        let m = sass_make_number(2.0, unit_buf.as_ptr() as *const c_char);
        unit_buf[0] = b'X';
        assert_eq!(read_opt(sass_number_get_unit(m)), Some(b"em".to_vec()));
        // Unitless is empty string, not NULL.
        let empty = CString::new("").unwrap();
        let u = sass_make_number(3.0, empty.as_ptr());
        assert_eq!(read_opt(sass_number_get_unit(u)), Some(b"".to_vec()));
        sass_number_set_value(n, -0.25);
        assert_eq!(sass_number_get_value(n), -0.25);
        // set_unit takes ownership (fresh malloc'd buffer, never freed here).
        sass_number_set_unit(n, owned("%"));
        assert_eq!(read_opt(sass_number_get_unit(n)), Some(b"%".to_vec()));
        // NULL unit fails the make (unitless is "").
        assert!(sass_make_number(1.0, std::ptr::null()).is_null());
        sass_delete_value(n);
        sass_delete_value(m);
        sass_delete_value(u);
    }
}

#[test]
fn string_contract() {
    unsafe {
        let s = CString::new("hi").unwrap();
        let plain = sass_make_string(s.as_ptr());
        assert!(!sass_string_is_quoted(plain));
        assert_eq!(read_opt(sass_string_get_value(plain)), Some(b"hi".to_vec()));
        let quoted = sass_make_qstring(s.as_ptr());
        assert!(sass_string_is_quoted(quoted));
        assert_eq!(
            read_opt(sass_string_get_value(quoted)),
            Some(b"hi".to_vec())
        );
        // Maker copies: caller's buffer mutation is invisible.
        let mut buf = b"ab".to_vec();
        buf.push(0);
        let c = sass_make_string(buf.as_ptr() as *const c_char);
        buf[0] = b'Z';
        assert_eq!(read_opt(sass_string_get_value(c)), Some(b"ab".to_vec()));
        sass_string_set_quoted(plain, true);
        assert!(sass_string_is_quoted(plain));
        // set_value takes ownership.
        sass_string_set_value(plain, owned("bye"));
        assert_eq!(
            read_opt(sass_string_get_value(plain)),
            Some(b"bye".to_vec())
        );
        assert!(sass_make_string(std::ptr::null()).is_null());
        assert!(sass_make_qstring(std::ptr::null()).is_null());
        sass_delete_value(plain);
        sass_delete_value(quoted);
        sass_delete_value(c);
    }
}

#[test]
fn color_contract() {
    unsafe {
        let c = sass_make_color(10.0, 20.0, 30.0, 0.5);
        assert_eq!(sass_color_get_r(c), 10.0);
        assert_eq!(sass_color_get_g(c), 20.0);
        assert_eq!(sass_color_get_b(c), 30.0);
        assert_eq!(sass_color_get_a(c), 0.5);
        sass_color_set_r(c, 1.0);
        sass_color_set_g(c, 2.0);
        sass_color_set_b(c, 3.0);
        sass_color_set_a(c, 1.0);
        assert_eq!(sass_color_get_r(c), 1.0);
        assert_eq!(sass_color_get_g(c), 2.0);
        assert_eq!(sass_color_get_b(c), 3.0);
        assert_eq!(sass_color_get_a(c), 1.0);
        sass_delete_value(c);
    }
}

#[test]
fn list_contract() {
    unsafe {
        let list = sass_make_list(2, Sass_Separator_SASS_SPACE, true);
        assert_eq!(sass_list_get_length(list), 2);
        assert_eq!(sass_list_get_separator(list), Sass_Separator_SASS_SPACE);
        assert!(sass_list_get_is_bracketed(list));
        // Entries transfer in (fresh values, never touched again here).
        sass_list_set_value(list, 0, sass_make_boolean(true));
        let px = CString::new("px").unwrap();
        sass_list_set_value(list, 1, sass_make_number(4.0, px.as_ptr()));
        let first = sass_list_get_value(list, 0);
        let second = sass_list_get_value(list, 1);
        assert!(sass_value_is_boolean(first));
        assert!(sass_boolean_get_value(first));
        assert_eq!(sass_number_get_value(second), 4.0);
        // Getters borrow: no delete of entries (whole list deletes once).
        sass_list_set_separator(list, Sass_Separator_SASS_COMMA);
        assert_eq!(sass_list_get_separator(list), Sass_Separator_SASS_COMMA);
        sass_list_set_is_bracketed(list, false);
        assert!(!sass_list_get_is_bracketed(list));
        // Empty lists carry their separator.
        let empty = sass_make_list(0, Sass_Separator_SASS_COMMA, false);
        assert!(!empty.is_null());
        assert_eq!(sass_list_get_length(empty), 0);
        sass_delete_value(list);
        sass_delete_value(empty);
    }
}

#[test]
fn map_contract() {
    unsafe {
        let map = sass_make_map(2);
        assert_eq!(sass_map_get_length(map), 2);
        let k1 = CString::new("a").unwrap();
        let k2 = CString::new("b").unwrap();
        sass_map_set_key(map, 0, sass_make_string(k1.as_ptr()));
        sass_map_set_value(map, 0, sass_make_boolean(false));
        sass_map_set_key(map, 1, sass_make_string(k2.as_ptr()));
        let px = CString::new("px").unwrap();
        sass_map_set_value(map, 1, sass_make_number(9.0, px.as_ptr()));
        let got_k = sass_map_get_key(map, 0);
        let got_v = sass_map_get_value(map, 1);
        assert_eq!(read_opt(sass_string_get_value(got_k)), Some(b"a".to_vec()));
        assert_eq!(sass_number_get_value(got_v), 9.0);
        let empty = sass_make_map(0);
        assert!(!empty.is_null());
        assert_eq!(sass_map_get_length(empty), 0);
        sass_delete_value(map);
        sass_delete_value(empty);
    }
}

#[test]
fn error_warning_contract() {
    unsafe {
        let msg = CString::new("boom").unwrap();
        let e = sass_make_error(msg.as_ptr());
        assert_eq!(read_opt(sass_error_get_message(e)), Some(b"boom".to_vec()));
        // set_message takes ownership.
        sass_error_set_message(e, owned("bam"));
        assert_eq!(read_opt(sass_error_get_message(e)), Some(b"bam".to_vec()));
        let w = sass_make_warning(msg.as_ptr());
        assert_eq!(
            read_opt(sass_warning_get_message(w)),
            Some(b"boom".to_vec())
        );
        sass_warning_set_message(w, owned("wam"));
        assert_eq!(read_opt(sass_warning_get_message(w)), Some(b"wam".to_vec()));
        assert!(sass_make_error(std::ptr::null()).is_null());
        assert!(sass_make_warning(std::ptr::null()).is_null());
        sass_delete_value(e);
        sass_delete_value(w);
    }
}

#[test]
fn clone_contract() {
    unsafe {
        // Nested list: clone is deep (mutating the original leaves it).
        let list = sass_make_list(1, Sass_Separator_SASS_COMMA, false);
        let s = CString::new("x").unwrap();
        sass_list_set_value(list, 0, sass_make_string(s.as_ptr()));
        let dup = sass_clone_value(list);
        assert_eq!(tag_of(dup), Sass_Tag_SASS_LIST);
        assert_eq!(sass_list_get_length(dup), 1);
        let child = sass_list_get_value(list, 0);
        sass_string_set_value(child, owned("mutated"));
        let dup_child = sass_list_get_value(dup, 0);
        assert_eq!(
            read_opt(sass_string_get_value(dup_child)),
            Some(b"x".to_vec())
        );
        // Quoted flag survives the clone.
        let q = CString::new("q").unwrap();
        let qs = sass_make_qstring(q.as_ptr());
        let qs_dup = sass_clone_value(qs);
        assert!(sass_string_is_quoted(qs_dup));
        // Map clone carries pairs.
        let map = sass_make_map(1);
        let k = CString::new("k").unwrap();
        sass_map_set_key(map, 0, sass_make_string(k.as_ptr()));
        sass_map_set_value(map, 0, sass_make_boolean(true));
        let map_dup = sass_clone_value(map);
        assert_eq!(sass_map_get_length(map_dup), 1);
        assert!(sass_boolean_get_value(sass_map_get_value(map_dup, 0)));
        // Scalar clones.
        let px = CString::new("px").unwrap();
        let n_dup = sass_clone_value(sass_make_number(5.0, px.as_ptr()));
        assert_eq!(sass_number_get_value(n_dup), 5.0);
        // NULL clone is NULL.
        assert!(sass_clone_value(std::ptr::null()).is_null());
        sass_delete_value(list);
        sass_delete_value(dup);
        sass_delete_value(qs);
        sass_delete_value(qs_dup);
        sass_delete_value(map);
        sass_delete_value(map_dup);
        sass_delete_value(n_dup);
    }
}

#[test]
fn op_stringify_contract() {
    // `sass_value_op` / `sass_value_stringify` need the eval engine (a later
    // increment wires them to real semantics). The cross-leg contract pins
    // only NULL-safety: both legs return non-null values, never NULL.
    // (Upstream computes real results; ours returns `sass_make_error`
    // values — the libsass failure idiom `value_op` itself uses.)
    unsafe {
        let px = CString::new("px").unwrap();
        let a = sass_make_number(1.0, px.as_ptr());
        let b = sass_make_number(1.0, px.as_ptr());
        let eq = sass_value_op(Sass_OP_EQ, a, b);
        assert!(!eq.is_null());
        sass_delete_value(eq);
        let s = sass_make_string(CString::new("x").unwrap().as_ptr());
        let strung = sass_value_stringify(s, false, 5);
        assert!(!strung.is_null());
        // Upstream computes a real (quoted) string; our stub returns the
        // error value by design — pinned per-leg, not across legs.
        #[cfg(not(feature = "upstream"))]
        assert_eq!(tag_of(strung), Sass_Tag_SASS_ERROR);
        #[cfg(feature = "upstream")]
        assert_eq!(tag_of(strung), Sass_Tag_SASS_STRING);
        sass_delete_value(strung);
        sass_delete_value(a);
        sass_delete_value(b);
        sass_delete_value(s);
    }
}
