// Copyright 2017 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/silent_comment.dart
// go-source: go/value/sass_statement_silent_comment.go

use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

#[derive(Clone, Debug)]
/// A silent Sass-style comment.
///
/// This never appears in the compiled CSS.
pub struct SilentComment<'parse> {
    /// The comment text, including the `//` markers.
    pub text: String,
    pub span: FileSpan<'parse>,
}

impl<'parse> SilentComment<'parse> {
    pub fn new(text: String, span: FileSpan<'parse>) -> Self {
        SilentComment { text, span }
    }

    /// Returns the subset of lines that are marked as documentation comments
    /// by beginning with `///`.
    ///
    /// Matches Dart: SilentComment.docComment
    pub fn doc_comment(&self) -> Option<String> {
        let mut builder = String::new();
        for line in self.text.split('\n') {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("///") {
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                builder.push_str(rest);
                builder.push('\n');
            }
        }
        let comment = builder.trim_end().to_string();
        if comment.is_empty() {
            None
        } else {
            Some(comment)
        }
    }
}

impl<'parse> SilentComment<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        buf.push_str(&self.text);
        Ok(buf)
    }
}

impl<'parse> AstNode<'parse> for SilentComment<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl fmt::Display for SilentComment<'_> {
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
        let span = make_span(&arena, "// comment");
        let sc = SilentComment::new("// comment".into(), span);
        assert_eq!(sc.text, "// comment");
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "// comment");
        let sc = SilentComment::new("// comment".into(), span);
        assert_eq!(sc.span().unwrap(), span);
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "// comment");
        let sc = SilentComment::new("// comment".into(), span);
        assert_eq!(format!("{sc}"), "// comment");
    }

    #[test]
    fn test_doc_comment() {
        let arena = Bump::new();
        let span = make_span(&arena, "/// doc");
        let sc = SilentComment::new("/// doc".into(), span);
        assert_eq!(sc.doc_comment().as_deref(), Some("doc"));
    }

    #[test]
    fn test_doc_comment_multi_line() {
        let arena = Bump::new();
        let span = make_span(&arena, "/// line 1\n/// line 2");
        let sc = SilentComment::new("/// line 1\n/// line 2\nnon-doc line".into(), span);
        let got = sc.doc_comment().unwrap();
        assert_eq!(got, "line 1\nline 2");
    }

    #[test]
    fn test_doc_comment_empty() {
        let arena = Bump::new();
        let span = make_span(&arena, "// regular");
        let sc = SilentComment::new("// regular".into(), span);
        assert_eq!(sc.doc_comment(), None);
    }
}
