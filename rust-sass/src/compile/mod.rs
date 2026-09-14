// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/compile.dart + lib/src/executable/compile_stylesheet.dart
// go-source: go/compile/compile.go + go/compile/compile_stylesheet.go

//! The public compile API: parsing, evaluation, and serialization in one call.
//!
//! [`compile_string`] is the main entry point (Dart's `compileString` with the
//! node-sass-compatible options); [`compile`] adds file loading with syntax
//! inference from the path; [`compile_stylesheet`] is the executable layer
//! (Dart's `compileStylesheet`: stdin/stdout handling, exit codes 65/66).
//! All three funnel through [`compile_stylesheet_inner`], which evaluates the
//! parsed stylesheet and serializes the result, rewriting source-map URLs
//! against the import cache.

pub mod options;
pub mod result;

use crate::ast::css::stylesheet::CssStylesheet;
use crate::callable::Callable;
use crate::common::file_span::FileSpan;
use crate::common::file_span::SourceLocation;
use crate::common::file_span::BOGUS_SPAN;
use crate::common::source_span_file_source::FileSource;
use crate::common::source_span_highlighter::HighlightOptions;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::deprecation::Deprecation;
use crate::eval::evaluate;
use crate::eval::importer::NodePackageImporter;
use crate::eval::result::EvaluateResult;
use crate::eval::syntax::syntax_for_path;
use crate::serialize::LineFeed;
use crate::serialize::SerializeResult;
use crate::termglyph::GlyphSet;
use crate::url::data_url_from_string;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use crate::url::SassUrl;
use bumpalo::Bump;

use crate::ast::sass::statement::Stylesheet;
use crate::common::exception::{SassError, SassResult};
use crate::deprecation::{COMPILE_STRING_RELATIVE_URL, LEGACY_JS_API};
use crate::eval::import_cache::ImportCache;
use crate::eval::importer::{FilesystemImporter, Importer, ImporterKind};
use crate::io::{Io, IoExt};
use crate::logger::{new_default_logger, DeprecationProcessingLogger, Logger};
use crate::parse::stylesheet::Syntax;
use crate::serialize::{self, SerializeOptions};

pub use options::CompileOptions;
pub use options::OutputStyle;
pub use result::{CompileResult, StylesheetError};

/// Compiles a Sass source string and returns the CSS result.
///
/// Like Dart's `compileString`, but with extra options to support the
/// node-sass-compatible API and the executable: the import cache is built from
/// `opts`' importers, load paths, and `SASS_PATH` (evaluated in that order),
/// the logger is wrapped for deprecation processing, and a relative `url`
/// with no node-package importer warns `COMPILE_STRING_RELATIVE_URL`.
/// Matches Go: compile.CompileString
#[rust_sass_macros::maybe_async]
pub async fn compile_string<'compile, 'parse>(
    source: &str,
    io: Rc<dyn Io>,
    mut opts: CompileOptions<'compile, 'parse>,
    arena: &'compile Bump,
) -> SassResult<CompileResult<'compile, 'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let stylesheet = arena.alloc(match opts.syntax {
        Syntax::Sass(_) => Stylesheet::parse_sass(source, opts.url.as_ref(), false, arena)?,
        Syntax::Css(_) => {
            Stylesheet::parse_css(source, opts.url.as_ref(), false, &HashSet::new(), arena)?
        }
        Syntax::Scss => Stylesheet::parse_scss(source, opts.url.as_ref(), false, arena)?,
    });

    let sass_path = if opts.sass_path.is_empty() {
        std::env::var("SASS_PATH").unwrap_or_default()
    } else {
        std::mem::take(&mut opts.sass_path)
    };

    // Matches Dart (compile.dart:59-66): the provided logger (or the default) is
    // always wrapped in a `DeprecationProcessingLogger`, which applies
    // silence/fatal/future deprecation settings and repetition limiting.
    // (A1a: Dart validates the config BEFORE parsing, compile.dart:141 before
    // :143 — but `dpl.validate()` only WARNS, never throws, so the position
    // after parse is unobservable. No reorder needed. This comment documents
    // the resolution; the construction below is unchanged.)

    let import_cache = ImportCache::new_with_options(
        arena,
        std::mem::take(&mut opts.importers),
        std::mem::take(&mut opts.load_paths),
        &sass_path,
        false,
        io.clone(),
        opts.package_config.take(),
    );

    // Matches Dart (compile.dart:59-66): the provided logger (or the default) is
    // always wrapped in a `DeprecationProcessingLogger`, which applies
    // silence/fatal/future deprecation settings and repetition limiting.
    let inner_logger: Rc<dyn Logger> = if let Some(user_logger) = opts.logger.take() {
        user_logger
    } else {
        Rc::new(new_default_logger(opts.unicode, io.clone()))
    };
    let dpl: Rc<DeprecationProcessingLogger<Rc<dyn Logger>>> =
        Rc::new(DeprecationProcessingLogger::new(
            inner_logger,
            &opts.silence_deprecations,
            &opts.fatal_deprecations,
            &opts.future_deprecations,
            !opts.verbose,
        ));
    dpl.validate();
    let logger: Rc<dyn Logger> = dpl.clone();

    // COMPILE_STRING_RELATIVE_URL deprecation
    if let Some(ref url) = opts.url {
        if url.is_relative() && opts.node_package_importer.is_none() {
            logger.warn_deprecation(
                &format!(
                    "Passing a relative `url` argument ({url}) to compileString() or \
                     related functions is deprecated and will be an error in Dart Sass 2.0.0.\n"
                ),
                None,
                &COMPILE_STRING_RELATIVE_URL,
                None,
            )?;
        }
    }

    // Default importer to FilesystemImporter with no load path
    let importer = if matches!(opts.importer.kind(), ImporterKind::NoOp) {
        Importer::new(
            arena,
            ImporterKind::Filesystem(FilesystemImporter::new_no_load_path(io.clone())),
        )
    } else {
        opts.importer
    };

    let node_importer = opts.node_package_importer;
    let js = node_importer.is_some();
    let functions = std::mem::take(&mut opts.functions);

    let result = compile_stylesheet_inner(
        stylesheet,
        import_cache,
        node_importer,
        importer,
        functions,
        logger,
        opts.style,
        opts.source_comments,
        opts.use_spaces,
        opts.indent_width,
        opts.line_feed,
        opts.quiet_deps,
        opts.source_map,
        opts.include_source_map_sources,
        opts.charset,
        opts.emit_error_css,
        opts.unicode,
        opts.alert_color,
        opts.alert_ascii,
        arena,
        io.clone(),
    )
    .await;

    // Matches Dart (compile.dart:103,171): emit a summary of repetition-limited
    // deprecation warnings on both success and error paths. `js` matches Dart's
    // `js: nodeImporter != null` (embedded => false).
    dpl.summarize(js);

    result
}

/// Evaluates and serializes a parsed stylesheet.
///
/// Matches Dart's private `_compileStylesheet`: warns `LEGACY_JS_API` when a
/// node-package importer is set, evaluates, then serializes. Arguments are
/// handled as for [`compile_string`].
/// Matches Go: compile.compileStylesheet
// Arity mirrors Dart's `_compileStylesheet` options threading; a params
// struct would diverge from the pinned Dart/Go signatures.
#[allow(clippy::too_many_arguments)]
#[rust_sass_macros::maybe_async]
async fn compile_stylesheet_inner<'compile, 'parse>(
    stylesheet: &'parse Stylesheet<'parse>,
    import_cache: ImportCache<'compile, 'parse>,
    node_importer: Option<&'parse NodePackageImporter>,
    importer: Importer<'parse>,
    functions: Vec<Callable<'compile, 'parse>>,
    logger: Rc<dyn Logger>,
    style: OutputStyle,
    source_comments: bool,
    use_spaces: bool,
    indent_width: Option<u32>,
    line_feed: LineFeed,
    quiet_deps: bool,
    source_map: bool,
    include_source_map_sources: bool,
    charset: bool,
    emit_error_css: bool,
    unicode: bool,
    alert_color: bool,
    alert_ascii: bool,
    arena: &'compile Bump,
    io: Rc<dyn Io>,
) -> SassResult<CompileResult<'compile, 'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    if node_importer.is_some() {
        logger.warn_deprecation(
            "The legacy JS API is deprecated and will be removed in \
             Dart Sass 2.0.0.\n\n\
             More info: https://sass-lang.com/d/legacy-js-api",
            None,
            &LEGACY_JS_API,
            None,
        )?;
    }

    let eval_result = evaluate(
        stylesheet,
        Some(import_cache),
        node_importer,
        importer,
        functions,
        logger,
        quiet_deps,
        source_map,
        unicode,
        alert_color,
        alert_ascii,
        arena,
        io.clone(),
    )
    .await;

    match eval_result {
        Ok(eval_result) => {
            let ser_opts = SerializeOptions {
                style,
                source_comments,
                inspect: false,
                use_spaces,
                indent_width: indent_width.unwrap_or(2),
                line_feed,
                charset,
                source_map,
                include_source_map_sources,
            };

            let mut ser_result = serialize::serialize(&eval_result.stylesheet, &ser_opts)?;

            // Post-process source map URLs (Go compile.go:196-217).
            // Dart gates the rewrite on `resultSourceMap != null &&
            // importCache != null` (compile.dart:224): with no import cache
            // (A3: the evaluate boundary takes it back only on success),
            // non-empty URLs pass through untouched instead of resolving
            // against a missing cache.
            if let Some(ref mut sm) = ser_result.source_map {
                let sspan = stylesheet.span;
                for url_str in &mut sm.urls {
                    if url_str.is_empty() {
                        // Dart rewrites `''` to a `data:` URL even without an
                        // import cache (`Uri.dataFromString(...)` branch).
                        if let Some(file) = sspan.file() {
                            *url_str = data_url_from_string(file.text());
                        }
                    } else if let Ok(parsed) = SassUrl::parse(url_str) {
                        if let Some(import_cache) = &eval_result.import_cache {
                            *url_str = import_cache.source_map_url(&parsed).to_string();
                        }
                    }
                }
            }

            Ok(CompileResult::new(eval_result, ser_result))
        }
        Err(err) => {
            if emit_error_css && is_sass_exception(&err) {
                Ok(CompileResult::new(
                    EvaluateResult {
                        stylesheet: CssStylesheet::new(vec![], BOGUS_SPAN),
                        loaded_urls: vec![],
                        import_cache: None,
                    },
                    SerializeResult {
                        css: err.to_css_string(io.as_ref()),
                        source_map: None,
                    },
                ))
            } else {
                Err(err)
            }
        }
    }
}

/// Decodes raw source bytes as UTF-8, returning a borrowed `&str` on success.
///
/// On invalid input, returns the Dart-exact `Invalid UTF-8.` error whose span
/// points at the first invalid byte (`valid_up_to`) inside the lossy string,
/// with [url] as the source url — the same shape the path-based [`compile`]
/// produces. Shared by [`compile`] (which reads the file via `io`) and the wasm
/// `compile_bytes` entry (which receives raw bytes from JS).
pub fn decode_source_utf8<'a>(
    source_bytes: &'a [u8],
    url: Option<SassUrl>,
    arena: &'a Bump,
) -> Result<&'a str, Box<SassError>> {
    match std::str::from_utf8(source_bytes) {
        Ok(s) => Ok(s),
        Err(e) => {
            let lossy = String::from_utf8_lossy(source_bytes).into_owned();
            let valid_up_to = e.valid_up_to();
            let fs = FileSource::new_in(arena, &lossy, url);
            let span = FileSpan::new(Some(fs), valid_up_to, valid_up_to);
            let span_ctx = SourceSpanWithContext::from_file_span(&span).unwrap_or_else(|_| {
                SourceSpanWithContext::new(
                    SourceLocation {
                        offset: 0,
                        line: 0,
                        column: 0,
                    },
                    SourceLocation {
                        offset: 0,
                        line: 0,
                        column: 0,
                    },
                    String::new(),
                    String::new(),
                    None,
                )
                .unwrap_or_else(|_| unreachable!())
            });
            Err(Box::new(SassError::Format {
                message: "Invalid UTF-8.".into(),
                span: span_ctx,
                original_source: None,
                cause: None,
                loaded_urls: vec![],
            }))
        }
    }
}

/// Compiles a Sass file and returns the CSS result.
///
/// Like Dart's `compile`: reads the file, resolves its absolute `file:` URL,
/// and infers the syntax from the path extension when `opts` leaves the
/// default `Scss` (an explicit `Sass`/`Css` passes through verbatim), then
/// delegates to [`compile_string`]. A missing file surfaces as an unspanned
/// `Script` error so the executable layer exits 66, not 65.
/// Matches Go: compile.Compile
#[rust_sass_macros::maybe_async]
pub async fn compile<'compile, 'parse>(
    path: &str,
    io: Rc<dyn Io>,
    opts: CompileOptions<'compile, 'parse>,
    arena: &'compile Bump,
) -> SassResult<CompileResult<'compile, 'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let source_bytes = io
        .read_file(Path::new(path))
        .await
        .map_err(|e| SassError::Script {
            message: format!("Error reading {path}: {e}."),
            argument_name: None,
        })?;
    let abs_path =
        PathBuf::from(
            io.canonicalize(Path::new(path))
                .await
                .map_err(|e| SassError::Script {
                    message: format!("Error resolving {path}: {e}."),
                    argument_name: None,
                })?,
        );

    let file_url = SassUrl::file_url_from_abs_path(&abs_path.to_string_lossy()).map_err(|_| {
        SassError::Script {
            message: format!("Could not create URL from path: {path}"),
            argument_name: None,
        }
    })?;

    let source = decode_source_utf8(&source_bytes, Some(file_url.clone()), arena)?;

    let mut compile_opts = CompileOptions {
        url: Some(file_url),
        ..opts
    };

    // Dart honors an explicit `syntax` verbatim (compile.dart:71-72): only a
    // `None` (default `Scss` here) is re-inferred from the path. (A2b: an
    // explicit `Scss` for a `.sass` file was overridden to Sass.)
    // NOTE: the public `CompileOptions.syntax` cannot distinguish "default"
    // from "explicit Scss" — the CLI always sets it via `syntax_for`, so this
    // keeps Dart's behavior for the paths that can express explicitness
    // (explicit Sass/Css pass through; only default-Scss is inferred).
    if compile_opts.syntax == Syntax::Scss {
        compile_opts.syntax = syntax_for_path(&abs_path.to_string_lossy());
    }

    compile_string(source, io, compile_opts, arena).await
}

/// Builds a [`CompileOptions`] from individual settings and compiles a Sass
/// source string. Convenience shim over [`compile_string`] for callers that
/// don't keep an options struct.
/// Matches Go: compile.CompileStringToResult
#[allow(clippy::too_many_arguments)]
#[rust_sass_macros::maybe_async]
pub async fn compile_string_to_result<'compile, 'parse>(
    source: &str,
    io: Rc<dyn Io>,
    syntax: Syntax,
    url: Option<SassUrl>,
    importers: Vec<Importer<'parse>>,
    importer: Importer<'parse>,
    functions: Vec<Callable<'compile, 'parse>>,
    style: OutputStyle,
    quiet_deps: bool,
    verbose: bool,
    source_map: bool,
    charset: bool,
    fatal_deprecations: Vec<&'static Deprecation>,
    silence_deprecations: Vec<&'static Deprecation>,
    future_deprecations: Vec<&'static Deprecation>,
    alert_color: bool,
    alert_ascii: bool,
    arena: &'compile Bump,
) -> SassResult<CompileResult<'compile, 'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let opts = CompileOptions {
        importers,
        importer,
        functions,
        style,
        quiet_deps,
        verbose,
        source_map,
        charset,
        alert_color,
        alert_ascii,
        silence_deprecations,
        fatal_deprecations,
        future_deprecations,
        syntax,
        url,
        ..CompileOptions::new(arena)
    };

    compile_string(source, io, opts, arena).await
}

/// Compiles the stylesheet at `source` to `destination`.
///
/// Matches Dart's `compileStylesheet`: an empty `source` reads from stdin, an
/// empty `destination` writes to stdout (otherwise the parent directory is
/// created first). On failure returns [`StylesheetError`] with exit code 65
/// for Sass errors (66 for filesystem errors), deleting a partially-written
/// `destination` unless error CSS was requested.
/// Matches Go: compile.CompileStylesheet
#[rust_sass_macros::maybe_async]
pub async fn compile_stylesheet<'compile, 'parse>(
    io: Rc<dyn IoExt>,
    source: &str,
    destination: &str,
    opts: CompileOptions<'compile, 'parse>,
    arena: &'compile Bump,
) -> Result<(), StylesheetError>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let emit_error_css = opts.emit_error_css;
    // Captured before `opts` is moved into `compile_string`/`compile` below; used
    // to render the error with the matching glyph set (`unicode: false` -> ASCII,
    // matching Dart's `--no-unicode`).
    let unicode = opts.unicode;

    let result = if source.is_empty() {
        let stdin_bytes = io.read_stdin().await.map_err(|e| StylesheetError {
            exit_code: 66,
            message: format!("Error reading stdin: {}.", e),
        })?;
        let stdin_str = String::from_utf8_lossy(&stdin_bytes).into_owned();
        compile_string(&stdin_str, io.clone(), opts, arena).await
    } else {
        compile(source, io.clone(), opts, arena).await
    };

    match result {
        Ok(compile_result) => {
            let css = compile_result.css();
            if destination.is_empty() {
                if !css.is_empty() {
                    io.print_output(&format!("{css}\n"));
                }
            } else {
                let dest_path = Path::new(destination);
                if let Some(parent) = dest_path.parent() {
                    if !parent.as_os_str().is_empty() {
                        io.ensure_dir(parent).await.map_err(|e| StylesheetError {
                            exit_code: 66,
                            message: format!("Error creating directory: {}.", e),
                        })?;
                    }
                }
                let output = format!("{css}\n");
                io.write_file(dest_path, output.as_bytes())
                    .await
                    .map_err(|e| StylesheetError {
                        exit_code: 66,
                        message: format!("Error writing {destination}: {}.", e),
                    })?;
            }
            Ok(())
        }
        Err(err) => {
            if is_sass_exception(&err) {
                if !destination.is_empty() && !emit_error_css {
                    let _ = io.delete_file(Path::new(destination)).await;
                }
                Err(StylesheetError {
                    exit_code: 65,
                    message: err.to_error_string_with_options(
                        &HighlightOptions {
                            glyphs: if unicode {
                                GlyphSet::Unicode
                            } else {
                                GlyphSet::Ascii
                            },
                            ..Default::default()
                        },
                        io.as_ref(),
                    ),
                })
            } else {
                Err(StylesheetError {
                    exit_code: 66,
                    message: err.to_string(),
                })
            }
        }
    }
}

/// Reports whether err is a Sass exception.
///
/// Matches Dart's `on SassException` (`executable/compile_stylesheet.dart`):
/// only the four spanned variants. `Script`/`MultiSpanScript` are Dart's
/// `SassScriptException` family (unspanned internals: I/O failures, argument
/// validation) — they bypass the Sass gate so a missing input file exits 66,
/// and `emit_error_css` never swallows them.
fn is_sass_exception(err: &SassError) -> bool {
    matches!(
        err,
        SassError::Sass { .. }
            | SassError::Runtime { .. }
            | SassError::Format { .. }
            | SassError::MultiSpan { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::SourceLocation;
    use crate::common::source_span_span_with_context::SourceSpanWithContext;
    use crate::io::DefaultIo;
    use crate::io::VirtualIo;
    use crate::serialize::LineFeed;
    use std::collections::HashMap;

    fn test_io() -> Rc<dyn Io> {
        Rc::new(DefaultIo::new())
    }

    #[test]
    fn decode_source_utf8_accepts_valid_bytes() {
        let arena = Bump::new();
        let src = b"a { b: c; }";
        let out = decode_source_utf8(src, None, &arena).unwrap();
        assert_eq!(out, "a { b: c; }");
    }

    #[test]
    fn decode_source_utf8_rejects_invalid_bytes() {
        let arena = Bump::new();
        // "$:D&(" (5 valid bytes) then two invalid bytes.
        let bytes = b"$:D&(\xff\xff22#222222";
        let err = decode_source_utf8(bytes, None, &arena).unwrap_err();
        assert_eq!(err.message(), "Invalid UTF-8.");
        let span = err.span().unwrap();
        assert_eq!(
            span.start.offset, 5,
            "span must point at the first invalid byte"
        );
        assert_eq!(span.start.line, 0);
        assert_eq!(span.start.column, 5);
    }

    #[test]
    fn decode_source_utf8_invalid_uses_provided_url() {
        let arena = Bump::new();
        let url = SassUrl::parse("file:///input.scss").unwrap();
        let err = decode_source_utf8(b"\xff", Some(url), &arena).unwrap_err();
        let span = err.span().unwrap();
        assert_eq!(
            span.source_url.as_ref().map(|u| u.as_str()),
            Some("file:///input.scss")
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compile_string_basic() {
        let arena = Bump::new();
        let result = compile_string(
            "a { color: red; }",
            test_io(),
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert!(!result.css().is_empty());
        assert!(result.css().contains("a"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compile_string_empty() {
        let arena = Bump::new();
        let result = compile_string("", test_io(), CompileOptions::new(&arena), &arena)
            .await
            .unwrap();
        assert_eq!(result.css(), "");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compile_string_compressed() {
        let arena = Bump::new();
        let opts = CompileOptions {
            style: OutputStyle::Compressed,
            ..CompileOptions::new(&arena)
        };
        let result = compile_string("a { color: red; }", test_io(), opts, &arena)
            .await
            .unwrap();
        assert!(!result.css().contains('\n'));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compile_string_source_map() {
        let arena = Bump::new();
        let opts = CompileOptions {
            source_map: true,
            ..CompileOptions::new(&arena)
        };
        let result = compile_string("a { color: red; }", test_io(), opts, &arena)
            .await
            .unwrap();
        assert!(result.source_map().is_some());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compile_string_no_source_map() {
        let arena = Bump::new();
        let result = compile_string(
            "a { color: red; }",
            test_io(),
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert!(result.source_map().is_none());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compile_string_charset_expanded() {
        let arena = Bump::new();
        let result = compile_string(
            "a { content: \"café\"; }",
            test_io(),
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert!(result.css().starts_with("@charset \"UTF-8\";\n"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compile_string_charset_compressed() {
        let arena = Bump::new();
        let opts = CompileOptions {
            style: OutputStyle::Compressed,
            ..CompileOptions::new(&arena)
        };
        let result = compile_string("a { content: \"café\"; }", test_io(), opts, &arena)
            .await
            .unwrap();
        assert!(result.css().starts_with("\u{FEFF}"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compile_string_charset_disabled() {
        let arena = Bump::new();
        let opts = CompileOptions {
            charset: false,
            ..CompileOptions::new(&arena)
        };
        let result = compile_string("a { content: \"café\"; }", test_io(), opts, &arena)
            .await
            .unwrap();
        assert!(!result.css().starts_with("@charset"));
        assert!(!result.css().starts_with("\u{FEFF}"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compile_string_error_propagation() {
        let arena = Bump::new();
        let result = compile_string(
            "$x: 1px + red;",
            test_io(),
            CompileOptions::new(&arena),
            &arena,
        )
        .await;
        assert!(result.is_err());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compile_string_emit_error_css() {
        let arena = Bump::new();
        let opts = CompileOptions {
            emit_error_css: true,
            ..CompileOptions::new(&arena)
        };
        let result = compile_string("@error \"test\";", test_io(), opts, &arena)
            .await
            .unwrap();
        assert!(result.css().contains("Error:"));
    }

    #[test]
    fn test_is_sass_exception() {
        let span = SourceSpanWithContext::new(
            SourceLocation {
                offset: 0,
                line: 0,
                column: 0,
            },
            SourceLocation {
                offset: 1,
                line: 0,
                column: 1,
            },
            "x".into(),
            "x".into(),
            None,
        )
        .unwrap();

        assert!(is_sass_exception(&SassError::Sass {
            message: "test".into(),
            span: span.clone(),
            cause: None,
            loaded_urls: vec![],
        }));
        assert!(is_sass_exception(&SassError::Runtime {
            message: "test".into(),
            span: span.clone(),
            trace: Default::default(),
            cause: None,
            loaded_urls: vec![],
        }));
        assert!(is_sass_exception(&SassError::Format {
            message: "test".into(),
            span: span.clone(),
            original_source: None,
            cause: None,
            loaded_urls: vec![],
        }));
        assert!(is_sass_exception(&SassError::MultiSpan {
            message: "test".into(),
            span,
            primary_label: None,
            secondary: vec![],
            original_source: None,
            cause: None,
            loaded_urls: vec![],
            trace: Default::default(),
        }));

        // Dart's `SassScriptException` family never matches `on SassException`.
        assert!(!is_sass_exception(&SassError::Script {
            message: "test".into(),
            argument_name: None,
        }));
        assert!(!is_sass_exception(&SassError::MultiSpanScript {
            message: "test".into(),
            primary_label: None,
            secondary: vec![],
            cause: None,
            loaded_urls: vec![],
        }));
    }

    // Dart threads `useSpaces`/`indentWidth`/`lineFeed` from `compile` into
    // `serialize` (compile.dart:48-50,122-124): `indentWidth: 4` widens the
    // indent, tabs replace spaces, CRLF replaces LF.
    #[rust_sass_macros::maybe_test]
    async fn test_serialize_options() {
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());

        let mut opts = CompileOptions::new(&arena);
        opts.indent_width = Some(4);
        let result = compile_string("a { b: c; }", io.clone(), opts, &arena)
            .await
            .unwrap();
        assert_eq!(result.css(), "a {\n    b: c;\n}");

        let mut opts = CompileOptions::new(&arena);
        opts.use_spaces = false;
        let result = compile_string("a { b: c; }", io.clone(), opts, &arena)
            .await
            .unwrap();
        // Dart `_writeTimes(_indentCharacter, _indentation * _indentWidth)`:
        // tabs multiply by indentWidth too.
        assert_eq!(result.css(), "a {\n\t\tb: c;\n}");

        let mut opts = CompileOptions::new(&arena);
        opts.line_feed = LineFeed {
            name: "crlf",
            text: "\r\n",
        };
        let result = compile_string("a { b: c; }", io.clone(), opts, &arena)
            .await
            .unwrap();
        assert_eq!(result.css(), "a {\r\n  b: c;\r\n}");
    }

    // libsass compat (`source_comments`, no Dart counterpart — Dart removed
    // source comments): `/* line N, path */` before each style rule.
    #[rust_sass_macros::maybe_test]
    async fn test_source_comments() {
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());

        let mut opts = CompileOptions::new(&arena);
        opts.source_comments = true;
        let result = compile_string("a { b: c; }", io.clone(), opts, &arena)
            .await
            .unwrap();
        assert_eq!(result.css(), "/* line 1, stdin */\na {\n  b: c;\n}");

        // Off by default.
        let result = compile_string(
            "a { b: c; }",
            io.clone(),
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap();
        assert_eq!(result.css(), "a {\n  b: c;\n}");
    }

    // Dart exits 66 (`FileSystemException`) for a missing input file and 65
    // (`SassException`) for a Sass error: the unspanned `Script` error from
    // `compile()`'s `read_file` must bypass the Sass gate.
    #[rust_sass_macros::maybe_test]
    async fn test_exit_codes() {
        let arena = Bump::new();
        let io: Rc<dyn IoExt> = Rc::new(VirtualIo::new());
        let err = compile_stylesheet(
            io.clone(),
            "does-not-exist.scss",
            "",
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        assert_eq!(err.exit_code, 66);
        assert!(
            err.message.starts_with("Error reading"),
            "got: {}",
            err.message
        );

        let mut files = HashMap::new();
        files.insert("/bad.scss".to_string(), "a { b: $undefined; }".to_string());
        let io: Rc<dyn IoExt> = Rc::new(VirtualIo::with_files(files));
        let err = compile_stylesheet(
            io.clone(),
            "/bad.scss",
            "",
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        assert_eq!(err.exit_code, 65);
        assert!(
            err.message.contains("Undefined variable"),
            "got: {}",
            err.message
        );
    }
}
