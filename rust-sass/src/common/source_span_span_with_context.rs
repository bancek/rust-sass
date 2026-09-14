// Copyright (c) 2014, the Dart project authors.  Please see the AUTHORS file
// for details. All rights reserved. Use of this source code is governed by a
// BSD-style license that can be found in the LICENSE file.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: (external) package:source_span/lib/src/span_with_context.dart + lib/src/highlighter.dart
// go-source: go/sasscommon/source_span_span_with_context.go + go/sasscommon/source_span_highlighter.go

use crate::common::source_span_highlighter::Highlighter;
use crate::url::SassUrl;

use std::fmt::Write;

use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::{column_at, is_whitespace_byte, same_url, FileSpan, SourceLocation};
use crate::common::pretty_uri::pretty_uri;
use crate::common::source_span_highlighter::HighlightOptions;
use crate::common::span::Span;
use crate::common::span_error::SpanError;

use crate::io::Io;

/// A segment of source text with additional display context.
///
/// Matches Dart: `SourceSpanWithContext`
/// (`package:source_span/lib/src/span_with_context.dart`) — `context` is the
/// text around the span including its line, and it must contain `text`
/// starting at `start.column` within some line. This is the owned boundary
/// type: [`from_span`](Self::from_span) copies the text out of the arena so
/// `'parse` never leaks into a public error. Rust shift: construction
/// returns `Result` (Dart throws `ArgumentError`), and the span-algebra
/// methods (`subspan`, `trim*`, `expand`, `before`/`after`/`between`,
/// `contains`) live here directly instead of on a `SourceSpanMixin`
/// extension (failures surface as [`SpanError`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSpanWithContext {
    /// The URL of the source, if known.
    pub source_url: Option<SassUrl>,
    /// The 0-based start location of the span.
    pub start: SourceLocation,
    /// The 0-based end location of the span (exclusive).
    pub end: SourceLocation,
    /// The source text for this span.
    pub text: String,
    /// Text around the span, including the line containing it.
    pub context: String,
}

impl SourceSpanWithContext {
    /// The 1-based line number where the span starts (for messages).
    pub fn line(&self) -> usize {
        self.start.line + 1
    }

    /// The 1-based column where the span starts (for messages).
    pub fn column(&self) -> usize {
        self.start.column + 1
    }

    /// The length of the span in bytes (end offset minus start offset).
    pub fn length(&self) -> usize {
        self.end.offset.saturating_sub(self.start.offset)
    }

    /// Whether the span covers multiple lines.
    ///
    /// Matches Dart: `isMultiline` (`package:source_span/lib/src/utils.dart`).
    pub fn is_multiline(&self) -> bool {
        self.start.line != self.end.line
    }

    /// Creates a span from `start` to `end` (exclusive) containing `text` in
    /// the given `context`.
    ///
    /// Matches Dart: the `SourceSpanWithContext` constructor
    /// (`span_with_context.dart`) — `context` must contain `text`, and
    /// `text` must start at `start.column` in a line within `context`
    /// (checked via [`find_line_start`]); Dart throws `ArgumentError`, Rust
    /// returns `SassError::Script`.
    pub fn new(
        start: SourceLocation,
        end: SourceLocation,
        text: String,
        context: String,
        source_url: Option<SassUrl>,
    ) -> SassResult<Self> {
        if !context.contains(&text) {
            return Err(Box::new(SassError::Script {
                message: format!(
                    "The context line \"{}\" must contain \"{}\".",
                    context, text
                ),
                argument_name: None,
            }));
        }
        if find_line_start(&context, &text, start.column).is_none() {
            return Err(Box::new(SassError::Script {
                message: format!(
                    "The span text \"{}\" must start at column {} in a line within \"{}\".",
                    text,
                    start.column + 1,
                    context
                ),
                argument_name: None,
            }));
        }
        Ok(SourceSpanWithContext {
            source_url,
            start,
            end,
            text,
            context,
        })
    }

    /// Copies an arena span into an owned boundary value.
    ///
    /// Matches Dart: resolving a `SourceSpan`'s start/end/text/context
    /// (`span.dart`); the copy is what keeps `'parse` out of public errors.
    pub fn from_span(span: &Span<'_>) -> SassResult<Self> {
        Ok(SourceSpanWithContext {
            source_url: span.source_url()?,
            start: span.start_location()?,
            end: span.end_location()?,
            text: span.text()?,
            context: span.context()?,
        })
    }

    /// Copies an arena file span into an owned boundary value.
    pub fn from_file_span(span: &FileSpan<'_>) -> SassResult<Self> {
        Self::from_span(&Span::File(*span))
    }

    /// Returns the sub-span from `sub_start` to `sub_end` (exclusive) after
    /// the beginning of this span.
    ///
    /// Matches Dart: `SourceSpanWithContextExtension.subspan`
    /// (`span_with_context.dart`; line/column math via `subspanLocations` in
    /// `utils.dart`). A full-range sub-span returns `self` unchanged.
    pub fn subspan(&self, sub_start: usize, sub_end: usize) -> Result<Self, SpanError> {
        if sub_start > sub_end || sub_end > self.text.len() {
            return Err(SpanError::Range("Subspan out of bounds".into()));
        }
        if sub_start == 0 && sub_end == self.text.len() {
            return Ok(self.clone());
        }
        Ok(SourceSpanWithContext {
            start: SourceLocation {
                offset: self.start.offset + sub_start,
                line: self.start.line
                    + self.text.as_bytes()[..sub_start]
                        .iter()
                        .filter(|&&b| b == b'\n')
                        .count(),
                column: column_at(&self.text, sub_start),
            },
            end: SourceLocation {
                offset: self.start.offset + sub_end,
                line: self.start.line
                    + self.text.as_bytes()[..sub_end]
                        .iter()
                        .filter(|&&b| b == b'\n')
                        .count(),
                column: column_at(&self.text, sub_end),
            },
            text: self.text[sub_start..sub_end].to_string(),
            context: self.context.clone(),
            source_url: self.source_url.clone(),
        })
    }

    /// Returns this span with trailing whitespace removed.
    ///
    /// Matches Dart: `SourceSpanMixin.trimRight` (via `span_mixin.dart`).
    pub fn trim_right(&self) -> Result<Self, SpanError> {
        let text = &self.text;
        let mut end = text.len();
        while end > 0 && is_whitespace_byte(text.as_bytes()[end - 1]) {
            end -= 1;
        }
        if end == text.len() {
            return Ok(self.clone());
        }
        let new_end = SourceLocation {
            offset: self.end.offset - (text.len() - end),
            line: self.end.line,
            column: column_at(text, end),
        };
        Ok(SourceSpanWithContext {
            start: self.start,
            end: new_end,
            text: text[..end].to_string(),
            context: self.context.clone(),
            source_url: self.source_url.clone(),
        })
    }

    /// Returns this span with leading whitespace removed.
    ///
    /// Matches Dart: `SourceSpanMixin.trimLeft` (via `span_mixin.dart`).
    pub fn trim_left(&self) -> Result<Self, SpanError> {
        let bytes = self.text.as_bytes();
        let mut start = 0;
        while start < bytes.len() && is_whitespace_byte(bytes[start]) {
            start += 1;
        }
        if start == 0 {
            return Ok(self.clone());
        }
        self.subspan(start, self.text.len())
    }

    /// Returns this span with leading and trailing whitespace removed.
    ///
    /// Matches Dart: `SourceSpanMixin.trim` (via `span_mixin.dart`).
    pub fn trim(&self) -> Result<Self, SpanError> {
        self.trim_left()?.trim_right()
    }

    /// Creates the union of `self` and `other`.
    ///
    /// Matches Dart: `SourceSpanMixin.expand` (via `span.dart` — both spans
    /// must share a source URL and overlap or be adjacent). The union keeps
    /// an empty text/context: it locates, not displays.
    pub fn expand(&self, other: &Self) -> Result<Self, SpanError> {
        if !same_url(self.source_url.as_ref(), other.source_url.as_ref()) {
            return Err(SpanError::Argument(format!(
                "Source URLs {:?} and {:?} don't match.",
                self.source_url.as_ref().map(|u| u.as_str()),
                other.source_url.as_ref().map(|u| u.as_str()),
            )));
        }
        let start = if other.start.offset < self.start.offset {
            other.start
        } else {
            self.start
        };
        let end = if other.end.offset > self.end.offset {
            other.end
        } else {
            self.end
        };
        Ok(SourceSpanWithContext {
            start,
            end,
            text: String::new(),
            context: String::new(),
            source_url: self.source_url.clone(),
        })
    }

    /// The span from the start of `self` to the start of `inner`.
    ///
    /// Matches Dart: `SourceSpanMixin.before` (`span.dart`) — `inner` must
    /// be contained in `self` with a matching source URL.
    pub fn before(&self, inner: &Self) -> Result<Self, SpanError> {
        if !same_url(self.source_url.as_ref(), inner.source_url.as_ref()) {
            return Err(SpanError::Argument("different files".into()));
        }
        if inner.start.offset < self.start.offset || inner.end.offset > self.end.offset {
            return Err(SpanError::Argument("not contained".into()));
        }
        let offset = inner.start.offset - self.start.offset;
        if offset == 0 || offset > self.text.len() {
            return Ok(SourceSpanWithContext {
                start: self.start,
                end: self.start,
                text: String::new(),
                context: self.context.clone(),
                source_url: self.source_url.clone(),
            });
        }
        Ok(SourceSpanWithContext {
            start: self.start,
            end: inner.start,
            text: self.text[..offset].to_string(),
            context: self.context.clone(),
            source_url: self.source_url.clone(),
        })
    }

    /// The span from the end of `inner` to the end of `self`.
    ///
    /// Matches Dart: `SourceSpanMixin.after` (`span.dart`) — `inner` must
    /// be contained in `self` with a matching source URL.
    pub fn after(&self, inner: &Self) -> Result<Self, SpanError> {
        if !same_url(self.source_url.as_ref(), inner.source_url.as_ref()) {
            return Err(SpanError::Argument("different files".into()));
        }
        if inner.start.offset < self.start.offset || inner.end.offset > self.end.offset {
            return Err(SpanError::Argument("not contained".into()));
        }
        let offset = inner.end.offset - self.start.offset;
        if offset >= self.text.len() {
            return Ok(SourceSpanWithContext {
                start: self.end,
                end: self.end,
                text: String::new(),
                context: self.context.clone(),
                source_url: self.source_url.clone(),
            });
        }
        Ok(SourceSpanWithContext {
            start: inner.end,
            end: self.end,
            text: self.text[offset..].to_string(),
            context: self.context.clone(),
            source_url: self.source_url.clone(),
        })
    }

    /// The gap between the end of `self` and the start of `other`.
    ///
    /// Matches Dart: `SourceSpanMixin.between` (`span.dart`) — `self` must
    /// end before `other` starts, with matching source URLs.
    pub fn between(&self, other: &Self) -> Result<Self, SpanError> {
        if !same_url(self.source_url.as_ref(), other.source_url.as_ref()) {
            return Err(SpanError::Argument("different files".into()));
        }
        if self.end.offset > other.start.offset {
            return Err(SpanError::Argument("s isn't before other".into()));
        }
        Ok(SourceSpanWithContext {
            start: self.end,
            end: other.start,
            text: String::new(),
            context: self.context.clone(),
            source_url: self.source_url.clone(),
        })
    }

    /// Whether `target` is contained in `self` with a matching source URL.
    ///
    /// Matches Dart: `SourceSpanMixin.contains` (`span.dart`) — unlike the
    /// other span-algebra methods this returns `false` (not an error) on a
    /// URL mismatch.
    pub fn contains(&self, target: &Self) -> Result<bool, SpanError> {
        if !same_url(self.source_url.as_ref(), target.source_url.as_ref()) {
            return Ok(false);
        }
        if self.start.offset > target.start.offset {
            return Ok(false);
        }
        Ok(self.end.offset >= target.end.offset)
    }

    /// Formats `message` as `line L, column C of <url>: <message>` plus the
    /// highlighted span text.
    ///
    /// Matches Dart: `SourceSpanMixin.message` (`span.dart`).
    pub fn message(
        &self,
        message: &str,
        opts: &HighlightOptions,
        io: &dyn Io,
    ) -> SassResult<String> {
        let mut buf = format!("line {}, column {}", self.line(), self.column());
        if let Some(ref url) = self.source_url {
            write!(buf, " of {}", pretty_uri(url, io)).unwrap();
        }
        write!(buf, ": {message}").unwrap();
        let hl = self.highlight(opts, io)?;
        if !hl.is_empty() {
            buf.push('\n');
            buf.push_str(&hl);
        }
        Ok(buf)
    }

    /// Returns the highlighted span text.
    ///
    /// Matches Dart: `SourceSpanMixin.highlight` (`span.dart`).
    pub fn highlight(&self, opts: &HighlightOptions, io: &dyn Io) -> SassResult<String> {
        let mut h = Highlighter::new(self, opts, io)?;
        h.highlight()
    }

    /// Returns the highlighted primary span text with labeled secondaries.
    ///
    /// Matches Dart: `SourceSpanMixin.highlightMultiple` (`span.dart`).
    pub fn highlight_multiple(
        &self,
        primary_label: &str,
        secondary: &[(SourceSpanWithContext, String)],
        opts: &HighlightOptions,
        io: &dyn Io,
    ) -> SassResult<String> {
        let mut h = Highlighter::new_multiple(self, primary_label, secondary, opts, io)?;
        h.highlight()
    }

    /// Formats `message` like [`message`](Self::message) but with the
    /// multi-span highlight.
    ///
    /// Matches Dart: `SourceSpanMixin.messageMultiple` (`span.dart`).
    pub fn message_multiple(
        &self,
        message: &str,
        primary_label: &str,
        secondary: &[(SourceSpanWithContext, String)],
        opts: &HighlightOptions,
        io: &dyn Io,
    ) -> SassResult<String> {
        let mut buf = format!("line {}, column {}", self.line(), self.column());
        if let Some(ref url) = self.source_url {
            write!(buf, " of {}", pretty_uri(url, io)).unwrap();
        }
        writeln!(buf, ": {message}").unwrap();
        let hl = self.highlight_multiple(primary_label, secondary, opts, io)?;
        buf.push_str(&hl);
        Ok(buf)
    }
}

/// Finds the line in `context` containing `text` at `column`.
///
/// Matches Dart: `findLineStart` (`package:source_span/lib/src/utils.dart`) —
/// returns the byte index where that line begins, or `None` if no line holds
/// `text` at the given column. An empty `text` matches the first line with
/// at least `column` characters.
pub(crate) fn find_line_start(context: &str, text: &str, column: usize) -> Option<usize> {
    if text.is_empty() {
        let mut beginning_of_line = 0;
        loop {
            match context[beginning_of_line..].find('\n') {
                None => {
                    return (context.len() - beginning_of_line >= column)
                        .then_some(beginning_of_line);
                }
                Some(idx) => {
                    if idx >= column {
                        return Some(beginning_of_line);
                    }
                    beginning_of_line += idx + 1;
                }
            }
        }
    }

    let mut idx = 0;
    loop {
        let found = context[idx..].find(text)?;
        idx += found;
        let line_start = if idx > 0 {
            context.as_bytes()[..idx]
                .iter()
                .rposition(|&b| b == b'\n')
                .map(|i| i + 1)
                .unwrap_or(0)
        } else {
            0
        };
        let text_column = idx - line_start;
        if column == text_column {
            return Some(line_start);
        }
        idx += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::io::VirtualIo;
    use crate::termglyph::GlyphSet;
    use bumpalo::Bump;

    fn test_io() -> VirtualIo {
        VirtualIo::with_cwd("/home/user")
    }

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

    fn loc(offset: usize, line: usize, column: usize) -> SourceLocation {
        SourceLocation {
            offset,
            line,
            column,
        }
    }

    fn ssc(
        start: SourceLocation,
        end: SourceLocation,
        text: &str,
        context: &str,
    ) -> SourceSpanWithContext {
        SourceSpanWithContext::new(start, end, text.to_string(), context.to_string(), None).unwrap()
    }

    fn ssc_url(
        start: SourceLocation,
        end: SourceLocation,
        text: &str,
        context: &str,
        url: &str,
    ) -> SourceSpanWithContext {
        SourceSpanWithContext::new(
            start,
            end,
            text.to_string(),
            context.to_string(),
            Some(SassUrl::parse(url).unwrap()),
        )
        .unwrap()
    }

    #[test]
    fn test_from_span_basic() {
        let arena = Bump::new();
        let fs = test_source(&arena, "body { color: red; }");
        let span = Span::File(FileSpan::new(Some(fs), 0, 20));
        let ssc = SourceSpanWithContext::from_span(&span).unwrap();
        assert_eq!(ssc.text, "body { color: red; }");
        assert_eq!(ssc.line(), 1);
        assert_eq!(ssc.column(), 1);
        assert!(!ssc.is_multiline());
    }

    #[test]
    fn test_from_span_multiline() {
        let arena = Bump::new();
        let fs = test_source(&arena, "a {\n  b: c;\n}");
        let span = Span::File(FileSpan::new(Some(fs), 0, 13));
        let ssc = SourceSpanWithContext::from_span(&span).unwrap();
        assert!(ssc.is_multiline());
        assert_eq!(ssc.start.line, 0);
        assert_eq!(ssc.end.line, 2);
        assert_eq!(ssc.length(), 13);
    }

    #[test]
    fn test_new_valid() {
        let start = loc(0, 0, 0);
        let end = loc(5, 0, 5);
        let ssc =
            SourceSpanWithContext::new(start, end, "hello".into(), "hello world".into(), None)
                .unwrap();
        assert_eq!(ssc.text, "hello");
        assert_eq!(ssc.context, "hello world");
    }

    #[test]
    fn test_new_text_not_in_context() {
        let result = SourceSpanWithContext::new(
            loc(0, 0, 0),
            loc(5, 0, 5),
            "hello".into(),
            "goodbye".into(),
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_new_bad_column() {
        let result = SourceSpanWithContext::new(
            loc(0, 0, 5),
            loc(5, 0, 10),
            "hello".into(),
            "hello world".into(),
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_accessors() {
        let s = ssc(loc(0, 0, 0), loc(5, 0, 5), "hello", "hello");
        assert_eq!(s.text, "hello");
        assert_eq!(s.context, "hello");
        assert_eq!(s.start, loc(0, 0, 0));
        assert_eq!(s.end, loc(5, 0, 5));
        assert_eq!(s.length(), 5);
    }

    #[test]
    fn test_source_url() {
        let s = ssc_url(
            loc(0, 0, 0),
            loc(5, 0, 5),
            "hello",
            "hello",
            "file:///test.scss",
        );
        assert_eq!(s.source_url.as_ref().unwrap().as_str(), "file:///test.scss");
    }

    // --- Subspan ---

    #[test]
    fn test_subspan() {
        let s = ssc(loc(10, 0, 0), loc(16, 0, 0), "abcdef", "abcdef");
        let sub = s.subspan(2, 5).unwrap();
        assert_eq!(sub.text, "cde");
        assert_eq!(sub.start.offset, 12);
        assert_eq!(sub.end.offset, 15);
    }

    #[test]
    fn test_subspan_full_range() {
        let s = ssc(loc(0, 0, 0), loc(10, 0, 0), "abcdefghij", "abcdefghij");
        let sub = s.subspan(0, 10).unwrap();
        assert_eq!(sub.text, s.text);
    }

    #[test]
    fn test_subspan_empty() {
        let s = ssc(loc(0, 0, 0), loc(10, 0, 0), "abcdefghij", "abcdefghij");
        let sub = s.subspan(3, 3).unwrap();
        assert_eq!(sub.text, "");
    }

    #[test]
    fn test_subspan_out_of_bounds() {
        let s = ssc(loc(0, 0, 0), loc(5, 0, 0), "hello", "hello");
        assert!(s.subspan(2, 10).is_err());
    }

    #[test]
    fn test_subspan_preserves_context() {
        let s = ssc(loc(0, 0, 0), loc(5, 0, 0), "hello", "hello world");
        let sub = s.subspan(0, 3).unwrap();
        assert_eq!(sub.context, "hello world");
    }

    #[test]
    fn test_subspan_multiline() {
        let s = ssc(
            loc(0, 0, 0),
            loc(12, 2, 0),
            "hello\nworld\n",
            "hello\nworld\n",
        );
        let sub = s.subspan(6, 11).unwrap();
        assert_eq!(sub.text, "world");
        assert_eq!(sub.start, loc(6, 1, 0));
        assert_eq!(sub.end, loc(11, 1, 5));
    }

    // --- TrimRight ---

    #[test]
    fn test_trim_right_no_op() {
        let s = ssc(loc(0, 0, 0), loc(3, 0, 3), "abc", "abc");
        assert_eq!(s.trim_right().unwrap().text, "abc");
    }

    #[test]
    fn test_trim_right() {
        let s = ssc(loc(0, 0, 0), loc(6, 0, 0), "abc   ", "abc   ");
        let trimmed = s.trim_right().unwrap();
        assert_eq!(trimmed.text, "abc");
        assert_eq!(trimmed.end.offset, 3);
    }

    #[test]
    fn test_trim_right_newlines_and_spaces() {
        let s = ssc(loc(0, 0, 0), loc(6, 0, 0), "abc\n\r ", "abc\n\r ");
        assert_eq!(s.trim_right().unwrap().text, "abc");
    }

    // --- TrimLeft ---

    #[test]
    fn test_trim_left_no_op() {
        let s = ssc(loc(0, 0, 0), loc(3, 0, 3), "abc", "abc");
        assert_eq!(s.trim_left().unwrap().text, "abc");
    }

    #[test]
    fn test_trim_left() {
        let s = ssc(loc(0, 0, 0), loc(5, 0, 0), "  abc", "  abc");
        let trimmed = s.trim_left().unwrap();
        assert_eq!(trimmed.text, "abc");
        assert_eq!(trimmed.start.offset, 2);
    }

    // --- Trim ---

    #[test]
    fn test_trim() {
        let s = ssc(loc(0, 0, 0), loc(7, 0, 0), "  abc  ", "  abc  ");
        let trimmed = s.trim().unwrap();
        assert_eq!(trimmed.text, "abc");
        assert_eq!(trimmed.start.offset, 2);
        assert_eq!(trimmed.end.offset, 5);
    }

    // --- Expand ---

    #[test]
    fn test_expand() {
        let a = ssc(loc(5, 0, 0), loc(10, 0, 0), "hello", "hello..........");
        let b = ssc(
            loc(0, 0, 0),
            loc(15, 0, 0),
            "hello..........",
            "hello..........",
        );
        let expanded = a.expand(&b).unwrap();
        assert_eq!(expanded.start.offset, 0);
        assert_eq!(expanded.end.offset, 15);
    }

    #[test]
    fn test_expand_same_start() {
        let a = ssc(loc(0, 0, 0), loc(10, 0, 0), "aaaaaaaaaa", "aaaaaaaaaa");
        let b = ssc(loc(0, 0, 0), loc(5, 0, 0), "aaaaa", "aaaaaaaaaa");
        let expanded = a.expand(&b).unwrap();
        assert_eq!(expanded.start.offset, 0);
        assert_eq!(expanded.end.offset, 10);
    }

    #[test]
    fn test_expand_different_url() {
        let a = ssc_url(
            loc(0, 0, 0),
            loc(5, 0, 0),
            "hello",
            "hello",
            "file:///a.scss",
        );
        let b = ssc_url(
            loc(0, 0, 0),
            loc(5, 0, 0),
            "hello",
            "hello",
            "file:///b.scss",
        );
        assert!(a.expand(&b).is_err());
    }

    // --- Before ---

    #[test]
    fn test_before() {
        let parent = ssc(loc(0, 0, 0), loc(10, 0, 0), "abcdefghij", "abcdefghij");
        let sub = ssc(loc(3, 0, 3), loc(6, 0, 6), "def", "abcdefghij");
        let before = parent.before(&sub).unwrap();
        assert_eq!(before.text, "abc");
    }

    #[test]
    fn test_before_at_start() {
        let parent = ssc(loc(0, 0, 0), loc(5, 0, 5), "hello", "hello");
        let sub = ssc(loc(0, 0, 0), loc(2, 0, 2), "he", "hello");
        assert!(parent.before(&sub).is_ok());
    }

    #[test]
    fn test_before_different_url() {
        let parent = ssc_url(
            loc(0, 0, 0),
            loc(10, 0, 0),
            "abcdefghij",
            "abcdefghij",
            "file:///a.scss",
        );
        let sub = ssc_url(
            loc(3, 0, 3),
            loc(6, 0, 6),
            "def",
            "abcdefghij",
            "file:///b.scss",
        );
        assert!(parent.before(&sub).is_err());
    }

    #[test]
    fn test_before_not_contained() {
        let parent = ssc(loc(5, 0, 5), loc(8, 0, 8), "fgh", "abcdefghij");
        let sub = ssc(loc(0, 0, 0), loc(10, 0, 10), "abcdefghij", "abcdefghij");
        assert!(parent.before(&sub).is_err());
    }

    // --- After ---

    #[test]
    fn test_after() {
        let parent = ssc(loc(0, 0, 0), loc(10, 0, 0), "abcdefghij", "abcdefghij");
        let sub = ssc(loc(3, 0, 3), loc(6, 0, 6), "def", "abcdefghij");
        let after = parent.after(&sub).unwrap();
        assert_eq!(after.text, "ghij");
    }

    #[test]
    fn test_after_at_end() {
        let parent = ssc(loc(0, 0, 0), loc(5, 0, 5), "hello", "hello");
        let sub = ssc(loc(3, 0, 3), loc(5, 0, 5), "lo", "hello");
        assert!(parent.after(&sub).is_ok());
    }

    #[test]
    fn test_after_different_url() {
        let parent = ssc_url(
            loc(0, 0, 0),
            loc(10, 0, 0),
            "abcdefghij",
            "abcdefghij",
            "file:///a.scss",
        );
        let sub = ssc_url(
            loc(3, 0, 3),
            loc(6, 0, 6),
            "def",
            "abcdefghij",
            "file:///b.scss",
        );
        assert!(parent.after(&sub).is_err());
    }

    // --- Between ---

    #[test]
    fn test_between() {
        let a = ssc(loc(2, 0, 2), loc(3, 0, 3), "c", "abcdefghij");
        let b = ssc(loc(7, 0, 7), loc(8, 0, 8), "h", "abcdefghij");
        let between = a.between(&b).unwrap();
        assert_eq!(between.start.offset, 3);
        assert_eq!(between.end.offset, 7);
    }

    #[test]
    fn test_between_wrong_order() {
        let a = ssc(loc(7, 0, 7), loc(8, 0, 8), "h", "abcdefghij");
        let b = ssc(loc(2, 0, 2), loc(3, 0, 3), "c", "abcdefghij");
        assert!(a.between(&b).is_err());
    }

    #[test]
    fn test_between_different_url() {
        let a = ssc_url(
            loc(2, 0, 2),
            loc(3, 0, 3),
            "c",
            "abcdefghij",
            "file:///a.scss",
        );
        let b = ssc_url(
            loc(7, 0, 7),
            loc(8, 0, 8),
            "h",
            "abcdefghij",
            "file:///b.scss",
        );
        assert!(a.between(&b).is_err());
    }

    // --- Contains ---

    #[test]
    fn test_contains() {
        let parent = ssc(loc(0, 0, 0), loc(10, 0, 0), "abcdefghij", "abcdefghij");
        let sub = ssc(loc(2, 0, 2), loc(5, 0, 5), "cde", "abcdefghij");
        assert!(parent.contains(&sub).unwrap());
    }

    #[test]
    fn test_contains_false() {
        let parent = ssc(loc(5, 0, 0), loc(10, 0, 0), "fghij", "fghij");
        let sub = ssc(loc(0, 0, 0), loc(15, 0, 0), "fghij", "fghij");
        assert!(!parent.contains(&sub).unwrap());
    }

    #[test]
    fn test_contains_different_url() {
        let parent = ssc_url(
            loc(0, 0, 0),
            loc(10, 0, 0),
            "abcdefghij",
            "abcdefghij",
            "file:///a.scss",
        );
        let sub = ssc_url(
            loc(2, 0, 2),
            loc(5, 0, 5),
            "cde",
            "abcdefghij",
            "file:///b.scss",
        );
        assert!(!parent.contains(&sub).unwrap());
    }

    #[test]
    fn test_contains_nil_urls() {
        let parent = ssc(loc(0, 0, 0), loc(10, 0, 0), "abcdefghij", "abcdefghij");
        let sub = ssc(loc(2, 0, 2), loc(5, 0, 5), "cde", "abcdefghij");
        assert!(parent.contains(&sub).unwrap());
    }

    // --- find_line_start ---

    #[test]
    fn test_find_line_start_empty_text() {
        let ctx = "abc\ndef\nghi";
        assert_eq!(find_line_start(ctx, "", 0), Some(0));
        assert_eq!(find_line_start(ctx, "", 2), Some(0));
        assert_eq!(find_line_start(ctx, "", 1), Some(0));
    }

    #[test]
    fn test_find_line_start_with_text() {
        let ctx = "  hello world\n  foo";
        assert_eq!(find_line_start(ctx, "hello", 2), Some(0));
    }

    #[test]
    fn test_find_line_start_not_found() {
        let ctx = "abc\ndef";
        assert_eq!(find_line_start(ctx, "xyz", 0), None);
    }

    // --- Message / Highlight ---

    fn ascii_hl_opts() -> HighlightOptions {
        HighlightOptions {
            glyphs: GlyphSet::Ascii,
            ..Default::default()
        }
    }

    #[test]
    fn test_message() {
        let s = ssc_url(
            loc(0, 0, 4),
            loc(7, 0, 7),
            "bar",
            "foo bar baz",
            "file:///foo.dart",
        );
        let out = s.message("oh no", &ascii_hl_opts(), &test_io()).unwrap();
        let want = [
            "line 1, column 5 of /foo.dart: oh no",
            "  ,",
            "1 | foo bar baz",
            "  |     ^^^",
            "  '",
        ]
        .join("\n");
        assert_eq!(out, want);
    }

    #[test]
    fn test_message_no_url() {
        let s = ssc(loc(0, 0, 4), loc(7, 0, 7), "bar", "foo bar baz");
        let out = s.message("oh no", &ascii_hl_opts(), &test_io()).unwrap();
        let want = [
            "line 1, column 5: oh no",
            "  ,",
            "1 | foo bar baz",
            "  |     ^^^",
            "  '",
        ]
        .join("\n");
        assert_eq!(out, want);
    }

    #[test]
    fn test_message_no_color() {
        let s = ssc_url(
            loc(0, 0, 4),
            loc(7, 0, 7),
            "bar",
            "foo bar baz",
            "file:///foo.dart",
        );
        let out = s.message("oh no", &ascii_hl_opts(), &test_io()).unwrap();
        assert!(!out.contains("\x1b["));
    }

    #[test]
    fn test_highlight() {
        let s = ssc(loc(0, 0, 4), loc(7, 0, 7), "bar", "foo bar baz");
        let out = s.highlight(&ascii_hl_opts(), &test_io()).unwrap();
        let want = ["  ,", "1 | foo bar baz", "  |     ^^^", "  '"].join("\n");
        assert_eq!(out, want);
    }

    #[test]
    fn test_message_multiple() {
        let s = ssc_url(
            loc(17, 1, 5),
            loc(21, 1, 9),
            "bang",
            "whiz bang boom",
            "file:///file1.txt",
        );
        let s2 = ssc_url(
            loc(4, 0, 4),
            loc(7, 0, 7),
            "bar",
            "foo bar baz",
            "file:///file1.txt",
        );
        let secondary = vec![(s2, "three".into())];
        let out = s
            .message_multiple("oh no", "one", &secondary, &ascii_hl_opts(), &test_io())
            .unwrap();
        assert!(out.contains("line 2, column 6"), "output:\n{out}");
        assert!(
            out.contains("one") && out.contains("three"),
            "output:\n{out}"
        );
    }

    #[test]
    fn test_highlight_multiple() {
        let s = ssc(loc(0, 0, 4), loc(7, 0, 7), "bar", "foo bar baz");
        let s2 = ssc(loc(0, 0, 8), loc(11, 0, 11), "baz", "foo bar baz");
        let secondary = vec![(s2, "baz".into())];
        let out = s
            .highlight_multiple("primary", &secondary, &ascii_hl_opts(), &test_io())
            .unwrap();
        assert!(out.contains("primary") && out.contains("baz"));
    }
}
