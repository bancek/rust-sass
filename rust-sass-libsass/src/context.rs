// Copyright 2012-2016 Sass Open Source Foundation. Use of this source code
// is governed by an MIT-style license that can be found in the LICENSE
// file or at https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// libsass-source: src/sass_context.cpp (contexts/results/errors/staged compiler) + src/context.cpp (compile pipeline) + include/sass/context.h

//! Compile contexts: `Sass_File_Context` / `Sass_Data_Context` heap structs,
//! the one-shot `sass_compile_*_context` entry points, the staged
//! `sass_make_*_compiler` / `parse` / `execute` lifecycle, and result
//! getters plus `take_*` transfer.
//!
//! Mirrors `sass/context.h`: `sass_make_file/data_context`,
//! `sass_delete_file/data_context`, the `*_get_context/options` upcasts,
//! `*_set_options` moves, `sass_compile_file/data_context`, the staged
//! compiler entry points and getters, the `take_*` family, and the result
//! getters (`error_*`, `output_string`, `source_map_string`, `included_files`
//! + size).
//!
//! # Ownership (mirrors libsass, plan §A.2 with hardening)
//!
//! - Each context embeds an [`OptionsBox`](crate::options::OptionsBox) inline
//!   (mirroring `Sass_Context : Sass_Options` inheritance): the
//!   `*_get_options` getters return a pointer to the embedded struct, and
//!   `*_set_options` moves the caller's options in via
//!   [`move_options_into`](crate::options::move_options_into) (plan G2).
//! - The data context owns its source buffer (transferred at make time, freed
//!   on delete — freed with the context even if compilation never runs).
//! - Result strings (`output_string`, `source_map_string`, `error_*`,
//!   `included_files`) are owned by the context and live until delete; the
//!   getters borrow them (never transfer).
//! - Compile is one-shot and synchronous: each `sass_compile_*` call runs the
//!   full pipeline on the calling thread via `block_on` (the core is sync by
//!   default; plan D9). Results are recomputed per call, replacing previous
//!   result strings.
//!
//! # Compile mapping (plan §9 + §5.9)
//!
//! - File contexts compile through [`compile`](rust_sass::compile::compile)
//!   (real filesystem via [`DefaultIo`](rust_sass::io::DefaultIo)); data
//!   contexts through
//!   [`compile_string`](rust_sass::compile::compile_string).
//! - Style: NESTED→Nested (exact libsass match via evaluator-stamped
//!   tabs), EXPANDED→Expanded, COMPACT→Compact (one block per line),
//!   COMPRESSED→Compressed. Precision: accepted, ignored (plan D4).
//!   `source_comments` honored (D6 lifted — `/* line N, path */` comments).
//!   `include_path` (joined) + pushed `include_paths` both feed
//!   `load_paths`; `is_indented_syntax_src` selects the Sass syntax.
//! - Source maps: enabled when `source_map_file` is set or `source_map_embed`
//!   is on (libsass gates on exactly this); `omit_source_map_url` suppresses
//!   the comment; `embed` emits a base64 data-URL comment; otherwise the file
//!   comment is appended. JSON keys mirror libsass
//!   (`version/file/sourceRoot?/sources/sourcesContent?/names/mappings`).
//! - Errors: `error_message` is the formatted trace
//!   (`to_error_string`, ASCII glyphs — the C API has no unicode/color
//!   negotiation); `error_text` the raw message; `error_json` the
//!   `{status,file,line,column,message,formatted}` object (plan D14);
//!   `error_file/line/column/src` from the primary span when present;
//!   `status` is always 1 for Sass errors. On failure the output and map
//!   strings are forced NULL (mirroring `handle_error`).

use crate::base::malloc_or_abort;
use crate::functions::build_custom_callables;
use crate::functions::FunctionEntry;
use crate::importers::build_custom_importers;
use crate::importers::ImporterEntry;
use std::ffi::{c_char, c_int, CStr};
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::ptr;
use std::rc::Rc;
use std::sync::Mutex;

use base64::Engine as _;
use bumpalo::Bump;
use serde::Serialize;
use std::collections::HashSet;

use rust_sass::ast::sass::statement::stylesheet::Stylesheet;
use rust_sass::ast::sass::visitor::find_dependencies::find_dependencies;
use rust_sass::common::exception::{SassError, SassResult, Trace};
use rust_sass::compile::{decode_source_utf8, CompileOptions, CompileResult};
use rust_sass::eval::import_cache::ImportCache;
use rust_sass::eval::importer::{FilesystemImporter, Importer, ImporterKind};
use rust_sass::eval::syntax::syntax_for_path;
use rust_sass::io::{DefaultIo, Io};
use rust_sass::logger::{Logger, NoOpWarnLogger};
use rust_sass::parse::stylesheet::{SassIndentState, Syntax};
use rust_sass::serialize::OutputStyle;
use rust_sass::url::SassUrl;

use crate::base::{copy_bytes_nul, guard};
use crate::functions::ImportStackEntry;
use crate::options::{
    move_options_into, opts, OptionsBox, SASS_STYLE_COMPACT, SASS_STYLE_COMPRESSED,
    SASS_STYLE_NESTED,
};

/// Builds the JSON `error_json` for a Sass error: keys
/// `status/file/line/column/message/formatted` (plan D14, mirroring
/// `handle_error`, sass_context.cpp:112-118). `formatted` is the same trace
/// as `error_message`; string errors (no span) omit file/line/column like
/// `handle_string_error` — but the C recipe always has a message, so `file`
/// falls back to `""` and line/column to 1 (matching libsass's JSON shape,
/// where the keys are always present for real errors).
///
/// Serialized with `serde_json` (2-space pretty, matching libsass's
/// `json_stringify(json_err, "  ")`) — never hand-rolled: string escaping
/// (quotes, backslashes, controls, non-ASCII passthrough) is the
/// serializer's job.
#[derive(Serialize)]
struct ErrorJson {
    status: u32,
    file: String,
    line: usize,
    column: usize,
    message: String,
    formatted: String,
}

/// JSON for maker/validation failures (no location): keys
/// `status/message/formatted`, mirroring `handle_string_error`
/// (sass_context.cpp:21-37).
#[derive(Serialize)]
struct StringErrorJson {
    status: u32,
    message: String,
    formatted: String,
}

fn error_json_bytes(err: &SassError, formatted: &str) -> Vec<u8> {
    let (file, line, column) = match err.span() {
        Some(span) => (
            span.source_url
                .as_ref()
                .map(|u| u.to_string())
                .unwrap_or_default(),
            span.line(),
            span.column(),
        ),
        None => (String::new(), 1, 1),
    };
    let value = ErrorJson {
        status: 1,
        file,
        line,
        column,
        message: err.message().to_string(),
        formatted: formatted.to_string(),
    };
    serde_json::to_string_pretty(&value)
        .unwrap_or_else(|_| "{\"status\": 1}".to_string())
        .into_bytes()
}

/// Shared result storage for both context flavors. Every field is an owned
/// nullable C string (or string array) freed on context delete; getters
/// borrow; a fresh compile replaces them all.
///
/// `pub` only because the public `ContextCommon.result` field names it.
/// All fields stay private; access is via `ContextResult` methods.
pub struct ContextResult {
    output_string: *mut c_char,
    source_map_string: *mut c_char,
    error_status: c_int,
    error_json: *mut c_char,
    error_text: *mut c_char,
    error_message: *mut c_char,
    error_file: *mut c_char,
    error_src: *mut c_char,
    error_line: usize,
    error_column: usize,
    included_files: *mut *mut c_char,
}

impl ContextResult {
    fn new() -> Self {
        ContextResult {
            output_string: ptr::null_mut(),
            source_map_string: ptr::null_mut(),
            error_status: 0,
            error_json: ptr::null_mut(),
            error_text: ptr::null_mut(),
            error_message: ptr::null_mut(),
            error_file: ptr::null_mut(),
            error_src: ptr::null_mut(),
            error_line: usize::MAX,
            error_column: usize::MAX,
            included_files: ptr::null_mut(),
        }
    }

    /// Frees one owned string slot (NULL-safe).
    fn free_slot(slot: &mut *mut c_char) {
        if !slot.is_null() {
            // SAFETY: non-null heap allocation owned by this struct.
            unsafe {
                libc::free(*slot as *mut libc::c_void);
            }
            *slot = ptr::null_mut();
        }
    }

    /// Frees the NULL-terminated string array (strings + array).
    fn free_files(slot: &mut *mut *mut c_char) {
        if slot.is_null() {
            return;
        }
        // SAFETY: NULL-terminated array of owned strings per construction in
        // `store_success`.
        unsafe {
            let mut cur = *slot;
            while !(*cur).is_null() {
                libc::free(*cur as *mut libc::c_void);
                cur = cur.add(1);
            }
            libc::free(*slot as *mut libc::c_void);
        }
        *slot = ptr::null_mut();
    }

    /// Clears all result fields (mirrors `sass_prepare_context` field reset +
    /// `sass_clear_context` frees, sass_context.cpp:276-285,537-554).
    fn clear(&mut self) {
        Self::free_slot(&mut self.output_string);
        Self::free_slot(&mut self.source_map_string);
        Self::free_slot(&mut self.error_json);
        Self::free_slot(&mut self.error_text);
        Self::free_slot(&mut self.error_message);
        Self::free_slot(&mut self.error_file);
        Self::free_slot(&mut self.error_src);
        Self::free_files(&mut self.included_files);
        self.error_status = 0;
        self.error_line = usize::MAX;
        self.error_column = usize::MAX;
    }

    /// Installs an already-owned string (NULL allowed).
    fn put(slot: &mut *mut c_char, owned: *mut c_char) {
        Self::free_slot(slot);
        *slot = owned;
    }
}

impl Drop for ContextResult {
    fn drop(&mut self) {
        self.clear();
    }
}

/// The shared half of both context flavors: embedded options + results.
///
/// `pub` because it is embedded in the public context boxes (which name it in
/// `pub struct` fields); all access stays inside the crate.
pub struct ContextCommon {
    pub(crate) options: OptionsBox,
    pub result: ContextResult,
}

impl ContextCommon {
    /// Fails the context with a plain message (maker errors and pre-compile
    /// validation — mirrors the `handle_string_error` shape: JSON
    /// `{status,message,formatted}`, no location; status always 1 here since
    /// only Sass-level failures reach this path).
    fn fail_with_message(&mut self, message: &str) {
        self.result.clear();
        self.result.error_status = 1;
        let formatted = format!("Internal Error: {message}\n");
        ContextResult::put(
            &mut self.result.error_message,
            copy_bytes_nul(formatted.as_bytes()),
        );
        ContextResult::put(
            &mut self.result.error_text,
            copy_bytes_nul(message.as_bytes()),
        );
        let json = serde_json::to_string_pretty(&StringErrorJson {
            status: 1,
            message: message.to_string(),
            formatted: formatted.clone(),
        })
        .unwrap_or_else(|_| "{\"status\": 1}".to_string());
        ContextResult::put(&mut self.result.error_json, copy_bytes_nul(json.as_bytes()));
    }

    /// Installs a Sass failure from the core error: formatted trace, raw
    /// message, JSON object, and span-derived file/line/column/src.
    fn fail_with_error(&mut self, err: &SassError, io: &dyn Io) {
        self.result.clear();
        self.result.error_status = 1;
        // ASCII glyphs, no color: the C API has no unicode/color negotiation.
        // `opts.unicode` (CompileOptions, default true) controls the core's
        // glyph set the same way (compile/mod.rs:492-494,541-544) — the
        // adapter sets it false so `error_message` matches the harness's
        // `--no-unicode --no-color` comparison (the wrapper strips those
        // flags only because sassc itself rejects them).
        let mut formatted = err.to_error_string_with_options(
            &rust_sass::common::source_span_highlighter::HighlightOptions {
                color: rust_sass::common::source_span_highlighter::HighlightColor::None,
                glyphs: rust_sass::termglyph::GlyphSet::Ascii,
                ..Default::default()
            },
            io,
        );
        // Trailing-newline parity: `format_single` (exception.rs:502) ends
        // the trace with the span line and NO trailing newline
        // (`append_trace_lines` writes `- 1:8  root stylesheet` bare), while
        // libsass's `msg_stream` always ends with `\n` (sass_context.cpp:64:
        // `if (!got_newline) msg_stream << "\n"` — a parse error's excerpt
        // never sets got_newline). sassc prints `error_message` with `%s`
        // (no added newline), and the harness strips trailing newlines before
        // comparing — so the presence/absence is invisible there, but the
        // direct getter must match libsass byte-for-byte.
        if !formatted.ends_with('\n') {
            formatted.push('\n');
        }
        let json = error_json_bytes(err, &formatted);
        ContextResult::put(
            &mut self.result.error_message,
            copy_bytes_nul(formatted.as_bytes()),
        );
        ContextResult::put(
            &mut self.result.error_text,
            copy_bytes_nul(err.message().as_bytes()),
        );
        ContextResult::put(&mut self.result.error_json, copy_bytes_nul(&json));
        match err.span() {
            Some(span) => {
                let file = span
                    .source_url
                    .as_ref()
                    .map(|u| u.to_string())
                    .unwrap_or_default();
                ContextResult::put(&mut self.result.error_file, copy_bytes_nul(file.as_bytes()));
                ContextResult::put(
                    &mut self.result.error_src,
                    copy_bytes_nul(span.text.as_bytes()),
                );
                self.result.error_line = span.line();
                self.result.error_column = span.column();
            }
            None => {
                self.result.error_line = usize::MAX;
                self.result.error_column = usize::MAX;
            }
        }
    }

    /// Stores a successful compile: CSS, source map (when enabled), and the
    /// included-files array (NULL-terminated, mirroring `copy_strings`).
    #[allow(clippy::too_many_arguments)]
    fn store_success(
        &mut self,
        css: &str,
        source_map_json: Option<String>,
        css_comment: Option<String>,
        loaded_urls: &[rust_sass::url::SassUrl],
        entry_is_data: bool,
    ) {
        self.result.clear();
        let mut css_text = css.to_string();
        if let Some(comment) = css_comment {
            // The map comment is separated from the CSS by a blank line
            // (upstream schedules a double linefeed after a top-level block
            // close before appending the comment).
            if !css_text.is_empty() {
                if !css_text.ends_with('\n') {
                    css_text.push('\n');
                }
                if !css_text.ends_with("\n\n") {
                    css_text.push('\n');
                }
            }
            css_text.push_str(&comment);
        }
        // libsass appends a trailing linefeed when the buffer lacks one
        // (`output.cpp:67-70`: non-empty output always ends with `linefeed`).
        // The core serializer leaves the CSS unterminated, so add it here.
        if !css_text.is_empty() && !css_text.ends_with('\n') {
            css_text.push('\n');
        }
        ContextResult::put(
            &mut self.result.output_string,
            copy_bytes_nul(css_text.as_bytes()),
        );
        if let Some(map) = source_map_json {
            ContextResult::put(
                &mut self.result.source_map_string,
                copy_bytes_nul(map.as_bytes()),
            );
        }
        store_files(&mut self.result, loaded_urls, entry_is_data);
    }
}

/// Reads the C option strings needed for one compile into an owned snapshot.
/// Borrowed pointers (indent/linefeed) are copied out here so the compile
/// does not borrow the context across `block_on`.
struct CompileSnapshot {
    output_style: u32,
    source_comments: bool,
    source_map_embed: bool,
    source_map_contents: bool,
    source_map_file_urls: bool,
    omit_source_map_url: bool,
    is_indented_syntax_src: bool,
    indent: Vec<u8>,
    linefeed: Vec<u8>,
    /// Kept for JSON `file`-key derivation in later increments (v0 records
    /// the field; the renderer currently uses `output_path`).
    #[allow(dead_code)]
    input_path: Option<Vec<u8>>,
    output_path: Option<Vec<u8>>,
    include_path: Option<Vec<u8>>,
    source_map_file: Option<Vec<u8>>,
    source_map_root: Option<Vec<u8>>,
    include_paths: Vec<Vec<u8>>,
    /// Custom-function list (borrowed for the compile; owned by the options).
    /// Read as a NULL-terminated entry array at compile time (step 6).
    c_functions: *mut *mut FunctionEntry,
    /// Custom-importer list (borrowed for the compile; owned by the options).
    /// Read as a NULL-terminated entry array at compile time (step 7).
    c_importers: *mut *mut ImporterEntry,
}

/// Copies a borrowed C string (NULL → None).
///
/// # Safety
///
/// `s` must be NULL or a valid NUL-terminated string; the struct owning it
/// must be live for the call.
unsafe fn copy_opt_str(s: *const c_char) -> Option<Vec<u8>> {
    if s.is_null() {
        return None;
    }
    // SAFETY: per contract above.
    Some(unsafe { CStr::from_ptr(s) }.to_bytes().to_vec())
}

impl CompileSnapshot {
    /// # Safety
    ///
    /// `options` must be a live `OptionsBox` for the call.
    unsafe fn take(options: &OptionsBox) -> Self {
        // SAFETY: caller guarantees liveness; every pointer is either NULL or
        // a valid string per the options contracts (G1/G19 for borrowed).
        unsafe {
            CompileSnapshot {
                output_style: options.output_style,
                source_comments: options.source_comments,
                source_map_embed: options.source_map_embed,
                source_map_contents: options.source_map_contents,
                source_map_file_urls: options.source_map_file_urls,
                omit_source_map_url: options.omit_source_map_url,
                is_indented_syntax_src: options.is_indented_syntax_src,
                indent: copy_opt_str(if options.indent.is_null() {
                    c"  ".as_ptr()
                } else {
                    options.indent
                })
                .unwrap_or_else(|| b"  ".to_vec()),
                linefeed: copy_opt_str(if options.linefeed.is_null() {
                    c"\n".as_ptr()
                } else {
                    options.linefeed
                })
                .unwrap_or_else(|| b"\n".to_vec()),
                input_path: options.input_path.bytes().map(|b| b.to_vec()),
                output_path: options.output_path.bytes().map(|b| b.to_vec()),
                include_path: options.include_path.bytes().map(|b| b.to_vec()),
                source_map_file: options.source_map_file.bytes().map(|b| b.to_vec()),
                source_map_root: options.source_map_root.bytes().map(|b| b.to_vec()),
                include_paths: options
                    .include_paths
                    .iter()
                    .filter_map(|e| copy_opt_str(e.ptr as *const c_char))
                    .collect(),
                c_functions: options.c_functions,
                c_importers: options.c_importers,
            }
        }
    }

    /// Parses the indent string into `(use_spaces, width)`: all-spaces →
    /// `(true, count)`, all-tabs → `(false, count)`, else default `(true, 2)`;
    /// width clamped to 10 (plan §9 — setters have no error channel).
    fn parse_indent(&self) -> (bool, u32) {
        if self.indent.is_empty() {
            return (true, 2);
        }
        if self.indent.iter().all(|&b| b == b' ') {
            (true, self.indent.len().min(10) as u32)
        } else if self.indent.iter().all(|&b| b == b'\t') {
            (false, self.indent.len().min(10) as u32)
        } else {
            (true, 2)
        }
    }

    fn output_style(&self) -> OutputStyle {
        match self.output_style {
            SASS_STYLE_NESTED => OutputStyle::Nested,
            SASS_STYLE_COMPACT => OutputStyle::Compact,
            SASS_STYLE_COMPRESSED => OutputStyle::Compressed,
            // EXPANDED renders expanded; unknown values harden to expanded
            // (upstream would misbehave).
            _ => OutputStyle::Expanded,
        }
    }

    fn syntax(&self) -> Syntax {
        if self.is_indented_syntax_src {
            // Zeroed indent state mirrors `Stylesheet::parse_sass`'s own
            // construction (ast/sass/statement/stylesheet.rs:142-150).
            Syntax::Sass(SassIndentState {
                current_indentation: 0,
                next_indentation: None,
                next_indentation_end: None,
                indent_spaces: None,
            })
        } else {
            Syntax::Scss
        }
    }

    /// Splits the joined include-path string on `:` (plan §9; `;` is
    /// Windows-only, out of scope) and chains the pushed list after it,
    /// mirroring how node-sass pre-joins and sassc pushes.
    fn load_paths(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(joined) = &self.include_path {
            if let Ok(text) = std::str::from_utf8(joined) {
                out.extend(
                    text.split(':')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string()),
                );
            }
        }
        for entry in &self.include_paths {
            if let Ok(text) = std::str::from_utf8(entry) {
                if !text.is_empty() {
                    out.push(text.to_string());
                }
            }
        }
        out
    }
}

/// A `Logger` that renders warnings/deprecations to text and appends them to
/// a shared buffer — the C-side equivalent of the harness's stderr capture.
/// Rendering matches `StderrLogger` blocks minus color (the harness compares
/// warnings via stderr text; sassc itself prints nothing to stderr, so the
/// buffer is exactly the warnings).
///
/// Location rendering (`pretty_uri`) needs the real compile `Io`: file URLs
/// are relativized against its CWD (a stub CWD would leak absolute paths
/// into `@debug`/warning locations).
#[derive(Debug)]
struct CaptureLogger {
    out: Mutex<String>,
    io: Rc<dyn Io>,
}

impl CaptureLogger {
    fn new(io: Rc<dyn Io>) -> Self {
        CaptureLogger {
            out: Mutex::new(String::new()),
            io,
        }
    }

    fn take(&self) -> String {
        std::mem::take(&mut *self.out.lock().unwrap())
    }
}

impl Logger for CaptureLogger {
    fn warn<'a>(
        &self,
        message: &str,
        span: Option<&rust_sass::common::span::Span<'a>>,
        trace: Option<&Trace>,
    ) {
        let stack = trace
            .map(|t| t.format(self.io.as_ref()))
            .unwrap_or_default();
        // `render_warning` returns Err only for fatal deprecations; the
        // capture logger never configures any, so `unwrap_or_default` is
        // unreachable in practice (and must not panic across the logger
        // seam — a warning failure must not fail the compile).
        let text = rust_sass::logger::stderr::render_warning(
            false,
            false,
            self.io.as_ref(),
            None,
            message,
            span,
            &stack,
        )
        .unwrap_or_default();
        self.out.lock().unwrap().push_str(&text);
    }

    fn debug<'a>(&self, message: &str, span: Option<&rust_sass::common::span::Span<'a>>) {
        let mut out = self.out.lock().unwrap();
        out.push_str(&rust_sass::logger::stderr::render_debug(
            false,
            self.io.as_ref(),
            message,
            span,
        ));
    }

    fn warn_deprecation<'a>(
        &self,
        message: &str,
        span: Option<&rust_sass::common::span::Span<'a>>,
        deprecation: &'static rust_sass::deprecation::Deprecation,
        trace: Option<&Trace>,
    ) -> Result<(), Box<SassError>> {
        let stack = trace
            .map(|t| t.format(self.io.as_ref()))
            .unwrap_or_default();
        let text = rust_sass::logger::stderr::render_warning(
            false,
            false,
            self.io.as_ref(),
            Some(deprecation),
            message,
            span,
            &stack,
        )
        .unwrap_or_default();
        self.out.lock().unwrap().push_str(&text);
        Ok(())
    }
}

/// Builds the entry frame seeding importer compiler tokens (mirrors
/// upstream's never-popped file/data entry: imp = the input path or
/// `"stdin"`, abs = absolutized — `rel2abs`, context.cpp:565-618).
fn token_entry(display: &str, io: &dyn Io) -> ImportStackEntry {
    let abs = if display.starts_with('/') {
        display.to_string()
    } else {
        format!("{}/{}", io.current_dir(), display)
    };
    ImportStackEntry {
        imp_path: display.to_string(),
        abs_path: abs,
    }
}

/// Runs one data-context compile synchronously and stores the result.
///
/// `source` borrows the caller's buffer (copied into the arena by the core);
/// `snapshot` carries the options (parsed per plan §9). The core is called
/// directly (never through `maybe_block_on!`): the adapter is sync-only
/// (plan D9), so in the sync build `compile_string` is a plain sync function
/// — and if anyone ever enables `rust-sass/async` underneath, the direct
/// call fails loudly at compile time instead of silently changing driving
/// behavior.
fn run_data_compile(common: &mut ContextCommon, source: &str, snapshot: &CompileSnapshot) {
    let arena = Bump::new();
    let io: Rc<dyn Io> = Rc::new(DefaultIo::new());
    let mut opts = CompileOptions::new(&arena);
    let capture = apply_snapshot_options(&mut opts, snapshot, io.clone());
    // Custom functions bind before evaluation; a bad signature (or missing
    // callback) fails the compile here, never mid-evaluation.
    match build_custom_callables(snapshot.c_functions, &arena) {
        Ok(functions) => opts.functions = functions,
        Err(e) => {
            common.fail_with_error(&e, &*io);
            return;
        }
    }
    // Custom importers bind in the same pre-eval slot (priority-sorted,
    // per-compile stash); a missing callback or multi-entry return fails
    // here, never mid-evaluation.
    match build_custom_importers(snapshot.c_importers, &arena, &token_entry("stdin", &*io)) {
        Ok(importers) => opts.importers = importers,
        Err(e) => {
            common.fail_with_error(&e, &*io);
            return;
        }
    }
    let outcome = rust_sass::compile::compile_string(source, io.clone(), opts, &arena);
    let want_map = snapshot.source_map_file.is_some() || snapshot.source_map_embed;
    finish_compile(common, outcome, snapshot, want_map, &*io, true);
    drain_warnings(common, &capture);
}

/// Runs one file-context compile synchronously and stores the result.
fn run_file_compile(common: &mut ContextCommon, path: &str, snapshot: &CompileSnapshot) {
    let arena = Bump::new();
    let io: Rc<dyn Io> = Rc::new(DefaultIo::new());
    let mut opts = CompileOptions::new(&arena);
    let capture = apply_snapshot_options(&mut opts, snapshot, io.clone());
    // `compile()` re-infers the syntax from the path when `opts.syntax` is
    // the default `Scss` (compile/mod.rs:411-413) — so pass Scss through and
    // only force Sass for explicitly indented sources (plan §9).
    if !snapshot.is_indented_syntax_src {
        opts.syntax = Syntax::Scss;
    }
    let want_map = snapshot.source_map_file.is_some() || snapshot.source_map_embed;
    opts.source_map = want_map;
    opts.include_source_map_sources = snapshot.source_map_contents;
    match build_custom_callables(snapshot.c_functions, &arena) {
        Ok(functions) => opts.functions = functions,
        Err(e) => {
            common.fail_with_error(&e, &*io);
            return;
        }
    }
    match build_custom_importers(snapshot.c_importers, &arena, &token_entry(path, &*io)) {
        Ok(importers) => opts.importers = importers,
        Err(e) => {
            common.fail_with_error(&e, &*io);
            return;
        }
    }
    let outcome = rust_sass::compile::compile(path, io.clone(), opts, &arena);
    finish_compile(common, outcome, snapshot, want_map, &*io, false);
    drain_warnings(common, &capture);
}

/// Appends captured warnings to stderr — the C-side equivalent of sassc's
/// world, where warnings reach the terminal via stderr and the harness
/// compares them. (The core's `DeprecationProcessingLogger` already validated
/// and summarized; the buffer holds exactly the rendered blocks.)
fn drain_warnings(common: &mut ContextCommon, capture: &CaptureLogger) {
    let text = capture.take();
    if text.is_empty() {
        return;
    }
    let _ = std::io::stderr().lock().write_all(text.as_bytes());
    // `error_message`-adjacent visibility: libsass-era consumers read
    // warnings from stderr, not from any context field — no context field is
    // set here by design (there is none upstream either).
    let _ = common;
}

/// Applies the snapshot's option mapping to `opts` (plan §9). Returns the
/// warning-capture logger so the caller can drain it after the compile.
/// The capture renders with the compile `Io` so locations relativize against
/// the real CWD.
fn apply_snapshot_options<'compile, 'parse>(
    opts: &mut CompileOptions<'compile, 'parse>,
    snapshot: &CompileSnapshot,
    io: Rc<dyn Io>,
) -> Rc<CaptureLogger>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let (use_spaces, indent_width) = snapshot.parse_indent();
    opts.style = snapshot.output_style();
    opts.use_spaces = use_spaces;
    opts.indent_width = Some(indent_width);
    // Custom linefeeds flow verbatim for the four known feeds (node-sass
    // `lf/crlf/cr/lfcr`); anything else falls back to LF — the core
    // `LineFeed.text` is `&'static`, so arbitrary runtime bytes cannot
    // flow without leaking (documented; only the known set is exercised
    // by any consumer).
    opts.line_feed = if snapshot.linefeed == b"\r\n" {
        rust_sass::serialize::LINE_FEED_CRLF
    } else if snapshot.linefeed == b"\r" {
        rust_sass::serialize::LINE_FEED_CR
    } else if snapshot.linefeed == b"\n\r" {
        rust_sass::serialize::LINE_FEED_LFCR
    } else {
        rust_sass::serialize::LINE_FEED_LF
    };
    // Precision is accepted, ignored (plan D4) — no field exists for it
    // on `CompileOptions` by design.
    opts.source_comments = snapshot.source_comments;
    opts.charset = true;
    // Unlimited deprecation repetition (`verbose = true`): the harness always
    // invokes `--command` with `--verbose` under `--impl dart-sass` (the
    // wrapper strips the flag only because sassc rejects it — expectations
    // still list every warning), and libsass never limited repetition, so
    // C-API consumers expect every warning. Without this the core collapses
    // repeats into "N repetitive deprecation warnings omitted."
    opts.verbose = true;
    // No unicode/color negotiation on the C API: render errors ASCII so
    // `error_message` matches the harness comparison (see `fail_with_error`).
    // Warnings/deprecations are captured into a buffer and drained to stderr
    // after the compile: the harness compares them, and sassc itself prints
    // nothing to stderr, so the buffer is exactly the warnings — rendered
    // colorless by construction (`false, false` below).
    opts.unicode = false;
    opts.alert_color = false;
    opts.alert_ascii = true;
    let capture = Rc::new(CaptureLogger::new(io));
    opts.logger = Some(capture.clone());
    opts.load_paths = snapshot.load_paths();
    opts.syntax = snapshot.syntax();
    let want_map = snapshot.source_map_file.is_some() || snapshot.source_map_embed;
    opts.source_map = want_map;
    opts.include_source_map_sources = snapshot.source_map_contents;
    capture
}

/// Stores a compile outcome: success → CSS + map + files; failure → error
/// fields (plan D14), output/map forced NULL.
fn finish_compile(
    common: &mut ContextCommon,
    outcome: Result<CompileResult<'_, '_>, Box<SassError>>,
    snapshot: &CompileSnapshot,
    want_map: bool,
    io: &dyn Io,
    entry_is_data: bool,
) {
    match outcome {
        Ok(result) => {
            let (map_json, comment) = if want_map {
                let json = result.source_map().map(|m| {
                    render_source_map_json(
                        m,
                        snapshot,
                        &result
                            .loaded_urls()
                            .iter()
                            .map(|u| u.to_string())
                            .collect::<Vec<_>>(),
                        &io.current_dir(),
                    )
                });
                let comment = if snapshot.omit_source_map_url {
                    None
                } else if snapshot.source_map_embed {
                    json.as_ref().map(|j| {
                        format!(
                            "/*# sourceMappingURL=data:application/json;base64,{} */",
                            base64::engine::general_purpose::STANDARD.encode(j.as_bytes())
                        )
                    })
                } else {
                    snapshot.source_map_file.as_ref().and_then(|f| {
                        String::from_utf8(f.clone()).ok().map(|name| {
                            // The comment carries the map path relative to
                            // the output (upstream `format_source_mapping_url`
                            // via `abs2rel(map, output)`).
                            let rel = abs2rel(
                                &name,
                                &effective_output_path(
                                    opt_bytes(&snapshot.output_path).as_deref(),
                                    opt_bytes(&snapshot.input_path).as_deref(),
                                ),
                                &io.current_dir(),
                            );
                            format!("/*# sourceMappingURL={rel} */")
                        })
                    })
                };
                (json, comment)
            } else {
                (None, None)
            };
            common.store_success(
                result.css(),
                map_json,
                comment,
                result.loaded_urls(),
                entry_is_data,
            );
        }
        Err(err) => common.fail_with_error(&err, io),
    }
}

/// Stores the included-files array on a cleared result: skips stdin/data
/// entries for data contexts (mirroring `get_included_files(skip=true)`,
/// context.cpp:704-714), pins the entry first for file contexts while
/// sorting the rest like upstream (`sort(begin+(skip?0:1), end)`), and sorts
/// everything for data contexts.
///
/// Shared by the one-shot path (`store_success`) and the staged parse phase
/// (which stores files with no CSS yet): both compute the list through this
/// one function so parse-phase and execute-phase lists cannot disagree on
/// shape.
fn store_files(
    result: &mut ContextResult,
    loaded_urls: &[rust_sass::url::SassUrl],
    entry_is_data: bool,
) {
    let mut texts: Vec<String> = Vec::new();
    for url in loaded_urls {
        let text = url.to_string();
        if entry_is_data && (text == "stdin" || text.is_empty()) {
            continue;
        }
        // Upstream reports filesystem paths, never `file:` URLs:
        // strip the scheme for file-backed entries (node-sass asserts
        // plain paths in `stats.includedFiles`).
        texts.push(
            url.as_url()
                .to_file_path()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or(text),
        );
    }
    if entry_is_data {
        texts.sort();
    } else if let Some((_, rest)) = texts.split_first_mut() {
        rest.sort();
    }
    let mut owned: Vec<*mut c_char> = Vec::new();
    for text in &texts {
        owned.push(copy_bytes_nul(text.as_bytes()));
    }
    if !owned.is_empty() {
        owned.push(ptr::null_mut());
        let len = owned.len();
        // SAFETY: fresh allocation of exactly len pointers; all written.
        let arr = malloc_or_abort(len * size_of::<*mut c_char>()) as *mut *mut c_char;
        unsafe {
            ptr::copy_nonoverlapping(owned.as_ptr(), arr, len);
        }
        // SAFETY: ownership of each string moves into the array; the Vec
        // itself holds no allocation to free (pointers only).
        std::mem::forget(owned);
        result.included_files = arr;
    }
}

/// Renders the core `SingleMapping` as libsass-shaped JSON
/// (`version/file/sourceRoot?/sources/sourcesContent?/names/mappings`,
/// plan §5.9). `sources` entries are absolutized to `file://` URLs when
/// `source_map_file_urls` is set; otherwise they pass through.
///
/// Serialized with `serde_json` using a tab-indented pretty formatter
/// (matching libsass's `json_stringify(json_srcmap, "\t")`) — never
/// hand-rolled. `mappings` and `sourcesContent` come from the core mapping;
/// `names` is always empty (the compiler never alters identifiers, mirroring
/// libsass's own empty `names` array in source_map.cpp).
#[derive(Serialize)]
struct SourceMapJson {
    version: u32,
    file: String,
    #[serde(rename = "sourceRoot", skip_serializing_if = "Option::is_none")]
    source_root: Option<String>,
    sources: Vec<String>,
    #[serde(rename = "sourcesContent", skip_serializing_if = "Vec::is_empty")]
    sources_content: Vec<String>,
    names: Vec<String>,
    mappings: String,
}

/// Serializes with tab indentation (libsass `json_stringify(…, "\t")`).
/// `serde_json`'s default pretty printer uses two spaces, so a custom
/// formatter is required — this is layout, not hand-rolled escaping.
fn to_json_tab<T: Serialize>(value: &T) -> String {
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    if value.serialize(&mut ser).is_err() {
        return "{\"version\": 3}".to_string();
    }
    String::from_utf8(buf).unwrap_or_default()
}

/// Collapses `/./` and duplicate `//` (mirrors `make_canonical_path`,
/// `file.cpp:161-196` — no filesystem access, no `..` resolution).
fn canonical_libsass_path(path: &str) -> String {
    let mut out = path.to_string();
    loop {
        let next = out.replace("//", "/").replace("/./", "/");
        if next == out {
            return out;
        }
        out = next;
    }
}

/// Joins `path` onto the CWD unless absolute (mirrors `rel2abs`,
/// `file.cpp:248-259`).
fn rel2abs(path: &str, cwd: &str) -> String {
    if path.starts_with('/') {
        return canonical_libsass_path(path);
    }
    canonical_libsass_path(&format!("{cwd}/{path}"))
}

/// Makes `path` relative to `base` (both resolved against `cwd` first),
/// mirroring `abs2rel` (`file.cpp:263-335`): common prefix to the last
/// shared `/`, then one `../` per remaining base segment (excluding `..`
/// segments), then the path remainder. Protocol outputs (`scheme://`)
/// pass through verbatim (`:269-280`).
fn abs2rel(path: &str, base: &str, cwd: &str) -> String {
    if path.contains("://") {
        return path.to_string();
    }
    let abs_path = rel2abs(path, cwd);
    let abs_base = rel2abs(base, cwd);
    let shared = abs_path
        .char_indices()
        .zip(abs_base.char_indices())
        .take_while(|((_, a), (_, b))| a == b)
        .last()
        .map(|((i, _), _)| i)
        .unwrap_or(0);
    // Back up to the last shared `/` (a partial-segment match is not a
    // shared directory).
    let cut = abs_path[..=shared].rfind('/').unwrap_or(0);
    let stripped_uri = abs_path[cut.min(abs_path.len())..].trim_start_matches('/');
    let stripped_base = abs_base[cut.min(abs_base.len())..].trim_start_matches('/');
    // One `../` per base *directory* (slashes, i.e. all segments but the
    // trailing filename, excluding `..` segments — `file.cpp:311-334`).
    let mut segments: Vec<&str> = stripped_base.split('/').filter(|s| !s.is_empty()).collect();
    segments.pop();
    let ups = segments.iter().filter(|s| **s != "..").count();
    format!("{}{}", "../".repeat(ups), stripped_uri)
}

/// Derives the effective output path: verbatim when set, else the input
/// minus extension + `.css`, else `"stdout"` (mirrors `safe_output`,
/// `context.cpp:36-43`). Shared by the JSON `file` key and the map comment.
fn effective_output_path(output: Option<&str>, input: Option<&str>) -> String {
    match output {
        Some(o) if !o.is_empty() => o.to_string(),
        _ => match input {
            Some(i) if !i.is_empty() => {
                let stem = match i.rfind('.').map(|d| (d, i.rfind('/').unwrap_or(0))) {
                    Some((d, s)) if d > s => &i[..d],
                    _ => i,
                };
                format!("{stem}.css")
            }
            _ => "stdout".to_string(),
        },
    }
}

/// Computes the source-map JSON `file` key: `abs2rel(output, map, cwd)`
/// (rule installed by `Emitter::set_filename`, `context.cpp:97`).
fn libsass_file_key(
    output: Option<&str>,
    map_file: Option<&str>,
    input: Option<&str>,
    cwd: &str,
) -> String {
    let output = effective_output_path(output, input);
    abs2rel(&output, map_file.unwrap_or(""), cwd)
}

/// Computes one `sources[]` entry: with `source_map_file_urls`, relative
/// paths absolutize to `file://` (mirrors `source_map.cpp:43-53`); otherwise
/// entries relativize against the map file like `file` does (upstream
/// `context.cpp:263` — consumers otherwise get absolute `file://` URLs
/// where libsass emits relative paths).
fn map_source(url: &str, map_ref: &str, file_urls: bool, cwd: &str) -> String {
    if url.is_empty() {
        return String::new();
    }
    if file_urls && !url.contains("://") {
        if url.starts_with('/') {
            return format!("file://{url}");
        }
        return format!("file:///{url}");
    }
    if file_urls {
        return url.to_string();
    }
    let path = url
        .strip_prefix("file://")
        .map(|s| s.to_string())
        .unwrap_or_else(|| url.to_string());
    abs2rel(&path, map_ref, cwd)
}

/// Reads an owned option string lossily (NULL/unset → None).
fn opt_bytes(slot: &Option<Vec<u8>>) -> Option<String> {
    slot.as_ref()
        .and_then(|b| String::from_utf8(b.clone()).ok())
}

fn render_source_map_json(
    map: &rust_sass::sourcemap::SingleMapping,
    snapshot: &CompileSnapshot,
    urls: &[String],
    cwd: &str,
) -> String {
    let file = libsass_file_key(
        opt_bytes(&snapshot.output_path).as_deref(),
        opt_bytes(&snapshot.source_map_file).as_deref(),
        opt_bytes(&snapshot.input_path).as_deref(),
        cwd,
    );
    let source_root = snapshot
        .source_map_root
        .as_ref()
        .and_then(|r| String::from_utf8(r.clone()).ok());
    let map_ref = opt_bytes(&snapshot.source_map_file).unwrap_or_default();
    let sources = urls
        .iter()
        .map(|url| map_source(url, &map_ref, snapshot.source_map_file_urls, cwd))
        .collect();
    to_json_tab(&SourceMapJson {
        version: 3,
        file,
        source_root,
        sources,
        sources_content: map.sources_content.clone(),
        names: Vec::new(),
        mappings: map.mappings.clone(),
    })
}

/// The `Sass_File_Context` heap struct: embedded options + results.
/// `input_path` lives in the embedded options (copied at make time).
pub struct FileContextBox {
    common: ContextCommon,
}

/// The `Sass_Data_Context` heap struct: embedded options + results + the
/// owned source buffer (transferred at make time).
pub struct DataContextBox {
    common: ContextCommon,
    /// Owned source buffer (transferred ownership, freed on delete even if
    /// compilation never runs — mirrors `Data_Context` ctor semantics).
    source: *mut c_char,
}

impl Drop for DataContextBox {
    fn drop(&mut self) {
        if !self.source.is_null() {
            // SAFETY: owned allocation transferred at make time, freed once.
            unsafe {
                libc::free(self.source as *mut libc::c_void);
            }
        }
    }
}

/// Reads a live file context (NULL → None; all entry points harden NULL).
///
/// # Safety
///
/// `ctx` must be NULL or a live pointer from [`sass_make_file_context`].
unsafe fn file_ctx(ctx: *mut FileContextBox) -> Option<&'static mut FileContextBox> {
    if ctx.is_null() {
        return None;
    }
    // SAFETY: live box per contract; borrow lasts only for the enclosing call.
    Some(unsafe { &mut *ctx })
}

/// Reads a live data context (NULL → None).
///
/// # Safety
///
/// `ctx` must be NULL or a live pointer from [`sass_make_data_context`].
unsafe fn data_ctx(ctx: *mut DataContextBox) -> Option<&'static mut DataContextBox> {
    if ctx.is_null() {
        return None;
    }
    // SAFETY: live box per contract; borrow lasts only for the enclosing call.
    Some(unsafe { &mut *ctx })
}

/// Creates a file context. Copies `input_path` in; NULL or empty paths are
/// maker errors (non-null context with `error_status` set — the later compile
/// early-returns it, mirroring sass_context.cpp:345-362,397-424).
///
/// # Safety
///
/// `input_path` must be NULL or a valid NUL-terminated string for the call.
/// The caller owns the result (frees with [`sass_delete_file_context`]).
#[no_mangle]
pub unsafe extern "C" fn sass_make_file_context(input_path: *const c_char) -> *mut FileContextBox {
    guard(ptr::null_mut(), || {
        let mut common = ContextCommon {
            options: OptionsBox::new(),
            result: ContextResult::new(),
        };
        // SAFETY: NULL checked below via read; validity per contract above.
        let path = if input_path.is_null() {
            None
        } else {
            // SAFETY: non-null NUL-terminated string per contract.
            Some(unsafe { CStr::from_ptr(input_path) }.to_bytes())
        };
        match path {
            Some(bytes) if !bytes.is_empty() => {
                // SAFETY: valid input string per contract.
                unsafe { common.options.input_path.set(input_path) };
            }
            _ => {
                common.fail_with_message(if input_path.is_null() {
                    "File context created without an input path"
                } else {
                    "File context created with empty input path"
                });
            }
        }
        // SAFETY: fresh box; ownership moves to the caller.
        Box::into_raw(Box::new(FileContextBox { common }))
    })
}

/// Creates a data context, taking ownership of `source_string` (which must be
/// a malloc'd buffer — freed with the context, never by the caller).
/// NULL source is a maker error like the file side.
///
/// # Safety
///
/// `source_string` must be NULL or a malloc'd NUL-terminated buffer whose
/// ownership transfers to the context. The caller owns the result (frees with
/// [`sass_delete_data_context`]; must NOT free `source_string` itself).
#[no_mangle]
pub unsafe extern "C" fn sass_make_data_context(source_string: *mut c_char) -> *mut DataContextBox {
    guard(ptr::null_mut(), || {
        let mut common = ContextCommon {
            options: OptionsBox::new(),
            result: ContextResult::new(),
        };
        if source_string.is_null() {
            common.fail_with_message("Data context created without a source string");
        } else {
            // SAFETY: non-null per check; emptiness read is side-effect free.
            let empty = unsafe { CStr::from_ptr(source_string) }
                .to_bytes()
                .is_empty();
            if empty {
                // Empty source is a maker error for data contexts (unlike the
                // file side's compile-time check) — mirrors
                // sass_context.cpp:373-379. The buffer is still owned.
                common.fail_with_message("Data context created with empty source string");
            }
        }
        // SAFETY: fresh box; ownership moves to the caller (with `source`).
        Box::into_raw(Box::new(DataContextBox {
            common,
            source: source_string,
        }))
    })
}

/// Deletes a file context and everything it owns. NULL-safe.
///
/// # Safety
///
/// `ctx` must be NULL or a live pointer from [`sass_make_file_context`],
/// freed exactly once.
#[no_mangle]
pub unsafe extern "C" fn sass_delete_file_context(ctx: *mut FileContextBox) {
    guard((), || {
        if ctx.is_null() {
            return;
        }
        // SAFETY: live box per contract; Drop frees options + results.
        unsafe {
            drop(Box::from_raw(ctx));
        }
    });
}

/// Deletes a data context, its source buffer, and everything else it owns.
/// NULL-safe.
///
/// # Safety
///
/// `ctx` must be NULL or a live pointer from [`sass_make_data_context`],
/// freed exactly once.
#[no_mangle]
pub unsafe extern "C" fn sass_delete_data_context(ctx: *mut DataContextBox) {
    guard((), || {
        if ctx.is_null() {
            return;
        }
        // SAFETY: live box per contract; Drop frees source + options + results.
        unsafe {
            drop(Box::from_raw(ctx));
        }
    });
}

macro_rules! context_options_access {
    ($get_ctx:ident, $get_opts:ident, $set_opts:ident, $box:ty, $ctx_fn:ident) => {
        /// Upcasts the context to its embedded `Sass_Context` (the common
        /// prefix: options + results). NULL hardens to NULL.
        ///
        /// # Safety
        ///
        /// `ctx` must be NULL or a live pointer from the matching maker.
        #[no_mangle]
        pub unsafe extern "C" fn $get_ctx(ctx: *mut $box) -> *mut ContextCommon {
            guard(ptr::null_mut(), || unsafe {
                $ctx_fn(ctx)
                    .map(|c| &mut c.common as *mut ContextCommon)
                    .unwrap_or(ptr::null_mut())
            })
        }

        /// Returns the embedded options (never NULL for a live context; NULL
        /// hardens to NULL). Mirrors the `*_get_options` direct upcasts
        /// (sass_context.cpp:605-607 — no allocation, just the address).
        ///
        /// # Safety
        ///
        /// `ctx` must be NULL or a live pointer from the matching maker.
        #[no_mangle]
        pub unsafe extern "C" fn $get_opts(ctx: *mut $box) -> *mut OptionsBox {
            guard(ptr::null_mut(), || unsafe {
                $ctx_fn(ctx)
                    .map(|c| &mut c.common.options as *mut OptionsBox)
                    .unwrap_or(ptr::null_mut())
            })
        }

        /// Moves the caller's options into the context (plan G2); the source
        /// is reset to defaults and must still be deletable. NULL contexts or
        /// options are safe no-ops.
        ///
        /// # Safety
        ///
        /// Both pointers must be NULL or live (context from the matching
        /// maker, options from `sass_make_options`); must not alias.
        #[no_mangle]
        pub unsafe extern "C" fn $set_opts(ctx: *mut $box, opt: *mut OptionsBox) {
            guard((), || {
                let (Some(c), Some(o)) = (unsafe { $ctx_fn(ctx) }, unsafe { opts(opt) }) else {
                    return;
                };
                // SAFETY: both live and non-aliasing per contract; the move
                // transfers ownership field-by-field, leaving `o` defaulted.
                move_options_into(&mut c.common.options, o);
            })
        }
    };
}

context_options_access!(
    sass_file_context_get_context,
    sass_file_context_get_options,
    sass_file_context_set_options,
    FileContextBox,
    file_ctx
);
context_options_access!(
    sass_data_context_get_context,
    sass_data_context_get_options,
    sass_data_context_set_options,
    DataContextBox,
    data_ctx
);

/// Returns the embedded options from a `Sass_Context` upcast pointer
/// (mirrors `sass_context_get_options`, sass_context.cpp:605).
///
/// # Safety
///
/// `ctx` must be NULL or a `*mut ContextCommon` previously returned by a
/// `*_get_context` call.
#[no_mangle]
pub unsafe extern "C" fn sass_context_get_options(ctx: *mut ContextCommon) -> *mut OptionsBox {
    guard(ptr::null_mut(), || {
        if ctx.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: live common prefix per contract.
        unsafe { &mut (*ctx).options as *mut OptionsBox }
    })
}

/// Runs a data-context compile now (shared by the one-shot entry point and
/// staged `execute`). The caller guarantees `c` is live.
fn compile_data_now(c: &mut DataContextBox) -> c_int {
    if c.common.result.error_status != 0 {
        return c.common.result.error_status;
    }
    if c.source.is_null() {
        c.common
            .fail_with_message("Data context has no source string");
        return c.common.result.error_status;
    }
    // SAFETY: non-null owned buffer per construction.
    let source = unsafe { CStr::from_ptr(c.source) }.to_bytes();
    let source_text = String::from_utf8_lossy(source).into_owned();
    // SAFETY: options live inside the live context.
    let snapshot = unsafe { CompileSnapshot::take(&c.common.options) };
    run_data_compile(&mut c.common, &source_text, &snapshot);
    c.common.result.error_status
}

/// Compiles a data context (one-shot, synchronous). NULL → 1; pre-existing
/// `error_status` early-returns it; missing source errors like upstream
/// (`"Data context has no source string"`, sass_context.cpp:397-410).
///
/// # Safety
///
/// `ctx` must be NULL or a live pointer from [`sass_make_data_context`].
#[no_mangle]
pub unsafe extern "C" fn sass_compile_data_context(ctx: *mut DataContextBox) -> c_int {
    guard(1, || {
        let Some(c) = (unsafe { data_ctx(ctx) }) else {
            return 1;
        };
        compile_data_now(c)
    })
}

/// Runs a file-context compile now (shared by the one-shot entry point and
/// staged `execute`). The caller guarantees `c` is live.
fn compile_file_now(c: &mut FileContextBox) -> c_int {
    if c.common.result.error_status != 0 {
        return c.common.result.error_status;
    }
    // SAFETY: options live inside the live context.
    let path = unsafe { c.common.options.input_path.bytes() }.map(|b| b.to_vec());
    match path {
        Some(bytes) if !bytes.is_empty() => {
            let path_text = String::from_utf8_lossy(&bytes).into_owned();
            // SAFETY: options live inside the live context.
            let snapshot = unsafe { CompileSnapshot::take(&c.common.options) };
            run_file_compile(&mut c.common, &path_text, &snapshot);
            c.common.result.error_status
        }
        _ => {
            c.common.fail_with_message("File context has no input path");
            c.common.result.error_status
        }
    }
}

/// Compiles a file context (one-shot, synchronous). NULL → 1; pre-existing
/// `error_status` early-returns it; missing/empty `input_path` errors like
/// upstream (sass_context.cpp:412-424).
///
/// # Safety
///
/// `ctx` must be NULL or a live pointer from [`sass_make_file_context`].
#[no_mangle]
pub unsafe extern "C" fn sass_compile_file_context(ctx: *mut FileContextBox) -> c_int {
    guard(1, || {
        let Some(c) = (unsafe { file_ctx(ctx) }) else {
            return 1;
        };
        compile_file_now(c)
    })
}

// ============================================================================
// Staged compiler
// ============================================================================

/// C ABI compiler-state constants, matching `Sass_Compiler_State` numeric
/// order (`CREATED=0, PARSED=1, EXECUTED=2`, context.h:25-29).
pub const SASS_COMPILER_CREATED: u32 = 0;
pub const SASS_COMPILER_PARSED: u32 = 1;
pub const SASS_COMPILER_EXECUTED: u32 = 2;

/// Which context flavor a staged compiler borrows (raw pointers — the
/// compiler never owns the context, mirroring `sass_delete_compiler`
/// freeing everything *except* contexts/options, sass_context.cpp:566-577).
#[derive(Clone, Copy)]
enum CompilerKind {
    File(*mut FileContextBox),
    Data(*mut DataContextBox),
}

/// The `Sass_Compiler` heap struct: a lifecycle state plus the borrowed
/// context. Unlike upstream's handle (which doubles as the callback token
/// via `cpp_ctx->c_compiler`), per-callback tokens stay ephemeral
/// [`CompilerBox`](crate::functions::CompilerBox)es here; this handle only
/// drives the make → parse → execute lifecycle.
pub struct StagedCompilerBox {
    state: u32,
    kind: CompilerKind,
}

/// Reads a live staged compiler (NULL → None).
///
/// # Safety
///
/// `compiler` must be NULL or a live pointer from
/// [`sass_make_file_compiler`] / [`sass_make_data_compiler`], and its
/// context must still be live — the compiler borrows it, so deleting the
/// context first is upstream-UB too.
unsafe fn staged(compiler: *mut StagedCompilerBox) -> Option<&'static mut StagedCompilerBox> {
    if compiler.is_null() {
        return None;
    }
    // SAFETY: live box per contract; borrow lasts only for the enclosing call.
    Some(unsafe { &mut *compiler })
}

/// Creates a staged compiler borrowing a file context (NULL → NULL).
/// Options are snapshotted at `parse`, not here, so `set_options` between
/// make and parse is honored (a benign superset of upstream, which copies
/// options into its C++ context at make time).
///
/// # Safety
///
/// `ctx` must be NULL or a live pointer from [`sass_make_file_context`].
/// The caller owns the result (frees with [`sass_delete_compiler`]); the
/// context must outlive it.
#[no_mangle]
pub unsafe extern "C" fn sass_make_file_compiler(
    ctx: *mut FileContextBox,
) -> *mut StagedCompilerBox {
    guard(ptr::null_mut(), || {
        if ctx.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: fresh box; ownership moves to the caller.
        Box::into_raw(Box::new(StagedCompilerBox {
            state: SASS_COMPILER_CREATED,
            kind: CompilerKind::File(ctx),
        }))
    })
}

/// Creates a staged compiler borrowing a data context (NULL → NULL).
///
/// # Safety
///
/// `ctx` must be NULL or a live pointer from [`sass_make_data_context`].
/// The caller owns the result (frees with [`sass_delete_compiler`]); the
/// context must outlive it.
#[no_mangle]
pub unsafe extern "C" fn sass_make_data_compiler(
    ctx: *mut DataContextBox,
) -> *mut StagedCompilerBox {
    guard(ptr::null_mut(), || {
        if ctx.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: fresh box; ownership moves to the caller.
        Box::into_raw(Box::new(StagedCompilerBox {
            state: SASS_COMPILER_CREATED,
            kind: CompilerKind::Data(ctx),
        }))
    })
}

/// Runs the staged parse phase for a data context: parses the entry and
/// stores the discovered file list (no CSS yet). Failures are stored on the
/// context; the phase itself always reports "attempted".
fn parse_data_now(c: &mut DataContextBox) {
    if c.source.is_null() {
        c.common
            .fail_with_message("Data context has no source string");
        return;
    }
    // SAFETY: non-null owned buffer per construction.
    let source = unsafe { CStr::from_ptr(c.source) }.to_bytes();
    let source_text = String::from_utf8_lossy(source).into_owned();
    // SAFETY: options live inside the live context.
    let snapshot = unsafe { CompileSnapshot::take(&c.common.options) };
    run_data_parse(&mut c.common, &source_text, &snapshot);
}

/// Runs the staged parse phase for a file context: reads + parses the entry
/// and stores the discovered file list (no CSS yet). Input-path handling
/// mirrors the one-shot path; failures are stored on the context.
fn parse_file_now(c: &mut FileContextBox) {
    // SAFETY: options live inside the live context.
    let path = unsafe { c.common.options.input_path.bytes() }.map(|b| b.to_vec());
    let Some(bytes) = path.filter(|b| !b.is_empty()) else {
        c.common.fail_with_message("File context has no input path");
        return;
    };
    let path_text = String::from_utf8_lossy(&bytes).into_owned();
    // SAFETY: options live inside the live context.
    let snapshot = unsafe { CompileSnapshot::take(&c.common.options) };
    run_file_parse(&mut c.common, &path_text, &snapshot);
}

/// Parses one data-context entry and stores its import closure's file list.
///
/// The entry parses with `parse_selectors: false` like `compile_string`;
/// custom importers bridge with the entry token so discovery canonicalizes
/// exactly like evaluation. Discovery warnings are dropped (`NoOpWarnLogger`
/// exists for speculative probes) — `execute` re-emits them through the
/// real pipeline, so nothing is lost and nothing renders twice.
fn run_data_parse(common: &mut ContextCommon, source: &str, snapshot: &CompileSnapshot) {
    let arena = Bump::new();
    let io: Rc<dyn Io> = Rc::new(DefaultIo::new());
    // `snapshot.syntax()` yields only Sass/Scss for data entries (never Css).
    let stylesheet = match snapshot.syntax() {
        Syntax::Sass(_) => Stylesheet::parse_sass(source, None, false, &arena),
        _ => Stylesheet::parse_scss(source, None, false, &arena),
    };
    let stylesheet = match stylesheet {
        Ok(s) => s,
        Err(e) => {
            common.fail_with_error(&e, &*io);
            return;
        }
    };
    match discover_imports(
        &stylesheet,
        None,
        snapshot,
        &token_entry("stdin", &*io),
        io.clone(),
        &arena,
    ) {
        Ok(urls) => {
            common.result.clear();
            store_files(&mut common.result, &urls, true);
        }
        Err(e) => common.fail_with_error(&e, &*io),
    }
}

/// Parses one file-context entry and stores its import closure's file list.
///
/// Entry loading mirrors `compile()` (compile/mod.rs:378-418): read,
/// canonicalize, `file:` URL, UTF-8 decode, syntax from the extension unless
/// the indented flag forces Sass. The entry URL seeds the closure (pinned
/// first by `store_files`, like upstream).
fn run_file_parse(common: &mut ContextCommon, path: &str, snapshot: &CompileSnapshot) {
    let arena = Bump::new();
    let io: Rc<dyn Io> = Rc::new(DefaultIo::new());
    let source_bytes = match io.read_file(Path::new(path)) {
        Ok(b) => b,
        Err(e) => {
            common.fail_with_error(
                &SassError::Script {
                    message: format!("Error reading {path}: {e}."),
                    argument_name: None,
                },
                &*io,
            );
            return;
        }
    };
    let abs_path = match io.canonicalize(Path::new(path)) {
        Ok(p) => PathBuf::from(p).to_string_lossy().into_owned(),
        Err(e) => {
            common.fail_with_error(
                &SassError::Script {
                    message: format!("Error resolving {path}: {e}."),
                    argument_name: None,
                },
                &*io,
            );
            return;
        }
    };
    let file_url = match SassUrl::file_url_from_abs_path(&abs_path) {
        Ok(u) => u,
        Err(_) => {
            common.fail_with_message(&format!("Could not create URL from path: {path}"));
            return;
        }
    };
    let source = match decode_source_utf8(&source_bytes, Some(file_url.clone()), &arena) {
        Ok(s) => s,
        Err(e) => {
            common.fail_with_error(&e, &*io);
            return;
        }
    };
    let syntax = if snapshot.is_indented_syntax_src {
        snapshot.syntax()
    } else {
        syntax_for_path(&abs_path)
    };
    let stylesheet = match syntax {
        Syntax::Sass(_) => Stylesheet::parse_sass(source, Some(&file_url), false, &arena),
        Syntax::Scss => Stylesheet::parse_scss(source, Some(&file_url), false, &arena),
        Syntax::Css(_) => Stylesheet::parse_css(
            source,
            Some(&file_url),
            false,
            &HashSet::<String>::new(),
            &arena,
        ),
    };
    let stylesheet = match stylesheet {
        Ok(s) => s,
        Err(e) => {
            common.fail_with_error(&e, &*io);
            return;
        }
    };
    match discover_imports(
        &stylesheet,
        Some(&file_url),
        snapshot,
        &token_entry(path, &*io),
        io.clone(),
        &arena,
    ) {
        Ok(deps) => {
            let mut all = vec![file_url];
            all.extend(deps);
            common.result.clear();
            store_files(&mut common.result, &all, false);
        }
        Err(e) => common.fail_with_error(&e, &*io),
    }
}

/// Discovers the import closure of a parsed entry: the canonical URLs of
/// every statically-discoverable `@use`/`@forward`/`@import`, transitively.
///
/// Each URL canonicalizes through a scratch [`ImportCache`] built exactly
/// like the compile pipeline's (custom C importers, load paths, `SASS_PATH`;
/// `NoOpWarnLogger` for the probe warnings `execute` will re-emit), with
/// the same base-importer shape evaluation uses (`load_stylesheet` passes
/// the default importer with the containing URL as base). Transitive loads
/// go through `import_canonical`, so nested stylesheets parse with the same
/// code evaluation loads with.
///
/// Best-effort by design: `find_dependencies` covers only top-level static
/// declarations (control flow, interpolations, and dynamic imports are not
/// statically analyzable), and unresolvable URLs fall through for `execute`
/// to report ("Can't find stylesheet"). Canonicalization *failures*
/// (e.g. a C importer error) fail the parse like upstream.
#[allow(clippy::too_many_arguments)]
fn discover_imports<'a>(
    stylesheet: &Stylesheet<'a>,
    base_url: Option<&SassUrl>,
    snapshot: &CompileSnapshot,
    token: &ImportStackEntry,
    io: Rc<dyn Io>,
    arena: &'a Bump,
) -> SassResult<Vec<SassUrl>> {
    // Direct calls, never through `maybe_block_on!`: the adapter is
    // sync-only (see `run_data_compile`), so the core entry points below are
    // plain sync functions here — an async core underneath would fail loudly
    // at compile time instead of silently changing driving behavior.
    let custom = build_custom_importers(snapshot.c_importers, arena, token)?;
    let sass_path = std::env::var("SASS_PATH").unwrap_or_default();
    let mut cache = ImportCache::new_with_options(
        arena,
        custom,
        snapshot.load_paths(),
        &sass_path,
        false,
        io.clone(),
        None,
    );
    let default = Importer::new(
        arena,
        ImporterKind::Filesystem(FilesystemImporter::new_no_load_path(io)),
    );
    let mut visited = HashSet::new();
    if let Some(entry) = base_url {
        visited.insert(entry.to_string());
    }
    let mut out = Vec::new();
    discover_step(
        stylesheet,
        base_url,
        &mut cache,
        &default,
        &mut visited,
        &mut out,
    )?;
    Ok(out)
}

/// Walks one parsed stylesheet's static dependencies into `out`, recursing
/// into each loaded stylesheet. `visited` (by canonical URL string) stops
/// import cycles; `out` collects canonical URLs in first-discovery order
/// (the caller sorts via `store_files`, so set-iteration order never leaks
/// into the C array).
fn discover_step<'a>(
    sheet: &Stylesheet<'a>,
    base_url: Option<&SassUrl>,
    cache: &mut ImportCache<'a, 'a>,
    default: &Importer<'a>,
    visited: &mut HashSet<String>,
    out: &mut Vec<SassUrl>,
) -> SassResult<()> {
    let report = find_dependencies(sheet)?;
    // The report sets iterate nondeterministically; sort for a stable
    // canonicalization order so custom-importer side effects don't depend on
    // hash order run to run.
    let modules = report.modules();
    let mut deps: Vec<(&SassUrl, bool)> = Vec::new();
    for url in &modules {
        deps.push((url, false));
    }
    for url in &report.imports {
        deps.push((url, true));
    }
    deps.sort_by(|a, b| a.0.to_string().cmp(&b.0.to_string()));
    let noop = NoOpWarnLogger;
    for (url, for_import) in deps {
        let canonical = match cache.canonicalize(url, Some(default), base_url, for_import, &noop)? {
            Some(cr) => cr,
            // Fallthrough (empty lists included): `execute` reports it.
            None => continue,
        };
        if !visited.insert(canonical.canonical_url.to_string()) {
            continue;
        }
        out.push(canonical.canonical_url.clone());
        let loaded = cache.import_canonical(
            &canonical.importer,
            &canonical.canonical_url,
            Some(&canonical.original_url),
        )?;
        if let Some(sheet) = loaded {
            discover_step(
                sheet,
                Some(&canonical.canonical_url),
                cache,
                default,
                visited,
                out,
            )?;
        }
    }
    Ok(())
}

/// Parses a staged compiler's entry (mirrors `sass_compiler_parse`,
/// sass_context.cpp:426-439): NULL → 1; already-`PARSED` → 0 (idempotent);
/// state ≠ `CREATED` → -1; pre-existing context errors return WITHOUT
/// advancing state. Otherwise advances to `PARSED` first (a failed parse
/// still advances, like `sass_parse_block`) and always returns 0 — failures
/// surface via the context fields only.
///
/// # Safety
///
/// `compiler` must be NULL or a live staged handle whose context is live.
#[no_mangle]
pub unsafe extern "C" fn sass_compiler_parse(compiler: *mut StagedCompilerBox) -> c_int {
    guard(1, || {
        let Some(sc) = (unsafe { staged(compiler) }) else {
            return 1;
        };
        if sc.state == SASS_COMPILER_PARSED {
            return 0;
        }
        if sc.state != SASS_COMPILER_CREATED {
            return -1;
        }
        let kind = sc.kind;
        let prior = match kind {
            CompilerKind::File(ptr) => unsafe {
                file_ctx(ptr).map(|c| c.common.result.error_status)
            },
            CompilerKind::Data(ptr) => unsafe {
                data_ctx(ptr).map(|c| c.common.result.error_status)
            },
        };
        let Some(prior) = prior else { return 1 };
        if prior != 0 {
            return prior;
        }
        sc.state = SASS_COMPILER_PARSED;
        match kind {
            CompilerKind::File(ptr) => {
                // SAFETY: validated live above; the context outlives the
                // compiler per contract.
                let Some(c) = (unsafe { file_ctx(ptr) }) else {
                    return 1;
                };
                parse_file_now(c);
            }
            CompilerKind::Data(ptr) => {
                // SAFETY: validated live above; the context outlives the
                // compiler per contract.
                let Some(c) = (unsafe { data_ctx(ptr) }) else {
                    return 1;
                };
                parse_data_now(c);
            }
        }
        0
    })
}

/// Executes a parsed staged compiler (mirrors `sass_compiler_execute`,
/// sass_context.cpp:441-462): NULL → 1; already-`EXECUTED` → 0; state ≠
/// `PARSED` → -1; a failed parse's stored error returns WITHOUT advancing
/// (state stays `PARSED`). Otherwise advances to `EXECUTED` and runs the
/// full compile through the shared one-shot path.
///
/// # Safety
///
/// `compiler` must be NULL or a live staged handle whose context is live.
#[no_mangle]
pub unsafe extern "C" fn sass_compiler_execute(compiler: *mut StagedCompilerBox) -> c_int {
    guard(1, || {
        let Some(sc) = (unsafe { staged(compiler) }) else {
            return 1;
        };
        if sc.state == SASS_COMPILER_EXECUTED {
            return 0;
        }
        if sc.state != SASS_COMPILER_PARSED {
            return -1;
        }
        let kind = sc.kind;
        let prior = match kind {
            CompilerKind::File(ptr) => unsafe {
                file_ctx(ptr).map(|c| c.common.result.error_status)
            },
            CompilerKind::Data(ptr) => unsafe {
                data_ctx(ptr).map(|c| c.common.result.error_status)
            },
        };
        let Some(prior) = prior else { return 1 };
        if prior != 0 {
            return prior;
        }
        // Upstream marks EXECUTED before rendering (sass_context.cpp:451).
        sc.state = SASS_COMPILER_EXECUTED;
        match kind {
            CompilerKind::File(ptr) => {
                // SAFETY: validated live above; the context outlives the
                // compiler per contract.
                let Some(c) = (unsafe { file_ctx(ptr) }) else {
                    return 1;
                };
                compile_file_now(c)
            }
            CompilerKind::Data(ptr) => {
                // SAFETY: validated live above; the context outlives the
                // compiler per contract.
                let Some(c) = (unsafe { data_ctx(ptr) }) else {
                    return 1;
                };
                compile_data_now(c)
            }
        }
    })
}

/// Deletes a staged compiler and nothing else (mirrors `sass_delete_compiler`,
/// sass_context.cpp:566-577): the borrowed context and options survive.
/// NULL-safe.
///
/// # Safety
///
/// `compiler` must be NULL or a live pointer from
/// [`sass_make_file_compiler`] / [`sass_make_data_compiler`], freed exactly
/// once.
#[no_mangle]
pub unsafe extern "C" fn sass_delete_compiler(compiler: *mut StagedCompilerBox) {
    guard((), || {
        if compiler.is_null() {
            return;
        }
        // SAFETY: live box per contract; the borrowed context is untouched.
        unsafe {
            drop(Box::from_raw(compiler));
        }
    });
}

/// Reads the staged compiler state (mirrors `sass_compiler_get_state`).
/// NULL hardens to `CREATED` (upstream would crash).
///
/// # Safety
///
/// `compiler` must be NULL or a live staged handle for the call.
#[no_mangle]
pub unsafe extern "C" fn sass_compiler_get_state(compiler: *mut StagedCompilerBox) -> u32 {
    guard(SASS_COMPILER_CREATED, || {
        unsafe { staged(compiler) }
            .map(|c| c.state)
            .unwrap_or(SASS_COMPILER_CREATED)
    })
}

/// Returns the borrowed context of a staged compiler (mirrors
/// `sass_compiler_get_context`). NULL hardens to NULL.
///
/// # Safety
///
/// `compiler` must be NULL or a live staged handle for the call. Borrowed;
/// freed with the context — never delete it.
#[no_mangle]
pub unsafe extern "C" fn sass_compiler_get_context(
    compiler: *mut StagedCompilerBox,
) -> *mut ContextCommon {
    guard(ptr::null_mut(), || {
        let Some(sc) = (unsafe { staged(compiler) }) else {
            return ptr::null_mut();
        };
        let common = match sc.kind {
            CompilerKind::File(ptr) => {
                unsafe { file_ctx(ptr) }.map(|c| &mut c.common as *mut ContextCommon)
            }
            CompilerKind::Data(ptr) => {
                unsafe { data_ctx(ptr) }.map(|c| &mut c.common as *mut ContextCommon)
            }
        };
        common.unwrap_or(ptr::null_mut())
    })
}

/// Returns the borrowed options of a staged compiler's context (mirrors
/// `sass_compiler_get_options`). NULL hardens to NULL.
///
/// # Safety
///
/// `compiler` must be NULL or a live staged handle for the call. Borrowed;
/// freed with the context — never delete it.
#[no_mangle]
pub unsafe extern "C" fn sass_compiler_get_options(
    compiler: *mut StagedCompilerBox,
) -> *mut OptionsBox {
    guard(ptr::null_mut(), || {
        let Some(sc) = (unsafe { staged(compiler) }) else {
            return ptr::null_mut();
        };
        let opts = match sc.kind {
            CompilerKind::File(ptr) => {
                unsafe { file_ctx(ptr) }.map(|c| &mut c.common.options as *mut OptionsBox)
            }
            CompilerKind::Data(ptr) => {
                unsafe { data_ctx(ptr) }.map(|c| &mut c.common.options as *mut OptionsBox)
            }
        };
        opts.unwrap_or(ptr::null_mut())
    })
}

macro_rules! result_getters {
    ($(($get:ident, $field:ident, $ty:ty, $safety:expr)),*) => {
        $(
            /// Reads the result field; NULL contexts harden to zero/NULL.
            ///
            /// # Safety
            ///
            #[doc = $safety]
            #[no_mangle]
            pub unsafe extern "C" fn $get(ctx: *mut ContextCommon) -> $ty {
                guard(<$ty>::default(), || {
                    if ctx.is_null() {
                        return <$ty>::default();
                    }
                    // SAFETY: live common prefix per contract.
                    unsafe { (*ctx).result.$field as $ty }
                })
            }
        )*
    };
}

result_getters!(
    (sass_context_get_error_status, error_status, c_int,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call."),
    (sass_context_get_error_json, error_json, *const c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Borrowed; freed with the context."),
    (sass_context_get_error_text, error_text, *const c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Borrowed; freed with the context."),
    (sass_context_get_error_message, error_message, *const c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Borrowed; freed with the context."),
    (sass_context_get_error_file, error_file, *const c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Borrowed; freed with the context."),
    (sass_context_get_error_src, error_src, *const c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Borrowed; freed with the context."),
    (sass_context_get_output_string, output_string, *const c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Borrowed; freed with the context."),
    (sass_context_get_source_map_string, source_map_string, *const c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Borrowed; freed with the context."),
    (sass_context_get_included_files, included_files, *const *const c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Borrowed NULL-terminated array; freed with the context.")
);

/// Reads `error_line` (1-based; `usize::MAX` when no location). NULL hardens
/// to 0 — the C type is `size_t`, which has no error value; callers check
/// `error_status` first (mirroring upstream's `npos` reset semantics where
/// the field is only meaningful on failure).
///
/// # Safety
///
/// See the other result getters.
#[no_mangle]
pub unsafe extern "C" fn sass_context_get_error_line(ctx: *mut ContextCommon) -> usize {
    guard(0, || {
        if ctx.is_null() {
            return 0;
        }
        // SAFETY: live common prefix per contract.
        let line = unsafe { (*ctx).result.error_line };
        if line == usize::MAX {
            0
        } else {
            line
        }
    })
}

/// Reads `error_column` (same contract as
/// [`sass_context_get_error_line`]).
///
/// # Safety
///
/// See the other result getters.
#[no_mangle]
pub unsafe extern "C" fn sass_context_get_error_column(ctx: *mut ContextCommon) -> usize {
    guard(0, || {
        if ctx.is_null() {
            return 0;
        }
        // SAFETY: live common prefix per contract.
        let col = unsafe { (*ctx).result.error_column };
        if col == usize::MAX {
            0
        } else {
            col
        }
    })
}

/// Counts the NULL-terminated included-files array (mirrors
/// `sass_context_get_included_files_size`, sass_context.cpp:624-626).
/// NULL hardens to 0.
///
/// # Safety
///
/// See the other result getters.
#[no_mangle]
pub unsafe extern "C" fn sass_context_get_included_files_size(ctx: *mut ContextCommon) -> usize {
    guard(0, || {
        if ctx.is_null() {
            return 0;
        }
        // SAFETY: live common prefix; array is NULL-terminated per construction.
        let mut count = 0;
        unsafe {
            let mut cur = (*ctx).result.included_files;
            // The terminator is a NULL *element*: test the element itself,
            // never dereference it (mirrors upstream's `while (i && *i)`).
            while !cur.is_null() && !(*cur).is_null() {
                // SAFETY: `cur` points into the live array; `*cur` is a live string.
                cur = cur.add(1);
                count += 1;
            }
        }
        count
    })
}

macro_rules! result_takers {
    ($(($take:ident, $field:ident, $ty:ty, $safety:expr)),*) => {
        $(
            /// Takes ownership of the result field (value on the context is
            /// set to NULL); NULL contexts harden to NULL. The caller owns
            /// the result (frees with `sass_free_memory`; arrays: each
            /// string, then the array).
            ///
            /// # Safety
            ///
            #[doc = $safety]
            #[no_mangle]
            pub unsafe extern "C" fn $take(ctx: *mut ContextCommon) -> $ty {
                guard(ptr::null_mut(), || {
                    if ctx.is_null() {
                        return ptr::null_mut();
                    }
                    // SAFETY: live common prefix per contract; the field is
                    // read out and nulled so a later delete cannot free it.
                    unsafe {
                        let taken = (*ctx).result.$field;
                        (*ctx).result.$field = ptr::null_mut();
                        taken
                    }
                })
            }
        )*
    };
}

result_takers!(
    (sass_context_take_error_json, error_json, *mut c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Caller owns the result."),
    (sass_context_take_error_text, error_text, *mut c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Caller owns the result."),
    (sass_context_take_error_message, error_message, *mut c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Caller owns the result."),
    (sass_context_take_error_file, error_file, *mut c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Caller owns the result."),
    (sass_context_take_error_src, error_src, *mut c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Caller owns the result."),
    (sass_context_take_output_string, output_string, *mut c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Caller owns the result."),
    (sass_context_take_source_map_string, source_map_string, *mut c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Caller owns the result."),
    (sass_context_take_included_files, included_files, *mut *mut c_char,
        "`ctx` must be NULL or a `*mut ContextCommon` from a `*_get_context` call. Caller owns the array and its strings.")
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::sass_free_memory;
    use crate::options::sass_delete_options;
    use crate::options::sass_make_options;
    use crate::options::sass_option_get_output_style;
    use crate::options::sass_option_set_output_style;
    use std::ffi::CString;
    use std::path::PathBuf;

    unsafe fn read_opt(ptr: *const c_char) -> Option<Vec<u8>> {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: test-only; non-null pointers are valid strings produced by
        // the functions under test.
        Some(unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec())
    }

    unsafe fn make_data_ctx(source: &str) -> *mut DataContextBox {
        let owned = CString::new(source).unwrap();
        let buf = copy_bytes_nul(owned.to_bytes());
        let ctx = unsafe { sass_make_data_context(buf) };
        assert!(!ctx.is_null());
        ctx
    }

    #[test]
    fn libsass_file_key_vectors() {
        // Same dir: basename only (node-sass api.js:33-53).
        assert_eq!(
            libsass_file_key(
                Some("/app/index-test.css"),
                Some("/app/index-test.css.map"),
                None,
                "/app"
            ),
            "index-test.css"
        );
        // Map nested deeper: one `../` per map-dir level (api.js:55-63).
        assert_eq!(
            libsass_file_key(
                Some("./index-test.css"),
                Some("./deep/nested/index.map"),
                None,
                "/app"
            ),
            "../../index-test.css"
        );
        // Disjoint trees via common ancestor (cli.js:498-512).
        assert_eq!(
            libsass_file_key(
                Some("css/nested/three.css"),
                Some("map/nested/three.css.map"),
                None,
                "/app"
            ),
            "../../css/nested/three.css"
        );
        // Missing output derives from input minus extension + `.css`.
        assert_eq!(
            libsass_file_key(None, Some("/app/out.map"), Some("/app/in.scss"), "/app"),
            "in.css"
        );
        // Missing everything falls back to `"stdout"`.
        assert_eq!(
            libsass_file_key(None, Some("/app/out.map"), None, "/app"),
            "stdout"
        );
        // Empty map file (embed path): relative to CWD.
        assert_eq!(
            libsass_file_key(Some("/app/a/b.css"), Some(""), None, "/app"),
            "a/b.css"
        );
        // Protocol outputs pass through verbatim.
        assert_eq!(
            libsass_file_key(
                Some("https://cdn/x/y.css"),
                Some("/app/out.map"),
                None,
                "/app"
            ),
            "https://cdn/x/y.css"
        );
    }

    #[test]
    fn map_source_vectors() {
        // Default: entries relativize against the map file (upstream
        // `context.cpp:263`), `file:` URLs becoming plain relative paths.
        assert_eq!(
            map_source("file:///app/css/a.scss", "/app/map/o.map", false, "/app"),
            "../css/a.scss"
        );
        assert_eq!(
            map_source("file:///app/a.scss", "/app/a.map", false, "/app"),
            "a.scss"
        );
        // `file_urls` flag: relatives absolutize, absolute URLs pass through.
        assert_eq!(
            map_source("css/a.scss", "/app/o.map", true, "/app"),
            "file:///css/a.scss"
        );
        assert_eq!(
            map_source("file:///app/a.scss", "/app/o.map", true, "/app"),
            "file:///app/a.scss"
        );
        // Non-file schemes pass through in both modes.
        assert_eq!(
            map_source("sass-c-importer://i/0", "/app/o.map", false, "/app"),
            "sass-c-importer://i/0"
        );
        assert_eq!(map_source("", "/app/o.map", false, "/app"), "");
    }

    /// Reads a NULL-terminated C string array into Rust strings.
    ///
    /// # Safety
    ///
    /// `arr` must be NULL or a live NULL-terminated array of live strings.
    unsafe fn read_files(arr: *mut *mut c_char) -> Vec<String> {
        let mut out = Vec::new();
        if arr.is_null() {
            return out;
        }
        // SAFETY: per contract above.
        unsafe {
            let mut cur = arr;
            while !(*cur).is_null() {
                let bytes = read_opt(*cur as *const c_char).unwrap_or_default();
                out.push(String::from_utf8_lossy(&bytes).into_owned());
                cur = cur.add(1);
            }
        }
        out
    }

    #[test]
    fn included_files_sorted_entry_first() {
        // File context: entry pinned first, rest sorted (mirrors
        // `get_included_files` + `sort(begin+1, end)`).
        let mut common = ContextCommon {
            options: OptionsBox::new(),
            result: ContextResult::new(),
        };
        let urls = [
            "file:///app/main.scss",
            "file:///app/zebra.scss",
            "file:///app/apple.scss",
            "file:///app/mango.scss",
        ]
        .map(|s| SassUrl::parse(s).unwrap());
        common.store_success("", None, None, &urls, false);
        assert_eq!(
            unsafe { read_files(common.result.included_files) },
            [
                "/app/main.scss",
                "/app/apple.scss",
                "/app/mango.scss",
                "/app/zebra.scss"
            ]
        );

        // Data context: stdin skipped, everything sorted.
        let mut common = ContextCommon {
            options: OptionsBox::new(),
            result: ContextResult::new(),
        };
        let urls =
            ["stdin", "file:///b.scss", "file:///a.scss"].map(|s| SassUrl::parse(s).unwrap());
        common.store_success("", None, None, &urls, true);
        assert_eq!(
            unsafe { read_files(common.result.included_files) },
            ["/a.scss", "/b.scss"]
        );
    }

    #[test]
    fn data_roundtrip_success() {
        unsafe {
            let ctx = make_data_ctx("a { b: c; }");
            assert_eq!(sass_compile_data_context(ctx), 0);
            let base = sass_data_context_get_context(ctx);
            assert_eq!(sass_context_get_error_status(base), 0);
            let css =
                String::from_utf8(read_opt(sass_context_get_output_string(base)).unwrap()).unwrap();
            assert!(css.contains("a {"), "got: {css}");
            assert!(css.contains("b: c"), "got: {css}");
            sass_delete_data_context(ctx);
        }
    }

    #[test]
    fn data_error_fields() {
        unsafe {
            let ctx = make_data_ctx("a { b: ; }");
            assert_ne!(sass_compile_data_context(ctx), 0);
            let base = sass_data_context_get_context(ctx);
            assert_ne!(sass_context_get_error_status(base), 0);
            assert!(sass_context_get_output_string(base).is_null());
            let msg =
                String::from_utf8(read_opt(sass_context_get_error_message(base)).unwrap()).unwrap();
            assert!(msg.contains("Error"), "got: {msg}");
            assert!(read_opt(sass_context_get_error_text(base)).is_some());
            assert!(read_opt(sass_context_get_error_json(base)).is_some());
            // Span-derived location is 1-based and consistent.
            let json =
                String::from_utf8(read_opt(sass_context_get_error_json(base)).unwrap()).unwrap();
            assert!(json.contains("\"status\": 1"), "got: {json}");
            assert!(json.contains("\"message\""), "got: {json}");
            let line = sass_context_get_error_line(base);
            let col = sass_context_get_error_column(base);
            assert!(line >= 1 && col >= 1, "line={line} col={col}");
            sass_delete_data_context(ctx);
        }
    }

    #[test]
    fn maker_errors_and_move() {
        unsafe {
            // NULL source: maker error, still deletable.
            let null_ctx = sass_make_data_context(ptr::null_mut());
            assert!(!null_ctx.is_null());
            assert_ne!(
                sass_context_get_error_status(sass_data_context_get_context(null_ctx)),
                0
            );
            assert_eq!(sass_compile_data_context(null_ctx), 1);
            sass_delete_data_context(null_ctx);
            // set_options move is observed by the context.
            let ctx = make_data_ctx("a { b: c; }");
            let opts = sass_make_options();
            sass_option_set_output_style(opts, SASS_STYLE_COMPRESSED);
            sass_data_context_set_options(ctx, opts);
            assert_eq!(
                sass_option_get_output_style(sass_data_context_get_options(ctx)),
                SASS_STYLE_COMPRESSED
            );
            sass_delete_options(opts);
            assert_eq!(sass_compile_data_context(ctx), 0);
            let base = sass_data_context_get_context(ctx);
            let css =
                String::from_utf8(read_opt(sass_context_get_output_string(base)).unwrap()).unwrap();
            assert!(!css.contains('\n') || !css.contains("  "), "got: {css:?}");
            sass_delete_data_context(ctx);
        }
    }

    #[test]
    fn file_context_missing_input() {
        unsafe {
            let path = CString::new("/no/such/file-xyz.scss").unwrap();
            let ctx = sass_make_file_context(path.as_ptr());
            assert!(!ctx.is_null());
            assert_ne!(sass_compile_file_context(ctx), 0);
            let base = sass_file_context_get_context(ctx);
            assert_ne!(sass_context_get_error_status(base), 0);
            assert!(read_opt(sass_context_get_error_message(base)).is_some());
            sass_delete_file_context(ctx);
            assert_eq!(sass_compile_file_context(ptr::null_mut()), 1);
            assert_eq!(sass_compile_data_context(ptr::null_mut()), 1);
        }
    }

    #[test]
    fn indent_and_style_snapshot() {
        unsafe {
            let ctx = make_data_ctx("a { b: c; }");
            let opts = sass_data_context_get_options(ctx);
            sass_option_set_output_style(opts, SASS_STYLE_COMPRESSED);
            assert_eq!(sass_compile_data_context(ctx), 0);
            let base = sass_data_context_get_context(ctx);
            let css =
                String::from_utf8(read_opt(sass_context_get_output_string(base)).unwrap()).unwrap();
            // Compressed + trailing linefeed parity (output.cpp:67-70).
            assert_eq!(css, "a{b:c}\n");
            sass_delete_data_context(ctx);
        }
    }

    /// Reads a NULL-terminated C string array into Rust strings.
    ///
    /// # Safety
    ///
    /// `arr` must be NULL or a live NULL-terminated array of live strings.
    unsafe fn read_staged_files(arr: *mut *mut c_char) -> Vec<String> {
        let mut out = Vec::new();
        if arr.is_null() {
            return out;
        }
        // SAFETY: per contract above.
        unsafe {
            let mut cur = arr;
            while !(*cur).is_null() {
                let bytes = read_opt(*cur as *const c_char).unwrap_or_default();
                out.push(String::from_utf8_lossy(&bytes).into_owned());
                cur = cur.add(1);
            }
        }
        out
    }

    /// Writes `files` into a scratch dir unique to this process; the caller
    /// removes the dir when done (kept out of the repo tree so census runs
    /// stay clean).
    fn staged_scratch(files: &[(&str, &str)]) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "rust-sass-libsass-unit-{}-{}",
            std::process::id(),
            files.len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, contents) in files {
            std::fs::write(dir.join(name), contents).unwrap();
        }
        dir
    }

    #[test]
    fn staged_data_roundtrip() {
        unsafe {
            let ctx = make_data_ctx("a { b: c; }");
            let base = sass_data_context_get_context(ctx);
            let compiler = sass_make_data_compiler(ctx);
            assert!(!compiler.is_null());
            assert_eq!(sass_compiler_get_state(compiler), SASS_COMPILER_CREATED);
            assert_eq!(sass_compiler_get_context(compiler), base);
            assert_eq!(
                sass_compiler_get_options(compiler),
                sass_data_context_get_options(ctx)
            );
            assert_eq!(sass_compiler_parse(compiler), 0);
            assert_eq!(sass_compiler_get_state(compiler), SASS_COMPILER_PARSED);
            // No output until execute.
            assert!(sass_context_get_output_string(base).is_null());
            assert_eq!(sass_compiler_execute(compiler), 0);
            assert_eq!(sass_compiler_get_state(compiler), SASS_COMPILER_EXECUTED);
            let css =
                String::from_utf8(read_opt(sass_context_get_output_string(base)).unwrap()).unwrap();
            assert!(css.contains("b: c"), "got: {css}");
            sass_delete_compiler(compiler);
            sass_delete_data_context(ctx);
        }
    }

    #[test]
    fn staged_state_vectors() {
        unsafe {
            let ctx = make_data_ctx("a { b: c; }");
            let compiler = sass_make_data_compiler(ctx);
            // Execute before parse is refused without advancing.
            assert_eq!(sass_compiler_execute(compiler), -1);
            assert_eq!(sass_compiler_get_state(compiler), SASS_COMPILER_CREATED);
            // Both phases are idempotent.
            assert_eq!(sass_compiler_parse(compiler), 0);
            assert_eq!(sass_compiler_parse(compiler), 0);
            assert_eq!(sass_compiler_execute(compiler), 0);
            assert_eq!(sass_compiler_execute(compiler), 0);
            // Parse after execute is refused.
            assert_eq!(sass_compiler_parse(compiler), -1);
            sass_delete_compiler(compiler);
            sass_delete_data_context(ctx);

            // NULL makers return NULL (upstream returns 0).
            assert!(sass_make_data_compiler(ptr::null_mut()).is_null());
            assert!(sass_make_file_compiler(ptr::null_mut()).is_null());
            // NULL parse/execute fail; NULL delete/getters/takes harden
            // (upstream would crash on all of these).
            assert_eq!(sass_compiler_parse(ptr::null_mut()), 1);
            assert_eq!(sass_compiler_execute(ptr::null_mut()), 1);
            sass_delete_compiler(ptr::null_mut());
            assert_eq!(
                sass_compiler_get_state(ptr::null_mut()),
                SASS_COMPILER_CREATED
            );
            assert!(sass_compiler_get_context(ptr::null_mut()).is_null());
            assert!(sass_compiler_get_options(ptr::null_mut()).is_null());
            assert!(sass_context_take_output_string(ptr::null_mut()).is_null());
            assert!(sass_context_take_included_files(ptr::null_mut()).is_null());
        }
    }

    #[test]
    fn staged_maker_error_early_return() {
        unsafe {
            // Maker-error contexts (NULL source) early-return the stored
            // status from parse without advancing; execute is then refused.
            let ctx = sass_make_data_context(ptr::null_mut());
            assert!(!ctx.is_null());
            let compiler = sass_make_data_compiler(ctx);
            assert!(!compiler.is_null());
            assert_eq!(sass_compiler_parse(compiler), 1);
            assert_eq!(sass_compiler_get_state(compiler), SASS_COMPILER_CREATED);
            assert_eq!(sass_compiler_execute(compiler), -1);
            sass_delete_compiler(compiler);
            sass_delete_data_context(ctx);
        }
    }

    #[test]
    fn staged_parse_error_returns_zero() {
        unsafe {
            // Syntax errors store on the context while parse still returns 0;
            // execute reports the stored error without recompiling.
            let ctx = make_data_ctx("}");
            let base = sass_data_context_get_context(ctx);
            let compiler = sass_make_data_compiler(ctx);
            assert_eq!(sass_compiler_parse(compiler), 0);
            assert_eq!(sass_compiler_get_state(compiler), SASS_COMPILER_PARSED);
            assert_ne!(sass_context_get_error_status(base), 0);
            assert!(sass_context_get_output_string(base).is_null());
            assert_ne!(sass_compiler_execute(compiler), 0);
            assert_eq!(sass_compiler_get_state(compiler), SASS_COMPILER_PARSED);
            sass_delete_compiler(compiler);
            sass_delete_data_context(ctx);
        }
    }

    #[test]
    fn staged_file_lists_files_after_parse() {
        unsafe {
            let dir = staged_scratch(&[
                ("main.scss", "@import \"partial\";\n"),
                ("_partial.scss", "x { y: z; }\n"),
            ]);
            let path = CString::new(dir.join("main.scss").to_str().unwrap().to_owned()).unwrap();
            let ctx = sass_make_file_context(path.as_ptr());
            let base = sass_file_context_get_context(ctx);
            let compiler = sass_make_file_compiler(ctx);
            assert_eq!(sass_compiler_parse(compiler), 0);
            // Files queryable before execute; output still NULL.
            let files =
                read_staged_files(sass_context_get_included_files(base) as *mut *mut c_char);
            assert_eq!(files.len(), 2, "got: {files:?}");
            assert!(
                files.iter().any(|f| f.ends_with("main.scss")),
                "got: {files:?}"
            );
            assert!(
                files.iter().any(|f| f.ends_with("_partial.scss")),
                "got: {files:?}"
            );
            assert!(sass_context_get_output_string(base).is_null());
            assert_eq!(sass_compiler_execute(compiler), 0);
            let css =
                String::from_utf8(read_opt(sass_context_get_output_string(base)).unwrap()).unwrap();
            assert!(css.contains("y: z"), "got: {css}");
            sass_delete_compiler(compiler);
            sass_delete_file_context(ctx);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn included_files_size_counts_entries() {
        // Regression: the size getter dereferenced the NULL terminator
        // element itself (`**cur`), aborting on any non-empty array. It
        // must test the element like upstream's `while (i && *i)`.
        let mut common = ContextCommon {
            options: OptionsBox::new(),
            result: ContextResult::new(),
        };
        let urls = ["file:///a.scss", "file:///b.scss"].map(|s| SassUrl::parse(s).unwrap());
        store_files(&mut common.result, &urls, false);
        assert_eq!(
            unsafe { sass_context_get_included_files_size(&mut common as *mut ContextCommon) },
            2
        );
    }

    #[test]
    fn take_transfer_and_delete_safe() {
        unsafe {
            let ctx = make_data_ctx("a { b: c; }");
            assert_eq!(sass_compile_data_context(ctx), 0);
            let base = sass_data_context_get_context(ctx);
            // Take transfers ownership: content matches, the slot reads NULL
            // after, a second take is NULL, and deleting the context
            // afterwards is safe (a double free would abort the binary).
            let taken = sass_context_take_output_string(base);
            assert!(!taken.is_null());
            let text =
                String::from_utf8(CStr::from_ptr(taken as *const c_char).to_bytes().to_vec())
                    .unwrap();
            assert!(text.contains("b: c"), "got: {text}");
            sass_free_memory(taken as *mut std::ffi::c_void);
            assert!(sass_context_get_output_string(base).is_null());
            assert!(sass_context_take_output_string(base).is_null());
            sass_delete_data_context(ctx);

            // Error takes behave the same way.
            let ectx = make_data_ctx("}");
            assert_ne!(sass_compile_data_context(ectx), 0);
            let ebase = sass_data_context_get_context(ectx);
            let msg = sass_context_take_error_message(ebase);
            assert!(!msg.is_null());
            sass_free_memory(msg as *mut std::ffi::c_void);
            assert!(sass_context_get_error_message(ebase).is_null());
            sass_delete_data_context(ectx);
        }
    }
}
