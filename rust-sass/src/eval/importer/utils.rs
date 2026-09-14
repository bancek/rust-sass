// Copyright 2017 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/importer/utils.dart (isValidUrlScheme, fromImport Zone
//   helpers — fromImport/inImportRule folded into CanonicalizeContext, see
//   canonicalize_context.rs; Syntax.forPath lives in parse/stylesheet)
// go-source: go/eval/importer_utils.go + go/eval/syntax.go

use std::collections::HashSet;
use std::path::Path;

use crate::parse::stylesheet::{CssState, SassIndentState, Syntax};

/// Returns whether `scheme` is a valid URL scheme (`[a-z0-9+.-]+`).
///
/// Matches Dart's `isValidUrlScheme` (`importer/utils.dart`).
pub fn is_valid_url_scheme(scheme: &str) -> bool {
    if scheme.is_empty() {
        return false;
    }
    scheme
        .chars()
        .all(|c| matches!(c, 'a'..='z' | '0'..='9' | '+' | '.' | '-'))
}

/// Returns the syntax inferred from a stylesheet path (`.sass`/`.css`, else
/// SCSS).
///
/// Rust counterpart of Dart's `Syntax.forPath` (`syntax.dart`); the real owner
/// is the `Syntax` parser entry in `parse/stylesheet`.
pub fn syntax_for_path(path: &Path) -> Syntax {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "sass" => Syntax::Sass(SassIndentState {
            current_indentation: 0,
            next_indentation: None,
            next_indentation_end: None,
            indent_spaces: None,
        }),
        "css" => Syntax::Css(CssState {
            disallowed_function_names: HashSet::new(),
        }),
        _ => Syntax::Scss,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_is_valid_url_scheme_valid() {
        assert!(is_valid_url_scheme("file"));
        assert!(is_valid_url_scheme("http"));
        assert!(is_valid_url_scheme("package"));
        assert!(is_valid_url_scheme("pkg"));
        assert!(is_valid_url_scheme("a+b.c-d"));
    }

    #[test]
    fn test_is_valid_url_scheme_invalid() {
        assert!(!is_valid_url_scheme(""));
        assert!(!is_valid_url_scheme("File")); // uppercase
        assert!(!is_valid_url_scheme("file_1")); // underscore
        assert!(!is_valid_url_scheme("f@le")); // @
    }

    #[test]
    fn test_syntax_for_path_scss() {
        let syntax = syntax_for_path(Path::new("/foo/style.scss"));
        assert_eq!(syntax, Syntax::Scss);
    }

    #[test]
    fn test_syntax_for_path_sass() {
        let syntax = syntax_for_path(Path::new("/foo/style.sass"));
        assert!(matches!(syntax, Syntax::Sass(_)));
    }

    #[test]
    fn test_syntax_for_path_css() {
        let syntax = syntax_for_path(Path::new("/foo/style.css"));
        assert!(matches!(syntax, Syntax::Css(_)));
    }

    #[test]
    fn test_syntax_for_path_no_extension() {
        let syntax = syntax_for_path(Path::new("/foo/style"));
        assert_eq!(syntax, Syntax::Scss);
    }
}
