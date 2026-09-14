// Copyright 2024 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/importer/canonicalize_context.dart
// go-source: go/eval/importer_canonicalize_context.go

use crate::url::SassUrl;

// Contextual information used by importers' `canonicalize`.
///
/// Rewritten from Dart's `CanonicalizeContext`
/// (`importer/canonicalize_context.dart`, internal): Dart threads the context
/// through a `Zone`; Rust passes it explicitly. Reading
/// [`CanonicalizeContext::containing_url`] marks it accessed, which is what
/// makes a canonicalization uncacheable (context-sensitive) in the import
/// cache.
#[derive(Clone, Debug)]
pub struct CanonicalizeContext {
    pub from_import: bool,
    containing_url: Option<SassUrl>,
    was_containing_url_accessed: bool,
}

impl CanonicalizeContext {
    /// Creates a context for a load from `containing_url`.
    ///
    /// Matches Dart: `CanonicalizeContext(containingUrl, fromImport)`.
    pub fn new(containing_url: Option<SassUrl>, from_import: bool) -> Self {
        CanonicalizeContext {
            from_import,
            containing_url,
            was_containing_url_accessed: false,
        }
    }

    /// The URL of the stylesheet containing the current load, if known.
    ///
    /// Only set when the containing stylesheet has a canonical URL and the
    /// URL being canonicalized is relative or has a non-canonical scheme.
    /// Reading it marks it accessed (see
    /// [`CanonicalizeContext::was_containing_url_accessed`]).
    ///
    /// Matches Dart: `CanonicalizeContext.containingUrl`.
    pub fn containing_url(&mut self) -> Option<&SassUrl> {
        self.was_containing_url_accessed = true;
        self.containing_url.as_ref()
    }

    /// Same as [`CanonicalizeContext::containing_url`] without marking it
    /// accessed (for internal checks that must not affect cacheability).
    ///
    /// Matches Dart: `CanonicalizeContext.containingUrlWithoutMarking`.
    pub fn containing_url_without_marking(&self) -> Option<&SassUrl> {
        self.containing_url.as_ref()
    }

    /// Whether [`CanonicalizeContext::containing_url`] was read.
    ///
    /// Used to decide whether a canonicalization result is cacheable.
    /// Matches Dart: `CanonicalizeContext.wasContainingUrlAccessed`.
    pub fn was_containing_url_accessed(&self) -> bool {
        self.was_containing_url_accessed
    }

    /// Runs `callback` with `from_import` set, restoring the old value after.
    ///
    /// Matches Dart: `CanonicalizeContext.withFromImport` (which additionally
    /// asserts the context is the ambient zone value — no zone exists in Rust,
    /// the context is passed explicitly).
    pub fn with_from_import<T>(&mut self, from_import: bool, callback: impl FnOnce() -> T) -> T {
        let old = self.from_import;
        self.from_import = from_import;
        let result = callback();
        self.from_import = old;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::url::SassUrl;

    #[test]
    fn new_with_from_import() {
        let ctx = CanonicalizeContext::new(None, true);
        assert!(ctx.from_import);
        assert!(ctx.containing_url_without_marking().is_none());
    }

    #[test]
    fn new_with_containing_url() {
        let url = SassUrl::parse("file:///test.scss").unwrap();
        let ctx = CanonicalizeContext::new(Some(url.clone()), false);
        assert!(!ctx.from_import);
        assert_eq!(
            ctx.containing_url_without_marking().unwrap().as_str(),
            "file:///test.scss"
        );
    }

    #[test]
    fn containing_url_marks_accessed() {
        let url = SassUrl::parse("file:///test.scss").unwrap();
        let mut ctx = CanonicalizeContext::new(Some(url), false);
        assert!(!ctx.was_containing_url_accessed());
        let _ = ctx.containing_url();
        assert!(ctx.was_containing_url_accessed());
    }

    #[test]
    fn containing_url_without_marking_does_not_mark() {
        let url = SassUrl::parse("file:///test.scss").unwrap();
        let ctx = CanonicalizeContext::new(Some(url), false);
        let _ = ctx.containing_url_without_marking();
        assert!(!ctx.was_containing_url_accessed());
    }

    #[test]
    fn with_from_import_restores_old_value() {
        let mut ctx = CanonicalizeContext::new(None, false);
        assert!(!ctx.from_import);
        let result = ctx.with_from_import(true, || 42);
        assert_eq!(result, 42);
        assert!(!ctx.from_import);
    }
}
