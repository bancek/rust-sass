// Copyright 2024 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/importer/node_package.dart
// go-source: go/eval/node_package_importer.go

use crate::common::time::SassTime;
use crate::io::clean_path;
use crate::url::SassUrl;
#[cfg(feature = "async")]
use futures::future::LocalBoxFuture;
use percent_encoding::percent_decode_str;
use serde_json::Value;
use std::cmp::Ordering;
use std::fmt;
use std::path::Path;
use std::rc::Rc;

use crate::common::exception::{SassError, SassResult};
use crate::eval::importer::resolve_import_path::resolve_import_path;
use crate::eval::importer::{CanonicalizeContext, FilesystemImporter, ImporterResult};
use crate::io::{Io, IoError};
use crate::logger::WarnLogger;

const VALID_EXTENSIONS: &[&str] = &[".scss", ".sass", ".css"];

fn io_err_to_sass(e: IoError) -> SassError {
    SassError::Script {
        message: e.to_string(),
        argument_name: None,
    }
}

/// Dart `p.canonicalize` = `p.normalize(p.absolute(path))`: lexical only — no
/// realpath, no case-correction. Used for the exports/root-values branches of
/// `pkg:` resolution (node_package.dart:83,95).
fn canonicalize_lexical(io: &dyn Io, path: &str) -> String {
    let abs = if Path::new(path).is_absolute() {
        path.to_string()
    } else {
        Path::new(&io.current_dir())
            .join(path)
            .to_string_lossy()
            .into_owned()
    };
    clean_path(&abs)
}

/// An importer that resolves `pkg:` URLs using the Node resolution algorithm.
///
/// Rewritten from Dart's `NodePackageImporter` (`importer/node_package.dart`).
/// `pkg:` is a non-canonical scheme (see
/// [`NodePackageImporter::is_non_canonical_scheme`]), so the containing URL is
/// visible during canonicalization and resolution starts from the containing
/// file's directory, falling back to the entry-point directory.
#[derive(Clone)]
pub struct NodePackageImporter {
    pub entry_point_directory: String,
    cwd: FilesystemImporter,
}

impl fmt::Debug for NodePackageImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodePackageImporter")
            .field("entry_point_directory", &self.entry_point_directory)
            .field("cwd", &self.cwd)
            .finish()
    }
}

impl NodePackageImporter {
    /// Creates a Node package importer with the associated entry point.
    ///
    /// Matches Dart: `NodePackageImporter(entryPointDirectory)`.
    pub fn new(entry_point_directory: &str, io: Rc<dyn Io>) -> Self {
        let abs = Path::new(&io.current_dir()).join(entry_point_directory);
        NodePackageImporter {
            entry_point_directory: abs.to_string_lossy().into_owned(),
            cwd: FilesystemImporter::new_cwd(io),
        }
    }

    /// Canonicalizes a `pkg:` URL: validates the URL shape, splits the bare
    /// import specifier into package name + subpath, walks up to the nearest
    /// `node_modules` root (`PACKAGE_RESOLVE`), then tries the `exports` map,
    /// the `sass`/`style` manifest keys plus `index` fallback, and finally the
    /// subpath relative to the package root.
    ///
    /// `file:` URLs delegate to the working-directory filesystem importer;
    /// other non-`pkg:` schemes return `None`. Invalid package names also
    /// return `None` so another importer may handle them.
    ///
    /// Matches Dart: `NodePackageImporter.canonicalize`.
    #[rust_sass_macros::maybe_async]
    pub async fn canonicalize<'parse>(
        &self,
        url: &SassUrl,
        ctx: &mut CanonicalizeContext,
        warn_logger: &dyn WarnLogger<'parse>,
    ) -> SassResult<Option<SassUrl>> {
        if url.is_file_like() {
            return self.cwd.canonicalize(url, ctx, warn_logger).await;
        }
        // Dart returns `None` for every non-`pkg` scheme INCLUDING relative
        // URLs (scheme `''`) return None like any other non-`pkg` scheme (node_package.dart:34).
        if url.scheme() != "pkg" {
            return Ok(None);
        }

        if url.has_host() {
            return Err(Box::new(SassError::Script {
                message: "A pkg: URL must not have a host, port, username or password.".to_string(),
                argument_name: None,
            }));
        }
        let url_path = url.path();
        if url_path.starts_with('/') {
            return Err(Box::new(SassError::Script {
                message: "A pkg: URL's path must not begin with /.".to_string(),
                argument_name: None,
            }));
        }
        if url_path.is_empty() {
            return Err(Box::new(SassError::Script {
                message: "A pkg: URL must not have an empty path.".to_string(),
                argument_name: None,
            }));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(Box::new(SassError::Script {
                message: "A pkg: URL must not have a query or fragment.".to_string(),
                argument_name: None,
            }));
        }

        let base_directory = if ctx
            .containing_url_without_marking()
            .map(|u| u.is_file_like())
            .unwrap_or(false)
        {
            let containing = ctx.containing_url().unwrap().clone();
            let path = containing.fs_path();
            Path::new(&path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| self.entry_point_directory.clone())
        } else {
            self.entry_point_directory.clone()
        };

        // Dart's `Uri.path` is percent-decoded and `_packageNameAndSubpath`
        // decodes each segment via `p.fromUri` (node_package.dart:113-121), so
        // `pkg:%66oo` resolves the package `foo`.
        let decoded_path: String = percent_decode_str(url_path)
            .decode_utf8_lossy()
            .into_owned();
        let (package_name, subpath) = Self::package_name_and_subpath(&decoded_path);

        if package_name.starts_with('.')
            || package_name.contains('\\')
            || package_name.contains('%')
            || (package_name.starts_with('@') && !package_name.contains('/'))
        {
            return Ok(None);
        }

        let package_root = self
            .resolve_package_root(&package_name, &base_directory)
            .await?;
        let package_root = match package_root {
            Some(p) => p,
            None => return Ok(None),
        };

        let json_path = Path::new(&package_root).join("package.json");
        // Dart `readFile` throws `FileSystemException` on a missing package.json
        // (io/vm.dart) — a MISSING file is not a parse error. Only a present
        // but malformed file throws "Failed to parse ...".
        let json_bytes = self
            .cwd
            .io
            .read_file(&json_path)
            .await
            .map_err(io_err_to_sass)?;
        let json_str = String::from_utf8_lossy(&json_bytes).into_owned();

        let package_manifest: serde_json::Value =
            serde_json::from_str(&json_str).map_err(|e| SassError::Script {
                message: format!(
                    "Failed to parse {} for \"pkg:{}\": {}",
                    json_path.display(),
                    package_name,
                    e
                ),
                argument_name: None,
            })?;

        if let Some(resolved) = self
            .resolve_package_exports(
                &package_root,
                subpath.as_deref(),
                &package_manifest,
                &package_name,
            )
            .await?
        {
            if let Some(ext) = Self::url_extension(&resolved) {
                if VALID_EXTENSIONS.contains(&ext) {
                    // Dart loads the import-only variant for `@import`
                    // (`_resolveImportOnly`, node_package.dart:86).
                    let resolved = self.resolve_import_only(&resolved, ctx.from_import).await?;
                    // Dart canonicalizes the exports/root-values branches with
                    // `p.canonicalize` (pure LEXICAL, no realpath, no
                    // case-correction) — node_package.dart:83,95. Match that.
                    let canonical = canonicalize_lexical(&*self.cwd.io, &resolved);
                    let file_url = SassUrl::file_url_from_abs_path(&canonical).map_err(|_| {
                        SassError::Script {
                            message: format!("Invalid path for URL: {canonical}"),
                            argument_name: None,
                        }
                    })?;
                    return Ok(Some(file_url));
                } else {
                    let label = subpath.as_deref().unwrap_or("root");
                    return Err(Box::new(SassError::Script {
                        message: format!(
                            "The export for '{}' in '{}' resolved to '{}', which is not \
                             a '.scss', '.sass', or '.css' file.",
                            label, package_name, resolved
                        ),
                        argument_name: None,
                    }));
                }
            }
        }

        if subpath.is_none() {
            if let Some(root_path) = self
                .resolve_package_root_values(&package_root, &package_manifest, ctx.from_import)
                .await?
            {
                let canonical = canonicalize_lexical(&*self.cwd.io, &root_path);
                let file_url =
                    SassUrl::file_url_from_abs_path(&canonical).map_err(|_| SassError::Script {
                        message: format!("Invalid path for URL: {canonical}"),
                        argument_name: None,
                    })?;
                return Ok(Some(file_url));
            }
            return Ok(None);
        }

        let subpath = subpath.unwrap();
        let subpath_in_root = Path::new(&package_root).join(&subpath);
        let subpath_url = SassUrl::file_url_from_abs_path(&subpath_in_root.to_string_lossy())
            .map_err(|_| SassError::Script {
                message: format!("Invalid subpath URL: {}", subpath_in_root.display()),
                argument_name: None,
            })?;
        self.cwd.canonicalize(&subpath_url, ctx, warn_logger).await
    }

    /// Loads a previously canonicalized URL from disk.
    ///
    /// Matches Dart: `NodePackageImporter.load`.
    #[rust_sass_macros::maybe_async]
    pub async fn load(&self, url: &SassUrl) -> SassResult<Option<ImporterResult>> {
        self.cwd.load(url).await
    }

    /// Always `true`: any URL could in principle resolve through `node_modules`.
    pub fn could_canonicalize(&self, _url: &SassUrl, _canonical_url: &SassUrl) -> bool {
        true
    }

    /// `pkg` is this importer's non-canonical scheme: it is never returned
    /// from `canonicalize`, but absolute `pkg:` URLs see the containing URL.
    ///
    /// Matches Dart: `NodePackageImporter.isNonCanonicalScheme`.
    pub fn is_non_canonical_scheme(&self, scheme: &str) -> bool {
        scheme == "pkg"
    }

    pub fn modification_time(&self, _url: &SassUrl) -> SassResult<SassTime> {
        Ok(SassTime::now())
    }
}

impl NodePackageImporter {
    fn url_extension(path: &str) -> Option<&str> {
        path.rfind('/')
            .map(|i| &path[i + 1..])
            .unwrap_or(path)
            .rfind('.')
            .map(|i| &path[path.rfind('/').map(|j| j + 1).unwrap_or(0) + i..])
    }

    /// Splits a bare import specifier into its package name and subpath.
    ///
    /// Always splits on `/` (this is a specifier, not a native path).
    /// Matches Dart: `NodePackageImporter._packageNameAndSubpath`.
    fn package_name_and_subpath(specifier: &str) -> (String, Option<String>) {
        let mut parts: Vec<&str> = specifier.split('/').collect();
        let mut name = if parts.is_empty() {
            String::new()
        } else {
            parts.remove(0).to_string()
        };

        if name.starts_with('@') && !parts.is_empty() {
            name = format!("{}/{}", name, parts.remove(0));
        }

        let subpath = if parts.is_empty() {
            None
        } else {
            Some(parts.join("/"))
        };
        (name, subpath)
    }

    /// Returns the root directory of the most proximate installed
    /// `package_name` above `base_directory`.
    ///
    /// Implements `PACKAGE_RESOLVE` from the Node resolution-algorithm
    /// specification. Matches Dart:
    /// `NodePackageImporter._resolvePackageRoot`.
    #[rust_sass_macros::maybe_async]
    async fn resolve_package_root(
        &self,
        package_name: &str,
        base_directory: &str,
    ) -> Result<Option<String>, Box<SassError>> {
        let mut dir = base_directory.to_string();
        loop {
            let potential = Path::new(&dir).join("node_modules").join(package_name);
            if self
                .cwd
                .io
                .dir_exists(&potential)
                .await
                .map_err(io_err_to_sass)?
            {
                return Ok(Some(potential.to_string_lossy().into_owned()));
            }
            let parent = Path::new(&dir).parent();
            match parent {
                Some(p) => {
                    let parent_str = p.to_string_lossy().to_string();
                    if parent_str == dir || parent_str.is_empty() {
                        return Ok(None);
                    }
                    dir = parent_str;
                }
                None => return Ok(None),
            }
        }
    }

    /// Returns a file path from the `sass`/`style` manifest values, or an
    /// `index` file at the package root, resolved for extensions and partials.
    ///
    /// Matches Dart: `NodePackageImporter._resolvePackageRootValues`.
    #[rust_sass_macros::maybe_async]
    async fn resolve_package_root_values(
        &self,
        package_root: &str,
        package_manifest: &serde_json::Value,
        from_import: bool,
    ) -> Result<Option<String>, Box<SassError>> {
        if let Some(Value::String(sass_value)) = package_manifest.get("sass") {
            let ext = Self::url_extension(sass_value).unwrap_or("");
            if VALID_EXTENSIONS.contains(&ext) {
                return Ok(Some(
                    self.resolve_import_only(
                        &Path::new(package_root).join(sass_value).to_string_lossy(),
                        from_import,
                    )
                    .await?,
                ));
            }
        }
        if let Some(Value::String(style_value)) = package_manifest.get("style") {
            let ext = Self::url_extension(style_value).unwrap_or("");
            if VALID_EXTENSIONS.contains(&ext) {
                return Ok(Some(
                    self.resolve_import_only(
                        &Path::new(package_root).join(style_value).to_string_lossy(),
                        from_import,
                    )
                    .await?,
                ));
            }
        }

        let index_path = Path::new(package_root).join("index");
        resolve_import_path(&*self.cwd.io, &index_path.to_string_lossy(), from_import).await
    }

    /// Returns either `path` or, if necessary, the import-only variant that
    /// should be loaded instead: for `@import` (`from_import`), a sibling
    /// `<name>.import<ext>` file takes precedence when it exists.
    ///
    /// Matches Dart: `NodePackageImporter._resolveImportOnly`.
    #[rust_sass_macros::maybe_async]
    async fn resolve_import_only(
        &self,
        path: &str,
        from_import: bool,
    ) -> Result<String, Box<SassError>> {
        if !from_import {
            return Ok(path.to_string());
        }
        let ext = Self::url_extension(path).unwrap_or("");
        debug_assert!(
            VALID_EXTENSIONS.contains(&ext),
            "resolve_import_only called with extension {ext:?} in {path:?}"
        );
        let stem = &path[..path.len() - ext.len()];
        let import_only = format!("{stem}.import{ext}");
        let exists = self
            .cwd
            .io
            .file_exists(Path::new(&import_only))
            .await
            .map_err(io_err_to_sass)?;
        Ok(if exists {
            import_only
        } else {
            path.to_string()
        })
    }

    /// Returns a file path for `subpath` from the manifest's `exports` section.
    ///
    /// Tries extension/partial variants first, then (for subpaths with an
    /// extension) gives up, otherwise retries with `/index` appended.
    /// Matches Dart: `NodePackageImporter._resolvePackageExports`.
    #[rust_sass_macros::maybe_async]
    async fn resolve_package_exports(
        &self,
        package_root: &str,
        subpath: Option<&str>,
        package_manifest: &serde_json::Value,
        package_name: &str,
    ) -> Result<Option<String>, Box<SassError>> {
        let exports = package_manifest.get("exports");
        let exports = match exports {
            Some(e) => e,
            None => return Ok(None),
        };

        let subpath_variants = Self::exports_to_check(subpath, false);
        if let Some(path) = self
            .node_package_exports_resolve(
                package_root,
                &subpath_variants,
                exports,
                subpath,
                package_name,
            )
            .await?
        {
            return Ok(Some(path));
        }

        let has_extension = subpath
            .map(|s| s.rfind('/').map(|i| &s[i + 1..]).unwrap_or(s).contains('.'))
            .unwrap_or(false);
        if subpath.is_some() && has_extension {
            return Ok(None);
        }

        let subpath_index_variants = Self::exports_to_check(subpath, true);
        self.node_package_exports_resolve(
            package_root,
            &subpath_index_variants,
            exports,
            subpath,
            package_name,
        )
        .await
    }

    /// Resolves one subpath variant list against the manifest's `exports`.
    ///
    /// Throws when several variants match, returns `None` when none match.
    /// Implements `PACKAGE_EXPORTS_RESOLVE` from the Node
    /// resolution-algorithm specification. Matches Dart:
    /// `NodePackageImporter._nodePackageExportsResolve`.
    #[rust_sass_macros::maybe_async]
    async fn node_package_exports_resolve(
        &self,
        package_root: &str,
        subpath_variants: &[Option<String>],
        exports: &serde_json::Value,
        subpath: Option<&str>,
        package_name: &str,
    ) -> Result<Option<String>, Box<SassError>> {
        if let Value::Object(map) = exports {
            let has_dot_keys = map.keys().any(|k| k.starts_with('.'));
            let has_non_dot_keys = map.keys().any(|k| !k.starts_with('.'));
            if has_dot_keys && has_non_dot_keys {
                let keys: Vec<String> = map.keys().map(|k| format!("\"{}\"", k)).collect();
                return Err(Box::new(SassError::Script {
                    message: format!(
                        "`exports` in {} can not have both conditions and paths \
                         at the same level.\nFound {} in {}.",
                        package_name,
                        keys.join(","),
                        Path::new(package_root).join("package.json").display()
                    ),
                    argument_name: None,
                }));
            }
        }

        let mut matches: Vec<String> = Vec::new();
        for variant in subpath_variants {
            let result = match variant {
                None => {
                    let main_export = Self::get_main_export(exports);
                    match main_export {
                        Some(me) => {
                            self.package_target_resolve(variant.as_deref(), me, package_root, None)
                                .await?
                        }
                        None => None,
                    }
                }
                Some(v) => match exports {
                    Value::Object(map) if map.keys().any(|k| k.starts_with('.')) => {
                        let match_key = format!("./{}", v);
                        if map.contains_key(&match_key)
                            && !map[&match_key].is_null()
                            && !match_key.contains('*')
                        {
                            self.package_target_resolve(
                                Some(v),
                                &map[&match_key],
                                package_root,
                                None,
                            )
                            .await?
                        } else {
                            let mut expansion_keys: Vec<&String> =
                                map.keys().filter(|k| k.matches('*').count() == 1).collect();
                            expansion_keys.sort_by(|a, b| Self::compare_expansion_keys(a, b));

                            let mut found = None;
                            for expansion_key in expansion_keys {
                                let parts: Vec<&str> = expansion_key.split('*').collect();
                                let pattern_base = parts[0];
                                let pattern_trailer = if parts.len() > 1 { parts[1] } else { "" };

                                if !match_key.starts_with(pattern_base) {
                                    continue;
                                }
                                if match_key == pattern_base {
                                    continue;
                                }
                                if pattern_trailer.is_empty()
                                    || (match_key.ends_with(pattern_trailer)
                                        && match_key.len() >= expansion_key.len())
                                {
                                    let target = map.get(expansion_key);
                                    if let Some(t) = target {
                                        if !t.is_null() {
                                            let pattern_match = &match_key[pattern_base.len()
                                                ..match_key.len() - pattern_trailer.len()];
                                            if let Some(resolved) = self
                                                .package_target_resolve(
                                                    Some(v),
                                                    t,
                                                    package_root,
                                                    Some(pattern_match),
                                                )
                                                .await?
                                            {
                                                found = Some(resolved);
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            found
                        }
                    }
                    _ => None,
                },
            };
            if let Some(r) = result {
                matches.push(r);
            }
        }

        match matches.len() {
            0 => Ok(None),
            1 => Ok(Some(matches.into_iter().next().unwrap())),
            _ => {
                let label = subpath.unwrap_or("root");
                Err(Box::new(SassError::Script {
                    message: format!(
                        "Unable to determine which of multiple potential resolutions \
                         found for {} in {} should be used. \n\nFound:\n{}",
                        label,
                        package_name,
                        matches.join("\n")
                    ),
                    argument_name: None,
                }))
            }
        }
    }

    fn compare_expansion_keys(key_a: &str, key_b: &str) -> Ordering {
        let base_len_a = if key_a.contains('*') {
            key_a.find('*').unwrap() + 1
        } else {
            key_a.len()
        };
        let base_len_b = if key_b.contains('*') {
            key_b.find('*').unwrap() + 1
        } else {
            key_b.len()
        };
        if base_len_a > base_len_b {
            return Ordering::Less;
        }
        if base_len_b > base_len_a {
            return Ordering::Greater;
        }
        if !key_a.contains('*') {
            return Ordering::Greater;
        }
        if !key_b.contains('*') {
            return Ordering::Less;
        }
        if key_a.len() > key_b.len() {
            return Ordering::Less;
        }
        if key_b.len() > key_a.len() {
            return Ordering::Greater;
        }
        Ordering::Equal
    }

    #[rust_sass_macros::maybe_async]
    // `subpath` threads unchanged through the recursion (mirrors Dart's
    // `packageTargetResolve` signature); only the recursive calls observe it.
    #[allow(clippy::only_used_in_recursion)]
    fn package_target_resolve<'a>(
        &'a self,
        subpath: Option<&'a str>,
        exports: &'a serde_json::Value,
        package_root: &'a str,
        pattern_match: Option<&'a str>,
    ) -> LocalBoxFuture<'a, Result<Option<String>, Box<SassError>>> {
        Box::pin(async move {
            Ok(match exports {
                // Dart throws on a non-relative export target
                // (node_package.dart:319-320) — swallowing it as `None` lets
                // later importers silently mis-resolve.
                Value::String(s) if !s.starts_with("./") => {
                    return Err(Box::new(SassError::Script {
                        message: format!(
                            "Export '{s}' must be a path relative to the package root \
                             at '{package_root}'."
                        ),
                        argument_name: None,
                    }));
                }
                Value::String(s) if pattern_match.is_some() => {
                    // Dart `replaceFirst` (node_package.dart:322) — one `*`
                    // only, not replace-all.
                    let replaced = s.replacen('*', pattern_match.unwrap(), 1);
                    let path = Path::new(package_root).join(&replaced);
                    if self
                        .cwd
                        .io
                        .file_exists(&path)
                        .await
                        .map_err(io_err_to_sass)?
                    {
                        Some(path.to_string_lossy().into_owned())
                    } else {
                        None
                    }
                }
                Value::String(s) => Some(
                    Path::new(package_root)
                        .join(s)
                        .to_string_lossy()
                        .into_owned(),
                ),
                Value::Object(map) => {
                    // Dart iterates the map's INSERTION order
                    // (node_package.dart:328-329), not a fixed
                    // sass→style→default order.
                    for (key, value) in map.iter() {
                        if !["sass", "style", "default"].contains(&key.as_str()) {
                            continue;
                        }
                        if value.is_null() {
                            continue;
                        }
                        if let Some(result) = self
                            .package_target_resolve(subpath, value, package_root, pattern_match)
                            .await?
                        {
                            return Ok(Some(result));
                        }
                    }
                    None
                }
                Value::Array(arr) if arr.is_empty() => None,
                Value::Array(arr) => {
                    for value in arr {
                        if value.is_null() {
                            continue;
                        }
                        if let Some(result) = self
                            .package_target_resolve(subpath, value, package_root, pattern_match)
                            .await?
                        {
                            return Ok(Some(result));
                        }
                    }
                    None
                }
                // Dart throws on an invalid exports value
                // (node_package.dart:363-364).
                other => {
                    return Err(Box::new(SassError::Script {
                        message: format!(
                            "Invalid 'exports' value {other} in {}.",
                            Path::new(package_root).join("package.json").display()
                        ),
                        argument_name: None,
                    }));
                }
            })
        })
    }

    fn get_main_export(exports: &serde_json::Value) -> Option<&serde_json::Value> {
        match exports {
            Value::String(_) => Some(exports),
            Value::Array(arr) if arr.iter().all(|v| v.is_string()) => Some(exports),
            Value::Object(map) if !map.keys().any(|k| k.starts_with('.')) => Some(exports),
            Value::Object(map) => map.get(".").filter(|v| !v.is_null()),
            _ => None,
        }
    }

    fn exports_to_check(subpath: Option<&str>, add_index: bool) -> Vec<Option<String>> {
        let mut subpath = subpath.map(|s| s.to_string());

        if subpath.is_none() && add_index {
            subpath = Some("index".to_string());
        } else if let Some(ref s) = subpath {
            if add_index {
                subpath = Some(format!("{}/index", s));
            }
        }

        let subpath = match subpath {
            Some(s) => s,
            None => return vec![None],
        };

        let mut paths = Vec::new();
        let ext = Self::url_extension(&subpath).unwrap_or("");
        if VALID_EXTENSIONS.contains(&ext) {
            paths.push(subpath.clone());
        } else {
            paths.push(subpath.clone());
            paths.push(format!("{}.scss", subpath));
            paths.push(format!("{}.sass", subpath));
            paths.push(format!("{}.css", subpath));
        }

        let basename_start = subpath.rfind('/').map(|i| i + 1).unwrap_or(0);
        let baseline = &subpath[basename_start..];
        if baseline.starts_with('_') {
            return paths.into_iter().map(Some).collect();
        }

        let dirname = if basename_start > 0 {
            &subpath[..basename_start]
        } else {
            ""
        };

        let mut result: Vec<Option<String>> = paths.iter().map(|p| Some(p.clone())).collect();
        for path in &paths {
            let path_baseline = &path[path.rfind('/').map(|i| i + 1).unwrap_or(0)..];
            if dirname.is_empty() {
                result.push(Some(format!("_{}", path_baseline)));
            } else {
                result.push(Some(format!("{}_", dirname) + path_baseline));
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::VirtualIo;
    use crate::logger::NoOpWarnLogger;
    use bumpalo::Bump;
    use std::collections::HashMap;

    fn make_io(files: HashMap<String, String>) -> Rc<dyn Io> {
        Rc::new(VirtualIo::with_files(files))
    }

    fn make_ctx() -> CanonicalizeContext {
        CanonicalizeContext::new(None, false)
    }

    fn make_import_ctx() -> CanonicalizeContext {
        CanonicalizeContext::new(None, true)
    }

    // Dart throws on a non-`./` export target (node_package.dart:319-320)
    // instead of swallowing it as `None`. Layout: `/app/node_modules/p/…`
    // with entry `/app` (base dir) so `resolve_package_root` finds it.
    #[rust_sass_macros::maybe_test]
    async fn test_non_relative_export_throws() {
        let _arena = Bump::new();
        let mut files = HashMap::new();
        files.insert(
            "/app/node_modules/p/package.json".to_string(),
            r#"{"name": "p", "exports": {"./a": "a.scss"}}"#.to_string(),
        );
        files.insert(
            "/app/node_modules/p/a.scss".to_string(),
            "a { b: 1; }".to_string(),
        );
        let io = make_io(files);
        let imp = NodePackageImporter::new("/app", io);
        let url = SassUrl::parse("pkg:p/a").unwrap();
        let mut ctx = make_ctx();
        let err = imp
            .canonicalize(&url, &mut ctx, &NoOpWarnLogger)
            .await
            .unwrap_err();
        match *err {
            SassError::Script { message, .. } => assert!(
                message.contains("must be a path relative to the package root"),
                "got: {message}"
            ),
            other => panic!("expected Script error, got {other:?}"),
        }
    }

    // Dart iterates the condition map in INSERTION order
    // (node_package.dart:328-329): `{"default": ..., "sass": ...}` resolves
    // `default` first even though `sass` is listed first in the fixed order.
    #[rust_sass_macros::maybe_test]
    async fn test_condition_insertion_order() {
        let arena = Bump::new();
        let mut files = HashMap::new();
        files.insert(
            "/app/node_modules/p/package.json".to_string(),
            r#"{"name": "p", "exports": {"./a": {"default": "./d.scss", "sass": "./s.scss"}}}"#
                .to_string(),
        );
        files.insert(
            "/app/node_modules/p/d.scss".to_string(),
            "a { b: 1; }".to_string(),
        );
        files.insert(
            "/app/node_modules/p/s.scss".to_string(),
            "a { b: 2; }".to_string(),
        );
        let io = make_io(files);
        let imp = NodePackageImporter::new("/app", io);
        let url = SassUrl::parse("pkg:p/a").unwrap();
        let mut ctx = make_ctx();
        let result = imp
            .canonicalize(&url, &mut ctx, &NoOpWarnLogger)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.to_string(), "file:///app/node_modules/p/d.scss");
        let _ = &arena;
    }

    // Dart throws on an invalid exports value (node_package.dart:363-364).
    #[rust_sass_macros::maybe_test]
    async fn test_invalid_exports_throws() {
        let arena = Bump::new();
        let mut files = HashMap::new();
        files.insert(
            "/app/node_modules/p/package.json".to_string(),
            r#"{"name": "p", "exports": {"./a": 42}}"#.to_string(),
        );
        let io = make_io(files);
        let imp = NodePackageImporter::new("/app", io);
        let url = SassUrl::parse("pkg:p/a").unwrap();
        let mut ctx = make_ctx();
        let err = imp
            .canonicalize(&url, &mut ctx, &NoOpWarnLogger)
            .await
            .unwrap_err();
        match *err {
            SassError::Script { message, .. } => {
                assert!(
                    message.contains("Invalid 'exports' value"),
                    "got: {message}"
                )
            }
            other => panic!("expected Script error, got {other:?}"),
        }
        let _ = &arena;
    }

    // Dart loads the import-only variant through `exports` for `@import`
    // but not for `@use` (node_package.dart `_resolveImportOnly`, #2772).
    // Layout: `/app/node_modules/p/…` with entry `/app`.
    #[rust_sass_macros::maybe_test]
    async fn test_exports_import_only_for_import() {
        let _arena = Bump::new();
        let mut files = HashMap::new();
        files.insert(
            "/app/node_modules/p/package.json".to_string(),
            r#"{"name": "p", "exports": {".": "./main.scss"}}"#.to_string(),
        );
        files.insert(
            "/app/node_modules/p/main.scss".to_string(),
            "a { b: 1; }".to_string(),
        );
        files.insert(
            "/app/node_modules/p/main.import.scss".to_string(),
            "a { b: 2; }".to_string(),
        );
        let io = make_io(files);
        let imp = NodePackageImporter::new("/app", io);
        let url = SassUrl::parse("pkg:p").unwrap();

        let mut import_ctx = make_import_ctx();
        let result = imp
            .canonicalize(&url, &mut import_ctx, &NoOpWarnLogger)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.to_string(),
            "file:///app/node_modules/p/main.import.scss"
        );

        let mut use_ctx = make_ctx();
        let result = imp
            .canonicalize(&url, &mut use_ctx, &NoOpWarnLogger)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.to_string(), "file:///app/node_modules/p/main.scss");
    }

    // Without an import-only sibling, `@import` falls back to the regular
    // file (node_package.dart `_resolveImportOnly`, #2772).
    #[rust_sass_macros::maybe_test]
    async fn test_exports_missing_import_only_falls_back() {
        let _arena = Bump::new();
        let mut files = HashMap::new();
        files.insert(
            "/app/node_modules/p/package.json".to_string(),
            r#"{"name": "p", "exports": {".": "./main.scss"}}"#.to_string(),
        );
        files.insert(
            "/app/node_modules/p/main.scss".to_string(),
            "a { b: 1; }".to_string(),
        );
        let io = make_io(files);
        let imp = NodePackageImporter::new("/app", io);
        let url = SassUrl::parse("pkg:p").unwrap();
        let mut ctx = make_import_ctx();
        let result = imp
            .canonicalize(&url, &mut ctx, &NoOpWarnLogger)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.to_string(), "file:///app/node_modules/p/main.scss");
    }

    // The `index` fallback also prefers the import-only variant for
    // `@import`: `resolveImportPath` tries `<name>.import<ext>` first in an
    // import context (importer/utils.dart `_ifInImport`, #2772). Empty
    // manifest, so neither `exports` nor `sass`/`style` applies.
    #[rust_sass_macros::maybe_test]
    async fn test_index_fallback_import_only_for_import() {
        let _arena = Bump::new();
        for (index, import_only) in [
            ("index.scss", "index.import.scss"),
            ("_index.scss", "_index.import.scss"),
        ] {
            let mut files = HashMap::new();
            files.insert(
                "/app/node_modules/p/package.json".to_string(),
                r#"{"name": "p"}"#.to_string(),
            );
            files.insert(
                format!("/app/node_modules/p/{index}"),
                "a { b: 1; }".to_string(),
            );
            files.insert(
                format!("/app/node_modules/p/{import_only}"),
                "a { b: 2; }".to_string(),
            );
            let io = make_io(files);
            let imp = NodePackageImporter::new("/app", io);
            let url = SassUrl::parse("pkg:p").unwrap();

            let mut import_ctx = make_import_ctx();
            let result = imp
                .canonicalize(&url, &mut import_ctx, &NoOpWarnLogger)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                result.to_string(),
                format!("file:///app/node_modules/p/{import_only}"),
                "for {index}"
            );

            let mut use_ctx = make_ctx();
            let result = imp
                .canonicalize(&url, &mut use_ctx, &NoOpWarnLogger)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                result.to_string(),
                format!("file:///app/node_modules/p/{index}"),
                "for {index}"
            );
        }
    }

    // Dart loads the import-only variant through the `sass` manifest key for
    // `@import` but not for `@use` (node_package.dart:150, #2772).
    #[rust_sass_macros::maybe_test]
    async fn test_sass_key_import_only_for_import() {
        let _arena = Bump::new();
        let mut files = HashMap::new();
        files.insert(
            "/app/node_modules/p/package.json".to_string(),
            r#"{"name": "p", "sass": "./via-sass.scss"}"#.to_string(),
        );
        files.insert(
            "/app/node_modules/p/via-sass.scss".to_string(),
            "a { b: 1; }".to_string(),
        );
        files.insert(
            "/app/node_modules/p/via-sass.import.scss".to_string(),
            "a { b: 2; }".to_string(),
        );
        let io = make_io(files);
        let imp = NodePackageImporter::new("/app", io);
        let url = SassUrl::parse("pkg:p").unwrap();

        let mut import_ctx = make_import_ctx();
        let result = imp
            .canonicalize(&url, &mut import_ctx, &NoOpWarnLogger)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.to_string(),
            "file:///app/node_modules/p/via-sass.import.scss"
        );

        let mut use_ctx = make_ctx();
        let result = imp
            .canonicalize(&url, &mut use_ctx, &NoOpWarnLogger)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.to_string(),
            "file:///app/node_modules/p/via-sass.scss"
        );
    }
}
