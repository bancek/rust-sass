// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/content_rule.dart
// go-source: go/value/sass_statement_content_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::argument_list::ArgumentList;

/// A `@content` rule.
///
/// Used inside a mixin to include the statement-level content passed by the
/// caller.
#[derive(Clone, Debug)]
pub struct ContentRule<'parse> {
    /// The arguments passed to this `@content` rule.
    ///
    /// This is an empty invocation when `@content` has no arguments.
    pub arguments: ArgumentList<'parse>,
    pub span: FileSpan<'parse>,
}

impl<'parse> ContentRule<'parse> {
    pub fn new(arguments: ArgumentList<'parse>, span: FileSpan<'parse>) -> Self {
        ContentRule { arguments, span }
    }
}

impl<'parse> AstNode<'parse> for ContentRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> ContentRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        if self.arguments.is_empty() {
            write!(buf, "@content;").unwrap();
        } else {
            write!(buf, "@content{};", self.arguments).unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for ContentRule<'parse> {
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
        let span = make_span(&arena, "@content;");
        let args = ArgumentList::empty(span);
        let cr = ContentRule::new(args, span);
        assert!(cr.arguments.is_empty());
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "@content;");
        let args = ArgumentList::empty(span);
        let cr = ContentRule::new(args, span);
        assert_eq!(cr.span().unwrap(), span);
    }

    #[test]
    fn test_display_empty() {
        let arena = Bump::new();
        let span = make_span(&arena, "@content;");
        let args = ArgumentList::empty(span);
        let cr = ContentRule::new(args, span);
        assert_eq!(format!("{cr}"), "@content;");
    }
}
