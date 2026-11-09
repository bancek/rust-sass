// Copyright 2022 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector/complex_component.dart
// go-source: go/value/selector_complex_component.go

use std::hash::Hash;
use std::hash::Hasher;

use crate::common::ast_css_value::CssValue;
use crate::common::file_span::FileSpan;
use crate::selector::combinator::Combinator;
use crate::selector::compound::CompoundSelector;
use crate::value::hash::hash_combine;
use bumpalo::Bump;

/// A component of a [`ComplexSelector`](super::complex::ComplexSelector).
///
/// This is a [`CompoundSelector`](super::compound::CompoundSelector) with zero
/// or more trailing [`Combinator`](super::combinator::Combinator)s.
#[derive(Clone, Debug)]
pub struct ComplexSelectorComponent<'parse> {
    /// This component's compound selector.
    pub selector: Box<CompoundSelector<'parse>>,
    /// This component's combinators.
    ///
    /// An empty list indicates an implicit descendant combinator. More than
    /// one element is invalid CSS, still supported for backwards-compatibility
    /// purposes.
    pub combinators: &'parse [CssValue<'parse, Combinator>],
    pub span: FileSpan<'parse>,
}

impl<'parse> ComplexSelectorComponent<'parse> {
    /// Creates a component from a compound selector and trailing combinators.
    pub fn new(
        arena: &'parse Bump,
        selector: Box<CompoundSelector<'parse>>,
        combinators: Vec<CssValue<'parse, Combinator>>,
        span: FileSpan<'parse>,
    ) -> Self {
        ComplexSelectorComponent {
            selector,
            combinators: &*arena.alloc_slice_fill_iter(combinators),
            span,
        }
    }

    pub fn hash_code(&self) -> i32 {
        let mut h = self.selector.hash_code();
        for comb in self.combinators {
            h = hash_combine(h, (comb.value as i32).wrapping_mul(31));
        }
        h
    }

    // Returns a copy of `self` with `combinators` appended to the end of
    // [`ComplexSelectorComponent::combinators`].
    pub fn with_additional_combinators(
        &self,
        arena: &'parse Bump,
        combinators: &[CssValue<'parse, Combinator>],
    ) -> Self {
        if combinators.is_empty() {
            return self.clone();
        }
        let mut new_combinators = self.combinators.to_vec();
        new_combinators.extend_from_slice(combinators);
        ComplexSelectorComponent {
            selector: self.selector.clone(),
            combinators: &*arena.alloc_slice_fill_iter(new_combinators),
            span: self.span,
        }
    }
}

impl<'parse> PartialEq for ComplexSelectorComponent<'parse> {
    fn eq(&self, other: &Self) -> bool {
        if self.combinators.len() != other.combinators.len() {
            return false;
        }
        for i in 0..self.combinators.len() {
            if self.combinators[i] != other.combinators[i] {
                return false;
            }
        }
        self.selector == other.selector
    }
}

impl<'parse> Eq for ComplexSelectorComponent<'parse> {}

impl<'parse> Hash for ComplexSelectorComponent<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.hash_code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::selector::class::ClassSelector;
    use crate::selector::SimpleSelector;
    use bumpalo::Bump;

    fn make_compound<'p>(arena: &'p Bump, name: &'static str) -> Box<CompoundSelector<'p>> {
        let simple = SimpleSelector::Class(ClassSelector::new(name, BOGUS_SPAN));
        Box::new(CompoundSelector::new(arena, vec![simple], BOGUS_SPAN).unwrap())
    }

    #[test]
    fn test_new() {
        let arena = Bump::new();
        let compound = make_compound(&arena, "foo");
        let c = ComplexSelectorComponent::new(&arena, compound.clone(), vec![], BOGUS_SPAN);
        assert_eq!(c.selector, compound);
    }

    #[test]
    fn test_hash_code() {
        let arena = Bump::new();
        let c1 =
            ComplexSelectorComponent::new(&arena, make_compound(&arena, "foo"), vec![], BOGUS_SPAN);
        let c2 =
            ComplexSelectorComponent::new(&arena, make_compound(&arena, "foo"), vec![], BOGUS_SPAN);
        assert_eq!(c1.hash_code(), c2.hash_code());
    }

    #[test]
    fn test_with_additional_combinators() {
        let arena = Bump::new();
        let c =
            ComplexSelectorComponent::new(&arena, make_compound(&arena, "foo"), vec![], BOGUS_SPAN);
        let comb = CssValue::new(Combinator::Child, BOGUS_SPAN);
        let result = c.with_additional_combinators(&arena, std::slice::from_ref(&comb));
        assert_eq!(result.combinators.len(), 1);
        assert_eq!(result.combinators[0], comb);
    }

    #[test]
    fn test_with_additional_combinators_empty() {
        let arena = Bump::new();
        let c =
            ComplexSelectorComponent::new(&arena, make_compound(&arena, "foo"), vec![], BOGUS_SPAN);
        let result = c.with_additional_combinators(&arena, &[]);
        assert_eq!(result.combinators.len(), 0);
    }

    #[test]
    fn test_eq() {
        let arena = Bump::new();
        let c1 =
            ComplexSelectorComponent::new(&arena, make_compound(&arena, "foo"), vec![], BOGUS_SPAN);
        let c2 =
            ComplexSelectorComponent::new(&arena, make_compound(&arena, "foo"), vec![], BOGUS_SPAN);
        assert_eq!(c1, c2);

        let c3 =
            ComplexSelectorComponent::new(&arena, make_compound(&arena, "bar"), vec![], BOGUS_SPAN);
        assert_ne!(c1, c3);
    }
}
