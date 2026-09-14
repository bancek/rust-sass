// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/legacy_if.dart
// go-source: go/value/sass_expression_legacy_if.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::expression::Expression;
use crate::ast::sass::interpolation::Interpolation;

/// A ternary expression.
///
/// This is defined as a separate syntactic construct rather than a normal
/// function because only one of the `$if-true` and `$if-false` arguments is
/// evaluated.
#[derive(Clone, Debug)]
pub struct LegacyIfExpression<'parse> {
    /// The arguments passed to `if()`.
    pub arguments: ArgumentList<'parse>,
    pub span: FileSpan<'parse>,
}

impl<'parse> LegacyIfExpression<'parse> {
    pub fn new(arguments: ArgumentList<'parse>, span: FileSpan<'parse>) -> Self {
        LegacyIfExpression { arguments, span }
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }

    // Returns a modern `if()` expression to use instead of `self`.
    //
    // Matches Dart: `modernSuggestion` (@nodoc/@internal).
    pub fn modern_suggestion(&self) -> SassResult<Option<String>> {
        if self.arguments.positional.len() == 3
            && self.arguments.named.is_empty()
            && self.arguments.rest.is_none()
        {
            let cond = Expression::to_display_string(&self.arguments.positional[0])?;
            let if_true = Expression::to_display_string(&self.arguments.positional[1])?;
            let if_false_val = &self.arguments.positional[2];
            let if_false = Expression::to_display_string(if_false_val)?;

            if matches!(if_false_val, Expression::Null(_)) {
                Ok(Some(format!("if(sass({cond}): {if_true})")))
            } else if matches!(&self.arguments.positional[1], Expression::Null(_)) {
                Ok(Some(format!("if(not sass({cond}): {if_false})")))
            } else {
                Ok(Some(format!(
                    "if(sass({cond}): {if_true}; else: {if_false})"
                )))
            }
        } else {
            Ok(None)
        }
    }
}

impl<'parse> AstNode<'parse> for LegacyIfExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> LegacyIfExpression<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "if{}", self.arguments).unwrap();
        Ok(buf)
    }
}

impl fmt::Display for LegacyIfExpression<'_> {
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
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_null::NullExpression;
    use crate::ast::sass::expression_number::NumberExpression;
    use crate::ast::sass::expression_string::StringExpression;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;
    use indexmap::IndexMap;

    #[test]
    fn test_construction() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "x", None);
        let span = FileSpan::new(Some(fs), 0, 1);
        let expr = LegacyIfExpression::new(ArgumentList::empty(span), span);
        expr.span().unwrap();
    }

    #[test]
    fn test_source_interpolation() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "x", None);
        let span = FileSpan::new(Some(fs), 0, 1);
        let expr = LegacyIfExpression::new(ArgumentList::empty(span), span);
        assert!(expr.source_interpolation().is_none());
    }

    #[test]
    fn test_modern_suggestion_three_args() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "modern", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let cond = Expression::String(StringExpression::plain("$cond", span, false));
        let if_true = Expression::Number(NumberExpression::new(1.0, span, None));
        let if_false = Expression::Number(NumberExpression::new(2.0, span, None));
        let args = ArgumentList::new(
            vec![cond, if_true, if_false],
            IndexMap::new(),
            IndexMap::new(),
            span,
            None,
            None,
        );
        let expr = LegacyIfExpression::new(args, span);
        let got = expr.modern_suggestion().unwrap();
        assert_eq!(got.as_deref(), Some("if(sass($cond): 1; else: 2)"));
    }

    #[test]
    fn test_modern_suggestion_null_else() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "modern", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let cond = Expression::String(StringExpression::plain("$cond", span, false));
        let if_true = Expression::Number(NumberExpression::new(1.0, span, None));
        let if_false = Expression::Null(NullExpression::new(span));
        let args = ArgumentList::new(
            vec![cond, if_true, if_false],
            IndexMap::new(),
            IndexMap::new(),
            span,
            None,
            None,
        );
        let expr = LegacyIfExpression::new(args, span);
        let got = expr.modern_suggestion().unwrap();
        assert_eq!(got.as_deref(), Some("if(sass($cond): 1)"));
    }

    #[test]
    fn test_modern_suggestion_null_if_true() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "modern", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let cond = Expression::String(StringExpression::plain("$cond", span, false));
        let if_true = Expression::Null(NullExpression::new(span));
        let if_false = Expression::Number(NumberExpression::new(2.0, span, None));
        let args = ArgumentList::new(
            vec![cond, if_true, if_false],
            IndexMap::new(),
            IndexMap::new(),
            span,
            None,
            None,
        );
        let expr = LegacyIfExpression::new(args, span);
        let got = expr.modern_suggestion().unwrap();
        assert_eq!(got.as_deref(), Some("if(not sass($cond): 2)"));
    }

    #[test]
    fn test_modern_suggestion_wrong_args() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "modern", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let cond = Expression::String(StringExpression::plain("$cond", span, false));
        let args = ArgumentList::new(
            vec![cond],
            IndexMap::new(),
            IndexMap::new(),
            span,
            None,
            None,
        );
        let expr = LegacyIfExpression::new(args, span);
        assert!(expr.modern_suggestion().unwrap().is_none());
    }

    #[test]
    fn test_modern_suggestion_with_named() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "modern", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let cond = Expression::String(StringExpression::plain("$cond", span, false));
        let if_true = Expression::Number(NumberExpression::new(1.0, span, None));
        let if_false = Expression::Number(NumberExpression::new(2.0, span, None));
        let mut named = IndexMap::new();
        named.insert(
            "extra".into(),
            Expression::Number(NumberExpression::new(3.0, span, None)),
        );
        let args = ArgumentList::new(
            vec![cond, if_true, if_false],
            named,
            IndexMap::new(),
            span,
            None,
            None,
        );
        let expr = LegacyIfExpression::new(args, span);
        assert!(expr.modern_suggestion().unwrap().is_none());
    }
}
