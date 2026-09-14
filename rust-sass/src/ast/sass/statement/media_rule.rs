// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/media_rule.dart
// go-source: go/value/sass_statement_media_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::statement::Statement;

#[derive(Clone, Debug)]
/// A `@media` rule.
pub struct MediaRule<'parse> {
    pub children: Vec<Statement<'parse>>,
    /// The query selecting which platforms the styles apply to.
    ///
    /// This is only parsed once interpolation has been resolved.
    pub query: Interpolation<'parse>,
    pub span: FileSpan<'parse>,
}

impl<'parse> MediaRule<'parse> {
    pub fn new(
        query: Interpolation<'parse>,
        children: Vec<Statement<'parse>>,
        span: FileSpan<'parse>,
    ) -> Self {
        MediaRule {
            children,
            query,
            span,
        }
    }
}

impl<'parse> AstNode<'parse> for MediaRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> MediaRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "@media {} {{", self.query).unwrap();
        for child in &self.children {
            write!(buf, " {child}").unwrap();
        }
        write!(buf, " }}").unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for MediaRule<'parse> {
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
        let span = make_span(&arena, "@media screen { }");
        let query = Interpolation::plain("screen".into(), span);
        let mr = MediaRule::new(query, vec![], span);
        assert_eq!(mr.span().unwrap(), span);
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "@media screen { }");
        let query = Interpolation::plain("screen".into(), span);
        let mr = MediaRule::new(query, vec![], span);
        let s = format!("{mr}");
        assert!(s.contains("@media"), "got {s:?}");
    }
}
