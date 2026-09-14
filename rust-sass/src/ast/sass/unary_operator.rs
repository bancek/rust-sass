// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/unary_operation.dart (UnaryOperator)
// go-source: go/value/sass_expression_unary_operation.go

use std::fmt;

/// A unary operator constant, as in `+$var` or `not fn()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    /// The numeric identity operator, `+`.
    Plus = 0,
    /// The numeric negation operator, `-`.
    Minus = 1,
    /// The leading slash operator, `/`.
    ///
    /// This is a historical artifact.
    Divide = 2,
    /// The boolean negation operator, `not`.
    Not = 3,
}

impl UnaryOperator {
    /// The Sass syntax for `self`.
    pub fn operator_syntax(&self) -> &'static str {
        match self {
            UnaryOperator::Plus => "+",
            UnaryOperator::Minus => "-",
            UnaryOperator::Divide => "/",
            UnaryOperator::Not => "not",
        }
    }
}

impl fmt::Display for UnaryOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnaryOperator::Plus => write!(f, "plus"),
            UnaryOperator::Minus => write!(f, "minus"),
            UnaryOperator::Divide => write!(f, "divide"),
            UnaryOperator::Not => write!(f, "not"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operator_syntax() {
        assert_eq!(UnaryOperator::Plus.operator_syntax(), "+");
        assert_eq!(UnaryOperator::Minus.operator_syntax(), "-");
        assert_eq!(UnaryOperator::Divide.operator_syntax(), "/");
        assert_eq!(UnaryOperator::Not.operator_syntax(), "not");
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", UnaryOperator::Plus), "plus");
        assert_eq!(format!("{}", UnaryOperator::Minus), "minus");
        assert_eq!(format!("{}", UnaryOperator::Divide), "divide");
        assert_eq!(format!("{}", UnaryOperator::Not), "not");
    }
}
