// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolated_selector/list.dart
// go-source: go/value/sass_interpolated_selector_list.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::common::span::Span;

use crate::ast::sass::interpolated_selector::complex::InterpolatedComplexSelector;

/// A selector list before interpolation is resolved.
///
/// Unlike the resolved selector list, this is parsed during the initial
/// stylesheet parse.
#[derive(Clone, Debug)]
pub struct InterpolatedSelectorList<'parse> {
    /// The components of this selector. Never empty.
    pub components: Vec<InterpolatedComplexSelector<'parse>>,
}

impl<'parse> InterpolatedSelectorList<'parse> {
    /// Creates a list from `components`, which must not be empty.
    pub fn new(components: Vec<InterpolatedComplexSelector<'parse>>) -> SassResult<Self> {
        if components.is_empty() {
            return Err(Box::new(SassError::Script {
                message: "components may not be empty.".into(),
                argument_name: None,
            }));
        }
        Ok(InterpolatedSelectorList { components })
    }

    /// Renders this selector as comma-separated source text.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        for (i, component) in self.components.iter().enumerate() {
            if i > 0 {
                write!(buf, ", ").unwrap();
            }
            write!(buf, "{}", component).unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> AstNode<'parse> for InterpolatedSelectorList<'parse> {
    /// The single component's span, or the span from the first to the last
    /// component.
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        if self.components.len() == 1 {
            return self.components[0].span();
        }
        let first = self.components.first().unwrap().span()?;
        let last = self.components.last().unwrap().span()?;
        first.expand(&Span::File(last)).map_err(Into::into)
    }
}

impl<'parse> fmt::Display for InterpolatedSelectorList<'parse> {
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
    use crate::ast::sass::interpolated_selector::InterpolatedComplexSelectorComponent;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    fn make_compound<'compile, 'parse>(
        arena: &'compile Bump,
        name: &str,
    ) -> InterpolatedCompoundSelector<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, name, None);
        let span = FileSpan::new(Some(fs), 1, name.len());
        let interp = Interpolation::plain(name.into(), span);
        let class = InterpolatedClassSelector::new(interp);
        InterpolatedCompoundSelector::new(vec![InterpolatedSimpleSelector::Class(class)]).unwrap()
    }

    fn make_complex<'compile, 'parse>(arena: &'compile Bump) -> InterpolatedComplexSelector<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, "test", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let compound = make_compound(arena, "foo");
        let comp = InterpolatedComplexSelectorComponent::new(compound, span, None);
        InterpolatedComplexSelector::new(vec![comp], span, None).unwrap()
    }

    #[test]
    fn test_list_empty() {
        let result = InterpolatedSelectorList::new(vec![]);
        assert!(result.is_err());
    }

    #[test]
    fn test_list_single() {
        let arena = Bump::new();
        let cs = make_complex(&arena);
        let list = InterpolatedSelectorList::new(vec![cs]).unwrap();
        assert_eq!(list.components.len(), 1);
    }
}
