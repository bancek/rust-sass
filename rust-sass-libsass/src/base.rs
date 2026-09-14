// Copyright 2012-2016 Sass Open Source Foundation. Use of this source code
// is governed by an MIT-style license that can be found in the LICENSE
// file or at https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// libsass-source: src/sass.cpp (alloc/copy/free) + src/util.cpp (quote/unquote) + include/sass/base.h

//! Memory management, version queries, and string quoting helpers.
//!
//! Mirrors `sass/base.h`: `sass_alloc_memory`, `sass_copy_c_string`,
//! `sass_free_memory`, `sass_string_quote`, `sass_string_unquote`,
//! `libsass_version`, `libsass_language_version`.
//!
//! # Safety boundary
//!
//! Every public function taking a pointer is `unsafe extern "C"` and documents
//! its contract under `# Safety`. Bodies are defensive: NULL inputs yield NULL
//! (never UB — where libsass itself would crash, e.g. `quote(NULL)`, we return
//! NULL per the plan's harden-don't-mirror rule), and panics are caught at the
//! boundary and converted to the NULL/error return. The two version getters
//! take no input and return static strings, so they are safe `extern "C"`
//! functions.

use std::ffi::{c_char, c_void, CStr};
use std::panic::AssertUnwindSafe;
use std::ptr;

/// ABI version reported by [`libsass_version`].
///
/// Plan decision D2: the last real libsass release — answers "which libsass is
/// this ABI compatible with", sorts above every real gate, parses everywhere.
static LIBSASS_VERSION_C: &[u8] = b"3.6.6\0";
/// Language version reported by [`libsass_language_version`].
///
/// Plan decision D3: the frozen libsass language claim; the engine behind this
/// ABI is a superset of it.
static LIBSASS_LANGUAGE_VERSION_C: &[u8] = b"3.5\0";

/// Runs `f`, converting a panic into `fallback`.
///
/// Every `extern "C"` entry point wraps its body in this: a panic unwinding
/// across the FFI boundary is undefined behavior. `AssertUnwindSafe` is sound
/// here — the adapter holds no locks or cross-call invariants; each call works
/// on caller-owned allocations or fresh boxes, so there is no state for a
/// panic to poison.
pub(crate) fn guard<F, T>(fallback: T, f: F) -> T
where
    F: FnOnce() -> T,
{
    std::panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

/// Allocates `size` bytes with libc malloc, aborting like libsass on OOM.
///
/// Encapsulates the `unsafe` allocator calls; safe to call because the only
/// contract — "the pointer is freed exactly once via [`sass_free_memory`]" —
/// is enforced by the C API's ownership rules by construction.
pub(crate) fn malloc_or_abort(size: usize) -> *mut c_void {
    // SAFETY: `malloc` accepts any size; the result is either NULL or a fresh
    // allocation owned by the caller.
    let got = unsafe { libc::malloc(size) };
    if got.is_null() {
        eprintln!("Out of memory.");
        // SAFETY: `exit` never returns and no destructors or guards are live
        // (mirrors libsass `sass_alloc_memory`, sass.cpp:37-45).
        unsafe {
            libc::exit(libc::EXIT_FAILURE);
        }
    }
    got
}

/// Copies `bytes` plus a NUL terminator into a fresh malloc'd buffer.
///
/// Interior NUL bytes are replaced with U+FFFD (EF BF BD): no C consumer can
/// observe a NUL through `char*` (`%s`/`strlen` stop there, silently dropping
/// the tail), so substitution preserves the message where truncation would
/// lose it. This mirrors libsass's own policy for unrepresentable bytes
/// (`emit_string` writes U+FFFD for invalid UTF-8, json.cpp). Inputs without
/// NULs are byte-exact.
///
/// The caller takes ownership (frees with [`sass_free_memory`]).
pub(crate) fn copy_bytes_nul(bytes: &[u8]) -> *mut c_char {
    // Fast path: no NULs (all existing callers in the common case).
    if !bytes.contains(&0) {
        let len = bytes.len();
        // SAFETY: fresh allocation of exactly len+1 bytes; both writes in bounds.
        let dst = malloc_or_abort(len + 1) as *mut u8;
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), dst, len);
            dst.add(len).write(0);
        }
        return dst as *mut c_char;
    }
    // Slow path: expand each NUL to EF BF BD, then terminate.
    let mut expanded = Vec::with_capacity(bytes.len() + 1);
    for chunk in bytes.split(|&b| b == 0) {
        if !expanded.is_empty() {
            expanded.extend_from_slice(&[0xef, 0xbf, 0xbd]);
        }
        expanded.extend_from_slice(chunk);
    }
    expanded.push(0);
    let dst = malloc_or_abort(expanded.len()) as *mut u8;
    unsafe {
        ptr::copy_nonoverlapping(expanded.as_ptr(), dst, expanded.len());
    }
    dst as *mut c_char
}

/// Allocates `size` bytes of libsass heap memory.
///
/// Mirrors libsass `sass_alloc_memory` (sass.cpp:37-45): plain `malloc`, and
/// on failure prints `Out of memory.` to stderr and exits the process.
///
/// # Safety
///
/// The returned pointer must be freed exactly once with [`sass_free_memory`].
/// It must not be freed with any other allocator.
#[no_mangle]
pub unsafe extern "C" fn sass_alloc_memory(size: usize) -> *mut c_void {
    malloc_or_abort(size)
}

/// Copies a C string into fresh libsass heap memory.
///
/// Mirrors libsass `sass_copy_c_string` (sass.cpp:47-54): NULL in → NULL out,
/// otherwise a byte-exact copy including the terminator. The caller takes
/// ownership (frees with [`sass_free_memory`]).
///
/// # Safety
///
/// `s` must be either NULL or a valid NUL-terminated string that remains
/// valid for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn sass_copy_c_string(s: *const c_char) -> *mut c_char {
    guard(ptr::null_mut(), || {
        if s.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: NULL checked; validity + NUL-termination per contract above.
        let bytes = unsafe { CStr::from_ptr(s) }.to_bytes();
        copy_bytes_nul(bytes)
    })
}

/// Frees libsass heap memory. NULL-safe no-op.
///
/// Mirrors libsass `sass_free_memory` (sass.cpp:57-60).
///
/// # Safety
///
/// `ptr` must be NULL or a pointer previously returned by this library's
/// allocators (`sass_alloc_memory`, `sass_copy_c_string`, `sass_string_quote`,
/// `sass_string_unquote`, …) that has not been freed yet.
#[no_mangle]
pub unsafe extern "C" fn sass_free_memory(ptr: *mut c_void) {
    guard((), || {
        if !ptr.is_null() {
            // SAFETY: non-NULL heap allocation per contract above.
            unsafe {
                libc::free(ptr);
            }
        }
    });
}

/// Quotes `s` with `quote_mark`, escaping embedded quotes and backslashes.
///
/// Mirrors libsass `sass_string_quote` (sass.cpp:63-67) over `quote`
/// (util.cpp:434-493): the mark is auto-detected unless forced —
/// a `'` anywhere forces `"`, otherwise a `"` switches a non-`"` mark to `'`
/// (`detect_best_quotemark`, util.cpp:257-271; mark `0`/`'*'` falls back to
/// `"`); empty input yields two quote chars; embedded marks and backslashes
/// are backslash-escaped; `\r\n` collapses to nothing for the `\r` and `\n`
/// emits `\a` plus a separating space when the next byte is a hex digit or
/// ASCII whitespace; non-ASCII bytes pass through untouched. The caller takes
/// ownership (frees with [`sass_free_memory`]).
///
/// Differs from libsass only in hardening: NULL input returns NULL instead of
/// crashing, and invalid UTF-8 bytes pass through singly instead of hitting
/// the C++ UTF-8 decoder.
///
/// # Safety
///
/// `s` must be either NULL or a valid NUL-terminated string that remains
/// valid for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn sass_string_quote(s: *const c_char, quote_mark: c_char) -> *mut c_char {
    guard(ptr::null_mut(), || {
        if s.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: NULL checked; validity + NUL-termination per contract above.
        let bytes = unsafe { CStr::from_ptr(s) }.to_bytes();
        copy_bytes_nul(&quote_bytes(bytes, quote_mark as u8))
    })
}

/// Removes one layer of surrounding quotes, resolving CSS escapes.
///
/// Mirrors libsass `sass_string_unquote` (sass.cpp:70-74) over `unquote`
/// (util.cpp:342-432, strict mode): inputs shorter than 2 bytes, or whose
/// first/last bytes are not matching `"`/`'` pairs, are returned unchanged; a
/// bare delimiter inside aborts and returns the input unchanged; `\` + hex
/// digits decode to the code point (U+0000 → U+FFFD, optional single trailing
/// space consumed); a lone backslash is dropped and the next byte kept
/// literally; a trailing lone backslash aborts and returns the input
/// unchanged. The caller takes ownership (frees with [`sass_free_memory`]).
///
/// Differs from libsass only in hardening: NULL input returns NULL instead of
/// crashing, and out-of-range code points map to U+FFFD instead of hitting
/// the C++ UTF-8 encoder's assertions.
///
/// # Safety
///
/// `s` must be either NULL or a valid NUL-terminated string that remains
/// valid for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn sass_string_unquote(s: *const c_char) -> *mut c_char {
    guard(ptr::null_mut(), || {
        if s.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: NULL checked; validity + NUL-termination per contract above.
        let bytes = unsafe { CStr::from_ptr(s) }.to_bytes();
        copy_bytes_nul(&unquote_bytes(bytes))
    })
}

/// Reports the libsass ABI version this library implements.
///
/// Plan decision D2: `"3.6.6"` — the last real libsass release. The returned
/// pointer is a static string and must NOT be freed.
#[no_mangle]
pub extern "C" fn libsass_version() -> *const c_char {
    LIBSASS_VERSION_C.as_ptr() as *const c_char
}

/// Reports the Sass language version this library implements.
///
/// Plan decision D3: `"3.5"` — the frozen libsass language claim. The returned
/// pointer is a static string and must NOT be freed.
#[no_mangle]
pub extern "C" fn libsass_language_version() -> *const c_char {
    LIBSASS_LANGUAGE_VERSION_C.as_ptr() as *const c_char
}

/// Reports the version of the bundled `sass2scss` converter.
///
/// Reports the upstream converter version the port implements (see
/// `sass2scss.rs`); kept as a separate symbol because `sassc -v` links it
/// directly.
#[no_mangle]
pub extern "C" fn sass2scss_version() -> *const c_char {
    static VERSION_C: &[u8] = b"1.1.1\0";
    VERSION_C.as_ptr() as *const c_char
}

/// Picks the quote mark: `'` anywhere forces `"`; a `"` switches a non-`"`,
/// non-fallback mark to `'`; mark `0`/`'*'` falls back to `"`.
/// Mirrors `detect_best_quotemark` (util.cpp:257-271).
fn detect_best_quotemark(s: &[u8], quote_mark: u8) -> u8 {
    let mut mark = if quote_mark != 0 && quote_mark != b'*' {
        quote_mark
    } else {
        b'"'
    };
    for &b in s {
        if b == b'\'' {
            return b'"';
        } else if b == b'"' {
            mark = b'\'';
        }
    }
    mark
}

/// ASCII whitespace as libsass defines it (`ascii_isspace`, util_string.hpp):
/// Rust's `is_ascii_whitespace` omits vertical tab, so this is spelled out.
fn is_libsass_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0C' | b'\r' | b'\x0B')
}

/// Decodes one UTF-8 sequence at `s[i]`, returning (code point, byte length).
/// Invalid bytes (bad lead, truncated tail, bad continuation, overlong,
/// surrogate, out-of-range) decode as the single raw byte — a hardening over
/// the C++ decoder, which cannot observe such input through the C API anyway
/// (documented on [`sass_string_quote`]).
fn decode_utf8_at(s: &[u8], i: usize) -> (u32, usize) {
    let b0 = s[i];
    if b0 < 0x80 {
        return (u32::from(b0), 1);
    }
    let len = if b0 >> 5 == 0b110 {
        2
    } else if b0 >> 4 == 0b1110 {
        3
    } else if b0 >> 3 == 0b11110 {
        4
    } else {
        return (u32::from(b0), 1);
    };
    if i + len > s.len() {
        return (u32::from(b0), 1);
    }
    let mut cp: u32 = u32::from(b0 & (0xff >> (len + 1)));
    for k in 1..len {
        let b = s[i + k];
        if b >> 6 != 0b10 {
            return (u32::from(b0), 1);
        }
        cp = (cp << 6) | u32::from(b & 0x3f);
    }
    // Minimum value per length rejects overlongs; surrogates and > U+10FFFF
    // are not characters.
    let min: u32 = match len {
        2 => 0x80,
        3 => 0x800,
        _ => 0x10000,
    };
    if cp < min || (0xd800..0xe000).contains(&cp) || cp > 0x10ffff {
        return (u32::from(b0), 1);
    }
    (cp, len)
}

/// Implements `quote` (util.cpp:434-493) over bytes; see [`sass_string_quote`].
///
/// NOTE: this deliberately does NOT reuse the core's `quote_inner_text`
/// (`rust-sass/src/ast/sass/expression_string.rs`) / `best_quote`, although
/// the shape is similar. They encode different engines' rules: the core
/// follows Dart (escapes lone `\r` and `\x0C`, omits the merge-space before
/// `\v`, optionally escapes `#{`, operates on `&str`), while this C entry
/// point promises libsass behavior (raw `\r`/`\x0C`, `\v` in the merge set,
/// no `#{` handling, arbitrary bytes). Collapsing the two would trade an
/// 80-line tested port for silent divergences plus lossy UTF-8 conversion at
/// the boundary.
fn quote_bytes(s: &[u8], quote_mark: u8) -> Vec<u8> {
    let q = detect_best_quotemark(s, quote_mark);
    if s.is_empty() {
        return vec![q, q];
    }
    let mut quoted = Vec::with_capacity(s.len() + 2);
    quoted.push(q);
    let mut i = 0;
    while i < s.len() {
        let b = s[i];
        if b == q {
            quoted.extend_from_slice(&[b'\\', b]);
            i += 1;
        } else if b == b'\\' {
            quoted.extend_from_slice(b"\\\\");
            i += 1;
        } else {
            let (cp, len) = decode_utf8_at(s, i);
            // `\r` followed by `\n` emits nothing for the `\r` (the `\n`
            // arm below handles the pair).
            if cp == u32::from(b'\r') && s.get(i + len) == Some(&b'\n') {
                i += len;
                continue;
            }
            if cp == u32::from(b'\n') {
                quoted.extend_from_slice(b"\\a");
                // A separating space is needed exactly when the next byte
                // would otherwise merge into the escape: hex digit or ASCII
                // whitespace (mirrors the Ruby-derived `alternatives` check).
                if let Some(&next) = s.get(i + len) {
                    if next.is_ascii_hexdigit() || is_libsass_space(next) {
                        quoted.push(b' ');
                    }
                }
            } else if cp < 127 {
                quoted.push(cp as u8);
            } else {
                quoted.extend_from_slice(&s[i..i + len]);
            }
            i += len;
        }
    }
    quoted.push(q);
    quoted
}

/// Implements `unquote` in strict mode (util.cpp:342-432) over bytes; see
/// [`sass_string_unquote`].
fn unquote_bytes(s: &[u8]) -> Vec<u8> {
    // Not enough room for quotes: no possibility to unquote.
    if s.len() < 2 {
        return s.to_vec();
    }
    let q = match (s[0], s[s.len() - 1]) {
        (b'"', b'"') => b'"',
        (b'\'', b'\'') => b'\'',
        _ => return s.to_vec(),
    };
    let mut unquoted = Vec::with_capacity(s.len().saturating_sub(2));
    let mut skipped = false;
    let mut i = 1;
    let last = s.len() - 1;
    // NOTE: this deliberately mirrors the C++ `for` loop, whose `++i` runs
    // even in the backslash arm: a lone backslash sets `skipped` and is
    // dropped on the *next* iteration, which pushes the following byte
    // literally.
    while i < last {
        let b = s[i];
        if b == b'\\' && !skipped {
            skipped = true;
            let mut len = 1;
            while i + len < last && s[i + len] != 0 && s[i + len].is_ascii_hexdigit() {
                len += 1;
            }
            if len > 1 {
                let hex = core::str::from_utf8(&s[i + 1..i + len]).unwrap_or("");
                let mut cp = u32::from_str_radix(hex, 16).unwrap_or(0xfffd);
                // U+0000 is asserted to U+FFFD upstream.
                if cp == 0 {
                    cp = 0xfffd;
                }
                let mut advance = len;
                // `i + len <= last` always holds (the hex scan stops at
                // `last`), so this indexes the closing quote at worst.
                if s[i + len] == b' ' {
                    advance += 1;
                }
                let ch = char::from_u32(cp).unwrap_or('\u{fffd}');
                let mut buf = [0u8; 4];
                unquoted.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                i += advance - 1;
                skipped = false;
            }
        } else {
            // Strict mode: a bare delimiter aborts and returns the input.
            if b == q {
                return s.to_vec();
            }
            skipped = false;
            unquoted.push(b);
        }
        i += 1;
    }
    // A trailing lone backslash leaves `skipped` set: unquoting fails and the
    // input is returned unchanged (util.cpp:428).
    if skipped {
        return s.to_vec();
    }
    unquoted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    /// Reads a malloc'd NUL-terminated C string into bytes (without taking
    /// ownership — ownership moves to the caller separately).
    unsafe fn read_c_str(ptr: *mut c_char) -> Vec<u8> {
        assert!(!ptr.is_null());
        // SAFETY: test-only; pointer is a valid NUL-terminated string produced
        // by the function under test.
        unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec()
    }

    #[test]
    fn alloc_free_roundtrip() {
        let got = unsafe { sass_alloc_memory(64) };
        assert!(!got.is_null());
        unsafe {
            (got as *mut u8).write_bytes(0xab, 64);
            assert_eq!((got as *mut u8).read(), 0xab);
            sass_free_memory(got);
            // NULL free is a no-op (must not crash).
            sass_free_memory(ptr::null_mut());
        }
    }

    #[test]
    fn alloc_zero_is_non_null() {
        // malloc(0) yields a freeable pointer on macOS/Linux; the adapter
        // targets those platforms only.
        let got = unsafe { sass_alloc_memory(0) };
        assert!(!got.is_null());
        unsafe {
            sass_free_memory(got);
        }
    }

    #[test]
    fn copy_c_string_contract() {
        unsafe {
            // NULL in → NULL out.
            assert!(sass_copy_c_string(ptr::null()).is_null());
            // Byte-exact copy at a distinct address, NUL-terminated.
            let src = CString::new("hello").unwrap();
            let got = sass_copy_c_string(src.as_ptr());
            assert!(!got.is_null());
            assert_ne!(got, src.as_ptr() as *mut c_char);
            assert_eq!(read_c_str(got), b"hello");
            sass_free_memory(got as *mut c_void);
            // Empty string copies to a lone NUL.
            let empty = CString::new("").unwrap();
            let got_empty = sass_copy_c_string(empty.as_ptr());
            assert_eq!(read_c_str(got_empty), b"");
            sass_free_memory(got_empty as *mut c_void);
        }
    }

    #[test]
    fn copy_bytes_nul_replaces_interior_nuls() {
        // No C consumer can observe an interior NUL through `char*`
        // (`%s`/`strlen` stop there); substitution with U+FFFD preserves the
        // tail where truncation would lose it (plan G26).
        let got = copy_bytes_nul(b"a\x00b\x00c");
        let bytes = unsafe { read_c_str(got) };
        assert_eq!(bytes, "a\u{fffd}b\u{fffd}c".as_bytes());
        unsafe {
            sass_free_memory(got as *mut c_void);
        }
        // Clean inputs stay byte-exact (fast path).
        let clean = copy_bytes_nul(b"plain");
        assert_eq!(unsafe { read_c_str(clean) }, b"plain");
        unsafe {
            sass_free_memory(clean as *mut c_void);
        }
    }

    /// Calls a `*const c_char -> *mut c_char` helper and returns the owned
    /// Rust string, freeing the C allocation.
    unsafe fn call_str_fn(
        f: unsafe extern "C" fn(*const c_char) -> *mut c_char,
        s: &str,
    ) -> String {
        let input = CString::new(s).unwrap();
        // SAFETY: test-only; input is a valid CString, output freed below.
        let got = unsafe { f(input.as_ptr()) };
        let out = String::from_utf8(unsafe { read_c_str(got) }).unwrap();
        unsafe {
            sass_free_memory(got as *mut c_void);
        }
        out
    }

    #[test]
    fn quote_vectors() {
        unsafe {
            let quote = |s: &str, mark: c_char| {
                let input = CString::new(s).unwrap();
                let got = sass_string_quote(input.as_ptr(), mark);
                let out = String::from_utf8(read_c_str(got)).unwrap();
                sass_free_memory(got as *mut c_void);
                out
            };
            // Plain strings take the requested mark; empty yields bare quotes.
            assert_eq!(quote("foo", b'"' as c_char), "\"foo\"");
            assert_eq!(quote("", b'"' as c_char), "\"\"");
            // A double quote inside switches an explicit `"` mark to `'`.
            assert_eq!(quote("a\"b", b'"' as c_char), "'a\"b'");
            // A single quote inside forces `"`.
            assert_eq!(quote("a'b", b'"' as c_char), "\"a'b\"");
            // Both kinds present: `'` wins, `"` is escaped.
            assert_eq!(quote("a'b\"c", 0), "\"a'b\\\"c\"");
            // Marks 0 and '*' fall back to `"`.
            assert_eq!(quote("x", 0), "\"x\"");
            assert_eq!(quote("x", b'*' as c_char), "\"x\"");
            // Backslashes double; newlines become `\a` with the merge-space
            // rules (`b` is a hex digit → space; existing space → space).
            assert_eq!(quote("a\\b", b'"' as c_char), "\"a\\\\b\"");
            assert_eq!(quote("a\nb", b'"' as c_char), "\"a\\a b\"");
            assert_eq!(quote("a\n b", b'"' as c_char), "\"a\\a  b\"");
            assert_eq!(quote("a\nzb", b'"' as c_char), "\"a\\azb\"");
            // Non-ASCII passes through untouched.
            assert_eq!(quote("héllo", b'"' as c_char), "\"héllo\"");
            // NULL hardens to NULL (libsass would crash).
            assert!(sass_string_quote(ptr::null(), b'"' as c_char).is_null());
        }
    }

    #[test]
    fn unquote_vectors() {
        unsafe {
            // Quoted pairs strip; too-short and unquoted pass through.
            assert_eq!(call_str_fn(sass_string_unquote, "\"foo\""), "foo");
            assert_eq!(call_str_fn(sass_string_unquote, "'foo'"), "foo");
            assert_eq!(call_str_fn(sass_string_unquote, "\"\""), "");
            assert_eq!(call_str_fn(sass_string_unquote, "a"), "a");
            assert_eq!(call_str_fn(sass_string_unquote, "ab"), "ab");
            assert_eq!(call_str_fn(sass_string_unquote, "\"ab'"), "\"ab'");
            // Hex escapes decode (`\41` → `A`), optional space consumed.
            assert_eq!(call_str_fn(sass_string_unquote, "\"\\41\""), "A");
            assert_eq!(call_str_fn(sass_string_unquote, "\"\\41 B\""), "AB");
            // Lone backslash drops, next byte kept literally…
            assert_eq!(call_str_fn(sass_string_unquote, "\"a\\nb\""), "anb");
            assert_eq!(call_str_fn(sass_string_unquote, "\"a\\\\b\""), "a\\b");
            // …but a bare delimiter aborts and returns the input unchanged.
            assert_eq!(call_str_fn(sass_string_unquote, "\"a\"b\""), "\"a\"b\"");
            // Trailing lone backslash aborts too.
            assert_eq!(call_str_fn(sass_string_unquote, "\"ab\\\""), "\"ab\\\"");
            // NULL hardens to NULL (libsass would crash).
            assert!(sass_string_unquote(ptr::null()).is_null());
        }
    }

    #[test]
    fn quote_unquote_roundtrip() {
        unsafe {
            for s in ["plain", "with space", "héllo wörld", "100%"] {
                let quoted = {
                    let input = CString::new(s).unwrap();
                    let got = sass_string_quote(input.as_ptr(), b'"' as c_char);
                    let out = String::from_utf8(read_c_str(got)).unwrap();
                    sass_free_memory(got as *mut c_void);
                    out
                };
                assert_eq!(call_str_fn(sass_string_unquote, &quoted), s);
            }
        }
    }

    #[test]
    fn version_strings() {
        unsafe {
            let version = CStr::from_ptr(libsass_version());
            assert_eq!(version.to_bytes(), b"3.6.6");
            let language = CStr::from_ptr(libsass_language_version());
            assert_eq!(language.to_bytes(), b"3.5");
            // Static strings: stable across calls, never freed.
            assert_eq!(libsass_version(), version.as_ptr());
            assert_eq!(libsass_language_version(), language.as_ptr());
        }
    }

    #[test]
    fn quotemark_detection() {
        assert_eq!(detect_best_quotemark(b"abc", b'"'), b'"');
        assert_eq!(detect_best_quotemark(b"abc", 0), b'"');
        assert_eq!(detect_best_quotemark(b"abc", b'*'), b'"');
        assert_eq!(detect_best_quotemark(b"a'b", b'\''), b'"');
        assert_eq!(detect_best_quotemark(b"a\"b", b'"'), b'\'');
        assert_eq!(detect_best_quotemark(b"a\"b", b'\''), b'\'');
    }
}
