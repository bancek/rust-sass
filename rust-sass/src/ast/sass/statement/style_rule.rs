// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/style_rule.dart
// go-source: go/value/sass_statement_style_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::interpolated_selector::list::InterpolatedSelectorList;
use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::statement::Statement;

#[derive(Clone, Debug)]
/// A style rule.
///
/// This applies its declarations to elements matching a given selector.
pub struct StyleRule<'parse> {
    pub children: Vec<Statement<'parse>>,
    /// The selector the declarations apply to.
    ///
    /// This is only parsed once interpolation has been resolved. Exactly one
    /// of [`selector`](Self::selector) and
    /// [`parsed_selector`](Self::parsed_selector) is `Some`.
    pub selector: Option<Interpolation<'parse>>,
    /// Like [`selector`](Self::selector), but with as much of the selector
    /// parsed as possible.
    ///
    /// Unused by evaluation internals; set only when the stylesheet is parsed
    /// with selector parsing enabled. Exactly one of
    /// [`selector`](Self::selector) and
    /// [`parsed_selector`](Self::parsed_selector) is `Some`.
    pub parsed_selector: Option<InterpolatedSelectorList<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> StyleRule<'parse> {
    /// Creates a style rule with [`selector`](Self::selector) set and
    /// [`parsed_selector`](Self::parsed_selector) unset.
    pub fn new(
        selector: Interpolation<'parse>,
        children: Vec<Statement<'parse>>,
        span: FileSpan<'parse>,
    ) -> Self {
        StyleRule {
            children,
            selector: Some(selector),
            parsed_selector: None,
            span,
        }
    }

    /// Creates a style rule with [`parsed_selector`](Self::parsed_selector)
    /// set and [`selector`](Self::selector) unset.
    pub fn with_parsed_selector(
        parsed_selector: InterpolatedSelectorList<'parse>,
        children: Vec<Statement<'parse>>,
        span: FileSpan<'parse>,
    ) -> Self {
        StyleRule {
            children,
            selector: None,
            parsed_selector: Some(parsed_selector),
            span,
        }
    }
}

impl<'parse> AstNode<'parse> for StyleRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> StyleRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        let sel = self
            .selector
            .as_ref()
            .map(|s| s.to_display_string())
            .transpose()?;
        match sel {
            Some(ref s) => write!(buf, "{s}").unwrap(),
            None => write!(buf, "[parsed selector]").unwrap(),
        }
        write!(buf, " {{").unwrap();
        for child in &self.children {
            write!(buf, " {child}").unwrap();
        }
        write!(buf, " }}").unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for StyleRule<'parse> {
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
        let span = make_span(&arena, ".foo { }");
        let sel = make_interp(&arena, ".foo");
        let sr = StyleRule::new(sel, vec![], span);
        assert!(sr.selector.is_some());
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, ".foo { }");
        let sel = make_interp(&arena, ".foo");
        let sr = StyleRule::new(sel, vec![], span);
        let s = format!("{sr}");
        assert!(s.contains(".foo"), "got {s:?}");
    }
}
