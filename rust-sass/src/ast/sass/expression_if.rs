// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/if.dart
// dart-source: lib/src/ast/sass/boolean_operator.dart (BooleanOperator, folded into this file)
// go-source: go/value/sass_expression_if.go

use crate::common::span::Span;
use crate::common::span_error::SpanError;
use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::core_errors::ArgumentError;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::common::source_span_span_with_context::SourceSpanWithContext;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression::IfConditionExpressionVisitor;
use crate::ast::sass::interpolation::Interpolation;
use crate::ast::sass::interpolation_buffer::InterpolationBuffer;

// --- If Branch ---

/// A single conditional branch of an [`IfExpression`].
//
// Rust-only shape (matches Go's `IfBranch`): Dart models branches as
// `(IfConditionExpression?, Expression)` tuples.
#[derive(Clone, Debug)]
pub struct IfBranch<'parse> {
    /// The branch condition, or [`None`] for an `else` branch that is always
    /// evaluated.
    pub condition: Option<IfConditionExpression<'parse>>,
    pub expression: Expression<'parse>,
}

// --- If Expression ---

/// A CSS `if()` expression.
///
/// In addition to supporting the plain-CSS syntax, this supports a `sass()`
/// condition that evaluates SassScript expressions.
#[derive(Clone, Debug)]
pub struct IfExpression<'parse> {
    /// The conditional branches that make up the `if()`.
    ///
    /// A [`None`] condition indicates an `else` branch that is always
    /// evaluated.
    pub branches: Vec<IfBranch<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> IfExpression<'parse> {
    pub fn new(
        branches: Vec<IfBranch<'parse>>,
        span: FileSpan<'parse>,
    ) -> Result<Self, ArgumentError> {
        if branches.is_empty() {
            return Err(ArgumentError {
                name: Some("branches".into()),
                message: "branches may not be empty".into(),
            });
        }
        Ok(IfExpression { branches, span })
    }

    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        None
    }
}

impl<'parse> AstNode<'parse> for IfExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> IfExpression<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "if(").unwrap();
        for (i, branch) in self.branches.iter().enumerate() {
            if i > 0 {
                write!(buf, "; ").unwrap();
            }
            if let Some(ref cond) = branch.condition {
                write!(buf, "{}", cond).unwrap();
            } else {
                write!(buf, "else").unwrap();
            }
            write!(buf, ": {}", branch.expression.to_display_string()?).unwrap();
        }
        write!(buf, ")").unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for IfExpression<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

// --- IfConditionExpression Enum ---

/// A condition in an [`IfExpression`].
///
/// Dart models this as a sealed class hierarchy; Rust folds the six subclasses
/// into variants of this enum.
#[derive(Clone, Debug)]
pub enum IfConditionExpression<'parse> {
    Parenthesized(IfConditionParenthesized<'parse>),
    Negation(IfConditionNegation<'parse>),
    Operation(IfConditionOperation<'parse>),
    Function(Box<IfConditionFunction<'parse>>),
    Sass(IfConditionSass<'parse>),
    Raw(IfConditionRaw<'parse>),
}

impl<'parse> IfConditionExpression<'parse> {
    pub fn accept<V: IfConditionExpressionVisitor<'parse> + ?Sized>(
        &self,
        visitor: &mut V,
    ) -> SassResult<V::Output> {
        match self {
            IfConditionExpression::Parenthesized(c) => visitor.visit_parenthesized(c),
            IfConditionExpression::Negation(c) => visitor.visit_negation(c),
            IfConditionExpression::Operation(c) => visitor.visit_operation(c),
            IfConditionExpression::Function(c) => visitor.visit_function(c),
            IfConditionExpression::Sass(c) => visitor.visit_sass(c),
            IfConditionExpression::Raw(c) => visitor.visit_raw(c),
        }
    }

    // Converts this condition into an interpolation that produces the same
    // value.
    //
    // Matches Dart: `IfConditionExpression.toInterpolation`
    // (@nodoc/@internal). Throws when the condition contains an
    // [`IfConditionSass`]; the passed node's span is used for that error.
    pub fn to_interpolation(
        &self,
        arbitrary_substitution: &dyn AstNode<'parse>,
    ) -> SassResult<Interpolation<'parse>> {
        match self {
            IfConditionExpression::Parenthesized(c) => c.to_interpolation(arbitrary_substitution),
            IfConditionExpression::Negation(c) => c.to_interpolation(arbitrary_substitution),
            IfConditionExpression::Operation(c) => c.to_interpolation(arbitrary_substitution),
            IfConditionExpression::Function(c) => c.to_interpolation(arbitrary_substitution),
            IfConditionExpression::Sass(c) => c.to_interpolation(arbitrary_substitution),
            IfConditionExpression::Raw(c) => c.to_interpolation(arbitrary_substitution),
        }
    }

    // Returns whether this is an arbitrary substitution condition which may be
    // replaced with multiple tokens at evaluation or render time.
    //
    // Matches Dart: `IfConditionExpression.isArbitrarySubstitution`
    // (@nodoc/@internal).
    pub fn is_arbitrary_substitution(&self) -> bool {
        match self {
            IfConditionExpression::Parenthesized(c) => c.is_arbitrary_substitution(),
            IfConditionExpression::Negation(c) => c.is_arbitrary_substitution(),
            IfConditionExpression::Operation(c) => c.is_arbitrary_substitution(),
            IfConditionExpression::Function(c) => c.is_arbitrary_substitution(),
            IfConditionExpression::Sass(c) => c.is_arbitrary_substitution(),
            IfConditionExpression::Raw(c) => c.is_arbitrary_substitution(),
        }
    }
}

impl<'parse> AstNode<'parse> for IfConditionExpression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            IfConditionExpression::Parenthesized(c) => c.span(),
            IfConditionExpression::Negation(c) => c.span(),
            IfConditionExpression::Operation(c) => c.span(),
            IfConditionExpression::Function(c) => c.span(),
            IfConditionExpression::Sass(c) => c.span(),
            IfConditionExpression::Raw(c) => c.span(),
        }
    }
}

impl<'parse> IfConditionExpression<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        match self {
            IfConditionExpression::Parenthesized(c) => c.to_display_string(),
            IfConditionExpression::Negation(c) => c.to_display_string(),
            IfConditionExpression::Operation(c) => c.to_display_string(),
            IfConditionExpression::Function(c) => c.as_ref().to_display_string(),
            IfConditionExpression::Sass(c) => c.to_display_string(),
            IfConditionExpression::Raw(c) => c.to_display_string(),
        }
    }
}

impl<'parse> fmt::Display for IfConditionExpression<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

// --- IfConditionParenthesized ---

/// A parenthesized condition.
#[derive(Clone, Debug)]
pub struct IfConditionParenthesized<'parse> {
    /// The parenthesized expression.
    pub expression: Box<IfConditionExpression<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> IfConditionParenthesized<'parse> {
    pub fn new(expression: IfConditionExpression<'parse>, span: FileSpan<'parse>) -> Self {
        IfConditionParenthesized {
            expression: Box::new(expression),
            span,
        }
    }

    pub fn is_arbitrary_substitution(&self) -> bool {
        false
    }

    pub fn to_interpolation(
        &self,
        arbitrary_substitution: &dyn AstNode<'parse>,
    ) -> SassResult<Interpolation<'parse>> {
        let mut buf = InterpolationBuffer::new();
        buf.write_char_code('(');
        let inner = self.expression.to_interpolation(arbitrary_substitution)?;
        buf.add_interpolation(&inner);
        buf.write_char_code(')');
        let span = self.span()?;
        buf.interpolation(span)
    }
}

impl<'parse> AstNode<'parse> for IfConditionParenthesized<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> IfConditionParenthesized<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "({})", self.expression).unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for IfConditionParenthesized<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

// --- IfConditionNegation ---

/// A negated condition.
#[derive(Clone, Debug)]
pub struct IfConditionNegation<'parse> {
    /// The expression negated by this.
    pub expression: Box<IfConditionExpression<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> IfConditionNegation<'parse> {
    pub fn new(expression: IfConditionExpression<'parse>, span: FileSpan<'parse>) -> Self {
        IfConditionNegation {
            expression: Box::new(expression),
            span,
        }
    }

    pub fn is_arbitrary_substitution(&self) -> bool {
        false
    }

    pub fn to_interpolation(
        &self,
        arbitrary_substitution: &dyn AstNode<'parse>,
    ) -> SassResult<Interpolation<'parse>> {
        let mut buf = InterpolationBuffer::new();
        buf.write("not ");
        let inner = self.expression.to_interpolation(arbitrary_substitution)?;
        buf.add_interpolation(&inner);
        let span = self.span()?;
        buf.interpolation(span)
    }
}

impl<'parse> AstNode<'parse> for IfConditionNegation<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> IfConditionNegation<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "not {}", self.expression).unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for IfConditionNegation<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

// --- BooleanOperator ---
//
// Matches Dart: `BooleanOperator` (`lib/src/ast/sass/boolean_operator.dart`),
// folded into this file. Currently CSS only supports conjunctions (`and`) and
// disjunctions (`or`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanOperator {
    And,
    Or,
}

impl BooleanOperator {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        match self {
            BooleanOperator::And => write!(buf, "and").unwrap(),
            BooleanOperator::Or => write!(buf, "or").unwrap(),
        }
        Ok(buf)
    }
}

impl fmt::Display for BooleanOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

// --- IfConditionOperation ---

/// A sequence of `and`s or `or`s.
#[derive(Clone, Debug)]
pub struct IfConditionOperation<'parse> {
    /// The expressions conjoined or disjoined by this operation.
    pub expressions: Vec<IfConditionExpression<'parse>>,
    pub operator: BooleanOperator,
}

impl<'parse> IfConditionOperation<'parse> {
    pub fn new(
        expressions: Vec<IfConditionExpression<'parse>>,
        operator: BooleanOperator,
    ) -> Result<Self, ArgumentError> {
        if expressions.len() < 2 {
            return Err(ArgumentError {
                name: Some("expressions".into()),
                message: "expressions must have length >= 2".into(),
            });
        }
        Ok(IfConditionOperation {
            expressions,
            operator,
        })
    }

    pub fn is_arbitrary_substitution(&self) -> bool {
        false
    }

    pub fn to_interpolation(
        &self,
        arbitrary_substitution: &dyn AstNode<'parse>,
    ) -> SassResult<Interpolation<'parse>> {
        let mut buf = InterpolationBuffer::new();
        for (i, expr) in self.expressions.iter().enumerate() {
            if i > 0 {
                buf.write_char_code(' ');
                buf.write(&format!("{}", self.operator));
                buf.write_char_code(' ');
            }
            let inner = expr.to_interpolation(arbitrary_substitution)?;
            buf.add_interpolation(&inner);
        }
        let span = self.span()?;
        buf.interpolation(span)
    }
}

impl<'parse> AstNode<'parse> for IfConditionOperation<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        let first = self.expressions[0].span()?;
        let last = self.expressions[self.expressions.len() - 1].span()?;
        first.expand(&Span::File(last)).map_err(|e| match e {
            SpanError::Sass(e) => e,
            _ => Box::new(SassError::Script {
                message: "span expansion failed".into(),
                argument_name: None,
            }),
        })
    }
}

impl<'parse> IfConditionOperation<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        for (i, expr) in self.expressions.iter().enumerate() {
            if i > 0 {
                write!(buf, " {} ", self.operator).unwrap();
            }
            write!(buf, "{}", expr).unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for IfConditionOperation<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

// --- IfConditionFunction ---

/// A plain-CSS function-style condition.
#[derive(Clone, Debug)]
pub struct IfConditionFunction<'parse> {
    /// The name of the function being called.
    pub name: Interpolation<'parse>,
    /// The arguments passed to the function call.
    pub arguments: Interpolation<'parse>,
    pub span: FileSpan<'parse>,
}

impl<'parse> IfConditionFunction<'parse> {
    pub fn new(
        name: Interpolation<'parse>,
        arguments: Interpolation<'parse>,
        span: FileSpan<'parse>,
    ) -> Self {
        IfConditionFunction {
            name,
            arguments,
            span,
        }
    }

    pub fn is_arbitrary_substitution(&self) -> bool {
        if let Some(plain) = self.name.as_plain() {
            let lower = plain.to_lowercase();
            return lower == "if" || lower == "var" || lower == "attr" || lower.starts_with("--");
        }
        false
    }

    pub fn to_interpolation(
        &self,
        _arbitrary_substitution: &dyn AstNode<'parse>,
    ) -> SassResult<Interpolation<'parse>> {
        let mut buf = InterpolationBuffer::new();
        buf.add_interpolation(&self.name);
        buf.write_char_code('(');
        buf.add_interpolation(&self.arguments);
        buf.write_char_code(')');
        let span = self.span()?;
        buf.interpolation(span)
    }
}

impl<'parse> AstNode<'parse> for IfConditionFunction<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> IfConditionFunction<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "{}({})", self.name, self.arguments).unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for IfConditionFunction<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

// --- IfConditionSass ---

/// A Sass condition that will evaluate to true or false at compile time.
#[derive(Clone, Debug)]
pub struct IfConditionSass<'parse> {
    /// The expression that determines whether this condition matches.
    pub expression: Box<Expression<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> IfConditionSass<'parse> {
    pub fn new(expression: Expression<'parse>, span: FileSpan<'parse>) -> Self {
        IfConditionSass {
            expression: Box::new(expression),
            span,
        }
    }

    pub fn is_arbitrary_substitution(&self) -> bool {
        false
    }

    pub fn to_interpolation(
        &self,
        arbitrary_substitution: &dyn AstNode<'parse>,
    ) -> SassResult<Interpolation<'parse>> {
        let sub_span = arbitrary_substitution.span()?;
        let c_span = self.span()?;
        Err(Box::new(SassError::MultiSpan {
            message:
                "if() conditions with arbitrary substitutions may not contain sass() expressions."
                    .into(),
            span: SourceSpanWithContext::from_file_span(&sub_span)?,
            primary_label: Some("arbitrary substitution".into()),
            secondary: vec![(
                SourceSpanWithContext::from_file_span(&c_span)?,
                "sass() expression".into(),
            )],
            original_source: None,
            cause: None,
            loaded_urls: vec![],
            trace: Default::default(),
        }))
    }
}

impl<'parse> AstNode<'parse> for IfConditionSass<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> IfConditionSass<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(
            buf,
            "sass({})",
            self.expression.as_ref().to_display_string()?
        )
        .unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for IfConditionSass<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

// --- IfConditionRaw ---

/// A chunk of raw text, possibly with interpolations.
///
/// This is used to represent explicit interpolation, as well as whole
/// expressions where arbitrary substitutions are used in place of operators.
#[derive(Clone, Debug)]
pub struct IfConditionRaw<'parse> {
    /// The text that encompasses this condition.
    pub text: Interpolation<'parse>,
}

impl<'parse> IfConditionRaw<'parse> {
    pub fn new(text: Interpolation<'parse>) -> Self {
        IfConditionRaw { text }
    }

    pub fn is_arbitrary_substitution(&self) -> bool {
        true
    }

    pub fn to_interpolation(
        &self,
        _arbitrary_substitution: &dyn AstNode<'parse>,
    ) -> SassResult<Interpolation<'parse>> {
        Ok(self.text.clone())
    }
}

impl<'parse> AstNode<'parse> for IfConditionRaw<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        self.text.span()
    }
}

impl<'parse> IfConditionRaw<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "{}", self.text).unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for IfConditionRaw<'parse> {
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
    fn test_if_construction() {
        let arena = Bump::new();
        let span = test_span(&arena, "x", 0, 1);
        let branches = vec![IfBranch {
            condition: Some(IfConditionExpression::Sass(IfConditionSass::new(
                Expression::Boolean(BooleanExpression::new(true, span)),
                span,
            ))),
            expression: Expression::Boolean(BooleanExpression::new(true, span)),
        }];
        let expr = IfExpression::new(branches, span).unwrap();
        assert_eq!(expr.branches.len(), 1);
    }

    #[test]
    fn test_if_empty_branches_error() {
        let arena = Bump::new();
        let span = test_span(&arena, "", 0, 0);
        assert!(IfExpression::new(vec![], span).is_err());
    }

    #[test]
    fn test_if_condition_display() {
        let arena = Bump::new();
        let span = test_span(&arena, "true", 0, 4);
        let cond = IfConditionExpression::Sass(IfConditionSass::new(
            Expression::Boolean(BooleanExpression::new(true, span)),
            span,
        ));
        assert_eq!(format!("{cond}"), "sass(true)");
    }

    #[test]
    fn test_if_condition_parenthesized() {
        let arena = Bump::new();
        let span = test_span(&arena, "true", 0, 4);
        let inner = IfConditionExpression::Sass(IfConditionSass::new(
            Expression::Boolean(BooleanExpression::new(true, span)),
            span,
        ));
        let cond = IfConditionParenthesized::new(inner, span);
        assert_eq!(format!("{cond}"), "(sass(true))");
    }

    #[test]
    fn test_if_condition_negation() {
        let arena = Bump::new();
        let span = test_span(&arena, "true", 0, 4);
        let inner = IfConditionExpression::Sass(IfConditionSass::new(
            Expression::Boolean(BooleanExpression::new(true, span)),
            span,
        ));
        let cond = IfConditionNegation::new(inner, span);
        assert_eq!(format!("{cond}"), "not sass(true)");
    }
}
