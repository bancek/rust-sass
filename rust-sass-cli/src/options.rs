// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/executable/options.dart
// go-source: N/A (Go port has no CLI)

//! Command-line option parsing for the `rust-sass` executable.
//!
//! Port of Dart Sass's `ExecutableOptions`: clap handles flag tokenization, this
//! module mirrors the post-parse "resolver" semantics (positional classification
//! and validation, source-map gating, deprecation resolution).

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use bumpalo::Bump;

use clap::{Arg, ArgAction, ArgMatches, Command};

use rust_sass::compile::CompileOptions;
use rust_sass::deprecation::{self, Deprecation};
use rust_sass::eval::importer::node_package::NodePackageImporter;
use rust_sass::eval::importer::{Importer, ImporterKind};
use rust_sass::io::Io;
use rust_sass::logger::QuietLogger;
use rust_sass::serialize::OutputStyle;

/// The Sass version reported by `--version` and used for `--fatal-deprecation`
/// version comparisons. Matches the embedded compiler's COMPILER_VERSION.
pub const SASS_VERSION: &str = "1.104.0";

/// A usage error: mirrors Dart's `UsageException` (exit code 64).
#[derive(Debug)]
pub struct UsageError(pub String);

/// Builds the clap command describing all supported flags.
pub fn build_command() -> Command {
    let mut cmd = Command::new("sass")
        .version(SASS_VERSION)
        .disable_version_flag(false)
        .arg(
            Arg::new("inputs")
                .num_args(0..)
                .allow_hyphen_values(true)
                .help("input.scss [output.css] or input.scss:output.css"),
        );

    cmd = neg_flag(
        cmd,
        "stdin",
        "no_stdin",
        "stdin",
        "no-stdin",
        "Read the stylesheet from stdin.",
    );
    cmd = neg_flag(
        cmd,
        "indented",
        "no_indented",
        "indented",
        "no-indented",
        "Use the indented syntax for input from stdin.",
    );
    cmd = cmd
        .arg(
            Arg::new("load-path")
                .long("load-path")
                .short('I')
                .action(ArgAction::Append)
                .value_name("PATH")
                .help("A path to use when resolving imports. May be passed multiple times."),
        )
        .arg(
            Arg::new("pkg-importer")
                .long("pkg-importer")
                .short('p')
                .action(ArgAction::Append)
                .value_name("TYPE")
                .value_parser(["node"])
                .help("Built-in importer(s) to use for pkg: URLs."),
        )
        .arg(
            Arg::new("style")
                .long("style")
                .short('s')
                .value_name("NAME")
                .value_parser(["expanded", "compressed"])
                .default_value("expanded")
                .help("Output style."),
        );
    cmd = neg_flag(
        cmd,
        "charset",
        "no_charset",
        "charset",
        "no-charset",
        "Emit a @charset or BOM for CSS with non-ASCII characters.",
    )
    .override_default("charset", true);
    cmd = neg_flag(cmd, "error-css", "no_error_css", "error-css", "no-error-css", "When an error occurs, emit a stylesheet describing it. Defaults to true when compiling to a file.");

    cmd = neg_flag(
        cmd,
        "source-map",
        "no_source_map",
        "source-map",
        "no-source-map",
        "Whether to generate source maps.",
    )
    .override_default("source-map", true);
    cmd = cmd.arg(
        Arg::new("source-map-urls")
            .long("source-map-urls")
            .value_name("TYPE")
            .value_parser(["relative", "absolute"])
            .default_value("relative")
            .help("How to link from source maps to source files."),
    );
    cmd = neg_flag(
        cmd,
        "embed-sources",
        "no_embed_sources",
        "embed-sources",
        "no-embed-sources",
        "Embed source file contents in source maps.",
    );
    cmd = neg_flag(
        cmd,
        "embed-source-map",
        "no_embed_source_map",
        "embed-source-map",
        "no-embed-source-map",
        "Embed source map contents in CSS.",
    );

    cmd = neg_flag(
        cmd,
        "quiet",
        "no_quiet",
        "quiet",
        "no-quiet",
        "Don't print warnings.",
    )
    .short_for("quiet", 'q');
    cmd = neg_flag(cmd, "quiet-deps", "no_quiet_deps", "quiet-deps", "no-quiet-deps", "Don't print compiler warnings from dependencies. Stylesheets imported through load paths count as dependencies.");
    cmd = neg_flag(
        cmd,
        "verbose",
        "no_verbose",
        "verbose",
        "no-verbose",
        "Print all deprecation warnings even when they're repetitive.",
    );
    cmd = cmd
        .arg(Arg::new("fatal-deprecation").long("fatal-deprecation").action(ArgAction::Append).value_name("DEPRECATION").help("Deprecations to treat as errors. You may also pass a Sass version to include any behavior deprecated in or before it."))
        .arg(Arg::new("silence-deprecation").long("silence-deprecation").action(ArgAction::Append).value_name("DEPRECATION").help("Deprecations to ignore."))
        .arg(Arg::new("future-deprecation").long("future-deprecation").action(ArgAction::Append).value_name("DEPRECATION").help("Opt in to a deprecation early."));

    cmd = neg_flag(
        cmd,
        "stop-on-error",
        "no_stop_on_error",
        "stop-on-error",
        "no-stop-on-error",
        "Don't compile more files once an error is encountered.",
    );
    cmd = neg_flag(
        cmd,
        "trace",
        "no_trace",
        "trace",
        "no-trace",
        "Print full stack traces for exceptions.",
    );
    cmd = neg_flag(
        cmd,
        "color",
        "no_color",
        "color",
        "no-color",
        "Whether to use terminal colors for messages.",
    )
    .short_for("color", 'c');
    cmd = neg_flag(
        cmd,
        "unicode",
        "no_unicode",
        "unicode",
        "no-unicode",
        "Whether to use Unicode characters for messages.",
    )
    .override_default("unicode", true);

    cmd = cmd
        // Hidden no-ops, accepted for sass-spec compatibility.
        .arg(
            Arg::new("precision")
                .long("precision")
                .hide(true)
                .value_parser(clap::value_parser!(u64))
                .help(""),
        )
        .arg(
            Arg::new("async")
                .long("async")
                .hide(true)
                .action(ArgAction::SetTrue)
                .help(""),
        );

    cmd
}

/// A `negatable()`-style helper: adds `--<long>` plus a hidden `--no-<long>`
/// pair that overrides it (mirrors dart `args` package's negatable-by-default
/// flags; clap has no built-in `--no-` support).
fn neg_flag(
    mut cmd: Command,
    id: &'static str,
    no_id: &'static str,
    long: &'static str,
    no_long: &'static str,
    help: &'static str,
) -> Command {
    cmd = cmd.arg(
        Arg::new(id)
            .long(long)
            .action(ArgAction::SetTrue)
            .overrides_with(no_id)
            .help(help),
    );
    cmd.arg(
        Arg::new(no_id)
            .long(no_long)
            .action(ArgAction::SetTrue)
            .overrides_with(id)
            .hide(true)
            .help(""),
    )
}

trait CommandExt {
    fn override_default(self, id: &'static str, value: bool) -> Self;
    fn short_for(self, id: &'static str, short: char) -> Self;
}

impl CommandExt for Command {
    fn override_default(mut self, id: &'static str, value: bool) -> Self {
        self = self.mut_arg(id, |a| {
            a.default_value(if value { "true" } else { "false" })
        });
        self
    }
    fn short_for(self, id: &'static str, short: char) -> Self {
        self.mut_arg(id, |a| a.short(short))
    }
}

fn parsed(m: &ArgMatches, name: &str) -> bool {
    let no_name = format!("no_{}", name.replace('-', "_"));
    let positive = m
        .value_source(name)
        .is_some_and(|s| s == clap::parser::ValueSource::CommandLine);
    let negative = m
        .value_source(&no_name)
        .is_some_and(|s| s == clap::parser::ValueSource::CommandLine);
    positive || negative
}

fn bool_flag(m: &ArgMatches, name: &str) -> bool {
    // `--no-<name>` overrides `--<name>` (last one wins via overrides_with).
    let no_name = format!("no_{}", name.replace('-', "_"));
    if m.get_flag(&no_name) {
        return false;
    }
    m.get_flag(name)
}

/// Whether a non-negatable value option was explicitly passed.
fn parsed_value(m: &ArgMatches, name: &str) -> bool {
    m.value_source(name)
        .is_some_and(|s| s == clap::parser::ValueSource::CommandLine)
}

/// The set of sources to compile, keyed by source path (null = stdin) to
/// destination path (null = stdout).
pub type SourceMap = Vec<(Option<String>, Option<String>)>;

/// Parsed and validated CLI options. Mirrors `ExecutableOptions`.
pub struct CliOptions {
    matches: ArgMatches,
    sources: SourceMap,
    emit_source_map: bool,
    silent: bool,
    verbose: bool,
    quiet_deps: bool,
    style: OutputStyle,
    charset: bool,
    emit_error_css: bool,
    unicode: bool,
    alert_color: bool,
    #[allow(dead_code)]
    trace: bool,
    stop_on_error: bool,
    load_paths: Vec<String>,
    node_package_importer: Option<NodePackageImporter>,
    silence_deprecations: Vec<&'static Deprecation>,
    fatal_deprecations: Vec<&'static Deprecation>,
    future_deprecations: Vec<&'static Deprecation>,
}

fn get_strings(m: &ArgMatches, name: &str) -> Vec<String> {
    m.get_many::<String>(name)
        .map(|v| v.cloned().collect())
        .unwrap_or_default()
}

/// Returns whether [path] contains an absolute Windows path at [index].
fn is_windows_path(string: &str, index: usize) -> bool {
    string.len() > index + 2
        && string.as_bytes()[index].is_ascii_alphabetic()
        && string.as_bytes()[index + 1] == b':'
}

/// Splits an argument that contains a colon and returns its source and its
/// destination component. Throws a usage error for a second colon.
fn split_source_and_destination(argument: &str) -> Result<(String, String), UsageError> {
    for (i, b) in argument.bytes().enumerate() {
        // A colon at position 1 may be a Windows drive letter and not a
        // separator.
        if i == 1 && is_windows_path(argument, i - 1) {
            continue;
        }
        if b == b':' {
            let next_colon = argument[i + 1..].find(':').map(|j| i + 1 + j);
            // A colon 2 characters after the separator may also be a Windows
            // drive letter.
            if let Some(nc) = next_colon {
                if nc == i + 2 && is_windows_path(argument, i + 1) {
                    continue;
                }
            }
            if next_colon.is_some() {
                return Err(UsageError(format!(
                    "\"{argument}\" may only contain one \":\"."
                )));
            }
            return Ok((argument[..i].to_string(), argument[i + 1..].to_string()));
        }
    }
    Err(UsageError(format!(
        "Expected \"{argument}\" to contain a colon."
    )))
}

impl CliOptions {
    pub fn from_matches(io: Rc<dyn Io>, matches: &ArgMatches) -> Result<CliOptions, UsageError> {
        // Validate the hidden no-op `--precision` if provided (ignored).
        let _ = matches.get_one::<u64>("precision");

        let silent = bool_flag(matches, "quiet");
        let verbose = bool_flag(matches, "verbose");
        let quiet_deps = bool_flag(matches, "quiet-deps");
        let style = if matches.get_one::<String>("style").map(|s| s.as_str()) == Some("compressed")
        {
            OutputStyle::Compressed
        } else {
            OutputStyle::Expanded
        };
        let charset = bool_flag(matches, "charset");
        let unicode = bool_flag(matches, "unicode");
        // `--color` defaults to terminal ANSI detection when not explicitly set.
        let alert_color = if parsed(matches, "color") {
            bool_flag(matches, "color")
        } else {
            io.supports_ansi_escapes()
        };
        let trace = bool_flag(matches, "trace");
        let stop_on_error = bool_flag(matches, "stop-on-error");
        let load_paths = get_strings(matches, "load-path");
        let node_package_importer = if get_strings(matches, "pkg-importer").is_empty() {
            None
        } else {
            Some(NodePackageImporter::new(".", io.clone()))
        };

        let silence_deprecations = resolve_deprecations(matches, "silence-deprecation")?;
        let future_deprecations = resolve_deprecations(matches, "future-deprecation")?;
        let fatal_deprecations = resolve_fatal_deprecations(matches)?;

        let rest: Vec<String> = matches
            .get_many::<String>("inputs")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();

        let stdin = bool_flag(matches, "stdin");
        let sources = resolve_sources(&rest, stdin)?;
        let emit_source_map = resolve_emit_source_map(matches, &sources)?;

        let emit_error_css = if parsed(matches, "error-css") {
            bool_flag(matches, "error-css")
        } else {
            sources.iter().any(|(_, d)| d.is_some())
        };

        Ok(CliOptions {
            matches: matches.clone(),
            sources,
            emit_source_map,
            silent,
            verbose,
            quiet_deps,
            style,
            charset,
            emit_error_css,
            unicode,
            alert_color,
            trace,
            stop_on_error,
            load_paths,
            node_package_importer,
            silence_deprecations,
            fatal_deprecations,
            future_deprecations,
        })
    }

    pub fn sources(&self) -> &SourceMap {
        &self.sources
    }

    #[allow(dead_code)]
    pub fn emit_source_map(&self) -> bool {
        self.emit_source_map
    }

    pub fn indented(&self) -> bool {
        bool_flag(&self.matches, "indented")
    }

    pub fn embed_sources(&self) -> bool {
        bool_flag(&self.matches, "embed-sources")
    }

    pub fn embed_source_map(&self) -> bool {
        bool_flag(&self.matches, "embed-source-map")
    }

    #[allow(dead_code)]
    pub fn silent(&self) -> bool {
        self.silent
    }

    #[allow(dead_code)]
    pub fn verbose(&self) -> bool {
        self.verbose
    }

    #[allow(dead_code)]
    pub fn quiet_deps(&self) -> bool {
        self.quiet_deps
    }

    pub fn style(&self) -> OutputStyle {
        self.style
    }

    #[allow(dead_code)]
    pub fn charset(&self) -> bool {
        self.charset
    }

    pub fn emit_error_css(&self) -> bool {
        self.emit_error_css
    }

    pub fn unicode(&self) -> bool {
        self.unicode
    }

    pub fn alert_color(&self) -> bool {
        self.alert_color
    }

    #[allow(dead_code)]
    pub fn trace(&self) -> bool {
        self.trace
    }

    pub fn stop_on_error(&self) -> bool {
        self.stop_on_error
    }

    #[allow(dead_code)]
    pub fn load_paths(&self) -> &[String] {
        &self.load_paths
    }

    #[allow(dead_code)]
    pub fn node_package_importer(&self) -> Option<&NodePackageImporter> {
        self.node_package_importer.as_ref()
    }

    #[allow(dead_code)]
    pub fn silence_deprecations(&self) -> &[&'static Deprecation] {
        &self.silence_deprecations
    }

    #[allow(dead_code)]
    pub fn fatal_deprecations(&self) -> &[&'static Deprecation] {
        &self.fatal_deprecations
    }

    #[allow(dead_code)]
    pub fn future_deprecations(&self) -> &[&'static Deprecation] {
        &self.future_deprecations
    }

    /// Builds the `CompileOptions` used for each compilation (fields that differ
    /// per-compilation — source, destination, syntax, importer — are set by the
    /// runner).
    pub fn compile_options<'compile, 'parse>(
        &self,
        arena: &'compile Bump,
        logger: Option<Rc<dyn rust_sass::logger::Logger>>,
    ) -> CompileOptions<'compile, 'parse> {
        CompileOptions {
            // `--pkg-importer node` installs the NodePackageImporter as an
            // importer (matching the embedded compiler); the legacy
            // `opts.node_package_importer` field is unused (and would emit a
            // spurious legacy-js-api deprecation).
            importers: self
                .node_package_importer
                .as_ref()
                .map(|npi| vec![Importer::new(arena, ImporterKind::NodePackage(npi.clone()))])
                .unwrap_or_default(),
            load_paths: self.load_paths.clone(),
            logger,
            quiet_deps: self.quiet_deps,
            source_map: self.emit_source_map,
            // The CLI handles `--error-css` itself (write the error CSS then
            // exit 65), so the library's `emit_error_css` Ok-with-error-CSS
            // behavior (the JS-API contract) must stay off.
            emit_error_css: false,
            style: self.style,
            charset: self.charset,
            silence_deprecations: self.silence_deprecations.clone(),
            fatal_deprecations: self.fatal_deprecations.clone(),
            future_deprecations: self.future_deprecations.clone(),
            verbose: self.verbose,
            unicode: self.unicode,
            alert_color: self.alert_color,
            ..CompileOptions::new(arena)
        }
    }

    /// Returns the quiet logger when `--quiet`, else `None` (the runner uses a
    /// stderr logger).
    pub fn quiet_logger(&self) -> Option<Rc<dyn rust_sass::logger::Logger>> {
        if self.silent {
            Some(Rc::new(QuietLogger))
        } else {
            None
        }
    }

    /// Whether we're writing to stdout instead of a file or files.
    fn write_to_stdout(&self) -> bool {
        self.sources.len() == 1 && self.sources[0].1.is_none()
    }

    /// Makes [path] absolute or relative (to the directory containing
    /// [destination]) according to the `source-map-urls` option.
    /// Mirrors Dart's `sourceMapUrl` (options.dart:556-567).
    pub fn source_map_url(&self, path: &str, destination: Option<&str>) -> String {
        // Only `file:` URLs are remapped; everything else is passed through.
        let path = path.strip_prefix("file://").unwrap_or(path);
        let abs = std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path));
        if self
            .matches
            .get_one::<String>("source-map-urls")
            .map(|s| s.as_str())
            == Some("relative")
            && !self.write_to_stdout()
        {
            if let Some(dest) = destination {
                if let Some(dir) = std::path::absolute(dest)
                    .ok()
                    .and_then(|d| d.parent().map(|p| p.to_path_buf()))
                {
                    return relative_path(&dir, &abs);
                }
            }
            path.to_string()
        } else {
            abs.to_string_lossy().into_owned()
        }
    }
}

fn relative_path(from: &Path, to: &Path) -> String {
    // Mirror Dart's `p.relative`: strip the common prefix, then `..` up the
    // remaining `from` components and append the remaining `to` components.
    let from = from.to_string_lossy();
    let to = to.to_string_lossy();
    let mut from_parts: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    let mut to_parts: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    while !from_parts.is_empty() && from_parts.first() == to_parts.first() {
        from_parts.remove(0);
        to_parts.remove(0);
    }
    if to_parts.is_empty() {
        return ".".to_string();
    }
    let mut out: Vec<String> = from_parts.iter().map(|_| "..".to_string()).collect();
    out.extend(to_parts.iter().map(|s| s.to_string()));
    out.join("/")
}

fn resolve_deprecations(
    matches: &ArgMatches,
    name: &str,
) -> Result<Vec<&'static Deprecation>, UsageError> {
    let mut out = Vec::new();
    for id in get_strings(matches, name) {
        match deprecation::from_id(&id) {
            Some(d) => out.push(d),
            None => {
                return Err(UsageError(format!("Invalid deprecation \"{id}\".")));
            }
        }
    }
    Ok(out)
}

fn resolve_fatal_deprecations(
    matches: &ArgMatches,
) -> Result<Vec<&'static Deprecation>, UsageError> {
    let mut out: HashSet<&'static Deprecation> = HashSet::new();
    for id in get_strings(matches, "fatal-deprecation") {
        if let Some(d) = deprecation::from_id(&id) {
            out.insert(d);
            continue;
        }
        // Try parsing as a version.
        if parse_version(&id).is_err() {
            return Err(UsageError(format!("Invalid deprecation \"{id}\".")));
        }
        let current = parse_version(SASS_VERSION).unwrap();
        if parse_version(&id).unwrap() > current {
            return Err(UsageError(format!(
                "Invalid version {id}. --fatal-deprecation requires a version less than or equal to the current Dart Sass version."
            )));
        }
        out.extend(deprecation::for_version(&id));
    }
    Ok(out.into_iter().collect())
}

/// Minimal semver comparison (major.minor.patch). Returns (major, minor, patch).
fn parse_version(v: &str) -> Result<(u64, u64, u64), ()> {
    let mut parts = v.trim().split('.');
    let major = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    let minor = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    let patch = parts.next().unwrap_or("0").parse().map_err(|_| ())?;
    Ok((major, minor, patch))
}

/// Resolves the positional arguments into a map of source → destination,
/// mirroring `ExecutableOptions._ensureSources`.
fn resolve_sources(rest: &[String], stdin: bool) -> Result<SourceMap, UsageError> {
    if rest.is_empty() && !stdin {
        return Err(UsageError("Compile Sass to CSS.".into()));
    }

    let mut directories: HashSet<String> = HashSet::new();
    let mut colon_args = false;
    let mut positional_args = false;
    for argument in rest {
        if argument.is_empty() {
            return Err(UsageError("Invalid argument \"\".".to_string()));
        }
        if contains_colon(argument) {
            colon_args = true;
        } else if is_dir(argument) {
            directories.insert(argument.clone());
        } else {
            positional_args = true;
        }
    }

    if positional_args || rest.is_empty() {
        if colon_args {
            return Err(UsageError(
                "Positional and \":\" arguments may not both be used.".into(),
            ));
        }
        if stdin {
            if rest.len() > 1 {
                return Err(UsageError(
                    "Only one argument is allowed with --stdin.".into(),
                ));
            }
            return Ok(vec![(None, rest.first().cloned())]);
        }
        if rest.len() > 2 {
            return Err(UsageError("Only two positional args may be passed.".into()));
        }
        if !directories.is_empty() {
            let first = directories.iter().next().unwrap().clone();
            let mut message = format!("Directory \"{first}\" may not be a positional arg.");
            let target = rest.last().cloned().unwrap_or_default();
            if rest.first().map(|s| s == &first).unwrap_or(false) && !is_file(&target) {
                message.push_str(&format!(
                    "\nTo compile all CSS in \"{first}\" to \"{target}\", use `sass {first}:{target}`."
                ));
            }
            return Err(UsageError(message));
        }
        let source = if rest.first().map(|s| s == "-").unwrap_or(false) {
            None
        } else {
            rest.first().cloned()
        };
        let destination = if rest.len() == 1 {
            None
        } else {
            rest.get(1).cloned()
        };
        return Ok(vec![(source, destination)]);
    }

    if stdin {
        return Err(UsageError(
            "--stdin may not be used with \":\" arguments.".into(),
        ));
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut sources: SourceMap = Vec::new();
    for argument in rest {
        if directories.contains(argument) {
            if !seen.insert(argument.clone()) {
                return Err(UsageError(format!("Duplicate source \"{argument}\".")));
            }
            let mut listed = list_source_directory(argument, argument)?;
            for (s, d) in listed.drain(..) {
                if !seen.contains(&s) {
                    seen.insert(s.clone());
                    sources.push((Some(s), Some(d)));
                }
            }
            continue;
        }

        let (source, destination) = split_source_and_destination(argument)?;
        if !seen.insert(source.clone()) {
            return Err(UsageError(format!("Duplicate source \"{source}\".")));
        }
        if source == "-" {
            sources.push((None, Some(destination)));
        } else if is_dir(&source) {
            let mut listed = list_source_directory(&source, &destination)?;
            for (s, d) in listed.drain(..) {
                if !seen.contains(&s) {
                    seen.insert(s.clone());
                    sources.push((Some(s), Some(d)));
                }
            }
        } else {
            sources.push((Some(source), Some(destination)));
        }
    }
    Ok(sources)
}

fn contains_colon(argument: &str) -> bool {
    // Mirrors Dart `_ensureSources`: a Windows drive letter (single letter +
    // colon at index 0) doesn't count unless there's another colon later.
    argument.contains(':') && (!is_windows_path(argument, 0) || argument[2..].contains(':'))
}

fn is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

fn is_file(path: &str) -> bool {
    Path::new(path).is_file()
}

/// Lists the entrypoint sources in [source], mapped to destinations under
/// [destination]. Mirrors `_listSourceDirectory`.
fn list_source_directory(
    source: &str,
    destination: &str,
) -> Result<Vec<(String, String)>, UsageError> {
    let mut out = Vec::new();
    let source_dir = Path::new(source);
    let mut entries = Vec::new();
    collect_files(source_dir, &mut entries);
    for path in entries {
        let path_str = path.to_string_lossy().into_owned();
        if !is_entrypoint(&path_str) {
            continue;
        }
        // Don't compile a CSS file to its own location.
        if source == destination
            && Path::new(&path_str)
                .extension()
                .map(|e| e == "css")
                .unwrap_or(false)
        {
            continue;
        }
        let rel = path.strip_prefix(source_dir).unwrap_or(&path);
        // `p.setExtension(rel, '.css')`: replace the source extension with .css.
        let mut dest_rel = rel.to_string_lossy().into_owned();
        if let Some(dot) = dest_rel.rfind('.') {
            dest_rel.truncate(dot);
        }
        dest_rel.push_str(".css");
        let dest = Path::new(destination)
            .join(dest_rel)
            .to_string_lossy()
            .into_owned();
        out.push((path_str, dest));
    }
    Ok(out)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                collect_files(&p, out);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
}

/// Returns whether [path] is a Sass entrypoint (that is, not a partial).
fn is_entrypoint(path: &str) -> bool {
    if Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().starts_with('_'))
        .unwrap_or(false)
    {
        return false;
    }
    matches!(
        Path::new(path).extension().and_then(|e| e.to_str()),
        Some("scss") | Some("sass") | Some("css")
    )
}

/// Resolves the `emitSourceMap` decision, mirroring `ExecutableOptions.emitSourceMap`.
fn resolve_emit_source_map(matches: &ArgMatches, sources: &SourceMap) -> Result<bool, UsageError> {
    let source_map = bool_flag(matches, "source-map");
    let source_map_urls = matches
        .get_one::<String>("source-map-urls")
        .map(|s| s.as_str());
    let embed_source_map = bool_flag(matches, "embed-source-map");

    if !source_map {
        if parsed_value(matches, "source-map-urls") {
            return Err(UsageError(
                "--source-map-urls isn't allowed with --no-source-map.".into(),
            ));
        }
        if parsed(matches, "embed-sources") {
            return Err(UsageError(
                "--embed-sources isn't allowed with --no-source-map.".into(),
            ));
        }
        if parsed(matches, "embed-source-map") {
            return Err(UsageError(
                "--embed-source-map isn't allowed with --no-source-map.".into(),
            ));
        }
    }

    let write_to_stdout = sources.len() == 1 && sources[0].1.is_none();
    if !write_to_stdout {
        return Ok(source_map);
    }

    if parsed_value(matches, "source-map-urls") && source_map_urls == Some("relative") {
        return Err(UsageError(
            "--source-map-urls=relative isn't allowed when printing to stdout.".into(),
        ));
    }
    if embed_source_map {
        return Ok(source_map);
    }
    if parsed(matches, "source-map") && source_map {
        return Err(UsageError(
            "When printing to stdout, --source-map requires --embed-source-map.".into(),
        ));
    }
    if parsed_value(matches, "source-map-urls") {
        return Err(UsageError(
            "When printing to stdout, --source-map-urls requires --embed-source-map.".into(),
        ));
    }
    if parsed(matches, "embed-sources") {
        return Err(UsageError(
            "When printing to stdout, --embed-sources requires --embed-source-map.".into(),
        ));
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rust_sass::io::DefaultIo;

    fn parse(args: &[&str]) -> Result<CliOptions, UsageError> {
        let io: Rc<dyn Io> = Rc::new(DefaultIo::new());
        let mut args = args.to_vec();
        args.insert(0, "sass");
        let cmd = build_command();
        let matches = cmd.clone().get_matches_from(args.iter());
        CliOptions::from_matches(io, &matches)
    }

    /// Returns the usage-error message for [args].
    fn parse_err(args: &[&str]) -> String {
        parse(args)
            .map(|_| String::new())
            .err()
            .map(|e| e.0)
            .unwrap_or_default()
    }

    #[test]
    fn no_args_is_usage_error() {
        assert_eq!(parse_err(&[]), "Compile Sass to CSS.");
    }

    #[test]
    fn single_file_to_stdout() {
        let o = parse(&["input.scss"]).unwrap();
        assert_eq!(o.sources(), &[(Some("input.scss".into()), None)]);
        assert!(!o.emit_source_map()); // stdout without --embed-source-map
    }

    #[test]
    fn two_positionals() {
        let o = parse(&["in.scss", "out.css"]).unwrap();
        assert_eq!(
            o.sources(),
            &[(Some("in.scss".into()), Some("out.css".into()))]
        );
    }

    #[test]
    fn stdin_flag_with_output() {
        let o = parse(&["--stdin", "out.css"]).unwrap();
        assert_eq!(o.sources(), &[(None, Some("out.css".into()))]);
    }

    #[test]
    fn dash_is_stdin() {
        let o = parse(&["-", "out.css"]).unwrap();
        assert_eq!(o.sources(), &[(None, Some("out.css".into()))]);
    }

    #[test]
    fn colon_form() {
        let o = parse(&["in.scss:out.css"]).unwrap();
        assert_eq!(
            o.sources(),
            &[(Some("in.scss".into()), Some("out.css".into()))]
        );
    }

    #[test]
    fn too_many_positionals() {
        assert_eq!(
            parse_err(&["a", "b", "c"]),
            "Only two positional args may be passed."
        );
    }

    #[test]
    fn mixing_positional_and_colon() {
        assert_eq!(
            parse_err(&["a", "bc:d"]),
            "Positional and \":\" arguments may not both be used."
        );
    }

    #[test]
    fn double_colon() {
        assert_eq!(
            parse_err(&["in.scss:out:css"]),
            "\"in.scss:out:css\" may only contain one \":\"."
        );
    }

    #[test]
    fn windows_drive_letter_colon_ok() {
        // On non-Windows this is just a weird relative path; the resolver only
        // treats a colon at index 1 followed by an alphabetic drive letter as
        // non-separator.
        let o = parse(&["C:foo.scss"]).unwrap();
        assert_eq!(o.sources(), &[(Some("C:foo.scss".into()), None)]);
    }

    #[test]
    fn stdin_too_many_args() {
        assert_eq!(
            parse_err(&["--stdin", "a", "b"]),
            "Only one argument is allowed with --stdin."
        );
    }

    #[test]
    fn stdin_with_colon_arg() {
        assert_eq!(
            parse_err(&["--stdin", "ab:c"]),
            "--stdin may not be used with \":\" arguments."
        );
    }

    #[test]
    fn no_source_map_blocks_map_flags() {
        assert_eq!(
            parse_err(&["--no-source-map", "--embed-sources", "a.scss"]),
            "--embed-sources isn't allowed with --no-source-map."
        );
    }

    #[test]
    fn stdout_requires_embed_source_map() {
        assert_eq!(
            parse_err(&["--source-map", "a.scss"]),
            "When printing to stdout, --source-map requires --embed-source-map."
        );
    }

    #[test]
    fn stdout_relative_urls_blocked() {
        assert_eq!(
            parse_err(&["--source-map-urls=relative", "a.scss"]),
            "--source-map-urls=relative isn't allowed when printing to stdout."
        );
    }

    #[test]
    fn invalid_deprecation() {
        assert_eq!(
            parse_err(&["--silence-deprecation=nope", "a.scss"]),
            "Invalid deprecation \"nope\"."
        );
    }

    #[test]
    fn style_and_charset() {
        let o = parse(&["--style", "compressed", "--no-charset", "a.scss"]).unwrap();
        assert_eq!(o.style(), OutputStyle::Compressed);
        assert!(!o.charset());
    }

    #[test]
    fn split_colon_respects_windows_drive() {
        assert_eq!(
            split_source_and_destination("C:\\foo.scss:out.css").unwrap(),
            ("C:\\foo.scss".to_string(), "out.css".to_string())
        );
    }

    #[test]
    fn precision_and_async_are_accepted() {
        let o = parse(&["--precision", "10", "--async", "a.scss"]).unwrap();
        assert_eq!(o.sources().len(), 1);
    }
}
