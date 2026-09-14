// dart-source: N/A (C-ABI contract tests — no Dart counterpart; see
//   docs/plans/libsass.md §8/D11).
//
// sass2scss contract: the indented-Sass→SCSS converter. These assertions run
// IDENTICALLY against our library (default features) and upstream libsass
// (`--no-default-features --features upstream`). Unlike the other contract
// suites (plan D11: never byte-compare CSS across legs), exact conversion
// bytes ARE the contract here: the port mirrors the upstream state machine,
// so both legs must agree byte-for-byte. Every expectation was captured from
// the upstream oracle, never hand-written.

use rust_sass_libsass_tests::bindings::*;
use std::ffi::{c_char, c_int, c_void, CStr, CString};

// Option constants mirror `sass2scss.h` (both legs share the same values).
const PRETTIFY_0: c_int = 0;
const PRETTIFY_1: c_int = 1;
const PRETTIFY_2: c_int = 2;
const PRETTIFY_3: c_int = 3;
const KEEP_COMMENT: c_int = 32;
const STRIP_COMMENT: c_int = 64;
const CONVERT_COMMENT: c_int = 128;

/// Reads a malloc'd C string into bytes and frees it with `sass_free_memory`
/// (pins the caller-frees ownership rule).
unsafe fn read_owned(ptr: *mut c_char) -> Vec<u8> {
    assert!(!ptr.is_null());
    // SAFETY: test-only; `ptr` is a fresh allocation from the function under
    // test, freed exactly once here.
    let out = unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec();
    unsafe {
        sass_free_memory(ptr as *mut c_void);
    }
    out
}

/// Converts through the C ABI.
unsafe fn convert(sass: &str, options: c_int) -> Vec<u8> {
    let input = CString::new(sass).unwrap();
    // SAFETY: test-only; valid input, output freed in `read_owned`.
    unsafe { read_owned(sass2scss(input.as_ptr(), options)) }
}

#[test]
fn convert_prettify_levels() {
    let nest = "a\n  color: red\n  b\n    x: y\n";
    unsafe {
        assert_eq!(convert(nest, PRETTIFY_0), b"a { color: red;b { x: y; } }");
        assert_eq!(
            convert(nest, PRETTIFY_1),
            b"a {\n  color: red;\n  b {\n    x: y; } }\n"
        );
        assert_eq!(
            convert(nest, PRETTIFY_2),
            b"a {\n  color: red;\n  b {\n    x: y;\n  }\n}\n"
        );
        assert_eq!(
            convert(nest, PRETTIFY_3),
            b"a\n{\n  color: red;\n  b\n  {\n    x: y;\n  }\n}\n"
        );
    }
}

#[test]
fn convert_comment_flags() {
    let comments = "// silent\na\n  color: red // trailing\n  /* loud */\n";
    unsafe {
        assert_eq!(
            convert(comments, PRETTIFY_1),
            b"a {\n  color: red; // trailing\n  /* loud */ }\n"
        );
        assert_eq!(
            convert(comments, PRETTIFY_1 | KEEP_COMMENT),
            b"// silent\na {\n  color: red; // trailing\n  /* loud */ }\n"
        );
        assert_eq!(
            convert(comments, PRETTIFY_1 | STRIP_COMMENT),
            b"a {\n  color: red; }\n\n"
        );
        assert_eq!(
            convert(comments, PRETTIFY_1 | CONVERT_COMMENT),
            b"/* silent */\na {\n  color: red; /* trailing */\n  /* loud */ }\n"
        );
    }
}

#[test]
fn convert_syntax_constructs() {
    unsafe {
        assert_eq!(
            convert("a,\nb\n  color: red\n", PRETTIFY_1),
            b"a,\nb {\n  color: red; }\n"
        );
        assert_eq!(
            convert("=box\n  color: red\n+box\n", PRETTIFY_1),
            b"@mixin box {\n  color: red; }\n@include box;\n"
        );
        assert_eq!(
            convert("@import foo, \"bar\", url(baz)\n", PRETTIFY_1),
            b"@import \"foo\", \"bar\", url(baz);\n"
        );
        // Empty input converts to empty output (fresh allocation, freed).
        assert_eq!(convert("", PRETTIFY_1), b"");
    }
}

#[test]
fn convert_ownership_and_null() {
    // Upstream dereferences the input (`strlen`) without a NULL check, so
    // NULL input segfaults there. Our adapter hardens it to NULL (the
    // harden-don't-mirror rule); that case runs on our leg alone.
    unsafe {
        #[cfg(not(feature = "upstream"))]
        assert!(sass2scss(std::ptr::null(), PRETTIFY_1).is_null());
        // The result is a fresh allocation at a distinct address.
        let input = CString::new("a\n  x: y\n").unwrap();
        let got = sass2scss(input.as_ptr(), PRETTIFY_1);
        assert!(!got.is_null());
        assert_ne!(got, input.as_ptr() as *mut c_char);
        assert_eq!(read_owned(got), b"a {\n  x: y; }\n");
    }
}

#[test]
fn version_contract() {
    unsafe {
        let version = CStr::from_ptr(sass2scss_version());
        assert!(!version.to_bytes().is_empty());
        #[cfg(not(feature = "upstream"))]
        assert_eq!(version.to_bytes(), b"1.1.1");
    }
}
