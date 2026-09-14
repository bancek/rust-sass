// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/argument_list.dart
// go-source: go/value/sass_argument_list.go

use std::fmt;
use std::fmt::Write;

use indexmap::IndexMap;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::value::ListSeparator;

use crate::ast::sass::expression::Expression;

/// A set of arguments passed to a function or mixin.
///
/// The positional arguments come first, followed by the named arguments
/// (`$name: value`), an optional rest argument (`$args...`), and an optional
/// keyword-rest argument (expected to hold a keyword map).
#[derive(Clone, Debug)]
pub struct ArgumentList<'parse> {
    /// The arguments passed by position, in source order.
    pub positional: Vec<Expression<'parse>>,
    /// The arguments passed by name, keyed by normalized name.
    pub named: IndexMap<String, Expression<'parse>>,
    /// The spans for the arguments passed by name, including their names.
    ///
    /// This always has the same keys as [`named`](Self::named) in the same
    /// order.
    pub named_spans: IndexMap<String, FileSpan<'parse>>,
    /// The first rest argument (as in `$args...`).
    pub rest: Option<Box<Expression<'parse>>>,
    /// The second rest argument, which is expected to only contain a keyword
    /// map.
    pub keyword_rest: Option<Box<Expression<'parse>>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> ArgumentList<'parse> {
    /// Creates an invocation with the given positional, named, and rest
    /// arguments.
    ///
    /// `rest` must be present whenever `keyword_rest` is.
    pub fn new(
        positional: Vec<Expression<'parse>>,
        named: IndexMap<String, Expression<'parse>>,
        named_spans: IndexMap<String, FileSpan<'parse>>,
        span: FileSpan<'parse>,
        rest: Option<Expression<'parse>>,
        keyword_rest: Option<Expression<'parse>>,
    ) -> Self {
        ArgumentList {
            positional,
            named,
            named_spans,
            span,
            rest: rest.map(Box::new),
            keyword_rest: keyword_rest.map(Box::new),
        }
    }

    /// Creates an invocation that passes no arguments.
    pub fn empty(span: FileSpan<'parse>) -> Self {
        ArgumentList {
            positional: Vec::new(),
            named: IndexMap::new(),
            named_spans: IndexMap::new(),
            span,
            rest: None,
            keyword_rest: None,
        }
    }

    /// Returns whether this invocation passes no arguments.
    ///
    /// Note that a present-but-empty [`keyword_rest`](Self::keyword_rest)
    /// still counts as empty, matching Dart's check (which only looks at
    /// `positional`, `named`, and `rest`).
    pub fn is_empty(&self) -> bool {
        self.positional.is_empty() && self.named.is_empty() && self.rest.is_none()
    }
}

/// Wraps comma-separated unbracketed lists in parentheses.
///
/// Matches Dart: ArgumentList._parenthesizeArgument
fn parenthesize_argument(arg: &Expression<'_>) -> SassResult<String> {
    if let Expression::List(list) = arg {
        if list.separator == ListSeparator::Comma && !list.has_brackets && list.contents.len() >= 2
        {
            return Ok(format!("({})", Expression::to_display_string(arg)?));
        }
    }
    Expression::to_display_string(arg)
}

impl<'parse> AstNode<'parse> for ArgumentList<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> ArgumentList<'parse> {
    /// Renders this invocation as `(positional, $name: value, rest...)`.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        buf.push('(');
        let mut first = true;
        for arg in &self.positional {
            if first {
                first = false;
            } else {
                write!(buf, ", ").unwrap();
            }
            write!(buf, "{}", parenthesize_argument(arg)?).unwrap();
        }
        for (name, value) in &self.named {
            if first {
                first = false;
            } else {
                write!(buf, ", ").unwrap();
            }
            write!(buf, "${name}: {}", parenthesize_argument(value)?).unwrap();
        }
        if let Some(ref rest) = self.rest {
            if first {
                first = false;
            } else {
                write!(buf, ", ").unwrap();
            }
            write!(buf, "{}...", parenthesize_argument(rest)?).unwrap();
        }
        if let Some(ref kw_rest) = self.keyword_rest {
            if !first {
                write!(buf, ", ").unwrap();
            }
            write!(buf, "{}...", parenthesize_argument(kw_rest)?).unwrap();
        }
        buf.push(')');
        Ok(buf)
    }
}

impl<'parse> fmt::Display for ArgumentList<'parse> {
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
    use crate::ast::sass::expression_boolean::BooleanExpression;
    use crate::ast::sass::expression_list::ListExpression;
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
    fn test_empty() {
        let arena = Bump::new();
        let span = test_span(&arena, "()", 0, 2);
        let al = ArgumentList::empty(span);
        assert!(al.is_empty());
        assert!(al.rest.is_none());
    }

    #[test]
    fn test_positional_only() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "(1, 2)", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let al = ArgumentList::new(
            vec![
                Expression::Boolean(BooleanExpression::new(true, span)),
                Expression::Boolean(BooleanExpression::new(false, span)),
            ],
            IndexMap::new(),
            IndexMap::new(),
            span,
            None,
            None,
        );
        assert!(!al.is_empty());
    }

    #[test]
    fn test_string_empty() {
        let arena = Bump::new();
        let span = test_span(&arena, "()", 0, 2);
        let al = ArgumentList::empty(span);
        assert_eq!(format!("{al}"), "()");
    }

    #[test]
    fn test_string_positional() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "(true)", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let al = ArgumentList::new(
            vec![Expression::Boolean(BooleanExpression::new(true, span))],
            IndexMap::new(),
            IndexMap::new(),
            span,
            None,
            None,
        );
        assert_eq!(format!("{al}"), "(true)");
    }

    #[test]
    fn test_string_named() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "($a: true)", None);
        let span = FileSpan::new(Some(fs), 0, 10);
        let mut named = IndexMap::new();
        let inner_span = FileSpan::new(Some(fs), 6, 10);
        named.insert(
            "a".into(),
            Expression::Boolean(BooleanExpression::new(true, inner_span)),
        );
        let al = ArgumentList::new(vec![], named, IndexMap::new(), span, None, None);
        assert_eq!(format!("{al}"), "($a: true)");
    }

    #[test]
    fn test_string_rest() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "(true)", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let al = ArgumentList::new(
            vec![],
            IndexMap::new(),
            IndexMap::new(),
            span,
            Some(Expression::Boolean(BooleanExpression::new(true, span))),
            None,
        );
        assert_eq!(format!("{al}"), "(true...)");
    }

    #[test]
    fn test_string_keyword_rest() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "(true)", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let al = ArgumentList::new(
            vec![],
            IndexMap::new(),
            IndexMap::new(),
            span,
            None,
            Some(Expression::Boolean(BooleanExpression::new(true, span))),
        );
        assert_eq!(format!("{al}"), "(true...)");
    }

    #[test]
    fn test_parenthesize_comma_list() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "(a, b)", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let list = Expression::List(ListExpression::new(
            vec![
                Expression::Boolean(BooleanExpression::new(true, span)),
                Expression::Boolean(BooleanExpression::new(false, span)),
            ],
            ListSeparator::Comma,
            span,
            false,
        ));
        let got = parenthesize_argument(&list).unwrap();
        assert!(got.starts_with('('), "expected parenthesized, got {got}");
    }

    #[test]
    fn test_parenthesize_non_list() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "true", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let got = parenthesize_argument(&Expression::Boolean(BooleanExpression::new(true, span)))
            .unwrap();
        assert_eq!(got, "true");
    }

    #[test]
    fn test_argument_list_span() {
        let arena = Bump::new();
        let span = test_span(&arena, "()", 0, 2);
        let al = ArgumentList::empty(span);
        let got = al.span().unwrap();
        assert_eq!(got.text(), "()");
    }
}
