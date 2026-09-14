// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/utils.dart (unvendor)
// go-source: go/unvendor/unvendor.go

/// Returns `name` without a vendor prefix.
///
/// Names with no vendor prefix (including `--custom` properties and names
/// with no second dash) are returned as-is.
/// Matches Dart: `unvendor`.
pub fn unvendor(name: &str) -> String {
    if name.len() < 2 || !name.starts_with('-') || name.as_bytes().get(1) == Some(&b'-') {
        return name.to_string();
    }
    for i in 2..name.len() {
        if name.as_bytes()[i] == b'-' {
            return name[i + 1..].to_string();
        }
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unvendor_no_prefix() {
        assert_eq!(unvendor("keyframes"), "keyframes");
    }

    #[test]
    fn test_unvendor_webkit() {
        assert_eq!(unvendor("-webkit-keyframes"), "keyframes");
    }

    #[test]
    fn test_unvendor_moz() {
        assert_eq!(unvendor("-moz-foo"), "foo");
    }

    #[test]
    fn test_unvendor_double_dash() {
        assert_eq!(unvendor("--custom"), "--custom");
    }

    #[test]
    fn test_unvendor_no_second_dash() {
        assert_eq!(unvendor("-x"), "-x");
    }

    #[test]
    fn test_unvendor_empty() {
        assert_eq!(unvendor(""), "");
    }
}
