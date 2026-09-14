// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/import/static.dart
// go-source: go/value/sass_static_import.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::interpolation::Interpolation;

/// An import that produces a plain CSS `@import` rule.
#[derive(Clone, Debug)]
pub struct StaticImport<'parse> {
    /// The URL for this import.
    ///
    /// This already contains quotes.
    pub url: Interpolation<'parse>,
    /// The modifiers (such as media or supports queries) attached to this
    /// import, or `None` if none are attached.
    pub modifiers: Option<Interpolation<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> StaticImport<'parse> {
    /// Creates a static import of `url` with optional trailing `modifiers`.
    pub fn new(
        url: Interpolation<'parse>,
        span: FileSpan<'parse>,
        modifiers: Option<Interpolation<'parse>>,
    ) -> Self {
        StaticImport {
            url,
            modifiers,
            span,
        }
    }
}

impl<'parse> AstNode<'parse> for StaticImport<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> StaticImport<'parse> {
    /// Renders `url` with a space-separated `modifiers` suffix when present.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "{}", self.url).unwrap();
        if let Some(ref modifiers) = self.modifiers {
            write!(buf, " {}", modifiers).unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for StaticImport<'parse> {
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

    fn plain_interp<'compile, 'parse>(arena: &'compile Bump, text: &str) -> Interpolation<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let span = make_span(arena, text);
        Interpolation::plain(text.into(), span)
    }

    #[test]
    fn test_new() {
        let arena = Bump::new();
        let span = make_span(&arena, "'url'");
        let url = plain_interp(&arena, "url");
        let si = StaticImport::new(url.clone(), span, None);
        assert!(si.modifiers.is_none());
    }

    #[test]
    fn test_new_with_modifiers() {
        let arena = Bump::new();
        let span = make_span(&arena, "'url' screen");
        let url = plain_interp(&arena, "url");
        let modifiers = plain_interp(&arena, "screen");
        let si = StaticImport::new(url.clone(), span, Some(modifiers.clone()));
        assert!(si.modifiers.is_some());
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "'url'");
        let url = plain_interp(&arena, "url");
        let si = StaticImport::new(url.clone(), span, None);
        assert_eq!(format!("{si}"), "url");
    }

    #[test]
    fn test_display_with_modifiers() {
        let arena = Bump::new();
        let span = make_span(&arena, "'url' screen");
        let url = plain_interp(&arena, "url");
        let modifiers = plain_interp(&arena, "screen");
        let si = StaticImport::new(url.clone(), span, Some(modifiers));
        assert_eq!(format!("{si}"), "url screen");
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "'url'");
        let url = plain_interp(&arena, "url");
        let si = StaticImport::new(url.clone(), span, None);
        assert_eq!(si.span().unwrap(), span);
    }
}
