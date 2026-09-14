// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/at_rule.dart
// go-source: go/value/sass_statement_at_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::statement::Statement;

/// An unknown at-rule.
///
/// Any at-rule the parser doesn't recognize specifically lands here, with
/// its name, value, and optional block preserved verbatim for evaluation.
#[derive(Clone, Debug)]
pub struct AtRule<'parse> {
    /// The block children, or `None` when the rule ends with `;` and has no
    /// block.
    pub children: Option<Vec<Statement<'parse>>>,
    /// The name of this rule, without the leading `@`.
    pub name: Interpolation<'parse>,
    /// The value of this rule, if it has one.
    pub value: Option<Interpolation<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> AtRule<'parse> {
    pub fn new(
        name: Interpolation<'parse>,
        span: FileSpan<'parse>,
        value: Option<Interpolation<'parse>>,
        children: Option<Vec<Statement<'parse>>>,
    ) -> Self {
        AtRule {
            children,
            name,
            value,
            span,
        }
    }
}

impl<'parse> AstNode<'parse> for AtRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> AtRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "@{}", self.name).unwrap();
        if let Some(ref value) = self.value {
            write!(buf, " {value}").unwrap();
        }
        match &self.children {
            None => write!(buf, ";").unwrap(),
            Some(children) => {
                write!(buf, " {{").unwrap();
                for child in children {
                    write!(buf, " {child}").unwrap();
                }
                write!(buf, " }}").unwrap();
            }
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for AtRule<'parse> {
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

    fn make_interp<'compile, 'parse>(arena: &'compile Bump, text: &str) -> Interpolation<'parse>
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
        let span = make_span(&arena, "@unknown;");
        let name = make_interp(&arena, "unknown");
        let ar = AtRule::new(name, span, None, None);
        assert!(ar.children.is_none());
    }

    #[test]
    fn test_display_no_children() {
        let arena = Bump::new();
        let span = make_span(&arena, "@unknown;");
        let name = make_interp(&arena, "unknown");
        let ar = AtRule::new(name, span, None, None);
        let s = format!("{ar}");
        assert!(s.ends_with(';'), "got {s:?}");
    }

    #[test]
    fn test_display_with_children() {
        let arena = Bump::new();
        let span = make_span(&arena, "@unknown { }");
        let name = make_interp(&arena, "unknown");
        let ar = AtRule::new(name, span, None, Some(vec![]));
        let s = format!("{ar}");
        assert!(!s.ends_with(';'), "got {s:?}");
    }

    #[test]
    fn test_nil_slice_invariant() {
        let arena = Bump::new();
        let span = make_span(&arena, "@unknown;");
        let name = make_interp(&arena, "unknown");
        let ar1 = AtRule::new(name.clone(), span, None, None);
        let ar2 = AtRule::new(name, span, None, Some(vec![]));
        // None = no block (ends with ;)
        assert!(ar1.children.is_none());
        // Some(vec![]) = empty {} block
        assert!(ar2.children.as_ref().unwrap().is_empty());
    }
}
