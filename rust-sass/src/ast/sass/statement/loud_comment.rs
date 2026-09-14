// Copyright 2017 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/loud_comment.dart
// go-source: go/value/sass_statement_loud_comment.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::interpolation::Interpolation;

#[derive(Clone, Debug)]
/// A loud CSS-style comment.
///
/// Unlike [`SilentComment`](super::silent_comment::SilentComment), this is
/// emitted into the compiled CSS.
pub struct LoudComment<'parse> {
    /// The interpolated comment text, including the `/*` and `*/` markers.
    pub text: Interpolation<'parse>,
}

impl<'parse> LoudComment<'parse> {
    pub fn new(text: Interpolation<'parse>) -> Self {
        LoudComment { text }
    }
}

impl<'parse> AstNode<'parse> for LoudComment<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        self.text.span()
    }
}

impl<'parse> LoudComment<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "{}", self.text).unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for LoudComment<'parse> {
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

    fn make_interp<'compile, 'parse>(arena: &'compile Bump, text: &str) -> Interpolation<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        let span = FileSpan::new(Some(fs), 0, text.len());
        Interpolation::plain(text.into(), span)
    }

    #[test]
    fn test_new() {
        let arena = Bump::new();
        let text = make_interp(&arena, "/* comment */");
        let _lc = LoudComment::new(text.clone());
        // verify construction doesn't panic
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let interp = make_interp(&arena, "/* comment */");
        let lc = LoudComment::new(interp.clone());
        let got = lc.span().unwrap();
        assert_eq!(got.text(), "/* comment */");
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let interp = make_interp(&arena, "/* comment */");
        let lc = LoudComment::new(interp);
        assert_eq!(format!("{lc}"), "/* comment */");
    }
}
