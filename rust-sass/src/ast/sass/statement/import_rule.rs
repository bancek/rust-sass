// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/import_rule.dart
// go-source: go/value/sass_statement_import_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::import::Import;

/// An `@import` rule.
#[derive(Clone, Debug)]
pub struct ImportRule<'parse> {
    /// The imports imported by this statement.
    pub imports: Vec<Import<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> ImportRule<'parse> {
    pub fn new(imports: Vec<Import<'parse>>, span: FileSpan<'parse>) -> Self {
        ImportRule { imports, span }
    }
}

impl<'parse> AstNode<'parse> for ImportRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> ImportRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        let parts: Vec<String> = self.imports.iter().map(|i| i.to_string()).collect();
        write!(buf, "@import {};", parts.join(", ")).unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for ImportRule<'parse> {
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
    use crate::ast::sass::dynamic_import::DynamicImport;
    use crate::ast::sass::import::Import;
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
        let span = make_span(&arena, "@import 'foo';");
        let di = DynamicImport::new("foo.scss".into(), span);
        let ir = ImportRule::new(vec![Import::Dynamic(di)], span);
        assert_eq!(ir.imports.len(), 1);
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "@import 'foo';");
        let di = DynamicImport::new("foo.scss".into(), span);
        let ir = ImportRule::new(vec![Import::Dynamic(di)], span);
        assert_eq!(ir.span().unwrap(), span);
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "@import 'foo';");
        let di = DynamicImport::new("foo.scss".into(), span);
        let ir = ImportRule::new(vec![Import::Dynamic(di)], span);
        let s = format!("{ir}");
        assert!(s.starts_with("@import "), "got {s:?}");
        assert!(s.ends_with(';'), "got {s:?}");
    }
}
