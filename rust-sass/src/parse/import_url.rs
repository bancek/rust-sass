// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/parse/stylesheet.dart (parseImportUrl, isPlainImportUrl)
// go-source: go/value/parse_import_url.go

use crate::util::character::is_alphabetic;

/// Parses `raw_url` as an import URL.
///
/// Matches Dart's `parseImportUrl` exactly:
///   - Absolute Windows paths (drive-letter, UNC, and backslash-rooted like
///     `\foo`) are converted to `file://` URLs, EXCEPT those that are
///     root-relative in URL context (only `/` counts, per `p.url.isRootRelative`).
///   - Otherwise the URL is returned UNCHANGED (no re-encoding). Validation
///     happens in the caller (Dart: `Uri.parse(url)` throws `FormatException`,
///     caught as "Invalid URL").
pub(crate) fn parse_import_url(raw_url: &str) -> String {
    if looks_like_windows_absolute_path(raw_url) && !is_root_relative(raw_url) {
        let bytes = raw_url.as_bytes();
        if bytes.len() >= 2 && bytes[0] == b'\\' && bytes[1] == b'\\' {
            let after_prefix = raw_url[2..].replace('\\', "/");
            let (host, path) = match after_prefix.split_once('/') {
                Some((h, p)) => (h, format!("/{p}")),
                None => (after_prefix.as_str(), String::new()),
            };
            return format!("file://{host}{path}");
        }
        // Backslash-rooted path: \foo\bar.scss → file:///foo/bar.scss
        if bytes[0] == b'\\' {
            return format!("file:///{}", raw_url[1..].replace('\\', "/"));
        }
        // Drive-letter path: C:\foo → file:///C:/foo
        return format!("file:///{}", raw_url.replace('\\', "/"));
    }
    // Return url unchanged on success (matches Dart: Uri.parse(url); return url).
    raw_url.to_string()
}

/// Returns whether `path` is an absolute Windows path: a drive-letter path
/// (C:\foo), a UNC path (\\server\share), or a backslash- or slash-rooted path
/// (\foo, /foo). Matches Dart's `p.windows.isAbsolute`.
pub(crate) fn looks_like_windows_absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    if !bytes.is_empty() && (bytes[0] == b'\\' || bytes[0] == b'/') {
        return true;
    }
    // Drive-letter path (e.g. C:\foo or D:/bar)
    bytes.len() >= 3
        && is_alphabetic(bytes[0] as char)
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// Returns whether `url` indicates that an `@import` is a plain CSS import:
///
/// a `.css` extension, a protocol-relative `//` prefix, or an `http(s)://`
/// scheme. Matches Dart's `isPlainImportUrl`.
pub(crate) fn is_plain_import_url(url: &str) -> bool {
    if url.len() < 5 {
        return false;
    }
    if url.ends_with(".css") {
        return true;
    }
    let bytes = url.as_bytes();
    match bytes[0] {
        b'/' => url.len() > 1 && bytes[1] == b'/',
        b'h' => url.starts_with("http://") || url.starts_with("https://"),
        _ => false,
    }
}

/// Returns whether `path` is root-relative in URL context.
///
/// Matches Dart's `p.url.isRootRelative`: only `/` counts as a root separator —
/// a backslash-rooted path like `\foo` is NOT root-relative in URL context.
pub(crate) fn is_root_relative(path: &str) -> bool {
    let bytes = path.as_bytes();
    !bytes.is_empty() && bytes[0] == b'/'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_looks_like_windows_absolute_path() {
        let true_cases = [
            r"C:\foo\bar.scss",
            r"C:/foo/bar.scss",
            r"D:\",
            r"X:\a\b\c\d",
            r"C:\",
            r"c:\path",
            r"z:\path",
            r"\\server\share",
            r"\\server\share\file",
            r"\foo\bar.scss",
            r"/absolute/unix/path.scss",
        ];
        for case in true_cases {
            assert!(
                looks_like_windows_absolute_path(case),
                "should be true: {case:?}"
            );
        }
        let false_cases = [
            "relative/path.scss",
            "../relative/path.scss",
            "foo.scss",
            "",
            "C:",
            "C:foo.scss",
            ":\\,",
            "5:\\path",
            "AB:\\path",
        ];
        for case in false_cases {
            assert!(
                !looks_like_windows_absolute_path(case),
                "should be false: {case:?}"
            );
        }
    }

    #[test]
    fn test_parse_import_url() {
        let cases: &[(&str, &str)] = &[
            ("windows backslash", r"C:\path\to\file.scss"),
            ("windows forward slash", r"D:/other/style.sass"),
            ("windows lowercase drive", r"c:\Foo\bar.sass"),
            ("unc path", r"\\server\share\file.scss"),
            ("root relative backslash", r"\foo\bar.scss"),
            ("backslash relative", r"a\b.scss"),
            ("unix absolute", "/srv/sass/style.scss"),
            ("relative path", "../theme/_vars.scss"),
            ("simple name", "style.scss"),
            ("http url", "https://example.com/a.css"),
            ("empty string", ""),
        ];
        let expected = [
            "file:///C:/path/to/file.scss",
            "file:///D:/other/style.sass",
            "file:///c:/Foo/bar.sass",
            "file://server/share/file.scss",
            "file:///foo/bar.scss",
            "a\\b.scss",
            "/srv/sass/style.scss",
            "../theme/_vars.scss",
            "style.scss",
            "https://example.com/a.css",
            "",
        ];
        for ((name, input), want) in cases.iter().zip(expected.iter()) {
            let got = parse_import_url(input);
            assert_eq!(
                got, *want,
                "{name}: parse_import_url({input:?}) = {got:?}, want {want:?}"
            );
        }
    }

    #[test]
    fn test_is_root_relative() {
        assert!(!is_root_relative("\\foo"));
        assert!(is_root_relative("/foo"));
        assert!(!is_root_relative("C:\\foo"));
        assert!(!is_root_relative("\\\\server"));
        assert!(!is_root_relative("foo"));
        assert!(!is_root_relative(""));
    }

    #[test]
    fn test_parse_import_url_unc() {
        let result = parse_import_url("\\\\server\\share\\file.scss");
        assert_eq!(result, "file://server/share/file.scss");
    }

    #[test]
    fn test_parse_import_url_backslash_relative() {
        let result = parse_import_url("a\\b.scss");
        assert_eq!(result, "a\\b.scss");
    }

    #[test]
    fn test_looks_like_windows_empty() {
        assert!(!looks_like_windows_absolute_path(""));
    }

    #[test]
    fn test_is_root_relative_backslash() {
        assert!(!is_root_relative("\\foo"));
    }
}
