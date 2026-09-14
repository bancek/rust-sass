// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolation.dart
// go-source: go/value/sass_interpolation.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::core_errors::ArgumentError;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::common::span::Span;

use crate::ast::sass::expression::Expression;

/// A component of an [`Interpolation`] — either plain text or a Sass
/// expression.
///
/// Rust models Dart's `String | Expression` contents union as an enum.
#[derive(Clone, Debug)]
pub enum InterpolationPart<'parse> {
    /// Plain text (never adjacent to another `Text`).
    Text(String),
    /// An interpolated expression (boxed: the recursive position).
    Expression(Box<Expression<'parse>>),
}

/// Plain text interpolated with Sass expressions.
#[derive(Clone, Debug)]
pub struct Interpolation<'parse> {
    /// The contents of this interpolation.
    ///
    /// This contains alternating [`InterpolationPart::Text`] and
    /// [`InterpolationPart::Expression`] parts. It never contains two
    /// adjacent `Text` parts.
    pub contents: Vec<InterpolationPart<'parse>>,
    /// The source spans for each [`Expression`] in [`contents`](Self::contents).
    ///
    /// Unlike `Expression::span`, which just covers the expression itself,
    /// these go from `#{` through `}`. `None` for `Text` parts, `Some(span)`
    /// for `Expression` parts.
    //
    // Dart marks `spans` `@internal`: callers outside interpolation
    // machinery should use `span_for_element` instead.
    pub spans: Vec<Option<FileSpan<'parse>>>,
    /// The span covering the entire interpolation.
    pub span: Span<'parse>,
}

impl<'parse> Interpolation<'parse> {
    /// Creates an interpolation with a single plain text element and no
    /// interpolated expressions.
    pub fn plain(text: String, span: impl Into<Span<'parse>>) -> Self {
        Interpolation {
            contents: vec![InterpolationPart::Text(text)],
            spans: vec![None],
            span: span.into(),
        }
    }

    /// Creates a new [`Interpolation`] with the given `contents`.
    ///
    /// `spans` must include a [`FileSpan`] for each [`Expression`] in
    /// `contents`, covering the entire `#{}` surrounding the expression.
    /// `Text` elements must have `None` spans.
    ///
    /// The single `span` must cover the entire interpolation.
    pub fn new(
        contents: Vec<InterpolationPart<'parse>>,
        spans: Vec<Option<FileSpan<'parse>>>,
        span: impl Into<Span<'parse>>,
    ) -> Result<Self, ArgumentError> {
        if spans.len() != contents.len() {
            return Err(ArgumentError {
                name: Some("spans".into()),
                message: format!(
                    "spans length ({}) must match contents length ({})",
                    spans.len(),
                    contents.len()
                ),
            });
        }

        for (i, part) in contents.iter().enumerate() {
            match part {
                InterpolationPart::Text(_) => {
                    if i > 0 {
                        if let InterpolationPart::Text(_) = &contents[i - 1] {
                            return Err(ArgumentError {
                                name: Some("contents".into()),
                                message: "contents may not contain adjacent strings".into(),
                            });
                        }
                    }
                    if spans[i].is_some() {
                        return Err(ArgumentError {
                            name: Some("spans".into()),
                            message: format!(
                                "spans may not have a value for string elements (at index {i})"
                            ),
                        });
                    }
                }
                InterpolationPart::Expression(_) => {
                    if spans[i].is_none() {
                        return Err(ArgumentError {
                            name: Some("spans".into()),
                            message: format!(
                                "spans must have a value for expression elements (at index {i})"
                            ),
                        });
                    }
                }
            }
        }

        Ok(Interpolation {
            contents,
            spans,
            span: span.into(),
        })
    }

    /// Returns whether this contains no interpolated expressions.
    ///
    /// An empty interpolation counts as plain (its text is `""`).
    pub fn is_plain(&self) -> bool {
        self.as_plain().is_some()
    }

    /// If this contains no interpolated expressions, returns its text contents.
    ///
    /// Otherwise returns `None`.
    pub fn as_plain(&self) -> Option<&str> {
        match self.contents.as_slice() {
            [] => Some(""),
            [InterpolationPart::Text(s)] => Some(s.as_str()),
            _ => None,
        }
    }

    /// Returns the plain text before the first interpolation, or the empty
    /// string.
    //
    // Dart marks `initialPlain` `@internal`: it backs plain-CSS checks, not
    // public API.
    pub fn initial_plain(&self) -> &str {
        match self.contents.first() {
            Some(InterpolationPart::Text(s)) => s.as_str(),
            _ => "",
        }
    }

    /// Returns the [`FileSpan`] covering the element of the interpolation at
    /// `index`.
    ///
    /// Unlike the expression's own span, which only covers the text of the
    /// expression itself, this typically covers the entire `#{}` surrounding
    /// it (no strong guarantee: interpolations built from bare Sass
    /// expressions may return the expression span as-is).
    ///
    /// For `Text` elements, this is the span covering the entire text,
    /// bounded by the interpolation's start/end or adjacent `#{}` spans.
    /// Unlike `contents[index].span`, this includes the quote for text at the
    /// beginning or end of quoted strings; the quote is never included for
    /// expressions.
    pub fn span_for_element(&self, index: usize) -> SassResult<FileSpan<'parse>> {
        match &self.contents[index] {
            InterpolationPart::Text(_) => {
                let start = if index == 0 {
                    self.span.start_location()?
                } else {
                    self.spans[index - 1].as_ref().unwrap().end_location()
                };

                let end = if index + 1 == self.spans.len() {
                    self.span.end_location()?
                } else {
                    self.spans[index + 1].as_ref().unwrap().start_location()
                };

                let file = self.span.file()?;
                Ok(FileSpan::new(file, start.offset, end.offset))
            }
            InterpolationPart::Expression(_) => Ok(self.spans[index].unwrap()),
        }
    }
}

impl<'parse> AstNode<'parse> for Interpolation<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        self.span.file_span()
    }
}

impl<'parse> Interpolation<'parse> {
    /// Renders the interpolation with expressions wrapped in `#{...}`.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        for part in &self.contents {
            match part {
                InterpolationPart::Text(s) => write!(buf, "{s}").unwrap(),
                InterpolationPart::Expression(expr) => {
                    write!(buf, "#{{{}}}", Expression::to_display_string(expr)?).unwrap()
                }
            }
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for Interpolation<'parse> {
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
    use crate::ast::sass::expression_variable::VariableExpression;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    fn test_span<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
        start: usize,
        end: usize,
    ) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), start, end)
    }

    #[test]
    fn test_plain() {
        let arena = Bump::new();
        let span = test_span(&arena, "hello", 0, 5);
        let interp = Interpolation::plain("hello".into(), span);

        assert!(interp.is_plain());
        assert_eq!(interp.as_plain(), Some("hello"));
        assert_eq!(interp.contents.len(), 1);
        assert!(matches!(
            &interp.contents[0],
            InterpolationPart::Text(s) if s == "hello"
        ));
        assert!(interp.spans[0].is_none());
    }

    #[test]
    fn test_with_expression() {
        let arena = Bump::new();
        let source = "a.#{$expr}.b";
        let file_span = test_span(&arena, source, 0, 12);
        let var_span = test_span(&arena, source, 4, 9); // $expr
        let interp_expr_span = test_span(&arena, source, 2, 10); // #{$expr}

        let expr = Expression::Variable(VariableExpression::new("expr".into(), var_span, None));

        let interp = Interpolation::new(
            vec![
                InterpolationPart::Text("a.".into()),
                InterpolationPart::Expression(Box::new(expr)),
                InterpolationPart::Text(".b".into()),
            ],
            vec![None, Some(interp_expr_span), None],
            file_span,
        )
        .unwrap();

        assert!(!interp.is_plain());
        assert_eq!(interp.as_plain(), None);
        assert_eq!(interp.initial_plain(), "a.");
        assert_eq!(format!("{interp}"), "a.#{$expr}.b");
    }

    #[test]
    fn test_adjacent_strings_error() {
        let arena = Bump::new();
        let span = test_span(&arena, "ab", 0, 2);
        let err = Interpolation::new(
            vec![
                InterpolationPart::Text("a".into()),
                InterpolationPart::Text("b".into()),
            ],
            vec![None, None],
            span,
        )
        .unwrap_err();
        assert!(err.message.contains("adjacent"));
    }

    #[test]
    fn test_mismatched_lengths_error() {
        let arena = Bump::new();
        let span = test_span(&arena, "a", 0, 1);
        let err = Interpolation::new(
            vec![InterpolationPart::Text("a".into())],
            vec![None, None],
            span,
        )
        .unwrap_err();
        assert!(err.message.contains("match"));
    }

    #[test]
    fn test_nil_span_for_expr_error() {
        let arena = Bump::new();
        let source = "a.#{$expr}.b";
        let file_span = test_span(&arena, source, 0, 12);
        let var_span = test_span(&arena, source, 4, 9);
        let expr = Expression::Variable(VariableExpression::new("expr".into(), var_span, None));

        let err = Interpolation::new(
            vec![
                InterpolationPart::Text("a.".into()),
                InterpolationPart::Expression(Box::new(expr)),
                InterpolationPart::Text(".b".into()),
            ],
            vec![None, None, None],
            file_span,
        )
        .unwrap_err();
        assert!(err.message.contains("expression"));
    }

    #[test]
    fn test_span_for_string_element_error() {
        let arena = Bump::new();
        let span = test_span(&arena, "a", 0, 1);
        let err = Interpolation::new(
            vec![InterpolationPart::Text("a".into())],
            vec![Some(test_span(&arena, "x", 0, 1))],
            span,
        )
        .unwrap_err();
        assert!(err.message.contains("string"));
    }

    #[test]
    fn test_span_for_element_first_string() {
        let arena = Bump::new();
        let source = "a.#{$expr}.b";
        let file_span = test_span(&arena, source, 0, 12);
        let var_span = test_span(&arena, source, 4, 9);
        let interp_expr_span = test_span(&arena, source, 2, 10);
        let expr = Expression::Variable(VariableExpression::new("expr".into(), var_span, None));

        let interp = Interpolation::new(
            vec![
                InterpolationPart::Text("a.".into()),
                InterpolationPart::Expression(Box::new(expr)),
                InterpolationPart::Text(".b".into()),
            ],
            vec![None, Some(interp_expr_span), None],
            file_span,
        )
        .unwrap();

        let el_span = interp.span_for_element(0).unwrap();
        assert_eq!(el_span.start_location().offset, 0);
        assert_eq!(el_span.end_location().offset, 2);
    }

    #[test]
    fn test_span_for_element_expression() {
        let arena = Bump::new();
        let source = "a.#{$expr}.b";
        let file_span = test_span(&arena, source, 0, 12);
        let var_span = test_span(&arena, source, 4, 9);
        let interp_expr_span = test_span(&arena, source, 2, 10);
        let expr = Expression::Variable(VariableExpression::new("expr".into(), var_span, None));

        let interp = Interpolation::new(
            vec![
                InterpolationPart::Text("a.".into()),
                InterpolationPart::Expression(Box::new(expr)),
                InterpolationPart::Text(".b".into()),
            ],
            vec![None, Some(interp_expr_span), None],
            file_span,
        )
        .unwrap();

        let el_span = interp.span_for_element(1).unwrap();
        assert_eq!(el_span.start_location().offset, 2);
        assert_eq!(el_span.end_location().offset, 10);
    }

    #[test]
    fn test_as_plain_empty() {
        let arena = Bump::new();
        let span = test_span(&arena, "", 0, 0);
        let interp = Interpolation::new(vec![], vec![], span).unwrap();
        assert_eq!(interp.as_plain(), Some(""));
        assert!(interp.is_plain());
    }

    #[test]
    fn test_interpolation_span() {
        let arena = Bump::new();
        let span = test_span(&arena, "hello", 0, 5);
        let interp = Interpolation::plain("hello".into(), span);
        let _got = interp.span().unwrap();
    }
}
