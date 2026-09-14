// Copyright 2017 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/extend/mode.dart
// go-source: go/extend/extend_mode.go

use std::fmt;

/// Different modes in which extension can run.
///
/// Selects how [`crate::extend::store::ExtensionStore`] applies extenders to
/// selectors: whether originals are kept and whether every target must match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtendMode {
    /// Normal mode, used with the `@extend` rule.
    ///
    /// Preserves existing selectors and extends each target individually.
    Normal,
    /// Replace mode, used by the `selector-replace()` function.
    ///
    /// Replaces existing selectors and requires every target to match before
    /// a given compound selector is extended.
    Replace,
    /// All-targets mode, used by the `selector-extend()` function.
    ///
    /// Preserves existing selectors but requires every target to match before
    /// a given compound selector is extended.
    AllTargets,
}

impl fmt::Display for ExtendMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtendMode::Normal => write!(f, "normal"),
            ExtendMode::Replace => write!(f, "replace"),
            ExtendMode::AllTargets => write!(f, "allTargets"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_normal() {
        assert_eq!(ExtendMode::Normal.to_string(), "normal");
    }

    #[test]
    fn test_string_replace() {
        assert_eq!(ExtendMode::Replace.to_string(), "replace");
    }

    #[test]
    fn test_string_all_targets() {
        assert_eq!(ExtendMode::AllTargets.to_string(), "allTargets");
    }
}
