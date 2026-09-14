// Copyright (c) 2014, the Dart project authors.  Please see the AUTHORS file
// for details. All rights reserved. Use of this source code is governed by a
// BSD-style license that can be found in the LICENSE file.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: (external) package:source_span/lib/src/file.dart + lib/src/util/span.dart
// go-source: go/sasscommon/source_span_file.go + go/sasscommon/source_span_span.go + go/sasscommon/util_span.go

use crate::url::SassUrl;
use std::rc::Rc;

use std::fmt;

use crate::common::exception::{SassError, SassResult};
use crate::common::source_span_file_source::FileSource;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::common::span::Span;
use crate::common::span_error::SpanError;

/// A single location within a source file: the 0-based byte `offset` plus its
/// 0-based `line` and `column`.
///
/// Mirrors `source_span`'s `SourceLocation` (offsets count characters in Dart,
/// bytes here — see `SpanScanner`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceLocation {
    pub offset: usize,
    pub line: usize,
    pub column: usize,
}

/// A span within a source file.
///
/// Mirrors `source_span`'s `FileSpan`: unlike the base `SourceSpan`, line and
/// column values derive from `file`'s contents, and [`FileSpan::expand`]
/// covers both spans even when disjoint. A 24-byte `Copy` value holding
/// optional arena-borrowed [`FileSource`]; core accessors are infallible
/// (a missing file yields empty/zero values) while the span-arithmetic
/// utilities return `Result<_, SpanError>`. Equality is structural on
/// offsets plus URL (matching Dart's value-only `_FileSpan ==`); use
/// `FileSource::identical` for provenance checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileSpan<'parse> {
    file: Option<&'parse FileSource<'parse>>,
    start: usize,
    end: usize,
}

/// A span that points nowhere.
///
/// Mirrors `util/span.dart`'s `bogusSpan`: used for fake AST nodes that are
/// never presented to the user, as well as for embedded compilation failures
/// with no associated spans. Reports `file() == None`, `text() == ""`, and
/// `len() == 0`.
pub const BOGUS_SPAN: FileSpan<'static> = FileSpan {
    file: None,
    start: 0,
    end: 0,
};

impl<'parse> FileSpan<'parse> {
    /// Returns a span from `start` to `end` (exclusive) in `file`.
    ///
    /// Mirrors `SourceFile.span`.
    pub fn new(file: Option<&'parse FileSource<'parse>>, start: usize, end: usize) -> Self {
        FileSpan { file, start, end }
    }

    /// The source text covered by this span.
    ///
    /// Mirrors `SourceSpan.text`. Empty when there is no file or the span is
    /// empty.
    pub fn text(&self) -> &'parse str {
        match self.file {
            Some(f) if self.start < self.end => f.get_text(self.start, self.end),
            _ => "",
        }
    }

    /// The start location of this span.
    ///
    /// Mirrors `SourceSpan.start`. Zero-valued when there is no file.
    pub fn start_location(&self) -> SourceLocation {
        match self.file {
            Some(f) => f.location(self.start),
            None => SourceLocation {
                offset: 0,
                line: 0,
                column: 0,
            },
        }
    }

    /// The end location of this span, exclusive.
    ///
    /// Mirrors `SourceSpan.end`. Zero-valued when there is no file.
    pub fn end_location(&self) -> SourceLocation {
        match self.file {
            Some(f) => f.location(self.end),
            None => SourceLocation {
                offset: 0,
                line: 0,
                column: 0,
            },
        }
    }

    /// The URL of the source (typically a file) of this span.
    ///
    /// Mirrors `SourceSpan.sourceUrl`: may be `None` when the source URL is
    /// unknown or unavailable.
    pub fn source_url(&self) -> Option<&SassUrl> {
        self.file.and_then(|f| f.url())
    }

    /// The URL formatted once per source, shared via `Rc<str>`.
    pub fn url_str_cached(&self) -> Option<Rc<str>> {
        self.file.and_then(|f| f.url_str_cached())
    }

    pub fn file(&self) -> Option<&'parse FileSource<'parse>> {
        self.file
    }

    /// The length of this span, in characters.
    ///
    /// Mirrors `SourceSpan.length`. Zero when there is no file.
    pub fn len(&self) -> usize {
        if self.file.is_none() {
            return 0;
        }
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The surrounding source lines for error rendering.
    ///
    /// Mirrors `SourceSpanWithContext.context` / `_FileSpan.context`: covers
    /// the full starting line through the full ending line, with the
    /// end-at-line-start and trailing-line edge cases handled as in Dart.
    pub fn context(&self) -> String {
        let file = match self.file {
            Some(f) => f,
            None => return String::new(),
        };

        let end_line = file.get_line(self.end);
        let end_column = file.get_column(self.end);

        let end_offset = if end_column == 0 && end_line != 0 {
            if self.end == self.start {
                if end_line == file.lines() - 1 {
                    return String::new();
                }
                return file
                    .get_text(file.get_offset(end_line), file.get_offset(end_line + 1))
                    .to_string();
            }
            self.end
        } else if end_line == file.lines() - 1 {
            file.len()
        } else {
            file.get_offset(end_line + 1)
        };

        file.get_text(file.get_offset(file.get_line(self.start)), end_offset)
            .to_string()
    }

    /// Returns a new span covering both `self` and `other`.
    ///
    /// Mirrors `_FileSpan.expand`: unlike `union`, `other` may be disjoint —
    /// the text between the two is covered as well. The URLs must match.
    pub fn expand(&self, other: &Span<'parse>) -> Result<FileSpan<'parse>, SpanError> {
        let other_url = other.source_url()?;
        let s_url = self.source_url();
        if !same_url(s_url, other_url.as_ref()) {
            return Err(SpanError::Argument(format!(
                "Source URLs {:?} and {:?} don't match.",
                s_url.map(|u| u.as_str()),
                other_url.as_ref().map(|u| u.as_str())
            )));
        }
        let self_file = self.file;
        let mut start = self.start;
        let mut end = self.end;
        let other_start = other.start_location()?;
        if other_start.offset < start {
            start = other_start.offset;
        }
        let other_end = other.end_location()?;
        if other_end.offset > end {
            end = other_end.offset;
        }
        Ok(FileSpan {
            file: self_file,
            start,
            end,
        })
    }

    /// Returns a subspan `start..end` relative to the beginning of this span.
    ///
    /// Mirrors `SourceSpanExtension.subspan` / `FileSpanExtension.subspan`: a
    /// full-range subspan returns the span unchanged.
    pub fn subspan(&self, start: usize, end: usize) -> Result<FileSpan<'parse>, SpanError> {
        let length = self.len();
        if start > end || end > length {
            return Err(SpanError::Range(format!(
                "Invalid subspan range: [{start}, {end}) in span of length {length}"
            )));
        }
        if start == 0 && end == length {
            return Ok(*self);
        }
        Ok(FileSpan {
            file: self.file,
            start: self.start + start,
            end: self.start + end,
        })
    }

    /// Returns this span with all trailing whitespace trimmed.
    ///
    /// Mirrors `SpanExtensions.trimRight`.
    pub fn trim_right(&self) -> Result<FileSpan<'parse>, SpanError> {
        let text = self.text();
        let mut end = text.len();
        while end > 0 && is_whitespace_byte(text.as_bytes()[end - 1]) {
            end -= 1;
        }
        if end == text.len() {
            return Ok(*self);
        }
        Ok(FileSpan {
            file: self.file,
            start: self.start,
            end: self.start + end,
        })
    }

    /// Returns this span with all leading whitespace trimmed.
    ///
    /// Mirrors `SpanExtensions.trimLeft`.
    pub fn trim_left(&self) -> Result<FileSpan<'parse>, SpanError> {
        trim_left_fs(self)
    }

    /// Returns this span with all whitespace trimmed from both sides.
    ///
    /// Mirrors `SpanExtensions.trim`.
    pub fn trim(&self) -> Result<FileSpan<'parse>, SpanError> {
        let trimmed = self.trim_left()?;
        trimmed.trim_right()
    }

    /// Returns a span covering the text from the beginning of this span to
    /// the beginning of `inner`.
    ///
    /// Mirrors `SpanExtensions.before`: throws (here: returns `Argument`)
    /// when `inner` isn't fully within this span or lives in a different
    /// file.
    pub fn before(&self, inner: &Span<'parse>) -> Result<FileSpan<'parse>, SpanError> {
        let inner_url = inner.source_url()?;
        let s_url = self.source_url();
        if !same_url(s_url, inner_url.as_ref()) {
            return Err(SpanError::Argument(format!(
                "{} and {inner} are in different files.",
                self,
            )));
        }
        let inner_start = inner.start_location()?;
        let inner_end = inner.end_location()?;
        if inner_start.offset < self.start || inner_end.offset > self.end {
            return Err(SpanError::Argument(format!(
                "{inner} isn't inside {}.",
                self
            )));
        }
        Ok(FileSpan {
            file: self.file,
            start: self.start,
            end: inner_start.offset,
        })
    }

    /// Returns a span covering the text from the end of `inner` to the end
    /// of this span.
    ///
    /// Mirrors `SpanExtensions.after`: throws (here: returns `Argument`)
    /// when `inner` isn't fully within this span or lives in a different
    /// file.
    pub fn after(&self, inner: &Span<'parse>) -> Result<FileSpan<'parse>, SpanError> {
        let inner_url = inner.source_url()?;
        let s_url = self.source_url();
        if !same_url(s_url, inner_url.as_ref()) {
            return Err(SpanError::Argument(format!(
                "{} and {inner} are in different files.",
                self,
            )));
        }
        let inner_start = inner.start_location()?;
        let inner_end = inner.end_location()?;
        if inner_start.offset < self.start || inner_end.offset > self.end {
            return Err(SpanError::Argument(format!(
                "{inner} isn't inside {}.",
                self
            )));
        }
        Ok(FileSpan {
            file: self.file,
            start: inner_end.offset,
            end: self.end,
        })
    }

    /// Returns a span covering the text after this span and before `other`.
    ///
    /// Mirrors `SpanExtensions.between`: throws (here: returns `Argument`)
    /// when `other` starts before this span ends or lives in a different
    /// file.
    pub fn between(&self, other: &Span<'parse>) -> Result<FileSpan<'parse>, SpanError> {
        let other_url = other.source_url()?;
        let s_url = self.source_url();
        if !same_url(s_url, other_url.as_ref()) {
            return Err(SpanError::Argument(format!(
                "{} and {other} are in different files.",
                self,
            )));
        }
        let other_start = other.start_location()?;
        if self.end > other_start.offset {
            return Err(SpanError::Argument(format!(
                "{} isn't before {other}.",
                self,
            )));
        }
        Ok(FileSpan {
            file: self.file,
            start: self.end,
            end: other_start.offset,
        })
    }

    /// Whether this span contains `target` within its inclusive
    /// `[start, end]` range.
    ///
    /// Mirrors `SpanExtensions.contains`: spans must be in the same file
    /// (compared by URL, as in Dart).
    pub fn contains(&self, target: &Span<'parse>) -> Result<bool, SpanError> {
        let target_url = target.source_url()?;
        let source_url = self.source_url();
        if !same_url(source_url, target_url.as_ref()) {
            return Ok(false);
        }
        let target_start = target.start_location()?;
        if self.start > target_start.offset {
            return Ok(false);
        }
        let target_end = target.end_location()?;
        Ok(self.end >= target_end.offset)
    }

    /// Returns a subspan excluding an initial at-rule and any whitespace
    /// after it.
    ///
    /// Mirrors `SpanExtensions.withoutInitialAtRule`: fails with a format
    /// error unless the span starts with `@`.
    pub fn without_initial_at_rule(&self) -> Result<FileSpan<'parse>, SpanError> {
        let text = self.text();
        if text.is_empty() || text.as_bytes()[0] != b'@' {
            return Err(SpanError::Sass(Box::new(SassError::Format {
                message: "Expected @.".into(),
                span: SourceSpanWithContext::from_file_span(self)?,
                original_source: None,
                cause: None,
                loaded_urls: vec![],
            })));
        }
        let pos = scan_ident(text, 1, self)?;
        if pos >= text.len() {
            return Ok(*self);
        }
        let result = self.subspan(pos, text.len())?;
        trim_left_fs(&result)
    }

    /// Returns the span of the quoted text at the start of this span.
    ///
    /// The span must start with `"` or `'`. Mirrors
    /// `SpanExtensions.initialQuoted`.
    pub fn initial_quoted(&self) -> Result<FileSpan<'parse>, SpanError> {
        let text = self.text();
        if text.is_empty() {
            return Err(SpanError::Sass(Box::new(SassError::Format {
                message: "Expected quote.".into(),
                span: SourceSpanWithContext::from_file_span(self)?,
                original_source: None,
                cause: None,
                loaded_urls: vec![],
            })));
        }
        let first = text.as_bytes()[0];
        if first != b'"' && first != b'\'' {
            return Err(SpanError::Sass(Box::new(SassError::Format {
                message: "Expected quote.".into(),
                span: SourceSpanWithContext::from_file_span(self)?,
                original_source: None,
                cause: None,
                loaded_urls: vec![],
            })));
        }
        let mut end = 1;
        while end < text.len() {
            if text.as_bytes()[end] == first {
                end += 1;
                return self.subspan(0, end);
            }
            if text.as_bytes()[end] == b'\\' {
                end += 1;
                if end < text.len() {
                    end += 1;
                }
                continue;
            }
            end += 1;
        }
        Err(SpanError::Sass(Box::new(SassError::Format {
            message: format!("Expected {}.", first as char),
            span: SourceSpanWithContext::from_file_span(self)?,
            original_source: None,
            cause: None,
            loaded_urls: vec![],
        })))
    }

    /// Returns the span of the identifier at the start of this span.
    ///
    /// When `include_leading` is greater than 0, that many additional
    /// characters are included before looking for the identifier. Mirrors
    /// `SpanExtensions.initialIdentifier`.
    pub fn initial_identifier(
        &self,
        include_leading: usize,
    ) -> Result<FileSpan<'parse>, SpanError> {
        let text = self.text();
        let mut pos = 0;
        for _ in 0..include_leading {
            if pos >= text.len() {
                return self.subspan(0, pos);
            }
            pos += 1;
        }
        pos = scan_ident(text, pos, self)?;
        self.subspan(0, pos)
    }

    /// Returns a subspan excluding the identifier at the start of this span.
    ///
    /// Mirrors `SpanExtensions.withoutInitialIdentifier`.
    pub fn without_initial_identifier(&self) -> Result<FileSpan<'parse>, SpanError> {
        let text = self.text();
        let pos = scan_ident(text, 0, self)?;
        self.subspan(pos, text.len())
    }

    /// Returns a subspan excluding a namespace and `.` at the start of this
    /// span.
    ///
    /// Mirrors `SpanExtensions.withoutNamespace` (note: unlike Dart, no error
    /// is raised when there is no `.` — the identifier-stripped span is
    /// returned as-is).
    pub fn without_namespace(&self) -> Result<FileSpan<'parse>, SpanError> {
        let after_ident = self.without_initial_identifier()?;
        let text = after_ident.text();
        if text.is_empty() || text.as_bytes()[0] != b'.' {
            return Ok(after_ident);
        }
        after_ident.subspan(1, text.len())
    }
}

impl<'parse> FileSpan<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        Ok(self.text().to_string())
    }
}

impl<'parse> fmt::Display for FileSpan<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text())
    }
}

pub(crate) fn same_url(a: Option<&SassUrl>, b: Option<&SassUrl>) -> bool {
    match (a, b) {
        (None, None) => true,
        (None, Some(_)) | (Some(_), None) => false,
        (Some(a), Some(b)) => a.as_str() == b.as_str(),
    }
}

pub(crate) fn scan_ident(
    text: &str,
    mut pos: usize,
    error_span: &FileSpan<'_>,
) -> Result<usize, SpanError> {
    while pos < text.len() {
        match text.as_bytes()[pos] {
            b'\\' => {
                pos = consume_escaped(text, pos, error_span)?;
            }
            b if is_name(b) => {
                pos += 1;
            }
            _ => {
                return Ok(pos);
            }
        }
    }
    Ok(pos)
}

pub(crate) fn consume_escaped(
    text: &str,
    pos: usize,
    error_span: &FileSpan<'_>,
) -> Result<usize, SpanError> {
    if pos >= text.len() || text.as_bytes()[pos] != b'\\' {
        return Ok(pos);
    }
    let mut pos = pos + 1;
    if pos >= text.len() {
        return Ok(pos);
    }
    let ch = text.as_bytes()[pos];
    match ch {
        b'\n' | b'\r' | b'\x0C' => {
            return Err(SpanError::Sass(Box::new(SassError::Format {
                message: "Expected escape sequence.".into(),
                span: SourceSpanWithContext::from_file_span(error_span)?,
                original_source: None,
                cause: None,
                loaded_urls: vec![],
            })));
        }
        _ if is_hex(ch) => {
            for _ in 0..6 {
                if pos < text.len() && is_hex(text.as_bytes()[pos]) {
                    pos += 1;
                } else {
                    break;
                }
            }
            if pos < text.len() && is_whitespace_byte(text.as_bytes()[pos]) {
                pos += 1;
            }
        }
        _ => {
            pos += 1;
        }
    }
    Ok(pos)
}

pub(crate) fn is_name(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric() || b == b'-' || b >= 0x80
}

pub(crate) fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

pub(crate) fn is_whitespace_byte(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' || b == b'\x0C'
}

pub(crate) fn column_at(text: &str, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    let slice = &text.as_bytes()[..offset];
    match slice.iter().rposition(|&b| b == b'\n') {
        Some(last_nl) => offset - last_nl - 1,
        None => offset,
    }
}

pub(crate) fn trim_left_fs<'parse>(span: &FileSpan<'parse>) -> Result<FileSpan<'parse>, SpanError> {
    let text = span.text();
    let bytes = text.as_bytes();
    let mut start = 0;
    while start < bytes.len() && is_whitespace_byte(bytes[start]) {
        start += 1;
    }
    if start == 0 {
        return Ok(*span);
    }
    let length = span.len();
    span.subspan(start, length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::span::Span;
    use bumpalo::Bump;

    fn test_source<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
    ) -> &'parse FileSource<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        FileSource::new_in(arena, text, None)
    }

    fn test_url(url_str: &str) -> SassUrl {
        SassUrl::parse(url_str).unwrap()
    }

    #[test]
    fn test_filespan_accessors() {
        let arena = Bump::new();
        let fs = test_source(&arena, "hello");
        let s = FileSpan::new(Some(fs), 0, 5);
        assert_eq!(s.file(), Some(fs));
        assert_eq!(
            s.start_location(),
            SourceLocation {
                offset: 0,
                line: 0,
                column: 0
            }
        );
        assert_eq!(
            s.end_location(),
            SourceLocation {
                offset: 5,
                line: 0,
                column: 5
            }
        );
        assert_eq!(s.text(), "hello");
        assert_eq!(s.len(), 5);
        assert_eq!(s.context(), "hello");
    }

    #[test]
    fn test_filespan_len() {
        let arena = Bump::new();
        let s = FileSpan::new(Some(test_source(&arena, "hello")), 5, 10);
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn test_new_filespan_in_file() {
        let arena = Bump::new();
        let s = FileSpan::new(Some(test_source(&arena, "..........")), 10, 20);
        assert_eq!(s.start_location().offset, 10);
        assert_eq!(s.end_location().offset, 20);
    }

    #[test]
    fn test_file_text() {
        let arena = Bump::new();
        let fs = test_source(&arena, "x\ny\n");
        let s = FileSpan::new(Some(fs), 0, 1);
        assert_eq!(s.file().unwrap().text(), "x\ny\n");
    }

    #[test]
    fn test_expand() {
        let arena = Bump::new();
        let fs = test_source(&arena, "hello..........");
        let a = Span::File(FileSpan::new(Some(fs), 5, 10));
        let b = Span::File(FileSpan::new(Some(fs), 0, 15));
        let expanded = a.expand(&b).unwrap();
        assert_eq!(expanded.start_location().unwrap().offset, 0);
        assert_eq!(expanded.end_location().unwrap().offset, 15);
    }

    #[test]
    fn test_expand_same_start() {
        let arena = Bump::new();
        let fs = test_source(&arena, "aaaaaaaaaa");
        let a = Span::File(FileSpan::new(Some(fs), 0, 10));
        let b = Span::File(FileSpan::new(Some(fs), 0, 5));
        let expanded = a.expand(&b).unwrap();
        assert_eq!(expanded.start_location().unwrap().offset, 0);
        assert_eq!(expanded.end_location().unwrap().offset, 10);
    }

    #[test]
    fn test_trim_right_no_whitespace() {
        let arena = Bump::new();
        let s = Span::File(FileSpan::new(Some(test_source(&arena, "abc")), 0, 3));
        assert_eq!(s.trim_right().unwrap().text().unwrap(), "abc");
    }

    #[test]
    fn test_trim_right_spaces() {
        let arena = Bump::new();
        let s = Span::File(FileSpan::new(Some(test_source(&arena, "abc  ")), 0, 5));
        let trimmed = s.trim_right().unwrap();
        assert_eq!(trimmed.text().unwrap(), "abc");
        assert_eq!(trimmed.end_location().unwrap().offset, 3);
    }

    #[test]
    fn test_trim_right_newlines_and_spaces() {
        let arena = Bump::new();
        let s = Span::File(FileSpan::new(Some(test_source(&arena, "abc\n\r ")), 0, 6));
        assert_eq!(s.trim_right().unwrap().text().unwrap(), "abc");
    }

    #[test]
    fn test_before() {
        let arena = Bump::new();
        let fs = test_source(&arena, "abcdefghij");
        let parent = Span::File(FileSpan::new(Some(fs), 0, 10));
        let sub = Span::File(FileSpan::new(Some(fs), 3, 6));
        assert_eq!(parent.before(&sub).unwrap().text().unwrap(), "abc");
    }

    #[test]
    fn test_before_different_file() {
        let arena = Bump::new();
        let u1 = test_url("file:///a.scss");
        let u2 = test_url("file:///b.scss");
        let fs1 = FileSource::new_in(&arena, "abcdefghij", Some(u1));
        let fs2 = FileSource::new_in(&arena, "x", Some(u2));
        let parent = Span::File(FileSpan::new(Some(fs1), 0, 10));
        let sub = Span::File(FileSpan::new(Some(fs2), 0, 1));
        assert!(parent.before(&sub).is_err());
    }

    #[test]
    fn test_before_not_contained() {
        let arena = Bump::new();
        let fs = test_source(&arena, "fghij");
        let parent = Span::File(FileSpan::new(Some(fs), 2, 4));
        let sub = Span::File(FileSpan::new(Some(fs), 0, 5));
        assert!(parent.before(&sub).is_err());
    }

    #[test]
    fn test_before_at_start() {
        let arena = Bump::new();
        let fs = test_source(&arena, "hello");
        let parent = Span::File(FileSpan::new(Some(fs), 0, 5));
        let sub = Span::File(FileSpan::new(Some(fs), 0, 2));
        assert_eq!(parent.before(&sub).unwrap().text().unwrap(), "");
    }

    #[test]
    fn test_after() {
        let arena = Bump::new();
        let fs = test_source(&arena, "abcdefghij");
        let parent = Span::File(FileSpan::new(Some(fs), 0, 10));
        let sub = Span::File(FileSpan::new(Some(fs), 3, 6));
        assert_eq!(parent.after(&sub).unwrap().text().unwrap(), "ghij");
    }

    #[test]
    fn test_after_at_end() {
        let arena = Bump::new();
        let fs = test_source(&arena, "hello");
        let parent = Span::File(FileSpan::new(Some(fs), 0, 5));
        let sub = Span::File(FileSpan::new(Some(fs), 3, 5));
        assert_eq!(parent.after(&sub).unwrap().text().unwrap(), "");
    }

    #[test]
    fn test_between() {
        let arena = Bump::new();
        let fs = test_source(&arena, "abcdefghij");
        let a = Span::File(FileSpan::new(Some(fs), 2, 3));
        let b = Span::File(FileSpan::new(Some(fs), 7, 8));
        assert_eq!(a.between(&b).unwrap().text().unwrap(), "defg");
    }

    #[test]
    fn test_between_wrong_order() {
        let arena = Bump::new();
        let fs = test_source(&arena, "abcdefghij");
        let a = Span::File(FileSpan::new(Some(fs), 7, 8));
        let b = Span::File(FileSpan::new(Some(fs), 2, 3));
        assert!(a.between(&b).is_err());
    }

    #[test]
    fn test_contains() {
        let arena = Bump::new();
        let fs = test_source(&arena, "abcdefghij");
        let parent = Span::File(FileSpan::new(Some(fs), 0, 10));
        let sub = Span::File(FileSpan::new(Some(fs), 2, 5));
        assert!(parent.contains(&sub).unwrap());
    }

    #[test]
    fn test_contains_different_file() {
        let arena = Bump::new();
        let u1 = test_url("file:///f.scss");
        let u2 = test_url("file:///g.scss");
        let fs1 = FileSource::new_in(&arena, "abcdefghij", Some(u1));
        let fs2 = FileSource::new_in(&arena, "cde", Some(u2));
        let parent = Span::File(FileSpan::new(Some(fs1), 0, 10));
        let sub = Span::File(FileSpan::new(Some(fs2), 2, 5));
        assert!(!parent.contains(&sub).unwrap());
    }

    #[test]
    fn test_contains_outside_range() {
        let arena = Bump::new();
        let fs = test_source(&arena, "fghij");
        let parent = Span::File(FileSpan::new(Some(fs), 5, 10));
        let sub = Span::File(FileSpan::new(Some(fs), 0, 15));
        assert!(!parent.contains(&sub).unwrap());
    }

    #[test]
    fn test_contains_nil_urls() {
        let arena = Bump::new();
        let fs = test_source(&arena, "abcdefghij");
        let parent = Span::File(FileSpan::new(Some(fs), 0, 10));
        let sub = Span::File(FileSpan::new(Some(fs), 2, 5));
        assert!(parent.contains(&sub).unwrap());
    }

    #[test]
    fn test_subspan() {
        let arena = Bump::new();
        let s = Span::File(FileSpan::new(
            Some(test_source(&arena, "abcdefghij")),
            0,
            10,
        ));
        let sub = s.subspan(2, 5).unwrap();
        assert_eq!(sub.text().unwrap(), "cde");
        assert_eq!(sub.start_location().unwrap().offset, 2);
        assert_eq!(sub.end_location().unwrap().offset, 5);
    }

    #[test]
    fn test_subspan_full_range() {
        let arena = Bump::new();
        let s = Span::File(FileSpan::new(
            Some(test_source(&arena, "abcdefghijklm")),
            3,
            13,
        ));
        let sub = s.subspan(0, 10).unwrap();
        assert_eq!(sub.text().unwrap(), "defghijklm");
        assert_eq!(sub.start_location().unwrap().offset, 3);
        assert_eq!(sub.end_location().unwrap().offset, 13);
    }

    #[test]
    fn test_subspan_empty() {
        let arena = Bump::new();
        let s = Span::File(FileSpan::new(
            Some(test_source(&arena, "abcdefghij")),
            5,
            15,
        ));
        assert_eq!(s.subspan(3, 3).unwrap().text().unwrap(), "");
    }

    #[test]
    fn test_bogus_span() {
        let bogus = Span::File(BOGUS_SPAN);
        assert_eq!(bogus.file().unwrap(), None);
        assert_eq!(bogus.text().unwrap(), "");
    }

    #[test]
    fn test_string() {
        let arena = Bump::new();
        let s = Span::File(FileSpan::new(Some(test_source(&arena, "hello")), 0, 5));
        assert_eq!(s.string().unwrap(), "hello");
    }

    #[test]
    fn test_filesource_get_line() {
        let arena = Bump::new();
        let fs = test_source(&arena, "foo\nbar\nbaz");
        assert_eq!(fs.get_line(0), 0);
        assert_eq!(fs.get_line(4), 1);
        assert_eq!(fs.get_line(8), 2);
    }

    #[test]
    fn test_filesource_get_column() {
        let arena = Bump::new();
        let fs = test_source(&arena, "foo\nbar\nbaz");
        assert_eq!(fs.get_column(0), 0);
        assert_eq!(fs.get_column(5), 1);
        assert_eq!(fs.get_column(6), 2);
    }

    #[test]
    fn test_filesource_get_offset() {
        let arena = Bump::new();
        let fs = test_source(&arena, "foo\nbar\nbaz");
        assert_eq!(fs.get_offset(1), 4);
    }

    #[test]
    fn test_filesource_get_text() {
        let arena = Bump::new();
        let fs = test_source(&arena, "hello world");
        assert_eq!(fs.get_text(0, 5), "hello");
        assert_eq!(fs.get_text(6, 11), "world");
    }

    #[test]
    fn test_same_url() {
        assert!(same_url(None, None));
        assert!(!same_url(None, Some(&test_url("file:///a.scss"))));
        assert!(!same_url(Some(&test_url("file:///a.scss")), None));
        assert!(same_url(
            Some(&test_url("file:///a.scss")),
            Some(&test_url("file:///a.scss"))
        ));
        assert!(!same_url(
            Some(&test_url("file:///a.scss")),
            Some(&test_url("file:///b.scss"))
        ));
    }

    #[test]
    fn test_is_whitespace_byte() {
        assert!(is_whitespace_byte(b' '));
        assert!(is_whitespace_byte(b'\t'));
        assert!(is_whitespace_byte(b'\n'));
        assert!(is_whitespace_byte(b'\r'));
        assert!(is_whitespace_byte(b'\x0C'));
        assert!(!is_whitespace_byte(b'a'));
    }

    #[test]
    fn test_is_name() {
        assert!(is_name(b'a'));
        assert!(is_name(b'Z'));
        assert!(is_name(b'0'));
        assert!(is_name(b'_'));
        assert!(is_name(b'-'));
        assert!(is_name(0x80));
        assert!(!is_name(b'.'));
        assert!(!is_name(b' '));
    }

    #[test]
    fn test_column_at() {
        assert_eq!(column_at("abc", 0), 0);
        assert_eq!(column_at("abc", 1), 1);
        assert_eq!(column_at("a\nb", 2), 0);
        assert_eq!(column_at("a\nbc", 3), 1);
    }
}
