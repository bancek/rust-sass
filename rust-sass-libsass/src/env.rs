// Copyright 2012-2016 Sass Open Source Foundation. Use of this source code
// is governed by an MIT-style license that can be found in the LICENSE
// file or at https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// libsass-source: src/sass_functions.cpp (env bridge) + src/eval.cpp (callee push sites) + src/expand.cpp (mixin sites) + include/sass/functions.h (callee/env surface)

use crate::values::SassValue;
use std::ffi::{c_char, CStr};
use std::ptr;

use bumpalo::Bump;

use rust_sass::ast::sass::statement::callable_declaration::CallableDeclaration;
use rust_sass::callable::CallableKind;
use rust_sass::common::file_span::{FileSpan, BOGUS_SPAN};
use rust_sass::environment::Environment;
use rust_sass::eval::EvalState;

use crate::base::{copy_bytes_nul, guard};
use crate::functions::{c_to_core, core_to_c, CompilerBox};

/// C ABI callee-type constants, matching `Sass_Callee_Type` numeric order
/// (`MIXIN=0, FUNCTION=1, C_FUNCTION=2`).
pub const SASS_CALLEE_MIXIN: u32 = 0;
pub const SASS_CALLEE_FUNCTION: u32 = 1;
pub const SASS_CALLEE_C_FUNCTION: u32 = 2;

/// One snapshotted callee frame: owned strings + type (plan G3 — the core
/// `StackFrame` chain is evaluator-owned and mutates on return, so every
/// field is copied at callback entry).
#[derive(Clone, Debug)]
pub(crate) struct CalleeSnapshot {
    name: String,
    path: String,
    line: usize,
    column: usize,
    ctype: u32,
}

/// The `Sass_Env` heap struct: a live environment handle plus the arena for
/// materializing C-origin values. All callee envs of one callback alias the
/// same live env (upstream gives each frame its own nesting level — the core
/// `StackFrame` chain carries no per-frame envs, so the innermost env is
/// exact and parents alias it; documented divergence, invisible to every
/// surveyed consumer).
pub struct SassEnvBox {
    env: Option<Environment<'static, 'static>>,
    arena: Option<&'static Bump>,
}

/// One materialized callee entry. Mirrors `Sass_Callee`
/// (sass_functions.hpp): borrowed name/path, 1-based line/column, type, and
/// the frame env. Owned by the `CompilerBox` (freed with the token — G3).
#[repr(C)]
pub struct SassCalleeBox {
    name: *mut c_char,
    path: *mut c_char,
    line: usize,
    column: usize,
    ctype: u32,
    env_box: *mut SassEnvBox,
}

/// Frees one env box (struct only — the env handle and arena are borrowed,
/// owned by the compile).
///
/// # Safety
///
/// `entry` must be NULL or a live box, freed exactly once here.
pub(crate) unsafe fn free_env_box(entry: *mut SassEnvBox) {
    if entry.is_null() {
        return;
    }
    // SAFETY: live box per contract; fields are borrows, nothing to drop.
    unsafe {
        drop(Box::from_raw(entry));
    }
}

/// Frees one callee entry (strings + struct; never the env box — owned by
/// the token's `env_boxes` list).
///
/// # Safety
///
/// `entry` must be NULL or a live entry, freed exactly once here.
pub(crate) unsafe fn free_callee_entry(entry: *mut SassCalleeBox) {
    if entry.is_null() {
        return;
    }
    // SAFETY: live entry per contract.
    unsafe {
        let owned = Box::from_raw(entry);
        for slot in [owned.name, owned.path] {
            if !slot.is_null() {
                libc::free(slot as *mut libc::c_void);
            }
        }
    }
}

/// Classifies one core stack frame, or `None` for import/load frames (which
/// carry the `dummy` placeholder and have no libsass counterpart — upstream
/// never pushes them; skipping keeps chain shapes identical).
fn frame_type(callable: &rust_sass::callable::Callable<'_, '_>) -> Option<u32> {
    match callable.kind() {
        CallableKind::UserDefined(u) => Some(match &u.declaration {
            CallableDeclaration::Mixin(_) => SASS_CALLEE_MIXIN,
            CallableDeclaration::Function(_) | CallableDeclaration::ContentBlock(_) => {
                SASS_CALLEE_FUNCTION
            }
        }),
        CallableKind::BuiltIn(_) => Some(SASS_CALLEE_FUNCTION),
        // The `dummy` placeholder (helpers.rs): import/load frames. Any other
        // plain-CSS fallback is a real function frame.
        CallableKind::PlainCss(p) if p.name == "dummy" => None,
        CallableKind::PlainCss(_) => Some(SASS_CALLEE_FUNCTION),
    }
}

/// Reads a frame span as `(path, 1-based line, 1-based column)` (empty path
/// when the span has no URL — e.g. `stdin` data compiles).
fn span_location(span: &FileSpan<'_>) -> (String, usize, usize) {
    let path = span.source_url().map(|u| u.to_string()).unwrap_or_default();
    let loc = span.start_location();
    (path, loc.line + 1, loc.column + 1)
}

/// Snapshots the callee chain at callback entry, outermost-first, with the
/// current C function synthesized on top (the core pushes no frame for
/// built-in invocations; upstream pushes `{name, call-site, C_FUNCTION}`
/// in `eval.cpp:1089-1107`).
///
/// `c_name` is the called C function name; the call-site span comes from
/// `state.callable_span` (set by `invoke_callable`), falling back to the
/// default warn span.
pub(crate) fn snapshot_callees(state: &EvalState<'_, '_>, c_name: &str) -> Vec<CalleeSnapshot> {
    let mut frames: Vec<CalleeSnapshot> = Vec::new();
    let mut current = state.stack.as_ref();
    while let Some(frame) = current {
        // NOTE: `frame.name` is the *caller's* member (`with_stack_frame`
        // stores the previous member); the callee name comes from the frame
        // callable — matching upstream, which stores the called name.
        if let Some(ctype) = frame_type(&frame.callable) {
            let (path, line, column) = span_location(&frame.span);
            frames.push(CalleeSnapshot {
                name: frame.callable.name().to_string(),
                path,
                line,
                column,
                ctype,
            });
        }
        current = frame.parent.as_ref();
    }
    frames.reverse();
    let call_site = state.callable_span.unwrap_or(state.default_warn_span);
    let (path, line, column) = span_location(&call_site);
    frames.push(CalleeSnapshot {
        name: c_name.to_string(),
        path,
        line,
        column,
        ctype: SASS_CALLEE_C_FUNCTION,
    });
    frames
}

/// Strips one leading `$` for the core's bare env keys (upstream
/// `lex_variable` keeps it — probed on the upstream leg during the step-8
/// build). Accepts both spellings; a bare name passes through.
fn normalize_name(name: &[u8]) -> String {
    let text = String::from_utf8_lossy(name).into_owned();
    text.strip_prefix('$').unwrap_or(&text).to_string()
}

/// Reads a borrowed variable-name argument (NULL → None).
///
/// # Safety
///
/// `s` must be NULL or a valid NUL-terminated string.
unsafe fn read_name(s: *const c_char) -> Option<String> {
    if s.is_null() {
        return None;
    }
    // SAFETY: per contract above.
    Some(normalize_name(unsafe { CStr::from_ptr(s) }.to_bytes()))
}

/// Materializes the token's callee snapshot as owned C entries (once per
/// token) plus one shared env box, and returns the idx-th entry or NULL.
///
/// # Safety
///
/// `compiler` must be NULL (→ NULL) or a live token for the call. Borrowed
/// for the call; freed with the token (G3).
unsafe fn callee_at(compiler: *mut CompilerBox, idx: usize) -> *mut SassCalleeBox {
    if compiler.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: live token per contract above.
    let compiler = unsafe { &mut *compiler };
    if compiler.callee_entries.is_empty() && !compiler.callees.is_empty() {
        // One env box shared by every entry (all alias the live callback
        // env — see `SassEnvBox`).
        // SAFETY: fresh box; owned by the token.
        let env_box = Box::into_raw(Box::new(SassEnvBox {
            env: compiler.live_env,
            arena: compiler.arena,
        }));
        compiler.env_boxes.push(env_box);
        for snap in &compiler.callees {
            // SAFETY: fresh entry; strings copied, owned here.
            let entry = Box::into_raw(Box::new(SassCalleeBox {
                name: copy_bytes_nul(snap.name.as_bytes()),
                path: copy_bytes_nul(snap.path.as_bytes()),
                line: snap.line,
                column: snap.column,
                ctype: snap.ctype,
                env_box,
            }));
            compiler.callee_entries.push(entry);
        }
    }
    compiler
        .callee_entries
        .get(idx)
        .copied()
        .unwrap_or(ptr::null_mut())
}

/// Counts the callee stack (mirrors `sass_compiler_get_callee_stack_size`).
/// NULL hardens to 0 (upstream would crash).
///
/// # Safety
///
/// `compiler` must be NULL or a live token from a C callback invocation.
#[no_mangle]
pub unsafe extern "C" fn sass_compiler_get_callee_stack_size(compiler: *mut CompilerBox) -> usize {
    guard(0, || {
        if compiler.is_null() {
            return 0;
        }
        // SAFETY: live token per contract above.
        unsafe { (*compiler).callees.len() }
    })
}

/// Returns the innermost callee (the running C function; borrowed for the
/// call, NULL when no C callback is active — hardening over upstream's
/// unchecked back-deref).
///
/// # Safety
///
/// `compiler` must be NULL or a live token. Borrowed; freed with the token
/// — never delete it.
#[no_mangle]
pub unsafe extern "C" fn sass_compiler_get_last_callee(
    compiler: *mut CompilerBox,
) -> *mut SassCalleeBox {
    guard(ptr::null_mut(), || {
        if compiler.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live token per contract above.
        let len = unsafe { (*compiler).callees.len() };
        if len == 0 {
            return ptr::null_mut();
        }
        // SAFETY: in range per the length check above.
        unsafe { callee_at(compiler, len - 1) }
    })
}

/// Returns the idx-th callee, outermost-first (borrowed; NULL when out of
/// range — hardening over upstream's unchecked index).
///
/// # Safety
///
/// See [`sass_compiler_get_last_callee`].
#[no_mangle]
pub unsafe extern "C" fn sass_compiler_get_callee_entry(
    compiler: *mut CompilerBox,
    idx: usize,
) -> *mut SassCalleeBox {
    guard(ptr::null_mut(), || {
        // SAFETY: NULL/live per contract; out-of-range hardens to NULL.
        unsafe { callee_at(compiler, idx) }
    })
}

macro_rules! callee_getter {
    ($get:ident, $field:ident, $ty:ty, $safety:expr) => {
        /// Reads the callee field; NULL entries harden to the type default.
        ///
        /// # Safety
        ///
        #[doc = $safety]
        #[no_mangle]
        pub unsafe extern "C" fn $get(entry: *mut SassCalleeBox) -> $ty {
            guard(<$ty>::default(), || {
                if entry.is_null() {
                    return <$ty>::default();
                }
                // SAFETY: live entry per contract above.
                unsafe { (*entry).$field as $ty }
            })
        }
    };
}

callee_getter!(
    sass_callee_get_line,
    line,
    usize,
    "`entry` must be NULL or a live entry for the call."
);
callee_getter!(
    sass_callee_get_column,
    column,
    usize,
    "`entry` must be NULL or a live entry for the call."
);
callee_getter!(
    sass_callee_get_type,
    ctype,
    u32,
    "`entry` must be NULL or a live entry for the call."
);

/// Reads the callee name (borrowed; NULL entry hardens to NULL).
///
/// # Safety
///
/// `entry` must be NULL or a live entry for the call. Borrowed; freed with
/// the compiler token.
#[no_mangle]
pub unsafe extern "C" fn sass_callee_get_name(entry: *mut SassCalleeBox) -> *const c_char {
    guard(ptr::null(), || {
        if entry.is_null() {
            return ptr::null();
        }
        // SAFETY: live entry per contract above.
        unsafe { (*entry).name as *const c_char }
    })
}

/// Reads the callee path (borrowed; NULL entry hardens to NULL).
///
/// # Safety
///
/// See [`sass_callee_get_name`].
#[no_mangle]
pub unsafe extern "C" fn sass_callee_get_path(entry: *mut SassCalleeBox) -> *const c_char {
    guard(ptr::null(), || {
        if entry.is_null() {
            return ptr::null();
        }
        // SAFETY: live entry per contract above.
        unsafe { (*entry).path as *const c_char }
    })
}

/// Reads the frame env (borrowed for the call; NULL entry hardens to NULL —
/// upstream returns `&entry->env` unconditionally, crashing on NULL).
///
/// # Safety
///
/// `entry` must be NULL or a live entry for the call. Valid only during the
/// callback (like upstream's pop-dangled pointer — never stash it).
#[no_mangle]
pub unsafe extern "C" fn sass_callee_get_env(entry: *mut SassCalleeBox) -> *mut SassEnvBox {
    guard(ptr::null_mut(), || {
        if entry.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live entry per contract above.
        unsafe { (*entry).env_box }
    })
}

/// Reads the live env + arena out of a box (both `None` outside a C
/// callback — e.g. an importer token — hardening to NULL/no-op).
///
/// # Safety
///
/// `env` must be NULL or a live box for the call.
unsafe fn live_env(env: *mut SassEnvBox) -> Option<(Environment<'static, 'static>, &'static Bump)> {
    if env.is_null() {
        return None;
    }
    // SAFETY: live box per contract above.
    unsafe { (*env).env.zip((*env).arena) }
}

/// Converts a core value to a fresh C value, or NULL on unrepresentable
/// kinds (upstream would hand back an error value mid-chain; at the env
/// boundary NULL is the documented miss shape).
fn core_to_c_or_null(v: rust_sass::value::Value<'static>) -> *mut SassValue {
    match core_to_c(v) {
        // SAFETY: `core_to_c` returns fresh malloc'd values.
        Ok(ptr) => ptr,
        Err(_) => ptr::null_mut(),
    }
}

/// Reads a variable through the full lexical scope (mirrors
/// `sass_env_get_lexical`, sass_functions.cpp:172-175 — minus the
/// default-insert: misses read NULL without polluting the env, plan G4).
/// Returns a fresh value (caller deletes) or NULL.
///
/// # Safety
///
/// `env` must be NULL or a live box; `name` NULL or a valid string.
/// `$`-prefixed and bare names both accepted.
#[no_mangle]
pub unsafe extern "C" fn sass_env_get_lexical(
    env: *mut SassEnvBox,
    name: *const c_char,
) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        // SAFETY: per contract above.
        let Some((live, _)) = (unsafe { live_env(env) }) else {
            return ptr::null_mut();
        };
        let Some(key) = (unsafe { read_name(name) }) else {
            return ptr::null_mut();
        };
        match live.get_variable(&key, None) {
            Ok(Some(v)) => core_to_c_or_null(v),
            _ => ptr::null_mut(),
        }
    })
}

/// Writes through to the found frame, else the current frame (mirrors
/// `sass_env_set_lexical` = `operator[]` store, sass_functions.cpp:177-179;
/// the core `set_variable` resolves the same way). Module-conflict errors
/// have no channel here and are swallowed (documented).
///
/// # Safety
///
/// `env` must be NULL (safe no-op) or a live box; `name` NULL (no-op) or a
/// valid string; `val` NULL (no-op) or a live value (never stolen).
#[no_mangle]
pub unsafe extern "C" fn sass_env_set_lexical(
    env: *mut SassEnvBox,
    name: *const c_char,
    val: *mut SassValue,
) {
    guard((), || {
        // SAFETY: per contract above.
        let Some((live, arena)) = (unsafe { live_env(env) }) else {
            return;
        };
        let (Some(key), false) = (unsafe { read_name(name) }, val.is_null()) else {
            return;
        };
        let Ok(value) = c_to_core(val as *const SassValue, arena) else {
            return;
        };
        let _ = live.set_variable(&key, value, BOGUS_SPAN, None, false);
    });
}

/// Reads the current frame only (mirrors `sass_env_get_local`).
/// Fresh value or NULL.
///
/// # Safety
///
/// See [`sass_env_get_lexical`].
#[no_mangle]
pub unsafe extern "C" fn sass_env_get_local(
    env: *mut SassEnvBox,
    name: *const c_char,
) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        // SAFETY: per contract above.
        let Some((live, _)) = (unsafe { live_env(env) }) else {
            return ptr::null_mut();
        };
        let Some(key) = (unsafe { read_name(name) }) else {
            return ptr::null_mut();
        };
        match live.get_local_variable(&key) {
            Some(v) => core_to_c_or_null(v),
            None => ptr::null_mut(),
        }
    })
}

/// Writes the current frame unconditionally (mirrors `sass_env_set_local`).
///
/// # Safety
///
/// See [`sass_env_set_lexical`].
#[no_mangle]
pub unsafe extern "C" fn sass_env_set_local(
    env: *mut SassEnvBox,
    name: *const c_char,
    val: *mut SassValue,
) {
    guard((), || {
        // SAFETY: per contract above.
        let Some((live, arena)) = (unsafe { live_env(env) }) else {
            return;
        };
        let (Some(key), false) = (unsafe { read_name(name) }, val.is_null()) else {
            return;
        };
        let Ok(value) = c_to_core(val as *const SassValue, arena) else {
            return;
        };
        live.set_local_variable(&key, value, BOGUS_SPAN);
    });
}

/// Reads from the global frame down (mirrors `sass_env_get_global`).
/// Fresh value or NULL.
///
/// # Safety
///
/// See [`sass_env_get_lexical`].
#[no_mangle]
pub unsafe extern "C" fn sass_env_get_global(
    env: *mut SassEnvBox,
    name: *const c_char,
) -> *mut SassValue {
    guard(ptr::null_mut(), || {
        // SAFETY: per contract above.
        let Some((live, _)) = (unsafe { live_env(env) }) else {
            return ptr::null_mut();
        };
        let Some(key) = (unsafe { read_name(name) }) else {
            return ptr::null_mut();
        };
        match live.get_global_variable(&key) {
            Ok(Some(v)) => core_to_c_or_null(v),
            _ => ptr::null_mut(),
        }
    })
}

/// Writes the global frame unconditionally (mirrors `sass_env_set_global`;
/// module-conflict errors swallowed, see [`sass_env_set_lexical`]).
///
/// # Safety
///
/// See [`sass_env_set_lexical`].
#[no_mangle]
pub unsafe extern "C" fn sass_env_set_global(
    env: *mut SassEnvBox,
    name: *const c_char,
    val: *mut SassValue,
) {
    guard((), || {
        // SAFETY: per contract above.
        let Some((live, arena)) = (unsafe { live_env(env) }) else {
            return;
        };
        let (Some(key), false) = (unsafe { read_name(name) }, val.is_null()) else {
            return;
        };
        let Ok(value) = c_to_core(val as *const SassValue, arena) else {
            return;
        };
        let _ = live.set_variable(&key, value, BOGUS_SPAN, None, true);
    });
}

/// Normalizes C variable names for the core's bare keys (one leading `$`
/// stripped; bare names pass through).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_normalization() {
        assert_eq!(normalize_name(b"$g"), "g");
        assert_eq!(normalize_name(b"g"), "g");
        assert_eq!(normalize_name(b"$$g"), "$g");
        assert_eq!(normalize_name(b""), "");
    }

    #[test]
    fn null_hardening() {
        unsafe {
            assert_eq!(sass_compiler_get_callee_stack_size(ptr::null_mut()), 0);
            assert!(sass_compiler_get_last_callee(ptr::null_mut()).is_null());
            assert!(sass_compiler_get_callee_entry(ptr::null_mut(), 0).is_null());
            assert!(sass_callee_get_name(ptr::null_mut()).is_null());
            assert!(sass_callee_get_path(ptr::null_mut()).is_null());
            assert_eq!(sass_callee_get_line(ptr::null_mut()), 0);
            assert_eq!(sass_callee_get_column(ptr::null_mut()), 0);
            assert_eq!(sass_callee_get_type(ptr::null_mut()), 0);
            assert!(sass_callee_get_env(ptr::null_mut()).is_null());
            assert!(sass_env_get_lexical(ptr::null_mut(), ptr::null()).is_null());
            assert!(sass_env_get_local(ptr::null_mut(), ptr::null()).is_null());
            assert!(sass_env_get_global(ptr::null_mut(), ptr::null()).is_null());
            sass_env_set_lexical(ptr::null_mut(), ptr::null(), ptr::null_mut());
            sass_env_set_local(ptr::null_mut(), ptr::null(), ptr::null_mut());
            sass_env_set_global(ptr::null_mut(), ptr::null(), ptr::null_mut());
            free_env_box(ptr::null_mut());
            free_callee_entry(ptr::null_mut());
        }
    }
}
