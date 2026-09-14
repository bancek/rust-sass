// Copyright (c) 2014, the Dart project authors.  Please see the AUTHORS file
// for details. All rights reserved. Use of this source code is governed by a
// BSD-style license that can be found in the LICENSE file.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: (external) package:string_scanner/lib/src/span_scanner.dart
// go-source: go/sasscommon/string_scanner_span_scanner.go

use crate::url::SassUrl;

use crate::common::exception::{ScanError, SpanScannerError, SpanScannerResult};
use crate::common::file_span::{FileSpan, SourceLocation};
use crate::common::source_span_file_source::FileSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineScannerState {
    pub position: usize,
    pub line: usize,
    pub column: usize,
}

/// The character-level scanner used by the parser.
///
/// Mirrors `string_scanner`'s `SpanScanner` (a `LineScanner` exposing matched
/// ranges as `FileSpan`s): scans `source` while tracking the byte `pos` plus
/// the 0-based `line`/`column`. `pos` is a byte offset; multi-byte UTF-8
/// advances it by byte length but line/column by one character. Scanner
/// failures surface as [`SpanScannerError`] (not [`SassError`]) so the parser
/// can adjust spans before conversion. See `docs/ref/common.md`.
pub struct SpanScanner<'parse> {
    source: &'parse FileSource<'parse>,
    pos: usize,
    line: usize,
    column: usize,
    // Mirrors string_scanner's `_lastMatch`: the span of the last successful
    // multi-character `scan`/`expect`. `error` without an explicit position
    // points at this span (string_scanner.dart:257-262). Lazily invalidated
    // once the position moves past the match's end.
    last_match: Option<(usize, usize)>,
    last_match_position: usize,
}

impl<'parse> SpanScanner<'parse> {
    /// Creates a new scanner starting at the beginning of `source`.
    ///
    /// Mirrors `SpanScanner(string, sourceUrl: ..., position: ...)`: here the
    /// text and URL both come from the [`FileSource`].
    pub fn new(source: &'parse FileSource<'parse>) -> Self {
        SpanScanner {
            source,
            pos: 0,
            line: 0,
            column: 0,
            last_match: None,
            last_match_position: 0,
        }
    }

    /// The full string being scanned.
    ///
    /// Mirrors `StringScanner.string`.
    pub fn text(&self) -> &'parse str {
        self.source.text()
    }

    /// The URL of the source being scanned, for error reporting.
    ///
    /// Mirrors `StringScanner.sourceUrl`: `None` when unknown/unavailable.
    pub fn source_url(&self) -> Option<&SassUrl> {
        self.source.url()
    }

    /// Whether the scanner has completely consumed the source.
    ///
    /// Mirrors `StringScanner.isDone`.
    pub fn is_done(&self) -> bool {
        self.pos >= self.source.len()
    }

    /// The current position of the scanner in the string.
    ///
    /// Mirrors `StringScanner.position` (here a byte offset, not a character
    /// index).
    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn line(&self) -> usize {
        self.line
    }

    pub fn column(&self) -> usize {
        self.column
    }

    /// The portion of the string that hasn't yet been scanned.
    ///
    /// Mirrors `StringScanner.rest`.
    pub fn rest(&self) -> &'parse str {
        &self.source.text()[self.pos..]
    }

    /// Returns the character `offset` away from the current position.
    ///
    /// Mirrors `StringScanner.peekChar`: returns `-1` when `offset` points
    /// outside the string, without affecting match state. A negative offset
    /// inspects already-consumed characters (only `-1` decodes backwards;
    /// any other negative offset returns `-1`).
    pub fn peek_char(&self, offset: isize) -> i32 {
        if offset < 0 {
            if offset != -1 {
                return -1;
            }
            if self.pos == 0 {
                return -1;
            }
            let before = &self.source.text()[..self.pos];
            match before.chars().next_back() {
                Some(c) => c as i32,
                None => -1,
            }
        } else {
            let idx = (self.pos as isize + offset) as usize;
            if idx >= self.source.len() {
                return -1;
            }
            match self.source.text()[idx..].chars().next() {
                Some(c) => c as i32,
                None => -1,
            }
        }
    }

    /// Consumes a single character and returns it.
    ///
    /// Mirrors `LineScanner.readChar` (via `StringScanner.readChar`): fails
    /// with an "expected more input." error at end of input and an
    /// "Invalid UTF-8." error on bad bytes. Does not affect match state.
    pub fn read_char(&mut self) -> SpanScannerResult<'parse, char> {
        if self.pos >= self.source.len() {
            return Err(self.err_current("expected more input.".into()));
        }
        let remaining = &self.source.text()[self.pos..];
        match remaining.chars().next() {
            Some(c) if c == '\u{FFFD}' && remaining.as_bytes()[0] < 0x80 => {
                Err(self.err_current("Invalid UTF-8.".into()))
            }
            Some(c) => {
                self.pos += c.len_utf8();
                self.advance_line_col(c);
                Ok(c)
            }
            None => Err(self.err_current("expected more input.".into())),
        }
    }

    /// Consumes `ch` when it is the next character.
    ///
    /// Mirrors `StringScanner.scanChar`: returns whether `ch` was consumed.
    pub fn scan_char(&mut self, ch: char) -> bool {
        if self.pos >= self.source.len() {
            return false;
        }
        let remaining = &self.source.text()[self.pos..];
        match remaining.chars().next() {
            Some(c) if c == ch => {
                self.pos += c.len_utf8();
                self.advance_line_col(c);
                true
            }
            _ => false,
        }
    }

    /// Consumes `ch` when it is the next character, failing otherwise.
    ///
    /// Mirrors `StringScanner.expectChar`: the error names the expected
    /// character (`"\\"` and `"\""` use their escaped forms).
    pub fn expect_char(&mut self, ch: char) -> SpanScannerResult<'parse, ()> {
        if self.scan_char(ch) {
            return Ok(());
        }
        let name = match ch {
            '\\' => r#""\\""#.to_string(),
            '"' => r#""\"""#.to_string(),
            _ => format!("\"{ch}\""),
        };
        Err(self.err_current(format!("expected {name}.")))
    }

    /// Scans forward while `s` matches at the current position.
    ///
    /// Mirrors `StringScanner.scan`: on success records the match (see
    /// [`SpanScanner::error`]); on failure restores the saved state and
    /// returns `false`.
    pub fn scan(&mut self, s: &str) -> bool {
        let start = self.pos;
        let state = self.state();
        for b in s.bytes() {
            if !self.scan_char(b as char) {
                self.set_state(state);
                return false;
            }
        }
        self.last_match = Some((start, self.pos));
        self.last_match_position = self.pos;
        true
    }

    /// Scans forward while `s` matches at the current position, failing
    /// otherwise.
    ///
    /// Mirrors `StringScanner.expect`: reports `expected {s:?}.` at the
    /// match start.
    pub fn expect(&mut self, s: &str) -> SpanScannerResult<'parse, ()> {
        let start = self.pos;
        for b in s.bytes() {
            if !self.scan_char(b as char) {
                return Err(self.err_at(start, format!("expected {s:?}.")));
            }
        }
        self.last_match = Some((start, self.pos));
        self.last_match_position = self.pos;
        Ok(())
    }

    /// Fails unless the string has been fully consumed.
    ///
    /// Mirrors `StringScanner.expectDone` (`expected no more input.`).
    pub fn expect_done(&self) -> SpanScannerResult<'parse, ()> {
        if !self.is_done() {
            return Err(SpanScannerError::Scan(ScanError {
                message: "expected no more input.".into(),
                span: self.empty_span(),
                cause: None,
            }));
        }
        Ok(())
    }

    /// Returns the substring between `start` and `end`.
    ///
    /// Mirrors `StringScanner.substring`: unlike `str` slicing, `end`
    /// defaults to the current position rather than the end of the string.
    pub fn substring(&self, start: usize, end: Option<usize>) -> &'parse str {
        let e = end.unwrap_or(self.pos);
        &self.source.text()[start..e]
    }

    /// The scanner's state, including line and column information.
    ///
    /// Mirrors `LineScanner.state`: used to save and restore the scanner
    /// when backtracking. This does not include match information.
    pub fn state(&self) -> LineScannerState {
        LineScannerState {
            position: self.pos,
            line: self.line,
            column: self.column,
        }
    }

    /// Restores a state previously saved with [`SpanScanner::state`].
    ///
    /// Mirrors `LineScanner.state=`: restores position, line, and column.
    /// Unlike Dart this takes the state by value and cannot reject a foreign
    /// scanner's state (the type system keeps states scanner-local by
    /// convention instead).
    pub fn set_state(&mut self, st: LineScannerState) {
        self.pos = st.position;
        self.line = st.line;
        self.column = st.column;
    }

    /// Moves the scanner to the byte offset `pos`, recomputing line/column.
    ///
    /// Mirrors `StringScanner.position=`: like Dart's setter this accepts
    /// any position within the source (including between a CRLF pair), but
    /// unlike Dart a jump out of bounds is a returned error rather than an
    /// `ArgumentError` throw.
    pub fn set_position(&mut self, pos: usize) -> SpanScannerResult<'parse, ()> {
        if pos > self.source.len() {
            return Err(self.err_current(format!(
                "invalid position {} (source length {})",
                pos,
                self.source.len()
            )));
        }
        self.pos = pos;
        let (line, column) = self.linecol(pos);
        self.line = line;
        self.column = column;
        Ok(())
    }

    /// The current location of the scanner.
    ///
    /// Mirrors `SpanScanner.location`: the [`FileSource`] location at the
    /// current byte position.
    pub fn location(&self) -> SourceLocation {
        let (line, column) = self.linecol(self.pos);
        SourceLocation {
            offset: self.pos,
            line,
            column,
        }
    }

    /// Returns an empty span at the current location.
    ///
    /// Mirrors `SpanScanner.emptySpan` (`location.pointSpan()`).
    pub fn empty_span(&self) -> FileSpan<'parse> {
        self.span_from_to(self.pos, self.pos)
    }

    /// Creates a [`FileSpan`] from `start_state` to the current position.
    ///
    /// Mirrors `SpanScanner.spanFrom`.
    pub fn span_from(&self, start: LineScannerState) -> FileSpan<'parse> {
        self.span_from_to(start.position, self.pos)
    }

    /// Creates a [`FileSpan`] from `start_pos` to the current position.
    ///
    /// Mirrors `SpanScanner.spanFromPosition`.
    pub fn span_from_pos(&self, start_pos: usize) -> FileSpan<'parse> {
        self.span_from_to(start_pos, self.pos)
    }

    /// Creates a [`FileSpan`] over the byte range `start..end`.
    ///
    /// Used by the span constructors above; mirrors `SourceFile.span`.
    pub fn span_from_to(&self, start: usize, end: usize) -> FileSpan<'parse> {
        FileSpan::new(Some(self.source), start, end)
    }

    /// Creates an error with `msg` over the given source range.
    ///
    /// Mirrors `StringScanner.error` / `SpanScanner.error`: with no explicit
    /// position, the error points at the last successful `scan`/`expect`
    /// match (so e.g. "Expected expression." after a `#{` points at the
    /// `#{`), falling back to an empty span at the current position once the
    /// position has moved on. A negative `length` means zero length.
    /// Produces a [`ScanError`] (via [`SpanScannerError::Scan`]) rather than
    /// throwing, so the parser can adjust the span before conversion.
    pub fn error(
        &self,
        msg: &str,
        position: Option<usize>,
        length: isize,
    ) -> SpanScannerError<'parse> {
        // Dart's StringScanner.error uses the last successful `scan`/`expect`
        // match when no position/length is given (string_scanner.dart:257-262),
        // so errors like "Expected expression." after a `#{` point at the `#{`.
        let (pos, len) = match (position, length) {
            (Some(p), _) => (p, if length < 0 { 0 } else { length as usize }),
            (None, 0) => match self.last_match {
                Some((start, end)) if self.pos == self.last_match_position => (start, end - start),
                _ => (self.pos, 0),
            },
            (None, l) => (self.pos, if l < 0 { 0 } else { l as usize }),
        };
        SpanScannerError::Scan(ScanError {
            message: msg.to_string(),
            span: self.span_from_to(pos, pos + len),
            cause: None,
        })
    }

    fn err_current(&self, msg: String) -> SpanScannerError<'parse> {
        self.error(&msg, None, 0)
    }

    fn err_at(&self, pos: usize, msg: String) -> SpanScannerError<'parse> {
        self.error(&msg, Some(pos), 0)
    }

    fn advance_line_col(&mut self, ch: char) {
        if ch == '\n' || (ch == '\r' && self.peek_char(0) != '\n' as i32) {
            self.line += 1;
            self.column = 0;
        } else {
            self.column += 1;
        }
    }

    fn linecol(&self, pos: usize) -> (usize, usize) {
        let starts = self.source.line_starts();
        let mut lo = 0;
        let mut hi = starts.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if starts[mid] <= pos {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let line = lo.saturating_sub(1);
        // Count Unicode characters, not bytes — Dart's SourceLocation columns
        // are char-based (multibyte characters count once).
        let col = self.source.text()[starts[line]..pos].chars().count();
        (line, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn scanner<'compile, 'parse>(arena: &'compile Bump, text: &str) -> SpanScanner<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        SpanScanner::new(test_source(arena, text))
    }

    #[test]
    fn test_empty_source() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "");
        assert!(s.is_done());
        assert_eq!(s.pos(), 0);
        assert_eq!(s.text(), "");
        assert!(s.read_char().is_err());
        assert!(s.peek_char(0) == -1);
        assert!(!s.scan_char('a'));
        assert!(s.expect_char('a').is_err());
    }

    #[test]
    fn test_basic_scan() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "abc");
        assert!(!s.is_done());
        assert!(s.scan_char('a'));
        assert_eq!(s.pos(), 1);
        assert!(s.scan_char('b'));
        assert_eq!(s.read_char().unwrap(), 'c');
        assert!(s.is_done());
    }

    #[test]
    fn test_peek_char() {
        let arena = Bump::new();
        let s = scanner(&arena, "abc");
        assert_eq!(s.peek_char(0), 'a' as i32);
        assert_eq!(s.peek_char(1), 'b' as i32);
        assert_eq!(s.peek_char(2), 'c' as i32);
        assert_eq!(s.peek_char(3), -1);
        assert_eq!(s.peek_char(-1), -1);
    }

    #[test]
    fn test_peek_char_negative() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "ab");
        s.read_char().unwrap();
        assert_eq!(s.peek_char(-1), 'a' as i32);
        assert_eq!(s.peek_char(0), 'b' as i32);
    }

    #[test]
    fn test_expect_char() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "abc");
        s.expect_char('a').unwrap();
        assert_eq!(s.pos(), 1);
    }

    #[test]
    fn test_expect_char_error() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "abc");
        let err = s.expect_char('x').unwrap_err();
        assert!(err.to_string().contains("expected"));
    }

    #[test]
    fn test_scan_string() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "hello world");
        assert!(s.scan("hello"));
        assert_eq!(s.pos(), 5);
        assert!(s.scan(" world"));
        assert!(s.is_done());
    }

    #[test]
    fn test_scan_string_fail_restores_state() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "hello world");
        assert!(!s.scan("hello!"));
        assert_eq!(s.pos(), 0);
    }

    #[test]
    fn test_expect_string() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "hello world");
        s.expect("hello").unwrap();
        assert_eq!(s.pos(), 5);
    }

    #[test]
    fn test_expect_done() {
        let arena = Bump::new();
        let s = scanner(&arena, "");
        s.expect_done().unwrap();
    }

    #[test]
    fn test_expect_done_error() {
        let arena = Bump::new();
        let s = scanner(&arena, "abc");
        assert!(s.expect_done().is_err());
    }

    #[test]
    fn test_rest() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "hello");
        s.scan_char('h');
        assert_eq!(s.rest(), "ello");
    }

    #[test]
    fn test_substring() {
        let arena = Bump::new();
        let s = scanner(&arena, "foo bar");
        // nil end uses scanner position (0 here), so returns empty
        assert_eq!(s.substring(0, None), "");
        // explicit end works
        assert_eq!(s.substring(0, Some(3)), "foo");
        // end at source length returns whole source
        assert_eq!(s.substring(0, Some(7)), "foo bar");
    }

    #[test]
    fn test_line_column() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "ab\ncd\nef");
        assert_eq!(s.line(), 0);
        assert_eq!(s.column(), 0);
        s.scan("ab\n");
        assert_eq!(s.line(), 1);
        assert_eq!(s.column(), 0);
        s.scan("cd\n");
        assert_eq!(s.line(), 2);
        s.scan("ef");
        assert_eq!(s.column(), 2);
    }

    #[test]
    fn test_line_column_r_n() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "ab\r\ncd");
        s.scan("ab\r\n");
        assert_eq!(s.line(), 1);
        assert_eq!(s.column(), 0);
    }

    #[test]
    fn test_peek_newline() {
        let arena = Bump::new();
        let s = scanner(&arena, "ab\ncd");
        assert_eq!(s.peek_char(2), '\n' as i32);
    }

    #[test]
    fn test_state_save_restore() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "abcdef");
        s.scan("abc");
        let st = s.state();
        s.scan("def");
        assert_eq!(s.pos(), 6);
        s.set_state(st);
        assert_eq!(s.pos(), 3);
        s.scan("def");
        assert_eq!(s.pos(), 6);
    }

    #[test]
    fn test_set_position() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "abc\ndef");
        s.set_position(4).unwrap();
        assert_eq!(s.pos(), 4);
        assert_eq!(s.line(), 1);
        assert_eq!(s.column(), 0);
    }

    #[test]
    fn test_set_position_out_of_bounds() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "abc");
        assert!(s.set_position(10).is_err());
    }

    #[test]
    fn test_span_from_to() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "hello");
        s.scan("hel");
        let span = s.span_from_to(0, 3);
        assert_eq!(span.text(), "hel");
        assert_eq!(span.start_location().offset, 0);
        assert_eq!(span.end_location().offset, 3);
    }

    #[test]
    fn test_empty_span() {
        let arena = Bump::new();
        let s = scanner(&arena, "hello");
        let span = s.empty_span();
        assert_eq!(span.text(), "");
    }

    #[test]
    fn test_span_from() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "hello");
        let st = s.state();
        s.scan("hel");
        let span = s.span_from(st);
        assert_eq!(span.text(), "hel");
    }

    #[test]
    fn test_error() {
        let arena = Bump::new();
        let s = scanner(&arena, "hello");
        let err = s.error("bad", Some(1), 2);
        assert!(matches!(err, SpanScannerError::Scan(_)));
    }

    #[test]
    fn test_location() {
        let arena = Bump::new();
        let mut s = scanner(&arena, "ab\ncd");
        s.scan("ab\n");
        let loc = s.location();
        assert_eq!(loc.offset, 3);
        assert_eq!(loc.line, 1);
        assert_eq!(loc.column, 0);
    }

    #[test]
    fn test_source_url() {
        let arena = Bump::new();
        let u = SassUrl::parse("file:///test.scss").unwrap();
        let fs = FileSource::new_in(&arena, "x", Some(u));
        let s = SpanScanner::new(fs);
        assert!(s.source_url().is_some());
    }
}
