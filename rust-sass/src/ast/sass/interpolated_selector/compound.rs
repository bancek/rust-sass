// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolated_selector/compound.dart
// go-source: go/value/sass_interpolated_selector_compound.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::common::span::Span;

use crate::ast::sass::interpolated_selector::simple::InterpolatedSimpleSelector;

/// A compound selector before interpolation is resolved.
///
/// Unlike the resolved compound selector, this is parsed during the initial
/// stylesheet parse.
#[derive(Clone, Debug)]
pub struct InterpolatedCompoundSelector<'parse> {
    /// The simple selectors making up this compound selector. Never empty.
    pub components: Vec<InterpolatedSimpleSelector<'parse>>,
}

impl<'parse> InterpolatedCompoundSelector<'parse> {
    /// Creates a compound selector from `components`, which must not be empty.
    pub fn new(components: Vec<InterpolatedSimpleSelector<'parse>>) -> SassResult<Self> {
        if components.is_empty() {
            return Err(Box::new(SassError::Script {
                message: "components may not be empty.".into(),
                argument_name: None,
            }));
        }
        Ok(InterpolatedCompoundSelector { components })
    }

    /// Renders this selector by concatenating its components with no separator.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        for component in &self.components {
            write!(buf, "{}", component).unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> AstNode<'parse> for InterpolatedCompoundSelector<'parse> {
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

impl<'parse> fmt::Display for InterpolatedCompoundSelector<'parse> {
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
    fn test_compound_empty() {
        let result = InterpolatedCompoundSelector::new(vec![]);
        assert!(result.is_err());
    }

    #[test]
    fn test_compound_single() {
        let arena = Bump::new();
        let span = make_span(&arena, "foo");
        let name = Interpolation::plain("foo".into(), span);
        let class = InterpolatedClassSelector::new(name);
        let simp = InterpolatedSimpleSelector::Class(class);
        let compound = InterpolatedCompoundSelector::new(vec![simp]).unwrap();
        assert_eq!(compound.components.len(), 1);
    }
}
