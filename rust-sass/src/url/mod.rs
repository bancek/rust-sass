// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: sdk/lib/core/uri.dart (Dart 3.13.5) — Uri.parse/resolve,
// normalization, and error messages; file paths follow package:path's
// toUri/fromUri behavior.
// go-source: none (Dart's Uri has no direct Go equivalent in this port).

//! A Dart-compatible URI.
//!
//! This is a port of `dart:core`'s `Uri` for the subset Sass needs: parsing
//! (including relative references), component normalization, resolution,
//! file-path conversion (`package:path`), and `data:` URIs. It deliberately
//! does not wrap the WHATWG `url` crate: Dart accepts URLs that crate cannot
//! represent and normalizes differently.

use bumpalo::Bump;
use std::fmt::{self, Display, Formatter};
use std::hash::{Hash, Hasher};

use tables::{CHAR_TABLES, SCANNER_TABLES};

// ---------------------------------------------------------------------------
// Dart Uri constants
// ---------------------------------------------------------------------------

const PERCENT: u16 = 0x25;
const BACKSLASH: u16 = 0x5c;
const COLON: u16 = 0x3a;
const DOT: u16 = 0x2e;
const LEFT_BRACKET: u16 = 0x5b;
const RIGHT_BRACKET: u16 = 0x5d;
const LOWER_CASE_A: u16 = 0x61;
const LOWER_CASE_F: u16 = 0x66;
const LOWER_CASE_V: u16 = 0x76;
const LOWER_CASE_Z: u16 = 0x7a;
const UPPER_CASE_A: u16 = 0x41;
const UPPER_CASE_Z: u16 = 0x5a;
const DIGIT_0: u16 = 0x30;
const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

const UNRESERVED_MASK: u16 = 0x0001;
const SCHEME_MASK: u16 = 0x0008;
const USERINFO_MASK: u16 = 0x0010;
const REG_NAME_MASK: u16 = 0x0020;
const PATH_CHAR_OR_SLASH_MASK: u16 = 0x0080;
const QUERY_CHAR_MASK: u16 = 0x0100;
const GEN_DELIMITERS_MASK: u16 = 0x0400;

// Scanner result indices (Dart `_schemeEndIndex`, ...).
const SCHEME_END_INDEX: usize = 1;
const HOST_START_INDEX: usize = 2;
const PORT_START_INDEX: usize = 3;
const PATH_START_INDEX: usize = 4;
const QUERY_START_INDEX: usize = 5;
const FRAGMENT_START_INDEX: usize = 6;
const NOT_SIMPLE_INDEX: usize = 7;

const URI_START: usize = 0;
const NON_SIMPLE_END_STATES: usize = 14;
const SCHEME_START: usize = 20;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A Dart `FormatException` raised by `Uri.parse`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DartUriError {
    pub message: String,
}

impl DartUriError {
    fn new(message: impl Into<String>) -> Self {
        DartUriError {
            message: message.into(),
        }
    }
}

impl Display for DartUriError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DartUriError {}

// ---------------------------------------------------------------------------
// OwnedUrl
// ---------------------------------------------------------------------------

/// Which Dart `Uri` implementation a value came from.
///
/// Dart parses `".."` as a full `_Uri` and `"../"` as a `_SimpleUri`; both
/// have the same public components and compare equal, but resolve differently.
/// The kind is not part of equality, hashing, or display; it only selects
/// `_simpleMerge` in [`OwnedUrl::resolve_uri`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UriKind {
    Simple,
    Full,
}

/// Read-only access to a Dart-compatible URI.
///
/// Implemented by [`OwnedUrl`] (heap-owned) and [`ArenaUrl`] (arena-backed).
/// The getters mirror `dart:core`'s `Uri` exactly, including the
/// empty-string/zero sentinels for absent components and the separate
/// `has_*` predicates.
pub trait Url: Display {
    fn scheme(&self) -> &str;
    fn user_info(&self) -> &str;
    /// Dart's `Uri.host` getter: strips `[...]` from IP literals, but not from
    /// IPvFuture literals (`[v...]`).
    fn host(&self) -> &str;
    fn port(&self) -> i64;
    fn path(&self) -> &str;
    fn query(&self) -> &str;
    fn fragment(&self) -> &str;
    fn has_authority(&self) -> bool;
    fn has_port(&self) -> bool;
    fn has_query(&self) -> bool;
    fn has_fragment(&self) -> bool;
    fn as_str(&self) -> &str;
    fn to_file_path_string(&self) -> Option<String>;

    fn has_scheme(&self) -> bool {
        !self.scheme().is_empty()
    }

    fn has_empty_path(&self) -> bool {
        self.path().is_empty()
    }

    fn has_absolute_path(&self) -> bool {
        self.path().starts_with('/')
    }

    fn is_absolute(&self) -> bool {
        self.has_scheme() && !self.has_fragment()
    }

    fn is_relative(&self) -> bool {
        !self.has_scheme()
    }

    fn is_file(&self) -> bool {
        self.scheme() == "file"
    }

    fn is_file_like(&self) -> bool {
        self.is_file() || self.is_relative()
    }

    /// Whether this URI cannot be used as a base (no authority and a
    /// rootless path).
    fn cannot_be_a_base(&self) -> bool {
        !self.has_authority() && !self.path().starts_with('/')
    }
}

/// A Dart-compatible URI.
#[derive(Clone, Debug)]
pub struct OwnedUrl {
    scheme: String,
    user_info: String,
    host: Option<String>,
    port: Option<i64>,
    path: String,
    query: Option<String>,
    fragment: Option<String>,
    /// Cached Dart `toString()` form.
    text: String,
    kind: UriKind,
}

impl OwnedUrl {
    fn make(
        scheme: String,
        user_info: String,
        host: Option<String>,
        port: Option<i64>,
        path: String,
        query: Option<String>,
        fragment: Option<String>,
    ) -> Self {
        let mut url = OwnedUrl {
            scheme,
            user_info,
            host,
            port,
            path,
            query,
            fragment,
            text: String::new(),
            kind: UriKind::Full,
        };
        url.text = url.initialize_text();
        url
    }

    /// Parses `raw` exactly as Dart's `Uri.parse` does.
    pub fn parse(raw: &str) -> Result<Self, DartUriError> {
        let units: Vec<u16> = raw.encode_utf16().collect();
        if let Some(rest) = strip_data_scheme(&units) {
            return parse_data_uri(&units, rest);
        }
        parse_reference(&units)
    }

    // --- Getters ---------------------------------------------------------

    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    pub fn has_scheme(&self) -> bool {
        !self.scheme.is_empty()
    }

    pub fn user_info(&self) -> &str {
        &self.user_info
    }

    /// Dart's `Uri.host` getter: strips `[...]` from IP literals, but not from
    /// IPvFuture literals (`[v...]`).
    pub fn host(&self) -> &str {
        match &self.host {
            None => "",
            Some(host) => {
                if host.starts_with('[') && !host.starts_with("[v") {
                    &host[1..host.len() - 1]
                } else {
                    host
                }
            }
        }
    }

    pub fn has_authority(&self) -> bool {
        self.host.is_some()
    }

    /// Dart's `Uri.port` getter (default port for the scheme when absent).
    pub fn port(&self) -> i64 {
        self.port.unwrap_or_else(|| default_port(&self.scheme))
    }

    pub fn has_port(&self) -> bool {
        self.port.is_some()
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn has_empty_path(&self) -> bool {
        self.path.is_empty()
    }

    pub fn has_absolute_path(&self) -> bool {
        self.path.starts_with('/')
    }

    pub fn query(&self) -> &str {
        self.query.as_deref().unwrap_or("")
    }

    pub fn has_query(&self) -> bool {
        self.query.is_some()
    }

    pub fn fragment(&self) -> &str {
        self.fragment.as_deref().unwrap_or("")
    }

    pub fn has_fragment(&self) -> bool {
        self.fragment.is_some()
    }

    pub fn is_absolute(&self) -> bool {
        self.has_scheme() && self.fragment.is_none()
    }

    pub fn is_relative(&self) -> bool {
        !self.has_scheme()
    }

    pub fn is_file(&self) -> bool {
        self.scheme == "file"
    }

    pub fn is_file_like(&self) -> bool {
        self.is_file() || self.is_relative()
    }

    /// Whether this URI cannot be used as a base (no authority and a
    /// rootless path).
    pub fn cannot_be_a_base(&self) -> bool {
        !self.has_authority() && !self.path.starts_with('/')
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    // --- Resolution -------------------------------------------------------

    /// Resolves `reference` against this URI using Dart's `Uri.resolve`.
    pub fn resolve(&self, reference: &str) -> Result<OwnedUrl, DartUriError> {
        Ok(self.resolve_uri(&OwnedUrl::parse(reference)?))
    }

    /// Resolves an already-parsed reference.
    pub fn resolve_uri(&self, reference: &OwnedUrl) -> OwnedUrl {
        let target_scheme;
        let mut target_user_info = String::new();
        let mut target_host: Option<String> = None;
        let mut target_port: Option<i64> = None;
        let target_path;
        let mut target_query: Option<String> = None;
        let mut used_simple_merge = false;

        if reference.has_scheme() {
            target_scheme = reference.scheme.clone();
            if reference.has_authority() {
                target_user_info = reference.user_info.clone();
                target_host = reference.host.clone();
                target_port = reference.port;
            }
            target_path = remove_dot_segments(&reference.path);
            if reference.has_query() {
                target_query = reference.query.clone();
            }
        } else {
            target_scheme = self.scheme.clone();
            if reference.has_authority() {
                target_user_info = reference.user_info.clone();
                target_host = reference.host.clone();
                target_port = make_port(reference.port, &target_scheme);
                target_path = remove_dot_segments(&reference.path);
                if reference.has_query() {
                    target_query = reference.query.clone();
                }
            } else {
                target_user_info = self.user_info.clone();
                target_host = self.host.clone();
                target_port = self.port;
                if reference.has_empty_path() {
                    target_path = self.path.clone();
                    if reference.has_query() {
                        target_query = reference.query.clone();
                    } else {
                        target_query = self.query.clone();
                    }
                } else if reference.has_absolute_path() {
                    target_path = remove_dot_segments(&reference.path);
                    if reference.has_query() {
                        target_query = reference.query.clone();
                    }
                } else if self.has_empty_path() {
                    if !self.has_authority() {
                        if !self.has_scheme() {
                            target_path = reference.path.clone();
                        } else {
                            target_path = remove_dot_segments(&reference.path);
                        }
                    } else {
                        target_path = remove_dot_segments(&format!("/{}", reference.path));
                    }
                    if reference.has_query() {
                        target_query = reference.query.clone();
                    }
                } else if self.kind == UriKind::Simple
                    && reference.kind == UriKind::Simple
                    && !self.has_scheme()
                    && !self.has_authority()
                    && !self.has_absolute_path()
                {
                    target_path = simple_merge_relative(&self.path, &reference.path);
                    if reference.has_query() {
                        target_query = reference.query.clone();
                    }
                    used_simple_merge = true;
                } else {
                    let merged = merge_paths(&self.path, &reference.path);
                    target_path = if self.has_scheme()
                        || self.has_authority()
                        || self.has_absolute_path()
                    {
                        remove_dot_segments(&merged)
                    } else {
                        normalize_relative_path(&merged, self.has_scheme() || self.has_authority())
                    };
                    if reference.has_query() {
                        target_query = reference.query.clone();
                    }
                }
            }
        }

        let fragment = reference.fragment.clone();
        let mut url = OwnedUrl::make(
            target_scheme,
            target_user_info,
            target_host,
            target_port,
            target_path,
            target_query,
            fragment,
        );
        url.kind = if used_simple_merge {
            UriKind::Simple
        } else {
            UriKind::Full
        };
        url
    }

    fn initialize_text(&self) -> String {
        let mut out = String::new();
        if !self.scheme.is_empty() {
            out.push_str(&self.scheme);
            out.push(':');
        }
        if self.has_authority() || self.scheme == "file" {
            out.push_str("//");
            if !self.user_info.is_empty() {
                out.push_str(&self.user_info);
                out.push('@');
            }
            if let Some(ref host) = self.host {
                out.push_str(host);
            }
            if let Some(port) = self.port {
                out.push(':');
                out.push_str(&port.to_string());
            }
        }
        out.push_str(&self.path);
        if let Some(ref query) = self.query {
            out.push('?');
            out.push_str(query);
        }
        if let Some(ref fragment) = self.fragment {
            out.push('#');
            out.push_str(fragment);
        }
        out
    }
}

impl Display for OwnedUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl PartialEq for OwnedUrl {
    fn eq(&self, other: &Self) -> bool {
        self.scheme == other.scheme
            && self.has_authority() == other.has_authority()
            && self.user_info == other.user_info
            && self.host() == other.host()
            && self.port() == other.port()
            && self.path == other.path
            && self.has_query() == other.has_query()
            && self.query() == other.query()
            && self.has_fragment() == other.has_fragment()
            && self.fragment() == other.fragment()
    }
}

impl Eq for OwnedUrl {}

impl Hash for OwnedUrl {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.scheme.hash(state);
        self.has_authority().hash(state);
        self.user_info.hash(state);
        self.host().hash(state);
        self.port().hash(state);
        self.path.hash(state);
        self.has_query().hash(state);
        self.query().hash(state);
        self.has_fragment().hash(state);
        self.fragment().hash(state);
    }
}

impl Url for OwnedUrl {
    fn scheme(&self) -> &str {
        OwnedUrl::scheme(self)
    }

    fn user_info(&self) -> &str {
        OwnedUrl::user_info(self)
    }

    fn host(&self) -> &str {
        OwnedUrl::host(self)
    }

    fn port(&self) -> i64 {
        OwnedUrl::port(self)
    }

    fn path(&self) -> &str {
        OwnedUrl::path(self)
    }

    fn query(&self) -> &str {
        OwnedUrl::query(self)
    }

    fn fragment(&self) -> &str {
        OwnedUrl::fragment(self)
    }

    fn has_authority(&self) -> bool {
        OwnedUrl::has_authority(self)
    }

    fn has_port(&self) -> bool {
        OwnedUrl::has_port(self)
    }

    fn has_query(&self) -> bool {
        OwnedUrl::has_query(self)
    }

    fn has_fragment(&self) -> bool {
        OwnedUrl::has_fragment(self)
    }

    fn as_str(&self) -> &str {
        OwnedUrl::as_str(self)
    }

    fn to_file_path_string(&self) -> Option<String> {
        OwnedUrl::to_file_path_string(self)
    }
}

/// An [`OwnedUrl`] whose components live in the compile arena.
///
/// The canonical `toString()` form is cached, like [`OwnedUrl`]. The struct is
/// `Copy`; only the components are allocated in the arena. Use
/// [`from_owned`](Self::from_owned) when an owned URL is already at hand, or
/// [`parse`](Self::parse) to parse and copy in one step.
#[derive(Clone, Copy, Debug)]
pub struct ArenaUrl<'parse> {
    scheme: &'parse str,
    user_info: &'parse str,
    host: Option<&'parse str>,
    port: Option<i64>,
    path: &'parse str,
    query: Option<&'parse str>,
    fragment: Option<&'parse str>,
    /// Cached Dart `toString()` form.
    text: &'parse str,
    kind: UriKind,
}

impl<'parse> ArenaUrl<'parse> {
    /// Copies `url`'s components into `arena` (no parsing).
    pub fn from_owned(arena: &'parse Bump, url: &OwnedUrl) -> Self {
        ArenaUrl {
            scheme: &*arena.alloc_str(&url.scheme),
            user_info: &*arena.alloc_str(&url.user_info),
            host: url.host.as_deref().map(|host| &*arena.alloc_str(host)),
            port: url.port,
            path: &*arena.alloc_str(&url.path),
            query: url.query.as_deref().map(|query| &*arena.alloc_str(query)),
            fragment: url
                .fragment
                .as_deref()
                .map(|fragment| &*arena.alloc_str(fragment)),
            text: &*arena.alloc_str(&url.text),
            kind: url.kind,
        }
    }

    /// Parses `raw` with Dart's `Uri.parse` semantics, then copies the
    /// components into `arena`.
    pub fn parse(arena: &'parse Bump, raw: &str) -> Result<Self, DartUriError> {
        Ok(Self::from_owned(arena, &OwnedUrl::parse(raw)?))
    }

    /// An owned copy of this URL, without reparsing.
    pub fn to_owned_url(&self) -> OwnedUrl {
        let mut owned = OwnedUrl::make(
            self.scheme.to_string(),
            self.user_info.to_string(),
            self.host.map(str::to_string),
            self.port,
            self.path.to_string(),
            self.query.map(str::to_string),
            self.fragment.map(str::to_string),
        );
        // `make` computes these for a freshly parsed URL; keep the arena
        // value's exact canonical form and Dart `UriKind`.
        owned.kind = self.kind;
        owned.text = self.text.to_string();
        owned
    }

    // --- Getters (mirror OwnedUrl) ---

    pub fn scheme(&self) -> &'parse str {
        self.scheme
    }

    pub fn has_scheme(&self) -> bool {
        !self.scheme.is_empty()
    }

    pub fn user_info(&self) -> &'parse str {
        self.user_info
    }

    /// Dart's `Uri.host` getter: strips `[...]` from IP literals, but not from
    /// IPvFuture literals (`[v...]`).
    pub fn host(&self) -> &'parse str {
        match self.host {
            None => "",
            Some(host) => {
                if host.starts_with('[') && !host.starts_with("[v") {
                    &host[1..host.len() - 1]
                } else {
                    host
                }
            }
        }
    }

    pub fn has_authority(&self) -> bool {
        self.host.is_some()
    }

    /// Dart's `Uri.port` getter (default port for the scheme when absent).
    pub fn port(&self) -> i64 {
        self.port.unwrap_or_else(|| default_port(self.scheme))
    }

    pub fn has_port(&self) -> bool {
        self.port.is_some()
    }

    pub fn path(&self) -> &'parse str {
        self.path
    }

    pub fn has_empty_path(&self) -> bool {
        self.path.is_empty()
    }

    pub fn has_absolute_path(&self) -> bool {
        self.path.starts_with('/')
    }

    pub fn query(&self) -> &'parse str {
        self.query.unwrap_or("")
    }

    pub fn has_query(&self) -> bool {
        self.query.is_some()
    }

    pub fn fragment(&self) -> &'parse str {
        self.fragment.unwrap_or("")
    }

    pub fn has_fragment(&self) -> bool {
        self.fragment.is_some()
    }

    pub fn is_absolute(&self) -> bool {
        self.has_scheme() && !self.has_fragment()
    }

    pub fn is_relative(&self) -> bool {
        !self.has_scheme()
    }

    pub fn is_file(&self) -> bool {
        self.scheme == "file"
    }

    pub fn is_file_like(&self) -> bool {
        self.is_file() || self.is_relative()
    }

    /// Whether this URI cannot be used as a base (no authority and a
    /// rootless path).
    pub fn cannot_be_a_base(&self) -> bool {
        !self.has_authority() && !self.path.starts_with('/')
    }

    pub fn as_str(&self) -> &'parse str {
        self.text
    }

    /// The filesystem path for a `file:` URL (Dart `Uri.toFilePath`).
    pub fn to_file_path_string(&self) -> Option<String> {
        if !self.is_file() {
            return None;
        }
        file_path::uri_to_path(self.path, self.host.filter(|host| !host.is_empty()))
    }

    /// The filesystem path for passing a `file:` URL to the [`Io`] layer.
    pub fn fs_path(&self) -> String {
        self.to_file_path_string()
            .unwrap_or_else(|| self.path.to_string())
    }
}

impl Display for ArenaUrl<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.text)
    }
}

impl PartialEq for ArenaUrl<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.scheme == other.scheme
            && self.has_authority() == other.has_authority()
            && self.user_info == other.user_info
            && self.host() == other.host()
            && self.port() == other.port()
            && self.path == other.path
            && self.has_query() == other.has_query()
            && self.query() == other.query()
            && self.has_fragment() == other.has_fragment()
            && self.fragment() == other.fragment()
    }
}

impl Eq for ArenaUrl<'_> {}

impl Hash for ArenaUrl<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.scheme.hash(state);
        self.has_authority().hash(state);
        self.user_info.hash(state);
        self.host().hash(state);
        self.port().hash(state);
        self.path.hash(state);
        self.has_query().hash(state);
        self.query().hash(state);
        self.has_fragment().hash(state);
        self.fragment().hash(state);
    }
}

impl Url for ArenaUrl<'_> {
    fn scheme(&self) -> &str {
        ArenaUrl::scheme(self)
    }

    fn user_info(&self) -> &str {
        ArenaUrl::user_info(self)
    }

    fn host(&self) -> &str {
        ArenaUrl::host(self)
    }

    fn port(&self) -> i64 {
        ArenaUrl::port(self)
    }

    fn path(&self) -> &str {
        ArenaUrl::path(self)
    }

    fn query(&self) -> &str {
        ArenaUrl::query(self)
    }

    fn fragment(&self) -> &str {
        ArenaUrl::fragment(self)
    }

    fn has_authority(&self) -> bool {
        ArenaUrl::has_authority(self)
    }

    fn has_port(&self) -> bool {
        ArenaUrl::has_port(self)
    }

    fn has_query(&self) -> bool {
        ArenaUrl::has_query(self)
    }

    fn has_fragment(&self) -> bool {
        ArenaUrl::has_fragment(self)
    }

    fn as_str(&self) -> &str {
        ArenaUrl::as_str(self)
    }

    fn to_file_path_string(&self) -> Option<String> {
        ArenaUrl::to_file_path_string(self)
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn parse_reference(units: &[u16]) -> Result<OwnedUrl, DartUriError> {
    let start = 0usize;
    let end = units.len();

    let mut indices: [isize; 8] = [0, -1, -1, -1, 0, end as isize, end as isize, -1];
    let mut state = scan(units, start, end, URI_START, &mut indices);
    if state >= NON_SIMPLE_END_STATES {
        indices[NOT_SIMPLE_INDEX] = end as isize;
    }
    let scheme_end = indices[SCHEME_END_INDEX];
    if scheme_end >= start as isize {
        state = scan(
            units,
            start,
            scheme_end as usize,
            SCHEME_START,
            &mut indices,
        );
        if state == SCHEME_START {
            indices[NOT_SIMPLE_INDEX] = scheme_end;
        }
    }

    let host_start = indices[HOST_START_INDEX] + 1;
    let mut port_start = indices[PORT_START_INDEX];
    let mut path_start = indices[PATH_START_INDEX];
    let mut query_start = indices[QUERY_START_INDEX];
    let fragment_start = indices[FRAGMENT_START_INDEX];

    if fragment_start < query_start {
        query_start = fragment_start;
    }
    if path_start < host_start {
        path_start = query_start;
    } else if path_start <= scheme_end {
        path_start = scheme_end + 1;
    }
    if port_start < host_start {
        port_start = path_start;
    }

    let mut is_simple = indices[NOT_SIMPLE_INDEX] < start as isize;
    if is_simple
        && (host_start > scheme_end + 3
            || (port_start > start as isize && port_start + 1 == path_start)
            || starts_with_at(units, path_start as usize, "\\")
            || (host_start > start as isize
                && (starts_with_at(units, (host_start - 1) as usize, "\\")
                    || starts_with_at(units, (host_start - 2) as usize, "\\")))
            || (query_start < end as isize
                && ((query_start == path_start + 2
                    && starts_with_at(units, path_start as usize, ".."))
                    || (query_start > path_start + 2
                        && starts_with_at(units, (query_start - 3) as usize, "/..")))))
    {
        is_simple = false;
    }

    let scheme = if scheme_end > start as isize {
        make_scheme(units, start, scheme_end as usize)?
    } else if scheme_end == start as isize {
        return Err(DartUriError::new("Invalid empty scheme"));
    } else {
        String::new()
    };

    let is_lowercase_file_scheme = scheme_end == 4 && units.starts_with(&[0x66, 0x69, 0x6c, 0x65]);

    let user_info = if host_start > start as isize {
        let user_info_start = scheme_end + 3;
        if user_info_start < host_start {
            make_user_info(units, user_info_start as usize, (host_start - 1) as usize)?
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let mut host = if host_start > start as isize {
        Some(make_host(
            units,
            host_start as usize,
            port_start as usize,
            false,
        )?)
    } else {
        None
    };

    let port = if port_start + 1 < path_start {
        let port_number = parse_dart_int(&utf16_string(
            units,
            (port_start + 1) as usize,
            path_start as usize,
        ))
        .ok_or_else(|| DartUriError::new("Invalid port"))?;
        make_port(Some(port_number), &scheme)
    } else {
        None
    };

    // Dart's fast path gives `file:` URIs an empty authority before invoking
    // `_Uri.notSimple`. Replicate that so `hasAuthority` and `toString` match.
    if is_lowercase_file_scheme && host.is_none() {
        host = Some(String::new());
    }

    let path = make_path(
        units,
        path_start as usize,
        query_start as usize,
        &scheme,
        host.is_some(),
    )?;

    let query = if query_start < fragment_start {
        Some(make_query(
            units,
            (query_start + 1) as usize,
            fragment_start as usize,
        )?)
    } else {
        None
    };

    let fragment = if fragment_start < end as isize {
        Some(make_fragment(units, (fragment_start + 1) as usize, end)?)
    } else {
        None
    };

    let mut url = OwnedUrl::make(scheme, user_info, host, port, path, query, fragment);
    url.kind = if is_simple {
        UriKind::Simple
    } else {
        UriKind::Full
    };
    Ok(url)
}

/// Dart `_scan`.
fn scan(
    units: &[u16],
    start: usize,
    end: usize,
    mut state: usize,
    indices: &mut [isize; 8],
) -> usize {
    const STATE_TABLE_SIZE: usize = 0x60;
    const CHAR_XOR: u16 = 96;
    const XOR_CHAR_LIMIT: u16 = 95;
    const INVALID_CHAR: usize = 0x7f ^ 96;

    for (i, &unit) in units.iter().enumerate().take(end).skip(start) {
        let mut ch = unit ^ CHAR_XOR;
        if ch > XOR_CHAR_LIMIT {
            ch = INVALID_CHAR as u16;
        }
        let transition = SCANNER_TABLES[state * STATE_TABLE_SIZE + ch as usize];
        state = (transition & 31) as usize;
        indices[(transition >> 5) as usize] = i as isize;
    }
    state
}

fn utf16_string(units: &[u16], start: usize, end: usize) -> String {
    String::from_utf16_lossy(&units[start..end])
}

// ---------------------------------------------------------------------------
// Component normalization (Dart `_normalize`, `_make*`)
// ---------------------------------------------------------------------------

fn normalize(
    units: &[u16],
    start: usize,
    end: usize,
    char_mask: u16,
    escape_delimiters: bool,
    replace_backslash: bool,
) -> Result<Option<String>, DartUriError> {
    let mut buffer: Option<String> = None;
    let mut section_start = start;
    let mut index = start;
    while index < end {
        let ch = units[index];
        if ch < 127 && (CHAR_TABLES[ch as usize] & char_mask) != 0 {
            index += 1;
            continue;
        }
        let (replacement, source_length) = if ch == PERCENT {
            match normalize_escape(units, index, false) {
                None => {
                    index += 3;
                    continue;
                }
                Some(replacement) if replacement == "%" => ("%25".to_string(), 1),
                Some(replacement) => (replacement, 3),
            }
        } else if ch == BACKSLASH && replace_backslash {
            ("/".to_string(), 1)
        } else if !escape_delimiters && is_general_delimiter(ch) {
            return Err(DartUriError::new("Invalid character"));
        } else {
            let (code_point, source_length) = code_point_at(units, index, end);
            (escape_char(code_point), source_length)
        };
        let out = buffer.get_or_insert_with(String::new);
        out.push_str(&utf16_string(units, section_start, index));
        out.push_str(&replacement);
        index += source_length;
        section_start = index;
    }
    match buffer {
        None => Ok(None),
        Some(mut out) => {
            out.push_str(&utf16_string(units, section_start, end));
            Ok(Some(out))
        }
    }
}

fn normalize_or_substring(
    units: &[u16],
    start: usize,
    end: usize,
    char_mask: u16,
    escape_delimiters: bool,
    replace_backslash: bool,
) -> Result<String, DartUriError> {
    Ok(normalize(
        units,
        start,
        end,
        char_mask,
        escape_delimiters,
        replace_backslash,
    )?
    .unwrap_or_else(|| utf16_string(units, start, end)))
}

fn normalize_escape(units: &[u16], index: usize, lower_case: bool) -> Option<String> {
    if index + 2 >= units.len() {
        return Some("%".to_string());
    }
    let first_digit = units[index + 1];
    let second_digit = units[index + 2];
    let first_value = hex_digit_value(first_digit);
    let second_value = hex_digit_value(second_digit);
    if first_value < 0 || second_value < 0 {
        return Some("%".to_string());
    }
    let mut value = (first_value * 16 + second_value) as u16;
    if is_unreserved_char(value) {
        if lower_case && (UPPER_CASE_A..=UPPER_CASE_Z).contains(&value) {
            value |= 0x20;
        }
        return Some(char::from_u32(value as u32)?.to_string());
    }
    if first_digit >= LOWER_CASE_A || second_digit >= LOWER_CASE_A {
        return Some(utf16_string(units, index, index + 3).to_uppercase());
    }
    None
}

fn code_point_at(units: &[u16], index: usize, end: usize) -> (u32, usize) {
    let ch = units[index];
    if (ch & 0xFC00) == 0xD800 && index + 1 < end {
        let tail = units[index + 1];
        if (tail & 0xFC00) == 0xDC00 {
            return (
                0x10000 + (((ch & 0x3ff) as u32) << 10) + (tail & 0x3ff) as u32,
                2,
            );
        }
    }
    (ch as u32, 1)
}

fn escape_char(ch: u32) -> String {
    let mut buf = [0u8; 4];
    let c = char::from_u32(ch).unwrap_or('\u{FFFD}');
    let mut out = String::new();
    for byte in c.encode_utf8(&mut buf).bytes() {
        out.push('%');
        out.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        out.push(HEX_DIGITS[(byte & 0xf) as usize] as char);
    }
    out
}

fn hex_digit_value(ch: u16) -> i32 {
    match ch {
        0x30..=0x39 => (ch - 0x30) as i32,
        0x61..=0x66 => (ch - 0x61 + 10) as i32,
        0x41..=0x46 => (ch - 0x41 + 10) as i32,
        _ => -1,
    }
}

fn is_alphabetic_character(ch: u16) -> bool {
    let lower = ch | 0x20;
    (LOWER_CASE_A..=LOWER_CASE_Z).contains(&lower)
}

fn is_unreserved_char(ch: u16) -> bool {
    ch < 127 && (CHAR_TABLES[ch as usize] & UNRESERVED_MASK) != 0
}

fn is_scheme_character(ch: u16) -> bool {
    ch < 128 && (CHAR_TABLES[ch as usize] & SCHEME_MASK) != 0
}

fn is_general_delimiter(ch: u16) -> bool {
    ch <= RIGHT_BRACKET && (CHAR_TABLES[ch as usize] & GEN_DELIMITERS_MASK) != 0
}

fn is_reg_name_char(ch: u16) -> bool {
    ch < 127 && (CHAR_TABLES[ch as usize] & REG_NAME_MASK) != 0
}

fn is_zone_id_char(ch: u16) -> bool {
    ch < 127 && (CHAR_TABLES[ch as usize] & UNRESERVED_MASK) != 0
}

fn is_ipvfuture_address_char(ch: u16) -> bool {
    if ch >= 127 {
        return true;
    }
    (CHAR_TABLES[ch as usize] & USERINFO_MASK) != 0
}

fn default_port(scheme: &str) -> i64 {
    match scheme {
        "http" => 80,
        "https" => 443,
        _ => 0,
    }
}

fn make_port(port: Option<i64>, scheme: &str) -> Option<i64> {
    match port {
        Some(port) if port == default_port(scheme) => None,
        other => other,
    }
}

fn make_scheme(units: &[u16], start: usize, end: usize) -> Result<String, DartUriError> {
    if start == end {
        return Ok(String::new());
    }
    if !is_alphabetic_character(units[start]) {
        return Err(DartUriError::new(
            "Scheme not starting with alphabetic character",
        ));
    }
    for &ch in &units[start..end] {
        if !is_scheme_character(ch) {
            return Err(DartUriError::new("Illegal scheme character"));
        }
    }
    Ok(utf16_string(units, start, end).to_lowercase())
}

fn make_user_info(units: &[u16], start: usize, end: usize) -> Result<String, DartUriError> {
    normalize_or_substring(units, start, end, USERINFO_MASK, false, false)
}

fn make_host(
    units: &[u16],
    start: usize,
    end: usize,
    strict_ipv6: bool,
) -> Result<String, DartUriError> {
    if start == end {
        return Ok(String::new());
    }
    if units[start] == LEFT_BRACKET {
        if units[end - 1] != RIGHT_BRACKET {
            return Err(DartUriError::new("Missing end `]` to match `[` in host"));
        }
        let mut zone_id = String::new();
        let mut index = end - 1;
        if units[start + 1] != LOWER_CASE_V {
            index = check_zone_id(units, start + 1, end - 1);
            if index < end - 1 {
                let zone_id_start = if starts_with_at(units, index + 1, "25") {
                    index + 3
                } else {
                    index + 1
                };
                zone_id = format!("%25{}", normalize_zone_id(units, zone_id_start, end - 1)?);
            }
        }
        let is_ipv6 = validate_ip_address(units, start + 1, index)?;
        let mut host_chars = utf16_string(units, start + 1, index);
        if is_ipv6 {
            host_chars = host_chars.to_lowercase();
        }
        return Ok(format!("[{host_chars}{zone_id}]"));
    }
    if !strict_ipv6 {
        for i in start..end {
            if units[i] == COLON {
                let mut zone_id = String::new();
                let index = check_zone_id(units, start, end);
                if index < end {
                    let zone_id_start = if starts_with_at(units, index + 1, "25") {
                        index + 3
                    } else {
                        index + 1
                    };
                    zone_id = format!("%25{}", normalize_zone_id(units, zone_id_start, end)?);
                }
                parse_ipv6_address(&utf16_string(units, start, index), 0, index - start)?;
                return Ok(format!("[{}]", utf16_string(units, start, index)) + &zone_id);
            }
        }
    }
    normalize_reg_name(units, start, end)
}

fn starts_with_at(units: &[u16], index: usize, needle: &str) -> bool {
    let needle: Vec<u16> = needle.encode_utf16().collect();
    units.len() >= index + needle.len() && units[index..index + needle.len()] == needle[..]
}

fn check_zone_id(units: &[u16], start: usize, end: usize) -> usize {
    match units[start..end].iter().position(|&ch| ch == PERCENT) {
        Some(offset) => start + offset,
        None => end,
    }
}

fn normalize_zone_id(units: &[u16], start: usize, end: usize) -> Result<String, DartUriError> {
    let mut out = String::new();
    let mut index = start;
    while index < end {
        let ch = units[index];
        if ch == PERCENT {
            match normalize_escape(units, index, true) {
                Some(replacement) if replacement == "%" => {
                    return Err(DartUriError::new("ZoneID should not contain % anymore"));
                }
                Some(replacement) => {
                    out.push_str(&replacement);
                    index += 3;
                }
                None => {
                    out.push_str(&utf16_string(units, index, index + 3));
                    index += 3;
                }
            }
        } else if is_zone_id_char(ch) {
            out.push(char::from_u32(ch as u32).unwrap_or('\u{FFFD}'));
            index += 1;
        } else {
            let (code_point, length) = code_point_at(units, index, end);
            out.push_str(&escape_char(code_point));
            index += length;
        }
    }
    Ok(out)
}

fn normalize_reg_name(units: &[u16], start: usize, end: usize) -> Result<String, DartUriError> {
    let mut out = String::new();
    let mut index = start;
    while index < end {
        let ch = units[index];
        if ch == PERCENT {
            match normalize_escape(units, index, true) {
                Some(replacement) if replacement == "%" => {
                    out.push_str("%25");
                    index += 1;
                }
                Some(replacement) => {
                    out.push_str(&replacement);
                    index += 3;
                }
                None => {
                    out.push_str(&utf16_string(units, index, index + 3));
                    index += 3;
                }
            }
        } else if is_reg_name_char(ch) {
            if (UPPER_CASE_A..=UPPER_CASE_Z).contains(&ch) {
                out.push((ch as u8 as char).to_ascii_lowercase());
            } else {
                out.push(char::from_u32(ch as u32).unwrap_or('\u{FFFD}'));
            }
            index += 1;
        } else if is_general_delimiter(ch) {
            return Err(DartUriError::new("Invalid character"));
        } else {
            let (code_point, length) = code_point_at(units, index, end);
            out.push_str(&escape_char(code_point));
            index += length;
        }
    }
    Ok(out)
}

fn make_path(
    units: &[u16],
    start: usize,
    end: usize,
    scheme: &str,
    has_authority: bool,
) -> Result<String, DartUriError> {
    let is_file = scheme == "file";
    let ensure_leading_slash = is_file || has_authority;
    let mut result =
        normalize_or_substring(units, start, end, PATH_CHAR_OR_SLASH_MASK, true, true)?;
    if result.is_empty() {
        if is_file {
            result.push('/');
        }
    } else if ensure_leading_slash && !result.starts_with('/') {
        result.insert(0, '/');
    }
    Ok(normalize_path(result, scheme, has_authority))
}

fn make_query(units: &[u16], start: usize, end: usize) -> Result<String, DartUriError> {
    normalize_or_substring(units, start, end, QUERY_CHAR_MASK, true, false)
}

fn make_fragment(units: &[u16], start: usize, end: usize) -> Result<String, DartUriError> {
    normalize_or_substring(units, start, end, QUERY_CHAR_MASK, true, false)
}

fn normalize_path(path: String, scheme: &str, has_authority: bool) -> String {
    if scheme.is_empty() && !has_authority && !path.starts_with('/') && !path.starts_with('\\') {
        normalize_relative_path(&path, false)
    } else {
        remove_dot_segments(&path)
    }
}

fn may_contain_dot_segments(path: &str) -> bool {
    path.starts_with('.') || path.contains("/.")
}

fn remove_dot_segments(path: &str) -> String {
    if !may_contain_dot_segments(path) {
        return path.to_string();
    }
    let mut output: Vec<&str> = Vec::new();
    let mut append_slash = false;
    for segment in path.split('/') {
        append_slash = false;
        if segment == ".." {
            if !output.is_empty() {
                output.pop();
                if output.is_empty() {
                    output.push("");
                }
            }
            append_slash = true;
        } else if segment == "." {
            append_slash = true;
        } else {
            output.push(segment);
        }
    }
    if append_slash {
        output.push("");
    }
    output.join("/")
}

fn normalize_relative_path(path: &str, allow_scheme: bool) -> String {
    if !may_contain_dot_segments(path) {
        return if allow_scheme {
            path.to_string()
        } else {
            escape_scheme(path)
        };
    }
    let mut output: Vec<String> = Vec::new();
    let mut append_slash = false;
    for raw_segment in path.split('/') {
        let mut segment = raw_segment;
        if segment == ".." {
            append_slash = true;
            if !output.is_empty() && output.last().map(String::as_str) != Some("..") {
                output.pop();
            } else {
                output.push("..".to_string());
            }
        } else if segment == "." {
            append_slash = true;
        } else {
            append_slash = false;
            if segment.is_empty() && output.is_empty() {
                segment = "./";
            }
            output.push(segment.to_string());
        }
    }
    if output.is_empty() {
        return "./".to_string();
    }
    if append_slash {
        output.push(String::new());
    }
    if !allow_scheme {
        output[0] = escape_scheme(&output[0]);
    }
    output.join("/")
}

fn escape_scheme(path: &str) -> String {
    if path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() {
        for (i, byte) in path.bytes().enumerate().skip(1) {
            if byte == COLON as u8 {
                return format!("{}%3A{}", &path[..i], &path[i + 1..]);
            }
            if byte > 127 || (CHAR_TABLES[byte as usize] & SCHEME_MASK) == 0 {
                break;
            }
        }
    }
    path.to_string()
}

/// Dart's `_SimpleUri._simpleMerge` path merge for two relative references.
fn simple_merge_relative(base: &str, reference: &str) -> String {
    let mut base_start = 0;
    while base[base_start..].starts_with("../") {
        base_start += 3;
    }
    let mut ref_start = 0;
    let mut back_count = 0;
    while reference[ref_start..].starts_with("../") {
        ref_start += 3;
        back_count += 1;
    }
    let mut insert = "";
    let mut base_end = base.len();
    while base_end > base_start {
        base_end -= 1;
        if base.as_bytes()[base_end] == b'/' {
            insert = "/";
            if back_count == 0 {
                break;
            }
            back_count -= 1;
        }
    }
    if base_end == base_start {
        insert = "";
        ref_start -= back_count * 3;
    }
    format!("{}{}{}", &base[..base_end], insert, &reference[ref_start..])
}

fn merge_paths(base: &str, reference: &str) -> String {
    let mut back_count = 0;
    let mut ref_start = 0;
    while reference[ref_start..].starts_with("../") {
        ref_start += 3;
        back_count += 1;
    }
    let mut base_end = base.rfind('/').map(|i| i as isize).unwrap_or(-1);
    while base_end > 0 && back_count > 0 {
        let new_end = base[..base_end as usize]
            .rfind('/')
            .map(|i| i as isize)
            .unwrap_or(-1);
        if new_end < 0 {
            break;
        }
        let delta = base_end - new_end;
        let bytes = base.as_bytes();
        if (delta == 2 || delta == 3)
            && bytes[(new_end + 1) as usize] == DOT as u8
            && (delta == 2 || bytes[(new_end + 2) as usize] == DOT as u8)
        {
            break;
        }
        base_end = new_end;
        back_count -= 1;
    }
    let suffix_start = ref_start - 3 * back_count;
    format!(
        "{}{}",
        &base[..(base_end + 1) as usize],
        &reference[suffix_start..]
    )
}

fn parse_dart_int(text: &str) -> Option<i64> {
    let text = text.trim();
    let (negative, text) = match text.as_bytes().first() {
        Some(b'+') => (false, &text[1..]),
        Some(b'-') => (true, &text[1..]),
        _ => (false, text),
    };
    let (radix, digits) = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => (16u32, hex),
        None => (10u32, text),
    };
    if digits.is_empty() {
        return None;
    }
    let mut value: u64 = 0;
    for ch in digits.chars() {
        let digit = ch.to_digit(radix)?;
        value = value.checked_mul(radix as u64)?.checked_add(digit as u64)?;
    }
    if negative {
        if value > i64::MAX as u64 + 1 {
            return None;
        }
        Some(-(value as i128) as i64)
    } else {
        if value > i64::MAX as u64 {
            return None;
        }
        Some(value as i64)
    }
}

// ---------------------------------------------------------------------------
// IPv4/IPv6/IPvFuture validation (Dart `Uri.parseIPv6Address` et al.)
// ---------------------------------------------------------------------------

fn validate_ip_address(units: &[u16], start: usize, end: usize) -> Result<bool, DartUriError> {
    if start == end {
        return Err(DartUriError::new("Empty IP address"));
    }
    if units[start] == LOWER_CASE_V {
        validate_ipvfuture(units, start, end)?;
        return Ok(false);
    }
    parse_ipv6_address(&utf16_string(units, start, end), 0, end - start)?;
    Ok(true)
}

fn parse_ipv6_address(host: &str, start: usize, end: usize) -> Result<(), DartUriError> {
    let units: Vec<u16> = host.encode_utf16().collect();
    let error = |message: &str| {
        Err(DartUriError::new(format!(
            "Illegal IPv6 address, {message}"
        )))
    };
    if end - start < 2 {
        return error("address is too short");
    }
    let mut part_start = start;
    let mut wildcard_at: Option<usize> = None;
    let mut part_count = 0usize;
    let mut dec_value = true;
    let mut cursor = start;
    if units[cursor] == COLON {
        if units[cursor + 1] == COLON {
            wildcard_at = Some(0);
            part_count = 1;
            cursor += 2;
            part_start = cursor;
        } else {
            return error("invalid start colon");
        }
    }
    loop {
        let ch = if cursor >= end { 0 } else { units[cursor] };
        let digit = if (DIGIT_0..=DIGIT_0 + 9).contains(&ch) {
            Some((ch - DIGIT_0) as u32)
        } else {
            let lower = ch | 0x20;
            if (LOWER_CASE_A..=LOWER_CASE_F).contains(&lower) {
                dec_value = false;
                Some((lower - LOWER_CASE_A + 10) as u32)
            } else {
                None
            }
        };
        if digit.is_some() {
            if cursor < part_start + 4 {
                cursor += 1;
                continue;
            }
            return error("an IPv6 part can contain a maximum of 4 hex digits");
        }
        if cursor > part_start {
            if ch == DOT {
                if dec_value {
                    if part_count <= 6 {
                        parse_ipv4_address(host, part_start, end)?;
                        cursor = end;
                        part_count += 2;
                        break;
                    }
                    return error("an address must contain at most 8 parts");
                }
                break;
            }
            part_count += 1;
            if ch == COLON {
                if part_count < 8 {
                    dec_value = true;
                    cursor += 1;
                    part_start = cursor;
                    continue;
                }
                return error("an address must contain at most 8 parts");
            }
            break;
        }
        if ch == COLON {
            if wildcard_at.is_none() {
                wildcard_at = Some(part_count);
                part_count += 1;
                cursor += 1;
                part_start = cursor;
                continue;
            }
            return error("only one wildcard `::` is allowed");
        }
        if wildcard_at != part_count.checked_sub(1) {
            return error("missing part");
        }
        break;
    }
    if cursor < end {
        return error("invalid character");
    }
    if part_count < 8 && wildcard_at.is_none() {
        return error("an address without a wildcard must contain exactly 8 parts");
    }
    Ok(())
}

fn parse_ipv4_address(host: &str, start: usize, end: usize) -> Result<(), DartUriError> {
    let units: Vec<u16> = host.encode_utf16().collect();
    let error = |message: &str| {
        Err(DartUriError::new(format!(
            "Illegal IPv4 address, {message}"
        )))
    };
    let mut cursor = start;
    let mut octet_start = cursor;
    let mut octet_index = 0usize;
    let mut octet_value = 0u32;
    loop {
        let ch = if cursor >= end { 0 } else { units[cursor] };
        let digit = ch.wrapping_sub(DIGIT_0);
        if digit <= 9 {
            if octet_value != 0 || cursor == octet_start {
                octet_value = octet_value * 10 + digit as u32;
                if octet_value <= 255 {
                    cursor += 1;
                    continue;
                }
                return error("each part must be in the range 0..255");
            }
            return error("parts must not have leading zeros");
        }
        if cursor == octet_start {
            if cursor == end {
                break;
            }
            return error("invalid character");
        }
        octet_index += 1;
        if ch == DOT {
            if octet_index < 4 {
                octet_value = 0;
                cursor += 1;
                octet_start = cursor;
                continue;
            }
            break;
        }
        if cursor == end {
            if octet_index == 4 {
                return Ok(());
            }
            break;
        }
        return error("invalid character");
    }
    error("IPv4 address should contain exactly 4 parts")
}

fn validate_ipvfuture(units: &[u16], start: usize, end: usize) -> Result<(), DartUriError> {
    let hex_start = start + 1;
    let mut cursor = hex_start;
    loop {
        if cursor < end {
            let ch = units[cursor];
            cursor += 1;
            if hex_digit_value(ch) >= 0 {
                continue;
            }
            if ch == DOT {
                if cursor - 1 == hex_start {
                    return Err(DartUriError::new("Missing hex-digit in IPvFuture address"));
                }
                break;
            }
            return Err(DartUriError::new("Unexpected character"));
        }
        if cursor - 1 == hex_start {
            return Err(DartUriError::new("Missing hex-digit in IPvFuture address"));
        }
        return Err(DartUriError::new("Missing '.' in IPvFuture address"));
    }
    if cursor == end {
        // Dart's message includes these stray words (sic).
        return Err(DartUriError::new(
            "Missing address in IPvFuture address, host, cursor",
        ));
    }
    for &ch in &units[cursor..end] {
        if !is_ipvfuture_address_char(ch) {
            return Err(DartUriError::new("Invalid IPvFuture address character"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// data: URIs
// ---------------------------------------------------------------------------

fn strip_data_scheme(units: &[u16]) -> Option<usize> {
    if units.len() < 5 {
        return None;
    }
    let prefix = [0x64u16, 0x61, 0x74, 0x61, 0x3a];
    for (i, expected) in prefix.iter().enumerate() {
        if (units[i] | 0x20) != *expected {
            return None;
        }
    }
    Some(5)
}

fn parse_data_uri(units: &[u16], start: usize) -> Result<OwnedUrl, DartUriError> {
    // Validate the media type/parameters/base64 marker and normalize the data
    // part, mirroring `UriData._parse`.
    let comma = 0x2cu16;
    let slash = 0x2fu16;
    let semicolon = 0x3bu16;
    let equals = 0x3du16;
    let _ = (comma, slash, semicolon, equals);

    let text = utf16_string(units, start, units.len());
    let mut path = crate::url::data::parse_uri_data(&text)?;
    let mut query = None;
    if let Some(query_index) = path.find('?') {
        let q = normalize_or_substring(
            &path.encode_utf16().collect::<Vec<_>>(),
            query_index + 1,
            path.len(),
            QUERY_CHAR_MASK,
            false,
            false,
        )?;
        query = Some(q);
        path.truncate(query_index);
    }
    let units: Vec<u16> = path.encode_utf16().collect();
    let path = normalize_or_substring(
        &units,
        0,
        units.len(),
        PATH_CHAR_OR_SLASH_MASK,
        false,
        false,
    )?;
    Ok(OwnedUrl::make(
        "data".to_string(),
        String::new(),
        None,
        None,
        path,
        query,
        None,
    ))
}

// ---------------------------------------------------------------------------
// File paths (package:path's toUri/fromUri)
// ---------------------------------------------------------------------------

impl OwnedUrl {
    /// Converts an absolute filesystem path to a `file:` URL, matching
    /// `package:path`'s `toUri` for the current platform.
    #[allow(clippy::result_unit_err)]
    pub fn file_url_from_abs_path(path: &str) -> Result<Self, ()> {
        file_path::absolute_path_to_uri(path)
    }

    /// The filesystem path for a `file:` URL (Dart `Uri.toFilePath`).
    pub fn to_file_path_string(&self) -> Option<String> {
        if !self.is_file() {
            return None;
        }
        file_path::uri_to_path(&self.path, self.host.as_deref().filter(|h| !h.is_empty()))
    }

    /// The filesystem path for passing a `file:` URL to the [`Io`] layer.
    pub fn fs_path(&self) -> String {
        self.to_file_path_string()
            .unwrap_or_else(|| self.path.clone())
    }
}

pub(crate) mod data;
pub(crate) mod file_path;
mod tables;

/// Resolves a file-like URL against a `file:` base.
///
/// Relative URLs stay relative until [resolve](OwnedUrl::resolve); this helper
/// is the one place the importer layer combines them with a base URL.
pub fn resolve_file_path(url: &OwnedUrl, base: &OwnedUrl) -> Option<OwnedUrl> {
    if !url.is_file_like() || !base.is_file() {
        return None;
    }
    if url.is_relative() {
        Some(base.resolve_uri(url))
    } else {
        Some(url.clone())
    }
}

// ---------------------------------------------------------------------------
// data: URL encoding (Dart `Uri.dataFromString`)
// ---------------------------------------------------------------------------

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

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

/// Builds the `data:` URL Dart uses as an `ImporterResult.sourceMapUrl` when
/// none is supplied (`Uri.dataFromString(contents, encoding: utf8)`):
/// `data:;charset=utf-8,<percent-encoded>`.
pub(crate) fn data_url_from_string(contents: &str) -> String {
    let encoded = utf8_percent_encode(contents, DATA_SAFE);
    format!("data:;charset=utf-8,{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(path: &str) -> serde_json::Value {
        serde_json::from_str(path).expect("valid fixture json")
    }

    fn str_field<'a>(case: &'a serde_json::Value, name: &str) -> Option<&'a str> {
        case.get(name).and_then(|value| value.as_str())
    }

    fn opt_str_field<'a>(case: &'a serde_json::Value, name: &str) -> Option<Option<&'a str>> {
        match case.get(name)? {
            serde_json::Value::Null => Some(None),
            serde_json::Value::String(value) => Some(Some(value)),
            _ => panic!("fixture field {name} is not a string/null"),
        }
    }

    #[test]
    fn dart_uri_parse_fixtures() {
        let cases = json(include_str!("fixtures/parse.json"));
        let cases = cases.as_array().expect("parse fixture array");
        let mut checked = 0;
        let mut failures = Vec::new();
        for case in cases {
            let input = str_field(case, "input").expect("input");
            let expected_ok = case["ok"].as_bool().expect("ok");
            let result = OwnedUrl::parse(input);
            let mismatch = match (result, expected_ok) {
                (Err(error), false) => {
                    let expected = str_field(case, "error").unwrap_or("");
                    if error.message == expected {
                        None
                    } else {
                        Some(format!(
                            "{input:?}: error {:?}, expected {:?}",
                            error.message, expected
                        ))
                    }
                }
                (Ok(url), true) => {
                    let expected_scheme = str_field(case, "scheme").unwrap_or("");
                    let expected_user_info = str_field(case, "userInfo").unwrap_or("");
                    let expected_host = str_field(case, "host").unwrap_or("");
                    let expected_path = str_field(case, "path").unwrap_or("");
                    let expected_text = str_field(case, "toString").unwrap_or("");
                    let expected_query = opt_str_field(case, "query").unwrap();
                    let expected_fragment = opt_str_field(case, "fragment").unwrap();
                    let expected_port = case["port"].as_i64();
                    let expected_authority = case["hasAuthority"].as_bool().unwrap();
                    let expected_absolute_path = case["hasAbsolutePath"].as_bool().unwrap();
                    let expected_absolute = case["isAbsolute"].as_bool().unwrap();
                    let actual_port = if url.has_port() {
                        Some(url.port())
                    } else {
                        None
                    };
                    let mut problems = Vec::new();
                    if url.scheme() != expected_scheme {
                        problems.push(format!("scheme {:?} != {expected_scheme:?}", url.scheme()));
                    }
                    if url.user_info() != expected_user_info {
                        problems.push(format!(
                            "userInfo {:?} != {expected_user_info:?}",
                            url.user_info()
                        ));
                    }
                    if url.host() != expected_host {
                        problems.push(format!("host {:?} != {expected_host:?}", url.host()));
                    }
                    if url.has_authority() != expected_authority {
                        problems.push(format!(
                            "hasAuthority {} != {expected_authority}",
                            url.has_authority()
                        ));
                    }
                    if actual_port != expected_port {
                        problems.push(format!("port {actual_port:?} != {expected_port:?}"));
                    }
                    if url.path() != expected_path {
                        problems.push(format!("path {:?} != {expected_path:?}", url.path()));
                    }
                    if url.has_absolute_path() != expected_absolute_path {
                        problems.push(format!(
                            "hasAbsolutePath {} != {expected_absolute_path}",
                            url.has_absolute_path()
                        ));
                    }
                    if url.query() != expected_query.unwrap_or("") {
                        problems.push(format!(
                            "query {:?} != {:?}",
                            url.query(),
                            expected_query.unwrap_or("")
                        ));
                    }
                    if url.fragment() != expected_fragment.unwrap_or("") {
                        problems.push(format!(
                            "fragment {:?} != {:?}",
                            url.fragment(),
                            expected_fragment.unwrap_or("")
                        ));
                    }
                    if url.is_absolute() != expected_absolute {
                        problems.push(format!(
                            "isAbsolute {} != {expected_absolute}",
                            url.is_absolute()
                        ));
                    }
                    if url.as_str() != expected_text {
                        problems.push(format!("toString {:?} != {expected_text:?}", url.as_str()));
                    }
                    if problems.is_empty() {
                        None
                    } else {
                        Some(format!("{input:?}: {}", problems.join("; ")))
                    }
                }
                (Err(error), true) => Some(format!("{input:?}: unexpected error {error:?}")),
                (Ok(_), false) => Some(format!(
                    "{input:?}: parsed, expected {:?}",
                    str_field(case, "error").unwrap_or("")
                )),
            };
            checked += 1;
            if let Some(mismatch) = mismatch {
                failures.push(mismatch);
                if failures.len() >= 20 {
                    break;
                }
            }
        }
        assert!(
            failures.is_empty(),
            "checked {checked}/{} cases; first failures:\n{}",
            cases.len(),
            failures.join("\n")
        );
    }

    fn assert_same_url(input: &str, url: &OwnedUrl, case: &serde_json::Value) -> Option<String> {
        let expected_scheme = str_field(case, "scheme").unwrap_or("");
        let expected_user_info = str_field(case, "userInfo").unwrap_or("");
        let expected_host = str_field(case, "host").unwrap_or("");
        let expected_path = str_field(case, "path").unwrap_or("");
        let expected_text = str_field(case, "toString").unwrap_or("");
        let expected_query = opt_str_field(case, "query").unwrap();
        let expected_fragment = opt_str_field(case, "fragment").unwrap();
        let expected_port = case["port"].as_i64();
        let expected_authority = case["hasAuthority"].as_bool().unwrap();
        let expected_absolute_path = case["hasAbsolutePath"].as_bool().unwrap();
        let expected_absolute = case["isAbsolute"].as_bool().unwrap();
        let actual_port = if url.has_port() {
            Some(url.port())
        } else {
            None
        };
        let mut problems = Vec::new();
        if url.scheme() != expected_scheme {
            problems.push(format!("scheme {:?} != {expected_scheme:?}", url.scheme()));
        }
        if url.user_info() != expected_user_info {
            problems.push(format!(
                "userInfo {:?} != {expected_user_info:?}",
                url.user_info()
            ));
        }
        if url.host() != expected_host {
            problems.push(format!("host {:?} != {expected_host:?}", url.host()));
        }
        if url.has_authority() != expected_authority {
            problems.push(format!(
                "hasAuthority {} != {expected_authority}",
                url.has_authority()
            ));
        }
        if actual_port != expected_port {
            problems.push(format!("port {actual_port:?} != {expected_port:?}"));
        }
        if url.path() != expected_path {
            problems.push(format!("path {:?} != {expected_path:?}", url.path()));
        }
        if url.has_absolute_path() != expected_absolute_path {
            problems.push(format!(
                "hasAbsolutePath {} != {expected_absolute_path}",
                url.has_absolute_path()
            ));
        }
        if url.query() != expected_query.unwrap_or("") {
            problems.push(format!(
                "query {:?} != {:?}",
                url.query(),
                expected_query.unwrap_or("")
            ));
        }
        if url.fragment() != expected_fragment.unwrap_or("") {
            problems.push(format!(
                "fragment {:?} != {:?}",
                url.fragment(),
                expected_fragment.unwrap_or("")
            ));
        }
        if url.is_absolute() != expected_absolute {
            problems.push(format!(
                "isAbsolute {} != {expected_absolute}",
                url.is_absolute()
            ));
        }
        if url.as_str() != expected_text {
            problems.push(format!("toString {:?} != {expected_text:?}", url.as_str()));
        }
        if problems.is_empty() {
            None
        } else {
            Some(format!("{input:?}: {}", problems.join("; ")))
        }
    }

    #[test]
    fn dart_uri_resolve_fixtures() {
        let cases = json(include_str!("fixtures/resolve.json"));
        let cases = cases.as_array().expect("resolve fixture array");
        let mut failures = Vec::new();
        for case in cases {
            let base_input = str_field(case, "base").expect("base");
            let ref_input = str_field(case, "ref").expect("ref");
            let expected_ok = case["ok"].as_bool().expect("ok");
            let base = match OwnedUrl::parse(base_input) {
                Ok(base) => base,
                Err(error) => {
                    if expected_ok {
                        failures.push(format!(
                            "{base_input:?} + {ref_input:?}: base parse error {error:?}"
                        ));
                    }
                    continue;
                }
            };
            match OwnedUrl::parse(ref_input) {
                Err(error) => {
                    if expected_ok {
                        failures.push(format!(
                            "{base_input:?} + {ref_input:?}: ref parse error {error:?}"
                        ));
                    } else if error.message != str_field(case, "error").unwrap_or("") {
                        failures.push(format!(
                            "{base_input:?} + {ref_input:?}: error {:?} != {:?}",
                            error.message,
                            str_field(case, "error").unwrap_or("")
                        ));
                    }
                }
                Ok(reference) => {
                    if !expected_ok {
                        failures.push(format!(
                            "{base_input:?} + {ref_input:?}: parsed, expected {:?}",
                            str_field(case, "error").unwrap_or("")
                        ));
                        continue;
                    }
                    let resolved = base.resolve_uri(&reference);
                    if let Some(problem) =
                        assert_same_url(&format!("{base_input} + {ref_input}"), &resolved, case)
                    {
                        failures.push(problem);
                    }
                }
            }
            if failures.len() >= 20 {
                break;
            }
        }
        assert!(
            failures.is_empty(),
            "{} resolve cases; first failures:\n{}",
            cases.len(),
            failures.join("\n")
        );
    }
    #[test]
    fn resolve_distinguishes_simple_and_full_references() {
        // Dart parses ".." as a full `_Uri` and "../" as a `_SimpleUri`.
        // They compare equal but resolve differently; the kind preserves it.
        let base = OwnedUrl::parse("foo/bar").unwrap();
        let full = OwnedUrl::parse("..").unwrap();
        let simple = OwnedUrl::parse("../").unwrap();
        assert_eq!(full, simple);
        assert_eq!(full.kind, UriKind::Full);
        assert_eq!(simple.kind, UriKind::Simple);
        assert_eq!(base.resolve_uri(&full).as_str(), "./");
        assert_eq!(base.resolve_uri(&simple).as_str(), "");
        assert_eq!(base.resolve("..").unwrap().as_str(), "./");
        assert_eq!(base.resolve("../").unwrap().as_str(), "");
    }

    #[test]
    fn dart_uri_path_fixtures() {
        let cases = json(include_str!("fixtures/path.json"));
        let cases = cases.as_array().expect("path fixture array");
        let mut failures = Vec::new();
        for case in cases {
            let windows = case["windows"].as_bool().expect("windows");
            let path = str_field(case, "path").expect("path");
            let expected_to_uri = opt_str_field(case, "toUri").unwrap();
            let expected_to_uri_error = opt_str_field(case, "toUriError").unwrap();
            let expected_file_uri = opt_str_field(case, "fileUri").unwrap();
            let expected_file_uri_error = opt_str_field(case, "fileUriError").unwrap();
            let to_uri = file_path::path_to_uri(path, windows);
            match (&to_uri, expected_to_uri) {
                (Ok(url), Some(expected)) => {
                    if url.as_str() != expected {
                        failures.push(format!(
                            "toUri({path:?}, windows={windows}): {:?} != {expected:?}",
                            url.as_str()
                        ));
                    }
                }
                (Err(_), None) if expected_to_uri_error.is_none() => {
                    failures.push(format!("toUri({path:?}, windows={windows}) errored"))
                }
                _ => {}
            }
            if let Ok(url) = &to_uri {
                let host = if url.has_authority() {
                    Some(url.host())
                } else {
                    None
                };
                if let Some(decoded) = file_path::uri_to_path_with(url.path(), host, windows) {
                    if let Some(expected) = opt_str_field(case, "fromUri").unwrap() {
                        if decoded != expected {
                            failures.push(format!(
                                "fromUri({:?}, windows={windows}): {decoded:?} != {expected:?}",
                                url.as_str()
                            ));
                        }
                    }
                }
            }

            match (file_path::uri_file(path, windows), expected_file_uri) {
                (Ok(url), Some(expected)) => {
                    if url.as_str() != expected {
                        failures.push(format!(
                            "Uri.file({path:?}, windows={windows}): {:?} != {expected:?}",
                            url.as_str()
                        ));
                    }
                    let host = if url.has_authority() {
                        Some(url.host())
                    } else {
                        None
                    };
                    if let Some(decoded) = file_path::uri_to_path_with(url.path(), host, windows) {
                        if let Some(expected) = opt_str_field(case, "filePath").unwrap() {
                            if decoded != expected {
                                failures.push(format!(
                                    "toFilePath({:?}, windows={windows}): {decoded:?} != {expected:?}",
                                    url.as_str()
                                ));
                            }
                        }
                    }
                }
                (Err(_), None) if expected_file_uri_error.is_none() => {
                    failures.push(format!("Uri.file({path:?}, windows={windows}) errored"))
                }
                _ => {}
            }
            if failures.len() >= 20 {
                break;
            }
        }
        assert!(
            failures.is_empty(),
            "{} path cases; first failures:\n{}",
            cases.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn dart_uri_data_fixtures() {
        let cases = json(include_str!("fixtures/data.json"));
        let cases = cases.as_array().expect("data fixture array");
        let mut failures = Vec::new();
        for case in cases {
            let contents = str_field(case, "contents").expect("contents");
            let expected = str_field(case, "data").expect("data");
            let actual = data_url_from_string(contents);
            if actual != expected {
                failures.push(format!("{contents:?}: {actual:?} != {expected:?}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    // --- ArenaUrl / Url ---

    use bumpalo::Bump;

    const URL_CORPUS: &[&str] = &[
        "file:///a/b.scss",
        "file:///a%20b/c.scss",
        "file:///C:/x/y.scss",
        "file:///a/../b/./c.scss",
        "file:///",
        "file://localhost/a.scss",
        "data:text/plain,hello%20world",
        "sass:math",
        "https://user:pass@example.com:8080/p?q=1#f",
        "https://example.com",
        "http://[::1]:80/x",
        "mailto:user@example.com",
        "a/b/c.scss",
        "../up/./x.scss",
        "..",
        "../",
        "#frag",
        "?q",
        "",
    ];

    fn hash_of<T: Hash>(value: &T) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    fn assert_url_accessors_match(a: &dyn Url, b: &dyn Url, raw: &str) {
        assert_eq!(a.as_str(), b.as_str(), "{raw}: as_str");
        assert_eq!(a.scheme(), b.scheme(), "{raw}: scheme");
        assert_eq!(a.has_scheme(), b.has_scheme(), "{raw}: has_scheme");
        assert_eq!(a.user_info(), b.user_info(), "{raw}: user_info");
        assert_eq!(a.host(), b.host(), "{raw}: host");
        assert_eq!(a.has_authority(), b.has_authority(), "{raw}: has_authority");
        assert_eq!(a.port(), b.port(), "{raw}: port");
        assert_eq!(a.has_port(), b.has_port(), "{raw}: has_port");
        assert_eq!(a.path(), b.path(), "{raw}: path");
        assert_eq!(
            a.has_empty_path(),
            b.has_empty_path(),
            "{raw}: has_empty_path"
        );
        assert_eq!(
            a.has_absolute_path(),
            b.has_absolute_path(),
            "{raw}: has_absolute_path"
        );
        assert_eq!(a.query(), b.query(), "{raw}: query");
        assert_eq!(a.has_query(), b.has_query(), "{raw}: has_query");
        assert_eq!(a.fragment(), b.fragment(), "{raw}: fragment");
        assert_eq!(a.has_fragment(), b.has_fragment(), "{raw}: has_fragment");
        assert_eq!(a.is_absolute(), b.is_absolute(), "{raw}: is_absolute");
        assert_eq!(a.is_relative(), b.is_relative(), "{raw}: is_relative");
        assert_eq!(a.is_file(), b.is_file(), "{raw}: is_file");
        assert_eq!(a.is_file_like(), b.is_file_like(), "{raw}: is_file_like");
        assert_eq!(
            a.cannot_be_a_base(),
            b.cannot_be_a_base(),
            "{raw}: cannot_be_a_base"
        );
        assert_eq!(
            a.to_file_path_string(),
            b.to_file_path_string(),
            "{raw}: to_file_path_string"
        );
        assert_eq!(a.to_string(), b.to_string(), "{raw}: display");
    }

    #[test]
    fn arena_url_round_trips_owned() {
        let arena = Bump::new();
        for raw in URL_CORPUS {
            let owned = OwnedUrl::parse(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            let arena_url = ArenaUrl::parse(&arena, raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert_eq!(arena_url.to_owned_url(), owned, "{raw}: owned round-trip");
            assert_eq!(hash_of(&arena_url), hash_of(&owned), "{raw}: hash");
            assert_url_accessors_match(&owned, &arena_url, raw);
        }
    }

    #[test]
    fn arena_url_from_owned_matches_parse() {
        let arena = Bump::new();
        for raw in URL_CORPUS {
            let owned = OwnedUrl::parse(raw).unwrap();
            let from_owned = ArenaUrl::from_owned(&arena, &owned);
            let parsed = ArenaUrl::parse(&arena, raw).unwrap();
            assert_eq!(from_owned, parsed, "{raw}: from_owned vs parse");
            assert_eq!(from_owned.as_str(), owned.as_str(), "{raw}: as_str");
        }
    }
}
