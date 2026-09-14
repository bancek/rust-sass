// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/import/dynamic.dart
// go-source: go/value/sass_dynamic_import.go

use std::fmt;
use std::fmt::Write;

use crate::url::SassUrl;

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
    pub span: FileSpan<'parse>,
}

impl<'parse> DynamicImport<'parse> {
    /// Creates a dynamic import of the file at `url_string`.
    pub fn new(url_string: String, span: FileSpan<'parse>) -> Self {
        DynamicImport { url_string, span }
    }

    /// Returns the parsed URL.
    ///
    /// Matches Dart: `DynamicImport.url` (`Uri.parse(urlString)`, which never
    /// throws for these inputs — the parser already rejected unparseable URLs
    /// via `SassUrl::parse` at construction in `atrule.rs`). Infallible by
    /// construction: any residual parse failure falls back to the raw string
    /// as a relative URL instead of panicking (no-`panic` rule).
    ///
    /// Matches Dart: DynamicImport.url
    pub fn url(&self) -> SassUrl {
        match SassUrl::parse(&self.url_string) {
            Ok(url) => url,
            Err(_) => SassUrl::parse_relative_fallback(&self.url_string),
        }
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
        let di = DynamicImport::new("foo.scss".into(), span);
        assert_eq!(di.url_string, "foo.scss");
    }

    #[test]
    fn test_url() {
        let arena = Bump::new();
        let span = make_span(&arena, "'foo.scss'");
        let di = DynamicImport::new("http://example.com/foo.scss".into(), span);
        let url = di.url();
        assert_eq!(url.as_str(), "http://example.com/foo.scss");
    }

    #[test]
    fn test_url_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "'foo.scss'");
        let di = DynamicImport::new("foo.scss".into(), span);
        assert!(di.url_span().is_ok());
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "'foo.scss'");
        let di = DynamicImport::new("foo.scss".into(), span);
        let s = format!("{di}");
        assert!(s.starts_with('"'), "got {s:?}");
        assert!(s.ends_with('"'), "got {s:?}");
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "'foo.scss'");
        let di = DynamicImport::new("foo.scss".into(), span);
        assert_eq!(di.span().unwrap(), span);
    }

    #[test]
    fn test_url_never_panics() {
        // `DynamicImport::new("http://[::1").url()` must
        // not panic (Dart `Uri.parse` never throws for these inputs — the
        // parser pre-validates; any residual failure is a fallback, not an
        // abort). `SassUrl::parse("http://[::1")` fails in the `url` crate
        // (invalid IPv6), which is exactly the case the old
        // `unwrap_or_else(panic!)` aborted on.
        assert!(
            SassUrl::parse("http://[::1").is_err(),
            "precondition: strict parse of the bad URL must fail"
        );
        let arena = Bump::new();
        let span = make_span(&arena, "'http://[::1'");
        let di = DynamicImport::new("http://[::1".into(), span);
        let url = di.url();
        assert_eq!(url.to_string(), "http://[::1");
    }
}
