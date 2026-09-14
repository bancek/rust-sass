// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/css.dart (_disallowedFunctionNames)
// go-source: go/functions/disallowed.go

use crate::functions::global_functions;
use bumpalo::Bump;
use std::collections::HashSet;

/// The set of global function names that are disallowed in plain CSS.
///
/// Every global function is disallowed except the 19 CSS-compatible ones
/// (`abs`, `alpha`, `color`, `grayscale`, `hsl`, `hsla`, `hwb`, `invert`,
/// `lab`, `lch`, `max`, `min`, `oklab`, `oklch`, `opacity`, `rgb`, `rgba`,
/// `round`, `saturate`); the count invariant
/// `len == global_functions().len() - 19` is locked by test below.
///
/// Matches Dart's private `_disallowedFunctionNames` in `parse/css.dart`,
/// consulted by the plain-CSS parser when it meets `name(`.
pub fn disallowed_function_names(arena: &Bump) -> HashSet<String> {
    let mut set: HashSet<String> = global_functions(arena)
        .iter()
        .map(|f| f.name().to_string())
        .collect();
    set.remove("abs");
    set.remove("alpha");
    set.remove("color");
    set.remove("grayscale");
    set.remove("hsl");
    set.remove("hsla");
    set.remove("hwb");
    set.remove("invert");
    set.remove("lab");
    set.remove("lch");
    set.remove("max");
    set.remove("min");
    set.remove("oklab");
    set.remove("oklch");
    set.remove("opacity");
    set.remove("rgb");
    set.remove("rgba");
    set.remove("round");
    set.remove("saturate");
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::functions::global_functions;

    #[test]
    fn test_disallowed_function_names_removals() {
        let arena = Bump::new();
        let set = disallowed_function_names(&arena);
        for name in [
            "abs",
            "alpha",
            "color",
            "grayscale",
            "hsl",
            "hsla",
            "hwb",
            "invert",
            "lab",
            "lch",
            "max",
            "min",
            "oklab",
            "oklch",
            "opacity",
            "rgb",
            "rgba",
            "round",
            "saturate",
        ] {
            assert!(!set.contains(name), "should not contain {name:?}");
        }
    }

    #[test]
    fn test_disallowed_function_names_contains() {
        let arena = Bump::new();
        let set = disallowed_function_names(&arena);
        for name in [
            "red",
            "green",
            "blue",
            "mix",
            "lighten",
            "darken",
            "length",
            "nth",
            "join",
            "map-get",
            "map-merge",
            "percentage",
            "comparable",
            "unit",
            "unitless",
            "ceil",
            "floor",
            "random",
            "is-superselector",
            "selector-parse",
            "simple-selectors",
            "unquote",
            "quote",
            "str-length",
            "str-insert",
            "str-index",
            "str-slice",
            "to-upper-case",
            "to-lower-case",
            "unique-id",
            "feature-exists",
            "inspect",
            "type-of",
            "keywords",
            "if",
        ] {
            assert!(set.contains(name), "should contain {name:?}");
        }
    }

    #[test]
    fn test_disallowed_function_names_count() {
        // Every removed name must have been present in global_functions(),
        // and global function names must be unique.
        let arena = Bump::new();
        let mut all: HashSet<String> = HashSet::new();
        for f in global_functions(&arena) {
            assert!(
                all.insert(f.name().to_string()),
                "duplicate global function name {:?}",
                f.name()
            );
        }
        let set = disallowed_function_names(&arena);
        assert_eq!(set.len(), all.len() - 19);
    }
}
