// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolated_selector/attribute.dart
// go-source: go/value/sass_interpolated_selector_attribute.go

use std::fmt;
use std::fmt::Write;

use crate::ast::sass::interpolation::Interpolation;
use crate::common::ast_css_value::CssValue;
use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::selector::attribute::AttributeOperator;

use crate::ast::sass::interpolated_selector::qualified_name::InterpolatedQualifiedName;

/// An attribute selector, still containing interpolation at parse time.
///
/// Unlike the resolved attribute selector, this is parsed during the initial
/// stylesheet parse.
#[derive(Clone, Debug)]
pub struct InterpolatedAttributeSelector<'parse> {
    /// The name of the attribute being selected for.
    pub name: InterpolatedQualifiedName<'parse>,
    /// The operator defining the semantics of [`value`](Self::value).
    ///
    /// This is [`None`] if and only if `value` is [`None`].
    pub op: Option<CssValue<'parse, AttributeOperator>>,
    /// An assertion about the value of [`name`](Self::name).
    ///
    /// This is [`None`] if and only if `op` is [`None`].
    pub value: Option<Interpolation<'parse>>,
    /// The modifier indicating how the selector should be processed.
    ///
    /// Always [`None`] when `op` is [`None`].
    pub modifier: Option<Interpolation<'parse>>,
    /// The source span covering this selector.
    pub span: FileSpan<'parse>,
}

impl<'parse> InterpolatedAttributeSelector<'parse> {
    /// Creates an attribute selector matching any element with a property of
    /// the given name.
    pub fn new(name: InterpolatedQualifiedName<'parse>, span: FileSpan<'parse>) -> Self {
        InterpolatedAttributeSelector {
            name,
            span,
            op: None,
            value: None,
            modifier: None,
        }
    }

    /// Creates an attribute selector matching an element with a property named
    /// `name` whose value matches `value` per the semantics of `op`.
    pub fn with_operator(
        name: InterpolatedQualifiedName<'parse>,
        op: CssValue<'parse, AttributeOperator>,
        value: Interpolation<'parse>,
        span: FileSpan<'parse>,
        modifier: Option<Interpolation<'parse>>,
    ) -> Self {
        InterpolatedAttributeSelector {
            name,
            span,
            op: Some(op),
            value: Some(value),
            modifier,
        }
    }

    /// Renders this selector as `[...]` source text.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "[{}", self.name).unwrap();
        if let (Some(ref op), Some(ref value)) = (&self.op, &self.value) {
            write!(buf, "{}{}", op, value).unwrap();
            if let Some(ref modifier) = self.modifier {
                write!(buf, " {}", modifier).unwrap();
            }
        }
        write!(buf, "]").unwrap();
        Ok(buf)
    }
}

impl<'parse> AstNode<'parse> for InterpolatedAttributeSelector<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for InterpolatedAttributeSelector<'parse> {
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
    fn test_attribute_selector_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "[href]");
        let name_span = make_span(&arena, "href");
        let name = Interpolation::plain("href".into(), name_span);
        let qn = InterpolatedQualifiedName::new(name, name_span, None);
        let sel = InterpolatedAttributeSelector::new(qn, span);
        assert!(sel.op.is_none());
    }

    #[test]
    fn test_attribute_selector_with_operator() {
        let arena = Bump::new();
        let span = make_span(&arena, "[href=val]");
        let name_span = make_span(&arena, "href");
        let name = Interpolation::plain("href".into(), name_span);
        let qn = InterpolatedQualifiedName::new(name, name_span, None);
        let op_span = make_span(&arena, "=");
        let op = CssValue::new(AttributeOperator::Equal, op_span);
        let val_span = make_span(&arena, "val");
        let val = Interpolation::plain("val".into(), val_span);
        let sel = InterpolatedAttributeSelector::with_operator(qn, op, val, span, None);
        assert!(sel.op.is_some());
    }

    #[test]
    fn test_attribute_selector_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "[href]");
        let name_span = make_span(&arena, "href");
        let name = Interpolation::plain("href".into(), name_span);
        let qn = InterpolatedQualifiedName::new(name, name_span, None);
        let sel = InterpolatedAttributeSelector::new(qn, span);
        assert_eq!(format!("{sel}"), "[href]");
    }
}
