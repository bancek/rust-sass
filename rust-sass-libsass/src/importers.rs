// Copyright 2012-2016 Sass Open Source Foundation. Use of this source code
// is governed by an MIT-style license that can be found in the LICENSE
// file or at https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// libsass-source: src/sass_functions.cpp (entries/results) + src/context.cpp (call_loader/register_resource) + src/sass.cpp (find helpers) + include/sass/functions.h (importer half)

use crate::functions::ImportStackEntry;
use crate::options::OptionsBox;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::fmt;
use std::path::Path;
use std::ptr;
use std::rc::Rc;

use bumpalo::Bump;

use rust_sass::common::exception::{SassError, SassResult};
use rust_sass::eval::importer::{
    CanonicalizeContext, Importer, ImporterKind, ImporterResult, UserImporter,
};
use rust_sass::eval::syntax::syntax_for_path;
use rust_sass::parse::stylesheet::Syntax;
use rust_sass::url::SassUrl;

use crate::base::{copy_bytes_nul, guard, malloc_or_abort};
use crate::functions::CompilerBox;

/// The consumer importer callback: `url` is the unquoted load path
/// (borrowed); the returned list transfers to the compiler (never delete it);
/// NULL means fallthrough. Mirrors `Sass_Importer_Fn` (functions.h:33-34).
pub type SassImporterFn = Option<
    unsafe extern "C" fn(
        url: *const c_char,
        entry: *mut ImporterEntry,
        compiler: *mut CompilerBox,
    ) -> *mut *mut ImportEntry,
>;

/// One custom-importer registration. Mirrors `Sass_Importer`
/// (sass_functions.hpp: `importer` + `priority` + `cookie` stored raw).
#[repr(C)]
pub struct ImporterEntry {
    callback: SassImporterFn,
    priority: f64,
    cookie: *mut c_void,
}

/// One importer result entry. Mirrors `Sass_Import` (sass_functions.hpp:
/// `imp_path`/`abs_path` copied at make, `source`/`srcmap` adopted as-is,
/// `error` copied by `set_error`, `line`/`column` init `-1`).
#[repr(C)]
pub struct ImportEntry {
    imp_path: *mut c_char,
    abs_path: *mut c_char,
    source: *mut c_char,
    srcmap: *mut c_char,
    error: *mut c_char,
    line: usize,
    column: usize,
}

/// Allocates a NULL-terminated importer list with `length` slots (mirrors
/// `calloc(length + 1, …)`, sass_functions.cpp:77-80).
///
/// # Safety
///
/// Always safe to call; the result is owned by the caller (frees with
/// [`sass_delete_importer_list`], or transfers to options).
#[no_mangle]
pub unsafe extern "C" fn sass_make_importer_list(length: usize) -> *mut *mut ImporterEntry {
    guard(ptr::null_mut(), || {
        // SAFETY: fresh zeroed array of exactly `length + 1` slots (the
        // terminator included, so the list is valid even before any set).
        let list = malloc_or_abort(
            length
                .wrapping_add(1)
                .wrapping_mul(size_of::<*mut ImporterEntry>()),
        ) as *mut *mut ImporterEntry;
        unsafe {
            ptr::write_bytes(list, 0, length + 1);
        }
        list
    })
}

/// Creates one importer entry, storing `callback`/`priority`/`cookie` raw
/// (never copies the cookie — consumer-owned, mirroring `sass_make_importer`,
/// sass_functions.cpp:56-64).
///
/// # Safety
///
/// `callback` may be NULL (a NULL callback fails the compile with a clear
/// message instead of crashing at invocation). The result is owned by the
/// caller.
#[no_mangle]
pub unsafe extern "C" fn sass_make_importer(
    callback: SassImporterFn,
    priority: f64,
    cookie: *mut c_void,
) -> *mut ImporterEntry {
    guard(ptr::null_mut(), || {
        // SAFETY: fresh box; ownership moves to the caller.
        Box::into_raw(Box::new(ImporterEntry {
            callback,
            priority,
            cookie,
        }))
    })
}

/// Frees one entry (struct only; never the cookie — consumer-owned, mirroring
/// `sass_delete_importer`, sass_functions.cpp:71-74). NULL-safe.
///
/// # Safety
///
/// `entry` must be NULL or a live entry, freed exactly once here.
#[no_mangle]
pub unsafe extern "C" fn sass_delete_importer(entry: *mut ImporterEntry) {
    guard((), || {
        if entry.is_null() {
            return;
        }
        // SAFETY: live entry per contract; struct has no owned strings.
        unsafe {
            drop(Box::from_raw(entry));
        }
    });
}

/// Frees a NULL-terminated importer list and every entry in it (NULL-list
/// safe). A NULL *entry* terminates the walk (mirrors the `while (*list)`
/// loop, sass_functions.cpp:83-92).
///
/// # Safety
///
/// `list` must be NULL or a live NULL-terminated list whose entries are all
/// live and freed exactly once here.
#[no_mangle]
pub unsafe extern "C" fn sass_delete_importer_list(list: *mut *mut ImporterEntry) {
    guard((), || {
        // SAFETY: NULL/live per contract above.
        unsafe { free_importer_list(list) };
    });
}

/// The list-free primitive shared by the explicit delete, the options Drop,
/// and set-overwrite (all free lists+entries, never cookies).
///
/// # Safety
///
/// See [`sass_delete_importer_list`].
pub(crate) unsafe fn free_importer_list(list: *mut *mut ImporterEntry) {
    if list.is_null() {
        return;
    }
    // SAFETY: live NULL-terminated list per contract; entries live.
    unsafe {
        let mut cur = list;
        while !(*cur).is_null() {
            drop(Box::from_raw(*cur));
            cur = cur.add(1);
        }
        libc::free(list as *mut libc::c_void);
    }
}

/// Reads the entry at `pos` (borrowed). Out-of-range dereference is consumer
/// UB like upstream (`list[pos]`) — but NULL lists harden to NULL.
///
/// # Safety
///
/// `list` must be NULL or a live list; `pos` must be in range for a non-NULL
/// list. Borrowed; freed with the list.
#[no_mangle]
pub unsafe extern "C" fn sass_importer_get_list_entry(
    list: *mut *mut ImporterEntry,
    pos: usize,
) -> *mut ImporterEntry {
    guard(ptr::null_mut(), || {
        if list.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live list + in-range per contract above.
        unsafe { *list.add(pos) }
    })
}

/// Stores `entry` at `pos` (no ownership change — the list owns it either
/// way). Same NULL/range contract as the getter.
///
/// # Safety
///
/// `list` must be NULL or a live list (NULL-safe no-op); `pos` must be in
/// range; `entry` must be NULL or a live entry whose ownership the list takes.
#[no_mangle]
pub unsafe extern "C" fn sass_importer_set_list_entry(
    list: *mut *mut ImporterEntry,
    pos: usize,
    entry: *mut ImporterEntry,
) {
    guard((), || {
        if list.is_null() {
            return;
        }
        // SAFETY: live list + in-range per contract above.
        unsafe {
            *list.add(pos) = entry;
        }
    })
}

/// Reads the entry's callback (NULL entry hardens to None).
///
/// # Safety
///
/// `entry` must be NULL or a live entry for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_importer_get_function(entry: *mut ImporterEntry) -> SassImporterFn {
    guard(None, || {
        if entry.is_null() {
            return None;
        }
        // SAFETY: live entry per contract above.
        unsafe { (*entry).callback }
    })
}

/// Reads the entry's priority (NULL entry hardens to 0).
///
/// # Safety
///
/// `entry` must be NULL or a live entry for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_importer_get_priority(entry: *mut ImporterEntry) -> f64 {
    guard(0.0, || {
        if entry.is_null() {
            return 0.0;
        }
        // SAFETY: live entry per contract above.
        unsafe { (*entry).priority }
    })
}

/// Reads the entry's cookie (NULL entry hardens to NULL).
///
/// # Safety
///
/// `entry` must be NULL or a live entry for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_importer_get_cookie(entry: *mut ImporterEntry) -> *mut c_void {
    guard(ptr::null_mut(), || {
        if entry.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live entry per contract above.
        unsafe { (*entry).cookie }
    })
}

/// Allocates a NULL-terminated import-result list with `length` slots
/// (mirrors `calloc(length + 1, …)`, sass_functions.cpp:98-101).
///
/// # Safety
///
/// Always safe to call; the result transfers to the compiler (or frees with
/// [`sass_delete_import_list`]).
#[no_mangle]
pub unsafe extern "C" fn sass_make_import_list(length: usize) -> *mut *mut ImportEntry {
    guard(ptr::null_mut(), || {
        // SAFETY: fresh zeroed array of exactly `length + 1` slots.
        let list = malloc_or_abort(
            length
                .wrapping_add(1)
                .wrapping_mul(size_of::<*mut ImportEntry>()),
        ) as *mut *mut ImportEntry;
        unsafe {
            ptr::write_bytes(list, 0, length + 1);
        }
        list
    })
}

/// Copies one C string field (NULL → NULL).
///
/// # Safety
///
/// `s` must be NULL or a valid NUL-terminated string.
unsafe fn copy_field(s: *const c_char) -> *mut c_char {
    if s.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: non-null NUL-terminated string per contract above.
    let bytes = unsafe { CStr::from_ptr(s) }.to_bytes();
    copy_bytes_nul(bytes)
}

/// Creates one result entry: COPIES `imp_path`/`abs_path`, ADOPTS
/// `source`/`srcmap` as-is (they must be malloc'd — freed with the entry).
/// `error` starts NULL, `line`/`column` start `-1` (mirrors `sass_make_import`,
/// sass_functions.cpp:105-117).
///
/// # Safety
///
/// `imp_path`/`abs_path` must be NULL or valid strings for the call;
/// `source`/`srcmap` must be NULL or malloc'd buffers whose ownership
/// transfers. The result transfers to the compiler (or frees with
/// [`sass_delete_import`]).
#[no_mangle]
pub unsafe extern "C" fn sass_make_import(
    imp_path: *const c_char,
    abs_path: *const c_char,
    source: *mut c_char,
    srcmap: *mut c_char,
) -> *mut ImportEntry {
    guard(ptr::null_mut(), || {
        // SAFETY: per contract above (copies paths, adopts buffers).
        unsafe {
            Box::into_raw(Box::new(ImportEntry {
                imp_path: copy_field(imp_path),
                abs_path: copy_field(abs_path),
                source,
                srcmap,
                error: ptr::null_mut(),
                line: usize::MAX,
                column: usize::MAX,
            }))
        }
    })
}

/// Creates one result entry with `path` as both paths (mirrors
/// `sass_make_import_entry`, sass_functions.cpp:120-123).
///
/// # Safety
///
/// See [`sass_make_import`].
#[no_mangle]
pub unsafe extern "C" fn sass_make_import_entry(
    path: *const c_char,
    source: *mut c_char,
    srcmap: *mut c_char,
) -> *mut ImportEntry {
    guard(ptr::null_mut(), || {
        // SAFETY: per contract above.
        unsafe { sass_make_import(path, path, source, srcmap) }
    })
}

/// Attaches an error to `entry`: copies `message`, maps falsy line/col (0)
/// to `-1` (mirrors `sass_import_set_error`, sass_functions.cpp:126-134 —
/// line 0 is inexpressible). Returns the entry.
///
/// # Safety
///
/// `entry` must be a live entry; `message` must be NULL or a valid string
/// for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_import_set_error(
    entry: *mut ImportEntry,
    message: *const c_char,
    line: usize,
    column: usize,
) -> *mut ImportEntry {
    guard(ptr::null_mut(), || {
        if entry.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live entry per contract; message validity per contract.
        unsafe {
            if !(*entry).error.is_null() {
                libc::free((*entry).error as *mut libc::c_void);
                (*entry).error = ptr::null_mut();
            }
            (*entry).error = copy_field(message);
            // Falsy (0) maps to -1: line 0 is inexpressible upstream.
            (*entry).line = if line == 0 { usize::MAX } else { line };
            (*entry).column = if column == 0 { usize::MAX } else { column };
            entry
        }
    })
}

/// Stores `entry` at `pos` (NULL-list safe no-op).
///
/// # Safety
///
/// `list` must be NULL or a live list; `pos` in range; `entry` NULL or live
/// with ownership transferring to the list.
#[no_mangle]
pub unsafe extern "C" fn sass_import_set_list_entry(
    list: *mut *mut ImportEntry,
    pos: usize,
    entry: *mut ImportEntry,
) {
    guard((), || {
        if list.is_null() {
            return;
        }
        // SAFETY: live list + in-range per contract above.
        unsafe {
            *list.add(pos) = entry;
        }
    })
}

/// Reads the entry at `pos` (borrowed; NULL list hardens to NULL).
///
/// # Safety
///
/// `list` must be NULL or a live list; `pos` in range for non-NULL lists.
#[no_mangle]
pub unsafe extern "C" fn sass_import_get_list_entry(
    list: *mut *mut ImportEntry,
    pos: usize,
) -> *mut ImportEntry {
    guard(ptr::null_mut(), || {
        if list.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live list + in-range per contract above.
        unsafe { *list.add(pos) }
    })
}

macro_rules! import_getter {
    ($get:ident, $field:ident, $safety:expr) => {
        /// Reads the entry field (borrowed; NULL entry hardens to NULL).
        ///
        /// # Safety
        ///
        #[doc = $safety]
        #[no_mangle]
        pub unsafe extern "C" fn $get(entry: *mut ImportEntry) -> *const c_char {
            guard(ptr::null(), || {
                if entry.is_null() {
                    return ptr::null();
                }
                // SAFETY: live entry per contract above.
                unsafe { (*entry).$field as *const c_char }
            })
        }
    };
}

import_getter!(
    sass_import_get_imp_path,
    imp_path,
    "`entry` must be NULL or a live entry for the call. Borrowed; freed with the entry."
);
import_getter!(
    sass_import_get_abs_path,
    abs_path,
    "`entry` must be NULL or a live entry for the call. Borrowed; freed with the entry."
);
import_getter!(
    sass_import_get_source,
    source,
    "`entry` must be NULL or a live entry for the call. Borrowed; freed with the entry."
);
import_getter!(
    sass_import_get_srcmap,
    srcmap,
    "`entry` must be NULL or a live entry for the call. Borrowed; freed with the entry."
);
import_getter!(
    sass_import_get_error_message,
    error,
    "`entry` must be NULL or a live entry for the call. Borrowed; freed with the entry."
);

/// Detaches `source` (returns it + nulls the field — mirrors
/// `sass_import_take_source`, sass_functions.cpp:207). Caller owns the result.
///
/// # Safety
///
/// `entry` must be NULL (→ NULL) or a live entry.
#[no_mangle]
pub unsafe extern "C" fn sass_import_take_source(entry: *mut ImportEntry) -> *mut c_char {
    guard(ptr::null_mut(), || {
        if entry.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live entry per contract above.
        unsafe { std::mem::replace(&mut (*entry).source, ptr::null_mut()) }
    })
}

/// Detaches `srcmap` (mirrors `sass_import_take_srcmap`).
///
/// # Safety
///
/// See [`sass_import_take_source`].
#[no_mangle]
pub unsafe extern "C" fn sass_import_take_srcmap(entry: *mut ImportEntry) -> *mut c_char {
    guard(ptr::null_mut(), || {
        if entry.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live entry per contract above.
        unsafe { std::mem::replace(&mut (*entry).srcmap, ptr::null_mut()) }
    })
}

/// Reads the error line (`usize::MAX` = unset `-1`; NULL hardens to it).
///
/// # Safety
///
/// `entry` must be NULL or a live entry for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_import_get_error_line(entry: *mut ImportEntry) -> usize {
    guard(usize::MAX, || {
        if entry.is_null() {
            return usize::MAX;
        }
        // SAFETY: live entry per contract above.
        unsafe { (*entry).line }
    })
}

/// Reads the error column (same contract as the line getter).
///
/// # Safety
///
/// `entry` must be NULL or a live entry for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_import_get_error_column(entry: *mut ImportEntry) -> usize {
    guard(usize::MAX, || {
        if entry.is_null() {
            return usize::MAX;
        }
        // SAFETY: live entry per contract above.
        unsafe { (*entry).column }
    })
}

/// Frees one entry and all five buffers (NULL-safe).
///
/// # Safety
///
/// `entry` must be NULL or a live entry, freed exactly once here.
#[no_mangle]
pub unsafe extern "C" fn sass_delete_import(entry: *mut ImportEntry) {
    guard((), || {
        // SAFETY: NULL/live per contract; Drop glue below frees buffers.
        unsafe { free_import_entry(entry) };
    });
}

/// The entry-free primitive shared by the explicit delete, list deletes, and
/// the `CompilerBox` drop.
///
/// # Safety
///
/// `entry` must be NULL or a live entry, freed exactly once here.
pub(crate) unsafe fn free_import_entry(entry: *mut ImportEntry) {
    if entry.is_null() {
        return;
    }
    // SAFETY: live entry per contract.
    unsafe {
        let owned = Box::from_raw(entry);
        for slot in [
            owned.imp_path,
            owned.abs_path,
            owned.source,
            owned.srcmap,
            owned.error,
        ] {
            if !slot.is_null() {
                libc::free(slot as *mut libc::c_void);
            }
        }
    }
}

/// Frees a NULL-terminated import-result list and every entry in it
/// (NULL-list safe; NULL entries terminate the walk, mirroring
/// `sass_delete_import_list`, sass_functions.cpp:141-150).
///
/// # Safety
///
/// `list` must be NULL or a live NULL-terminated list whose entries are all
/// live and freed exactly once here.
#[no_mangle]
pub unsafe extern "C" fn sass_delete_import_list(list: *mut *mut ImportEntry) {
    guard((), || {
        if list.is_null() {
            return;
        }
        // SAFETY: live NULL-terminated list per contract; entries live.
        unsafe {
            let mut cur = list;
            while !(*cur).is_null() {
                free_import_entry(*cur);
                cur = cur.add(1);
            }
            libc::free(list as *mut libc::c_void);
        }
    });
}

// ── Bridge ────────────────────────────────────────────────────────────────

/// One copied-out C result entry (owned Rust bytes; the C originals are
/// already freed by `sass_delete_import_list` — take-before-delete, mirroring
/// `call_loader`, context.cpp:435-436,475).
struct RawImport {
    imp_path: Option<Vec<u8>>,
    abs_path: Option<Vec<u8>>,
    source: Option<Vec<u8>>,
    srcmap: Option<Vec<u8>>,
    error: Option<Vec<u8>>,
    line: usize,
    column: usize,
}

/// Reads a borrowed C string field lossily (NULL → None).
///
/// # Safety
///
/// `s` must be NULL or a valid NUL-terminated string.
unsafe fn read_field(s: *const c_char) -> Option<Vec<u8>> {
    if s.is_null() {
        return None;
    }
    // SAFETY: per contract above.
    Some(unsafe { CStr::from_ptr(s) }.to_bytes().to_vec())
}

/// Stashed per-canonical-URL import data: `canonicalize` invokes the C
/// callback once and caches the outcome for `load` (the core splits what
/// upstream does in one `call_loader` round-trip into two calls).
#[derive(Debug)]
struct StoredImport {
    contents: String,
    syntax: Syntax,
    srcmap: Option<String>,
    /// `prev` for nested imports whose containing URL is this canonical:
    /// the usable `abs_path` when file-backed (upstream's `path_key`), else
    /// the raw load path (upstream's `uniq_path` for contents-only entries).
    prev: String,
    /// `Some` = path-only return: no contents were served, `load` reads the
    /// file from disk itself (mirrors the `import_url` fallback,
    /// context.cpp:461-470).
    filesystem_path: Option<String>,
}

/// A `UserImporter` wrapping one C importer entry. Raw C pointers are
/// captured (the owning options outlive every compile — same contract as the
/// step-6 function bridge); the per-compile `CompilerBox` token is
/// stack-local per invocation.
struct CImporter {
    entry: *mut ImporterEntry,
    callback: unsafe extern "C" fn(
        *const c_char,
        *mut ImporterEntry,
        *mut CompilerBox,
    ) -> *mut *mut ImportEntry,
    index: usize,
    /// Entry frame seeding every per-call token (upstream's never-popped
    /// file/data entry — keeps `get_last_import` non-NULL).
    token_entry: ImportStackEntry,
    stash: RefCell<HashMap<String, StoredImport>>,
}

impl fmt::Debug for CImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CImporter")
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

/// Builds an absolute canonical URL for served contents: a usable `abs_path`
/// becomes a `file:` URL (on-disk identity, like upstream's `path_key`),
/// otherwise a synthetic `sass-c-importer://` URL (absolute and stable per
/// importer, satisfying the canonical-URL contract).
fn canonical_for(index: usize, nonce: usize, abs_path: Option<&str>) -> Option<SassUrl> {
    if let Some(abs) = abs_path {
        if !abs.is_empty() {
            if let Ok(url) = SassUrl::file_url_from_abs_path(abs) {
                return Some(url);
            }
            if let Ok(url) = SassUrl::parse(abs) {
                if !url.is_relative() {
                    return Some(url);
                }
            }
        }
    }
    SassUrl::parse(&format!("sass-c-importer://importer-{index}/{nonce}")).ok()
}

/// Reports a containing URL in the shape upstream's import stack uses:
/// `file:` URLs become filesystem paths (what the importer returned / what
/// the filesystem resolved — upstream `abs_path` entries never carry a URL
/// scheme), anything else passes through verbatim.
///
/// Load-bearing for `prev`: node-sass forwards `get_last_import →
/// `abs_path`` to JS, where consumers compare it against the plain path
/// they returned.
fn prev_form(url: &SassUrl) -> String {
    if url.is_file() {
        if let Ok(path) = url.as_url().to_file_path() {
            return path.to_string_lossy().into_owned();
        }
    }
    url.to_string()
}

fn script_error(message: String) -> Box<SassError> {
    Box::new(SassError::Script {
        message,
        argument_name: None,
    })
}

impl UserImporter for CImporter {
    /// Libsass-compat shape (see `prefers_raw_load_paths`): the core
    /// consults this importer with the raw load path, so `url` here is the
    /// verbatim `@import` string exactly as upstream's `call_loader` passes
    /// it.
    fn prefers_raw_load_paths(&self) -> bool {
        true
    }

    fn canonicalize<'a>(
        &'a self,
        url: &'a SassUrl,
        context: &'a mut CanonicalizeContext,
    ) -> SassResult<Option<SassUrl>> {
        let url_text = url.to_string();
        // `prev` for the C callback comes from the containing URL (what
        // node-sass reads via `get_last_import` → `abs_path`). Read without
        // marking: the served result does not vary with the load site, so the
        // canonicalization stays cacheable.
        let containing = context.containing_url_without_marking();
        // `prev` for the token: the stashed original shape when the
        // containing canonical came from this importer (upstream's
        // `path_key`/`uniq_path`), else the path form of a `file:` URL
        // (upstream `abs_path` never carries a scheme), else verbatim.
        let containing_prev = containing.map(|u| {
            if let Some(stored) = self.stash.borrow().get(&u.to_string()) {
                return stored.prev.clone();
            }
            prev_form(u)
        });
        let mut compiler = CompilerBox::for_import(containing_prev.as_deref(), &self.token_entry);
        let url_c = CString::new(url_text.clone())
            .map_err(|_| script_error(format!("import URL contains a NUL byte: {url_text:?}")))?;
        // SAFETY: `url_c` is a live NUL-terminated string; `entry` outlives
        // the compile per contract; `compiler` is stack-local for the call.
        // The consumer must not retain pointers or unwind across the boundary
        // (`guard` converts a Rust panic below, but a C longjmp is UB — same
        // contract as the step-6 function bridge).
        let raw_list = guard(ptr::null_mut(), || unsafe {
            (self.callback)(url_c.as_ptr(), self.entry, &mut compiler)
        });
        if raw_list.is_null() {
            return Ok(None);
        }
        // Copy every field out first, then free the C list (take-before-
        // delete: the delete would free the buffers out from under us).
        let mut raws: Vec<RawImport> = Vec::new();
        // SAFETY: live NULL-terminated list per maker contract.
        unsafe {
            let mut cur = raw_list;
            while !(*cur).is_null() {
                let e = *cur;
                raws.push(RawImport {
                    imp_path: read_field((*e).imp_path),
                    abs_path: read_field((*e).abs_path),
                    source: read_field((*e).source),
                    srcmap: read_field((*e).srcmap),
                    error: read_field((*e).error),
                    line: (*e).line,
                    column: (*e).column,
                });
                cur = cur.add(1);
            }
            sass_delete_import_list(raw_list);
        }
        if raws.is_empty() {
            // Empty list / all-NULL entries: fall through to the next
            // importer / filesystem. (Upstream treats an empty list as a
            // handled-but-empty import — the "hide import" quirk,
            // api-importer-example.md:98-111 — deliberately not mirrored: a
            // silent drop hides stylesheets, fallthrough is observable.)
            return Ok(None);
        }
        if raws.len() > 1 {
            // One C call maps onto one core canonicalization (single URL).
            // Fail fast rather than silently serving the first entry and
            // dropping the rest.
            return Err(script_error(format!(
                "custom importer returned {} imports; only single-import returns are supported",
                raws.len()
            )));
        }
        let raw = raws.pop().unwrap_or(RawImport {
            imp_path: None,
            abs_path: None,
            source: None,
            srcmap: None,
            error: None,
            line: usize::MAX,
            column: usize::MAX,
        });
        if let Some(err) = raw.error {
            let mut message = String::from_utf8_lossy(&err).into_owned();
            if raw.line != usize::MAX || raw.column != usize::MAX {
                let line = if raw.line == usize::MAX { 0 } else { raw.line };
                let col = if raw.column == usize::MAX {
                    0
                } else {
                    raw.column
                };
                message.push_str(&format!(" (line {line}, column {col})"));
            }
            return Err(script_error(message));
        }
        let abs_str = raw
            .abs_path
            .as_ref()
            .and_then(|b| std::str::from_utf8(b).ok());
        if let Some(source) = raw.source {
            let contents = String::from_utf8_lossy(&source).into_owned();
            let syntax = syntax_for_path(
                abs_str
                    .or_else(|| {
                        raw.imp_path
                            .as_ref()
                            .and_then(|b| std::str::from_utf8(b).ok())
                    })
                    .unwrap_or(&url_text),
            );
            let srcmap = raw
                .srcmap
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned());
            let nonce = self.stash.borrow().len();
            let Some(canonical) = canonical_for(self.index, nonce, abs_str) else {
                return Err(script_error(format!(
                    "custom importer returned an unusable path for {url_text:?}"
                )));
            };
            // `prev` for nested imports (see `StoredImport::prev`).
            let prev = match abs_str {
                Some(abs) if !abs.is_empty() => abs.to_string(),
                _ => url_text.clone(),
            };
            self.stash.borrow_mut().insert(
                canonical.to_string(),
                StoredImport {
                    contents,
                    syntax,
                    srcmap,
                    prev,
                    filesystem_path: None,
                },
            );
            return Ok(Some(canonical));
        }
        if let Some(abs) = abs_str {
            if !abs.is_empty() {
                // Path-only return: resolve via the normal filesystem flow
                // (mirrors `import_url(abs)`, context.cpp:461-470). The
                // canonical URL is the file URL itself; `load` reads it.
                let parsed = SassUrl::file_url_from_abs_path(abs)
                    .ok()
                    .or_else(|| SassUrl::parse(abs).ok().filter(|u| !u.is_relative()));
                if let Some(canonical) = parsed {
                    self.stash.borrow_mut().insert(
                        canonical.to_string(),
                        StoredImport {
                            contents: String::new(),
                            syntax: Syntax::Scss,
                            srcmap: None,
                            prev: abs.to_string(),
                            filesystem_path: Some(abs.to_string()),
                        },
                    );
                    return Ok(Some(canonical));
                }
            }
        }
        Ok(None)
    }

    fn load<'a>(&'a self, url: &'a SassUrl) -> SassResult<Option<ImporterResult>> {
        let key = url.to_string();
        let stash = self.stash.borrow();
        let Some(stored) = stash.get(&key) else {
            return Ok(None);
        };
        if let Some(path) = &stored.filesystem_path {
            // Path-only delegation: read from disk here (the core gives
            // `load` no `Io`, so read directly — same real filesystem the
            // compile uses).
            match std::fs::read(path) {
                Ok(bytes) => {
                    let contents = String::from_utf8_lossy(&bytes).into_owned();
                    let syntax = syntax_for_path(path);
                    Ok(Some(ImporterResult::new(contents, syntax, None)?))
                }
                Err(_) => Ok(None),
            }
        } else {
            // `srcmap` content becomes a `data:` URL (the core models a map
            // location, not map bytes). Over-encoded vs Dart's DATA_SAFE but
            // decodes identically — valid either way.
            let srcmap_url = stored.srcmap.as_deref().and_then(|m| {
                if m.is_empty() {
                    None
                } else {
                    let encoded = percent_encoding::utf8_percent_encode(
                        m,
                        percent_encoding::NON_ALPHANUMERIC,
                    )
                    .to_string();
                    SassUrl::parse(&format!("data:;charset=utf-8,{encoded}")).ok()
                }
            });
            Ok(Some(ImporterResult::new(
                stored.contents.clone(),
                stored.syntax.clone(),
                srcmap_url,
            )?))
        }
    }
}

/// Builds core importers from a NULL-terminated C importer list (NULL list
/// → no importers). NULL callbacks fail the whole compile (upstream would
/// crash at invocation).
///
/// `token_entry` seeds every per-call compiler token (the entry frame).
///
/// Entries are stable-sorted by descending `priority`, mirroring upstream's
/// `sort_importers` (context.cpp:27-28) — stable so ties keep registration
/// order deterministically (upstream's `std::sort` leaves ties unspecified).
pub(crate) fn build_custom_importers<'compile, 'parse>(
    list: *mut *mut ImporterEntry,
    arena: &'compile Bump,
    token_entry: &ImportStackEntry,
) -> SassResult<Vec<Importer<'parse>>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let mut out = Vec::new();
    if list.is_null() {
        return Ok(out);
    }
    // SAFETY: live NULL-terminated list per contract (entries live until the
    // owning options/context is deleted — strictly longer than this compile).
    let mut indexed: Vec<(usize, *mut ImporterEntry)> = Vec::new();
    unsafe {
        let mut i = 0usize;
        while !(*list.add(i)).is_null() {
            indexed.push((i, *list.add(i)));
            i += 1;
        }
    }
    let mut with_prio: Vec<(usize, f64, *mut ImporterEntry)> = indexed
        .into_iter()
        .map(|(i, entry)| {
            // SAFETY: live entry per the walk above.
            let prio = unsafe {
                if entry.is_null() {
                    0.0
                } else {
                    (*entry).priority
                }
            };
            (i, prio, entry)
        })
        .collect();
    // Descending priority; NaN ties as equal (registration order wins).
    with_prio.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
    for (order, (_, _, entry)) in with_prio.into_iter().enumerate() {
        if entry.is_null() {
            continue;
        }
        // SAFETY: live entry per contract. The cookie flows to the consumer
        // through `entry` itself (`sass_importer_get_cookie`, as in
        // node-sass) — the bridge passes the entry through untouched.
        let callback = unsafe { (*entry).callback };
        let Some(callback) = callback else {
            return Err(script_error("custom importer has no callback".to_string()));
        };
        out.push(Importer::new(
            arena,
            ImporterKind::User(Rc::new(CImporter {
                entry,
                callback,
                index: order,
                token_entry: token_entry.clone(),
                stash: RefCell::new(HashMap::new()),
            })),
        ));
    }
    Ok(out)
}

// ── Compiler query getters ────────────────────────────────────────────────

/// Materializes the compiler's import-stack snapshot as owned C entries
/// (once per token) and returns the idx-th, or NULL when out of range.
/// Borrowed for the call; freed with the token (G3).
///
/// # Safety
///
/// `compiler` must be NULL (→ NULL) or a live token for the call.
unsafe fn compiler_import_at(compiler: *mut CompilerBox, idx: usize) -> *mut ImportEntry {
    if compiler.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: live token per contract above.
    let compiler = unsafe { &mut *compiler };
    if compiler.import_entries.is_empty() {
        for frame in &compiler.import_stack {
            let imp = CString::new(frame.imp_path.clone()).unwrap_or_default();
            let abs = CString::new(frame.abs_path.clone()).unwrap_or_default();
            // SAFETY: fresh malloc'd entry from valid inputs; owned here.
            let entry = unsafe {
                sass_make_import(imp.as_ptr(), abs.as_ptr(), ptr::null_mut(), ptr::null_mut())
            };
            if !entry.is_null() {
                compiler.import_entries.push(entry);
            }
        }
    }
    compiler
        .import_entries
        .get(idx)
        .copied()
        .unwrap_or(ptr::null_mut())
}

/// Counts the import stack (mirrors `sass_compiler_get_import_stack_size`).
/// NULL hardens to 0 (upstream would crash).
///
/// # Safety
///
/// `compiler` must be NULL or a live token from a C callback invocation.
#[no_mangle]
pub unsafe extern "C" fn sass_compiler_get_import_stack_size(compiler: *mut CompilerBox) -> usize {
    guard(0, || {
        if compiler.is_null() {
            return 0;
        }
        // SAFETY: live token per contract above.
        unsafe { (*compiler).import_stack.len() }
    })
}

/// Returns the top of the import stack (borrowed for the call; NULL when the
/// stack is empty — hardening over upstream's unchecked back-deref).
///
/// # Safety
///
/// `compiler` must be NULL or a live token from a C callback invocation.
/// Borrowed; freed with the token — never delete it.
#[no_mangle]
pub unsafe extern "C" fn sass_compiler_get_last_import(
    compiler: *mut CompilerBox,
) -> *mut ImportEntry {
    guard(ptr::null_mut(), || {
        if compiler.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live token per contract above.
        let len = unsafe { (*compiler).import_stack.len() };
        if len == 0 {
            return ptr::null_mut();
        }
        // SAFETY: in range per the length check above.
        unsafe { compiler_import_at(compiler, len - 1) }
    })
}

/// Returns the idx-th import-stack entry (borrowed; NULL when out of range —
/// hardening over upstream's unchecked index).
///
/// # Safety
///
/// See [`sass_compiler_get_last_import`].
#[no_mangle]
pub unsafe extern "C" fn sass_compiler_get_import_entry(
    compiler: *mut CompilerBox,
    idx: usize,
) -> *mut ImportEntry {
    guard(ptr::null_mut(), || {
        // SAFETY: NULL/live per contract; out-of-range hardens to NULL.
        unsafe { compiler_import_at(compiler, idx) }
    })
}

/// Resolves `file` against `dirs` (in order), probing the libsass filename
/// conventions: exact, `*.{scss,sass,css}`, `_partial` variants. Returns the
/// first hit, else None. Best-effort subset of upstream's `File::find_*`
/// (no `index` files, no custom extensions — documented).
fn probe_file(file: &str, dirs: &[String]) -> Option<String> {
    if file.is_empty() {
        return None;
    }
    let mut candidates: Vec<String> = Vec::new();
    candidates.push(file.to_string());
    if Path::new(file).extension().is_none() {
        for ext in ["scss", "sass", "css"] {
            candidates.push(format!("{file}.{ext}"));
        }
        if let Some(name) = Path::new(file).file_name().and_then(|n| n.to_str()) {
            let parent = Path::new(file)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            for ext in ["scss", "sass", "css"] {
                if parent.is_empty() {
                    candidates.push(format!("_{name}.{ext}"));
                } else {
                    candidates.push(format!("{parent}/_{name}.{ext}"));
                }
            }
        }
    }
    for dir in dirs {
        for cand in &candidates {
            let joined = if dir.is_empty() {
                cand.clone()
            } else {
                format!("{dir}/{cand}")
            };
            if Path::new(&joined).is_file() {
                return Some(joined);
            }
        }
    }
    None
}

/// Reads the borrowed `file` argument lossily (NULL → None).
///
/// # Safety
///
/// `s` must be NULL or a valid NUL-terminated string.
unsafe fn read_path(s: *const c_char) -> Option<String> {
    if s.is_null() {
        return None;
    }
    // SAFETY: per contract above.
    Some(String::from_utf8_lossy(unsafe { CStr::from_ptr(s) }.to_bytes()).into_owned())
}

/// Collects lookup dirs: last-import dir first (compiler variants), then the
/// options' include paths (joined + pushed), then CWD-last (empty = relative).
///
/// # Safety
///
/// `compiler` must be NULL or a live token; `opt` must be NULL or live
/// options (reads only).
unsafe fn lookup_dirs(compiler: *mut CompilerBox, opt: *const OptionsBox) -> Vec<String> {
    let mut dirs = Vec::new();
    if !compiler.is_null() {
        // SAFETY: live token per caller contract.
        let stack = unsafe { &(*compiler).import_stack };
        if let Some(top) = stack.last() {
            let base = top.abs_path.clone();
            // `file://` URLs → filesystem path; else dir_name of the raw path.
            let dir = base
                .strip_prefix("file://")
                .map(|p| p.to_string())
                .unwrap_or(base);
            let dir = Path::new(&dir)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !dir.is_empty() {
                dirs.push(dir);
            }
        }
    }
    if !opt.is_null() {
        // SAFETY: live options per caller contract; string reads only.
        let opt = unsafe { &*opt };
        // SAFETY: options live per caller contract.
        if let Some(joined) = unsafe { opt.include_path.bytes() } {
            if let Ok(text) = std::str::from_utf8(joined) {
                dirs.extend(
                    text.split(':')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string()),
                );
            }
        }
        for entry in &opt.include_paths {
            // SAFETY: entries are owned NUL-terminated strings per construction.
            if let Some(bytes) = unsafe { read_field(entry.ptr as *const c_char) } {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    if !text.is_empty() {
                        dirs.push(text.to_string());
                    }
                }
            }
        }
    }
    dirs.push(String::new());
    dirs
}

/// Finds `file` against the options' include paths (mirrors
/// `sass_find_file`/`sass_find_include`, sass.cpp:107-121 — minus the
/// import-stack base, which only the compiler variants have). Returns a fresh
/// copy (caller frees); a miss returns an empty-string copy, mirroring
/// upstream's never-NULL miss shape.
///
/// # Safety
///
/// `file` must be NULL (→ empty copy) or a valid string; `opt` must be NULL
/// or live options.
fn find_with_dirs(file: *const c_char, dirs: Vec<String>) -> *mut c_char {
    guard(copy_bytes_nul(b""), || {
        // SAFETY: NULL/valid per contract above.
        let Some(name) = (unsafe { read_path(file) }) else {
            return copy_bytes_nul(b"");
        };
        match probe_file(&name, &dirs) {
            Some(hit) => copy_bytes_nul(hit.as_bytes()),
            None => copy_bytes_nul(b""),
        }
    })
}

/// Resolves `path` against the options' include paths (see `find_with_dirs`).
///
/// # Safety
///
/// `path` must be NULL or a valid NUL-terminated string; `opt` must be NULL
/// or live options from `sass_make_options`. Caller owns the result (frees
/// with `sass_free_memory`).
#[no_mangle]
pub unsafe extern "C" fn sass_find_file(path: *const c_char, opt: *mut OptionsBox) -> *mut c_char {
    // SAFETY: per contract above (reads only).
    let dirs = unsafe { lookup_dirs(ptr::null_mut(), opt as *const OptionsBox) };
    find_with_dirs(path, dirs)
}

/// Resolves `path` against the options' include paths (same shape as
/// [`sass_find_file`]; upstream's `find_include` differs only in CSS-import
/// handling, which has no meaning outside a compile).
///
/// # Safety
///
/// See [`sass_find_file`].
#[no_mangle]
pub unsafe extern "C" fn sass_find_include(
    path: *const c_char,
    opt: *mut OptionsBox,
) -> *mut c_char {
    // SAFETY: per contract above (reads only).
    let dirs = unsafe { lookup_dirs(ptr::null_mut(), opt as *const OptionsBox) };
    find_with_dirs(path, dirs)
}

/// Resolves `path` against the last-import dir + include paths (mirrors
/// `sass_compiler_find_file`, sass.cpp:90-102; never derefs an empty stack
/// like upstream — falls back to include paths + CWD).
///
/// # Safety
///
/// `path` must be NULL or a valid string; `compiler` must be NULL or a live
/// token. Caller owns the result. The include paths come from the ambient
/// process CWD only (the token carries no options pointer) — documented.
#[no_mangle]
pub unsafe extern "C" fn sass_compiler_find_file(
    path: *const c_char,
    compiler: *mut CompilerBox,
) -> *mut c_char {
    // SAFETY: per contract above (reads only).
    let dirs = unsafe { lookup_dirs(compiler, ptr::null()) };
    find_with_dirs(path, dirs)
}

/// Resolves `path` against the last-import dir + include paths (same shape
/// as [`sass_compiler_find_file`]).
///
/// # Safety
///
/// See [`sass_compiler_find_file`].
#[no_mangle]
pub unsafe extern "C" fn sass_compiler_find_include(
    path: *const c_char,
    compiler: *mut CompilerBox,
) -> *mut c_char {
    // SAFETY: per contract above (reads only).
    let dirs = unsafe { lookup_dirs(compiler, ptr::null()) };
    find_with_dirs(path, dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::sass_copy_c_string;
    use crate::base::sass_free_memory;
    use crate::functions::ImportStackEntry;

    fn test_arena() -> Bump {
        Bump::new()
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
    fn entry_lifecycle_and_error_mapping() {
        unsafe {
            // List + importer entries.
            let list = sass_make_importer_list(2);
            assert!(!list.is_null());
            let e1 = sass_make_importer(None, 1.5, ptr::null_mut());
            assert!(!e1.is_null());
            assert_eq!(sass_importer_get_priority(e1), 1.5);
            assert!(sass_importer_get_function(e1).is_none());
            assert!(sass_importer_get_cookie(e1).is_null());
            sass_importer_set_list_entry(list, 0, e1);
            assert_eq!(sass_importer_get_list_entry(list, 0), e1);
            assert!(sass_importer_get_list_entry(ptr::null_mut(), 0).is_null());
            sass_importer_set_list_entry(ptr::null_mut(), 0, ptr::null_mut());
            sass_delete_importer_list(list);

            // Import entry: paths copied, buffers adopted.
            let p = CString::new("p").unwrap();
            let s = CString::new("x").unwrap();
            let sc = CString::new("m").unwrap();
            let e = sass_make_import(
                p.as_ptr(),
                p.as_ptr(),
                sass_copy_c_string(s.as_ptr()),
                sass_copy_c_string(sc.as_ptr()),
            );
            assert_eq!(read_opt(sass_import_get_imp_path(e)), Some(b"p".to_vec()));
            assert_eq!(read_opt(sass_import_get_abs_path(e)), Some(b"p".to_vec()));
            assert_eq!(read_opt(sass_import_get_source(e)), Some(b"x".to_vec()));
            assert_eq!(read_opt(sass_import_get_srcmap(e)), Some(b"m".to_vec()));
            // take_* detach.
            let taken = sass_import_take_source(e);
            assert!(!taken.is_null());
            assert!(sass_import_get_source(e).is_null());
            sass_free_memory(taken as *mut c_void);
            let taken = sass_import_take_srcmap(e);
            assert!(!taken.is_null());
            assert!(sass_import_get_srcmap(e).is_null());
            sass_free_memory(taken as *mut c_void);
            // set_error: falsy 0 → -1 (usize::MAX).
            let m = CString::new("bad").unwrap();
            sass_import_set_error(e, m.as_ptr(), 0, 0);
            assert_eq!(sass_import_get_error_line(e), usize::MAX);
            assert_eq!(sass_import_get_error_column(e), usize::MAX);
            assert_eq!(
                read_opt(sass_import_get_error_message(e)),
                Some(b"bad".to_vec())
            );
            sass_delete_import(e);
            sass_delete_import(ptr::null_mut());
            sass_delete_import_list(ptr::null_mut());
            sass_delete_importer(ptr::null_mut());
        }
    }

    #[test]
    fn build_empty_and_null_callback() {
        let arena = test_arena();
        let entry = ImportStackEntry {
            imp_path: "stdin".to_string(),
            abs_path: "stdin".to_string(),
        };
        let empty = build_custom_importers(ptr::null_mut(), &arena, &entry).unwrap();
        assert!(empty.is_empty());
        drop(empty);
        unsafe {
            let list = sass_make_importer_list(1);
            let cb_entry = sass_make_importer(None, 0.0, ptr::null_mut());
            sass_importer_set_list_entry(list, 0, cb_entry);
            assert!(build_custom_importers(list, &arena, &entry).is_err());
            sass_delete_importer_list(list);
        }
    }

    #[test]
    fn compiler_token_queries_harden_null() {
        unsafe {
            assert_eq!(sass_compiler_get_import_stack_size(ptr::null_mut()), 0);
            assert!(sass_compiler_get_last_import(ptr::null_mut()).is_null());
            assert!(sass_compiler_get_import_entry(ptr::null_mut(), 0).is_null());
            let mut compiler = CompilerBox::new();
            assert_eq!(sass_compiler_get_import_stack_size(&mut compiler), 0);
            assert!(sass_compiler_get_last_import(&mut compiler).is_null());
            assert!(sass_compiler_get_import_entry(&mut compiler, 3).is_null());
        }
    }

    #[test]
    fn canonical_shape_helpers() {
        // Absolute paths become file: URLs; garbage falls back to synthetic.
        let url = canonical_for(0, 0, Some("/tmp/x.scss")).unwrap();
        assert_eq!(url.scheme(), "file");
        let url = canonical_for(1, 7, None).unwrap();
        assert_eq!(url.to_string(), "sass-c-importer://importer-1/7");
        // Priority sort is descending and stable (unit-level pin of D16).
        let arena = test_arena();
        unsafe {
            let list = sass_make_importer_list(3);
            extern "C" fn cb(
                _u: *const c_char,
                _e: *mut ImporterEntry,
                _c: *mut CompilerBox,
            ) -> *mut *mut ImportEntry {
                ptr::null_mut()
            }
            for (i, p) in [1.0, 10.0, 1.0].iter().enumerate() {
                let e = sass_make_importer(Some(cb), *p, ptr::null_mut());
                sass_importer_set_list_entry(list, i, e);
            }
            let token_entry = ImportStackEntry {
                imp_path: "stdin".to_string(),
                abs_path: "stdin".to_string(),
            };
            let built = build_custom_importers(list, &arena, &token_entry).unwrap();
            assert_eq!(built.len(), 3);
            sass_delete_importer_list(list);
        }
    }
}
