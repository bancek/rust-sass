// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/supports_condition/declaration.dart
// go-source: go/value/sass_supports_condition_declaration.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::common::span::Span;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;
use crate::ast::sass::supports_condition::convert_span_error;

#[derive(Clone, Debug)]
/// A condition that selects for browsers where a given declaration is
/// supported, e.g. `(display: flex)`.
pub struct SupportsDeclaration<'parse> {
    /// The name of the declaration being tested.
    pub name: Expression<'parse>,
    /// The value of the declaration being tested.
    pub value: Expression<'parse>,
    /// The span covering the whole `(name: value)` condition.
    pub span: FileSpan<'parse>,
}

impl<'parse> SupportsDeclaration<'parse> {
    pub fn new(
        name: Expression<'parse>,
        value: Expression<'parse>,
        span: FileSpan<'parse>,
    ) -> Self {
        SupportsDeclaration { name, value, span }
    }

    // Rebuilds an interpolation with the same text. An unquoted plain-text
    // name is spliced in as raw interpolation text; everything else (and any
    // value without its own source interpolation) is embedded as an
    // expression so it re-evaluates to the same source slice.
    pub fn to_interpolation(&self) -> SassResult<Interpolation<'parse>> {
        let name_fs = self.name.span()?;
        let name_s = Span::File(name_fs);
        let value_fs = self.value.span()?;
        let value_s = Span::File(value_fs);

        let before = self.span.before(&name_s).map_err(convert_span_error)?;
        let after = self.span.after(&value_s).map_err(convert_span_error)?;
        let between = name_fs.between(&value_s).map_err(convert_span_error)?;

        let mut buf = InterpolationBuffer::new();
        buf.write(before.text());

        if let Expression::String(se) = &self.name {
            if !se.has_quotes {
                buf.add_interpolation(&se.text);
            } else {
                buf.add(self.name.clone(), name_fs);
            }
        } else {
            buf.add(self.name.clone(), name_fs);
        }

        buf.write(between.text());

        if let Some(interp) = self.value.source_interpolation() {
            buf.add_interpolation(interp);
        } else {
            buf.add(self.value.clone(), value_fs);
        }

        buf.write(after.text());
        buf.interpolation(self.span)
    }

    // Returns a copy of this condition with `span` as its span.
    pub fn with_span(&self, span: FileSpan<'parse>) -> Self {
        Self::new(self.name.clone(), self.value.clone(), span)
    }

    /// Returns whether this is a CSS custom property declaration.
    ///
    /// Note that this can return `false` for declarations that will ultimately
    /// be serialized as custom properties if they aren't *parsed as* custom
    /// properties, such as `#{--foo}: ...`.
    ///
    /// If this returns `true`, then [`SupportsDeclaration::value`] holds a
    /// plain string expression.
    pub fn is_custom_property(&self) -> bool {
        if let Expression::String(se) = &self.name {
            !se.has_quotes && se.text.initial_plain().starts_with("--")
        } else {
            false
        }
    }
}

impl<'parse> AstNode<'parse> for SupportsDeclaration<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl SupportsDeclaration<'_> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "({}: {})", self.name, self.value).unwrap();
        Ok(buf)
    }
}

impl fmt::Display for SupportsDeclaration<'_> {
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
    use crate::ast::sass::expression_string::StringExpression;
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
        let span = test_span(&arena, "(a: b)", 0, 6);
        let name_text = Interpolation::plain("a".into(), test_span(&arena, "a", 1, 2));
        let val_text = Interpolation::plain("b".into(), test_span(&arena, "b", 4, 5));
        let name = Expression::String(StringExpression::new(name_text, false));
        let value = Expression::String(StringExpression::new(val_text, false));
        let cond = SupportsDeclaration::new(name, value, span);
        assert_eq!(format!("{cond}"), "(a: b)");
        cond.span().unwrap();
    }

    #[test]
    fn test_is_custom_property_true() {
        let arena = Bump::new();
        let span = test_span(&arena, "--x", 0, 3);
        let name_text = Interpolation::plain("--x".into(), span);
        let name = Expression::String(StringExpression::new(name_text, false));
        let val_text = Interpolation::plain("y".into(), span);
        let value = Expression::String(StringExpression::new(val_text, false));
        let cond = SupportsDeclaration::new(name, value, span);
        assert!(cond.is_custom_property());
    }

    #[test]
    fn test_is_custom_property_false() {
        let arena = Bump::new();
        let span = test_span(&arena, "a", 0, 1);
        let name_text = Interpolation::plain("a".into(), span);
        let name = Expression::String(StringExpression::new(name_text, false));
        let val_text = Interpolation::plain("b".into(), span);
        let value = Expression::String(StringExpression::new(val_text, false));
        let cond = SupportsDeclaration::new(name, value, span);
        assert!(!cond.is_custom_property());
    }
}
