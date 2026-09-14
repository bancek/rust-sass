// Copyright (c) 2012, the Dart project authors.  Please see the AUTHORS file
// for details. All rights reserved. Use of this source code is governed by a
// BSD-style license that can be found in the LICENSE file.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: (external) package:path (Context.prettyUri, lib/src/context.dart)
// go-source: go/sasscommon/pretty_uri.go

use std::path::Path;

use crate::url::SassUrl;

use crate::io::Io;

/// Returns a relative path from `base` to `path`, using `..` as needed.
///
/// Rust-only helper realizing the `relative(path)` step of Dart's
/// `Context.prettyUri` (`package:path/lib/src/context.dart`); the caller
/// applies Dart's shorter-than-absolute rule.
fn rel_path_to(path: &Path, base: &Path) -> Option<String> {
    let path_components: Vec<_> = path.components().collect();
    let base_components: Vec<_> = base.components().collect();

    let common_len = path_components
        .iter()
        .zip(base_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut parts: Vec<String> =
        std::iter::repeat_n("..".to_string(), base_components.len() - common_len).collect();

    parts.extend(
        path_components[common_len..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );

    if parts.is_empty() {
        return Some(".".to_string());
    }
    Some(parts.join("/"))
}

/// Returns a human-readable representation of `uri` for messages.
///
/// Matches Dart: `Context.prettyUri` (`package:path/lib/src/context.dart`;
/// surfaced as `p.prettyUri`) — `file:` URIs render as paths relative to
/// the current directory, falling back to the absolute path when the
/// relative form would be longer; every other scheme renders as-is. The
/// result is for human consumption and may be URI- or path-formatted.
pub fn pretty_uri(uri: &SassUrl, io: &dyn Io) -> String {
    if uri.is_file_like() {
        let abs = uri.path();
        let cwd = io.current_dir();
        let cwd_path = Path::new(&cwd);
        let abs_path = Path::new(abs);
        if let Some(rel) = rel_path_to(abs_path, cwd_path) {
            if rel.split('/').count() <= abs.split('/').count() {
                return rel;
            }
        }
        return abs.to_string();
    }
    uri.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::url::SassUrl;
    use std::path::Path;

    use crate::io::VirtualIo;

    // ── rel_path_to tests (no Io dependency) ──

    #[test]
    fn test_rel_to_child() {
        let got = rel_path_to(Path::new("/a/b/c/d/e.scss"), Path::new("/a/b/c")).unwrap();
        assert_eq!(got, "d/e.scss");
    }

    #[test]
    fn test_rel_to_sibling() {
        let got = rel_path_to(Path::new("/a/b/d/e/f.scss"), Path::new("/a/b/c")).unwrap();
        assert_eq!(got, "../d/e/f.scss");
    }

    #[test]
    fn test_rel_to_same_dir() {
        let got = rel_path_to(Path::new("/a/b/c/f.scss"), Path::new("/a/b/c")).unwrap();
        assert_eq!(got, "f.scss");
    }

    #[test]
    fn test_rel_to_same_path() {
        let got = rel_path_to(Path::new("/a/b/c"), Path::new("/a/b/c")).unwrap();
        assert_eq!(got, ".");
    }

    #[test]
    fn test_rel_to_unrelated() {
        let got = rel_path_to(Path::new("/a/b/c/d.scss"), Path::new("/x/y/z")).unwrap();
        assert_eq!(got, "../../../a/b/c/d.scss");
    }

    #[test]
    fn test_rel_to_root() {
        let got = rel_path_to(Path::new("/a/b/c"), Path::new("/")).unwrap();
        assert_eq!(got, "a/b/c");
    }

    // ── pretty_uri tests (deterministic via VirtualIo) ──

    #[test]
    fn test_file_scheme_relative_shorter() {
        let io = VirtualIo::with_cwd("/home/user");
        let uri = SassUrl::parse("file:///home/user/project/style.scss").unwrap();
        assert_eq!(pretty_uri(&uri, &io), "project/style.scss");
    }

    #[test]
    fn test_file_scheme_relative_same_dir_is_dot() {
        let io = VirtualIo::with_cwd("/home/user");
        let uri = SassUrl::parse("file:///home/user").unwrap();
        assert_eq!(pretty_uri(&uri, &io), ".");
    }

    #[test]
    fn test_file_scheme_relative_longer_falls_back_to_absolute() {
        let io = VirtualIo::with_cwd("/home/user");
        let uri = SassUrl::parse("file:///other/style.scss").unwrap();
        assert_eq!(pretty_uri(&uri, &io), "/other/style.scss");
    }

    #[test]
    fn test_file_scheme_default_cwd_root() {
        let io = VirtualIo::new();
        let uri = SassUrl::parse("file:///a/b/c.scss").unwrap();
        assert_eq!(pretty_uri(&uri, &io), "a/b/c.scss");
    }

    #[test]
    fn test_non_file_scheme_passes_through() {
        let io = VirtualIo::new();
        let uri = SassUrl::parse("https://example.com/path").unwrap();
        assert_eq!(pretty_uri(&uri, &io), "https://example.com/path");
    }

    #[test]
    fn test_sass_scheme_passes_through() {
        let io = VirtualIo::new();
        let uri = SassUrl::parse("sass:color").unwrap();
        assert_eq!(pretty_uri(&uri, &io), "sass:color");
    }
}
