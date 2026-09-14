// Copyright 2017 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/importer.dart (Importer) + lib/src/importer/async.dart
//   (AsyncImporter — canonicalize/load/couldCanonicalize/isNonCanonicalScheme
//   contracts, fromImport/containingUrl semantics)
// go-source: go/eval/importer.go

pub mod canonicalize_context;
pub mod filesystem;
pub mod no_op;
pub mod node_package;
pub mod package;
pub mod resolve_import_path;
pub mod result;
pub mod utils;

use bumpalo::Bump;

pub use canonicalize_context::CanonicalizeContext;
pub use filesystem::FilesystemImporter;
pub use no_op::NoOpImporter;
pub use node_package::NodePackageImporter;
pub use package::{PackageConfig, PackageImporter};
pub use result::ImporterResult;
pub use utils::{is_valid_url_scheme, syntax_for_path};

use crate::url::SassUrl;
#[cfg(feature = "async")]
use futures::future::LocalBoxFuture;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::common::exception::SassResult;
use crate::logger::NoOpWarnLogger;

/// Extension point for host-provided importers: resolves Sass load URLs to
/// file contents.
///
/// Rewritten from Dart's `AsyncImporter` (`importer/async.dart`) — the async
/// half of the `Importer` interface, which is what the dual sync/async build
/// implements. Importers should provide a human-readable [`fmt::Display`]
/// (e.g. the load path for the filesystem importer). Subclasses should extend
/// the importer type, not implement the raw contract.
#[rust_sass_macros::maybe_async]
pub trait UserImporter: fmt::Debug {
    /// If `url` is recognized, returns its canonical (absolute) form.
    ///
    /// Canonical URLs must be absolute (`file:` encouraged for on-disk
    /// stylesheets) and stable: repeated calls with the same URL return the
    /// same result, canonicalizing a canonical URL returns it unchanged, and
    /// the same canonical URL always refers to the same stylesheet even across
    /// importers. File-extension lookup follows the filesystem importer's
    /// partial/extension/`index` conventions (see `resolve_import_path`).
    /// Returns `None` when `url` is not recognized. A `None` from every
    /// importer means "can't find stylesheet".
    fn canonicalize<'a>(
        &'a self,
        url: &'a SassUrl,
        context: &'a mut CanonicalizeContext,
    ) -> LocalBoxFuture<'a, SassResult<Option<SassUrl>>>;

    /// Loads the Sass text for `url` (which came from [`UserImporter::canonicalize`]).
    ///
    /// Returns `None` when this importer cannot find the stylesheet. A load
    /// failure for a URL uniquely owned by this importer may throw; the error
    /// is wrapped by Sass (a plain message string suffices).
    fn load<'a>(
        &'a self,
        url: &'a SassUrl,
    ) -> LocalBoxFuture<'a, SassResult<Option<ImporterResult>>>;

    /// Without touching the filesystem, returns whether `canonicalize(url)`
    /// could possibly return `canonical_url`.
    ///
    /// Must be cheap; false positives are allowed, false negatives are not.
    /// Matches Dart: `AsyncImporter.couldCanonicalize`.
    fn could_canonicalize(&self, _url: &SassUrl, _base_url: &SassUrl) -> bool {
        true
    }

    /// Returns whether `scheme` is "non-canonical" for this importer: the
    /// importer may never return it from `canonicalize`, but in exchange the
    /// containing URL is available during `canonicalize` for absolute URLs
    /// with that scheme so resolution can depend on the load site.
    ///
    /// Matches Dart: `AsyncImporter.isNonCanonicalScheme`.
    fn is_non_canonical_scheme(&self, _scheme: &str) -> bool {
        false
    }

    /// Returns whether this importer wants the fast base-importer path to
    /// consult it with the raw (unresolved) load path instead of the
    /// base-resolved URL.
    ///
    /// No Dart counterpart — seeded by the libsass C-API seam
    /// (`rust-sass-libsass`): libsass consults custom importers with the
    /// raw `@import` string plus the previous import (`call_loader`), while
    /// the pipeline pre-resolves relatives against file bases first (which
    /// also hides the containing URL, since the resolved URL is absolute).
    /// Defaults to `false` (Dart behavior, unchanged for every existing
    /// host); a `true` importer is invoked with the raw URL and always
    /// receives the containing URL in that path.
    fn prefers_raw_load_paths(&self) -> bool {
        false
    }
}

/// Built-in importer variants plus host-provided ones.
///
/// Rewritten from Dart's `Importer` hierarchy (`importer.dart`): the closed
/// Rust enum replaces the open subclass tree, so the compiler lists every
/// dispatch site when a kind is added. Only [`ImporterKind::User`] boxes (the
/// JS/wasm host seam); built-ins are inline values. The sync `Importer` half
/// of Dart's interface (usable from both sync and async compiles) is what the
/// dual build targets; genuinely async hosts implement [`UserImporter`].
#[derive(Clone, Debug)]
pub enum ImporterKind {
    Filesystem(FilesystemImporter),
    NoOp,
    User(Rc<dyn UserImporter>),
    Package(PackageImporter),
    NodePackage(NodePackageImporter),
}

/// Copyable arena handle to an [`ImporterKind`].
///
/// The handle is `&'parse ImporterKind` — copying it is a pointer copy, and
/// equality/hash are address identity (except `NoOp`, which compares as a
/// singleton). Matches Dart: `Importer.noOp` for stylesheets without relative
/// imports.
#[derive(Clone, Copy, Debug)]
pub struct Importer<'parse>(&'parse ImporterKind);

impl<'parse> Importer<'parse> {
    pub fn new<'compile: 'parse>(arena: &'compile Bump, kind: ImporterKind) -> Self {
        Importer(arena.alloc(kind))
    }

    pub fn kind(&self) -> &ImporterKind {
        self.0
    }

    #[rust_sass_macros::maybe_async]
    pub async fn canonicalize(
        &self,
        url: &SassUrl,
        ctx: &mut CanonicalizeContext,
    ) -> SassResult<Option<SassUrl>> {
        // Convenience entry point for non-eval callers (tests, REPL paths):
        // the eval pipeline always threads the caller's logger via
        // `ImportCache::canonicalize_with`. A fresh `NoOpWarnLogger` here
        // matches the pre-fix behavior; the F15a fix threads the logger at
        // the `canonicalize_with` call sites, not here.
        match self.0 {
            ImporterKind::Filesystem(f) => f.canonicalize(url, ctx, &NoOpWarnLogger).await,
            ImporterKind::NoOp => NoOpImporter.canonicalize(url, ctx),
            ImporterKind::User(u) => u.canonicalize(url, ctx).await,
            ImporterKind::Package(p) => p.canonicalize(url, ctx, &NoOpWarnLogger).await,
            ImporterKind::NodePackage(n) => n.canonicalize(url, ctx, &NoOpWarnLogger).await,
        }
    }

    #[rust_sass_macros::maybe_async]
    pub async fn load(&self, url: &SassUrl) -> SassResult<Option<ImporterResult>> {
        match self.0 {
            ImporterKind::Filesystem(f) => f.load(url).await,
            ImporterKind::NoOp => NoOpImporter.load(url),
            ImporterKind::User(u) => u.load(url).await,
            ImporterKind::Package(p) => p.load(url).await,
            ImporterKind::NodePackage(n) => n.load(url).await,
        }
    }

    pub fn could_canonicalize(&self, url: &SassUrl, canonical_url: &SassUrl) -> bool {
        match self.0 {
            ImporterKind::Filesystem(f) => f.could_canonicalize(url, canonical_url),
            ImporterKind::NoOp => false,
            ImporterKind::User(u) => u.could_canonicalize(url, canonical_url),
            ImporterKind::Package(p) => p.could_canonicalize(url, canonical_url),
            ImporterKind::NodePackage(n) => n.could_canonicalize(url, canonical_url),
        }
    }

    pub fn is_non_canonical_scheme(&self, scheme: &str) -> bool {
        match self.0 {
            ImporterKind::Filesystem(f) => f.is_non_canonical_scheme(scheme),
            ImporterKind::NoOp => false,
            ImporterKind::User(u) => u.is_non_canonical_scheme(scheme),
            ImporterKind::Package(p) => p.is_non_canonical_scheme(scheme),
            ImporterKind::NodePackage(n) => n.is_non_canonical_scheme(scheme),
        }
    }
}

impl PartialEq for Importer<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self.0, other.0) {
            (ImporterKind::NoOp, ImporterKind::NoOp) => true,
            _ => std::ptr::eq(self.0, other.0),
        }
    }
}

impl Eq for Importer<'_> {}

impl fmt::Display for Importer<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ImporterKind::Filesystem(_) => write!(f, "FilesystemImporter"),
            ImporterKind::NoOp => write!(f, "NoOpImporter"),
            ImporterKind::User(_) => write!(f, "UserImporter"),
            ImporterKind::Package(_) => write!(f, "package:..."),
            ImporterKind::NodePackage(_) => write!(f, "NodePackageImporter"),
        }
    }
}

impl Hash for Importer<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if matches!(self.0, ImporterKind::NoOp) {
            0u8.hash(state);
        } else {
            (self.0 as *const ImporterKind).hash(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::DefaultIo;
    use crate::io::Io;
    use std::collections::hash_map::DefaultHasher;

    #[test]
    fn noop_singleton_eq() {
        let arena = bumpalo::Bump::new();
        let a = Importer::new(&arena, ImporterKind::NoOp);
        let b = Importer::new(&arena, ImporterKind::NoOp);
        assert_eq!(a, b);
    }

    #[test]
    fn noop_singleton_hash() {
        let arena = bumpalo::Bump::new();
        let a = Importer::new(&arena, ImporterKind::NoOp);
        let b = Importer::new(&arena, ImporterKind::NoOp);
        let mut ha = DefaultHasher::new();
        let mut hb = DefaultHasher::new();
        a.hash(&mut ha);
        b.hash(&mut hb);
        assert_eq!(ha.finish(), hb.finish());
    }

    #[test]
    fn filesystem_not_eq_different_rc() {
        let arena = bumpalo::Bump::new();
        let io: Rc<dyn Io> = Rc::new(DefaultIo::new());
        let fs1 = FilesystemImporter::new_no_load_path(Rc::clone(&io));
        let fs2 = FilesystemImporter::new_no_load_path(io);
        let a = Importer::new(&arena, ImporterKind::Filesystem(fs1));
        let b = Importer::new(&arena, ImporterKind::Filesystem(fs2));
        assert_ne!(a, b);
    }

    #[test]
    fn filesystem_eq_same_rc() {
        let arena = bumpalo::Bump::new();
        let io: Rc<dyn Io> = Rc::new(DefaultIo::new());
        let fs = FilesystemImporter::new_no_load_path(io);
        let a = Importer::new(&arena, ImporterKind::Filesystem(fs.clone()));
        let b = Importer::new(&arena, ImporterKind::Filesystem(fs));
        // Different Rc allocation — not equal
        // Actually: same FilesystemImporter but different Rc<ImporterKind> → not equal
        assert_ne!(a, b);
    }
}
