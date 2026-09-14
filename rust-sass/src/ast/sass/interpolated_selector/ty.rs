// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolated_selector/type.dart
// go-source: go/value/sass_interpolated_selector_type.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::interpolated_selector::qualified_name::InterpolatedQualifiedName;

/// A type selector, still containing interpolation at parse time.
///
/// Unlike the resolved type selector, this is parsed during the initial
/// stylesheet parse.
#[derive(Clone, Debug)]
pub struct InterpolatedTypeSelector<'parse> {
    /// The element name being selected for.
    pub name: InterpolatedQualifiedName<'parse>,
}

impl<'parse> InterpolatedTypeSelector<'parse> {
    /// Creates a type selector matching elements with the given name.
    pub fn new(name: InterpolatedQualifiedName<'parse>) -> Self {
        InterpolatedTypeSelector { name }
    }

    /// Renders this selector as its qualified-name source text.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "{}", self.name).unwrap();
        Ok(buf)
    }
}

impl<'parse> AstNode<'parse> for InterpolatedTypeSelector<'parse> {
    /// The span of the qualified name.
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        self.name.span()
    }
}

impl<'parse> fmt::Display for InterpolatedTypeSelector<'parse> {
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
    use crate::ast::sass::interpolation::Interpolation;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    #[test]
    fn test_type_selector_construction() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "div", None);
        let span = FileSpan::new(Some(fs), 0, 3);
        let name = Interpolation::plain("div".into(), span);
        let qn = InterpolatedQualifiedName::new(name, span, None);
        let sel = InterpolatedTypeSelector::new(qn);
        assert_eq!(sel.span().unwrap(), span);
    }
}
