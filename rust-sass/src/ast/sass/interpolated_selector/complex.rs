// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolated_selector/complex.dart
// go-source: go/value/sass_interpolated_selector_complex.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_css_value::CssValue;
use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::selector::combinator::Combinator;

use crate::ast::sass::interpolated_selector::complex_component::InterpolatedComplexSelectorComponent;

/// A complex selector before interpolation is resolved.
///
/// Unlike the resolved complex selector, this is parsed during the initial
/// stylesheet parse.
#[derive(Clone, Debug)]
pub struct InterpolatedComplexSelector<'parse> {
    /// This selector's leading combinator, if it has one.
    pub leading_combinator: Option<CssValue<'parse, Combinator>>,
    /// The components of this selector.
    ///
    /// Only empty when [`leading_combinator`](Self::leading_combinator) is set.
    pub components: Vec<InterpolatedComplexSelectorComponent<'parse>>,
    /// The source span covering this selector.
    pub span: FileSpan<'parse>,
}

impl<'parse> InterpolatedComplexSelector<'parse> {
    /// Creates a complex selector; `components` may only be empty when a
    /// leading combinator is present.
    pub fn new(
        components: Vec<InterpolatedComplexSelectorComponent<'parse>>,
        span: FileSpan<'parse>,
        leading_combinator: Option<CssValue<'parse, Combinator>>,
    ) -> SassResult<Self> {
        if leading_combinator.is_none() && components.is_empty() {
            return Err(Box::new(SassError::Script {
                message: "components may not be empty if leadingCombinator is null.".into(),
                argument_name: None,
            }));
        }
        Ok(InterpolatedComplexSelector {
            leading_combinator,
            components,
            span,
        })
    }

    /// Renders this selector as space-separated source text.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        for (i, component) in self.components.iter().enumerate() {
            if i > 0 {
                write!(buf, " ").unwrap();
            }
            write!(buf, "{}", component).unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> AstNode<'parse> for InterpolatedComplexSelector<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for InterpolatedComplexSelector<'parse> {
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
    use crate::ast::sass::interpolated_selector::class::InterpolatedClassSelector;
    use crate::ast::sass::interpolated_selector::compound::InterpolatedCompoundSelector;
    use crate::ast::sass::interpolated_selector::simple::InterpolatedSimpleSelector;
    use crate::ast::sass::interpolation::Interpolation;
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

    fn make_compound<'compile, 'parse>(
        arena: &'compile Bump,
        name: &str,
    ) -> InterpolatedCompoundSelector<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let span = make_span(arena, name);
        let interp = Interpolation::plain(name.into(), span);
        let class = InterpolatedClassSelector::new(interp);
        InterpolatedCompoundSelector::new(vec![InterpolatedSimpleSelector::Class(class)]).unwrap()
    }

    #[test]
    fn test_complex_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let compound = make_compound(&arena, "foo");
        let comp = InterpolatedComplexSelectorComponent::new(compound, span, None);
        let cs = InterpolatedComplexSelector::new(vec![comp], span, None).unwrap();
        assert!(cs.leading_combinator.is_none());
    }

    #[test]
    fn test_complex_empty() {
        let arena = Bump::new();
        let span = make_span(&arena, "test");
        let result = InterpolatedComplexSelector::new(vec![], span, None);
        assert!(result.is_err());
    }
}
