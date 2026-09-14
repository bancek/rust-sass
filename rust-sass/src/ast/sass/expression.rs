// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression.dart + lib/src/visitor/interface/expression.dart (ExpressionVisitor) + lib/src/visitor/interface/if_condition_expression.dart (IfConditionExpressionVisitor)
// go-source: go/value/sass_expression.go

use crate::ast::sass::expression_if::IfConditionFunction;
use crate::ast::sass::expression_if::IfConditionNegation;
use crate::ast::sass::expression_if::IfConditionOperation;
use crate::ast::sass::expression_if::IfConditionParenthesized;
use crate::ast::sass::expression_if::IfConditionRaw;
use crate::ast::sass::expression_if::IfConditionSass;
use crate::ast::sass::visitor::is_calculation_safe::IsCalculationSafeVisitor;
use crate::ast::sass::visitor::is_plain_css::IsPlainCssVisitor;
use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::expression_binary_operation::BinaryOperationExpression;
use crate::ast::sass::expression_boolean::BooleanExpression;
use crate::ast::sass::expression_color::ColorExpression;
use crate::ast::sass::expression_function::FunctionExpression;
use crate::ast::sass::expression_if::IfExpression;
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
use crate::ast::sass::interpolation::Interpolation;

/// A SassScript expression in a Sass syntax tree.
///
/// One variant per expression node type (18 — every file under
/// `expression_*.rs` plus the value/variable leaves). Dispatch through
/// [`accept`](Self::accept) with an [`ExpressionVisitor`]; the evaluator
/// instead matches directly in free functions (see `architecture.md` §4).
#[derive(Clone, Debug)]
pub enum Expression<'parse> {
    BinaryOperation(BinaryOperationExpression<'parse>),
    Boolean(BooleanExpression<'parse>),
    Color(ColorExpression<'parse>),
    Function(FunctionExpression<'parse>),
    If(IfExpression<'parse>),
    InterpolatedFunction(InterpolatedFunctionExpression<'parse>),
    LegacyIf(LegacyIfExpression<'parse>),
    List(ListExpression<'parse>),
    Map(MapExpression<'parse>),
    Null(NullExpression<'parse>),
    Number(NumberExpression<'parse>),
    Parenthesized(ParenthesizedExpression<'parse>),
    Selector(SelectorExpression<'parse>),
    String(StringExpression<'parse>),
    Supports(SupportsExpression<'parse>),
    UnaryOperation(UnaryOperationExpression<'parse>),
    Value(ValueExpression<'parse>),
    Variable(VariableExpression<'parse>),
}

impl<'parse> Expression<'parse> {
    /// Calls the appropriate visit method on `visitor`.
    pub fn accept<V: ExpressionVisitor<'parse> + ?Sized>(
        &self,
        visitor: &mut V,
    ) -> SassResult<V::Output> {
        match self {
            Expression::BinaryOperation(e) => visitor.visit_binary_operation(e),
            Expression::Boolean(e) => visitor.visit_boolean(e),
            Expression::Color(e) => visitor.visit_color(e),
            Expression::Function(e) => visitor.visit_function(e),
            Expression::If(e) => visitor.visit_if(e),
            Expression::InterpolatedFunction(e) => visitor.visit_interpolated_function(e),
            Expression::LegacyIf(e) => visitor.visit_legacy_if(e),
            Expression::List(e) => visitor.visit_list(e),
            Expression::Map(e) => visitor.visit_map(e),
            Expression::Null(e) => visitor.visit_null(e),
            Expression::Number(e) => visitor.visit_number(e),
            Expression::Parenthesized(e) => visitor.visit_parenthesized(e),
            Expression::Selector(e) => visitor.visit_selector(e),
            Expression::String(e) => visitor.visit_string(e),
            Expression::Supports(e) => visitor.visit_supports(e),
            Expression::UnaryOperation(e) => visitor.visit_unary_operation(e),
            Expression::Value(e) => visitor.visit_value(e),
            Expression::Variable(e) => visitor.visit_variable(e),
        }
    }

    /// Whether this expression is valid plain CSS that will produce the same
    /// result as it would in Sass.
    ///
    /// When `allow_interpolation` is true, interpolated expressions are
    /// allowed as an exception, even if they contain SassScript.
    pub fn is_plain_css(&self, allow_interpolation: bool) -> SassResult<bool> {
        let mut visitor = IsPlainCssVisitor::new(allow_interpolation);
        self.accept(&mut visitor)
    }

    /// Whether this expression can be used in a calculation context.
    pub fn is_calculation_safe(&self) -> SassResult<bool> {
        let mut visitor = IsCalculationSafeVisitor::new();
        self.accept(&mut visitor)
    }

    /// If this expression is valid interpolated plain CSS, returns the
    /// equivalent of parsing its source as an interpolated unknown value.
    ///
    /// Otherwise, returns `None`.
    //
    // Dart marks `sourceInterpolation` `@internal`: it backs plain-CSS
    // fast paths in the parser/evaluator, not public API.
    pub fn source_interpolation(&self) -> Option<&Interpolation<'parse>> {
        match self {
            Expression::BinaryOperation(e) => e.source_interpolation(),
            Expression::Boolean(e) => e.source_interpolation(),
            Expression::Color(e) => e.source_interpolation(),
            Expression::Function(e) => e.source_interpolation(),
            Expression::If(e) => e.source_interpolation(),
            Expression::InterpolatedFunction(e) => e.source_interpolation(),
            Expression::LegacyIf(e) => e.source_interpolation(),
            Expression::List(e) => e.source_interpolation(),
            Expression::Map(e) => e.source_interpolation(),
            Expression::Null(e) => e.source_interpolation(),
            Expression::Number(e) => e.source_interpolation(),
            Expression::Parenthesized(e) => e.source_interpolation(),
            Expression::Selector(e) => e.source_interpolation(),
            Expression::String(e) => e.source_interpolation(),
            Expression::Supports(e) => e.source_interpolation(),
            Expression::UnaryOperation(e) => e.source_interpolation(),
            Expression::Value(e) => e.source_interpolation(),
            Expression::Variable(_) => None,
        }
    }
}

impl<'parse> AstNode<'parse> for Expression<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            Expression::BinaryOperation(e) => e.span(),
            Expression::Boolean(e) => e.span(),
            Expression::Color(e) => e.span(),
            Expression::Function(e) => e.span(),
            Expression::If(e) => e.span(),
            Expression::InterpolatedFunction(e) => e.span(),
            Expression::LegacyIf(e) => e.span(),
            Expression::List(e) => e.span(),
            Expression::Map(e) => e.span(),
            Expression::Null(e) => e.span(),
            Expression::Number(e) => e.span(),
            Expression::Parenthesized(e) => e.span(),
            Expression::Selector(e) => e.span(),
            Expression::String(e) => e.span(),
            Expression::Supports(e) => e.span(),
            Expression::UnaryOperation(e) => e.span(),
            Expression::Value(e) => e.span(),
            Expression::Variable(e) => e.span(),
        }
    }
}

impl<'parse> Expression<'parse> {
    /// Renders this expression as written in source (the `Display` form).
    pub fn to_display_string(&self) -> SassResult<String> {
        match self {
            Expression::BinaryOperation(e) => e.to_display_string(),
            Expression::Boolean(e) => e.to_display_string(),
            Expression::Color(e) => e.to_display_string(),
            Expression::Function(e) => e.to_display_string(),
            Expression::If(e) => e.to_display_string(),
            Expression::InterpolatedFunction(e) => e.to_display_string(),
            Expression::LegacyIf(e) => e.to_display_string(),
            Expression::List(e) => e.to_display_string(),
            Expression::Map(e) => e.to_display_string(),
            Expression::Null(e) => e.to_display_string(),
            Expression::Number(e) => e.to_display_string(),
            Expression::Parenthesized(e) => e.to_display_string(),
            Expression::Selector(e) => e.to_display_string(),
            Expression::String(e) => e.to_display_string(),
            Expression::Supports(e) => e.to_display_string(),
            Expression::UnaryOperation(e) => e.to_display_string(),
            Expression::Value(e) => e.to_display_string(),
            Expression::Variable(e) => e.to_display_string(),
        }
    }
}

impl<'parse> fmt::Display for Expression<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

/// A visitor that traverses SassScript expressions.
///
/// One method per [`Expression`] variant; the return type is the associated
/// [`Output`](Self::Output) type. Ported from Dart's
/// `ExpressionVisitor<T>` (visitor/interface/expression.dart).
pub trait ExpressionVisitor<'parse> {
    type Output;
    fn visit_binary_operation(
        &mut self,
        node: &BinaryOperationExpression<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_boolean(&mut self, node: &BooleanExpression<'parse>) -> SassResult<Self::Output>;
    fn visit_color(&mut self, node: &ColorExpression<'parse>) -> SassResult<Self::Output>;
    fn visit_function(&mut self, node: &FunctionExpression<'parse>) -> SassResult<Self::Output>;
    fn visit_if(&mut self, node: &IfExpression<'parse>) -> SassResult<Self::Output>;
    fn visit_interpolated_function(
        &mut self,
        node: &InterpolatedFunctionExpression<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_legacy_if(&mut self, node: &LegacyIfExpression<'parse>) -> SassResult<Self::Output>;
    fn visit_list(&mut self, node: &ListExpression<'parse>) -> SassResult<Self::Output>;
    fn visit_map(&mut self, node: &MapExpression<'parse>) -> SassResult<Self::Output>;
    fn visit_null(&mut self, node: &NullExpression<'parse>) -> SassResult<Self::Output>;
    fn visit_number(&mut self, node: &NumberExpression<'parse>) -> SassResult<Self::Output>;
    fn visit_parenthesized(
        &mut self,
        node: &ParenthesizedExpression<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_selector(&mut self, node: &SelectorExpression<'parse>) -> SassResult<Self::Output>;
    fn visit_string(&mut self, node: &StringExpression<'parse>) -> SassResult<Self::Output>;
    fn visit_supports(&mut self, node: &SupportsExpression<'parse>) -> SassResult<Self::Output>;
    fn visit_unary_operation(
        &mut self,
        node: &UnaryOperationExpression<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_value(&mut self, node: &ValueExpression<'parse>) -> SassResult<Self::Output>;
    fn visit_variable(&mut self, node: &VariableExpression<'parse>) -> SassResult<Self::Output>;
}

/// A visitor that traverses `@if`-condition expressions.
///
/// One method per `IfCondition*` variant in `expression_if.rs`. Ported from
/// Dart's `IfConditionExpressionVisitor`
/// (visitor/interface/if_condition_expression.dart); the Rust variants live
/// alongside `IfExpression` rather than in a separate file.
pub trait IfConditionExpressionVisitor<'parse> {
    type Output;
    fn visit_parenthesized(
        &mut self,
        node: &IfConditionParenthesized<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_negation(&mut self, node: &IfConditionNegation<'parse>) -> SassResult<Self::Output>;
    fn visit_operation(&mut self, node: &IfConditionOperation<'parse>) -> SassResult<Self::Output>;
    fn visit_function(&mut self, node: &IfConditionFunction<'parse>) -> SassResult<Self::Output>;
    fn visit_sass(&mut self, node: &IfConditionSass<'parse>) -> SassResult<Self::Output>;
    fn visit_raw(&mut self, node: &IfConditionRaw<'parse>) -> SassResult<Self::Output>;
}
