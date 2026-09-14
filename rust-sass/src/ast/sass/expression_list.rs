// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/list.dart
// go-source: go/value/sass_expression_list.go

use crate::ast::sass::unary_operator::UnaryOperator;
use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::value::ListSeparator;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::interpolation::Interpolation;

/// A list literal.
#[derive(Clone, Debug)]
pub struct ListExpression<'parse> {
    /// The elements of this list.
    pub contents: Vec<Expression<'parse>>,
    /// Which separator this list uses.
    pub separator: ListSeparator,
    /// Whether the list has square brackets or not.
    pub has_brackets: bool,
    pub span: FileSpan<'parse>,
}

impl<'parse> ListExpression<'parse> {
    pub fn new(
        contents: Vec<Expression<'parse>>,
        separator: ListSeparator,
        span: FileSpan<'parse>,
        has_brackets: bool,
    ) -> Self {
        ListExpression {
            contents,
            separator,
            has_brackets,
            span,
        }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }
}

impl<'parse> ListExpression<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        if self.has_brackets {
            buf.push('[');
        } else if self.contents.is_empty()
            || (self.contents.len() == 1 && self.separator == ListSeparator::Comma)
        {
            buf.push('(');
        }

        for (i, element) in self.contents.iter().enumerate() {
            if i > 0 {
                if self.separator == ListSeparator::Comma {
                    buf.push_str(", ");
                } else {
                    buf.push(' ');
                }
            }
            if self.element_needs_parens(element) {
                write!(buf, "({})", element.to_display_string()?).unwrap();
            } else {
                write!(buf, "{}", element.to_display_string()?).unwrap();
            }
        }

        if self.has_brackets {
            buf.push(']');
        } else if self.contents.is_empty() {
            buf.push(')');
        } else if self.contents.len() == 1 && self.separator == ListSeparator::Comma {
            buf.push_str(",)");
        }

        Ok(buf)
    }

    // element_needs_parens stays unchanged
}

impl<'parse> AstNode<'parse> for ListExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for ListExpression<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

impl<'parse> ListExpression<'parse> {
    fn element_needs_parens(&self, expression: &Expression<'parse>) -> bool {
        if let Expression::List(list) = expression {
            if list.contents.len() >= 2 && !list.has_brackets {
                let child_sep = list.separator;
                if self.separator == ListSeparator::Comma {
                    return child_sep == ListSeparator::Comma;
                }
                return child_sep != ListSeparator::Undecided;
            }
        }
        if let Expression::UnaryOperation(unary) = expression {
            return self.separator == ListSeparator::Space
                && matches!(unary.operator, UnaryOperator::Plus | UnaryOperator::Minus);
        }
        false
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
    fn test_construction() {
        let arena = Bump::new();
        let span = test_span(&arena, "a, b", 0, 4);
        let contents = vec![
            Expression::String(StringExpression::new(
                Interpolation::plain("a".into(), span),
                true,
            )),
            Expression::String(StringExpression::new(
                Interpolation::plain("b".into(), span),
                true,
            )),
        ];
        let expr = ListExpression::new(contents, ListSeparator::Comma, span, false);
        assert_eq!(expr.contents.len(), 2);
    }

    #[test]
    fn test_string_comma() {
        let arena = Bump::new();
        let span = test_span(&arena, "a, b, c", 0, 7);
        let contents = vec![
            Expression::String(StringExpression::new(
                Interpolation::plain("a".into(), span),
                true,
            )),
            Expression::String(StringExpression::new(
                Interpolation::plain("b".into(), span),
                true,
            )),
            Expression::String(StringExpression::new(
                Interpolation::plain("c".into(), span),
                true,
            )),
        ];
        let expr = ListExpression::new(contents, ListSeparator::Comma, span, false);
        let result = format!("{expr}");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let span = test_span(&arena, "a, b", 0, 4);
        let expr = ListExpression::new(vec![], ListSeparator::Space, span, false);
        assert!(expr.source_interpolation().is_none());
    }
}
