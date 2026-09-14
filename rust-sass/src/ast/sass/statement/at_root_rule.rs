// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/at_root_rule.dart
// go-source: go/value/sass_statement_at_root_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::statement::Statement;

/// An `@at-root` rule.
///
/// This moves its contents "up" the tree through parent nodes.
#[derive(Clone, Debug)]
pub struct AtRootRule<'parse> {
    pub children: Vec<Statement<'parse>>,
    /// The query specifying which statements this should move its contents
    /// through.
    pub query: Option<Interpolation<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> AtRootRule<'parse> {
    pub fn new(
        children: Vec<Statement<'parse>>,
        span: FileSpan<'parse>,
        query: Option<Interpolation<'parse>>,
    ) -> Self {
        AtRootRule {
            children,
            query,
            span,
        }
    }
}

impl<'parse> AstNode<'parse> for AtRootRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> AtRootRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "@at-root ").unwrap();
        if let Some(ref query) = self.query {
            write!(buf, "{query} ").unwrap();
        }
        write!(buf, "{{").unwrap();
        for child in &self.children {
            write!(buf, " {child}").unwrap();
        }
        write!(buf, " }}").unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for AtRootRule<'parse> {
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
        let span = make_span(&arena, "@at-root { }");
        let rule = AtRootRule::new(vec![], span, None);
        assert!(rule.query.is_none());
    }

    #[test]
    fn test_with_query() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "@at-root (with: rule) { }", None);
        let span = FileSpan::new(Some(fs), 0, 24);
        let query = Interpolation::plain("(with: rule)".into(), FileSpan::new(Some(fs), 9, 21));
        let rule = AtRootRule::new(vec![], span, Some(query));
        assert!(rule.query.is_some());
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "@at-root { }");
        let rule = AtRootRule::new(vec![], span, None);
        assert_eq!(rule.span().unwrap(), span);
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "@at-root { }");
        let rule = AtRootRule::new(vec![], span, None);
        let s = format!("{rule}");
        assert!(s.contains("@at-root"), "got {s:?}");
    }
}
