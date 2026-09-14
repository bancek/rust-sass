// Copyright 2020 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/supports_condition/anything.dart
// go-source: go/value/sass_supports_condition_anything.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::common::span::Span;

use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::ast::sass::supports_condition::convert_span_error;

#[derive(Clone, Debug)]
/// A supports condition holding the forwards-compatible `<general-enclosed>`
/// production: anything that is neither a declaration nor a recognized
/// function test, kept verbatim so future syntax still parses.
pub struct SupportsAnything<'parse> {
    /// The raw contents of the condition.
    pub contents: Interpolation<'parse>,
    /// The span covering the whole condition, including the outer parens.
    pub span: FileSpan<'parse>,
}

impl<'parse> SupportsAnything<'parse> {
    pub fn new(contents: Interpolation<'parse>, span: FileSpan<'parse>) -> Self {
        SupportsAnything { contents, span }
    }

    // Rebuilds an interpolation with the same text by splicing the stored
    // contents back between the surrounding `before`/`after` source slices.
    pub fn to_interpolation(&self) -> SassResult<Interpolation<'parse>> {
        let contents_span = Span::File(self.contents.span()?);
        let before = self
            .span
            .before(&contents_span)
            .map_err(convert_span_error)?;
        let after = self
            .span
            .after(&contents_span)
            .map_err(convert_span_error)?;

        let mut buf = InterpolationBuffer::new();
        buf.write(before.text());
        buf.add_interpolation(&self.contents);
        buf.write(after.text());
        buf.interpolation(self.span)
    }

    // Returns a copy of this condition with `span` as its span.
    pub fn with_span(&self, span: FileSpan<'parse>) -> Self {
        Self::new(self.contents.clone(), span)
    }
}

impl<'parse> AstNode<'parse> for SupportsAnything<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> SupportsAnything<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "({})", self.contents).unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for SupportsAnything<'parse> {
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

    fn test_span<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
        s: usize,
        e: usize,
    ) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), s, e)
    }

    #[test]
    fn test_construction_and_display() {
        let arena = Bump::new();
        let span = test_span(&arena, "(foo)", 0, 5);
        let contents = Interpolation::plain("foo".into(), test_span(&arena, "foo", 1, 4));
        let cond = SupportsAnything::new(contents, span);
        assert_eq!(format!("{cond}"), "(foo)");
        cond.span().unwrap();
    }
}
