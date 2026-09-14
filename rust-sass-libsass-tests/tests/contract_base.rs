// dart-source: N/A (C-ABI contract tests — no Dart counterpart; see
//   docs/plans/libsass.md §8/D11).
//
// Base contract: allocation, string helpers, versions. These assertions run
// IDENTICALLY against our library (default features) and upstream libsass
// (`--no-default-features --features upstream`) — they pin the ABI contract,
// never implementation output.

use rust_sass_libsass_tests::bindings::*;
use std::ffi::{c_char, c_void, CStr, CString};

/// Reads a borrowed NUL-terminated C string into bytes.
unsafe fn read_borrowed(ptr: *const c_char) -> Vec<u8> {
    assert!(!ptr.is_null());
    // SAFETY: test-only; every pointer here is a valid string produced by the
    // function under test, borrowed (not freed) for the read.
    unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec()
}

/// Reads a malloc'd C string into bytes and frees it.
unsafe fn read_owned(ptr: *mut c_char) -> Vec<u8> {
    let out = unsafe { read_borrowed(ptr) };
    // SAFETY: test-only; `ptr` is a fresh allocation from the function under
    // test, freed exactly once here.
    unsafe {
        sass_free_memory(ptr as *mut c_void);
    }
    out
}

#[test]
fn alloc_free_contract() {
    unsafe {
        let got = sass_alloc_memory(64);
        assert!(!got.is_null());
        (got as *mut u8).write_bytes(0xab, 64);
        assert_eq!((got as *mut u8).read(), 0xab);
        sass_free_memory(got);
        // NULL free is a no-op.
        sass_free_memory(std::ptr::null_mut());
        // Zero-size allocation yields a freeable pointer on macOS/Linux.
        let zero = sass_alloc_memory(0);
        assert!(!zero.is_null());
        sass_free_memory(zero);
    }
}

#[test]
fn copy_c_string_contract() {
    unsafe {
        assert!(sass_copy_c_string(std::ptr::null()).is_null());
        let src = CString::new("hello").unwrap();
        let got = sass_copy_c_string(src.as_ptr());
        assert!(!got.is_null());
        assert_ne!(got, src.as_ptr() as *mut c_char);
        assert_eq!(read_owned(got), b"hello");
        let empty = CString::new("").unwrap();
        assert_eq!(read_owned(sass_copy_c_string(empty.as_ptr())), b"");
    }
}

#[test]
fn quote_contract() {
    unsafe fn quote(s: &str, mark: c_char) -> Vec<u8> {
        let input = CString::new(s).unwrap();
        // SAFETY: test-only; valid input, output freed in `read_owned`.
        read_owned(unsafe { sass_string_quote(input.as_ptr(), mark) })
    }
    unsafe {
        assert_eq!(quote("foo", b'"' as c_char), b"\"foo\"");
        assert_eq!(quote("", b'"' as c_char), b"\"\"");
        assert_eq!(quote("a\"b", b'"' as c_char), b"'a\"b'");
        assert_eq!(quote("a'b", b'"' as c_char), b"\"a'b\"");
        assert_eq!(quote("a'b\"c", 0), b"\"a'b\\\"c\"");
        assert_eq!(quote("x", 0), b"\"x\"");
        assert_eq!(quote("a\\b", b'"' as c_char), b"\"a\\\\b\"");
        assert_eq!(quote("a\nb", b'"' as c_char), b"\"a\\a b\"");
        assert_eq!(quote("a\nzb", b'"' as c_char), b"\"a\\azb\"");
        assert_eq!(quote("a\n b", b'"' as c_char), b"\"a\\a  b\"");
    }
}

#[test]
fn unquote_contract() {
    unsafe fn unquote(s: &str) -> Vec<u8> {
        let input = CString::new(s).unwrap();
        // SAFETY: test-only; valid input, output freed in `read_owned`.
        read_owned(unsafe { sass_string_unquote(input.as_ptr()) })
    }
    unsafe {
        assert_eq!(unquote("\"foo\""), b"foo");
        assert_eq!(unquote("'foo'"), b"foo");
        assert_eq!(unquote("\"\""), b"");
        assert_eq!(unquote("a"), b"a");
        assert_eq!(unquote("ab"), b"ab");
        assert_eq!(unquote("\"ab'"), b"\"ab'");
        assert_eq!(unquote("\"\\41\""), b"A");
        assert_eq!(unquote("\"\\41 B\""), b"AB");
        assert_eq!(unquote("\"a\\nb\""), b"anb");
        assert_eq!(unquote("\"a\\\\b\""), b"a\\b");
        assert_eq!(unquote("\"a\"b\""), b"\"a\"b\"");
        assert_eq!(unquote("\"ab\\"), b"\"ab\\");
    }
}

#[test]
fn version_contract() {
    unsafe {
        // Both legs: non-null, NUL-terminated, non-empty. Upstream stamps its
        // build id (`3.6.6-2-g…`), so exact text is pinned on our leg only.
        assert!(!read_borrowed(libsass_version()).is_empty());
        assert!(!read_borrowed(libsass_language_version()).is_empty());
        #[cfg(not(feature = "upstream"))]
        {
            assert_eq!(read_borrowed(libsass_version()), b"3.6.6");
            assert_eq!(read_borrowed(libsass_language_version()), b"3.5");
        }
    }
}
