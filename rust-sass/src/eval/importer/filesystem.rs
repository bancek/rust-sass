// Copyright 2017 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/importer/filesystem.dart
// go-source: go/eval/import_cache.go (FilesystemImporter section)

use crate::deprecation::FS_IMPORTER_CWD;
use crate::url::SassUrl;
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::common::exception::{SassError, SassResult};
use crate::eval::importer::resolve_import_path::resolve_import_path;
use crate::eval::importer::utils::syntax_for_path;
use crate::eval::importer::{CanonicalizeContext, ImporterResult};
use crate::io::Io;
use crate::logger::WarnLogger;

/// An importer that loads files from a load path on the filesystem, either
/// relative to the path passed to [`FilesystemImporter::new`] or absolute
/// `file:` URLs.
///
/// Rewritten from Dart's `FilesystemImporter` (`importer/filesystem.dart`).
/// Use [`FilesystemImporter::new_no_load_path`] to only load absolute `file:`
/// URLs and URLs relative to the current file.
#[derive(Clone)]
pub struct FilesystemImporter {
    pub load_path: Option<String>,
    pub load_path_deprecated: bool,
    pub io: Rc<dyn Io>,
}

impl fmt::Debug for FilesystemImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FilesystemImporter")
            .field("load_path", &self.load_path)
            .field("load_path_deprecated", &self.load_path_deprecated)
            .field("io", &"...")
            .finish()
    }
}

impl FilesystemImporter {
    /// Creates an importer that loads files relative to `load_path`.
    ///
    /// Matches Dart: `FilesystemImporter(loadPath)`.
    pub fn new(load_path: &str, io: Rc<dyn Io>) -> Self {
        let abs = PathBuf::from(io.current_dir()).join(load_path);
        FilesystemImporter {
            load_path: Some(abs.to_string_lossy().into_owned()),
            load_path_deprecated: false,
            io,
        }
    }

    /// Creates an importer that loads files relative to the current working
    /// directory, warning (deprecated) when the load path is actually used.
    ///
    /// Rewritten from Dart's `FilesystemImporter.cwd` (deprecated): prefer
    /// [`FilesystemImporter::new_no_load_path`] when the load path does not
    /// matter, or `new(".")` to preserve the old behavior explicitly.
    pub fn new_cwd(io: Rc<dyn Io>) -> Self {
        let abs = io.current_dir();
        FilesystemImporter {
            load_path: Some(abs),
            load_path_deprecated: true,
            io,
        }
    }

    /// Creates an importer that only loads absolute `file:` URLs and URLs
    /// relative to the current file.
    ///
    /// Matches Dart: `FilesystemImporter.noLoadPath`.
    pub fn new_no_load_path(io: Rc<dyn Io>) -> Self {
        FilesystemImporter {
            load_path: None,
            load_path_deprecated: false,
            io,
        }
    }

    fn basename_of_url_path(url_path: &str) -> &str {
        match url_path.rfind('/') {
            Some(idx) => &url_path[idx + 1..],
            None => url_path,
        }
    }

    fn without_extension(basename: &str) -> &str {
        match basename.rfind('.') {
            Some(idx) => &basename[..idx],
            None => basename,
        }
    }

    pub(crate) fn could_canonicalize_path(&self, url_path: &str, canonical_url: &SassUrl) -> bool {
        if !canonical_url.is_file() {
            return false;
        }

        let basename = Self::basename_of_url_path(url_path);
        let mut canonical_basename = Self::basename_of_url_path(canonical_url.path());

        if !basename.starts_with('_') && canonical_basename.starts_with('_') {
            canonical_basename = &canonical_basename[1..];
        }

        basename == canonical_basename || basename == Self::without_extension(canonical_basename)
    }

    /// Canonicalizes `url`: `file:` URLs resolve directly via
    /// [`resolve_import_path`](super::resolve_import_path::resolve_import_path);
    /// scheme-less URLs resolve against the load path (emitting the
    /// current-working-directory deprecation when the implicit CWD load path
    /// applies); anything else returns `None`. A hit is canonicalized to an
    /// absolute `file:` URL.
    ///
    /// Matches Dart: `FilesystemImporter.canonicalize`.
    #[rust_sass_macros::maybe_async]
    pub async fn canonicalize<'parse>(
        &self,
        url: &SassUrl,
        ctx: &mut CanonicalizeContext,
        warn_logger: &dyn WarnLogger<'parse>,
    ) -> SassResult<Option<SassUrl>> {
        let resolved = if url.is_file() {
            // `path` stays in URL space for the wrapped-relative tail
            // computation below; `fs_path` is the filesystem form for the
            // `Io` layer (`/C:/…` URL paths are not valid Windows paths).
            let path = url.path();
            let fs_path = url.fs_path();
            let r = resolve_import_path(&*self.io, &fs_path, ctx.from_import).await?;
            if r.is_some() {
                r
            } else if let Some(ref load_path) = self.load_path {
                // Relative URLs arrive here either as `sass-relative:` (kept
                // relative by the import cache) or wrapped as absolute `file:`
                // URLs (resolved against a `file:` base by `resolve_file_path`).
                // The fallback retries the wrapped-relative case under the load
                // path — but a genuine absolute `file:` URL must resolve
                // directly only (filesystem.dart:68-90). Distinguisher: the
                // import cache resolves `sass-relative:` URLs against a `file:`
                // base via `resolve_file_path` (path-based join), then marks the
                // result wrapped-relative so this fallback applies; genuine
                // absolute `file:` URLs arrive unmarked and skip it.
                // (Marker plumbing: `SassUrl::mark_wrapped_relative` /
                // `is_wrapped_relative` in url.rs — added with this fix.)
                if !url.is_wrapped_relative() {
                    // Genuine absolute `file:` URL: resolve directly only.
                    r
                } else {
                    // Wrapped-relative fallback under the load path.
                    let base_dir = ctx
                        .containing_url_without_marking()
                        .filter(|b| b.is_file())
                        .and_then(|b| b.path().rfind('/').map(|i| b.path()[..i + 1].to_string()));
                    let tail = match base_dir {
                        Some(dir) if path.starts_with(dir.as_str()) => &path[dir.len()..],
                        _ => path.strip_prefix('/').unwrap_or(path),
                    };
                    let joined = Path::new(load_path).join(tail);
                    resolve_import_path(&*self.io, &joined.to_string_lossy(), ctx.from_import)
                        .await?
                }
            } else {
                r
            }
        } else if url.is_relative() {
            // Scheme-less relative URL. Dart resolves it against the load path
            // (`p.join(loadPath, p.fromUri(url))`); an importer without a load
            // path (noLoadPath) only loads `file:` URLs and URLs relative to the
            // current file, so it must NOT fall back to the current directory.
            if let Some(ref load_path) = self.load_path {
                let joined = Path::new(load_path).join(url.path());
                let path_str = joined.to_string_lossy().to_string();
                let resolved = resolve_import_path(&*self.io, &path_str, ctx.from_import).await?;
                if resolved.is_some() && self.load_path_deprecated {
                    warn_logger.warn_deprecation(
                        "Using the current working directory as an implicit load path is \
                         deprecated. Either add it as an explicit load path or importer, or \
                         load this stylesheet from a different URL.",
                        &FS_IMPORTER_CWD,
                        None,
                    );
                }
                resolved
            } else {
                None
            }
        } else if url.scheme() != "" {
            return Ok(None);
        } else if let Some(ref load_path) = self.load_path {
            let joined = Path::new(load_path).join(url.path());
            let path_str = joined.to_string_lossy().to_string();
            let resolved = resolve_import_path(&*self.io, &path_str, ctx.from_import).await?;
            if resolved.is_some() && self.load_path_deprecated {
                warn_logger.warn_deprecation(
                    "Using the current working directory as an implicit load path is \
                     deprecated. Either add it as an explicit load path or importer, or \
                     load this stylesheet from a different URL.",
                    &FS_IMPORTER_CWD,
                    None,
                );
            }
            resolved
        } else {
            return Ok(None);
        };

        match resolved {
            None => Ok(None),
            Some(resolved_path) => {
                let canonical = self
                    .io
                    .canonicalize(Path::new(&resolved_path))
                    .await
                    .map_err(|e| SassError::Script {
                        message: e.to_string(),
                        argument_name: None,
                    })?;
                let file_url =
                    SassUrl::file_url_from_abs_path(&canonical).map_err(|_| SassError::Script {
                        message: format!("Invalid canonical path for URL: {canonical}"),
                        argument_name: None,
                    })?;
                Ok(Some(file_url))
            }
        }
    }

    /// Loads the file at `url`, inferring the parse syntax from its path.
    ///
    /// Matches Dart: `FilesystemImporter.load`.
    #[rust_sass_macros::maybe_async]
    pub async fn load(&self, url: &SassUrl) -> SassResult<Option<ImporterResult>> {
        let path_str = if url.is_file_like() {
            url.fs_path()
        } else {
            return Err(Box::new(SassError::Script {
                message: format!("Cannot load non-file URL: {}", url),
                argument_name: None,
            }));
        };
        let path = Path::new(&path_str);
        let contents = self
            .io
            .read_file(path)
            .await
            .map_err(|e| SassError::Script {
                message: e.to_string(),
                argument_name: None,
            })?;
        let contents_str = String::from_utf8_lossy(&contents).into_owned();
        let syntax = syntax_for_path(path);
        Ok(Some(ImporterResult::new(
            contents_str,
            syntax,
            Some(url.clone()),
        )?))
    }

    /// Without touching the filesystem, returns whether `url` could resolve to
    /// `canonical_url`: both must be file-like, and basenames must match up to
    /// the `_` partial prefix and the file extension.
    ///
    /// Matches Dart: `FilesystemImporter.couldCanonicalize`.
    pub fn could_canonicalize(&self, url: &SassUrl, canonical_url: &SassUrl) -> bool {
        if !url.is_file_like() && url.scheme() != "" {
            return false;
        }
        self.could_canonicalize_path(url.path(), canonical_url)
    }

    pub fn is_non_canonical_scheme(&self, _scheme: &str) -> bool {
        false
    }
}

impl fmt::Display for FilesystemImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.load_path {
            Some(ref lp) => write!(f, "{}", lp),
            None => write!(f, "<absolute file importer>"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::DefaultIo;
    use crate::io::VirtualIo;
    use crate::logger::NoOpWarnLogger;
    use crate::url::resolve_file_path;
    use bumpalo::Bump;
    use std::collections::HashMap;

    fn make_io() -> Rc<dyn Io> {
        Rc::new(DefaultIo::new())
    }

    #[test]
    fn new_constructor_sets_load_path() {
        let io = make_io();
        let imp = FilesystemImporter::new("/tmp/sass", io);
        assert!(imp.load_path.unwrap().ends_with("sass"));
        assert!(!imp.load_path_deprecated);
    }

    #[test]
    fn new_cwd_sets_deprecated() {
        let io = make_io();
        let imp = FilesystemImporter::new_cwd(io);
        assert!(imp.load_path_deprecated);
    }

    #[test]
    fn new_no_load_path_has_none() {
        let io = make_io();
        let imp = FilesystemImporter::new_no_load_path(io);
        assert!(imp.load_path.is_none());
        assert!(!imp.load_path_deprecated);
    }

    #[test]
    fn is_non_canonical_scheme_always_false() {
        let io = make_io();
        let imp = FilesystemImporter::new_no_load_path(io);
        assert!(!imp.is_non_canonical_scheme("file"));
        assert!(!imp.is_non_canonical_scheme("pkg"));
    }

    #[test]
    fn could_canonicalize_rejects_non_file_urls() {
        let io = make_io();
        let imp = FilesystemImporter::new_no_load_path(io);
        let url = SassUrl::parse("http://example.com/style.scss").unwrap();
        let canonical = SassUrl::parse("file:///style.scss").unwrap();
        assert!(!imp.could_canonicalize(&url, &canonical));
    }

    #[test]
    fn could_canonicalize_matches_basename() {
        let io = make_io();
        let imp = FilesystemImporter::new_no_load_path(io);
        let url = SassUrl::parse("file:///foo/bar/style.scss").unwrap();
        let canonical = SassUrl::parse("file:///baz/style.scss").unwrap();
        assert!(imp.could_canonicalize(&url, &canonical));
    }

    #[test]
    fn could_canonicalize_allows_partial_prefix() {
        let io = make_io();
        let imp = FilesystemImporter::new_no_load_path(io);
        let url = SassUrl::parse("file:///foo/style.scss").unwrap();
        let canonical = SassUrl::parse("file:///foo/_style.scss").unwrap();
        assert!(imp.could_canonicalize(&url, &canonical));
    }

    #[test]
    fn could_canonicalize_ignores_extension() {
        let io = make_io();
        let imp = FilesystemImporter::new_no_load_path(io);
        let url = SassUrl::parse("file:///foo/style").unwrap();
        let canonical = SassUrl::parse("file:///foo/style.scss").unwrap();
        assert!(imp.could_canonicalize(&url, &canonical));
    }

    // Dart resolves `file:` URLs directly only (filesystem.dart:68-90): a
    // genuine absolute `file:` URL never falls back to the load path, while a
    // wrapped-relative `file:` URL (resolved against a `file:` base by the
    // import cache) does.
    #[rust_sass_macros::maybe_test]
    async fn test_absolute_file_url_no_load_path_fallback() {
        let _arena = Bump::new();
        // `/lp/real.scss` exists ONLY under the load path; `/abs/real.scss`
        // exists ONLY at its absolute location. A relative `@use "real"`
        // from `/ctx/main.scss` wraps to `file:///ctx/real.scss` (missing
        // directly) and must fall back to the load path; a genuine absolute
        // `file:` URL resolves directly only.
        let mut files = HashMap::new();
        files.insert("/lp/real.scss".to_string(), "a { b: 1; }".to_string());
        files.insert("/abs/real.scss".to_string(), "a { b: 2; }".to_string());
        let io: Rc<dyn Io> = Rc::new(VirtualIo::with_files(files));
        // `FilesystemImporter::new` joins the load path onto the CWD
        // (`VirtualIo` CWD is `/`), so pass the absolute path through.
        let imp = FilesystemImporter::new("lp", io);
        let ctx_url = SassUrl::parse("file:///ctx/main.scss").unwrap();

        // Wrapped-relative `file:` URL (as the import cache produces for a
        // relative `@use` from a `file:` base): falls back to the load path.
        let wrapped = resolve_file_path(&SassUrl::parse("real").unwrap(), &ctx_url).unwrap();
        assert!(wrapped.is_wrapped_relative());
        // Sanity: the wrapped path misses directly (no /ctx/real.scss).
        assert_eq!(wrapped.to_string(), "file:///ctx/real");
        let mut ctx = CanonicalizeContext::new(Some(ctx_url.clone()), false);
        let result = imp
            .canonicalize(&wrapped, &mut ctx, &NoOpWarnLogger)
            .await
            .unwrap();
        assert_eq!(
            result.map(|u| u.to_string()).as_deref(),
            Some("file:///lp/real.scss"),
        );

        // Genuine absolute `file:` URL pointing at an existing file resolves
        // directly and never consults the load path — even though `/lp` also
        // contains a same-named file that would shadow under fallback.
        let abs_url = SassUrl::parse("file:///abs/real.scss").unwrap();
        assert!(!abs_url.is_wrapped_relative());
        let mut ctx = CanonicalizeContext::new(Some(ctx_url), false);
        let result = imp
            .canonicalize(&abs_url, &mut ctx, &NoOpWarnLogger)
            .await
            .unwrap();
        assert_eq!(
            result.map(|u| u.to_string()).as_deref(),
            Some("file:///abs/real.scss"),
        );
    }
}
