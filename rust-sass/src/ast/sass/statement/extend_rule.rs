// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/extend_rule.dart
// go-source: go/value/sass_statement_extend_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::interpolation::Interpolation;

/// An `@extend` rule.
///
/// This gives one selector all the styling of another.
#[derive(Clone, Debug)]
pub struct ExtendRule<'parse> {
    /// The interpolation for the selector that will be extended.
    pub selector: Interpolation<'parse>,

    /// Whether this is an optional extension.
    ///
    /// If an extension isn't optional, it will emit an error if it doesn't
    /// match any selectors.
    pub is_optional: bool,

    pub span: FileSpan<'parse>,
}

impl<'parse> ExtendRule<'parse> {
    pub fn new(selector: Interpolation<'parse>, span: FileSpan<'parse>, is_optional: bool) -> Self {
        ExtendRule {
            selector,
            is_optional,
            span,
        }
    }
}

impl<'parse> AstNode<'parse> for ExtendRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl ExtendRule<'_> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "@extend {}", self.selector).unwrap();
        if self.is_optional {
            write!(buf, " !optional").unwrap();
        }
        write!(buf, ";").unwrap();
        Ok(buf)
    }
}

impl fmt::Display for ExtendRule<'_> {
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
        let fs = FileSource::new_in(arena, text, None);
        let span = FileSpan::new(Some(fs), 0, text.len());
        Interpolation::plain(text.to_string(), span)
    }

    #[test]
    fn test_new_extend_rule() {
        let arena = Bump::new();
        let span = make_span(&arena, "@extend .foo;");
        let sel = plain_interp(&arena, ".foo");
        let r = ExtendRule::new(sel.clone(), span, false);

        assert!(!r.is_optional);
    }

    #[test]
    fn test_new_extend_rule_optional() {
        let arena = Bump::new();
        let span = make_span(&arena, "@extend .foo !optional;");
        let sel = plain_interp(&arena, ".foo");
        let r = ExtendRule::new(sel.clone(), span, true);

        assert!(r.is_optional);
    }

    #[test]
    fn test_extend_rule_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "@extend .foo;");
        let sel = plain_interp(&arena, ".foo");
        let r = ExtendRule::new(sel.clone(), span, false);

        r.span().unwrap();
    }

    #[test]
    fn test_extend_rule_string() {
        let arena = Bump::new();
        let span = make_span(&arena, "@extend .foo;");
        let sel = plain_interp(&arena, ".foo");
        let r = ExtendRule::new(sel.clone(), span, false);

        let s = r.to_display_string().unwrap();
        assert!(s.starts_with("@extend "), "got {s:?}");
        assert!(s.ends_with(';'), "got {s:?}");
    }

    #[test]
    fn test_extend_rule_string_optional() {
        let arena = Bump::new();
        let span = make_span(&arena, "@extend .foo !optional;");
        let sel = plain_interp(&arena, ".foo");
        let r = ExtendRule::new(sel.clone(), span, true);

        let s = r.to_display_string().unwrap();
        assert!(s.contains("!optional"), "got {s:?}");
    }
}
