// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolated_selector/id.dart
// go-source: go/value/sass_interpolated_selector_id.go

use std::fmt;
use std::fmt::Write;

use crate::ast::sass::interpolation::Interpolation;
use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;

/// An ID selector, still containing interpolation at parse time.
///
/// Unlike the resolved ID selector, this is parsed during the initial
/// stylesheet parse.
#[derive(Clone, Debug)]
pub struct InterpolatedIDSelector<'parse> {
    /// The ID name this selects for.
    pub name: Interpolation<'parse>,
}

impl<'parse> InterpolatedIDSelector<'parse> {
    /// Creates an ID selector for `name`.
    pub fn new(name: Interpolation<'parse>) -> Self {
        InterpolatedIDSelector { name }
    }

    /// Renders this selector as `#name` source text.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "#{}", self.name).unwrap();
        Ok(buf)
    }
}

impl<'parse> AstNode<'parse> for InterpolatedIDSelector<'parse> {
    /// The span covering the `#` prefix plus the name.
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        let ns = self.name.span()?;
        let file = ns.file().ok_or_else(|| SassError::Script {
            message: "No file source.".into(),
            argument_name: None,
        })?;
        let start = ns.start_location().offset.saturating_sub(1);
        let end = ns.end_location().offset;
        Ok(FileSpan::new(Some(file), start, end))
    }
}

impl<'parse> fmt::Display for InterpolatedIDSelector<'parse> {
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
        FileSpan::new(Some(fs), 1, text.len())
    }

    #[test]
    fn test_id_selector_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "bar");
        let name = Interpolation::plain("bar".into(), span);
        let sel = InterpolatedIDSelector::new(name);
        assert_eq!(format!("{sel}"), "#bar");
    }
}
