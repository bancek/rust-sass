// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector.dart
// go-source: go/value/selector_util.go

// Returns whether `s` is a parent selector (`&`).
//
// No direct Dart counterpart — Dart pattern-matches `is ParentSelector` at
// each call site (e.g. `nestWithin`). Matches Go: `IsParentSelector`
// (go/value/selector_util.go, itself sourced from `ast/selector.dart`).
use crate::selector::SimpleSelector;
pub fn is_parent_selector(s: &SimpleSelector<'_>) -> bool {
    matches!(s, SimpleSelector::Parent(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::selector::class::ClassSelector;
    use crate::selector::parent::ParentSelector;
    use crate::selector::SimpleSelector;
    use bumpalo::Bump;

    #[test]
    fn test_is_parent_selector() {
        let arena = Bump::new();
        let parent = SimpleSelector::Parent(ParentSelector::new(&arena, BOGUS_SPAN, None));
        assert!(is_parent_selector(&parent));

        let class = SimpleSelector::Class(ClassSelector::new("foo".into(), BOGUS_SPAN));
        assert!(!is_parent_selector(&class));
    }
}
