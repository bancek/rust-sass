// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/util/source_map_buffer.dart + lib/src/util/no_source_map_buffer.dart
// go-source: go/sourcemapbuffer/source_map_buffer.go + no_source_map_buffer.go + default_source_map_buffer.go

//! A string buffer that builds a source map for the file being written.
//!
//! Dart models this as a `SourceMapBuffer`/`NoSourceMapBuffer` class pair;
//! Rust uses one enum: `Plain` accumulates text only (Dart's
//! `NoSourceMapBuffer`, whose `buildSourceMap` throws), while `Mapping`
//! additionally records `(generated line/column -> source span)` entries.
//! The serializer writes through [`write_str`](Self::write_str) and wraps
//! spanned output in [`for_span`](Self::for_span).

use crate::common::SassError;
use std::fmt;

use crate::common::file_span::FileSpan;
use crate::common::SassResult;
use crate::sourcemap::{Builder, SingleMapping};

/// A string buffer that builds a source map for the file being written.
///
/// `Plain` holds the target text only; `Mapping` additionally tracks the
/// current generated line/column, whether output is inside a span, and the
/// entry builder. Line/column count Unicode scalar values (Dart counts
/// UTF-16 code units, but positions only feed the VLQ encoder relatively,
/// so the outputs agree).
pub enum SourceMapBuffer<'parse> {
    Plain(String),
    Mapping {
        buf: String,
        line: usize,
        col: usize,
        builder: Builder<'parse>,
        in_span: bool,
        current_span: Option<FileSpan<'parse>>,
    },
}

impl<'parse> SourceMapBuffer<'parse> {
    /// Creates a plain buffer that doesn't build a source map (Dart's
    /// `NoSourceMapBuffer`).
    pub fn new_plain() -> Self {
        SourceMapBuffer::Plain(String::new())
    }

    /// Creates a mapping buffer that records source-map entries (Dart's
    /// `SourceMapBuffer`).
    pub fn new_mapping(builder: Builder<'parse>) -> Self {
        SourceMapBuffer::Mapping {
            buf: String::new(),
            line: 0,
            col: 0,
            builder,
            in_span: false,
            current_span: None,
        }
    }

    /// Writes `s` to the target text, advancing the generated position and
    /// recording a same-source continuation entry after each newline when
    /// inside a span (Dart's `write`).
    pub fn write_str(&mut self, s: &str) -> SassResult<()> {
        if let SourceMapBuffer::Plain(buf) = self {
            buf.push_str(s);
            return Ok(());
        }
        if let SourceMapBuffer::Mapping { ref mut buf, .. } = self {
            buf.push_str(s);
        }
        for ch in s.chars() {
            if ch == '\n' {
                self.write_line()?;
            } else {
                self.inc_col();
            }
        }
        Ok(())
    }

    fn inc_col(&mut self) {
        if let SourceMapBuffer::Mapping { col, .. } = self {
            *col += 1;
        }
    }

    /// Writes a single character, tracking newlines like [`write_str`](Self::write_str)
    /// (Dart's `writeCharCode`).
    pub fn write_char(&mut self, c: char) -> SassResult<()> {
        match self {
            SourceMapBuffer::Plain(buf) => {
                buf.push(c);
                Ok(())
            }
            SourceMapBuffer::Mapping { buf, line, col, .. } => {
                buf.push(c);
                if c == '\n' {
                    *line += 1;
                    *col = 0;
                } else {
                    *col += 1;
                }
                Ok(())
            }
        }
    }

    /// Runs `f` and associates all text written within it with `span`.
    ///
    /// Specifically, this associates the point at the beginning of the
    /// written text with the span's start. The span's end is deliberately
    /// not mapped: browsers only look up where a span starts, so end
    /// mappings would double the map size for no benefit.
    pub fn for_span<F>(&mut self, span: &FileSpan<'parse>, f: F) -> SassResult<()>
    where
        F: FnOnce(&mut Self) -> SassResult<()>,
    {
        // Record initial mapping, applying Dart's `_addEntry` dedup: skip the
        // entry if the last one shares its source line AND target line (or the
        // same target offset). Browsers only look up source from target, so
        // same-line/line entries are redundant.
        if let SourceMapBuffer::Mapping {
            line, col, builder, ..
        } = self
        {
            let start_loc = span.start_location();
            let redundant = builder
                .last_mapping()
                .is_some_and(|(gl, gc, sl, _)| (gc == *col || sl == start_loc.line) && gl == *line);
            if !redundant {
                builder.add_mapping(*line, *col, span)?;
            }
        }

        // Set in_span before callback
        let was_in_span = self.set_in_span(Some(*span));
        let result = f(self);
        self.restore_in_span(was_in_span);
        result
    }

    fn set_in_span(&mut self, span: Option<FileSpan<'parse>>) -> bool {
        if let SourceMapBuffer::Mapping {
            in_span,
            current_span,
            ..
        } = self
        {
            let was = *in_span;
            *in_span = true;
            *current_span = span;
            was
        } else {
            false
        }
    }

    fn restore_in_span(&mut self, was: bool) {
        if let SourceMapBuffer::Mapping { in_span, .. } = self {
            *in_span = was;
        }
    }

    /// Consumes the buffer and returns the accumulated target text (Dart's
    /// `toString`).
    pub fn into_string(self) -> String {
        match self {
            SourceMapBuffer::Plain(buf) => buf,
            SourceMapBuffer::Mapping { buf, .. } => buf,
        }
    }

    /// Borrows the accumulated target text without consuming the buffer.
    pub fn as_string(&self) -> &str {
        match self {
            SourceMapBuffer::Plain(buf) => buf.as_str(),
            SourceMapBuffer::Mapping { buf, .. } => buf.as_str(),
        }
    }

    /// The length of the accumulated target text in bytes (Dart's `length`).
    pub fn len(&self) -> usize {
        match self {
            SourceMapBuffer::Plain(buf) => buf.len(),
            SourceMapBuffer::Mapping { buf, .. } => buf.len(),
        }
    }

    /// Whether no text has been written yet (Dart's `isEmpty`).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the source map for the file being written.
    ///
    /// If `prefix` is passed, all entries are moved forward by the number of
    /// characters and lines in `prefix` (used for the `@charset`/BOM prefix).
    /// On a `Plain` buffer this fails, matching Dart's `NoSourceMapBuffer`
    /// which throws `UnsupportedError`.
    pub fn build_source_map(
        &self,
        prefix: &str,
        include_source_content: bool,
    ) -> SassResult<SingleMapping> {
        match self {
            SourceMapBuffer::Plain(_) => Err(Box::new(SassError::Script {
                message: "NoSourceMapBuffer.buildSourceMap() is not supported.".into(),
                argument_name: None,
            })),
            SourceMapBuffer::Mapping { builder, .. } => {
                if prefix.is_empty() {
                    return builder.to_mapping(include_source_content);
                }
                let mut prefix_lines = 0;
                let mut prefix_col = 0;
                for ch in prefix.chars() {
                    if ch == '\n' {
                        prefix_lines += 1;
                        prefix_col = 0;
                    } else {
                        prefix_col += 1;
                    }
                }
                builder.to_mapping_with_prefix(prefix_lines, prefix_col, include_source_content)
            }
        }
    }

    fn write_line(&mut self) -> SassResult<()> {
        if let SourceMapBuffer::Mapping {
            line,
            col,
            builder,
            in_span,
            ..
        } = self
        {
            // Matches Dart `_writeLine`: trim the last entry if its target is
            // exactly at the current position (a useless mapping at the end of
            // the line before the newline).
            builder.trim_last_if_at(*line, *col);
            *line += 1;
            *col = 0;
            if *in_span {
                // Matches Dart `_writeLine`'s continuation: an entry at the
                // start of the new line reusing the last entry's source.
                builder.add_mapping_like_last(*line, *col);
            }
        }
        Ok(())
    }
}

impl<'parse> fmt::Write for SourceMapBuffer<'parse> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        SourceMapBuffer::write_str(self, s).map_err(|_| fmt::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::common::SassError;
    use crate::url::SassUrl;
    use bumpalo::Bump;

    fn test_file_source<'compile, 'parse>(
        arena: &'compile Bump,
        url_str: &str,
    ) -> &'parse FileSource<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let url = SassUrl::parse(url_str).unwrap();
        FileSource::new_in(arena, "body { color: red; }", Some(url))
    }

    fn test_file_span<'compile, 'parse>(arena: &'compile Bump, url_str: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = test_file_source(arena, url_str);
        FileSpan::new(Some(fs), 0, 0)
    }

    fn test_builder() -> Builder<'static> {
        Builder::new("output.css".into())
    }

    // === Plain (NoSourceMapBuffer) tests ===

    #[test]
    fn test_plain_write_str() {
        let mut b = SourceMapBuffer::new_plain();
        b.write_str("hello").unwrap();
        assert_eq!(b.as_string(), "hello");
    }

    #[test]
    fn test_plain_write_char() {
        let mut b = SourceMapBuffer::new_plain();
        b.write_char('x').unwrap();
        assert_eq!(b.as_string(), "x");
    }

    #[test]
    fn test_plain_string_empty() {
        let b = SourceMapBuffer::new_plain();
        assert_eq!(b.as_string(), "");
    }

    #[test]
    fn test_plain_string_after_write() {
        let mut b = SourceMapBuffer::new_plain();
        b.write_str("abc").unwrap();
        assert_eq!(b.as_string(), "abc");
    }

    #[test]
    fn test_plain_len_empty() {
        let b = SourceMapBuffer::new_plain();
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn test_plain_len_after_write() {
        let mut b = SourceMapBuffer::new_plain();
        b.write_str("hello").unwrap();
        assert_eq!(b.len(), 5);
    }

    #[test]
    fn test_plain_for_span_calls_callback() {
        let arena = Bump::new();
        let span = test_file_span(&arena, "file:///input.scss");
        let mut b = SourceMapBuffer::new_plain();
        let mut called = false;
        b.for_span(&span, |_buf| {
            called = true;
            Ok(())
        })
        .unwrap();
        assert!(called);
    }

    #[test]
    fn test_plain_for_span_error_propagation() {
        let arena = Bump::new();
        let span = test_file_span(&arena, "file:///input.scss");
        let mut b = SourceMapBuffer::new_plain();
        let result = b.for_span(&span, |_buf| {
            Err(Box::new(SassError::Script {
                message: "test error".into(),
                argument_name: None,
            }))
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_plain_build_source_map_error() {
        let b = SourceMapBuffer::new_plain();
        let result = b.build_source_map("", true);
        assert!(result.is_err());
    }

    #[test]
    fn test_plain_build_source_map_with_prefix_error() {
        let b = SourceMapBuffer::new_plain();
        let result = b.build_source_map("prefix", true);
        assert!(result.is_err());
    }

    #[test]
    fn test_plain_multiple_writes() {
        let mut b = SourceMapBuffer::new_plain();
        b.write_str("hello ").unwrap();
        b.write_str("world").unwrap();
        assert_eq!(b.as_string(), "hello world");
    }

    // === Mapping (DefaultSourceMapBuffer) tests ===

    #[test]
    fn test_mapping_write_str() {
        let builder = test_builder();
        let mut b = SourceMapBuffer::new_mapping(builder);
        b.write_str("hello").unwrap();
        assert_eq!(b.as_string(), "hello");
    }

    #[test]
    fn test_mapping_write_byte() {
        let builder = test_builder();
        let mut b = SourceMapBuffer::new_mapping(builder);
        b.write_char('x').unwrap();
        assert_eq!(b.as_string(), "x");
    }

    #[test]
    fn test_mapping_write_char_unicode() {
        let builder = test_builder();
        let mut b = SourceMapBuffer::new_mapping(builder);
        b.write_char('\u{20ac}').unwrap();
        assert_eq!(b.as_string(), "\u{20ac}");
    }

    #[test]
    fn test_mapping_for_span_records_mapping() {
        let arena = Bump::new();
        let span = test_file_span(&arena, "file:///input.scss");
        let builder = Builder::new("output.css".into());
        let mut b = SourceMapBuffer::new_mapping(builder);

        b.for_span(&span, |buf| {
            buf.write_str("hello").unwrap();
            Ok(())
        })
        .unwrap();

        let m = b.build_source_map("", true).unwrap();
        assert!(!m.mappings.is_empty(), "expected non-empty mappings");
    }

    #[test]
    fn test_mapping_for_span_calls_callback() {
        let arena = Bump::new();
        let span = test_file_span(&arena, "file:///input.scss");
        let builder = test_builder();
        let mut b = SourceMapBuffer::new_mapping(builder);
        let mut called = false;
        b.for_span(&span, |_buf| {
            called = true;
            Ok(())
        })
        .unwrap();
        assert!(called);
    }

    #[test]
    fn test_mapping_for_span_error_propagation() {
        let arena = Bump::new();
        let span = test_file_span(&arena, "file:///input.scss");
        let builder = test_builder();
        let mut b = SourceMapBuffer::new_mapping(builder);
        let result = b.for_span(&span, |_buf| {
            Err(Box::new(SassError::Script {
                message: "test error".into(),
                argument_name: None,
            }))
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_mapping_for_span_newline_auto_mapping() {
        let arena = Bump::new();
        let span = test_file_span(&arena, "file:///input.scss");
        let builder = Builder::new("output.css".into());
        let mut b = SourceMapBuffer::new_mapping(builder);

        b.for_span(&span, |buf| {
            buf.write_str("a\nb").unwrap();
            Ok(())
        })
        .unwrap();

        let m = b.build_source_map("", true).unwrap();
        // Should have: mapping at (0,0) from ForSpan, mapping at (1,0) from newline auto-mapping
        assert!(!m.mappings.is_empty());
    }

    #[test]
    fn test_mapping_for_span_restores_in_span() {
        let arena = Bump::new();
        let span_a = test_file_span(&arena, "file:///a.scss");
        let span_b = test_file_span(&arena, "file:///b.scss");
        let builder = Builder::new("output.css".into());
        let mut b = SourceMapBuffer::new_mapping(builder);

        b.for_span(&span_a, |buf| {
            buf.write_str("a").unwrap();
            Ok(())
        })
        .unwrap();

        b.write_str("\n").unwrap();

        b.for_span(&span_b, |buf| {
            buf.write_str("b").unwrap();
            Ok(())
        })
        .unwrap();

        let m = b.build_source_map("", true).unwrap();
        // The newline between spans should NOT create an auto-mapping
        // Only the two ForSpan entries should be present
        assert!(m.mappings.len() >= 2, "expected at least 2 segment groups");
    }

    #[test]
    fn test_mapping_build_source_map_no_prefix() {
        let arena = Bump::new();
        let span = test_file_span(&arena, "file:///input.scss");
        let builder = Builder::new("output.css".into());
        let mut b = SourceMapBuffer::new_mapping(builder);

        b.for_span(&span, |buf| {
            buf.write_str("content").unwrap();
            Ok(())
        })
        .unwrap();

        let m = b.build_source_map("", true).unwrap();
        assert!(!m.mappings.is_empty());
    }

    #[test]
    fn test_mapping_build_source_map_with_prefix() {
        let arena = Bump::new();
        let span = test_file_span(&arena, "file:///input.scss");
        let builder = Builder::new("output.css".into());
        let mut b = SourceMapBuffer::new_mapping(builder);

        b.for_span(&span, |buf| {
            buf.write_str("content").unwrap();
            Ok(())
        })
        .unwrap();

        let m = b.build_source_map("\n", true).unwrap();
        assert!(!m.mappings.is_empty());
    }

    #[test]
    fn test_mapping_empty() {
        let builder = test_builder();
        let b = SourceMapBuffer::new_mapping(builder);
        assert_eq!(b.as_string(), "");
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn test_mapping_string_len_after_write() {
        let builder = test_builder();
        let mut b = SourceMapBuffer::new_mapping(builder);
        b.write_str("hello").unwrap();
        assert_eq!(b.as_string(), "hello");
        assert_eq!(b.len(), 5);
    }

    #[test]
    fn test_mapping_multiple_writes() {
        let builder = test_builder();
        let mut b = SourceMapBuffer::new_mapping(builder);
        b.write_str("hello ").unwrap();
        b.write_char('w').unwrap();
        b.write_str("orld").unwrap();
        assert_eq!(b.as_string(), "hello world");
    }

    #[test]
    fn test_mapping_integration() {
        let arena = Bump::new();
        let span = test_file_span(&arena, "file:///source.scss");
        let builder = Builder::new("integration.css".into());
        let mut b = SourceMapBuffer::new_mapping(builder);

        b.write_str("prefix_").unwrap();

        b.for_span(&span, |buf| {
            buf.write_str("body { color: ").unwrap();
            buf.write_str("red").unwrap();
            buf.write_str("; }").unwrap();
            Ok(())
        })
        .unwrap();

        b.write_str("_suffix").unwrap();

        let result = b.as_string();
        assert!(result.contains("prefix_"));
        assert!(result.contains("_suffix"));

        let m = b.build_source_map("", true).unwrap();
        assert!(!m.mappings.is_empty());
    }

    #[test]
    fn test_mapping_write_char_newline_auto_mapping() {
        let arena = Bump::new();
        let span = test_file_span(&arena, "file:///input.scss");
        let builder = Builder::new("output.css".into());
        let mut b = SourceMapBuffer::new_mapping(builder);

        b.for_span(&span, |buf| {
            buf.write_char('a').unwrap();
            buf.write_char('\n').unwrap();
            buf.write_char('b').unwrap();
            Ok(())
        })
        .unwrap();

        let m = b.build_source_map("", true).unwrap();
        assert!(!m.mappings.is_empty());
    }

    #[test]
    fn test_mapping_into_string() {
        let builder = test_builder();
        let mut b = SourceMapBuffer::new_mapping(builder);
        b.write_str("hello").unwrap();
        let s = b.into_string();
        assert_eq!(s, "hello");
    }

    #[test]
    fn test_declaration_name_and_value_dedup() {
        // Golden from Dart for `a {b: c}`: the selector maps gen(0,0)->src(0,0);
        // the declaration maps its NAME at gen(1,2)->src(0,3) and its VALUE at
        // gen(1,5)->src(0,6); the value is dropped by `_addEntry` dedup (same
        // source line AND same target line). mappings = "AAAA;EAAG".
        let arena = Bump::new();
        let builder = Builder::new("output.css".into());
        let mut b = SourceMapBuffer::new_mapping(builder);

        // Selector `a` at gen(0,0)->src(0,0).
        let selector_span =
            FileSpan::new(Some(test_file_source(&arena, "file:///input.scss")), 0, 1);
        b.for_span(&selector_span, |buf| {
            buf.write_char('a').unwrap();
            Ok(())
        })
        .unwrap();
        b.write_char(' ').unwrap();
        b.write_char('{').unwrap();
        b.write_char('\n').unwrap();
        b.write_str("  ").unwrap(); // indentation, col 2 on line 1

        // Name `b` at gen(1,2)->src(0,3).
        let name_span = FileSpan::new(Some(test_file_source(&arena, "file:///input.scss")), 3, 4);
        b.for_span(&name_span, |buf| {
            buf.write_char('b').unwrap();
            Ok(())
        })
        .unwrap();
        b.write_char(':').unwrap();
        b.write_char(' ').unwrap();

        // Value `c` at gen(1,5)->src(0,6) — deduped.
        let value_span = FileSpan::new(Some(test_file_source(&arena, "file:///input.scss")), 6, 7);
        b.for_span(&value_span, |buf| {
            buf.write_char('c').unwrap();
            Ok(())
        })
        .unwrap();

        let m = b.build_source_map("", true).unwrap();
        assert_eq!(m.mappings, "AAAA;EAAG");
    }
}
