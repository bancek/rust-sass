// Copyright 2012-2016 Sass Open Source Foundation. Use of this source code
// is governed by an MIT-style license that can be found in the LICENSE
// file or at https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// libsass-source: src/sass_context.cpp (option surface) + include/sass/context.h

//! Compile options: the `Sass_Options` heap struct with all getters/setters.
//!
//! Mirrors `sass/context.h` option functions: `sass_make_options`,
//! `sass_delete_options`, every `sass_option_get/set_*`, the `push_*_path`
//! lists with indexed getters, and the `c_functions/c_importers/c_headers`
//! registration slots (`c_functions` bridged in [`crate::functions`],
//! `c_importers` in [`crate::importers`], `c_headers` as an opaque stub).
//!
//! # Ownership (mirrors libsass, plan §A.2–A.3 with hardening)
//!
//! - `Sass_Options` is an opaque heap box. `sass_make_options` allocates it
//!   (calloc-zero + defaults); `sass_delete_options` frees it and everything
//!   it owns; NULL-safe on delete.
//! - Copied strings (`input/output/include/plugin/source_map_file/root`):
//!   setters free the old value and store a fresh copy; NULL clears.
//! - Borrowed strings (`indent`/`linefeed`): setters store the pointer only
//   (plan G1/G19) — the caller must keep the buffer alive — and NULL resets
//   to the default literal. Getters return the stored pointer, or the default
//   when unset (hardening over upstream's raw pointer, which would return
//   NULL or dangle).
//! - Path lists (`include_paths`/`plugin_paths`): pushed strings are copied;
//!   indexed getters return NULL out of range (hardening over upstream's
//!   unchecked walk — contract pins in-range behavior identically).
//! - `c_*` lists: stored opaquely (pointer + length bookkeeping is the
//!   consumer's until bridging lands); getters return the stored pointers.

use crate::functions::free_function_list;
use crate::functions::FunctionEntry;
use crate::importers::free_importer_list;
use crate::importers::ImporterEntry;
use std::ffi::{c_char, c_int, CStr};
use std::ptr;

use crate::base::{copy_bytes_nul, guard};

/// C ABI style constants, matching `Sass_Output_Style` numeric order
/// (`include/sass/base.h:64-73`; plan G8). The low two map to our expanded
/// output, the high one to compressed (plan D5).
pub const SASS_STYLE_NESTED: u32 = 0;
pub const SASS_STYLE_EXPANDED: u32 = 1;
pub const SASS_STYLE_COMPACT: u32 = 2;
pub const SASS_STYLE_COMPRESSED: u32 = 3;

/// Library-side precision default (plan D4: stored, returned, never applied).
const DEFAULT_PRECISION: c_int = 10;
/// Default indent/linefeed literals (plan G1/G19).
const DEFAULT_INDENT: &[u8] = b"  \0";
const DEFAULT_LINEFEED: &[u8] = b"\n\0";

/// Owned nullable C string with libsass setter semantics: `set` frees the old
/// value and stores a fresh copy (NULL clears); `get` returns the stored
/// pointer or NULL.
pub(crate) struct OwnedCString {
    ptr: *mut c_char,
}

impl OwnedCString {
    pub(crate) fn new() -> Self {
        OwnedCString {
            ptr: ptr::null_mut(),
        }
    }

    /// Stores a copy of `s` (NULL clears). Mirrors
    /// `IMPLEMENT_SASS_OPTION_STRING_SETTER` (sass_context.cpp:231-233).
    ///
    /// # Safety
    ///
    /// `s` must be NULL or a valid NUL-terminated string for the call.
    pub(crate) unsafe fn set(&mut self, s: *const c_char) {
        if !self.ptr.is_null() {
            // SAFETY: non-null heap allocation owned by this struct.
            unsafe {
                libc::free(self.ptr as *mut libc::c_void);
            }
            self.ptr = ptr::null_mut();
        }
        if !s.is_null() {
            // SAFETY: non-null NUL-terminated string per caller contract.
            let bytes = unsafe { CStr::from_ptr(s) }.to_bytes();
            self.ptr = copy_bytes_nul(bytes);
        }
    }

    fn get(&self) -> *const c_char {
        self.ptr as *const c_char
    }

    /// Takes ownership out (leaving NULL behind) — the `take_*` / move
    /// primitive for `set_options`-style transfers.
    pub(crate) fn take(&mut self) -> *mut c_char {
        std::mem::replace(&mut self.ptr, ptr::null_mut())
    }

    /// Installs an already-owned pointer (NULL clears), freeing the old value
    /// first — the receiving half of a move.
    pub(crate) fn put(&mut self, owned: *mut c_char) {
        if !self.ptr.is_null() {
            // SAFETY: non-null heap allocation owned by this struct.
            unsafe {
                libc::free(self.ptr as *mut libc::c_void);
            }
        }
        self.ptr = owned;
    }

    /// Reads the stored bytes, if any.
    ///
    /// # Safety
    ///
    /// The struct must be live; the returned slice borrows it.
    pub(crate) unsafe fn bytes(&self) -> Option<&[u8]> {
        if self.ptr.is_null() {
            return None;
        }
        // SAFETY: non-null NUL-terminated string owned by this live struct.
        Some(unsafe { CStr::from_ptr(self.ptr) }.to_bytes())
    }
}

impl Drop for OwnedCString {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: owned allocation, freed exactly once here.
            unsafe {
                libc::free(self.ptr as *mut libc::c_void);
            }
        }
    }
}

/// One pushed path entry. The string is always an owned copy (NULL pushes
/// store NULL, mirroring upstream's `path ? copy : 0`).
pub(crate) struct PathEntry {
    pub(crate) ptr: *mut c_char,
}

impl Drop for PathEntry {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: owned allocation, freed exactly once here.
            unsafe {
                libc::free(self.ptr as *mut libc::c_void);
            }
        }
    }
}

/// The `Sass_Options` heap struct. Opaque to C consumers (bindgen sees only a
/// zero-sized type); all access goes through the functions below. Fields are
/// `pub(crate)` so `context.rs` can embed/move the whole struct (mirroring
/// libsass's `Sass_Context : Sass_Options` inheritance + `copy_options`
/// move semantics, plan G2).
pub struct OptionsBox {
    pub(crate) precision: c_int,
    pub(crate) output_style: u32,
    pub(crate) source_comments: bool,
    pub(crate) source_map_embed: bool,
    pub(crate) source_map_contents: bool,
    pub(crate) source_map_file_urls: bool,
    pub(crate) omit_source_map_url: bool,
    pub(crate) is_indented_syntax_src: bool,
    /// Borrowed per G1/G19; NULL means "default literal".
    pub(crate) indent: *const c_char,
    pub(crate) linefeed: *const c_char,
    pub(crate) input_path: OwnedCString,
    pub(crate) output_path: OwnedCString,
    pub(crate) include_path: OwnedCString,
    pub(crate) plugin_path: OwnedCString,
    pub(crate) source_map_file: OwnedCString,
    pub(crate) source_map_root: OwnedCString,
    pub(crate) include_paths: Vec<PathEntry>,
    pub(crate) plugin_paths: Vec<PathEntry>,
    /// Owned custom-function list (NULL-terminated `FunctionEntry` array).
    /// Ownership transfers at `set_c_functions` (mirroring upstream, which
    /// frees lists+entries on clear): freed on drop, overwrite, and
    /// move-out; never the cookies (consumer-owned, like upstream's
    /// `sass_delete_function`).
    pub(crate) c_functions: *mut *mut FunctionEntry,
    /// Owned custom-importer list (NULL-terminated `ImporterEntry` array).
    /// Same takeover as `c_functions` (plan §7 step 7 — closes the v0
    /// consumer-owned gap). `c_headers` stays opaque + consumer-owned (D15).
    pub(crate) c_importers: *mut *mut ImporterEntry,
    pub(crate) c_headers: *mut libc::c_void,
}

impl Drop for OptionsBox {
    fn drop(&mut self) {
        // SAFETY: both lists are NULL or live NULL-terminated lists per the
        // setter/move contracts; freeing here mirrors upstream's
        // `sass_clear_options` (entries + array, never cookies).
        unsafe {
            free_function_list(self.c_functions);
            free_importer_list(self.c_importers);
        }
    }
}

impl OptionsBox {
    pub(crate) fn new() -> Self {
        OptionsBox {
            precision: DEFAULT_PRECISION,
            output_style: SASS_STYLE_NESTED,
            source_comments: false,
            source_map_embed: false,
            source_map_contents: false,
            source_map_file_urls: false,
            omit_source_map_url: false,
            is_indented_syntax_src: false,
            indent: ptr::null(),
            linefeed: ptr::null(),
            input_path: OwnedCString::new(),
            output_path: OwnedCString::new(),
            include_path: OwnedCString::new(),
            plugin_path: OwnedCString::new(),
            source_map_file: OwnedCString::new(),
            source_map_root: OwnedCString::new(),
            include_paths: Vec::new(),
            plugin_paths: Vec::new(),
            c_functions: ptr::null_mut(),
            c_importers: ptr::null_mut(),
            c_headers: ptr::null_mut(),
        }
    }
}

/// Creates and initializes an option struct (calloc-zero + defaults:
/// precision 10, NESTED style, `"  "`/`"\n"`, everything else zero).
///
/// Mirrors `sass_make_options` + `init_options` (sass_context.cpp:330-352).
/// The caller owns the result (frees with [`sass_delete_options`]).
///
/// # Safety
///
/// Always safe to call (takes no pointers); the returned pointer must be
/// freed exactly once with [`sass_delete_options`].
#[no_mangle]
pub unsafe extern "C" fn sass_make_options() -> *mut OptionsBox {
    guard(ptr::null_mut(), || {
        // SAFETY: fresh Box allocation; ownership moves to the caller.
        Box::into_raw(Box::new(OptionsBox::new()))
    })
}

/// Releases all memory owned by `options` (strings, path lists) and itself.
/// NULL-safe no-op.
///
/// Mirrors `sass_delete_options` = `sass_clear_options` + free
/// (sass_context.cpp:483-534,581-583). `c_functions`/`c_importers` lists are
/// freed with their entries (never cookies); `c_headers` stays consumer-owned
/// (D15 stub — entries must still be deleted by consumers via their own
/// delete calls).
///
/// # Safety
///
/// `options` must be NULL or a live pointer from [`sass_make_options`],
/// freed exactly once.
#[no_mangle]
pub unsafe extern "C" fn sass_delete_options(options: *mut OptionsBox) {
    guard((), || {
        if options.is_null() {
            return;
        }
        // SAFETY: live heap box per contract; `Drop` frees owned strings and
        // the box itself exactly once.
        unsafe {
            drop(Box::from_raw(options));
        }
    });
}

/// Moves every option field from `from` into `to` (freeing `to`'s old owned
/// values first) and resets `from` to defaults — mirroring `copy_options` =
/// clear-target + shallow-copy + `sass_reset_options` (sass_context.cpp:215-
/// 224,465-532). `OwnedCString`s move by pointer (no re-copy); borrowed
/// indent/linefeed copy the pointer value; path lists move the whole vec
/// (no per-entry re-copy — same ownership result as upstream's node move).
pub(crate) fn move_options_into(to: &mut OptionsBox, from: &mut OptionsBox) {
    if ptr::eq(to, from) {
        return; // `copy_options` self-guard (`to == from`).
    }
    to.precision = from.precision;
    to.output_style = from.output_style;
    to.source_comments = from.source_comments;
    to.source_map_embed = from.source_map_embed;
    to.source_map_contents = from.source_map_contents;
    to.source_map_file_urls = from.source_map_file_urls;
    to.omit_source_map_url = from.omit_source_map_url;
    to.is_indented_syntax_src = from.is_indented_syntax_src;
    to.indent = std::mem::replace(&mut from.indent, ptr::null());
    to.linefeed = std::mem::replace(&mut from.linefeed, ptr::null());
    for (dst, src) in [
        (&mut to.input_path, &mut from.input_path),
        (&mut to.output_path, &mut from.output_path),
        (&mut to.include_path, &mut from.include_path),
        (&mut to.plugin_path, &mut from.plugin_path),
        (&mut to.source_map_file, &mut from.source_map_file),
        (&mut to.source_map_root, &mut from.source_map_root),
    ] {
        dst.put(src.take());
    }
    to.include_paths = std::mem::take(&mut from.include_paths);
    to.plugin_paths = std::mem::take(&mut from.plugin_paths);
    to.c_functions = std::mem::replace(&mut from.c_functions, ptr::null_mut());
    // `c_importers` moves the same way (ownership transfer, no re-copy);
    // `from` is left NULL so its Drop stays a no-op for both lists.
    to.c_importers = std::mem::replace(&mut from.c_importers, ptr::null_mut());
    to.c_headers = std::mem::replace(&mut from.c_headers, ptr::null_mut());
}

/// Reads `options`, returning None for NULL input (all entry points harden
/// NULL — upstream would crash; plan harden-don't-mirror rule).
///
/// # Safety
///
/// `options` must be NULL or a live pointer from [`sass_make_options`].
pub(crate) unsafe fn opts(options: *mut OptionsBox) -> Option<&'static mut OptionsBox> {
    if options.is_null() {
        return None;
    }
    // SAFETY: live box per contract; the borrow lasts only for the enclosing
    // extern call (no guard is held across the boundary — every getter/setter
    // body completes synchronously inside `guard`).
    Some(unsafe { &mut *options })
}

/// Reads a borrowed string setter input: NULL → None, else the bytes.
///
/// # Safety
///
/// `s` must be NULL or a valid NUL-terminated string for the call.
unsafe fn read_opt_str(s: *const c_char) -> Option<Vec<u8>> {
    if s.is_null() {
        return None;
    }
    // SAFETY: per contract above.
    Some(unsafe { CStr::from_ptr(s) }.to_bytes().to_vec())
}

macro_rules! scalar_option {
    ($get:ident, $set:ident, $field:ident, $ty:ty, $safety:expr) => {
        /// Reads the option field; NULL options harden to the type default.
        ///
        /// # Safety
        ///
        #[doc = $safety]
        #[no_mangle]
        pub unsafe extern "C" fn $get(options: *mut OptionsBox) -> $ty {
            guard(<$ty>::default(), || unsafe {
                opts(options).map(|o| o.$field).unwrap_or_default()
            })
        }

        /// Writes the option field; NULL options are a safe no-op.
        ///
        /// # Safety
        ///
        #[doc = $safety]
        #[no_mangle]
        pub unsafe extern "C" fn $set(options: *mut OptionsBox, value: $ty) {
            guard((), || {
                if let Some(o) = unsafe { opts(options) } {
                    o.$field = value;
                }
            })
        }
    };
}

scalar_option!(
    sass_option_get_precision,
    sass_option_set_precision,
    precision,
    c_int,
    "`options` must be NULL or a live pointer from `sass_make_options`."
);
scalar_option!(
    sass_option_get_output_style,
    sass_option_set_output_style,
    output_style,
    u32,
    "`options` must be NULL or a live pointer from `sass_make_options`."
);
scalar_option!(
    sass_option_get_source_comments,
    sass_option_set_source_comments,
    source_comments,
    bool,
    "`options` must be NULL or a live pointer from `sass_make_options`."
);
scalar_option!(
    sass_option_get_source_map_embed,
    sass_option_set_source_map_embed,
    source_map_embed,
    bool,
    "`options` must be NULL or a live pointer from `sass_make_options`."
);
scalar_option!(
    sass_option_get_source_map_contents,
    sass_option_set_source_map_contents,
    source_map_contents,
    bool,
    "`options` must be NULL or a live pointer from `sass_make_options`."
);
scalar_option!(
    sass_option_get_source_map_file_urls,
    sass_option_set_source_map_file_urls,
    source_map_file_urls,
    bool,
    "`options` must be NULL or a live pointer from `sass_make_options`."
);
scalar_option!(
    sass_option_get_omit_source_map_url,
    sass_option_set_omit_source_map_url,
    omit_source_map_url,
    bool,
    "`options` must be NULL or a live pointer from `sass_make_options`."
);
scalar_option!(
    sass_option_get_is_indented_syntax_src,
    sass_option_set_is_indented_syntax_src,
    is_indented_syntax_src,
    bool,
    "`options` must be NULL or a live pointer from `sass_make_options`."
);

/// Implements one borrowed indent/linefeed pair (plan G1/G19): the setter
/// stores the pointer (NULL resets to default); the getter returns it, or the
/// default literal when unset.
///
/// # Safety
///
/// Same pointer contracts as the individual functions below.
macro_rules! borrowed_str_option {
    ($get:ident, $set:ident, $field:ident, $default:ident, $safety_get:expr, $safety_set:expr) => {
        /// Reads the borrowed string, or the default literal when unset; NULL
        /// options harden to NULL.
        ///
        /// # Safety
        ///
        #[doc = $safety_get]
        #[no_mangle]
        pub unsafe extern "C" fn $get(options: *mut OptionsBox) -> *const c_char {
            guard(ptr::null(), || {
                let Some(o) = (unsafe { opts(options) }) else {
                    return ptr::null();
                };
                if o.$field.is_null() {
                    $default.as_ptr() as *const c_char
                } else {
                    o.$field
                }
            })
        }

        /// Stores the caller's pointer (NULL resets to default); NULL options
        /// are a safe no-op.
        ///
        /// # Safety
        ///
        #[doc = $safety_set]
        #[no_mangle]
        pub unsafe extern "C" fn $set(options: *mut OptionsBox, value: *const c_char) {
            guard((), || {
                if let Some(o) = unsafe { opts(options) } {
                    o.$field = value;
                }
            })
        }
    };
}

borrowed_str_option!(
    sass_option_get_indent,
    sass_option_set_indent,
    indent,
    DEFAULT_INDENT,
    "`options` must be NULL or live. The returned pointer is borrowed (caller's buffer or a static default) and must NOT be freed.",
    "`options` must be NULL or live (NULL-safe no-op); `value` must be NULL or point to a NUL-terminated string that outlives the options."
);
borrowed_str_option!(
    sass_option_get_linefeed,
    sass_option_set_linefeed,
    linefeed,
    DEFAULT_LINEFEED,
    "`options` must be NULL or live. The returned pointer is borrowed (caller's buffer or a static default) and must NOT be freed.",
    "`options` must be NULL or live (NULL-safe no-op); `value` must be NULL or point to a NUL-terminated string that outlives the options."
);

/// Implements one copied string pair: the setter frees the old value and
/// stores a fresh copy (NULL clears); the getter returns the stored pointer
/// or NULL (`safe_str` semantics, sass_context.cpp:229-236,644-647).
///
/// # Safety
///
/// Same pointer contracts as the individual functions below.
macro_rules! owned_str_option {
    ($get:ident, $set:ident, $field:ident, $safety_get:expr, $safety_set:expr) => {
        /// Reads the stored copy, or NULL when unset; NULL options harden to NULL.
        ///
        /// # Safety
        ///
        #[doc = $safety_get]
        #[no_mangle]
        pub unsafe extern "C" fn $get(options: *mut OptionsBox) -> *const c_char {
            guard(ptr::null(), || unsafe {
                opts(options).map(|o| o.$field.get()).unwrap_or(ptr::null())
            })
        }

        /// Frees the old value and stores a fresh copy (NULL clears); NULL
        /// options are a safe no-op.
        ///
        /// # Safety
        ///
        #[doc = $safety_set]
        #[no_mangle]
        pub unsafe extern "C" fn $set(options: *mut OptionsBox, value: *const c_char) {
            guard((), || {
                if let Some(o) = unsafe { opts(options) } {
                    // SAFETY: `value` validity per setter contract below.
                    unsafe { o.$field.set(value) };
                }
            })
        }
    };
}

owned_str_option!(
    sass_option_get_input_path,
    sass_option_set_input_path,
    input_path,
    "`options` must be NULL or live. The returned pointer is owned by the options and must NOT be freed.",
    "`options` must be NULL or live (NULL-safe no-op); `value` must be NULL or a valid NUL-terminated string for the call."
);
owned_str_option!(
    sass_option_get_output_path,
    sass_option_set_output_path,
    output_path,
    "`options` must be NULL or live. The returned pointer is owned by the options and must NOT be freed.",
    "`options` must be NULL or live (NULL-safe no-op); `value` must be NULL or a valid NUL-terminated string for the call."
);
// NOTE: no `sass_option_get_include_path` / `sass_option_get_plugin_path`
// single-string getters exist in the C API — those names are the indexed
// path-list getters (defined by `path_list_getters!` below). The
// single-string include/plugin options expose ONLY setters upstream
// (`sass_option_set_include_path`, `sass_option_set_plugin_path`), which set
// the joined string, not the list (plan §A.3) — so they are hand-written
// here, not emitted by `owned_str_option!` (that would collide).
/// Sets the joined include-path string (copied; NULL clears).
///
/// # Safety
///
/// `options` must be NULL or live (NULL-safe no-op); `value` must be NULL or
/// a valid NUL-terminated string for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_option_set_include_path(
    options: *mut OptionsBox,
    value: *const c_char,
) {
    guard((), || {
        if let Some(o) = unsafe { opts(options) } {
            // SAFETY: `value` validity per setter contract above.
            unsafe { o.include_path.set(value) };
        }
    });
}

/// Sets the joined plugin-path string (copied; NULL clears).
///
/// # Safety
///
/// See [`sass_option_set_include_path`].
#[no_mangle]
pub unsafe extern "C" fn sass_option_set_plugin_path(
    options: *mut OptionsBox,
    value: *const c_char,
) {
    guard((), || {
        if let Some(o) = unsafe { opts(options) } {
            // SAFETY: `value` validity per setter contract above.
            unsafe { o.plugin_path.set(value) };
        }
    });
}
owned_str_option!(
    sass_option_get_source_map_file,
    sass_option_set_source_map_file,
    source_map_file,
    "`options` must be NULL or live. The returned pointer is owned by the options and must NOT be freed.",
    "`options` must be NULL or live (NULL-safe no-op); `value` must be NULL or a valid NUL-terminated string for the call."
);
owned_str_option!(
    sass_option_get_source_map_root,
    sass_option_set_source_map_root,
    source_map_root,
    "`options` must be NULL or live. The returned pointer is owned by the options and must NOT be freed.",
    "`options` must be NULL or live (NULL-safe no-op); `value` must be NULL or a valid NUL-terminated string for the call."
);

/// Pushes a copied path onto the include list. NULL-safe (stores NULL entry,
/// mirroring upstream `path ? copy : 0`; sass_context.cpp:673-688).
///
/// # Safety
///
/// `options` must be NULL or live (NULL-safe no-op); `path` must be NULL or a
/// valid NUL-terminated string for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_option_push_include_path(
    options: *mut OptionsBox,
    path: *const c_char,
) {
    guard((), || {
        let Some(o) = (unsafe { opts(options) }) else {
            return;
        };
        let copied = match unsafe { read_opt_str(path) } {
            Some(bytes) => copy_bytes_nul(&bytes),
            None => ptr::null_mut(),
        };
        o.include_paths.push(PathEntry { ptr: copied });
    });
}

/// Pushes a copied path onto the plugin list (same contract as
/// [`sass_option_push_include_path`]; sass_context.cpp:724+).
///
/// # Safety
///
/// See [`sass_option_push_include_path`].
#[no_mangle]
pub unsafe extern "C" fn sass_option_push_plugin_path(
    options: *mut OptionsBox,
    path: *const c_char,
) {
    guard((), || {
        let Some(o) = (unsafe { opts(options) }) else {
            return;
        };
        let copied = match unsafe { read_opt_str(path) } {
            Some(bytes) => copy_bytes_nul(&bytes),
            None => ptr::null_mut(),
        };
        o.plugin_paths.push(PathEntry { ptr: copied });
    });
}

/// Implements one indexed path getter pair: size + element access. Out of
/// range (or NULL options) yields NULL — hardening over upstream's unchecked
/// walk (plan §A.8); the contract pins in-range behavior identically.
macro_rules! path_list_getters {
    ($size:ident, $get:ident, $field:ident, $safety_size:expr, $safety_get:expr) => {
        /// Counts pushed paths; NULL options harden to 0.
        ///
        /// # Safety
        ///
        #[doc = $safety_size]
        #[no_mangle]
        pub unsafe extern "C" fn $size(options: *mut OptionsBox) -> usize {
            guard(0, || unsafe {
                opts(options).map(|o| o.$field.len()).unwrap_or(0)
            })
        }

        /// Reads the pushed path at `i`, or NULL when out of range (hardening
        /// over upstream's unchecked walk); NULL options harden to NULL.
        ///
        /// # Safety
        ///
        #[doc = $safety_get]
        #[no_mangle]
        pub unsafe extern "C" fn $get(options: *mut OptionsBox, i: usize) -> *const c_char {
            guard(ptr::null(), || unsafe {
                opts(options)
                    .and_then(|o| o.$field.get(i))
                    .map(|e| e.ptr as *const c_char)
                    .unwrap_or(ptr::null())
            })
        }
    };
}

path_list_getters!(
    sass_option_get_include_path_size,
    sass_option_get_include_path,
    include_paths,
    "`options` must be NULL or live.",
    "`options` must be NULL or live. The returned pointer is owned by the options and must NOT be freed."
);
path_list_getters!(
    sass_option_get_plugin_path_size,
    sass_option_get_plugin_path,
    plugin_paths,
    "`options` must be NULL or live.",
    "`options` must be NULL or live. The returned pointer is owned by the options and must NOT be freed."
);

/// Implements one opaque `c_*` list slot: stored pointer in, stored pointer
/// out (importers/headers stay consumer-owned until bridging lands; plan §7).
macro_rules! opaque_list_option {
    ($get:ident, $set:ident, $field:ident, $safety_get:expr, $safety_set:expr) => {
        /// Reads the stored callback-list pointer (NULL when unset); NULL
        /// options harden to NULL.
        ///
        /// # Safety
        ///
        #[doc = $safety_get]
        #[no_mangle]
        pub unsafe extern "C" fn $get(options: *mut OptionsBox) -> *mut libc::c_void {
            guard(ptr::null_mut(), || unsafe {
                opts(options).map(|o| o.$field).unwrap_or(ptr::null_mut())
            })
        }

        /// Stores the callback-list pointer (entries stay consumer-owned until
        /// bridging lands); NULL options are a safe no-op.
        ///
        /// # Safety
        ///
        #[doc = $safety_set]
        #[no_mangle]
        pub unsafe extern "C" fn $set(options: *mut OptionsBox, value: *mut libc::c_void) {
            guard((), || {
                if let Some(o) = unsafe { opts(options) } {
                    o.$field = value;
                }
            })
        }
    };
}

/// Reads the stored custom-function list (NULL when unset); NULL options
/// harden to NULL.
///
/// # Safety
///
/// `options` must be NULL or live. Borrowed; freed with the options (never
/// free the getter result — use `sass_delete_function_list` only for lists
/// never handed to options).
#[no_mangle]
pub unsafe extern "C" fn sass_option_get_c_functions(
    options: *mut OptionsBox,
) -> *mut *mut FunctionEntry {
    guard(ptr::null_mut(), || unsafe {
        opts(options)
            .map(|o| o.c_functions)
            .unwrap_or(ptr::null_mut())
    })
}

/// Stores the custom-function list, taking ownership (frees any previously
/// stored list + entries first — hardening over upstream's overwriting
/// store, which leaks; same class as `OwnedCString`). Never the cookies.
///
/// Mirrors `sass_option_set_c_functions` (plain upstream store +
/// `sass_clear_options` frees on delete/move).
///
/// # Safety
///
/// `options` must be NULL or live (NULL-safe no-op); `value` must be NULL or
/// a live NULL-terminated entry list whose ownership (list + entries, not
/// cookies) transfers. Must not alias the currently stored list.
#[no_mangle]
pub unsafe extern "C" fn sass_option_set_c_functions(
    options: *mut OptionsBox,
    value: *mut *mut FunctionEntry,
) {
    guard((), || {
        if let Some(o) = unsafe { opts(options) } {
            // SAFETY: currently stored list (if any) is owned per contract.
            unsafe { free_function_list(o.c_functions) };
            o.c_functions = value;
        }
    })
}
/// Reads the stored custom-importer list (NULL when unset); NULL options
/// harden to NULL.
///
/// # Safety
///
/// `options` must be NULL or live. Borrowed; freed with the options (never
/// free the getter result — use `sass_delete_importer_list` only for lists
/// never handed to options).
#[no_mangle]
pub unsafe extern "C" fn sass_option_get_c_importers(
    options: *mut OptionsBox,
) -> *mut *mut ImporterEntry {
    guard(ptr::null_mut(), || unsafe {
        opts(options)
            .map(|o| o.c_importers)
            .unwrap_or(ptr::null_mut())
    })
}

/// Stores the custom-importer list, taking ownership (frees any previously
/// stored list + entries first — hardening over upstream's overwriting
/// store, which leaks; same class as `OwnedCString`). Never the cookies.
///
/// Mirrors `sass_option_set_c_importers` (plain upstream store +
/// `sass_clear_options` frees on delete/move).
///
/// # Safety
///
/// `options` must be NULL or live (NULL-safe no-op); `value` must be NULL or
/// a live NULL-terminated entry list whose ownership (list + entries, not
/// cookies) transfers. Must not alias the currently stored list.
#[no_mangle]
pub unsafe extern "C" fn sass_option_set_c_importers(
    options: *mut OptionsBox,
    value: *mut *mut ImporterEntry,
) {
    guard((), || {
        if let Some(o) = unsafe { opts(options) } {
            // SAFETY: currently stored list (if any) is owned per contract.
            unsafe { free_importer_list(o.c_importers) };
            o.c_importers = value;
        }
    })
}
opaque_list_option!(
    sass_option_get_c_headers,
    sass_option_set_c_headers,
    c_headers,
    "`options` must be NULL or live. The returned pointer is stored opaquely and must NOT be freed by the getter.",
    "`options` must be NULL or live (NULL-safe no-op). Entries stay consumer-owned until callback bridging lands."
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    unsafe fn read_opt(ptr: *const c_char) -> Option<Vec<u8>> {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: test-only; non-null pointers are valid strings produced by
        // the functions under test.
        Some(unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec())
    }

    unsafe fn make() -> *mut OptionsBox {
        let opts = unsafe { sass_make_options() };
        assert!(!opts.is_null());
        opts
    }

    #[test]
    fn defaults_match_libsass_init_options() {
        unsafe {
            let opts = make();
            assert_eq!(sass_option_get_precision(opts), 10);
            assert_eq!(sass_option_get_output_style(opts), SASS_STYLE_NESTED);
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
            assert_eq!(sass_option_get_include_path_size(opts), 0);
            assert_eq!(sass_option_get_plugin_path_size(opts), 0);
            assert!(sass_option_get_c_functions(opts).is_null());
            sass_delete_options(opts);
        }
    }

    #[test]
    fn owned_strings_copy_and_clear() {
        unsafe {
            let opts = make();
            let first = CString::new("a.scss").unwrap();
            sass_option_set_input_path(opts, first.as_ptr());
            assert_eq!(
                read_opt(sass_option_get_input_path(opts)),
                Some(b"a.scss".to_vec())
            );
            // The stored pointer is a copy at a distinct address.
            assert_ne!(
                sass_option_get_input_path(opts),
                first.as_ptr() as *const c_char
            );
            // Overwrite replaces; NULL clears.
            let second = CString::new("b.scss").unwrap();
            sass_option_set_input_path(opts, second.as_ptr());
            assert_eq!(
                read_opt(sass_option_get_input_path(opts)),
                Some(b"b.scss".to_vec())
            );
            sass_option_set_input_path(opts, ptr::null());
            assert_eq!(read_opt(sass_option_get_input_path(opts)), None);
            sass_delete_options(opts);
        }
    }

    #[test]
    fn borrowed_strings_reset_to_default() {
        unsafe {
            let opts = make();
            let tabs = CString::new("\t").unwrap();
            sass_option_set_indent(opts, tabs.as_ptr());
            assert_eq!(sass_option_get_indent(opts), tabs.as_ptr() as *const c_char);
            sass_option_set_indent(opts, ptr::null());
            assert_eq!(read_opt(sass_option_get_indent(opts)), Some(b"  ".to_vec()));
            sass_delete_options(opts);
            // NULL options are safe on every accessor shape.
            assert_eq!(sass_option_get_precision(ptr::null_mut()), 0);
            assert!(sass_option_get_indent(ptr::null_mut()).is_null());
            assert!(sass_option_get_input_path(ptr::null_mut()).is_null());
            assert_eq!(sass_option_get_include_path_size(ptr::null_mut()), 0);
            sass_delete_options(ptr::null_mut());
            sass_option_set_precision(ptr::null_mut(), 3);
        }
    }

    #[test]
    fn path_lists_copy_push_and_index() {
        unsafe {
            let opts = make();
            let a = CString::new("/a").unwrap();
            sass_option_push_include_path(opts, a.as_ptr());
            let mut owned = b"/b".to_vec();
            owned.push(0);
            sass_option_push_include_path(opts, owned.as_ptr() as *const c_char);
            owned[0] = b'X';
            assert_eq!(sass_option_get_include_path_size(opts), 2);
            assert_eq!(
                read_opt(sass_option_get_include_path(opts, 1)),
                Some(b"/b".to_vec())
            );
            // Out of range hardens to NULL (upstream: UB).
            assert!(sass_option_get_include_path(opts, 9).is_null());
            sass_delete_options(opts);
        }
    }
}
