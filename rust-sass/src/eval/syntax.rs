// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/syntax.dart
// go-source: go/eval/syntax.go

use std::collections::HashSet;
use std::path::Path;

use crate::parse::stylesheet::{CssState, SassIndentState, Syntax};

/// Returns the default syntax to use for a file loaded from `path`.
///
/// Matches Dart: `Syntax.forPath` (`syntax.dart:21-25`) — a `switch` on
/// `p.extension(path)` mapping `.sass` to the indented syntax and `.css` to
/// plain CSS, with everything else (including extensionless paths and
/// dotfiles such as `.sass`, whose `p.extension` is `""`) falling through to
/// SCSS. The [`Syntax`] type itself (plus the `Sass`/`Css` variant state) is
/// documented at its owner in [`crate::parse::stylesheet`]; this free
/// function is the only `syntax.dart` logic evaluated here.
pub fn syntax_for_path(path: &str) -> Syntax {
    // Dart `Syntax.forPath` matches on `p.extension(path)` — the extension
    // INCLUDING the dot, or `""` when there is none. A dotfile like `.sass`
    // has extension `""` (basename starting with a dot is not an extension),
    // so it maps to SCSS, not Sass. `ends_with` gets this wrong.
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    match ext.as_str() {
        ".sass" => Syntax::Sass(SassIndentState {
            current_indentation: 0,
            next_indentation: None,
            next_indentation_end: None,
            indent_spaces: None,
        }),
        ".css" => Syntax::Css(CssState {
            disallowed_function_names: HashSet::new(),
        }),
        _ => Syntax::Scss,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Dart `Syntax.forPath` matches on `p.extension` — a dotfile like
    // `.sass` has extension `""` and maps to SCSS (U13 suspect, confirmed).
    #[test]
    fn test_dotfile_syntax_for_path() {
        assert!(matches!(syntax_for_path(".sass"), Syntax::Scss));
        assert!(matches!(syntax_for_path(".css"), Syntax::Scss));
        assert!(matches!(syntax_for_path("a.sass"), Syntax::Sass(_)));
        assert!(matches!(syntax_for_path("a.css"), Syntax::Css(_)));
        assert!(matches!(syntax_for_path("a.scss"), Syntax::Scss));
        assert!(matches!(syntax_for_path("noext"), Syntax::Scss));
    }
}
