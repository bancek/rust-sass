// Copyright 2020 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/supports_condition/function.dart
// go-source: go/value/sass_supports_condition_function.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::common::span::Span;

use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::ast::sass::supports_condition::convert_span_error;

#[derive(Clone, Debug)]
/// A function-syntax condition, e.g. `selector(...)` or `font-tech(...)`.
pub struct SupportsFunction<'parse> {
    /// The name of the function.
    pub name: Interpolation<'parse>,
    /// The arguments to the function.
    pub arguments: Interpolation<'parse>,
    /// The span covering the whole `name(arguments)` condition.
    pub span: FileSpan<'parse>,
}

impl<'parse> SupportsFunction<'parse> {
    pub fn new(
        name: Interpolation<'parse>,
        arguments: Interpolation<'parse>,
        span: FileSpan<'parse>,
    ) -> Self {
        SupportsFunction {
            name,
            arguments,
            span,
        }
    }

    // Rebuilds an interpolation with the same text by splicing the stored
    // name and arguments back between the original `between`/`after` source
    // slices.
    pub fn to_interpolation(&self) -> SassResult<Interpolation<'parse>> {
        let name_fs = self.name.span()?;
        let args_fs = self.arguments.span()?;
        let args_s = Span::File(args_fs);

        let between = name_fs.between(&args_s).map_err(convert_span_error)?;
        let after = self.span.after(&args_s).map_err(convert_span_error)?;

        let mut buf = InterpolationBuffer::new();
        buf.add_interpolation(&self.name);
        buf.write(between.text());
        buf.add_interpolation(&self.arguments);
        buf.write(after.text());
        buf.interpolation(self.span)
    }

    // Returns a copy of this condition with `span` as its span.
    pub fn with_span(&self, span: FileSpan<'parse>) -> Self {
        Self::new(self.name.clone(), self.arguments.clone(), span)
    }
}

impl<'parse> AstNode<'parse> for SupportsFunction<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl SupportsFunction<'_> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "{}({})", self.name, self.arguments).unwrap();
        Ok(buf)
    }
}

impl fmt::Display for SupportsFunction<'_> {
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

    fn test_span<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
        s: usize,
        e: usize,
    ) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), s, e)
    }

    #[test]
    fn test_construction_and_display() {
        let arena = Bump::new();
        let span = test_span(&arena, "fn(args)", 0, 8);
        let name = Interpolation::plain("fn".into(), test_span(&arena, "fn", 0, 2));
        let args = Interpolation::plain("args".into(), test_span(&arena, "args", 3, 7));
        let cond = SupportsFunction::new(name, args, span);
        assert_eq!(format!("{cond}"), "fn(args)");
        cond.span().unwrap();
    }
}
