// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/import_cache.dart
// go-source: go/eval/import_cache.go

use crate::common::time::SassTime;
use crate::parse::stylesheet::Syntax;
use crate::url::resolve_file_path;
use crate::url::SassUrl;
use std::collections::hash_map;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use bumpalo::Bump;

use crate::ast::sass::statement::stylesheet::Stylesheet;
use crate::common::exception::{SassError, SassResult};
use crate::eval::importer::{
    CanonicalizeContext, FilesystemImporter, Importer, ImporterKind, ImporterResult, PackageConfig,
    PackageImporter,
};
use crate::functions::disallowed::disallowed_function_names;
use crate::io::Io;
use crate::logger::WarnLogger;

/// A canonicalized URL and the importer that canonicalized it, plus the URL
/// originally passed to the importer (which may have been resolved relative
/// to a base URL).
///
/// Rewritten from Dart's `CanonicalizeResult` record (`import_cache.dart`).
#[derive(Clone, Debug)]
pub struct CanonicalizeResult<'parse> {
    /// Importer that canonicalized the URL.
    pub importer: Importer<'parse>,
    /// Canonical URL returned by the importer.
    pub canonical_url: SassUrl,
    /// URL passed to the importer (resolved relative to the base URL, if any).
    pub original_url: SassUrl,
}

/// In-memory cache of parsed stylesheets imported by Sass.
///
/// Rewritten from Dart's `ImportCache` (`import_cache.dart`): resolves new
/// imports through the importer chain and caches both canonicalizations and
/// parsed stylesheets. Flows by ownership (created in `compile_string`,
/// stored in [`EvalState`](crate::eval::EvalState), taken back on success).
pub struct ImportCache<'compile, 'parse> {
    pub arena: &'compile Bump,
    importers: Vec<Importer<'parse>>,
    parse_selectors: bool,

    canonicalize_cache: HashMap<(SassUrl, bool), Option<CanonicalizeResult<'parse>>>,
    per_importer_canonicalize_cache:
        HashMap<(Importer<'parse>, SassUrl, bool), Option<CanonicalizeResult<'parse>>>,
    non_canonical_relative_urls: HashMap<(Importer<'parse>, SassUrl, bool), SassUrl>,

    import_cache: HashMap<SassUrl, Option<&'parse Stylesheet<'parse>>>,
    results_cache: HashMap<SassUrl, ImporterResult>,
    load_times: HashMap<SassUrl, SassTime>,
}

impl<'compile: 'parse, 'parse> ImportCache<'compile, 'parse> {
    /// Creates an import cache that resolves imports using `importers`.
    ///
    /// Rewritten from Dart's `ImportCache.new` (`import_cache.dart`): the Rust
    /// entry point splits construction into [`ImportCache::new`] (explicit
    /// importer list) plus [`ImportCache::new_with_options`] (which builds the
    /// full chain: user importers, then each load path as a filesystem
    /// importer, then each `SASS_PATH` entry, then the package-config
    /// importer).
    pub fn new(
        arena: &'compile Bump,
        importers: Vec<Importer<'parse>>,
        parse_selectors: bool,
    ) -> Self {
        ImportCache {
            arena,
            importers,
            parse_selectors,
            canonicalize_cache: HashMap::new(),
            per_importer_canonicalize_cache: HashMap::new(),
            non_canonical_relative_urls: HashMap::new(),
            import_cache: HashMap::new(),
            results_cache: HashMap::new(),
            load_times: HashMap::new(),
        }
    }

    /// Creates an import cache without any globally-available importers.
    ///
    /// Matches Dart: `ImportCache.none`.
    pub fn new_none(arena: &'compile Bump, parse_selectors: bool) -> Self {
        ImportCache::new(arena, Vec::new(), parse_selectors)
    }

    /// Creates an import cache with only the passed importers.
    ///
    /// Matches Dart: `ImportCache.only`.
    pub fn new_only(
        arena: &'compile Bump,
        importers: Vec<Importer<'parse>>,
        parse_selectors: bool,
    ) -> Self {
        ImportCache::new(arena, importers.to_vec(), parse_selectors)
    }

    pub fn new_with_options(
        arena: &'compile Bump,
        importers: Vec<Importer<'parse>>,
        load_paths: Vec<String>,
        sass_path: &str,
        parse_selectors: bool,
        io: Rc<dyn Io>,
        package_config: Option<Rc<dyn PackageConfig>>,
    ) -> Self {
        let importers =
            ImportCache::to_importers(arena, importers, load_paths, sass_path, io, package_config);
        ImportCache::new(arena, importers, parse_selectors)
    }

    fn to_importers(
        arena: &'compile Bump,
        importers: Vec<Importer<'parse>>,
        load_paths: Vec<String>,
        sass_path: &str,
        io: Rc<dyn Io>,
        package_config: Option<Rc<dyn PackageConfig>>,
    ) -> Vec<Importer<'parse>> {
        let mut result = importers;
        for path in load_paths {
            result.push(Importer::new(
                arena,
                ImporterKind::Filesystem(FilesystemImporter::new(&path, Rc::clone(&io))),
            ));
        }
        if !sass_path.is_empty() {
            let sep = if cfg!(target_os = "windows") {
                ";"
            } else {
                ":"
            };
            for path in sass_path.split(sep) {
                if !path.is_empty() {
                    result.push(Importer::new(
                        arena,
                        ImporterKind::Filesystem(FilesystemImporter::new(path, Rc::clone(&io))),
                    ));
                }
            }
        }
        if let Some(pc) = package_config {
            result.push(Importer::new(
                arena,
                ImporterKind::Package(PackageImporter::new(pc, Rc::clone(&io))),
            ));
        }
        result
    }

    /// Canonicalizes `url` according to one of this cache's importers.
    ///
    /// `base_url` is the canonical URL of the containing stylesheet, if it
    /// exists. If `base_importer` is set, a scheme-less (relative) `url` is
    /// first resolved against `base_url` and tried against the base importer.
    /// Returns the importer used, the canonical URL, and the original URL
    /// (resolved relative to `base_url` if applicable) — or `None` when no
    /// importer recognizes `url`.
    ///
    /// Rewritten from Dart's `ImportCache.canonicalize` (`import_cache.dart`):
    /// the global cache covers loads through the whole importer chain; the
    /// per-importer cache covers base-importer relative loads and results
    /// where some chain element was uncacheable (accessed the containing URL).
    #[rust_sass_macros::maybe_async]
    pub async fn canonicalize(
        &mut self,
        url: &SassUrl,
        base_importer: Option<&Importer<'parse>>,
        base_url: Option<&SassUrl>,
        for_import: bool,
        warn_logger: &dyn WarnLogger<'parse>,
    ) -> SassResult<Option<CanonicalizeResult<'parse>>> {
        // Libsass `@import` precedence: opt-in `User` importers (see
        // `prefers_raw_load_paths` — only C-ABI bridges today) see the raw
        // URL + containing URL before the base importer, mirroring
        // upstream's consult-customs-first `call_loader`. `None` falls
        // through to the Dart order below. Gated on `for_import` so
        // `@use`/`@forward` keep base-first resolution; default-`false`
        // hosts never take this branch (scan short-circuits on the first
        // non-opt-in importer only in the sense that `any` is false —
        // no URLs, containing rules, or cache keys change for them).
        // No cache write here: upstream has no import cache, and a result
        // may vary by containing URL while the global keys omit it.
        if for_import && url.scheme().is_empty() && base_url.is_some() {
            let any_opt_in = self.importers.iter().any(|i| match i.kind() {
                ImporterKind::User(u) => u.prefers_raw_load_paths(),
                _ => false,
            });
            if any_opt_in {
                let importers = std::mem::take(&mut self.importers);
                let mut hit: Option<CanonicalizeResult<'parse>> = None;
                for importer in importers.iter().filter(|i| match i.kind() {
                    ImporterKind::User(u) => u.prefers_raw_load_paths(),
                    _ => false,
                }) {
                    // `Some(url)` = raw load path + forced containing (the
                    // opt-in shape in `canonicalize_with`).
                    let (r, _) = self
                        .canonicalize_with(
                            importer,
                            url,
                            base_url,
                            for_import,
                            warn_logger,
                            Some(url),
                        )
                        .await?;
                    if r.is_some() {
                        hit = r;
                        break;
                    }
                }
                self.importers = importers;
                if let Some(cr) = hit {
                    return Ok(Some(cr));
                }
            }
        }
        // Matches Dart (import_cache.dart:165-187): whenever a scheme-less
        // (relative) URL is loaded, the base importer is tried first with the
        // URL resolved against the base URL (if any). Rust stores relative URLs
        // as `sass-relative:` with an empty `scheme()`, so this generalizes the
        // old file-only handling to any base URL (e.g. an entrypoint importer
        // with a `u:` scheme). File bases keep the path-based `resolve_file_path`
        // (which preserves `..`); other bases resolve via `SassUrl::resolve`.
        if let Some(base_imp) = base_importer {
            if url.scheme().is_empty() {
                let resolved_url = if url.is_file_like() && base_url.is_some_and(|b| b.is_file()) {
                    resolve_file_path(url, base_url.unwrap()).unwrap_or_else(|| url.clone())
                } else if let Some(base) = base_url {
                    base.resolve(url.path()).unwrap_or_else(|| url.clone())
                } else {
                    url.clone()
                };
                let key = (Importer::clone(base_imp), resolved_url.clone(), for_import);

                if let Some(cached) = self.per_importer_canonicalize_cache.get(&key) {
                    if let Some(cr) = cached {
                        return Ok(Some(cr.clone()));
                    }
                } else {
                    let (result, cacheable) = self
                        .canonicalize_with(
                            base_imp,
                            &resolved_url,
                            base_url,
                            for_import,
                            warn_logger,
                            // The raw load path rides along for opt-in hosts
                            // (see `prefers_raw_load_paths`); everyone else
                            // keeps the resolved URL exactly as before.
                            Some(url),
                        )
                        .await?;
                    debug_assert!(cacheable);
                    if base_url.is_some() {
                        self.non_canonical_relative_urls
                            .insert(key.clone(), url.clone());
                    }
                    self.per_importer_canonicalize_cache
                        .insert(key, result.clone());
                    if let Some(cr) = result {
                        return Ok(Some(cr));
                    }
                }
            }
        }

        let global_key = (url.clone(), for_import);
        if let Some(cached) = self.canonicalize_cache.get(&global_key) {
            return Ok(cached.clone());
        }

        let mut cacheable = true;
        let importers = std::mem::take(&mut self.importers);
        let _n = importers.len();
        for (i, importer) in importers.iter().enumerate() {
            let per_key = (*importer, url.clone(), for_import);
            if let Some(cached) = self.per_importer_canonicalize_cache.get(&per_key) {
                match cached {
                    Some(cr) => {
                        self.importers = importers;
                        return Ok(Some(cr.clone()));
                    }
                    None => continue,
                }
            }

            let (result, cr_cacheable) = self
                .canonicalize_with(importer, url, base_url, for_import, warn_logger, None)
                .await?;

            match (result, cr_cacheable, cacheable) {
                (Some(cr), true, true) => {
                    let result = Some(cr.clone());
                    self.canonicalize_cache
                        .insert(global_key.clone(), result.clone());
                    self.importers = importers;
                    return Ok(result);
                }
                (None, true, true) => {
                    // Not found, still globally cacheable — continue loop
                }
                (cr, true, false) => {
                    let result = cr;
                    self.per_importer_canonicalize_cache
                        .insert(per_key.clone(), result.clone());
                    if result.is_some() {
                        self.importers = importers;
                        return Ok(result);
                    }
                }
                (cr, false, _) => {
                    if cacheable {
                        for importer in importers.iter().take(i) {
                            let prev_key = (*importer, url.clone(), for_import);
                            self.per_importer_canonicalize_cache.insert(prev_key, None);
                        }
                        cacheable = false;
                    }
                    if let Some(r) = cr {
                        self.importers = importers;
                        return Ok(Some(r.clone()));
                    }
                }
            }
        }

        if cacheable {
            self.canonicalize_cache.insert(global_key, None);
        }
        self.importers = importers;
        Ok(None)
    }

    // Calls `importer.canonicalize` and reports whether the result is
    // cacheable. Rewritten from Dart's `ImportCache._canonicalize`
    // (import_cache.dart): the containing URL is passed only for relative URLs
    // or non-canonical schemes, so canonical URLs stay context-independent; a
    // result is cacheable unless the importer read the containing URL.
    // Canonicalizing to a URL with a scheme the importer declares
    // non-canonical is an error.
    // `raw_url` carries the pre-resolution load path for the base-importer
    // fast path; only a `User` importer opting in via
    // `prefers_raw_load_paths` ever sees it (invoked with the raw URL and a
    // forced containing URL — libsass `call_loader` shape). Everyone else
    // keeps `url` and the Dart containing rule, unchanged.
    #[rust_sass_macros::maybe_async]
    async fn canonicalize_with(
        &mut self,
        importer: &Importer<'parse>,
        url: &SassUrl,
        base_url: Option<&SassUrl>,
        for_import: bool,
        warn_logger: &dyn WarnLogger<'parse>,
        raw_url: Option<&SassUrl>,
    ) -> SassResult<(Option<CanonicalizeResult<'parse>>, bool)> {
        let raw = raw_url.filter(|_| match importer.kind() {
            ImporterKind::User(u) => u.prefers_raw_load_paths(),
            _ => false,
        });
        let call_url = raw.unwrap_or(url);
        let pass_containing_url = base_url.is_some()
            && (raw.is_some()
                || url.is_relative()
                || url.scheme().is_empty()
                || importer.is_non_canonical_scheme(url.scheme()));

        let mut canonicalize_ctx = CanonicalizeContext::new(
            if pass_containing_url {
                base_url.cloned()
            } else {
                None
            },
            for_import,
        );

        let result = match importer.kind() {
            ImporterKind::Filesystem(fs) => {
                fs.canonicalize(url, &mut canonicalize_ctx, warn_logger)
                    .await
            }
            ImporterKind::Package(p) => {
                p.canonicalize(url, &mut canonicalize_ctx, warn_logger)
                    .await
            }
            ImporterKind::NodePackage(np) => {
                np.canonicalize(url, &mut canonicalize_ctx, warn_logger)
                    .await
            }
            _ => importer.canonicalize(call_url, &mut canonicalize_ctx).await,
        }?;

        let cacheable = !pass_containing_url || !canonicalize_ctx.was_containing_url_accessed();

        match result {
            None => Ok((None, cacheable)),
            Some(canonical_url) => {
                if !canonical_url.scheme().is_empty()
                    && importer.is_non_canonical_scheme(canonical_url.scheme())
                {
                    // Dart interpolates the importer with `toString()`
                    // (import_cache.dart:275-276), not `{:?}`.
                    return Err(Box::new(SassError::Script {
                        message: format!(
                            "Importer {} canonicalized {} to {}, which uses a \
                             scheme declared as non-canonical.",
                            importer, url, canonical_url
                        ),
                        argument_name: None,
                    }));
                }
                Ok((
                    Some(CanonicalizeResult {
                        importer: *importer,
                        canonical_url,
                        // The URL the importer actually saw (raw in the
                        // opt-in path, resolved otherwise).
                        original_url: call_url.clone(),
                    }),
                    cacheable,
                ))
            }
        }
    }

    /// Tries to import `url` using one of this cache's importers.
    ///
    /// If `base_importer` is set, it is tried first with `url` resolved
    /// relative to `base_url`. Returns the importer plus the parsed stylesheet
    /// on success, `None` when no importer can load `url`. Results are cached.
    ///
    /// Matches Dart: `ImportCache.import`.
    #[rust_sass_macros::maybe_async]
    pub async fn import(
        &mut self,
        url: &SassUrl,
        base_importer: Option<&Importer<'parse>>,
        base_url: Option<&SassUrl>,
        for_import: bool,
        warn_logger: &dyn WarnLogger<'parse>,
    ) -> SassResult<Option<(Importer<'parse>, &'parse Stylesheet<'parse>)>> {
        let result = self
            .canonicalize(url, base_importer, base_url, for_import, warn_logger)
            .await?;
        match result {
            Some(cr) => {
                let importer = cr.importer;
                let stylesheet = self
                    .import_canonical(&importer, &cr.canonical_url, Some(&cr.original_url))
                    .await?;
                match stylesheet {
                    Some(s) => Ok(Some((importer, s))),
                    None => Ok(None),
                }
            }
            None => Ok(None),
        }
    }

    /// Tries to load the canonicalized `canonical_url` using `importer`.
    ///
    /// Returns the parsed stylesheet, or `None` when the importer cannot load
    /// it. If passed, `original_url` is the pre-canonicalization URL, used to
    /// resolve a relative canonical URL (kept for backwards compatibility).
    ///
    /// Matches Dart: `ImportCache.importCanonical`. Load times are recorded
    /// success-only: a miss caches `None` with no load time.
    #[rust_sass_macros::maybe_async]
    pub async fn import_canonical(
        &mut self,
        importer: &Importer<'parse>,
        canonical_url: &SassUrl,
        original_url: Option<&SassUrl>,
    ) -> SassResult<Option<&'parse Stylesheet<'parse>>> {
        let resolved_url = match original_url {
            Some(orig) => orig
                .resolve(canonical_url.as_str())
                .unwrap_or_else(|| canonical_url.clone()),
            None => canonical_url.clone(),
        };

        match self.import_cache.entry(canonical_url.clone()) {
            hash_map::Entry::Occupied(entry) => Ok(*entry.get()),
            hash_map::Entry::Vacant(entry) => {
                let load_time = SassTime::now();
                let result = importer.load(canonical_url).await?;
                if result.is_none() {
                    // Dart records `load_times` success-only
                    // (import_cache.dart:328-333): a miss inserts `None` into
                    // the import cache but no load time.
                    entry.insert(None);
                    return Ok(None);
                }
                let result = result.unwrap();
                self.load_times.insert(canonical_url.clone(), load_time);
                self.results_cache
                    .insert(canonical_url.clone(), result.clone());

                let stylesheet = self.arena.alloc(parse_stylesheet(
                    &result.contents,
                    &resolved_url,
                    &result.syntax,
                    self.parse_selectors,
                    self.arena,
                )?);
                entry.insert(Some(stylesheet));
                Ok(Some(stylesheet))
            }
        }
    }

    /// Returns a human-friendly URL for `canonical_url` for stack traces.
    ///
    /// Picks the shortest original URL that canonicalized to it (with the
    /// canonical basename, so e.g. `package:example/_example.scss` rather than
    /// `package:example/example` shows), or the canonical URL itself when
    /// nothing was loaded by this cache. Original URLs without a scheme are
    /// ignored: they can be ambiguous with `file:` URLs resolved relative to
    /// the current working directory (sass/dart-sass#2777).
    ///
    /// Matches Dart: `ImportCache.humanize`.
    pub fn humanize(&self, canonical_url: &SassUrl) -> String {
        let shortest = self
            .canonicalize_cache
            .values()
            .filter_map(|v| v.as_ref())
            .filter(|r| r.canonical_url == *canonical_url)
            .filter(|r| r.original_url.has_scheme())
            .min_by_key(|r| r.original_url.path().len());

        match shortest {
            Some(cr) => {
                let path = Path::new(canonical_url.path());
                let base = path
                    .file_name()
                    .map(|n| n.to_str().unwrap_or(""))
                    .unwrap_or("");
                if let Some(resolved) = cr.original_url.resolve(base) {
                    return resolved.to_string();
                }
                canonical_url.to_string()
            }
            None => canonical_url.to_string(),
        }
    }

    /// Returns the URL to use in the source map for `canonical_url`.
    ///
    /// Falls back to the canonical URL itself when it was not loaded by this
    /// cache. Matches Dart: `ImportCache.sourceMapUrl`.
    pub fn source_map_url(&self, canonical_url: &SassUrl) -> SassUrl {
        self.results_cache
            .get(canonical_url)
            .map(|r| r.source_map_url().clone())
            .unwrap_or_else(|| canonical_url.clone())
    }

    /// Returns the most recent load time for `canonical_url`, or `None` if it
    /// was never loaded.
    ///
    /// Matches Dart: `ImportCache.loadTime` (internal).
    pub fn load_time(&self, canonical_url: &SassUrl) -> Option<&SassTime> {
        self.load_times.get(canonical_url)
    }

    /// Clears all cached canonicalizations that could produce `canonical_url`.
    ///
    /// Matches Dart: `ImportCache.clearCanonicalize` (internal/nodoc).
    pub fn clear_canonicalize(&mut self, canonical_url: &SassUrl) {
        let keys: Vec<(SassUrl, bool)> = self.canonicalize_cache.keys().cloned().collect();
        for key in keys {
            for importer in &self.importers {
                if importer.could_canonicalize(&key.0, canonical_url) {
                    self.canonicalize_cache.remove(&key);
                    break;
                }
            }
        }

        let per_keys: Vec<(Importer<'parse>, SassUrl, bool)> = self
            .per_importer_canonicalize_cache
            .keys()
            .cloned()
            .collect();
        for (importer, url, _for_import) in per_keys {
            if importer.could_canonicalize(&url, canonical_url) {
                self.per_importer_canonicalize_cache
                    .remove(&(importer, url, _for_import));
            }
        }
    }

    /// Clears the cached parse tree for `canonical_url`.
    ///
    /// No effect when the file was not cached. Matches Dart:
    /// `ImportCache.clearImport` (internal/nodoc).
    pub fn clear_import(&mut self, canonical_url: &SassUrl) {
        self.results_cache.remove(canonical_url);
        self.import_cache.remove(canonical_url);
    }
}

fn parse_stylesheet<'compile: 'parse, 'parse>(
    contents: &str,
    url: &SassUrl,
    syntax: &Syntax,
    parse_selectors: bool,
    arena: &'compile Bump,
) -> SassResult<Stylesheet<'parse>> {
    match syntax {
        Syntax::Scss => Stylesheet::parse_scss(contents, Some(url), parse_selectors, arena),
        Syntax::Sass(_) => Stylesheet::parse_sass(contents, Some(url), parse_selectors, arena),
        Syntax::Css(_) => {
            let disallowed = disallowed_function_names(arena);
            Stylesheet::parse_css(contents, Some(url), parse_selectors, &disallowed, arena)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::importer::UserImporter;
    use crate::io::DefaultIo;
    use crate::logger::{BufferedWarnLogger, NoOpWarnLogger};
    use crate::url::resolve_file_path;
    use std::cell::RefCell;
    // Needed only in async mode: sync expansion rewrites the hand-written
    // `-> LocalBoxFuture` returns away (see rust-sass-macros Patch 2).
    #[cfg(feature = "async")]
    use futures::future::LocalBoxFuture;

    use std::rc::Rc;

    fn make_io() -> Rc<dyn Io> {
        Rc::new(DefaultIo::new())
    }

    fn make_fs_importer<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        io: Rc<dyn Io>,
    ) -> Importer<'parse> {
        Importer::new(
            arena,
            ImporterKind::Filesystem(FilesystemImporter::new_no_load_path(io)),
        )
    }

    #[test]
    fn new_creates_empty_caches() {
        let arena = Bump::new();
        let cache = ImportCache::new_none(&arena, false);
        assert!(cache.importers.is_empty());
        assert!(cache.canonicalize_cache.is_empty());
        assert!(cache.import_cache.is_empty());
    }

    #[test]
    fn to_importers_adds_load_paths() {
        let io = make_io();
        let arena = Bump::new();
        let importers = ImportCache::to_importers(
            &arena,
            vec![],
            vec!["/tmp/sass".to_string()],
            "",
            Rc::clone(&io),
            None,
        );
        assert_eq!(importers.len(), 1);
    }

    #[test]
    fn to_importers_splits_sass_path() {
        let io = make_io();
        let arena = Bump::new();
        // `SASS_PATH` entries split on `;` on Windows, `:` elsewhere
        // (matching Dart) — build the input with the platform separator.
        let sep = if cfg!(target_os = "windows") {
            ";"
        } else {
            ":"
        };
        let importers = ImportCache::to_importers(
            &arena,
            vec![],
            vec![],
            &format!("/a{sep}/b"),
            Rc::clone(&io),
            None,
        );
        assert_eq!(importers.len(), 2);
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_with_filesystem_finds_file() {
        let io = make_io();
        let arena = Bump::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.scss");
        std::fs::write(&path, b"a { color: red; }").unwrap();

        let importer = make_fs_importer(&arena, Rc::clone(&io));
        let mut cache = ImportCache::new(&arena, vec![importer], false);
        let url = SassUrl::parse(&format!("file:///{}", path.to_string_lossy())).unwrap();

        let result = cache
            .canonicalize(&url, None, None, false, &BufferedWarnLogger::new(&arena))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.importer, importer);
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_cache_hit() {
        let io = make_io();
        let arena2 = Bump::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.scss");
        std::fs::write(&path, b"a { color: red; }").unwrap();

        let importer = make_fs_importer(&arena2, Rc::clone(&io));
        let mut cache = ImportCache::new(&arena2, vec![importer], false);
        let url = SassUrl::parse(&format!("file:///{}", path.to_string_lossy())).unwrap();

        let r1 = cache
            .canonicalize(&url, None, None, false, &BufferedWarnLogger::new(&arena2))
            .await
            .unwrap();
        let r2 = cache
            .canonicalize(&url, None, None, false, &BufferedWarnLogger::new(&arena2))
            .await
            .unwrap();
        // Both should return the same result (second from cache)
        assert_eq!(
            r1.as_ref().unwrap().canonical_url,
            r2.as_ref().unwrap().canonical_url
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn import_canonical_parses_scss() {
        let io = make_io();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.scss");
        std::fs::write(&path, b"a { color: red; }").unwrap();
        let canonical = io.canonicalize(&path).await.unwrap();

        let arena = Bump::new();
        let importer = make_fs_importer(&arena, Rc::clone(&io));
        let mut cache = ImportCache::new(&arena, vec![], false);
        let url = SassUrl::parse(&format!("file:///{}", canonical)).unwrap();

        let stylesheet = cache
            .import_canonical(&importer, &url, None)
            .await
            .unwrap()
            .unwrap();
        assert!(!stylesheet.children.is_empty());
    }

    #[rust_sass_macros::maybe_test]
    async fn import_canonical_caches_result() {
        let io = make_io();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.scss");
        std::fs::write(&path, b"a { color: red; }").unwrap();
        let canonical = io.canonicalize(&path).await.unwrap();

        let arena = Bump::new();
        let importer = make_fs_importer(&arena, Rc::clone(&io));
        let mut cache = ImportCache::new(&arena, vec![], false);
        let url = SassUrl::parse(&format!("file:///{}", canonical)).unwrap();

        let _s1 = cache.import_canonical(&importer, &url, None).await.unwrap();
        // Second call should hit cache
        let s2 = cache.import_canonical(&importer, &url, None).await.unwrap();
        assert!(s2.is_some());
    }

    /// A user importer that mirrors the JS file importer (`findFileUrl`): for
    /// non-`file:` URLs it reads the containing URL and resolves `x.scss`
    /// against it (marking the containing URL as accessed). Regression for
    /// sass/dart-sass#2208 — a relative URL resolved from different base URLs
    /// must not be globally cached.
    #[derive(Debug)]
    struct ContainingUrlProbeImporter {
        fs: FilesystemImporter,
        // Test-probe call log; a named alias would be single-use test noise.
        #[allow(clippy::type_complexity)]
        calls: Rc<RefCell<Vec<(String, Option<String>)>>>,
    }

    #[rust_sass_macros::maybe_async]
    impl UserImporter for ContainingUrlProbeImporter {
        fn canonicalize<'a>(
            &'a self,
            url: &'a SassUrl,
            context: &'a mut CanonicalizeContext,
        ) -> LocalBoxFuture<'a, SassResult<Option<SassUrl>>> {
            if url.scheme() == "file" {
                let fs = &self.fs;
                return Box::pin(
                    async move { fs.canonicalize(url, context, &NoOpWarnLogger).await },
                );
            }
            let containing = context.containing_url().cloned();
            let calls = self.calls.clone();
            let fs = self.fs.clone();
            Box::pin(async move {
                calls
                    .borrow_mut()
                    .push((url.to_string(), containing.as_ref().map(|c| c.to_string())));
                if url.to_string() == "y" {
                    if let Some(base) = containing {
                        let x = SassUrl::parse("x.scss").unwrap();
                        if let Some(fu) = resolve_file_path(&x, &base) {
                            return fs.canonicalize(&fu, context, &NoOpWarnLogger).await;
                        }
                    }
                }
                Ok(None)
            })
        }

        fn load<'a>(
            &'a self,
            url: &'a SassUrl,
        ) -> LocalBoxFuture<'a, SassResult<Option<ImporterResult>>> {
            let fs = &self.fs;
            Box::pin(async move { fs.load(url).await })
        }

        fn is_non_canonical_scheme(&self, scheme: &str) -> bool {
            scheme != "file"
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn relative_url_from_different_bases_not_globally_cached() {
        let io = make_io();
        let arena = Bump::new();
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        for sub in ["sub1", "sub1/sub2"] {
            std::fs::create_dir_all(base.join(sub)).unwrap();
            std::fs::write(base.join(sub).join("test.scss"), "").unwrap();
            std::fs::write(base.join(sub).join("x.scss"), "").unwrap();
        }
        std::fs::write(base.join("main.scss"), "").unwrap();
        let f =
            |p: &str| SassUrl::parse(&format!("file://{}/{}", base.to_string_lossy(), p)).unwrap();

        let fs_imp = make_fs_importer(&arena, Rc::clone(&io));
        let probe = Rc::new(ContainingUrlProbeImporter {
            fs: FilesystemImporter::new_no_load_path(Rc::clone(&io)),
            calls: Rc::new(RefCell::new(Vec::new())),
        });
        let probe_imp = Importer::new(&arena, ImporterKind::User(probe.clone()));
        let mut cache = ImportCache::new(&arena, vec![probe_imp], false);
        let warn = BufferedWarnLogger::new(&arena);

        // main.scss loads `@use "sub1/test"` and `@use "sub1/sub2/test"`.
        let main = f("main.scss");
        let sub1 = f("sub1/test.scss");
        let sub2 = f("sub1/sub2/test.scss");
        let r = cache
            .canonicalize(
                &SassUrl::parse("sub1/test").unwrap(),
                Some(&fs_imp),
                Some(&main),
                false,
                &warn,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.canonical_url, sub1);
        let r = cache
            .canonicalize(
                &SassUrl::parse("sub1/sub2/test").unwrap(),
                Some(&fs_imp),
                Some(&main),
                false,
                &warn,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.canonical_url, sub2);

        // `@use "y"` from sub1/test.scss then from sub1/sub2/test.scss: the
        // relative URL is context-sensitive (depends on the containing URL),
        // so it must reach the importer twice and canonicalize to different
        // files.
        let r1 = cache
            .canonicalize(
                &SassUrl::parse("y").unwrap(),
                Some(&fs_imp),
                Some(&sub1),
                false,
                &warn,
            )
            .await
            .unwrap()
            .unwrap();
        let r2 = cache
            .canonicalize(
                &SassUrl::parse("y").unwrap(),
                Some(&fs_imp),
                Some(&sub2),
                false,
                &warn,
            )
            .await
            .unwrap()
            .unwrap();

        let calls = probe.calls.borrow();
        assert_eq!(calls.len(), 2, "calls: {calls:?}");
        assert_eq!(calls[0].0, "y");
        assert_eq!(calls[1].0, "y");
        assert_ne!(calls[0].1, calls[1].1);
        assert!(r1.canonical_url.as_str().ends_with("sub1/x.scss"));
        assert!(r2.canonical_url.as_str().ends_with("sub1/sub2/x.scss"));
    }

    #[test]
    fn source_map_url_falls_back_to_canonical() {
        let arena = Bump::new();
        let cache = ImportCache::new_none(&arena, false);
        let url = SassUrl::parse("file:///style.scss").unwrap();
        assert_eq!(cache.source_map_url(&url).as_str(), "file:///style.scss");
    }

    /// A user importer recording `(url, containing)` per call, with a
    /// `prefers_raw_load_paths` switch. Regression for the libsass C-API
    /// seam: an opt-in importer must see the raw load path plus the
    /// containing URL on the base-importer fast path (libsass `call_loader`
    /// shape); default hosts keep the resolved URL without containing
    /// (Dart behavior, unchanged).
    #[derive(Debug)]
    struct RawProbeImporter {
        // Test-probe call log; a named alias would be single-use test noise.
        #[allow(clippy::type_complexity)]
        calls: Rc<RefCell<Vec<(String, Option<String>)>>>,
        raw: bool,
    }

    #[rust_sass_macros::maybe_async]
    impl UserImporter for RawProbeImporter {
        fn canonicalize<'a>(
            &'a self,
            url: &'a SassUrl,
            context: &'a mut CanonicalizeContext,
        ) -> LocalBoxFuture<'a, SassResult<Option<SassUrl>>> {
            let seen_url = url.to_string();
            let seen_containing = context
                .containing_url_without_marking()
                .map(|u| u.to_string());
            let calls = self.calls.clone();
            Box::pin(async move {
                calls.borrow_mut().push((seen_url.clone(), seen_containing));
                Ok(Some(
                    SassUrl::parse(&format!("file:///probe/{seen_url}")).unwrap(),
                ))
            })
        }

        fn load<'a>(
            &'a self,
            _url: &'a SassUrl,
        ) -> LocalBoxFuture<'a, SassResult<Option<ImporterResult>>> {
            Box::pin(async move { Ok(None) })
        }

        fn prefers_raw_load_paths(&self) -> bool {
            self.raw
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn raw_opt_in_sees_raw_url_and_containing() {
        let arena = Bump::new();
        let probe = Rc::new(RawProbeImporter {
            calls: Rc::new(RefCell::new(Vec::new())),
            raw: true,
        });
        let probe_imp = Importer::new(&arena, ImporterKind::User(probe.clone()));
        let mut cache = ImportCache::new(&arena, vec![probe_imp], false);
        let warn = BufferedWarnLogger::new(&arena);
        // Fake file base (no filesystem needed — the probe serves all URLs).
        let base = SassUrl::parse("file:///sub/test.scss").unwrap();
        let r = cache
            .canonicalize(
                &SassUrl::parse("y").unwrap(),
                Some(&probe_imp),
                Some(&base),
                false,
                &warn,
            )
            .await
            .unwrap()
            .unwrap();
        let calls = probe.calls.borrow();
        assert_eq!(
            calls.as_slice(),
            &[("y".to_string(), Some("file:///sub/test.scss".to_string()))],
            "calls: {calls:?}"
        );
        assert_eq!(r.canonical_url.as_str(), "file:///probe/y");
    }

    #[rust_sass_macros::maybe_test]
    async fn raw_opt_out_sees_resolved_url_without_containing() {
        let arena = Bump::new();
        let probe = Rc::new(RawProbeImporter {
            calls: Rc::new(RefCell::new(Vec::new())),
            raw: false,
        });
        let probe_imp = Importer::new(&arena, ImporterKind::User(probe.clone()));
        let mut cache = ImportCache::new(&arena, vec![probe_imp], false);
        let warn = BufferedWarnLogger::new(&arena);
        let base = SassUrl::parse("file:///sub/test.scss").unwrap();
        let r = cache
            .canonicalize(
                &SassUrl::parse("y").unwrap(),
                Some(&probe_imp),
                Some(&base),
                false,
                &warn,
            )
            .await
            .unwrap()
            .unwrap();
        let calls = probe.calls.borrow();
        assert_eq!(
            calls.as_slice(),
            &[(String::from("file:///sub/y"), None)],
            "calls: {calls:?}"
        );
        assert_eq!(r.canonical_url.as_str(), "file:///probe/file:///sub/y");
    }

    /// Libsass `@import` precedence: an opt-in `User` importer is consulted
    /// with the raw URL + containing URL before the filesystem base, even
    /// when the file exists on disk (upstream consults customs first per
    /// import; the Dart order consults the base first).
    #[rust_sass_macros::maybe_test]
    async fn opt_in_importer_precedes_filesystem_base_for_import() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("y.scss"), "a { b: c; }").unwrap();
        let base = SassUrl::parse(&format!(
            "file://{}/sub/test.scss",
            dir.path().to_string_lossy()
        ))
        .unwrap();

        let arena = Bump::new();
        let io = make_io();
        let fs_imp = make_fs_importer(&arena, Rc::clone(&io));
        let out = Rc::new(RawProbeImporter {
            calls: Rc::new(RefCell::new(Vec::new())),
            raw: false,
        });
        let out_imp = Importer::new(&arena, ImporterKind::User(out.clone()));
        let inn = Rc::new(RawProbeImporter {
            calls: Rc::new(RefCell::new(Vec::new())),
            raw: true,
        });
        let in_imp = Importer::new(&arena, ImporterKind::User(inn.clone()));
        let mut cache = ImportCache::new(&arena, vec![out_imp, in_imp], false);
        let warn = BufferedWarnLogger::new(&arena);
        let r = cache
            .canonicalize(
                &SassUrl::parse("y").unwrap(),
                Some(&fs_imp),
                Some(&base),
                true,
                &warn,
            )
            .await
            .unwrap()
            .unwrap();
        // Opt-in fires first with raw + containing and wins, although
        // `sub/y.scss` exists; the opt-out importer never fires.
        assert_eq!(
            inn.calls.borrow().as_slice(),
            &[("y".to_string(), Some(base.to_string()))],
            "calls: {:?}",
            inn.calls.borrow()
        );
        assert!(out.calls.borrow().is_empty());
        assert_eq!(r.canonical_url.as_str(), "file:///probe/y");
    }

    /// `@use` keeps Dart order: the filesystem base wins, opt-in importers
    /// are not pre-consulted.
    #[rust_sass_macros::maybe_test]
    async fn opt_in_importer_keeps_dart_order_for_use() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("y.scss"), "a { b: c; }").unwrap();
        let base = SassUrl::parse(&format!(
            "file://{}/sub/test.scss",
            dir.path().to_string_lossy()
        ))
        .unwrap();

        let arena = Bump::new();
        let io = make_io();
        let fs_imp = make_fs_importer(&arena, Rc::clone(&io));
        let inn = Rc::new(RawProbeImporter {
            calls: Rc::new(RefCell::new(Vec::new())),
            raw: true,
        });
        let in_imp = Importer::new(&arena, ImporterKind::User(inn.clone()));
        let mut cache = ImportCache::new(&arena, vec![in_imp], false);
        let warn = BufferedWarnLogger::new(&arena);
        let r = cache
            .canonicalize(
                &SassUrl::parse("y").unwrap(),
                Some(&fs_imp),
                Some(&base),
                false,
                &warn,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(inn.calls.borrow().is_empty());
        assert!(r.canonical_url.as_str().ends_with("/sub/y.scss"));
    }

    #[rust_sass_macros::maybe_test]
    async fn clear_import_removes_from_cache() {
        let io = make_io();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.scss");
        std::fs::write(&path, b"a { color: red; }").unwrap();
        let canonical = io.canonicalize(&path).await.unwrap();

        let arena = Bump::new();
        let importer = make_fs_importer(&arena, Rc::clone(&io));
        let mut cache = ImportCache::new(&arena, vec![], false);
        let url = SassUrl::parse(&format!("file:///{}", canonical)).unwrap();

        cache.import_canonical(&importer, &url, None).await.unwrap();
        cache.clear_import(&url);
        assert!(!cache.import_cache.contains_key(&url));
        assert!(!cache.results_cache.contains_key(&url));
    }

    // Dart records `load_times` success-only (import_cache.dart:328-333): a
    // canonicalize miss inserts `None` into the import cache but no load time.
    // (`import_canonical` on a missing file *errors* at `load` — the miss path
    // is a canonicalize returning `None`, exercised here via a `User` importer
    // whose `load` returns `None`.)
    #[rust_sass_macros::maybe_test]
    async fn test_load_time_success_only() {
        #[derive(Debug)]
        struct NoneImporter;
        #[rust_sass_macros::maybe_async]
        impl UserImporter for NoneImporter {
            fn canonicalize<'a>(
                &'a self,
                url: &'a SassUrl,
                _ctx: &'a mut CanonicalizeContext,
            ) -> LocalBoxFuture<'a, SassResult<Option<SassUrl>>> {
                Box::pin(async move { Ok(Some(url.clone())) })
            }
            fn load<'a>(
                &'a self,
                _url: &'a SassUrl,
            ) -> LocalBoxFuture<'a, SassResult<Option<ImporterResult>>> {
                Box::pin(async move { Ok(None) })
            }
        }

        let arena = Bump::new();
        let io = make_io();
        let importer = Importer::new(&arena, ImporterKind::User(Rc::new(NoneImporter)));
        let mut cache = ImportCache::new(&arena, vec![], false);
        let url = SassUrl::parse("file:///does-not-exist-xyz.scss").unwrap();

        let result = cache.import_canonical(&importer, &url, None).await.unwrap();
        assert!(result.is_none());
        assert!(
            cache.load_time(&url).is_none(),
            "miss must not record a load time"
        );
        // A hit still records one.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hit.scss");
        std::fs::write(&path, b"a { color: red; }").unwrap();
        let canonical = io.canonicalize(&path).await.unwrap();
        let hit_url = SassUrl::parse(&format!("file:///{}", canonical)).unwrap();
        let fs_importer = make_fs_importer(&arena, Rc::clone(&io));
        cache
            .import_canonical(&fs_importer, &hit_url, None)
            .await
            .unwrap();
        assert!(cache.load_time(&hit_url).is_some());
    }

    // `humanize` ignores original URLs without a scheme: they can be
    // ambiguous with `file:` URLs resolved relative to the current working
    // directory (sass/dart-sass#2777, #2778). Here the schemeless `dep/...`
    // original is shorter, so without the filter it would win over the
    // `file:` URL.
    #[test]
    fn humanize_ignores_schemeless_original_urls() {
        let arena = Bump::new();
        let io = make_io();
        let importer = make_fs_importer(&arena, Rc::clone(&io));
        let mut cache = ImportCache::new(&arena, vec![importer], false);

        let canonical = SassUrl::parse("file:///app/dep/_lib.scss").unwrap();
        let schemeless = SassUrl::parse("dep/_lib.scss").unwrap();
        assert!(!schemeless.has_scheme());
        let with_scheme = SassUrl::parse("file:///app/dep/_lib.scss").unwrap();
        assert!(with_scheme.has_scheme());
        for original in [schemeless, with_scheme] {
            cache.canonicalize_cache.insert(
                (original.clone(), false),
                Some(CanonicalizeResult {
                    importer,
                    canonical_url: canonical.clone(),
                    original_url: original,
                }),
            );
        }

        assert_eq!(cache.humanize(&canonical), "file:///app/dep/_lib.scss");
    }

    // Without any schemed original URL, `humanize` falls back to the
    // canonical URL itself (unchanged behavior, #2778).
    #[test]
    fn humanize_schemeless_only_falls_back_to_canonical() {
        let arena = Bump::new();
        let io = make_io();
        let importer = make_fs_importer(&arena, Rc::clone(&io));
        let mut cache = ImportCache::new(&arena, vec![importer], false);

        let canonical = SassUrl::parse("file:///app/dep/_lib.scss").unwrap();
        let schemeless = SassUrl::parse("dep/_lib.scss").unwrap();
        cache.canonicalize_cache.insert(
            (schemeless.clone(), false),
            Some(CanonicalizeResult {
                importer,
                canonical_url: canonical.clone(),
                original_url: schemeless,
            }),
        );

        assert_eq!(cache.humanize(&canonical), "file:///app/dep/_lib.scss");
    }
}
