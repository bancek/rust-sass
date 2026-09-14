// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/compile.dart (compile/compileString parameters)
// go-source: go/compile/compile_options.go

use crate::eval::importer::ImporterKind;
use std::rc::Rc;

use bumpalo::Bump;

use crate::url::SassUrl;

use crate::callable::Callable;
use crate::deprecation::Deprecation;
use crate::eval::importer::node_package::NodePackageImporter;
use crate::eval::importer::Importer;
use crate::eval::importer::PackageConfig;
use crate::logger::Logger;
use crate::parse::stylesheet::Syntax;
use crate::serialize::LINE_FEED_LF;

pub use crate::serialize::{LineFeed, OutputStyle};

/// Settings for a single compilation.
///
/// Collects the keyword parameters of Dart's `compile`/`compileString` into
/// one struct (Dart threads them as named arguments; Rust has no equivalent,
/// so the entry points take `opts` by value). [`CompileOptions::new`] builds
/// the defaults; callers override individual fields.
/// Matches Go: compile.CompileOptions
#[derive(Clone)]
pub struct CompileOptions<'compile, 'parse> {
    /// Custom importers, evaluated before load paths (and before the
    /// `SASS_PATH` entries, which are appended after `load_paths`).
    pub importers: Vec<Importer<'parse>>,
    /// The entrypoint importer for the compiled source itself. Defaults to a
    /// `NoOp` placeholder, replaced with a `FilesystemImporter` with no load
    /// path at compile time.
    pub importer: Importer<'parse>,
    /// Directories searched for `@use`/`@import` targets, in order.
    pub load_paths: Vec<String>,
    /// Contents of `SASS_PATH`, or the `SASS_PATH` environment variable when
    /// empty. Searched after `load_paths`.
    pub sass_path: String,
    /// The canonical URL of the stylesheet being compiled. When set, it is
    /// used as the base for relative imports and as the source-map entry;
    /// a relative value triggers the `COMPILE_STRING_RELATIVE_URL`
    /// deprecation unless a node-package importer is present.
    pub url: Option<SassUrl>,
    /// Package resolution configuration for `pkg:` URLs.
    pub package_config: Option<Rc<dyn PackageConfig>>,
    /// When set, legacy Node.js-style resolution applies and the
    /// `LEGACY_JS_API` deprecation fires.
    pub node_package_importer: Option<&'parse NodePackageImporter>,
    /// Host-provided custom functions, registered before the built-ins so
    /// they take precedence on name conflicts.
    pub functions: Vec<Callable<'compile, 'parse>>,
    /// Logger for `@warn`/`@debug` output. Defaults to the standard logger;
    /// always wrapped in a `DeprecationProcessingLogger` at compile time.
    pub logger: Option<Rc<dyn Logger>>,
    /// Whether to avoid emitting warnings for files loaded from dependencies.
    pub quiet_deps: bool,
    /// Whether to track source-map information during serialization.
    pub source_map: bool,
    /// Whether to embed source file contents in the source map as
    /// `sourcesContent`.
    pub include_source_map_sources: bool,
    /// Whether to render evaluation failures as CSS (via
    /// `exception_to_css_string`) inside a synthetic `Ok` instead of
    /// propagating the error.
    pub emit_error_css: bool,
    /// The output style (expanded or compressed).
    pub style: OutputStyle,
    /// Whether to emit libsass-style `/* line N, path */` source comments
    /// before each style rule. No Dart counterpart (Dart removed source
    /// comments); seeded by the libsass C-API seam. Defaults to `false`.
    pub source_comments: bool,
    /// Whether indentation uses spaces (`true`) or tabs (`false`).
    /// Defaults to `true`.
    pub use_spaces: bool,
    /// The number of spaces (or tabs) per indentation level. `None` means
    /// the serializer default of 2, mirroring Dart's `indentWidth ??= 2`.
    pub indent_width: Option<u32>,
    /// The line-feed sequence emitted between lines. Defaults to LF.
    pub line_feed: LineFeed,
    /// Whether to emit `@charset "UTF-8";` (expanded) or a BOM (compressed)
    /// when the output contains non-ASCII characters. Defaults to `true`.
    pub charset: bool,
    /// Deprecations to silence entirely.
    pub silence_deprecations: Vec<&'static Deprecation>,
    /// Deprecations to raise as errors instead of warnings.
    pub fatal_deprecations: Vec<&'static Deprecation>,
    /// Deprecations to opt into early, before their `deprecatedIn` version.
    pub future_deprecations: Vec<&'static Deprecation>,
    /// When `false`, repetition limiting applies to deprecation warnings
    /// (Dart's `limitRepetition: !verbose`). Defaults to `false`.
    pub verbose: bool,
    /// Whether terminal output may use Unicode glyphs. Defaults to `true`.
    pub unicode: bool,
    /// Whether to colorize alert (warning/error) output.
    pub alert_color: bool,
    /// Whether to render alerts with ASCII fallbacks instead of Unicode.
    pub alert_ascii: bool,
    /// The syntax used to parse string input (`Scss` by default; re-inferred
    /// from the path extension by [`compile`](super::compile) unless
    /// explicitly overridden).
    pub syntax: Syntax,
}

impl<'compile: 'parse, 'parse> CompileOptions<'compile, 'parse> {
    /// Creates options with all defaults. The arena-backed `importer` defaults
    /// to a `NoOp` importer (replaced by the compile entry-point importer at
    /// compile time), matching `compile_string`'s behavior.
    pub fn new(arena: &'compile Bump) -> Self {
        CompileOptions {
            importers: Vec::new(),
            importer: Importer::new(arena, ImporterKind::NoOp),
            load_paths: Vec::new(),
            sass_path: String::new(),
            url: None,
            package_config: None,
            node_package_importer: None,
            functions: Vec::new(),
            logger: None,
            quiet_deps: false,
            source_map: false,
            include_source_map_sources: false,
            emit_error_css: false,
            style: OutputStyle::Expanded,
            source_comments: false,
            use_spaces: true,
            indent_width: None,
            line_feed: LINE_FEED_LF,
            charset: true,
            silence_deprecations: Vec::new(),
            fatal_deprecations: Vec::new(),
            future_deprecations: Vec::new(),
            verbose: false,
            unicode: true,
            alert_color: false,
            alert_ascii: false,
            syntax: Syntax::Scss,
        }
    }
}
