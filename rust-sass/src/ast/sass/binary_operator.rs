// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/expression/binary_operation.dart (BinaryOperator)
// go-source: go/value/sass_expression_binary_operation.go

use std::fmt;

/// A binary operator constant, as in `1 + 2` or `$this and $other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    /// The Microsoft equals operator, `=`.
    SingleEquals = 0,
    /// The disjunction operator, `or`.
    Or = 1,
    /// The conjunction operator, `and`.
    And = 2,
    /// The equality operator, `==`.
    Equals = 3,
    /// The inequality operator, `!=`.
    NotEquals = 4,
    /// The greater-than operator, `>`.
    GreaterThan = 5,
    /// The greater-than-or-equal-to operator, `>=`.
    GreaterThanOrEquals = 6,
    /// The less-than operator, `<`.
    LessThan = 7,
    /// The less-than-or-equal-to operator, `<=`.
    LessThanOrEquals = 8,
    /// The addition operator, `+`.
    Plus = 9,
    /// The subtraction operator, `-`.
    Minus = 10,
    /// The multiplication operator, `*`.
    Times = 11,
    /// The division operator, `/`.
    DividedBy = 12,
    /// The modulo operator, `%`.
    Modulo = 13,
}

impl BinaryOperator {
    /// The English name of `self`.
    pub fn name(&self) -> &'static str {
        match self {
            BinaryOperator::SingleEquals => "single equals",
            BinaryOperator::Or => "or",
            BinaryOperator::And => "and",
            BinaryOperator::Equals => "equals",
            BinaryOperator::NotEquals => "not equals",
            BinaryOperator::GreaterThan => "greater than",
            BinaryOperator::GreaterThanOrEquals => "greater than or equals",
            BinaryOperator::LessThan => "less than",
            BinaryOperator::LessThanOrEquals => "less than or equals",
            BinaryOperator::Plus => "plus",
            BinaryOperator::Minus => "minus",
            BinaryOperator::Times => "times",
            BinaryOperator::DividedBy => "divided by",
            BinaryOperator::Modulo => "modulo",
        }
    }

    /// The precedence of `self`.
    ///
    /// An operator with higher precedence binds tighter.
    pub fn precedence(&self) -> i32 {
        match self {
            BinaryOperator::SingleEquals => 0,
            BinaryOperator::Or => 1,
            BinaryOperator::And => 2,
            BinaryOperator::Equals | BinaryOperator::NotEquals => 3,
            BinaryOperator::GreaterThan
            | BinaryOperator::GreaterThanOrEquals
            | BinaryOperator::LessThan
            | BinaryOperator::LessThanOrEquals => 4,
            BinaryOperator::Plus | BinaryOperator::Minus => 5,
            BinaryOperator::Times | BinaryOperator::DividedBy | BinaryOperator::Modulo => 6,
        }
    }

    /// The Sass syntax for `self`.
    pub fn operator_syntax(&self) -> &'static str {
        match self {
            BinaryOperator::SingleEquals => "=",
            BinaryOperator::Or => "or",
            BinaryOperator::And => "and",
            BinaryOperator::Equals => "==",
            BinaryOperator::NotEquals => "!=",
            BinaryOperator::GreaterThan => ">",
            BinaryOperator::GreaterThanOrEquals => ">=",
            BinaryOperator::LessThan => "<",
            BinaryOperator::LessThanOrEquals => "<=",
            BinaryOperator::Plus => "+",
            BinaryOperator::Minus => "-",
            BinaryOperator::Times => "*",
            BinaryOperator::DividedBy => "/",
            BinaryOperator::Modulo => "%",
        }
    }

    /// Whether this operation has the associative property.
    ///
    /// See <https://en.wikipedia.org/wiki/Associative_property>.
    pub fn is_associative(&self) -> bool {
        matches!(
            self,
            BinaryOperator::Or | BinaryOperator::And | BinaryOperator::Plus | BinaryOperator::Times
        )
    }
}

impl fmt::Display for BinaryOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name() {
        let cases = [
            (BinaryOperator::SingleEquals, "single equals"),
            (BinaryOperator::Or, "or"),
            (BinaryOperator::And, "and"),
            (BinaryOperator::Equals, "equals"),
            (BinaryOperator::NotEquals, "not equals"),
            (BinaryOperator::GreaterThan, "greater than"),
            (
                BinaryOperator::GreaterThanOrEquals,
                "greater than or equals",
            ),
            (BinaryOperator::LessThan, "less than"),
            (BinaryOperator::LessThanOrEquals, "less than or equals"),
            (BinaryOperator::Plus, "plus"),
            (BinaryOperator::Minus, "minus"),
            (BinaryOperator::Times, "times"),
            (BinaryOperator::DividedBy, "divided by"),
            (BinaryOperator::Modulo, "modulo"),
        ];
        for (op, expected) in cases {
            assert_eq!(op.name(), expected);
        }
    }

    #[test]
    fn test_precedence() {
        let cases = [
            (BinaryOperator::SingleEquals, 0),
            (BinaryOperator::Or, 1),
            (BinaryOperator::And, 2),
            (BinaryOperator::Equals, 3),
            (BinaryOperator::NotEquals, 3),
            (BinaryOperator::GreaterThan, 4),
            (BinaryOperator::GreaterThanOrEquals, 4),
            (BinaryOperator::LessThan, 4),
            (BinaryOperator::LessThanOrEquals, 4),
            (BinaryOperator::Plus, 5),
            (BinaryOperator::Minus, 5),
            (BinaryOperator::Times, 6),
            (BinaryOperator::DividedBy, 6),
            (BinaryOperator::Modulo, 6),
        ];
        for (op, expected) in cases {
            assert_eq!(op.precedence(), expected);
        }
    }

    #[test]
    fn test_operator_syntax() {
        let cases = [
            (BinaryOperator::SingleEquals, "="),
            (BinaryOperator::Or, "or"),
            (BinaryOperator::And, "and"),
            (BinaryOperator::Equals, "=="),
            (BinaryOperator::NotEquals, "!="),
            (BinaryOperator::GreaterThan, ">"),
            (BinaryOperator::GreaterThanOrEquals, ">="),
            (BinaryOperator::LessThan, "<"),
            (BinaryOperator::LessThanOrEquals, "<="),
            (BinaryOperator::Plus, "+"),
            (BinaryOperator::Minus, "-"),
            (BinaryOperator::Times, "*"),
            (BinaryOperator::DividedBy, "/"),
            (BinaryOperator::Modulo, "%"),
        ];
        for (op, expected) in cases {
            assert_eq!(op.operator_syntax(), expected);
        }
    }

    #[test]
    fn test_is_associative() {
        assert!(!BinaryOperator::SingleEquals.is_associative());
        assert!(BinaryOperator::Or.is_associative());
        assert!(BinaryOperator::And.is_associative());
        assert!(!BinaryOperator::Equals.is_associative());
        assert!(!BinaryOperator::NotEquals.is_associative());
        assert!(!BinaryOperator::GreaterThan.is_associative());
        assert!(!BinaryOperator::GreaterThanOrEquals.is_associative());
        assert!(!BinaryOperator::LessThan.is_associative());
        assert!(!BinaryOperator::LessThanOrEquals.is_associative());
        assert!(BinaryOperator::Plus.is_associative());
        assert!(!BinaryOperator::Minus.is_associative());
        assert!(BinaryOperator::Times.is_associative());
        assert!(!BinaryOperator::DividedBy.is_associative());
        assert!(!BinaryOperator::Modulo.is_associative());
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", BinaryOperator::Plus), "plus");
    }
}
