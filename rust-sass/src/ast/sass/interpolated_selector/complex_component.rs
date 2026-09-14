// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolated_selector/complex_component.dart
// go-source: go/value/sass_interpolated_selector_complex_component.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_css_value::CssValue;
use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::selector::combinator::Combinator;

use crate::ast::sass::interpolated_selector::compound::InterpolatedCompoundSelector;

/// A component of an [`InterpolatedComplexSelector`](super::complex::InterpolatedComplexSelector).
///
/// Unlike the resolved component, this is parsed during the initial
/// stylesheet parse.
#[derive(Clone, Debug)]
pub struct InterpolatedComplexSelectorComponent<'parse> {
    /// This component's compound selector.
    pub selector: InterpolatedCompoundSelector<'parse>,
    /// This component's combinator, or [`None`] for an implicit descendant
    /// combinator.
    pub combinator: Option<CssValue<'parse, Combinator>>,
    /// The source span covering this component.
    pub span: FileSpan<'parse>,
}

impl<'parse> InterpolatedComplexSelectorComponent<'parse> {
    /// Creates a component with the given compound selector and combinator.
    pub fn new(
        selector: InterpolatedCompoundSelector<'parse>,
        span: FileSpan<'parse>,
        combinator: Option<CssValue<'parse, Combinator>>,
    ) -> Self {
        InterpolatedComplexSelectorComponent {
            selector,
            combinator,
            span,
        }
    }

    /// Renders this component, appending the combinator when present.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "{}", self.selector).unwrap();
        if let Some(ref combinator) = self.combinator {
            write!(buf, " {}", combinator).unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> AstNode<'parse> for InterpolatedComplexSelectorComponent<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for InterpolatedComplexSelectorComponent<'parse> {
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
    use crate::ast::sass::interpolated_selector::InterpolatedClassSelector;
    use crate::ast::sass::interpolated_selector::InterpolatedSimpleSelector;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 1, text.len())
    }

    #[test]
    fn test_complex_component_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let compound = InterpolatedCompoundSelector::new(vec![InterpolatedSimpleSelector::Class(
            InterpolatedClassSelector::new(Interpolation::plain(
                "foo".into(),
                make_span(&arena, "foo"),
            )),
        )])
        .unwrap();
        let c = InterpolatedComplexSelectorComponent::new(compound, span, None);
        assert!(c.combinator.is_none());
    }
}
