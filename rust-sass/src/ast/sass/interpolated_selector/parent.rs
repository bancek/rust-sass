// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolated_selector/parent.dart
// go-source: go/value/sass_interpolated_selector_parent.go

use std::fmt;
use std::fmt::Write;

use crate::ast::sass::interpolation::Interpolation;
use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

/// A parent selector, still containing interpolation at parse time.
///
/// Unlike the resolved parent selector, this is parsed during the initial
/// stylesheet parse.
#[derive(Clone, Debug)]
pub struct InterpolatedParentSelector<'parse> {
    /// The suffix added to the parent selector once it has been resolved.
    pub suffix: Option<Interpolation<'parse>>,
    /// The source span covering this selector.
    pub span: FileSpan<'parse>,
}

impl<'parse> InterpolatedParentSelector<'parse> {
    /// Creates a parent selector with an optional `suffix`.
    pub fn new(span: FileSpan<'parse>, suffix: Option<Interpolation<'parse>>) -> Self {
        InterpolatedParentSelector { suffix, span }
    }

    /// Renders this selector as `&` plus the suffix when present.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        if let Some(ref suffix) = self.suffix {
            write!(buf, "&{}", suffix).unwrap();
        } else {
            write!(buf, "&").unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> AstNode<'parse> for InterpolatedParentSelector<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for InterpolatedParentSelector<'parse> {
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
    fn test_parent_selector_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "&");
        let sel = InterpolatedParentSelector::new(span, None);
        assert_eq!(sel.span().unwrap(), span);
    }

    #[test]
    fn test_parent_selector_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "&");
        let sel = InterpolatedParentSelector::new(span, None);
        assert_eq!(format!("{sel}"), "&");
    }

    #[test]
    fn test_parent_selector_with_suffix() {
        let arena = Bump::new();
        let span = make_span(&arena, "&suffix");
        let sfx_span = make_span(&arena, "suffix");
        let suffix = Interpolation::plain("suffix".into(), sfx_span);
        let sel = InterpolatedParentSelector::new(span, Some(suffix));
        assert!(sel.suffix.is_some());
    }
}
