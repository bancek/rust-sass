// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/content_block.dart
// go-source: go/value/sass_statement_content_block.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::parameter_list::ParameterList;
use crate::ast::sass::statement::Statement;

/// An anonymous block of code invoked for a `@content` rule.
///
/// This is the content block passed to a mixin invocation, run wherever the
/// mixin body issues `@content`.
#[derive(Clone, Debug)]
pub struct ContentBlock<'parse> {
    /// The parameters the content block accepts.
    pub parameters: ParameterList<'parse>,
    pub children: Vec<Statement<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> ContentBlock<'parse> {
    pub fn new(
        parameters: ParameterList<'parse>,
        children: Vec<Statement<'parse>>,
        span: FileSpan<'parse>,
    ) -> Self {
        ContentBlock {
            parameters,
            children,
            span,
        }
    }
}

impl<'parse> AstNode<'parse> for ContentBlock<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> ContentBlock<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        if !self.parameters.is_empty() {
            write!(buf, " using ({})", self.parameters).unwrap();
        }
        write!(buf, " {{").unwrap();
        for child in &self.children {
            write!(buf, " {child}").unwrap();
        }
        write!(buf, " }}").unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for ContentBlock<'parse> {
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
        let span = make_span(&arena, "{ }");
        let params = ParameterList::empty(span);
        let cb = ContentBlock::new(params, vec![], span);
        assert_eq!(cb.span().unwrap(), span);
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "{ }");
        let params = ParameterList::empty(span);
        let cb = ContentBlock::new(params, vec![], span);
        let s = format!("{cb}");
        assert!(s.contains('{'), "got {s:?}");
    }
}
