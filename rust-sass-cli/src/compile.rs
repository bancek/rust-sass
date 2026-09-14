// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/executable/compile_stylesheet.dart + concurrent.dart

//! The compile runner: mirrors Dart's `compileStylesheet` and `compileStylesheets`
//! (single-file and directory/multi-file modes).

use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use bumpalo::Bump;

use rust_sass::common::exception::SassError;
use rust_sass::common::source_span_highlighter::HighlightOptions;
use rust_sass::compile::{compile, compile_string, CompileOptions, CompileResult};
use rust_sass::eval::importer::{FilesystemImporter, Importer, ImporterKind};
use rust_sass::io::IoExt;
use rust_sass::logger::{Logger, StderrLogger};
use rust_sass::parse::stylesheet::{CssState, SassIndentState, Syntax};
use rust_sass::termglyph::GlyphSet;

use crate::options::CliOptions;

/// An error returned by the compile runner, carrying the process exit code.
#[derive(Debug)]
pub struct CompileError {
    pub exit_code: i32,
    pub message: String,
}

impl CompileError {
    fn new(exit_code: i32, message: String) -> Self {
        CompileError { exit_code, message }
    }
}

/// Returns whether [err] is a Sass exception.
///
/// Mirrors Dart's `on SassException`: only the four spanned variants. The
/// unspanned `Script`/`MultiSpanScript` family (I/O failures, argument
/// validation) exits 66, and `emit_error_css` never renders it as CSS.
fn is_sass_exception(err: &SassError) -> bool {
    matches!(
        err,
        SassError::Sass { .. }
            | SassError::Runtime { .. }
            | SassError::Format { .. }
            | SassError::MultiSpan { .. }
    )
}

/// The highlight options used for error rendering: matches `--unicode`/`--color`.
fn highlight_options(unicode: bool, color: bool) -> HighlightOptions {
    HighlightOptions {
        color: if color {
            rust_sass::common::source_span_highlighter::HighlightColor::Default
        } else {
            rust_sass::common::source_span_highlighter::HighlightColor::None
        },
        glyphs: if unicode {
            GlyphSet::default()
        } else {
            GlyphSet::Ascii
        },
        ..Default::default()
    }
}

/// Compiles every source in [opts]. Returns the max exit code (0 on success).
#[rust_sass_macros::maybe_async]
pub async fn compile_all(io: Rc<dyn IoExt>, arena: &Bump, opts: &CliOptions) -> i32 {
    let mut exit_code = 0;
    let sources = opts.sources().clone();
    for (source, dest) in &sources {
        let source = source.as_deref();
        let dest = dest.as_deref();
        match compile_one(io.clone(), arena, opts, source, dest).await {
            // Dart only prints a "Compiled ..." line in `--update`/`--watch`
            // mode (compile_stylesheet.dart:209); neither is implemented here,
            // so a plain file compile stays silent.
            Ok(()) => {}
            Err(e) => {
                if e.message.is_empty() {
                    // Errors were already printed inline; nothing more to do.
                } else {
                    io.print_error(&e.message);
                }
                if e.exit_code > exit_code {
                    exit_code = e.exit_code;
                }
                if opts.stop_on_error() {
                    break;
                }
            }
        }
    }
    exit_code
}

/// Compiles a single source (stdin or file) to a destination (stdout or file).
#[rust_sass_macros::maybe_async]
pub async fn compile_one(
    io: Rc<dyn IoExt>,
    arena: &Bump,
    opts: &CliOptions,
    source: Option<&str>,
    dest: Option<&str>,
) -> Result<(), CompileError> {
    let syntax = syntax_for(source, opts);

    let logger: Rc<dyn Logger> = match opts.quiet_logger() {
        Some(q) => q,
        None => Rc::new(StderrLogger::new(
            opts.alert_color(),
            opts.unicode(),
            io.clone(),
        )),
    };

    let mut c_opts: CompileOptions<'_, '_> = opts.compile_options(arena, Some(logger));
    c_opts.syntax = syntax;
    if source.is_none() {
        // Dart's CLI compiles stdin with `FilesystemImporter.cwd` so relative
        // imports resolve against the current directory.
        c_opts.importer = Importer::new(
            arena,
            ImporterKind::Filesystem(FilesystemImporter::new_cwd(io.clone())),
        );
    }

    let result = match source {
        None => {
            let stdin_bytes = io
                .read_stdin()
                .await
                .map_err(|e| CompileError::new(66, format!("Error reading stdin: {e}.")))?;
            let stdin_str = String::from_utf8_lossy(&stdin_bytes).into_owned();
            compile_string(&stdin_str, io.clone(), c_opts, arena).await
        }
        Some(source) => compile(source, io.clone(), c_opts, arena).await,
    };

    match result {
        Ok(compile_result) => {
            let mut css = compile_result.css().to_string();
            css.push_str(&write_source_map(io.as_ref(), opts, compile_result, dest).await?);
            if let Some(dest) = dest {
                ensure_dir(io.as_ref(), dest).await?;
                write_file(io.as_ref(), dest, format!("{css}\n").as_bytes()).await?;
            } else if !css.is_empty() {
                io.print_output(&format!("{css}\n"));
            }
            Ok(())
        }
        Err(err) => {
            if is_sass_exception(&err) {
                if opts.emit_error_css() {
                    let css = err.to_css_string(io.as_ref());
                    if let Some(dest) = dest {
                        ensure_dir(io.as_ref(), dest).await?;
                        write_file(io.as_ref(), dest, format!("{css}\n").as_bytes()).await?;
                    } else {
                        io.print_output(&format!("{css}\n"));
                    }
                } else if let Some(dest) = dest {
                    let _ = io.delete_file(Path::new(dest)).await;
                }
                let msg = err.to_error_string_with_options(
                    &highlight_options(opts.unicode(), opts.alert_color()),
                    io.as_ref(),
                );
                Err(CompileError::new(65, msg))
            } else {
                Err(CompileError::new(66, err.to_string()))
            }
        }
    }
}

/// Determines the syntax for a compilation, mirroring Dart's `compileStylesheet`.
fn syntax_for(source: Option<&str>, opts: &CliOptions) -> Syntax {
    if opts.indented() {
        Syntax::Sass(SassIndentState {
            current_indentation: 0,
            next_indentation: None,
            next_indentation_end: None,
            indent_spaces: None,
        })
    } else if let Some(source) = source {
        syntax_for_path(source)
    } else {
        Syntax::Scss
    }
}

/// Returns the syntax implied by [path]'s extension.
pub fn syntax_for_path(path: &str) -> Syntax {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("sass") => Syntax::Sass(SassIndentState {
            current_indentation: 0,
            next_indentation: None,
            next_indentation_end: None,
            indent_spaces: None,
        }),
        Some("css") => Syntax::Css(CssState {
            disallowed_function_names: HashSet::new(),
        }),
        _ => Syntax::Scss,
    }
}

#[rust_sass_macros::maybe_async]
async fn ensure_dir(io: &dyn IoExt, dest: &str) -> Result<(), CompileError> {
    if let Some(parent) = Path::new(dest).parent() {
        if !parent.as_os_str().is_empty() {
            io.ensure_dir(parent)
                .await
                .map_err(|e| CompileError::new(66, format!("Error creating directory: {}.", e)))?;
        }
    }
    Ok(())
}

#[rust_sass_macros::maybe_async]
async fn write_file(io: &dyn IoExt, dest: &str, contents: &[u8]) -> Result<(), CompileError> {
    io.write_file(Path::new(dest), contents)
        .await
        .map_err(|e| CompileError::new(66, format!("Error writing {dest}: {}.", e)))?;
    Ok(())
}

/// Writes the source map given by [result] to disk (if necessary) according to
/// [opts], returning the source map comment to append to the CSS.
/// Port of `_writeSourceMap` (compile_stylesheet.dart).
#[rust_sass_macros::maybe_async]
async fn write_source_map(
    io: &dyn IoExt,
    opts: &CliOptions,
    result: CompileResult<'_, '_>,
    dest: Option<&str>,
) -> Result<String, CompileError> {
    let source_map = match result.source_map() {
        Some(sm) => sm,
        None => return Ok(String::new()),
    };

    // Work on a clone so we can remap URLs / drop sourcesContent in place.
    let mut mapping = source_map.clone();
    if !opts.embed_sources() {
        mapping.sources_content.clear();
    }
    for url in &mut mapping.urls {
        *url = opts.source_map_url(url, dest);
    }

    let file = dest.map(|d| {
        Path::new(d)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| d.to_string())
    });
    let text = mapping
        .json_with_target(file)
        .map_err(|e| CompileError::new(66, format!("Error encoding source map: {}.", e)))?;
    let text = String::from_utf8_lossy(&text).into_owned();

    let url = if opts.embed_source_map() {
        format!("data:application/json;charset=utf-8,{text}")
    } else {
        // [dest] can't be null here because --embed-source-map is incompatible
        // with writing to stdout.
        let dest = dest.unwrap();
        let map_path = format!("{dest}.map");
        if let Some(parent) = Path::new(&map_path).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = io.ensure_dir(parent).await;
            }
        }
        let _ = io.write_file(Path::new(&map_path), text.as_bytes()).await;
        opts.source_map_url(&map_path, Some(dest))
            .trim_start_matches("file://")
            .to_string()
    };

    let escaped = url.replace("*/", "%2A/");
    Ok(format!(
        "{}{}",
        if opts.style() == rust_sass::serialize::OutputStyle::Compressed {
            ""
        } else {
            "\n\n"
        },
        format_args!("/*# sourceMappingURL={escaped} */")
    ))
}
