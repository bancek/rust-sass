// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolated_selector/pseudo.dart
// go-source: go/value/sass_interpolated_selector_pseudo.go

use std::fmt;
use std::fmt::Write;

use crate::ast::sass::interpolation::Interpolation;
use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::interpolated_selector::list::InterpolatedSelectorList;

/// A pseudo-class or pseudo-element selector, still containing interpolation.
///
/// Unlike the resolved pseudo selector, this is parsed during the initial
/// stylesheet parse.
#[derive(Clone, Debug)]
pub struct InterpolatedPseudoSelector<'parse> {
    /// The name of this selector, including any vendor prefixes.
    pub name: Interpolation<'parse>,
    /// Whether this is syntactically a pseudo-class selector.
    ///
    /// This is `true` if and only if [`is_syntactic_element`](Self::is_syntactic_element)
    /// is `false`.
    pub is_syntactic_class: bool,
    /// The non-selector argument passed to this selector.
    ///
    /// [`None`] when there is no argument. When both `argument` and
    /// [`selector`](Self::selector) are set, the selector follows the argument.
    pub argument: Option<Interpolation<'parse>>,
    /// The selector argument passed to this selector.
    ///
    /// [`None`] when there is no selector. When both
    /// [`argument`](Self::argument) and `selector` are set, the selector
    /// follows the argument.
    pub selector: Option<InterpolatedSelectorList<'parse>>,
    /// The source span covering this selector.
    pub span: FileSpan<'parse>,
}

impl<'parse> InterpolatedPseudoSelector<'parse> {
    /// Creates a pseudo selector; pass `element = true` for a `::` element.
    pub fn new(
        name: Interpolation<'parse>,
        span: FileSpan<'parse>,
        element: bool,
        argument: Option<Interpolation<'parse>>,
        selector: Option<InterpolatedSelectorList<'parse>>,
    ) -> Self {
        InterpolatedPseudoSelector {
            name,
            span,
            is_syntactic_class: !element,
            argument,
            selector,
        }
    }

    /// Whether this is syntactically a pseudo-element selector.
    ///
    /// This is `true` if and only if
    /// [`is_syntactic_class`](Self::is_syntactic_class) is `false`.
    pub fn is_syntactic_element(&self) -> bool {
        !self.is_syntactic_class
    }

    /// Renders this selector, appending the `(argument selector)` suffix when
    /// either argument is present.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        let prefix = if self.is_syntactic_class { ":" } else { "::" };
        write!(buf, "{}{}", prefix, self.name).unwrap();
        if self.argument.is_some() || self.selector.is_some() {
            write!(buf, "(").unwrap();
            if let Some(ref arg) = self.argument {
                write!(buf, "{}", arg).unwrap();
                if self.selector.is_some() {
                    write!(buf, " ").unwrap();
                }
            }
            if let Some(ref sel) = self.selector {
                write!(buf, "{}", sel).unwrap();
            }
            write!(buf, ")").unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> AstNode<'parse> for InterpolatedPseudoSelector<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for InterpolatedPseudoSelector<'parse> {
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
    fn test_pseudo_class() {
        let arena = Bump::new();
        let span = make_span(&arena, ":hover");
        let name = Interpolation::plain("hover".into(), make_span(&arena, "hover"));
        let sel = InterpolatedPseudoSelector::new(name, span, false, None, None);
        assert!(sel.is_syntactic_class);
        assert!(!sel.is_syntactic_element());
    }

    #[test]
    fn test_pseudo_element() {
        let arena = Bump::new();
        let span = make_span(&arena, "::before");
        let name = Interpolation::plain("before".into(), make_span(&arena, "before"));
        let sel = InterpolatedPseudoSelector::new(name, span, true, None, None);
        assert!(!sel.is_syntactic_class);
        assert!(sel.is_syntactic_element());
    }

    #[test]
    fn test_pseudo_with_argument() {
        let arena = Bump::new();
        let span = make_span(&arena, ":nth-child(2n+1)");
        let name = Interpolation::plain("nth-child".into(), make_span(&arena, "nth-child"));
        let arg_span = make_span(&arena, "2n+1");
        let arg = Interpolation::plain("2n+1".into(), arg_span);
        let sel = InterpolatedPseudoSelector::new(name, span, false, Some(arg), None);
        assert!(sel.argument.is_some());
    }
}
