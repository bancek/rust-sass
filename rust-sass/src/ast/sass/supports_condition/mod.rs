// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/supports_condition.dart
// go-source: go/value/sass_supports_condition.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::common::span_error::SpanError;

use crate::ast::sass::interpolation::Interpolation;

pub mod anything;
pub mod declaration;
pub mod function;
pub mod interpolation;
pub mod negation;
pub mod operation;

pub use anything::SupportsAnything;
pub use declaration::SupportsDeclaration;
pub use function::SupportsFunction;
pub use interpolation::SupportsInterpolation;
pub use negation::SupportsNegation;
pub use operation::SupportsOperation;

#[derive(Clone, Debug)]
/// A condition selecting what a `@supports` rule applies to.
///
/// Six variants (see `ref/ast.md`); unlike the if-condition hierarchy this
/// has no visitor trait — dispatch is a plain `match` on the enum.
// Variant sizes mirror Dart's six cases; boxing would churn matches for no
// observable change.
#[allow(clippy::large_enum_variant)]
pub enum SupportsCondition<'parse> {
    Anything(SupportsAnything<'parse>),
    Declaration(SupportsDeclaration<'parse>),
    Function(SupportsFunction<'parse>),
    Interpolation(SupportsInterpolation<'parse>),
    Negation(SupportsNegation<'parse>),
    Operation(SupportsOperation<'parse>),
}

impl<'parse> SupportsCondition<'parse> {
    // Converts this condition into an interpolation that produces the same
    // value.
    pub fn to_interpolation(&self) -> SassResult<Interpolation<'parse>> {
        match self {
            Self::Anything(c) => c.to_interpolation(),
            Self::Declaration(c) => c.to_interpolation(),
            Self::Function(c) => c.to_interpolation(),
            Self::Interpolation(c) => c.to_interpolation(),
            Self::Negation(c) => c.to_interpolation(),
            Self::Operation(c) => c.to_interpolation(),
        }
    }

    // Returns a copy of this condition with `span` as its span.
    pub fn with_span(&self, span: FileSpan<'parse>) -> Self {
        match self {
            Self::Anything(c) => Self::Anything(c.with_span(span)),
            Self::Declaration(c) => Self::Declaration(c.with_span(span)),
            Self::Function(c) => Self::Function(c.with_span(span)),
            Self::Interpolation(c) => Self::Interpolation(c.with_span(span)),
            Self::Negation(c) => Self::Negation(c.with_span(span)),
            Self::Operation(c) => Self::Operation(c.with_span(span)),
        }
    }
}

impl<'parse> AstNode<'parse> for SupportsCondition<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            Self::Anything(c) => c.span(),
            Self::Declaration(c) => c.span(),
            Self::Function(c) => c.span(),
            Self::Interpolation(c) => c.span(),
            Self::Negation(c) => c.span(),
            Self::Operation(c) => c.span(),
        }
    }
}

impl<'parse> SupportsCondition<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        match self {
            Self::Anything(c) => write!(buf, "{}", c).unwrap(),
            Self::Declaration(c) => write!(buf, "{}", c).unwrap(),
            Self::Function(c) => write!(buf, "{}", c).unwrap(),
            Self::Interpolation(c) => write!(buf, "{}", c).unwrap(),
            Self::Negation(c) => write!(buf, "{}", c).unwrap(),
            Self::Operation(c) => write!(buf, "{}", c).unwrap(),
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for SupportsCondition<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

pub(crate) fn convert_span_error(e: SpanError) -> Box<SassError> {
    match e {
        SpanError::Sass(e) => e,
        SpanError::Argument(msg) | SpanError::Range(msg) => Box::new(SassError::Script {
            message: msg,
            argument_name: None,
        }),
    }
}
