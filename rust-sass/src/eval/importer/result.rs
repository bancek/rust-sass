// Copyright 2017 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/importer/result.dart
// go-source: go/eval/importer_result.go

use crate::url::data_url_from_string;
use crate::url::SassUrl;

use crate::common::exception::SassResult;
use crate::parse::stylesheet::Syntax;

/// The result of importing a Sass stylesheet, as returned by importer `load`.
///
/// Rewritten from Dart's `ImporterResult` (`importer/result.dart`).
#[derive(Clone, Debug)]
pub struct ImporterResult {
    /// The contents of the stylesheet.
    pub contents: String,
    /// The syntax used to parse the stylesheet.
    pub syntax: Syntax,
    source_map_url: Option<SassUrl>,
}

impl ImporterResult {
    /// Creates a new importer result.
    ///
    /// Matches Dart: `ImporterResult(contents, {sourceMapUrl, syntax})`. The
    /// deprecated `indented` fallback and the absolute-`sourceMapUrl`
    /// validation live at the call boundary, not here; `syntax` is required.
    pub fn new(
        contents: String,
        syntax: Syntax,
        source_map_url: Option<SassUrl>,
    ) -> SassResult<Self> {
        Ok(ImporterResult {
            contents,
            syntax,
            source_map_url,
        })
    }

    /// Absolute, browser-accessible URL for the imported stylesheet's location.
    ///
    /// A `file:` URL when available (`http:` is also acceptable); when no URL
    /// was supplied, a `data:` URL is generated automatically from
    /// [`ImporterResult::contents`]. Matches Dart: `ImporterResult.sourceMapUrl`.
    pub fn source_map_url(&self) -> SassUrl {
        if let Some(ref url) = self.source_map_url {
            return url.clone();
        }
        SassUrl::parse(&data_url_from_string(&self.contents)).expect("valid data: URL")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::stylesheet::Syntax;

    #[test]
    fn new_with_contents_and_syntax() {
        let ir = ImporterResult::new("a {}".to_string(), Syntax::Scss, None).unwrap();
        assert_eq!(ir.contents, "a {}");
        assert_eq!(ir.syntax, Syntax::Scss);
    }

    #[test]
    fn new_with_source_map_url() {
        let url = SassUrl::parse("file:///test.scss").unwrap();
        let ir = ImporterResult::new("a {}".to_string(), Syntax::Scss, Some(url.clone())).unwrap();
        assert_eq!(ir.source_map_url, Some(url));
    }

    #[test]
    fn source_map_url_returns_provided_url() {
        let url = SassUrl::parse("file:///test.scss").unwrap();
        let ir = ImporterResult::new("a {}".to_string(), Syntax::Scss, Some(url.clone())).unwrap();
        assert_eq!(ir.source_map_url(), url);
    }

    #[test]
    fn source_map_url_generates_data_url() {
        let ir = ImporterResult::new("a {}".to_string(), Syntax::Scss, None).unwrap();
        let generated = ir.source_map_url();
        assert_eq!(generated.scheme(), "data");
        assert!(generated.to_string().contains("a%20%7B%7D"));
    }
}
