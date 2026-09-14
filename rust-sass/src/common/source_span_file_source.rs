// Copyright (c) 2014, the Dart project authors.  Please see the AUTHORS file
// for details. All rights reserved. Use of this source code is governed by a
// BSD-style license that can be found in the LICENSE file.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: (external) package:source_span/lib/src/file.dart
// go-source: go/sasscommon/source_span_file_source.go

use crate::url::SassUrl;
use bumpalo::Bump;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::common::file_span::{FileSpan, SourceLocation};

/// A chunk of source text, usually with a URL attached.
///
/// Matches Dart: `SourceFile` (`package:source_span/lib/src/file.dart`) — it
/// does not necessarily correspond to a file on disk, just a run of text.
/// Rust shifts: the text and the precomputed `line_starts` table live in the
/// compile arena (`&'parse`), so offsets/line lookups borrow instead of
/// copying; construction is infallible (Dart constructors throw `RangeError`
/// on bad offsets, but this port validates at the call site instead).
#[derive(Debug)]
pub struct FileSource<'parse> {
    /// The URL where the source is located, if known.
    url: Option<SassUrl>,
    /// The full source text (arena copy).
    text: &'parse str,
    /// Byte offset of the first character *after* each newline; entry 0 is
    /// always `0`. A lone `\r` counts as a newline, matching Dart's
    /// `_fromList` normalization.
    line_starts: &'parse [usize],
    /// Single-entry memo for `get_line`, mirroring source_span's
    /// `_cachedLine`: consecutive span queries cluster on nearby lines.
    cached_line: Cell<Option<usize>>,
    /// Lazily formatted URL string (shared via `Rc<str>` so dedup keys and
    /// trace frames can clone by refcount instead of re-formatting).
    url_str_cache: RefCell<Option<Rc<str>>>,
}

impl<'parse> PartialEq for FileSource<'parse> {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url && self.text == other.text && self.line_starts == other.line_starts
    }
}
impl<'parse> Eq for FileSource<'parse> {}

impl<'parse> FileSource<'parse> {
    /// Creates a source from `text`, copying it into the compile arena.
    ///
    /// Matches Dart: `SourceFile.fromString` (`file.dart`) — the `[url]`
    /// argument may be absent. Rust shift: `line_starts` is precomputed here
    /// (Dart computes it in `_fromList`) and both slices borrow from `arena`,
    /// so the result lives as long as the compile.
    pub fn new_in<'compile: 'parse>(
        arena: &'compile Bump,
        text: &str,
        url: Option<SassUrl>,
    ) -> &'parse Self {
        let arena_text = arena.alloc_str(text);
        let line_starts = compute_line_starts(arena, text);
        arena.alloc(FileSource {
            url,
            text: arena_text,
            line_starts,
            cached_line: Cell::new(None),
            url_str_cache: RefCell::new(None),
        })
    }

    /// The URL as a shared string, formatted once per source.
    ///
    /// Warning-dedup keys and stack-trace frames need this repeatedly;
    /// cloning an `Rc<str>` beats re-stringifying a `SassUrl` every time.
    pub fn url_str_cached(&self) -> Option<Rc<str>> {
        if let Some(s) = self.url_str_cache.borrow().as_ref() {
            return Some(Rc::clone(s));
        }
        let s = Rc::from(self.url.as_ref()?.to_string().as_str());
        *self.url_str_cache.borrow_mut() = Some(Rc::clone(&s));
        Some(s)
    }

    /// Dart `identical()` on `SourceFile` — address equality, not `PartialEq`.
    ///
    /// Matches Dart: `SourceFile` defines no `operator ==` (so `==` on files
    /// is identity), while `_FileSpan ==`
    /// (`package:source_span/lib/src/file.dart`) compares only
    /// `start + end + sourceUrl`. `PartialEq` here stays value equality;
    /// call sites that mirror Dart `identical(file, ...)`
    /// (`interpolation_map.dart` `_isMapped`) or `==` on `SourceFile`
    /// (`binary_operation.dart` `operatorSpan`) use this instead.
    ///
    /// Takes `Option` because Rust files are nullable (`FileSpan::file()`)
    /// while Dart's `FileSpan.file` is non-null. `(None, None)` is `true`,
    /// matching Dart `identical(null, null)`.
    pub fn identical(a: Option<&FileSource<'_>>, b: Option<&FileSource<'_>>) -> bool {
        match (a, b) {
            (Some(x), Some(y)) => std::ptr::eq(x, y),
            (None, None) => true,
            _ => false,
        }
    }

    /// The URL where the source is located, if known.
    pub fn url(&self) -> Option<&SassUrl> {
        self.url.as_ref()
    }

    /// The full source text.
    pub fn text(&self) -> &'parse str {
        self.text
    }

    /// The length of the source in bytes.
    ///
    /// Matches Dart: `SourceFile.length` (which counts UTF-16 code units;
    /// identical for BMP text).
    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// Whether the source is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The number of lines in the source.
    ///
    /// Matches Dart: `SourceFile.lines` (the `line_starts` table length).
    pub fn lines(&self) -> usize {
        self.line_starts.len()
    }

    /// Byte offsets where each line begins (entry 0 is `0`).
    pub(crate) fn line_starts(&self) -> &[usize] {
        self.line_starts
    }

    /// Returns the 0-based line containing `offset`.
    ///
    /// Matches Dart: `SourceFile.getLine` (`file.dart`) — sequential calls
    /// for nearby offsets hit the single-entry `_cachedLine` memo first
    /// (same line, then next line) before falling back to binary search.
    pub fn get_line(&self, offset: usize) -> usize {
        // Dart-parity single-entry memo (source_span `_cachedLine`):
        // consecutive queries usually sit on the same or next line.
        if let Some(cached) = self.cached_line.get() {
            let starts = self.line_starts;
            if offset >= starts[cached]
                && (cached + 1 >= starts.len() || offset < starts[cached + 1])
            {
                return cached;
            }
            if cached + 1 < starts.len()
                && offset >= starts[cached + 1]
                && (cached + 2 >= starts.len() || offset < starts[cached + 2])
            {
                self.cached_line.set(Some(cached + 1));
                return cached + 1;
            }
        }

        let line = match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(line) => line.saturating_sub(1),
        };
        self.cached_line.set(Some(line));
        line
    }

    /// Returns the 0-based column of `offset` within its line.
    ///
    /// Matches Dart: `SourceFile.getColumn` (`file.dart`); when `line` is
    /// omitted there it is derived via `getLine` first, as here.
    pub fn get_column(&self, offset: usize) -> usize {
        let line = self.get_line(offset);
        offset - self.line_starts[line]
    }

    /// Returns the offset where `line` begins.
    ///
    /// Matches Dart: `SourceFile.getOffset(line, [column])` with the default
    /// column of 0 (Rust has no column parameter — callers add it).
    pub fn get_offset(&self, line: usize) -> usize {
        self.line_starts[line]
    }

    /// Returns the source text from `start` to `end` (exclusive).
    ///
    /// Matches Dart: `SourceFile.getText` (`file.dart`; defaults `end` to the
    /// end of the file, which Rust callers pass explicitly).
    pub fn get_text(&self, start: usize, end: usize) -> &'parse str {
        &self.text[start..end]
    }

    /// Returns a span from `start` to `end` (exclusive) in this source.
    ///
    /// Matches Dart: `SourceFile.span(start, [end])` (`file.dart`; Rust
    /// callers always pass `end` explicitly).
    pub fn span(&'parse self, start: usize, end: usize) -> FileSpan<'parse> {
        FileSpan::new(Some(self), start, end)
    }

    /// Returns the 0-based location at `offset`, with the column counted in
    /// characters (not bytes).
    ///
    /// Matches Dart: `SourceFile.location` → `FileLocation` (`file.dart`),
    /// which lazily derives line/column from the offset.
    pub fn location(&self, offset: usize) -> SourceLocation {
        let line = self.get_line(offset);
        let column = if offset <= self.text.len() {
            let line_start = self.line_starts[line];
            let bytes = &self.text.as_bytes()[line_start..offset];
            // Fast path: pure-ASCII prefixes have one character per byte, so
            // the column is the byte distance — no UTF-8 decoding needed.
            // Non-ASCII falls back to exact Unicode character counting
            // (Dart's SourceLocation columns are char-based).
            match bytes.iter().position(|&b| b >= 0x80) {
                None => offset - line_start,
                Some(_) => self.text[line_start..offset].chars().count(),
            }
        } else {
            offset - self.line_starts[line]
        };
        SourceLocation {
            offset,
            line,
            column,
        }
    }
}

fn compute_line_starts<'arena>(arena: &'arena Bump, text: &str) -> &'arena [usize] {
    let bytes = text.as_bytes();
    let mut starts: Vec<usize> = Vec::new();
    starts.push(0);
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' || (b == b'\r' && (i + 1 >= bytes.len() || bytes[i + 1] != b'\n')) {
            starts.push(i + 1);
        }
    }
    arena.alloc_slice_copy(&starts)
}
