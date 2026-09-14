// Copyright 2017 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/importer/package.dart
// go-source: go/eval/importer_package.go

use crate::url::SassUrl;
use std::fmt;
use std::rc::Rc;

use crate::common::exception::{SassError, SassResult};
use crate::eval::importer::{CanonicalizeContext, FilesystemImporter, ImporterResult};
use crate::io::Io;
use crate::logger::WarnLogger;

/// Host hook that resolves `package:` URLs to `file:` URLs.
///
/// Rust counterpart of the `package_config` `PackageConfig.resolve` used by
/// Dart's `PackageImporter`.
pub trait PackageConfig: fmt::Debug {
    /// Resolves a `package:` URL, or `None` when the package is unknown.
    fn resolve(&self, url: &SassUrl) -> Option<SassUrl>;
}

/// An importer that loads stylesheets from `package:` imports.
///
/// Rewritten from Dart's `PackageImporter` (`importer/package.dart`):
/// `file:` URLs delegate to the working-directory filesystem importer,
/// `package:` URLs resolve through the config (`"Unknown package."` when
/// unresolvable, `"Unsupported URL …."` for non-file resolutions), and all
/// other schemes return `None`. Displays as `"package:..."`.
#[derive(Clone)]
pub struct PackageImporter {
    config: Rc<dyn PackageConfig>,
    cwd: FilesystemImporter,
}

impl fmt::Debug for PackageImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PackageImporter")
            .field("config", &self.config)
            .field("cwd", &self.cwd)
            .finish()
    }
}

impl PackageImporter {
    /// Creates an importer resolving `package:` URLs via `config`.
    ///
    /// Matches Dart: `PackageImporter(packageConfig)`.
    pub fn new(config: Rc<dyn PackageConfig>, io: Rc<dyn Io>) -> Self {
        PackageImporter {
            config,
            cwd: FilesystemImporter::new_cwd(io),
        }
    }

    /// Canonicalizes `url` as described on [`PackageImporter`].
    ///
    /// Matches Dart: `PackageImporter.canonicalize`.
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
        // Dart returns `None` for every non-`package` scheme INCLUDING
        // relative URLs (scheme `''`) — routing them into `config.resolve`
        // lets a resolving config shadow later importers (package.dart:27).
        if url.scheme() != "package" {
            return Ok(None);
        }

        let resolved = self.config.resolve(url);
        match resolved {
            None => Err(Box::new(SassError::Script {
                message: "Unknown package.".to_string(),
                argument_name: None,
            })),
            Some(ref u) if !u.is_file_like() => Err(Box::new(SassError::Script {
                message: format!("Unsupported URL {}.", u),
                argument_name: None,
            })),
            Some(u) => self.cwd.canonicalize(&u, ctx, warn_logger).await,
        }
    }

    /// Loads a previously canonicalized URL from disk.
    ///
    /// Matches Dart: `PackageImporter.load`.
    #[rust_sass_macros::maybe_async]
    pub async fn load(&self, url: &SassUrl) -> SassResult<Option<ImporterResult>> {
        self.cwd.load(url).await
    }

    /// Whether `url` could canonicalize to `canonical_url`, by scheme plus the
    /// filesystem basename check.
    ///
    /// Matches Dart: `PackageImporter.couldCanonicalize`.
    pub fn could_canonicalize(&self, url: &SassUrl, canonical_url: &SassUrl) -> bool {
        let scheme = url.scheme();
        if !url.is_file_like() && scheme != "package" && !scheme.is_empty() {
            return false;
        }
        self.cwd.could_canonicalize_path(url.path(), canonical_url)
    }

    pub fn is_non_canonical_scheme(&self, _scheme: &str) -> bool {
        false
    }
}

impl fmt::Display for PackageImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "package:...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::DefaultIo;
    use crate::logger::BufferedWarnLogger;
    use bumpalo::Bump;
    use std::collections::HashMap;

    fn make_io() -> Rc<dyn Io> {
        Rc::new(DefaultIo::new())
    }

    #[derive(Debug)]
    struct TestConfig {
        mapping: HashMap<String, String>,
    }

    impl PackageConfig for TestConfig {
        fn resolve(&self, url: &SassUrl) -> Option<SassUrl> {
            let package_name = url.path().to_string();
            self.mapping
                .get(&package_name)
                .map(|path| SassUrl::file_url_from_abs_path(path).unwrap())
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_non_package_returns_none() {
        let arena = Bump::new();
        let io = make_io();
        let config = TestConfig {
            mapping: Default::default(),
        };
        let imp = PackageImporter::new(Rc::new(config), io);
        let url = SassUrl::parse("http://example.com/style.scss").unwrap();
        let mut ctx = CanonicalizeContext::new(None, false);
        let result = imp
            .canonicalize(&url, &mut ctx, &BufferedWarnLogger::new(&arena))
            .await
            .unwrap();
        assert!(result.is_none());
    }

    // Dart returns `None` for relative URLs (package.dart:27) instead of
    // routing them into `config.resolve` where a resolving config would
    // shadow later importers.
    #[rust_sass_macros::maybe_test]
    async fn test_relative_url_returns_none() {
        let arena = Bump::new();
        let io = make_io();
        let mut mapping = HashMap::new();
        mapping.insert("foo".to_string(), "/resolved/foo.scss".to_string());
        let imp = PackageImporter::new(Rc::new(TestConfig { mapping }), io);
        let url = SassUrl::parse("foo").unwrap();
        assert!(url.is_relative());
        let mut ctx = CanonicalizeContext::new(None, false);
        let result = imp
            .canonicalize(&url, &mut ctx, &BufferedWarnLogger::new(&arena))
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_unknown_package_errors() {
        let arena = Bump::new();
        let io = make_io();
        let config = TestConfig {
            mapping: Default::default(),
        };
        let imp = PackageImporter::new(Rc::new(config), io);
        let url = SassUrl::parse("package:unknown").unwrap();
        let mut ctx = CanonicalizeContext::new(None, false);
        let err = imp
            .canonicalize(&url, &mut ctx, &BufferedWarnLogger::new(&arena))
            .await
            .unwrap_err();
        match *err {
            SassError::Script { message, .. } => {
                assert_eq!(message, "Unknown package.");
            }
            _ => panic!("unexpected error variant"),
        }
    }

    #[test]
    fn is_non_canonical_scheme_returns_false() {
        let io = make_io();
        let config = TestConfig {
            mapping: Default::default(),
        };
        let imp = PackageImporter::new(Rc::new(config), io);
        assert!(!imp.is_non_canonical_scheme("package"));
    }

    #[test]
    fn could_canonicalize_checks_scheme() {
        let io = make_io();
        let config = TestConfig {
            mapping: Default::default(),
        };
        let imp = PackageImporter::new(Rc::new(config), io);
        let url = SassUrl::parse("http://example.com/style").unwrap();
        let canonical = SassUrl::parse("file:///style.scss").unwrap();
        assert!(!imp.could_canonicalize(&url, &canonical));
    }
}
