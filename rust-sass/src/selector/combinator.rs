// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector/combinator.dart
// go-source: go/value/selector_combinator.go

use std::fmt;

/// A combinator that defines the relationship between selectors in a
/// [`ComplexSelector`](super::complex::ComplexSelector).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Combinator {
    /// Matches the right-hand selector if it's immediately adjacent to the
    /// left-hand selector in the DOM tree.
    NextSibling,
    /// Matches the right-hand selector if it's a direct child of the
    /// left-hand selector in the DOM tree.
    Child,
    /// Matches the right-hand selector if it comes after the left-hand
    /// selector in the DOM tree.
    FollowingSibling,
}

impl fmt::Display for Combinator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Combinator::NextSibling => f.write_str("+"),
            Combinator::Child => f.write_str(">"),
            Combinator::FollowingSibling => f.write_str("~"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_combinator_display_next_sibling() {
        assert_eq!(Combinator::NextSibling.to_string(), "+");
    }

    #[test]
    fn test_combinator_display_child() {
        assert_eq!(Combinator::Child.to_string(), ">");
    }

    #[test]
    fn test_combinator_display_following_sibling() {
        assert_eq!(Combinator::FollowingSibling.to_string(), "~");
    }

    #[test]
    fn test_combinator_values_distinct() {
        assert_ne!(Combinator::NextSibling, Combinator::Child);
        assert_ne!(Combinator::NextSibling, Combinator::FollowingSibling);
        assert_ne!(Combinator::Child, Combinator::FollowingSibling);
    }

    #[test]
    fn test_combinator_eq() {
        assert_eq!(Combinator::NextSibling, Combinator::NextSibling);
        assert_eq!(Combinator::Child, Combinator::Child);
        assert_eq!(Combinator::FollowingSibling, Combinator::FollowingSibling);
    }
}
