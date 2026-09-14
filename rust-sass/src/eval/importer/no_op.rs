// Copyright 2017 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/importer/no_op.dart
// go-source: go/eval/importer_no_op.go

use crate::common::time::SassTime;
use crate::url::SassUrl;
use std::fmt;

use crate::common::exception::SassResult;
use crate::eval::importer::{CanonicalizeContext, ImporterResult};

/// An importer that never imports any stylesheets.
///
/// Rewritten from Dart's `NoOpImporter` (`importer/no_op.dart`): used for
/// stylesheets which don't support relative imports, such as those compiled
/// from plain strings. Displays as `"(unknown)"`.
#[derive(Clone, Debug)]
pub struct NoOpImporter;

impl NoOpImporter {
    pub fn canonicalize(
        &self,
        _url: &SassUrl,
        _ctx: &CanonicalizeContext,
    ) -> SassResult<Option<SassUrl>> {
        Ok(None)
    }

    pub fn load(&self, _url: &SassUrl) -> SassResult<Option<ImporterResult>> {
        Ok(None)
    }

    pub fn could_canonicalize(&self, _url: &SassUrl, _canonical_url: &SassUrl) -> bool {
        false
    }

    pub fn is_non_canonical_scheme(&self, _scheme: &str) -> bool {
        false
    }

    pub fn modification_time(&self, _url: &SassUrl) -> SassResult<SassTime> {
        Ok(SassTime::now())
    }
}

impl fmt::Display for NoOpImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(unknown)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::url::SassUrl;

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_returns_none() {
        let imp = NoOpImporter;
        let url = SassUrl::parse("file:///test.scss").unwrap();
        let ctx = CanonicalizeContext::new(None, false);
        let result = imp.canonicalize(&url, &ctx).unwrap();
        assert!(result.is_none());
    }

    #[rust_sass_macros::maybe_test]
    async fn load_returns_none() {
        let imp = NoOpImporter;
        let url = SassUrl::parse("file:///test.scss").unwrap();
        let result = imp.load(&url).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn could_canonicalize_returns_false() {
        let imp = NoOpImporter;
        let url = SassUrl::parse("file:///test.scss").unwrap();
        assert!(!imp.could_canonicalize(&url, &url));
    }

    #[test]
    fn is_non_canonical_scheme_returns_false() {
        let imp = NoOpImporter;
        assert!(!imp.is_non_canonical_scheme("file"));
    }
}
