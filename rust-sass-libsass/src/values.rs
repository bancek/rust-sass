// Copyright 2012-2016 Sass Open Source Foundation. Use of this source code
// is governed by an MIT-style license that can be found in the LICENSE
// file or at https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// libsass-source: src/sass_values.cpp + src/sass_values.hpp (union) + include/sass/values.h

//! Sass values: the `Sass_Value` heap union with all makers, getters,
//! setters, predicates, clone, and delete.
//!
//! Mirrors `sass/values.h`: `sass_make_*`, `sass_delete_value`,
//! `sass_clone_value`, `sass_value_get_tag`, `sass_value_is_*`, and every
//! per-kind getter/setter. `sass_value_op` / `sass_value_stringify` need the
//! eval engine and arrive with custom-function bridging (plan §7 step 6);
//! until then they return `sass_make_error` values — the same failure idiom
//! `sass_value_op` itself uses for exceptional results.
//!
//! # Ownership (mirrors libsass, plan §A.2 with hardening)
//!
//! - Every value is a malloc'd `Sass_Value` union owned by its holder;
//!   `sass_delete_value` frees recursively (lists/maps) and is NULL-safe.
//! - Makers COPY input strings (the caller's buffer stays owned by the
//!   caller); NULL inputs fail the make (return NULL) for number units and
//!   string/error/warning payloads — unitless is `""`, not NULL.
//! - `set_unit` / `set_value` / `set_message` TAKE ownership (store raw, no
//!   copy — node-sass passes fresh `create_string` buffers with no free).
//!   Unlike upstream (which overwrites and leaks the old string), the
//!   adapter frees the previously owned value first — unobservable except
//!   to leak checkers.
//! - `list/map_set_*` TAKE ownership of the entry (freeing any previously
//!   stored entry first, same hardening); getters BORROW (no transfer).
//! - Indexed getters harden out-of-range/NULL to NULL (upstream
//!   unchecked-derefs — the contract pins in-range behavior identically).
//! - The union layout mirrors `sass_values.hpp` field-for-field (`tag`
//!   first in every arm); consumers only ever touch values through these
//!   functions (the public headers keep `Sass_Value` opaque), but exactness
//!   keeps the door open for any consumer that peeks.

use std::ffi::{c_char, c_double, c_int, CStr};
use std::ptr;

use crate::base::{copy_bytes_nul, guard, malloc_or_abort};

/// `Sass_Tag` values, matching `include/sass/values.h:17-27`.
pub const SASS_BOOLEAN: u32 = 0;
pub const SASS_NUMBER: u32 = 1;
pub const SASS_COLOR: u32 = 2;
pub const SASS_STRING: u32 = 3;
pub const SASS_LIST: u32 = 4;
pub const SASS_MAP: u32 = 5;
pub const SASS_NULL: u32 = 6;
pub const SASS_ERROR: u32 = 7;
pub const SASS_WARNING: u32 = 8;

/// `Sass_Separator` values (`values.h:30-36`; `HASH` is the unevaluated-map
/// sentinel — stored and returned verbatim, never interpreted here).
pub const SASS_COMMA: u32 = 0;
pub const SASS_SPACE: u32 = 1;
pub const SASS_HASH: u32 = 2;

/// `Sass_OP` values (`values.h:39-44`); consumed by `sass_value_op` when it
/// lands with engine bridging.
pub const SASS_OP_AND: u32 = 0;
pub const SASS_OP_OR: u32 = 1;
pub const SASS_OP_EQ: u32 = 2;
pub const SASS_OP_NEQ: u32 = 3;
pub const SASS_OP_GT: u32 = 4;
pub const SASS_OP_GTE: u32 = 5;
pub const SASS_OP_LT: u32 = 6;
pub const SASS_OP_LTE: u32 = 7;
pub const SASS_OP_ADD: u32 = 8;
pub const SASS_OP_SUB: u32 = 9;
pub const SASS_OP_MUL: u32 = 10;
pub const SASS_OP_DIV: u32 = 11;
pub const SASS_OP_MOD: u32 = 12;

#[repr(C)]
#[derive(Clone, Copy)]
struct SassUnknown {
    tag: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SassBoolean {
    tag: u32,
    value: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SassNumber {
    tag: u32,
    value: c_double,
    unit: *mut c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SassColor {
    tag: u32,
    r: c_double,
    g: c_double,
    b: c_double,
    a: c_double,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SassString {
    tag: u32,
    quoted: bool,
    value: *mut c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SassList {
    tag: u32,
    separator: u32,
    is_bracketed: bool,
    length: usize,
    values: *mut *mut SassValue,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SassMapPair {
    key: *mut SassValue,
    value: *mut SassValue,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SassMap {
    tag: u32,
    length: usize,
    pairs: *mut SassMapPair,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SassNull {
    tag: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SassError {
    tag: u32,
    message: *mut c_char,
}

/// The `Sass_Value` heap union. Mirrors `sass_values.hpp` arm-for-arm.
#[repr(C)]
pub union SassValue {
    unknown: SassUnknown,
    boolean: SassBoolean,
    number: SassNumber,
    color: SassColor,
    string: SassString,
    list: SassList,
    map: SassMap,
    null: SassNull,
    error: SassError,
    warning: SassError,
}

/// Allocates a zeroed value union (mirrors the `calloc(1, …)` makers).
fn alloc_value() -> *mut SassValue {
    // SAFETY: fresh zeroed allocation of exactly one union; ownership moves
    // to the caller. `malloc_or_abort` never returns NULL (aborts instead,
    // like libsass's alloc path).
    let got = malloc_or_abort(size_of::<SassValue>()) as *mut SassValue;
    unsafe {
        ptr::write_bytes(got, 0, 1);
    }
    got
}

/// Copies a C string or fails NULL (the maker-failure idiom for NULL inputs:
/// `if (… == 0) { free(v); return 0; }`).
///
/// # Safety
///
/// `s` must be NULL or a valid NUL-terminated string for the call.
unsafe fn copy_or_null(s: *const c_char) -> *mut c_char {
    if s.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: non-null NUL-terminated string per contract above.
    let bytes = unsafe { CStr::from_ptr(s) }.to_bytes();
    copy_bytes_nul(bytes)
}

/// Reads a live value's tag (NULL → `SASS_NULL`-shaped zero is NOT applied:
/// callers must not pass NULL — all tag/predicate entry points harden NULL
/// to the false/zero return instead).
///
/// # Safety
///
/// `v` must be NULL or a live value for the call.
unsafe fn tag_of(v: *const SassValue) -> u32 {
    if v.is_null() {
        return u32::MAX;
    }
    // SAFETY: live value per contract; `tag` is valid for every arm.
    unsafe { (*v).unknown.tag }
}

/// Frees one owned string slot (NULL-safe).
fn free_string(slot: *mut c_char) {
    if !slot.is_null() {
        // SAFETY: non-null heap allocation owned by the value.
        unsafe {
            libc::free(slot as *mut libc::c_void);
        }
    }
}

/// Recursive delete body (mirrors `sass_delete_value`, sass_values.cpp:191).
///
/// # Safety
///
/// `v` must be NULL or a live value, freed exactly once here.
unsafe fn delete_inner(v: *mut SassValue) {
    if v.is_null() {
        return;
    }
    // SAFETY: live value per contract; arms read only their own fields.
    unsafe {
        match (*v).unknown.tag {
            SASS_NUMBER => free_string((*v).number.unit),
            SASS_STRING => free_string((*v).string.value),
            SASS_LIST => {
                let len = (*v).list.length;
                let values = (*v).list.values;
                if !values.is_null() {
                    for i in 0..len {
                        // SAFETY: `values` holds `len` entries per construction.
                        delete_inner(*values.add(i));
                    }
                    libc::free(values as *mut libc::c_void);
                }
            }
            SASS_MAP => {
                let len = (*v).map.length;
                let pairs = (*v).map.pairs;
                if !pairs.is_null() {
                    for i in 0..len {
                        // SAFETY: `pairs` holds `len` entries per construction.
                        let pair = pairs.add(i);
                        delete_inner((*pair).key);
                        delete_inner((*pair).value);
                    }
                    libc::free(pairs as *mut libc::c_void);
                }
            }
            SASS_ERROR | SASS_WARNING => free_string((*v).error.message),
            // Null, boolean, color: no owned memory.
            _ => {}
        }
        libc::free(v as *mut libc::c_void);
    }
}

/// Deep-clone body (mirrors `sass_clone_value`, sass_values.cpp:235).
///
/// # Safety
///
/// `v` must be NULL or a live value for the call; the fresh copy is owned by
/// the caller. NULL in → NULL out.
unsafe fn clone_inner(v: *const SassValue) -> *mut SassValue {
    if v.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: live value per contract; arms read only their own fields, and
    // every maker below copies owned strings (never aliases).
    unsafe {
        match (*v).unknown.tag {
            SASS_NULL => sass_make_null(),
            SASS_BOOLEAN => sass_make_boolean((*v).boolean.value),
            SASS_NUMBER => sass_make_number((*v).number.value, (*v).number.unit),
            SASS_COLOR => sass_make_color((*v).color.r, (*v).color.g, (*v).color.b, (*v).color.a),
            SASS_STRING => {
                if (*v).string.quoted {
                    sass_make_qstring((*v).string.value)
                } else {
                    sass_make_string((*v).string.value)
                }
            }
            SASS_LIST => {
                let out = sass_make_list(
                    (*v).list.length,
                    (*v).list.separator,
                    (*v).list.is_bracketed,
                );
                if !out.is_null() {
                    for i in 0..(*v).list.length {
                        // SAFETY: both arrays hold `length` entries.
                        let child = clone_inner(*(*v).list.values.add(i));
                        *(*out).list.values.add(i) = child;
                    }
                }
                out
            }
            SASS_MAP => {
                let out = sass_make_map((*v).map.length);
                if !out.is_null() {
                    for i in 0..(*v).map.length {
                        // SAFETY: both arrays hold `length` entries.
                        let pair = (*v).map.pairs.add(i);
                        let out_pair = (*out).map.pairs.add(i);
                        (*out_pair).key = clone_inner((*pair).key);
                        (*out_pair).value = clone_inner((*pair).value);
                    }
                }
                out
            }
            SASS_ERROR => sass_make_error((*v).error.message),
            SASS_WARNING => sass_make_warning((*v).warning.message),
            _ => ptr::null_mut(),
        }
    }
}

/// Creates a null value.
///
/// Mirrors `sass_make_null` (sass_values.cpp:162-168).
///
/// # Safety
///
/// Always safe to call; the result is owned by the caller.
#[no_mangle]
pub unsafe extern "C" fn sass_make_null() -> *mut SassValue {
    guard(ptr::null_mut(), || {
        let v = alloc_value();
        // SAFETY: fresh allocation.
        unsafe {
            (*v).null = SassNull { tag: SASS_NULL };
        }
        v
    })
}

/// Creates a boolean value.
///
/// Mirrors `sass_make_boolean` (sass_values.cpp:84-91).
///
/// # Safety
///
/// Always safe to call; the result is owned by the caller.
#[no_mangle]
pub unsafe extern "C" fn sass_make_boolean(val: bool) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        let v = alloc_value();
        // SAFETY: fresh allocation.
        unsafe {
            (*v).boolean = SassBoolean {
                tag: SASS_BOOLEAN,
                value: val,
            };
        }
        v
    })
}

/// Creates a number value, copying `unit`. NULL units fail the make (return
/// NULL) — unitless is `""`, not NULL.
///
/// Mirrors `sass_make_number` (sass_values.cpp:93-102).
///
/// # Safety
///
/// `unit` must be NULL or a valid NUL-terminated string for the call. The
/// result is owned by the caller.
#[no_mangle]
pub unsafe extern "C" fn sass_make_number(val: c_double, unit: *const c_char) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        // SAFETY: NULL/validity per contract above.
        let owned = unsafe { copy_or_null(unit) };
        if owned.is_null() {
            return ptr::null_mut();
        }
        let v = alloc_value();
        // SAFETY: fresh allocation.
        unsafe {
            (*v).number = SassNumber {
                tag: SASS_NUMBER,
                value: val,
                unit: owned,
            };
        }
        v
    })
}

/// Creates a color value.
///
/// Mirrors `sass_make_color` (sass_values.cpp:104-114).
///
/// # Safety
///
/// Always safe to call; the result is owned by the caller.
#[no_mangle]
pub unsafe extern "C" fn sass_make_color(
    r: c_double,
    g: c_double,
    b: c_double,
    a: c_double,
) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        let v = alloc_value();
        // SAFETY: fresh allocation.
        unsafe {
            (*v).color = SassColor {
                tag: SASS_COLOR,
                r,
                g,
                b,
                a,
            };
        }
        v
    })
}

/// Creates an unquoted string value, copying `val`. NULL fails the make.
///
/// Mirrors `sass_make_string` (sass_values.cpp:116-125).
///
/// # Safety
///
/// `val` must be NULL or a valid NUL-terminated string for the call. The
/// result is owned by the caller.
#[no_mangle]
pub unsafe extern "C" fn sass_make_string(val: *const c_char) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        // SAFETY: NULL/validity per contract above.
        let owned = unsafe { copy_or_null(val) };
        if owned.is_null() {
            return ptr::null_mut();
        }
        let v = alloc_value();
        // SAFETY: fresh allocation.
        unsafe {
            (*v).string = SassString {
                tag: SASS_STRING,
                quoted: false,
                value: owned,
            };
        }
        v
    })
}

/// Creates a quoted string value, copying `val`. NULL fails the make.
///
/// Mirrors `sass_make_qstring` (sass_values.cpp:127-136).
///
/// # Safety
///
/// See [`sass_make_string`].
#[no_mangle]
pub unsafe extern "C" fn sass_make_qstring(val: *const c_char) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        // SAFETY: NULL/validity per contract above.
        let owned = unsafe { copy_or_null(val) };
        if owned.is_null() {
            return ptr::null_mut();
        }
        let v = alloc_value();
        // SAFETY: fresh allocation.
        unsafe {
            (*v).string = SassString {
                tag: SASS_STRING,
                quoted: true,
                value: owned,
            };
        }
        v
    })
}

/// Creates a list value with `len` slots (initialized NULL) and the given
/// separator/bracketed flags.
///
/// Mirrors `sass_make_list` (sass_values.cpp:138-149).
///
/// # Safety
///
/// Always safe to call; the result (including not-yet-set slots) is owned
/// by the caller. `sass_delete_value` frees NULL slots safely.
#[no_mangle]
pub unsafe extern "C" fn sass_make_list(
    len: usize,
    sep: u32,
    is_bracketed: bool,
) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        // SAFETY: fresh zeroed array of exactly `len` pointers (malloc(0) is
        // non-NULL on the adapter's macOS/Linux targets; see the
        // `alloc_zero_is_non_null` test in base.rs).
        let values =
            malloc_or_abort(len.wrapping_mul(size_of::<*mut SassValue>())) as *mut *mut SassValue;
        unsafe {
            ptr::write_bytes(values, 0, len);
        }
        let v = alloc_value();
        // SAFETY: fresh allocation.
        unsafe {
            (*v).list = SassList {
                tag: SASS_LIST,
                separator: sep,
                is_bracketed,
                length: len,
                values,
            };
        }
        v
    })
}

/// Creates a map value with `len` pairs (keys/values initialized NULL).
///
/// Mirrors `sass_make_map` (sass_values.cpp:151-160).
///
/// # Safety
///
/// See [`sass_make_list`].
#[no_mangle]
pub unsafe extern "C" fn sass_make_map(len: usize) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        // SAFETY: fresh zeroed array of exactly `len` pairs (see above).
        let pairs = malloc_or_abort(len.wrapping_mul(size_of::<SassMapPair>())) as *mut SassMapPair;
        unsafe {
            ptr::write_bytes(pairs, 0, len);
        }
        let v = alloc_value();
        // SAFETY: fresh allocation.
        unsafe {
            (*v).map = SassMap {
                tag: SASS_MAP,
                length: len,
                pairs,
            };
        }
        v
    })
}

/// Creates an error value, copying `msg`. NULL fails the make.
///
/// Mirrors `sass_make_error` (sass_values.cpp:170-178).
///
/// # Safety
///
/// `msg` must be NULL or a valid NUL-terminated string for the call. The
/// result is owned by the caller.
#[no_mangle]
pub unsafe extern "C" fn sass_make_error(msg: *const c_char) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        // SAFETY: NULL/validity per contract above.
        let owned = unsafe { copy_or_null(msg) };
        if owned.is_null() {
            return ptr::null_mut();
        }
        let v = alloc_value();
        // SAFETY: fresh allocation.
        unsafe {
            (*v).error = SassError {
                tag: SASS_ERROR,
                message: owned,
            };
        }
        v
    })
}

/// Creates a warning value, copying `msg`. NULL fails the make.
///
/// Mirrors `sass_make_warning` (sass_values.cpp:180-188).
///
/// # Safety
///
/// See [`sass_make_error`].
#[no_mangle]
pub unsafe extern "C" fn sass_make_warning(msg: *const c_char) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        // SAFETY: NULL/validity per contract above.
        let owned = unsafe { copy_or_null(msg) };
        if owned.is_null() {
            return ptr::null_mut();
        }
        let v = alloc_value();
        // SAFETY: fresh allocation.
        unsafe {
            (*v).warning = SassError {
                tag: SASS_WARNING,
                message: owned,
            };
        }
        v
    })
}

/// Frees a value recursively (lists/maps free entries first). NULL-safe.
///
/// Mirrors `sass_delete_value` (sass_values.cpp:191-232).
///
/// # Safety
///
/// `val` must be NULL or a live value, freed exactly once here.
#[no_mangle]
pub unsafe extern "C" fn sass_delete_value(val: *mut SassValue) {
    guard((), || {
        // SAFETY: NULL/live per contract above.
        unsafe { delete_inner(val) };
    });
}

/// Deep-clones a value (NULL in → NULL out). String clones preserve the
/// quoted flag.
///
/// Mirrors `sass_clone_value` (sass_values.cpp:235-282).
///
/// # Safety
///
/// `val` must be NULL or a live value for the call. The result is owned by
/// the caller.
#[no_mangle]
pub unsafe extern "C" fn sass_clone_value(val: *const SassValue) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        // SAFETY: NULL/live per contract above.
        unsafe { clone_inner(val) }
    })
}

/// Executes an operation (stub until engine bridging lands in plan §7 step 6:
/// returns a `sass_make_error` value, never NULL — the failure idiom
/// `sass_value_op` itself uses for exceptional results).
///
/// # Safety
///
/// All pointers must be NULL or live values for the call. The result is owned
/// by the caller.
#[no_mangle]
pub unsafe extern "C" fn sass_value_op(
    _op: u32,
    _a: *const SassValue,
    _b: *const SassValue,
) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        let msg = c"operation requires the evaluation engine".as_ptr();
        // SAFETY: static NUL-terminated literal.
        unsafe { sass_make_error(msg) }
    })
}

/// Stringifies a value (stub until engine bridging lands: returns a
/// `sass_make_error` value, never NULL — same idiom as above).
///
/// # Safety
///
/// `v` must be NULL or a live value for the call. The result is owned by the
/// caller.
#[no_mangle]
pub unsafe extern "C" fn sass_value_stringify(
    _v: *const SassValue,
    _compressed: bool,
    _precision: c_int,
) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        let msg = c"stringify requires the evaluation engine".as_ptr();
        // SAFETY: static NUL-terminated literal.
        unsafe { sass_make_error(msg) }
    })
}

/// Returns the value's tag. NULL hardens to `u32::MAX` (upstream
/// unchecked-derefs — the contract never passes NULL).
///
/// Mirrors `sass_value_get_tag` (sass_values.cpp:17).
///
/// # Safety
///
/// `v` must be NULL or a live value for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_value_get_tag(v: *const SassValue) -> u32 {
    guard(u32::MAX, || {
        // SAFETY: NULL/live per contract above.
        unsafe { tag_of(v) }
    })
}

macro_rules! predicate {
    ($name:ident, $tag:ident, $safety:expr) => {
        /// Tag predicate; NULL hardens to false.
        ///
        /// # Safety
        ///
        #[doc = $safety]
        #[no_mangle]
        pub unsafe extern "C" fn $name(v: *const SassValue) -> bool {
            guard(false, || {
                // SAFETY: NULL/live per contract below.
                unsafe { tag_of(v) == $tag }
            })
        }
    };
}

predicate!(
    sass_value_is_null,
    SASS_NULL,
    "`v` must be NULL or a live value for the call."
);
predicate!(
    sass_value_is_number,
    SASS_NUMBER,
    "`v` must be NULL or a live value for the call."
);
predicate!(
    sass_value_is_string,
    SASS_STRING,
    "`v` must be NULL or a live value for the call."
);
predicate!(
    sass_value_is_boolean,
    SASS_BOOLEAN,
    "`v` must be NULL or a live value for the call."
);
predicate!(
    sass_value_is_color,
    SASS_COLOR,
    "`v` must be NULL or a live value for the call."
);
predicate!(
    sass_value_is_list,
    SASS_LIST,
    "`v` must be NULL or a live value for the call."
);
predicate!(
    sass_value_is_map,
    SASS_MAP,
    "`v` must be NULL or a live value for the call."
);
predicate!(
    sass_value_is_error,
    SASS_ERROR,
    "`v` must be NULL or a live value for the call."
);
predicate!(
    sass_value_is_warning,
    SASS_WARNING,
    "`v` must be NULL or a live value for the call."
);

/// Reads a live number value (NULL hardens to a zeroed arm — upstream
/// unchecked-derefs; the contract never passes NULL).
///
/// # Safety
///
/// `v` must be NULL or a live number value for the call.
macro_rules! number_scalar {
    ($get:ident, $set:ident, $field:ident, $ty:ty, $safety:expr) => {
        /// Reads the scalar; NULL hardens to the type default.
        ///
        /// # Safety
        ///
        #[doc = $safety]
        #[no_mangle]
        pub unsafe extern "C" fn $get(v: *const SassValue) -> $ty {
            guard(<$ty>::default(), || {
                if v.is_null() {
                    return <$ty>::default();
                }
                // SAFETY: live number per contract below.
                unsafe { (*v).number.$field }
            })
        }

        /// Writes the scalar; NULL is a safe no-op.
        ///
        /// # Safety
        ///
        #[doc = $safety]
        #[no_mangle]
        pub unsafe extern "C" fn $set(v: *mut SassValue, value: $ty) {
            guard((), || {
                if v.is_null() {
                    return;
                }
                // SAFETY: live number per contract below.
                unsafe {
                    (*v).number.$field = value;
                }
            })
        }
    };
}

number_scalar!(
    sass_number_get_value,
    sass_number_set_value,
    value,
    c_double,
    "`v` must be NULL or a live number value for the call."
);

/// Reads the unit (borrowed; NULL value hardens to NULL).
///
/// Mirrors `sass_number_get_unit` (sass_values.cpp:33).
///
/// # Safety
///
/// `v` must be NULL or a live number value for the call. Borrowed; freed
/// with the value.
#[no_mangle]
pub unsafe extern "C" fn sass_number_get_unit(v: *const SassValue) -> *const c_char {
    guard(ptr::null(), || {
        if v.is_null() {
            return ptr::null();
        }
        // SAFETY: live number per contract above.
        unsafe { (*v).number.unit as *const c_char }
    })
}

/// Stores `unit`, taking ownership (no copy — node-sass passes fresh
/// buffers). Frees the previously owned unit first (hardening over
/// upstream's overwriting store, sass_values.cpp:34).
///
/// # Safety
///
/// `v` must be NULL or a live number value (NULL-safe no-op); `unit` must be
/// NULL or a malloc'd NUL-terminated buffer whose ownership transfers.
#[no_mangle]
pub unsafe extern "C" fn sass_number_set_unit(v: *mut SassValue, unit: *mut c_char) {
    guard((), || {
        if v.is_null() {
            // Leaks `unit` like any unconsumed transfer on a NULL receiver;
            // mirrors the NULL-receiver no-op convention used throughout.
            return;
        }
        // SAFETY: live number per contract above.
        unsafe {
            free_string((*v).number.unit);
            (*v).number.unit = unit;
        }
    })
}

/// Reads the string payload (borrowed; NULL value hardens to NULL).
///
/// Mirrors `sass_string_get_value` (sass_values.cpp:37).
///
/// # Safety
///
/// `v` must be NULL or a live string value for the call. Borrowed; freed
/// with the value.
#[no_mangle]
pub unsafe extern "C" fn sass_string_get_value(v: *const SassValue) -> *const c_char {
    guard(ptr::null(), || {
        if v.is_null() {
            return ptr::null();
        }
        // SAFETY: live string per contract above.
        unsafe { (*v).string.value as *const c_char }
    })
}

/// Stores `value`, taking ownership (no copy). Frees the previously owned
/// payload first (hardening over sass_values.cpp:38).
///
/// # Safety
///
/// `v` must be NULL or a live string value (NULL-safe no-op); `value` must
/// be NULL or a malloc'd NUL-terminated buffer whose ownership transfers.
#[no_mangle]
pub unsafe extern "C" fn sass_string_set_value(v: *mut SassValue, value: *mut c_char) {
    guard((), || {
        if v.is_null() {
            return;
        }
        // SAFETY: live string per contract above.
        unsafe {
            free_string((*v).string.value);
            (*v).string.value = value;
        }
    })
}

/// Reads the quoted flag (NULL hardens to false).
///
/// Mirrors `sass_string_is_quoted` (sass_values.cpp:39).
///
/// # Safety
///
/// `v` must be NULL or a live string value for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_string_is_quoted(v: *const SassValue) -> bool {
    guard(false, || {
        if v.is_null() {
            return false;
        }
        // SAFETY: live string per contract above.
        unsafe { (*v).string.quoted }
    })
}

/// Writes the quoted flag (NULL-safe no-op).
///
/// Mirrors `sass_string_set_quoted` (sass_values.cpp:40).
///
/// # Safety
///
/// `v` must be NULL or a live string value for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_string_set_quoted(v: *mut SassValue, quoted: bool) {
    guard((), || {
        if v.is_null() {
            return;
        }
        // SAFETY: live string per contract above.
        unsafe {
            (*v).string.quoted = quoted;
        }
    })
}

macro_rules! boolean_scalar {
    ($get:ident, $set:ident, $safety:expr) => {
        /// Reads the boolean; NULL hardens to false.
        ///
        /// # Safety
        ///
        #[doc = $safety]
        #[no_mangle]
        pub unsafe extern "C" fn $get(v: *const SassValue) -> bool {
            guard(false, || {
                if v.is_null() {
                    return false;
                }
                // SAFETY: live boolean per contract below.
                unsafe { (*v).boolean.value }
            })
        }

        /// Writes the boolean; NULL-safe no-op.
        ///
        /// # Safety
        ///
        #[doc = $safety]
        #[no_mangle]
        pub unsafe extern "C" fn $set(v: *mut SassValue, value: bool) {
            guard((), || {
                if v.is_null() {
                    return;
                }
                // SAFETY: live boolean per contract below.
                unsafe {
                    (*v).boolean.value = value;
                }
            })
        }
    };
}

boolean_scalar!(
    sass_boolean_get_value,
    sass_boolean_set_value,
    "`v` must be NULL or a live boolean value for the call."
);

macro_rules! color_channel {
    ($get:ident, $set:ident, $field:ident, $safety:expr) => {
        /// Reads the channel; NULL hardens to 0.0.
        ///
        /// # Safety
        ///
        #[doc = $safety]
        #[no_mangle]
        pub unsafe extern "C" fn $get(v: *const SassValue) -> c_double {
            guard(0.0, || {
                if v.is_null() {
                    return 0.0;
                }
                // SAFETY: live color per contract below.
                unsafe { (*v).color.$field }
            })
        }

        /// Writes the channel; NULL-safe no-op.
        ///
        /// # Safety
        ///
        #[doc = $safety]
        #[no_mangle]
        pub unsafe extern "C" fn $set(v: *mut SassValue, value: c_double) {
            guard((), || {
                if v.is_null() {
                    return;
                }
                // SAFETY: live color per contract below.
                unsafe {
                    (*v).color.$field = value;
                }
            })
        }
    };
}

color_channel!(
    sass_color_get_r,
    sass_color_set_r,
    r,
    "`v` must be NULL or a live color value for the call."
);
color_channel!(
    sass_color_get_g,
    sass_color_set_g,
    g,
    "`v` must be NULL or a live color value for the call."
);
color_channel!(
    sass_color_get_b,
    sass_color_set_b,
    b,
    "`v` must be NULL or a live color value for the call."
);
color_channel!(
    sass_color_get_a,
    sass_color_set_a,
    a,
    "`v` must be NULL or a live color value for the call."
);

/// Reads the list length (NULL hardens to 0).
///
/// Mirrors `sass_list_get_length` (sass_values.cpp:57).
///
/// # Safety
///
/// `v` must be NULL or a live list value for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_list_get_length(v: *const SassValue) -> usize {
    guard(0, || {
        if v.is_null() {
            return 0;
        }
        // SAFETY: live list per contract above.
        unsafe { (*v).list.length }
    })
}

/// Reads the separator (NULL hardens to `SASS_COMMA`).
///
/// Mirrors `sass_list_get_separator` (sass_values.cpp:58).
///
/// # Safety
///
/// `v` must be NULL or a live list value for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_list_get_separator(v: *const SassValue) -> u32 {
    guard(SASS_COMMA, || {
        if v.is_null() {
            return SASS_COMMA;
        }
        // SAFETY: live list per contract above.
        unsafe { (*v).list.separator }
    })
}

/// Writes the separator (NULL-safe no-op).
///
/// Mirrors `sass_list_set_separator` (sass_values.cpp:59).
///
/// # Safety
///
/// `v` must be NULL or a live list value for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_list_set_separator(v: *mut SassValue, separator: u32) {
    guard((), || {
        if v.is_null() {
            return;
        }
        // SAFETY: live list per contract above.
        unsafe {
            (*v).list.separator = separator;
        }
    })
}

/// Reads the bracketed flag (NULL hardens to false).
///
/// Mirrors `sass_list_get_is_bracketed` (sass_values.cpp:60).
///
/// # Safety
///
/// `v` must be NULL or a live list value for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_list_get_is_bracketed(v: *const SassValue) -> bool {
    guard(false, || {
        if v.is_null() {
            return false;
        }
        // SAFETY: live list per contract above.
        unsafe { (*v).list.is_bracketed }
    })
}

/// Writes the bracketed flag (NULL-safe no-op).
///
/// Mirrors `sass_list_set_is_bracketed` (sass_values.cpp:61).
///
/// # Safety
///
/// `v` must be NULL or a live list value for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_list_set_is_bracketed(v: *mut SassValue, is_bracketed: bool) {
    guard((), || {
        if v.is_null() {
            return;
        }
        // SAFETY: live list per contract above.
        unsafe {
            (*v).list.is_bracketed = is_bracketed;
        }
    })
}

/// Reads the entry at `i` (borrowed). Out-of-range or NULL hardens to NULL
/// (upstream unchecked-derefs).
///
/// Mirrors `sass_list_get_value` (sass_values.cpp:63).
///
/// # Safety
///
/// `v` must be NULL or a live list value for the call. Borrowed; freed with
/// the list (never delete entries separately).
#[no_mangle]
pub unsafe extern "C" fn sass_list_get_value(v: *const SassValue, i: usize) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        if v.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live list per contract above; bounds checked (hardening).
        unsafe {
            if i >= (*v).list.length || (*v).list.values.is_null() {
                return ptr::null_mut();
            }
            *(*v).list.values.add(i)
        }
    })
}

/// Stores `value` at `i`, taking ownership (freeing any previously stored
/// entry first — hardening over sass_values.cpp:64). Out-of-range or NULL
/// list hardens to a no-op (leaking `value` like any unconsumed transfer on
/// a failed store; callers must pass in-range indexes).
///
/// # Safety
///
/// `v` must be NULL or a live list value; `i` must be in range;
/// `value` must be NULL or a live value whose ownership transfers.
#[no_mangle]
pub unsafe extern "C" fn sass_list_set_value(v: *mut SassValue, i: usize, value: *mut SassValue) {
    guard((), || {
        if v.is_null() {
            return;
        }
        // SAFETY: live list per contract above; bounds checked (hardening).
        unsafe {
            if i >= (*v).list.length || (*v).list.values.is_null() {
                return;
            }
            let slot = (*v).list.values.add(i);
            delete_inner(*slot);
            *slot = value;
        }
    })
}

/// Reads the map length (NULL hardens to 0).
///
/// Mirrors `sass_map_get_length` (sass_values.cpp:67).
///
/// # Safety
///
/// `v` must be NULL or a live map value for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_map_get_length(v: *const SassValue) -> usize {
    guard(0, || {
        if v.is_null() {
            return 0;
        }
        // SAFETY: live map per contract above.
        unsafe { (*v).map.length }
    })
}

/// Reads the key at `i` (borrowed). Out-of-range or NULL hardens to NULL.
///
/// Mirrors `sass_map_get_key` (sass_values.cpp:69).
///
/// # Safety
///
/// `v` must be NULL or a live map value for the call. Borrowed; freed with
/// the map.
#[no_mangle]
pub unsafe extern "C" fn sass_map_get_key(v: *const SassValue, i: usize) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        if v.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live map per contract above; bounds checked (hardening).
        unsafe {
            if i >= (*v).map.length || (*v).map.pairs.is_null() {
                return ptr::null_mut();
            }
            (*(*v).map.pairs.add(i)).key
        }
    })
}

/// Reads the value at `i` (borrowed). Out-of-range or NULL hardens to NULL.
///
/// Mirrors `sass_map_get_value` (sass_values.cpp:70).
///
/// # Safety
///
/// See [`sass_map_get_key`].
#[no_mangle]
pub unsafe extern "C" fn sass_map_get_value(v: *const SassValue, i: usize) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        if v.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live map per contract above; bounds checked (hardening).
        unsafe {
            if i >= (*v).map.length || (*v).map.pairs.is_null() {
                return ptr::null_mut();
            }
            (*(*v).map.pairs.add(i)).value
        }
    })
}

/// Stores `key` at `i`, taking ownership (freeing any previously stored key
/// first — hardening over sass_values.cpp:71). Out-of-range hardens to a
/// no-op.
///
/// # Safety
///
/// `v` must be NULL or a live map value; `i` must be in range; `key` must be
/// NULL or a live value whose ownership transfers.
#[no_mangle]
pub unsafe extern "C" fn sass_map_set_key(v: *mut SassValue, i: usize, key: *mut SassValue) {
    guard((), || {
        if v.is_null() {
            return;
        }
        // SAFETY: live map per contract above; bounds checked (hardening).
        unsafe {
            if i >= (*v).map.length || (*v).map.pairs.is_null() {
                return;
            }
            let slot = &mut (*(*v).map.pairs.add(i)).key;
            delete_inner(*slot);
            *slot = key;
        }
    })
}

/// Stores `value` at `i`, taking ownership (freeing any previously stored
/// value first — hardening over sass_values.cpp:72). Out-of-range hardens to
/// a no-op.
///
/// # Safety
///
/// See [`sass_map_set_key`] (for the value slot).
#[no_mangle]
pub unsafe extern "C" fn sass_map_set_value(v: *mut SassValue, i: usize, value: *mut SassValue) {
    guard((), || {
        if v.is_null() {
            return;
        }
        // SAFETY: live map per contract above; bounds checked (hardening).
        unsafe {
            if i >= (*v).map.length || (*v).map.pairs.is_null() {
                return;
            }
            let slot = &mut (*(*v).map.pairs.add(i)).value;
            delete_inner(*slot);
            *slot = value;
        }
    })
}

/// Reads the error message (borrowed; NULL value hardens to NULL).
///
/// Mirrors `sass_error_get_message` (sass_values.cpp:75).
///
/// # Safety
///
/// `v` must be NULL or a live error value for the call. Borrowed; freed
/// with the value.
#[no_mangle]
pub unsafe extern "C" fn sass_error_get_message(v: *const SassValue) -> *mut c_char {
    guard(ptr::null_mut(), || {
        if v.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live error per contract above.
        unsafe { (*v).error.message }
    })
}

/// Stores `msg`, taking ownership (no copy). Frees the previously owned
/// message first (hardening over sass_values.cpp:76).
///
/// # Safety
///
/// `v` must be NULL or a live error value (NULL-safe no-op); `msg` must be
/// NULL or a malloc'd NUL-terminated buffer whose ownership transfers.
#[no_mangle]
pub unsafe extern "C" fn sass_error_set_message(v: *mut SassValue, msg: *mut c_char) {
    guard((), || {
        if v.is_null() {
            return;
        }
        // SAFETY: live error per contract above.
        unsafe {
            free_string((*v).error.message);
            (*v).error.message = msg;
        }
    })
}

/// Reads the warning message (borrowed; NULL value hardens to NULL).
///
/// Mirrors `sass_warning_get_message` (sass_values.cpp:79).
///
/// # Safety
///
/// `v` must be NULL or a live warning value for the call. Borrowed; freed
/// with the value.
#[no_mangle]
pub unsafe extern "C" fn sass_warning_get_message(v: *const SassValue) -> *mut c_char {
    guard(ptr::null_mut(), || {
        if v.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live warning per contract above.
        unsafe { (*v).warning.message }
    })
}

/// Stores `msg`, taking ownership (no copy). Frees the previously owned
/// message first (hardening over sass_values.cpp:80).
///
/// # Safety
///
/// `v` must be NULL or a live warning value (NULL-safe no-op); `msg` must be
/// NULL or a malloc'd NUL-terminated buffer whose ownership transfers.
#[no_mangle]
pub unsafe extern "C" fn sass_warning_set_message(v: *mut SassValue, msg: *mut c_char) {
    guard((), || {
        if v.is_null() {
            return;
        }
        // SAFETY: live warning per contract above.
        unsafe {
            free_string((*v).warning.message);
            (*v).warning.message = msg;
        }
    })
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

    unsafe fn read_opt(ptr: *const c_char) -> Option<Vec<u8>> {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: test-only; non-null pointers are valid strings produced by
        // the functions under test.
        Some(unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec())
    }

    #[test]
    fn union_layout_matches_upstream() {
        // Field-for-field layout check against sass_values.hpp: every arm
        // starts with the tag, and the union spans the largest arm (color).
        assert_eq!(size_of::<SassValue>(), size_of::<SassColor>());
        assert_eq!(size_of::<SassColor>(), 40);
        assert_eq!(size_of::<SassNumber>(), 24);
        assert_eq!(size_of::<SassString>(), 16);
        assert_eq!(size_of::<SassList>(), 32);
        assert_eq!(size_of::<SassMap>(), 24);
        assert_eq!(size_of::<SassError>(), 16);
        assert_eq!(align_of::<SassValue>(), 8);
        // Tag offsets are zero in every arm.
        assert_eq!(std::mem::offset_of!(SassBoolean, tag), 0);
        assert_eq!(std::mem::offset_of!(SassNumber, tag), 0);
        assert_eq!(std::mem::offset_of!(SassColor, tag), 0);
        assert_eq!(std::mem::offset_of!(SassString, tag), 0);
        assert_eq!(std::mem::offset_of!(SassList, tag), 0);
        assert_eq!(std::mem::offset_of!(SassMap, tag), 0);
        assert_eq!(std::mem::offset_of!(SassError, tag), 0);
    }

    #[test]
    fn null_maker_and_predicates() {
        unsafe {
            let n = sass_make_null();
            assert_eq!(sass_value_get_tag(n), SASS_NULL);
            assert!(sass_value_is_null(n));
            assert!(!sass_value_is_number(n));
            sass_delete_value(n);
            // NULL hardens everywhere (upstream would crash).
            assert_eq!(sass_value_get_tag(ptr::null()), u32::MAX);
            assert!(!sass_value_is_null(ptr::null()));
            assert!(sass_list_get_value(ptr::null_mut(), 0).is_null());
            assert!(sass_map_get_key(ptr::null_mut(), 0).is_null());
            assert_eq!(sass_list_get_length(ptr::null()), 0);
            sass_delete_value(ptr::null_mut());
        }
    }

    #[test]
    fn number_unit_and_value() {
        unsafe {
            let px = CString::new("px").unwrap();
            let n = sass_make_number(2.5, px.as_ptr());
            assert_eq!(sass_number_get_value(n), 2.5);
            assert_eq!(read_opt(sass_number_get_unit(n)), Some(b"px".to_vec()));
            sass_number_set_value(n, 1.0);
            assert_eq!(sass_number_get_value(n), 1.0);
            // Setter takes ownership of a fresh buffer, freeing the old unit.
            let pct = copy_bytes_nul(b"%");
            sass_number_set_unit(n, pct);
            assert_eq!(read_opt(sass_number_get_unit(n)), Some(b"%".to_vec()));
            // NULL unit fails the make.
            assert!(sass_make_number(1.0, ptr::null()).is_null());
            sass_delete_value(n);
        }
    }

    #[test]
    fn string_quoted_flag_and_payload() {
        unsafe {
            let s = CString::new("a").unwrap();
            let plain = sass_make_string(s.as_ptr());
            let quoted = sass_make_qstring(s.as_ptr());
            assert!(!sass_string_is_quoted(plain));
            assert!(sass_string_is_quoted(quoted));
            sass_string_set_quoted(plain, true);
            assert!(sass_string_is_quoted(plain));
            let b = copy_bytes_nul(b"b");
            sass_string_set_value(plain, b);
            assert_eq!(read_opt(sass_string_get_value(plain)), Some(b"b".to_vec()));
            assert!(sass_make_string(ptr::null()).is_null());
            sass_delete_value(plain);
            sass_delete_value(quoted);
        }
    }

    #[test]
    fn list_map_index_hardening() {
        unsafe {
            let list = sass_make_list(1, SASS_SPACE, false);
            // Out-of-range reads harden to NULL (upstream: UB).
            assert!(sass_list_get_value(list, 7).is_null());
            // Out-of-range stores are no-ops (must not corrupt slot 0).
            sass_list_set_value(list, 7, sass_make_null());
            assert!(sass_list_get_value(list, 0).is_null());
            // Overwriting a slot frees the old entry (no leak, no crash).
            sass_list_set_value(list, 0, sass_make_boolean(true));
            sass_list_set_value(list, 0, sass_make_boolean(false));
            assert!(!sass_boolean_get_value(sass_list_get_value(list, 0)));
            let map = sass_make_map(1);
            assert!(sass_map_get_key(map, 3).is_null());
            assert!(sass_map_get_value(map, 3).is_null());
            sass_map_set_key(map, 3, sass_make_null());
            sass_map_set_value(map, 3, sass_make_null());
            sass_delete_value(list);
            sass_delete_value(map);
        }
    }

    #[test]
    fn error_message_takeover() {
        unsafe {
            let e = sass_make_error(CString::new("x").unwrap().as_ptr());
            let owned = copy_bytes_nul(b"y");
            sass_error_set_message(e, owned);
            assert_eq!(read_c_str(sass_error_get_message(e)), b"y");
            assert!(sass_make_error(ptr::null()).is_null());
            let w = sass_make_warning(CString::new("z").unwrap().as_ptr());
            assert_eq!(read_c_str(sass_warning_get_message(w)), b"z");
            sass_delete_value(e);
            sass_delete_value(w);
        }
    }

    #[test]
    fn clone_is_deep_and_op_stub_errors() {
        unsafe {
            let inner = sass_make_list(1, SASS_COMMA, false);
            sass_list_set_value(inner, 0, sass_make_boolean(true));
            let dup = sass_clone_value(inner);
            sass_boolean_set_value(sass_list_get_value(inner, 0), false);
            assert!(sass_boolean_get_value(sass_list_get_value(dup, 0)));
            // Stubs return error values, never NULL (libsass failure idiom).
            let px = CString::new("px").unwrap();
            let a = sass_make_number(1.0, px.as_ptr());
            let b = sass_make_number(2.0, px.as_ptr());
            let op = sass_value_op(SASS_OP_ADD, a, b);
            assert!(!op.is_null());
            assert_eq!(sass_value_get_tag(op), SASS_ERROR);
            let strung = sass_value_stringify(a, false, 5);
            assert!(!strung.is_null());
            assert_eq!(sass_value_get_tag(strung), SASS_ERROR);
            sass_delete_value(inner);
            sass_delete_value(dup);
            sass_delete_value(a);
            sass_delete_value(b);
            sass_delete_value(op);
            sass_delete_value(strung);
        }
    }
}
