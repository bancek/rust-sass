// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/import/dynamic.dart
// go-source: go/value/sass_dynamic_import.go

use std::fmt;
use std::fmt::Write;

use crate::url::OwnedUrl;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

/// An import that will load a Sass file at runtime.
#[derive(Clone, Debug)]
pub struct DynamicImport<'parse> {
    /// The URL of the file to import, as a string so that a leading `./` is
    /// visible for Node Sass imports.
    ///
    /// If this is relative, it's relative to the containing file.
    //
    // Dart marks `urlString` `@internal`: it exists so the raw text (with any
    // leading `./`) survives for Node Sass compatibility.
    pub url_string: String,
    /// The parsed URL, fixed at construction (Dart re-parses lazily, but the
    /// parser has already validated it and Rust avoids fallible getters).
    pub url: OwnedUrl,
    pub span: FileSpan<'parse>,
}

impl<'parse> DynamicImport<'parse> {
    /// Creates a dynamic import of the file at `url_string`.
    pub fn new(url_string: String, url: OwnedUrl, span: FileSpan<'parse>) -> Self {
        DynamicImport {
            url_string,
            url,
            span,
        }
    }

    /// Returns the parsed URL.
    ///
    /// Matches Dart: `DynamicImport.url`. The URL is parsed once by the
    /// parser (`parseImportUrl` validates it via `OwnedUrl::parse`).
    pub fn url(&self) -> OwnedUrl {
        self.url.clone()
    }

    /// The span of the URL, including the quotes.
    pub fn url_span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> AstNode<'parse> for DynamicImport<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> DynamicImport<'parse> {
    /// Renders the URL quoted, as written in `@import`.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "{:?}", self.url_string).unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for DynamicImport<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    #[test]
    fn test_new() {
        let arena = Bump::new();
        let span = make_span(&arena, "'foo.scss'");
        let di = DynamicImport::new(
            "foo.scss".into(),
            OwnedUrl::parse("foo.scss").unwrap(),
            span,
        );
        assert_eq!(di.url_string, "foo.scss");
    }

    #[test]
    fn test_url() {
        let arena = Bump::new();
        let span = make_span(&arena, "'foo.scss'");
        let di = DynamicImport::new(
            "http://example.com/foo.scss".into(),
            OwnedUrl::parse("http://example.com/foo.scss").unwrap(),
            span,
        );
        let url = di.url();
        assert_eq!(url.as_str(), "http://example.com/foo.scss");
    }

    #[test]
    fn test_url_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "'foo.scss'");
        let di = DynamicImport::new(
            "foo.scss".into(),
            OwnedUrl::parse("foo.scss").unwrap(),
            span,
        );
        assert!(di.url_span().is_ok());
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "'foo.scss'");
        let di = DynamicImport::new(
            "foo.scss".into(),
            OwnedUrl::parse("foo.scss").unwrap(),
            span,
        );
        let s = format!("{di}");
        assert!(s.starts_with('"'), "got {s:?}");
        assert!(s.ends_with('"'), "got {s:?}");
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "'foo.scss'");
        let di = DynamicImport::new(
            "foo.scss".into(),
            OwnedUrl::parse("foo.scss").unwrap(),
            span,
        );
        assert_eq!(di.span().unwrap(), span);
    }

    #[test]
    fn test_url_round_trip() {
        let arena = Bump::new();
        let span = make_span(&arena, "'foo.scss'");
        let url = OwnedUrl::parse("foo.scss").unwrap();
        let di = DynamicImport::new("foo.scss".into(), url, span);
        assert_eq!(di.url().to_string(), "foo.scss");
        assert_eq!(di.url_string, "foo.scss");
    }
}
