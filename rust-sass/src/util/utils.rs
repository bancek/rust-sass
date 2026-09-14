// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/utils.dart (isPublic, pluralize, toSentence — the only members with a Rust owner here; see the module note for the split map)
// go-source: go/util/utils.go

//! Small member-name and message-formatting helpers.
//!
//! Ports the `utils.dart` members owned here. Split map for the rest of
//! `utils.dart` (split rule — docs live with the logic): `trimAscii*` →
//! [`super::trim_ascii`], `unvendor` → [`crate::unvendor`],
//! `consumeEscapedCharacter` + `startsWithIgnoreCase` → `parse/parser.rs`
//! (`consume_escaped_character`, `has_prefix_ignore_case`),
//! codepoint/code-unit index helpers → `functions/string.rs`,
//! `flattenVertically` → `selector/list.rs` (local helper),
//! `declarationName` → `ast/sass/parameter.rs` (local helper).
//! No Rust owner (omit per rule 4): `equalsIgnoreCase`, `longestCommonSubsequence`,
//! `indent`, `bulletedList`, `frameForSpan`, trace helpers, `parseSignature`
//! (parser-owned logic lives in `parse/`).

/// Returns whether `name` is a public member name (does not start with `-`
/// or `_`).
///
/// Assumes `name` is a valid, non-empty Sass identifier.
/// Matches Dart: `isPublic`.
pub fn is_public(name: &str) -> bool {
    let start = name.as_bytes()[0];
    start != b'-' && start != b'_'
}

/// Returns whether `name` is a private member name (the negation of
/// [`is_public`]).
///
/// Rust-only convenience; Dart inlines `!isPublic(...)` at call sites.
pub fn is_private_member(name: &str) -> bool {
    !is_public(name)
}

/// Returns `name` if `number` is 1, or its plural otherwise.
///
/// Uses `plural` when given, else appends `"s"`.
/// Matches Dart: `pluralize`.
pub fn pluralize(name: &str, number: i32, plural: Option<&str>) -> String {
    if number == 1 {
        return name.to_string();
    }
    match plural {
        Some(p) => p.to_string(),
        None => format!("{name}s"),
    }
}

/// Converts `items` into a sentence, separating each word with `conjunction`
/// (Dart defaults it to `"and"` — callers here always pass it explicitly):
/// `[a]` → `"a"`, `[a, b]` → `"a or b"`, `[a, b, c]` → `"a, b, or c"`.
///
/// Matches Dart: `toSentence`.
pub fn to_sentence(items: &[String], conjunction: &str) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        2 => format!("{} {conjunction} {}", items[0], items[1]),
        _ => {
            let (last, rest) = items.split_last().unwrap();
            format!("{}, {conjunction} {last}", rest.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_public() {
        assert!(is_public("abc"));
        assert!(is_public("abc-def"));
        assert!(!is_public("-private"));
        assert!(!is_public("_private"));
    }

    #[test]
    fn test_is_private_member() {
        assert!(is_private_member("-foo"));
        assert!(is_private_member("_foo"));
        assert!(!is_private_member("foo"));
    }

    #[test]
    fn test_pluralize_singular() {
        assert_eq!(pluralize("thing", 1, None), "thing");
    }

    #[test]
    fn test_pluralize_default() {
        assert_eq!(pluralize("thing", 2, None), "things");
    }

    #[test]
    fn test_pluralize_custom() {
        assert_eq!(pluralize("thing", 3, Some("thingies")), "thingies");
    }
}
