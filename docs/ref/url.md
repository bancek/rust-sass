# Module: `url.rs`

Dart-compatible URL resolution. `SassUrl` is a newtype over the `url` crate's
`Url` — there is no Dart source file (it adapts `Uri` semantics where the
`url` crate diverges).

## Why the wrapper exists

Rust's `url` crate cannot represent scheme-less (relative) URLs; Dart's `Uri`
(and Go's `net/url`) allow empty scheme with paths like `../foo`. Internally,
relative URLs are stored as `sass-relative:<path>` — an opaque URL whose path
preserves `..` segments. The scheme is stripped by `Display`, so it is never
user-visible (`url.scheme().is_empty()` is the "is relative" test).

## `SassUrl`

```rust
pub struct SassUrl(Url, WrappedRelative);   // Clone, Debug, PartialEq, Eq, Hash
impl SassUrl {
    pub fn parse(raw: &str) -> Result<Self, url::ParseError>;
    pub fn parse_relative_fallback(raw: &str) -> Self;  // infallible: wraps raw as relative (DynamicImport::url never panics, like Dart Uri.parse)
    pub fn file_url_from_abs_path(path: &str) -> Result<Self, ()>;
    pub fn scheme(&self) -> &str;        // "" for relative (sass-relative:)
    pub fn is_relative(&self) -> bool;
    pub fn is_file(&self) -> bool;
    pub fn is_file_like(&self) -> bool;
    pub fn is_wrapped_relative(&self) -> bool;  // file: URL from a sass-relative: base join
    pub fn resolve(&self, reference: &str) -> Option<SassUrl>;  // Dart Uri.resolve, incl. opaque/cannot-be-a-base bases like sass:color
    pub fn join(&self, p: &str) -> Result<Self, url::ParseError>;
    // ... path(), query(), fragment(), has_host(), cannot_be_a_base(), as_str(), as_url()
}
pub fn resolve_file_path(url: &SassUrl, base: &SassUrl) -> Option<SassUrl>;
```

`resolve_file_path` preserves `..` segments for file bases (where `Url::join`
would normalize them away — Dart keeps them for `sass-relative:` display).
A `sass-relative:` input resolved against a `file:` base is tagged
wrapped-relative (`mark_wrapped_relative`); genuine absolute `file:` URLs are
never tagged. The filesystem importer's load-path fallback applies to
wrapped-relative `file:` URLs only — never to genuine absolute `file:` URLs
(Dart `filesystem.dart` resolves `file:` URLs directly only).

## `data:` URLs

`data_url_from_string` mirrors Dart's `Uri.dataFromString(..., encoding: utf8)`
byte-for-byte: RFC 3986 unreserved chars plus most reserved chars pass through
unencoded, except `#`, `[`, `]` (and space/non-ASCII, which percent-encode).
The allowlist was verified empirically against the `sass` npm package
(`ImporterResult::source_map_url` falls back to it).

## Working here

- `PartialEq`/`Eq`/`Hash` on `SassUrl` operate on the _stored_ (possibly
  `sass-relative:`-wrapped) form — two URLs that display identically but were
  built differently can compare unequal. Canonicalize before using URLs as
  map keys (see `eval/import_cache.rs`).
- `Display` strips the internal scheme; `Debug` does not. Never log `Debug`
  output into user-facing messages.
- New URL-dependent behavior needs a differential test against
  `dart run bin/sass.dart` (URL edge cases are where the `url` crate and
  Dart's `Uri` most often disagree: trailing slashes, empty segments,
  percent-encoding, Windows paths).
