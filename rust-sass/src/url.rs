//! Dart-compatible URL resolution with SassUrl newtype.
//!
//! Rust's `url` crate cannot represent scheme-less (relative) URLs.
//! Go's `net/url` and Dart's `Uri` both allow `Scheme: ""`, `Path: "../foo"`.
//! We wrap relative URLs internally as `sass-relative:<path>` — an opaque URL
//! whose path preserves `..` segments. The scheme is stripped by `Display`.
//!
//! This module also provides `resolve()` for opaque/cannot-be-a-base URLs
//! (like `sass:color`) matching Dart's `Uri.resolve` semantics.
//!
//! No `dart-source:` annotation: Rust-only infrastructure with no single Dart
//! counterpart (behavior mirrors `Uri.parse`/`resolve`, `ImporterResult`
//! `sourceMapUrl` encoding, and the filesystem importer's load-path fallback;
//! `go-source:` likewise omitted — the `sass-relative:` scheme and
//! wrapped-relative tagging are original to this port).

#[cfg(target_os = "windows")]
use crate::io::clean_path;
#[cfg(target_arch = "wasm32")]
use percent_encoding::CONTROLS;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use std::fmt::{self, Display, Formatter};
use url::Url;

// ── Constants ────────────────────────────────────────────────────────────

/// Internal scheme for relative URLs. Never exposed to users.
pub(crate) const SASS_RELATIVE: &str = "sass-relative";

/// Marker for a `file:` URL produced by resolving a `sass-relative:` URL
/// against a `file:` base (see `resolve_file_path`). The filesystem importer's
/// load-path fallback applies to these wrapped-relative URLs only — never to
/// genuine absolute `file:` URLs (filesystem.dart:68-90).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
struct WrappedRelative(bool);

/// Bytes that Dart's `Uri.dataFromString(..., encoding: utf8)` leaves
/// unencoded in a `data:` URI (importer/result.dart `sourceMapUrl`): RFC 3986
/// unreserved chars plus the reserved chars except `#`, `[`, `]` (verified
/// empirically against the `sass` npm package). Everything else — space,
/// `#%<>[]\^`{}|` and non-ASCII — is UTF-8 percent-encoded.
const DATA_SAFE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'!')
    .remove(b'$')
    .remove(b'&')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')')
    .remove(b'*')
    .remove(b'+')
    .remove(b',')
    .remove(b'/')
    .remove(b':')
    .remove(b';')
    .remove(b'=')
    .remove(b'?')
    .remove(b'@');

// ── SassUrl newtype ─────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SassUrl(Url, WrappedRelative);

impl SassUrl {
    // ── Construction ────────────────────────────────────────────────

    /// Parse a URL string. Relative paths (no scheme) are stored as
    /// `sass-relative:<path>`, preserving `..` segments.
    pub fn parse(raw: &str) -> Result<Self, url::ParseError> {
        if raw.is_empty() {
            // Go: url.Parse("") returns a valid relative URL (never fails).
            // We represent empty-string URLs as a relative URL with empty path.
            return Url::parse(&format!("{SASS_RELATIVE}:"))
                .map(|u| SassUrl(u, WrappedRelative(false)));
        }
        match Url::parse(raw) {
            Ok(url) => Ok(SassUrl(url, WrappedRelative(false))),
            Err(url::ParseError::RelativeUrlWithoutBase) => {
                Url::parse(&format!("{SASS_RELATIVE}:{raw}"))
                    .map(|u| SassUrl(u, WrappedRelative(false)))
            }
            Err(e) => Err(e),
        }
    }

    /// Convert an absolute filesystem path to a `file://` SassUrl.
    ///
    /// The `()` error carries no detail by design: every caller maps it to a
    /// context-rich [`crate::common::SassError`] at the call site.
    #[allow(clippy::result_unit_err)]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn file_url_from_abs_path(path: &str) -> Result<Self, ()> {
        if let Ok(url) = Url::from_file_path(path) {
            return Ok(SassUrl(url, WrappedRelative(false)));
        }
        // `Url::from_file_path` rejects drive-less absolute paths on Windows
        // (`/main.scss`, or `\main.scss` once `clean_path` normalizes path
        // separators to `\`), while Dart's `Uri.file` accepts them as
        // root-relative. Normalize `\` first — Windows-only, since on unix
        // `\` is an ordinary filename character and normalizing there would
        // wrongly accept the relative path `\foo` as `/foo`. (On unix this
        // branch is reachable only for relative paths, which still fail
        // below exactly as before.)
        #[cfg(target_os = "windows")]
        let path = path.replace('\\', "/");
        #[cfg(not(target_os = "windows"))]
        let path = path.to_string();
        if !path.starts_with('/') {
            return Err(());
        }
        Url::parse(&format!("file://{path}"))
            .map(|u| SassUrl(u, WrappedRelative(false)))
            .map_err(|_| ())
    }

    /// Convert an absolute filesystem path to a `file://` SassUrl.
    /// On wasm, manually encodes each path segment and constructs the URL via
    /// `Url::parse`, since `Url::from_file_path` is not available on wasm.
    ///
    /// The `()` error carries no detail by design: every caller maps it to a
    /// context-rich [`crate::common::SassError`] at the call site.
    #[allow(clippy::result_unit_err)]
    #[cfg(target_arch = "wasm32")]
    pub fn file_url_from_abs_path(path: &str) -> Result<Self, ()> {
        if !path.starts_with('/') {
            return Err(());
        }
        // Mirrors Url::from_file_path's SPECIAL_PATH_SEGMENT encoding.
        // CONTROLS + SP + '"' + '<' + '>' + '`' + '#' + '?' + '{' + '}' + '/' + '%' + '\'
        const FILE_SEGMENT: &AsciiSet = &CONTROLS
            .add(b' ')
            .add(b'"')
            .add(b'<')
            .add(b'>')
            .add(b'`')
            .add(b'#')
            .add(b'?')
            .add(b'{')
            .add(b'}')
            .add(b'/')
            .add(b'%')
            .add(b'\\');

        let encoded_path: String = path
            .split('/')
            .skip(1)
            .map(|seg| utf8_percent_encode(seg, FILE_SEGMENT).to_string())
            .collect::<Vec<_>>()
            .join("/");

        let url_str = if encoded_path.is_empty() {
            "file:///".to_string()
        } else {
            format!("file:///{encoded_path}")
        };

        Url::parse(&url_str)
            .map(|u| SassUrl(u, WrappedRelative(false)))
            .map_err(|_| ())
    }

    // ── Access ─────────────────────────────────────────────────────

    pub fn as_url(&self) -> &Url {
        &self.0
    }

    // ── Scheme-aware queries ────────────────────────────────────────

    /// Scheme. Returns `""` for relative URLs — matches Go `url.Scheme`
    /// and Dart `Uri.scheme` for scheme-less URLs.
    pub fn scheme(&self) -> &str {
        if self.is_relative() {
            ""
        } else {
            self.0.scheme()
        }
    }

    /// Whether this URL has a scheme. Relative URLs don't — matches Dart
    /// `Uri.hasScheme`.
    pub fn has_scheme(&self) -> bool {
        !self.scheme().is_empty()
    }

    pub fn is_relative(&self) -> bool {
        self.0.scheme() == SASS_RELATIVE
    }

    pub fn is_file(&self) -> bool {
        self.0.scheme() == "file"
    }

    /// True for URLs that should be resolved against a file base.
    pub fn is_file_like(&self) -> bool {
        self.is_file() || self.is_relative()
    }

    // ── Delegates ───────────────────────────────────────────────────

    pub fn path(&self) -> &str {
        self.0.path()
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn join(&self, p: &str) -> Result<Self, url::ParseError> {
        self.0.join(p).map(|u| SassUrl(u, WrappedRelative(false)))
    }

    pub fn cannot_be_a_base(&self) -> bool {
        self.0.cannot_be_a_base()
    }

    pub fn has_host(&self) -> bool {
        self.0.has_host()
    }

    /// Filesystem path for passing a `file:` URL to the [`Io`] layer.
    ///
    /// On Windows this is a proper conversion (`C:\…`, percent-decoded);
    /// when the URL has no drive letter (unix-style fixture paths used with
    /// in-memory IO), `to_file_path` fails and the cleaned URL path is used
    /// instead. On other platforms this is the raw URL path — existing
    /// behavior, byte-identical.
    ///
    /// [`Io`]: crate::io::Io
    /// Filesystem path for `file:` URLs, or `None` when unrepresentable —
    /// including on `wasm32`, where `Url::to_file_path` doesn't exist.
    pub fn to_file_path_string(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0
                .to_file_path()
                .map(|p| p.to_string_lossy().into_owned())
                .ok()
        }
    }

    /// Filesystem path for passing a `file:` URL to the [`Io`] layer.
    ///
    /// On Windows this is a proper conversion (`C:\…`, percent-decoded);
    /// when the URL has no drive letter (unix-style fixture paths used with
    /// in-memory IO), `to_file_path` fails and the cleaned URL path is used
    /// instead. On other platforms this is the raw URL path — existing
    /// behavior, byte-identical.
    ///
    /// [`Io`]: crate::io::Io
    pub fn fs_path(&self) -> String {
        #[cfg(target_os = "windows")]
        {
            self.to_file_path_string()
                .unwrap_or_else(|| clean_path(self.path()))
        }
        #[cfg(not(target_os = "windows"))]
        {
            self.path().to_string()
        }
    }

    pub fn query(&self) -> Option<&str> {
        self.0.query()
    }

    pub fn fragment(&self) -> Option<&str> {
        self.0.fragment()
    }

    // ── Resolution ─────────────────────────────────────────────────

    /// Resolve a URI reference string against this SassUrl.
    /// Handles opaque/cannot-be-a-base URLs (like `sass:color`).
    /// Matches Dart's `Uri.resolve`.
    pub fn resolve(&self, reference: &str) -> Option<SassUrl> {
        if let Ok(abs) = SassUrl::parse(reference) {
            if !abs.is_relative() && !abs.0.scheme().is_empty() {
                return Some(abs);
            }
        }

        if !self.cannot_be_a_base() {
            return self.join(reference).ok();
        }

        let base_path = self.path();

        if let Some(rest) = reference.strip_prefix('/') {
            let cleaned = path_clean(rest);
            let new_url = format!("{}:{cleaned}", self.0.scheme());
            return Url::parse(&new_url)
                .map(|u| SassUrl(u, WrappedRelative(false)))
                .ok();
        }

        let dir = base_path.rfind('/').map(|i| &base_path[..i]).unwrap_or("");
        let resolved = if dir.is_empty() {
            reference.to_string()
        } else {
            format!("{dir}/{reference}")
        };
        let cleaned = path_clean(&resolved);
        let new_url = if cleaned.is_empty() {
            format!("{}:", self.0.scheme())
        } else {
            format!("{}:{cleaned}", self.0.scheme())
        };
        Url::parse(&new_url)
            .map(|u| SassUrl(u, WrappedRelative(false)))
            .ok()
    }

    /// Whether this `file:` URL was produced by resolving a `sass-relative:`
    /// URL against a `file:` base (see `resolve_file_path`). The filesystem
    /// importer's load-path fallback applies to these only.
    pub fn is_wrapped_relative(&self) -> bool {
        self.1 .0
    }

    /// Tag a `file:` URL as wrapped-relative (set by `resolve_file_path`).
    pub(crate) fn mark_wrapped_relative(mut self) -> Self {
        self.1 .0 = true;
        self
    }

    /// Infallible fallback for URLs that fail strict parsing: wraps the raw
    /// string as a `sass-relative:` URL (the `url` crate accepts any string
    /// after an opaque scheme). Used by `DynamicImport::url`, whose inputs
    /// are pre-validated by the parser — this path is a no-`panic` guard,
    /// never the normal route.
    pub fn parse_relative_fallback(raw: &str) -> Self {
        Url::parse(&format!("{SASS_RELATIVE}:{raw}"))
            .map(|u| SassUrl(u, WrappedRelative(false)))
            .unwrap_or_else(|_| {
                Url::parse(&format!("{SASS_RELATIVE}:"))
                    .map(|u| SassUrl(u, WrappedRelative(false)))
                    .expect("empty sass-relative: URL must parse")
            })
    }
}

// ── Display — strips sass-relative: prefix ─────────────────────────────

impl Display for SassUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.is_relative() {
            f.write_str(self.path())
        } else {
            Display::fmt(&self.0, f)
        }
    }
}

// ── Path resolution utilities ────────────────────────────────────────────

fn path_clean(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    let result = parts.join("/");
    if result == "." {
        String::new()
    } else {
        result
    }
}

/// Resolve a file-like URL against a base file URL.
///
/// The key fix: sass-relative paths preserve `..` so `base.join(path)`
/// resolves correctly (unlike the old `file:///../foo` → normalized away).
/// A `sass-relative:` input resolved against a `file:` base is tagged
/// wrapped-relative so the filesystem importer's load-path fallback can tell
/// it apart from a genuine absolute `file:` URL.
pub fn resolve_file_path(url: &SassUrl, base: &SassUrl) -> Option<SassUrl> {
    if !url.is_file_like() || !base.is_file() {
        return None;
    }
    let rel = url.path();
    if rel.is_empty() {
        return Some(base.clone());
    }
    let joined = base.join(rel).ok()?;
    if url.is_relative() {
        return Some(joined.mark_wrapped_relative());
    }
    Some(joined)
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // ── Parse: absolute URLs (unchanged behavior) ──────────────────

    #[test]
    fn test_parse_opaque() {
        let u = SassUrl::parse("sass:color").unwrap();
        assert_eq!(u.scheme(), "sass");
        assert_eq!(u.path(), "color");
    }

    #[test]
    fn test_parse_opaque_foo_bar() {
        let u = SassUrl::parse("u:foo/bar").unwrap();
        assert_eq!(u.scheme(), "u");
        assert_eq!(u.path(), "foo/bar");
    }

    #[test]
    fn test_parse_opaque_orange() {
        let u = SassUrl::parse("u:orange").unwrap();
        assert_eq!(u.scheme(), "u");
        assert_eq!(u.path(), "orange");
    }

    #[test]
    fn test_parse_percent_encoded() {
        let u = SassUrl::parse("pkg:%66oo").unwrap();
        assert_eq!(u.scheme(), "pkg");
        assert_eq!(u.path(), "%66oo");
        assert_eq!(u.as_str(), "pkg:%66oo");
    }

    #[test]
    fn test_parse_percent_encoded_path() {
        let u = SassUrl::parse("u:%25percent").unwrap();
        assert_eq!(u.scheme(), "u");
        assert_eq!(u.path(), "%25percent");
        assert_eq!(u.as_str(), "u:%25percent");
    }

    #[test]
    fn test_parse_file() {
        let u = SassUrl::parse("file:///tmp/test.scss").unwrap();
        assert_eq!(u.scheme(), "file");
        assert_eq!(u.path(), "/tmp/test.scss");
    }

    #[test]
    fn test_parse_http() {
        let u = SassUrl::parse("http://example.com/style.scss").unwrap();
        assert_eq!(u.scheme(), "http");
        assert_eq!(u.path(), "/style.scss");
    }

    // ── Parse: relative URLs ────────────────────────────────────────

    #[test]
    fn test_parse_relative_path() {
        let u = SassUrl::parse("callable/arguments/function/utils").unwrap();
        assert!(u.is_relative());
        assert_eq!(u.scheme(), "");
        assert_eq!(u.path(), "callable/arguments/function/utils");
        assert_eq!(u.to_string(), "callable/arguments/function/utils");
    }

    #[test]
    fn test_parse_relative_parent() {
        let u = SassUrl::parse("../utils").unwrap();
        assert!(u.is_relative());
        assert_eq!(u.scheme(), "");
        assert_eq!(u.path(), "../utils");
        assert_eq!(u.to_string(), "../utils");
    }

    #[test]
    fn test_parse_relative_current() {
        let u = SassUrl::parse("./utils").unwrap();
        assert!(u.is_relative());
        assert_eq!(u.scheme(), "");
        assert_eq!(u.path(), "./utils");
        assert_eq!(u.to_string(), "./utils");
    }

    #[test]
    fn test_parse_relative_simple() {
        let u = SassUrl::parse("utils").unwrap();
        assert!(u.is_relative());
        assert_eq!(u.scheme(), "");
        assert_eq!(u.path(), "utils");
        assert_eq!(u.to_string(), "utils");
    }

    #[test]
    fn test_parse_relative_scss() {
        let u = SassUrl::parse("relative/path/to/file.scss").unwrap();
        assert!(u.is_relative());
        assert_eq!(u.scheme(), "");
        assert_eq!(u.path(), "relative/path/to/file.scss");
        assert_eq!(u.to_string(), "relative/path/to/file.scss");
    }

    // ── Resolve tests ───────────────────────────────────────────────

    #[test]
    fn test_resolve_opaque_relative() {
        let base = SassUrl::parse("sass:color").unwrap();
        let resolved = base.resolve("red").unwrap();
        assert_eq!(resolved.scheme(), "sass");
        assert_eq!(resolved.path(), "red");
    }

    #[test]
    fn test_resolve_opaque_absolute_path() {
        let base = SassUrl::parse("sass:color").unwrap();
        let resolved = base.resolve("/red").unwrap();
        assert_eq!(resolved.path(), "red");
    }

    #[test]
    fn test_resolve_opaque_nested() {
        let base = SassUrl::parse("sass:color/foo/bar").unwrap();
        let resolved = base.resolve("../baz").unwrap();
        assert_eq!(resolved.path(), "color/baz");
    }

    #[test]
    fn test_resolve_file() {
        let base = SassUrl::parse("file:///foo/bar.scss").unwrap();
        let resolved = base.resolve("../baz/qux.scss").unwrap();
        assert_eq!(resolved.path(), "/baz/qux.scss");
    }

    #[test]
    fn test_resolve_http() {
        let base = SassUrl::parse("https://example.com/css/style.scss").unwrap();
        let resolved = base.resolve("theme.scss").unwrap();
        assert_eq!(resolved.path(), "/css/theme.scss");
    }

    #[test]
    fn test_resolve_opaque_sibling() {
        let base = SassUrl::parse("sass:color").unwrap();
        let resolved = base.resolve("math").unwrap();
        assert_eq!(resolved.path(), "math");
    }

    // ── file_url_from_abs_path tests ────────────────────────────────

    #[test]
    fn test_file_url_unix() {
        let url = SassUrl::file_url_from_abs_path("/tmp/foo.txt").unwrap();
        assert!(url.is_file());
        assert_eq!(url.path(), "/tmp/foo.txt");
    }

    #[test]
    fn test_file_url_rejects_relative() {
        assert!(SassUrl::file_url_from_abs_path("../foo.txt").is_err());
    }

    #[test]
    fn test_file_url_rejects_empty() {
        assert!(SassUrl::file_url_from_abs_path("").is_err());
    }

    #[test]
    fn test_file_url_accepts_bare_root_path() {
        // Dart's `Uri.file` accepts drive-less absolute paths as
        // root-relative; `Url::from_file_path` rejects them on Windows, where
        // the fallback below produces the identical `file://` URL unix
        // produces directly.
        let url = SassUrl::file_url_from_abs_path("/main.scss").unwrap();
        assert!(url.is_file());
        assert_eq!(url.as_str(), "file:///main.scss");
        // On Windows the same path arrives backslash-normalized from
        // `clean_path` (`\main.scss`); it must resolve identically.
        #[cfg(target_os = "windows")]
        {
            let url = SassUrl::file_url_from_abs_path("\\main.scss").unwrap();
            assert!(url.is_file());
            assert_eq!(url.as_str(), "file:///main.scss");
        }
    }

    // ── resolve_file_path tests ─────────────────────────────────────

    #[test]
    fn test_resolve_file_path_relative() {
        let url = SassUrl::parse("other").unwrap();
        let base = SassUrl::parse("file:///dir/style.scss").unwrap();
        let resolved = resolve_file_path(&url, &base).unwrap();
        assert_eq!(resolved.path(), "/dir/other");
    }

    #[test]
    fn test_resolve_file_path_subdir() {
        let url = SassUrl::parse("subdir/module").unwrap();
        let base = SassUrl::parse("file:///dir/style.scss").unwrap();
        let resolved = resolve_file_path(&url, &base).unwrap();
        assert_eq!(resolved.path(), "/dir/subdir/module");
    }

    #[test]
    fn test_resolve_file_path_not_file_scheme() {
        let url = SassUrl::parse("sass:color").unwrap();
        let base = SassUrl::parse("file:///dir/style.scss").unwrap();
        assert!(resolve_file_path(&url, &base).is_none());
    }

    #[test]
    fn test_resolve_file_path_non_file_base() {
        let url = SassUrl::parse("file:///other").unwrap();
        let base = SassUrl::parse("sass:color").unwrap();
        assert!(resolve_file_path(&url, &base).is_none());
    }

    // ── Relative URL preservation (the core fix) ────────────────────

    #[test]
    fn test_relative_preserves_dotdot() {
        let u = SassUrl::parse("../test-hue").unwrap();
        assert!(u.is_relative());
        assert_eq!(u.scheme(), "");
        assert_eq!(u.path(), "../test-hue");
        assert_eq!(u.to_string(), "../test-hue");
    }

    #[test]
    fn test_resolve_dotdot_against_file_base() {
        let base = SassUrl::parse("file:///a/b/input.scss").unwrap();
        let url = SassUrl::parse("../test-hue").unwrap();
        let resolved = resolve_file_path(&url, &base).unwrap();
        assert_eq!(resolved.path(), "/a/test-hue");
    }

    #[test]
    fn test_is_file_like() {
        assert!(SassUrl::parse("file:///a.scss").unwrap().is_file_like());
        assert!(SassUrl::parse("../a.scss").unwrap().is_file_like());
        assert!(!SassUrl::parse("sass:color").unwrap().is_file_like());
    }

    #[test]
    fn test_display_strips_sass_relative() {
        let u = SassUrl::parse("../test-hue").unwrap();
        let display = u.to_string();
        assert!(!display.contains("sass-relative"));
        assert_eq!(display, "../test-hue");
    }

    #[test]
    fn test_clone_preserves_relative() {
        let u = SassUrl::parse("../test-hue").unwrap();
        let c = u.clone();
        assert!(c.is_relative());
        assert_eq!(c.path(), "../test-hue");
    }

    #[test]
    fn test_hash_works() {
        let mut set = HashSet::new();
        set.insert(SassUrl::parse("file:///a.scss").unwrap());
        set.insert(SassUrl::parse("file:///a.scss").unwrap());
        assert_eq!(set.len(), 1);
        set.insert(SassUrl::parse("../a.scss").unwrap());
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_as_url_accesses_inner() {
        let u = SassUrl::parse("../test-hue").unwrap();
        let inner = u.as_url();
        assert_eq!(inner.scheme(), SASS_RELATIVE);
        assert_eq!(inner.path(), "../test-hue");
    }

    #[test]
    fn test_relative_scheme_returns_empty() {
        let rel = SassUrl::parse("../test-hue").unwrap();
        let builtin = SassUrl::parse("sass:color").unwrap();
        assert_eq!(rel.scheme(), "");
        assert_eq!(builtin.scheme(), "sass");
    }
}

/// Builds the `data:` URL Dart uses as an `ImporterResult.sourceMapUrl` when
/// none is supplied (`Uri.dataFromString(contents, encoding: utf8)`):
/// `data:;charset=utf-8,<percent-encoded>`. Encodes with [DATA_SAFE] so the
/// output matches Dart byte-for-byte.
pub(crate) fn data_url_from_string(contents: &str) -> String {
    let encoded = utf8_percent_encode(contents, DATA_SAFE);
    format!("data:;charset=utf-8,{encoded}")
}

#[cfg(test)]
mod data_url_tests {
    use super::*;

    #[test]
    fn matches_dart_data_uri_encoding() {
        // Expected string verified byte-for-byte against the `sass` npm
        // package (`Uri.dataFromString(contents, encoding: utf8)`).
        let contents = ".x { color: red; } // $`#%&'()*+,/:;<=>?@[\\]^_{|}~ \u{e1}\u{e9}\0";
        assert_eq!(
            data_url_from_string(contents),
            "data:;charset=utf-8,.x%20%7B%20color:%20red;%20%7D%20//%20$%60%23%25&'()*+,/:;%3C=%3E?@%5B%5C%5D%5E_%7B%7C%7D~%20%C3%A1%C3%A9%00"
        );
    }

    #[test]
    fn leaves_reserved_and_unreserved_unencoded() {
        assert_eq!(
            data_url_from_string("$c: #123456;"),
            "data:;charset=utf-8,$c:%20%23123456;"
        );
        assert_eq!(
            data_url_from_string(".foo {\n  color: $c;\n}"),
            "data:;charset=utf-8,.foo%20%7B%0A%20%20color:%20$c;%0A%7D"
        );
    }
}
