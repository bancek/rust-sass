// Copyright 2023 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/replace_expression.dart
// go-source: go/value/sass_replace_expression.go

use crate::common::source_span_span_with_context::SourceSpanWithContext;
use indexmap::IndexMap;

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::expression::Expression;
use crate::ast::sass::expression::ExpressionVisitor;
use crate::ast::sass::expression::IfConditionExpressionVisitor;
use crate::ast::sass::expression_binary_operation::BinaryOperationExpression;
use crate::ast::sass::expression_boolean::BooleanExpression;
use crate::ast::sass::expression_color::ColorExpression;
use crate::ast::sass::expression_function::FunctionExpression;
use crate::ast::sass::expression_if::{
    IfBranch, IfConditionExpression, IfConditionFunction, IfConditionNegation,
    IfConditionOperation, IfConditionParenthesized, IfConditionRaw, IfConditionSass, IfExpression,
};
use crate::ast::sass::expression_interpolated_function::InterpolatedFunctionExpression;
use crate::ast::sass::expression_legacy_if::LegacyIfExpression;
use crate::ast::sass::expression_list::ListExpression;
use crate::ast::sass::expression_map::MapExpression;
use crate::ast::sass::expression_null::NullExpression;
use crate::ast::sass::expression_number::NumberExpression;
use crate::ast::sass::expression_parenthesized::ParenthesizedExpression;
use crate::ast::sass::expression_selector::SelectorExpression;
use crate::ast::sass::expression_string::StringExpression;
use crate::ast::sass::expression_supports::SupportsExpression;
use crate::ast::sass::expression_unary_operation::UnaryOperationExpression;
use crate::ast::sass::expression_value::ValueExpression;
use crate::ast::sass::expression_variable::VariableExpression;
use crate::ast::sass::interpolation::{Interpolation, InterpolationPart};
use crate::ast::sass::supports_condition::{
    SupportsCondition, SupportsDeclaration, SupportsInterpolation, SupportsNegation,
    SupportsOperation,
};
use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};

/// A visitor that recursively traverses each expression in a SassScript AST
/// and replaces its contents with the values returned by nested recursion.
///
/// Identity for leaves (boolean, color, null, number, selector, value,
/// variable); rebuilds composite nodes bottom-up. It adds even more helpers:
///
/// * `replace_argument_list` — replaces each expression in an invocation; the
///   default visit methods call this to replace any argument invocation in an
///   expression.
/// * `replace_supports_condition` — replaces each expression in a condition;
///   the default visit methods call this to visit any [`SupportsCondition`]
///   they encounter. Returns `Err` for `Anything`/`Function`, which have no
///   replaceable expressions.
/// * `replace_interpolation` — replaces each expression in an interpolation;
///   the default visit methods call this to visit any interpolation in an
///   expression.
///
/// Each helper takes the calling visitor as `visitor` so overrides compose
/// through nested recursion.
pub struct ReplaceExpressionVisitor;

impl Default for ReplaceExpressionVisitor {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplaceExpressionVisitor {
    pub fn new() -> Self {
        ReplaceExpressionVisitor
    }

    pub fn replace_argument_list<
        'parse,
        V: ExpressionVisitor<'parse, Output = Expression<'parse>>,
    >(
        visitor: &mut V,
        args: &ArgumentList<'parse>,
    ) -> SassResult<ArgumentList<'parse>> {
        let mut positional = Vec::with_capacity(args.positional.len());
        for e in &args.positional {
            positional.push(e.accept(visitor)?);
        }
        let mut named = IndexMap::with_capacity(args.named.len());
        for (k, v) in &args.named {
            named.insert(k.clone(), v.accept(visitor)?);
        }
        let rest = match args.rest {
            Some(ref e) => Some(e.accept(visitor)?),
            None => None,
        };
        let keyword_rest = match args.keyword_rest {
            Some(ref e) => Some(e.accept(visitor)?),
            None => None,
        };
        Ok(ArgumentList::new(
            positional,
            named,
            args.named_spans.clone(),
            args.span,
            rest,
            keyword_rest,
        ))
    }

    pub fn replace_supports_condition<
        'parse,
        V: ExpressionVisitor<'parse, Output = Expression<'parse>>,
    >(
        visitor: &mut V,
        condition: &SupportsCondition<'parse>,
    ) -> SassResult<SupportsCondition<'parse>> {
        match condition {
            SupportsCondition::Operation(op) => {
                let left = Self::replace_supports_condition(visitor, &op.left)?;
                let right = Self::replace_supports_condition(visitor, &op.right)?;
                Ok(SupportsCondition::Operation(SupportsOperation::new(
                    left,
                    right,
                    op.operator,
                    op.span,
                )))
            }
            SupportsCondition::Negation(neg) => {
                let inner = Self::replace_supports_condition(visitor, &neg.condition)?;
                Ok(SupportsCondition::Negation(SupportsNegation::new(
                    inner, neg.span,
                )))
            }
            SupportsCondition::Interpolation(interp) => {
                let replaced = interp.expression.accept(visitor)?;
                let resolved_span = interp.span;
                Ok(SupportsCondition::Interpolation(
                    SupportsInterpolation::new(replaced, resolved_span),
                ))
            }
            SupportsCondition::Declaration(decl) => {
                let name = decl.name.accept(visitor)?;
                let value = decl.value.accept(visitor)?;
                Ok(SupportsCondition::Declaration(SupportsDeclaration::new(
                    name, value, decl.span,
                )))
            }
            // Dart `ReplaceExpressionVisitor.visitSupportsCondition` throws a
            // catchable `SassException("BUG: Unknown SupportsCondition …")` for
            // `SupportsAnything`/`SupportsFunction` — reachable via
            // `expression_to_calc` on a `SupportsExpression` wrapping those.
            // Must return `Err`, never panic (no-`panic` rule).
            SupportsCondition::Anything(_) | SupportsCondition::Function(_) => {
                let span = condition.span()?;
                Err(Box::new(SassError::Sass {
                    message: format!("BUG: Unknown SupportsCondition {condition}."),
                    span: SourceSpanWithContext::from_file_span(&span)?,
                    cause: None,
                    loaded_urls: vec![],
                }))
            }
        }
    }

    pub fn replace_interpolation<
        'parse,
        V: ExpressionVisitor<'parse, Output = Expression<'parse>>,
    >(
        visitor: &mut V,
        interp: &Interpolation<'parse>,
    ) -> SassResult<Interpolation<'parse>> {
        let mut new_contents = Vec::with_capacity(interp.contents.len());
        for part in &interp.contents {
            match part {
                InterpolationPart::Expression(expr) => {
                    let replaced = expr.accept(visitor)?;
                    new_contents.push(InterpolationPart::Expression(Box::new(replaced)));
                }
                InterpolationPart::Text(s) => {
                    new_contents.push(InterpolationPart::Text(s.clone()));
                }
            }
        }
        Interpolation::new(new_contents, interp.spans.clone(), interp.span.clone())
            .map_err(|e| Box::new(SassError::from(e)))
    }
}

impl<'parse> ExpressionVisitor<'parse> for ReplaceExpressionVisitor {
    type Output = Expression<'parse>;

    fn visit_binary_operation(
        &mut self,
        node: &BinaryOperationExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        let left = node.left.accept(self)?;
        let right = node.right.accept(self)?;
        Ok(Expression::BinaryOperation(BinaryOperationExpression::new(
            node.operator,
            left,
            right,
        )))
    }

    fn visit_boolean(
        &mut self,
        node: &BooleanExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        Ok(Expression::Boolean(node.clone()))
    }

    fn visit_color(&mut self, node: &ColorExpression<'parse>) -> SassResult<Expression<'parse>> {
        Ok(Expression::Color(node.clone()))
    }

    fn visit_function(
        &mut self,
        node: &FunctionExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        let args = Self::replace_argument_list(self, &node.arguments)?;
        Ok(Expression::Function(FunctionExpression::new(
            node.original_name.clone(),
            args,
            node.span,
            node.namespace.clone(),
        )))
    }

    fn visit_if(&mut self, node: &IfExpression<'parse>) -> SassResult<Expression<'parse>> {
        let mut new_branches = Vec::with_capacity(node.branches.len());
        for branch in &node.branches {
            let condition = match &branch.condition {
                Some(c) => Some(c.accept(self)?),
                None => None,
            };
            let expression = branch.expression.accept(self)?;
            new_branches.push(IfBranch {
                condition,
                expression,
            });
        }
        Ok(Expression::If(IfExpression::new(new_branches, node.span)?))
    }

    fn visit_interpolated_function(
        &mut self,
        node: &InterpolatedFunctionExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        let name = Self::replace_interpolation(self, &node.name)?;
        let args = Self::replace_argument_list(self, &node.arguments)?;
        Ok(Expression::InterpolatedFunction(
            InterpolatedFunctionExpression::new(name, args, node.span),
        ))
    }

    fn visit_legacy_if(
        &mut self,
        node: &LegacyIfExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        let args = Self::replace_argument_list(self, &node.arguments)?;
        Ok(Expression::LegacyIf(LegacyIfExpression::new(
            args, node.span,
        )))
    }

    fn visit_list(&mut self, node: &ListExpression<'parse>) -> SassResult<Expression<'parse>> {
        let mut contents = Vec::with_capacity(node.contents.len());
        for item in &node.contents {
            contents.push(item.accept(self)?);
        }
        Ok(Expression::List(ListExpression::new(
            contents,
            node.separator,
            node.span,
            node.has_brackets,
        )))
    }

    fn visit_map(&mut self, node: &MapExpression<'parse>) -> SassResult<Expression<'parse>> {
        let mut pairs = Vec::with_capacity(node.pairs.len());
        for (k, v) in &node.pairs {
            pairs.push((k.accept(self)?, v.accept(self)?));
        }
        Ok(Expression::Map(MapExpression::new(pairs, node.span)))
    }

    fn visit_null(&mut self, node: &NullExpression<'parse>) -> SassResult<Expression<'parse>> {
        Ok(Expression::Null(node.clone()))
    }

    fn visit_number(&mut self, node: &NumberExpression<'parse>) -> SassResult<Expression<'parse>> {
        Ok(Expression::Number(node.clone()))
    }

    fn visit_parenthesized(
        &mut self,
        node: &ParenthesizedExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        let expr = node.expression.accept(self)?;
        Ok(Expression::Parenthesized(ParenthesizedExpression::new(
            expr, node.span,
        )))
    }

    fn visit_selector(
        &mut self,
        node: &SelectorExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        Ok(Expression::Selector(node.clone()))
    }

    fn visit_string(&mut self, node: &StringExpression<'parse>) -> SassResult<Expression<'parse>> {
        let text = Self::replace_interpolation(self, &node.text)?;
        Ok(Expression::String(StringExpression::new(
            text,
            node.has_quotes,
        )))
    }

    fn visit_supports(
        &mut self,
        node: &SupportsExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        let cond = Self::replace_supports_condition(self, &node.condition)?;
        Ok(Expression::Supports(SupportsExpression::new(cond)))
    }

    fn visit_unary_operation(
        &mut self,
        node: &UnaryOperationExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        let operand = node.operand.accept(self)?;
        Ok(Expression::UnaryOperation(UnaryOperationExpression::new(
            node.operator,
            operand,
            node.span,
        )))
    }

    fn visit_value(&mut self, node: &ValueExpression<'parse>) -> SassResult<Expression<'parse>> {
        Ok(Expression::Value(node.clone()))
    }

    fn visit_variable(
        &mut self,
        node: &VariableExpression<'parse>,
    ) -> SassResult<Expression<'parse>> {
        Ok(Expression::Variable(node.clone()))
    }
}

impl<'parse> IfConditionExpressionVisitor<'parse> for ReplaceExpressionVisitor {
    type Output = IfConditionExpression<'parse>;

    fn visit_parenthesized(
        &mut self,
        node: &IfConditionParenthesized<'parse>,
    ) -> SassResult<IfConditionExpression<'parse>> {
        let expr = node.expression.accept(self)?;
        Ok(IfConditionExpression::Parenthesized(
            IfConditionParenthesized::new(expr, node.span),
        ))
    }

    fn visit_negation(
        &mut self,
        node: &IfConditionNegation<'parse>,
    ) -> SassResult<IfConditionExpression<'parse>> {
        let expr = node.expression.accept(self)?;
        Ok(IfConditionExpression::Negation(IfConditionNegation::new(
            expr, node.span,
        )))
    }

    fn visit_operation(
        &mut self,
        node: &IfConditionOperation<'parse>,
    ) -> SassResult<IfConditionExpression<'parse>> {
        let mut exprs = Vec::with_capacity(node.expressions.len());
        for e in &node.expressions {
            exprs.push(e.accept(self)?);
        }
        Ok(IfConditionExpression::Operation(
            IfConditionOperation::new(exprs, node.operator)
                .map_err(|e| Box::new(SassError::from(e)))?,
        ))
    }

    fn visit_function(
        &mut self,
        node: &IfConditionFunction<'parse>,
    ) -> SassResult<IfConditionExpression<'parse>> {
        let name = Self::replace_interpolation(self, &node.name)?;
        let arguments = Self::replace_interpolation(self, &node.arguments)?;
        Ok(IfConditionExpression::Function(Box::new(
            IfConditionFunction::new(name, arguments, node.span),
        )))
    }

    fn visit_sass(
        &mut self,
        node: &IfConditionSass<'parse>,
    ) -> SassResult<IfConditionExpression<'parse>> {
        let expr = node.expression.accept(self)?;
        Ok(IfConditionExpression::Sass(IfConditionSass::new(
            expr, node.span,
        )))
    }

    fn visit_raw(
        &mut self,
        node: &IfConditionRaw<'parse>,
    ) -> SassResult<IfConditionExpression<'parse>> {
        let text = Self::replace_interpolation(self, &node.text)?;
        Ok(IfConditionExpression::Raw(IfConditionRaw::new(text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::binary_operator::BinaryOperator;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_boolean::BooleanExpression;
    use crate::ast::sass::expression_if::BooleanOperator;
    use crate::ast::sass::expression_list::ListExpression;
    use crate::ast::sass::expression_null::NullExpression;
    use crate::ast::sass::expression_number::NumberExpression;
    use crate::ast::sass::expression_string::StringExpression;
    use crate::ast::sass::expression_variable::VariableExpression;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::ast::sass::supports_condition::{
        SupportsAnything, SupportsCondition, SupportsDeclaration, SupportsFunction,
        SupportsOperation,
    };
    use crate::ast::sass::visitor::expression_to_calc::expression_to_calc;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::value::ListSeparator;
    use bumpalo::Bump;
    use indexmap::IndexMap;

    fn test_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    #[test]
    fn test_replace_binary_operation() {
        let arena = Bump::new();
        let mut v = ReplaceExpressionVisitor::new();
        let left = Expression::Boolean(BooleanExpression::new(true, test_span(&arena, "true")));
        let right = Expression::Boolean(BooleanExpression::new(false, test_span(&arena, "false")));
        let node = BinaryOperationExpression::new(BinaryOperator::And, left, right);
        let result = Expression::BinaryOperation(node).accept(&mut v).unwrap();
        assert!(matches!(result, Expression::BinaryOperation(_)));
    }

    #[test]
    fn test_replace_boolean_identity() {
        let arena = Bump::new();
        let mut v = ReplaceExpressionVisitor::new();
        let node = BooleanExpression::new(true, test_span(&arena, "true"));
        let result = Expression::Boolean(node.clone()).accept(&mut v).unwrap();
        assert!(matches!(result, Expression::Boolean(_)));
    }

    #[test]
    fn test_replace_null_identity() {
        let arena = Bump::new();
        let mut v = ReplaceExpressionVisitor::new();
        let node = NullExpression::new(test_span(&arena, "null"));
        let result = Expression::Null(node).accept(&mut v).unwrap();
        assert!(matches!(result, Expression::Null(_)));
    }

    #[test]
    fn test_replace_number_identity() {
        let arena = Bump::new();
        let mut v = ReplaceExpressionVisitor::new();
        let node = NumberExpression::new(1.0, test_span(&arena, "1"), None);
        let result = Expression::Number(node).accept(&mut v).unwrap();
        assert!(matches!(result, Expression::Number(_)));
    }

    #[test]
    fn test_replace_variable_identity() {
        let arena = Bump::new();
        let mut v = ReplaceExpressionVisitor::new();
        let node = VariableExpression::new("x".into(), test_span(&arena, "$x"), None);
        let result = Expression::Variable(node).accept(&mut v).unwrap();
        assert!(matches!(result, Expression::Variable(_)));
    }

    #[test]
    fn test_replace_function() {
        let arena = Bump::new();
        let mut v = ReplaceExpressionVisitor::new();
        let span = test_span(&arena, "fn(1)");
        let args = ArgumentList::new(
            vec![Expression::Number(NumberExpression::new(
                1.0,
                test_span(&arena, "1"),
                None,
            ))],
            IndexMap::new(),
            IndexMap::new(),
            span,
            None,
            None,
        );
        let node = FunctionExpression::new("fn".into(), args, span, None);
        let result = Expression::Function(node).accept(&mut v).unwrap();
        match result {
            Expression::Function(fe) => {
                assert_eq!(fe.original_name, "fn");
            }
            _ => panic!("expected FunctionExpression"),
        }
    }

    #[test]
    fn test_replace_list() {
        let arena = Bump::new();
        let mut v = ReplaceExpressionVisitor::new();
        let span = test_span(&arena, "(a, b)");
        let items = vec![
            Expression::String(StringExpression::plain("a", test_span(&arena, "a"), true)),
            Expression::String(StringExpression::plain("b", test_span(&arena, "b"), true)),
        ];
        let node = ListExpression::new(items, ListSeparator::Comma, span, true);
        let result = Expression::List(node).accept(&mut v).unwrap();
        match result {
            Expression::List(le) => {
                assert_eq!(le.contents.len(), 2);
            }
            _ => panic!("expected ListExpression"),
        }
    }

    #[test]
    fn test_replace_map() {
        let arena = Bump::new();
        let mut v = ReplaceExpressionVisitor::new();
        let span = test_span(&arena, "(a: 1)");
        let node = MapExpression::new(
            vec![(
                Expression::String(StringExpression::plain("a", test_span(&arena, "a"), false)),
                Expression::Number(NumberExpression::new(1.0, test_span(&arena, "1"), None)),
            )],
            span,
        );
        let result = Expression::Map(node).accept(&mut v).unwrap();
        match result {
            Expression::Map(me) => {
                assert_eq!(me.pairs.len(), 1);
            }
            _ => panic!("expected MapExpression"),
        }
    }

    #[test]
    fn test_replace_string() {
        let arena = Bump::new();
        let mut v = ReplaceExpressionVisitor::new();
        let text = Interpolation::plain("hello".into(), test_span(&arena, "hello"));
        let node = StringExpression::new(text, true);
        let result = Expression::String(node).accept(&mut v).unwrap();
        match result {
            Expression::String(se) => {
                assert!(se.has_quotes);
            }
            _ => panic!("expected StringExpression"),
        }
    }

    #[test]
    fn test_replace_parenthesized() {
        let arena = Bump::new();
        let mut v = ReplaceExpressionVisitor::new();
        let inner = Expression::Number(NumberExpression::new(1.0, test_span(&arena, "1"), None));
        let node = ParenthesizedExpression::new(inner, test_span(&arena, "(1)"));
        let result = Expression::Parenthesized(node).accept(&mut v).unwrap();
        assert!(matches!(result, Expression::Parenthesized(_)));
    }

    #[test]
    fn test_replace_supports_condition() {
        let arena = Bump::new();
        let mut v = ReplaceExpressionVisitor::new();
        let left = SupportsCondition::Declaration(SupportsDeclaration::new(
            Expression::String(StringExpression::plain("a", test_span(&arena, "a"), false)),
            Expression::String(StringExpression::plain("b", test_span(&arena, "b"), false)),
            test_span(&arena, "(a: b)"),
        ));
        let right = SupportsCondition::Declaration(SupportsDeclaration::new(
            Expression::String(StringExpression::plain("c", test_span(&arena, "c"), false)),
            Expression::String(StringExpression::plain("d", test_span(&arena, "d"), false)),
            test_span(&arena, "(c: d)"),
        ));
        let cond = SupportsCondition::Operation(SupportsOperation::new(
            left,
            right,
            BooleanOperator::And,
            test_span(&arena, "(a: b) and (c: d)"),
        ));
        let result = ReplaceExpressionVisitor::replace_supports_condition(&mut v, &cond).unwrap();
        assert!(matches!(result, SupportsCondition::Operation(_)));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_unsupported_condition_returns_err() {
        // Wrapping `SupportsFunction` in a
        // `SupportsExpression` and calling `expression_to_calc` must return a
        // catchable error (Dart throws `SassException`), not abort the
        // process via `unreachable!()/panic!`.
        let arena = Bump::new();
        let mut v = ReplaceExpressionVisitor::new();
        let name = Interpolation::plain("unknown-fn".into(), test_span(&arena, "unknown-fn"));
        let args = Interpolation::plain("(1)".into(), test_span(&arena, "(1)"));
        let cond = SupportsCondition::Function(SupportsFunction::new(
            name,
            args,
            test_span(&arena, "unknown-fn(1)"),
        ));
        let err = ReplaceExpressionVisitor::replace_supports_condition(&mut v, &cond)
            .expect_err("SupportsFunction must error, not panic");
        match *err {
            SassError::Sass { message, .. } => assert!(
                message.starts_with("BUG: Unknown SupportsCondition"),
                "unexpected message: {message:?}"
            ),
            other => panic!("expected SassError::Sass, got {other:?}"),
        }
        // Same via the Anything variant.
        let anything = SupportsCondition::Anything(SupportsAnything::new(
            Interpolation::plain("x".into(), test_span(&arena, "x")),
            test_span(&arena, "(x)"),
        ));
        let err = ReplaceExpressionVisitor::replace_supports_condition(&mut v, &anything)
            .expect_err("SupportsAnything must error, not panic");
        match *err {
            SassError::Sass { message, .. } => assert!(
                message.starts_with("BUG: Unknown SupportsCondition"),
                "unexpected message: {message:?}"
            ),
            other => panic!("expected SassError::Sass, got {other:?}"),
        }
        // And end-to-end through expression_to_calc on a SupportsExpression.
        let expr = Expression::Supports(SupportsExpression::new(cond));
        let err = expression_to_calc(&expr)
            .expect_err("expression_to_calc over SupportsFunction must error");
        match *err {
            SassError::Sass { message, .. } => assert!(
                message.starts_with("BUG: Unknown SupportsCondition"),
                "unexpected message: {message:?}"
            ),
            other => panic!("expected SassError::Sass, got {other:?}"),
        }
    }

    #[test]
    fn test_replace_interpolation() {
        let arena = Bump::new();
        let mut v = ReplaceExpressionVisitor::new();
        let span = test_span(&arena, "a #{$x} b");
        let expr_span = test_span(&arena, "#{$x}");
        let interp = Interpolation::new(
            vec![
                InterpolationPart::Text("a ".into()),
                InterpolationPart::Expression(Box::new(Expression::Variable(
                    VariableExpression::new("x".into(), test_span(&arena, "$x"), None),
                ))),
                InterpolationPart::Text(" b".into()),
            ],
            vec![None, Some(expr_span), None],
            span,
        )
        .unwrap();
        let node = StringExpression::new(interp, false);
        let result = Expression::String(node).accept(&mut v).unwrap();
        assert!(matches!(result, Expression::String(_)));
    }
}
