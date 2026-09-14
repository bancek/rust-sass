// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/css/import.dart + lib/src/ast/css/modifiable/import.dart
// go-source: go/value/css_import.go + go/value/css_modifiable_import.go

use std::fmt;

use crate::common::ast_css_value::CssValue;
use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

/// A plain CSS `@import`.
#[derive(Clone, Debug)]
pub struct CssImport<'parse> {
    /// The URL being imported, including quotes.
    pub url: CssValue<'parse, String>,
    /// The modifiers (such as media or supports queries) attached to this import.
    pub modifiers: Option<CssValue<'parse, String>>,
    /// The source span for this import.
    pub span: FileSpan<'parse>,
    /// Whether this node was the last in a nested Sass tree flattened during
    /// evaluation. See [`CssNode::is_group_end`](super::node::CssNode::is_group_end).
    pub is_group_end: bool,
}

impl<'parse> CssImport<'parse> {
    /// Creates an `@import` with optional modifiers.
    pub fn new(
        url: CssValue<'parse, String>,
        span: FileSpan<'parse>,
        modifiers: Option<CssValue<'parse, String>>,
    ) -> Self {
        CssImport {
            url,
            modifiers,
            span,
            is_group_end: false,
        }
    }
}

impl<'parse> AstNode<'parse> for CssImport<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for CssImport<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@import {}", self.url)?;
        if let Some(ref m) = self.modifiers {
            write!(f, " {}", m)?;
        }
        write!(f, ";")
    }
}

// Frozen-class docs ported from `CssImport` (import.dart). The modifiable
// counterpart (`ModifiableCssImport` in modifiable/import.dart) implements the
// frozen interface for use during evaluation; see `docs/ref/ast.md` (CSS AST).
// This file merges `clone_css`-adjacent conversion (`from_css_import`) into
// the modifiable struct (split rule); no separate `cloneCss` exists on Dart's
// `ModifiableCssImport`.
#[derive(Clone, Debug)]
pub struct ModifiableCssImport<'parse> {
    /// The URL being imported, including quotes.
    pub url: CssValue<'parse, String>,
    /// The modifiers (such as media or supports queries) attached to this import.
    pub modifiers: Option<CssValue<'parse, String>>,
    /// The source span for this import.
    pub span: FileSpan<'parse>,
}

impl<'parse> ModifiableCssImport<'parse> {
    /// Creates a modifiable import by copying the fields of a frozen one.
    pub fn from_css_import(node: &CssImport<'parse>) -> Self {
        ModifiableCssImport {
            url: node.url.clone(),
            modifiers: node.modifiers.clone(),
            span: node.span,
        }
    }
}

impl<'parse> AstNode<'parse> for ModifiableCssImport<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for ModifiableCssImport<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@import {}", self.url)?;
        if let Some(ref m) = self.modifiers {
            write!(f, " {}", m)?;
        }
        write!(f, ";")
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

    fn make_val<'compile, 'parse>(arena: &'compile Bump, s: &str) -> CssValue<'parse, String>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        CssValue::new(s.into(), make_span(arena, s))
    }

    #[test]
    fn test_import_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let url = make_val(&arena, "\"foo.css\"");
        let imp = ModifiableCssImport::from_css_import(&CssImport::new(url, span, None));
        assert_eq!(imp.url.value, "\"foo.css\"");
        assert!(imp.modifiers.is_none());
    }

    #[test]
    fn test_import_with_modifiers() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let url = make_val(&arena, "\"foo.css\"");
        let modi = make_val(&arena, "screen");
        let imp = CssImport::new(url, span, Some(modi));
        assert!(imp.modifiers.is_some());
    }

    #[test]
    fn test_import_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let url = make_val(&arena, "\"foo.css\"");
        let imp = CssImport::new(url, span, None);
        assert_eq!(format!("{imp}"), "@import \"foo.css\";");
    }
}
