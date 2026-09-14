// Copyright 2023 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/interpolation_map.dart
// go-source: go/value/sass_interpolation_map.go

use crate::common::ast_node::AstNode;
use crate::common::core_errors::ArgumentError;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::{FileSpan, SourceLocation};
use crate::common::source_span_file_source::FileSource;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::common::span::Span;
use crate::common::span_error::SpanError;

use crate::ast::sass::interpolation::{Interpolation, InterpolationPart};

/// Represents a mapped location — either a concrete FileSpan (for expression
/// positions) or a SourceLocation (for plain text positions).
///
/// Rust names Dart's `FileSpan | FileLocation` union: expression text maps to
/// the interpolated expression's full span, plain text to a location in the
/// interpolation's file.
pub enum MapLocationResult<'parse> {
    /// Text generated from an interpolated expression: the expression's span.
    Span(FileSpan<'parse>),
    /// Un-interpolated text: the corresponding location in the source file.
    Location(SourceLocation),
}

/// A map from locations in a string generated from an [`Interpolation`] to the
/// original source code in the interpolation.
#[derive(Debug)]
pub struct InterpolationMap<'parse> {
    /// The interpolation from which this map was generated.
    interpolation: Interpolation<'parse>,
    /// Location offsets in the generated string.
    ///
    /// Each of these indicates the location in the generated string that
    /// corresponds to the end of the component at the same index of
    /// `interpolation.contents`. Its length is always one less than the
    /// contents length because the last element always ends the string.
    target_offsets: Vec<usize>,
}

impl<'parse> InterpolationMap<'parse> {
    /// Creates a new import map that maps the given `target_offsets` in the
    /// generated string to the contents of `interpolation`.
    ///
    /// Each target offset at index `i` corresponds to the character in the
    /// generated string after `interpolation.contents[i]`.
    pub fn new(
        interpolation: Interpolation<'parse>,
        target_offsets: Vec<usize>,
    ) -> Result<Self, ArgumentError> {
        let expected = std::cmp::max(0, interpolation.contents.len().saturating_sub(1));
        if target_offsets.len() != expected {
            return Err(ArgumentError {
                name: Some("targetOffsets".into()),
                message: format!(
                    "InterpolationMap must have {expected} targetOffsets if the interpolation has {} components.",
                    interpolation.contents.len()
                ),
            });
        }
        Ok(InterpolationMap {
            interpolation,
            target_offsets,
        })
    }

    /// Returns whether `target` has already been mapped by this mapper (i.e.
    /// it originates from the same file source as the interpolation's span).
    ///
    /// Matches Dart `_isMapped` (`interpolation_map.dart`): `identical(file,
    /// _interpolation.span.file)` — address identity, not structural
    /// `FileSource::PartialEq` (two distinct allocations with equal content
    /// are `==` but never `identical`).
    pub fn is_mapped(&self, target: &Span<'parse>) -> SassResult<bool> {
        match target {
            Span::File(fs) => {
                let target_file = fs.file();
                let interp_file = self.interpolation.span()?.file();
                // Matches Dart: `identical(file, _interpolation.span.file)`.
                Ok(FileSource::identical(target_file, interp_file))
            }
            _ => Ok(false),
        }
    }

    /// Maps a span in the string generated from this interpolation to its
    /// original source.
    ///
    /// Returns `target` as-is if it's already been mapped.
    pub fn map_span(&self, target: &Span<'parse>) -> SassResult<Span<'parse>> {
        // Matches Dart `mapSpan` (`interpolation_map.dart`): already-mapped
        // spans (Dart `_isMapped` idempotency) return as-is — without this,
        // double-mapping an already-mapped error produces a wrong span plus
        // a spurious secondary span downstream in `map_exception`.
        if self.is_mapped(target)? {
            return Ok(target.clone());
        }
        self.map_span_inner(target)
    }

    /// Maps generated-output offsets to original source without the
    /// already-mapped guard.
    ///
    /// `map_file_span` and `map_exception` synthesize their query spans in
    /// the interpolation's own file (the owned `FileSpan` /
    /// `SourceSpanWithContext` snapshot carries generated-output offsets but
    /// no generated-file ref), so the query file is a coordinate-space
    /// placeholder, not a provenance marker — routing it through the
    /// `is_mapped` guard would always early-return unmapped. Dart never
    /// synthesizes (it passes the real generated-file span to `mapSpan`),
    /// so bypassing the guard here reproduces Dart's mapping exactly:
    /// `map_location` only reads offsets, never the query file.
    fn map_span_inner(&self, target: &Span<'parse>) -> SassResult<Span<'parse>> {
        let start = target.start_location()?;
        let end = target.end_location()?;

        let start_loc = self.map_location(start)?;
        let end_loc = self.map_location(end)?;

        match (start_loc, end_loc) {
            (MapLocationResult::Span(start_span), MapLocationResult::Span(end_span)) => {
                match start_span.expand(&Span::File(end_span)) {
                    Ok(fs) => Ok(Span::File(fs)),
                    Err(e) => self.span_err_into_sass(e),
                }
            }
            (MapLocationResult::Span(start_span), MapLocationResult::Location(end_loc)) => {
                let interp_span = self.interpolation.span()?;
                let interp_file = interp_span.file();
                let start_start = start_span.start_location();
                let offs = self.expand_interpolation_span_left(start_start.offset)?;
                Ok(Span::File(FileSpan::new(interp_file, offs, end_loc.offset)))
            }
            (MapLocationResult::Location(start_loc), MapLocationResult::Span(end_span)) => {
                let interp_span = self.interpolation.span()?;
                let interp_file = interp_span.file();
                let end_end = end_span.end_location();
                let offs = self.expand_interpolation_span_right(end_end.offset)?;
                Ok(Span::File(FileSpan::new(
                    interp_file,
                    start_loc.offset,
                    offs,
                )))
            }
            (MapLocationResult::Location(start_loc), MapLocationResult::Location(end_loc)) => {
                let interp_span = self.interpolation.span()?;
                let interp_file = interp_span.file();
                Ok(Span::File(FileSpan::new(
                    interp_file,
                    start_loc.offset,
                    end_loc.offset,
                )))
            }
        }
    }

    /// Maps a FileSpan from generated-output coordinates back to original source
    /// coordinates via this interpolation map. Returns the original span unchanged
    /// if the mapping is a no-op.
    ///
    /// This works at the FileSpan level (preserving file access for downstream
    /// span adjustment), unlike `map_exception` which operates on SassError with
    /// SourceSpanWithContext.
    ///
    /// Matches Go: interpolationMap.mapSpan (internal, lines 166-208 of
    /// map_exception logic applied directly to FileSpan).
    pub fn map_file_span(&self, target: FileSpan<'parse>) -> SassResult<Span<'parse>> {
        let interp_span = self.interpolation.span()?;

        // Already in the interpolation's source file — no mapping needed.
        // Matches Dart `identical`: address identity, not structural `==`.
        if FileSource::identical(target.file(), interp_span.file()) {
            return Ok(Span::File(target));
        }

        // Build a query span: use the target's offsets in the interpolation's
        // file coordinate space. Matches Go: parse_parser.go lines 166-170.
        // The query file is a coordinate-space placeholder (the offsets are
        // in generated-output space), so this bypasses the `is_mapped` guard
        // (see `map_span_inner`); the guard above already handled the
        // genuinely-mapped case.
        let query = Span::File(FileSpan::new(
            interp_span.file(),
            target.start_location().offset,
            target.end_location().offset,
        ));

        self.map_span_inner(&query)
    }

    /// Maps `error`'s span in the string generated from this interpolation to
    /// its original source.
    ///
    /// Returns `error` unchanged if its span is absent (a spanless script
    /// error) or already mapped. An empty interpolation remaps any error to
    /// the interpolation span; a range covering an interpolated expression
    /// becomes a multi-span error with an "error in interpolated output"
    /// secondary pointing at the generated output.
    pub fn map_exception(&self, error: &SassError) -> SassError {
        // Extract the span from the error
        let (span, original_source, _is_format) = match error {
            SassError::Format {
                ref span,
                ref original_source,
                ..
            } => (span, original_source.clone(), true),
            SassError::MultiSpan {
                ref span,
                ref original_source,
                trace: _,
                ..
            } => (span, original_source.clone(), false),
            _ => return error.clone(),
        };

        // No URL-identity early return here: Dart `mapException` has none
        // (its idempotency is `_isMapped` on the empty-contents branch plus
        // `identical(source, target)` after `mapSpan`). The error carries an
        // owned `SourceSpanWithContext` snapshot (strings + `Option<SassUrl>`,
        // no `FileSource` ref), so file identity is unrecoverable when URLs
        // are absent — `None == None` would wrongly skip mapping for every
        // URL-less error. The post-mapping no-op check below (same file +
        // same offsets) is the owned-snapshot analogue of Dart's
        // `identical(source, target)`.
        let source = span.clone();
        let interp_span = match self.interpolation.span() {
            Ok(s) => s,
            Err(_) => return error.clone(),
        };

        if self.interpolation.contents.is_empty() {
            let new_span = match SourceSpanWithContext::from_file_span(&interp_span) {
                Ok(s) => s,
                Err(_) => return error.clone(),
            };
            return SassError::Format {
                message: error.message().into(),
                span: new_span,
                original_source,
                cause: None,
                loaded_urls: vec![],
            };
        }

        // Create a Span from the error span coordinates using the interpolation's file.
        // The target span's offsets are in the generated output's coordinate space.
        let target_span = Span::File(FileSpan::new(
            interp_span.file(),
            source.start.offset,
            source.end.offset,
        ));

        // The query span is synthesized in the interpolation's own file
        // (see `map_span_inner`), so this must bypass the `is_mapped` guard.
        let mapped = match self.map_span_inner(&target_span) {
            Ok(s) => s,
            Err(_) => return error.clone(),
        };

        let source_fs = match &mapped {
            Span::File(fs) => *fs,
            _ => return error.clone(),
        };

        // Check if mapping returned the same span (no-op) — the
        // owned-snapshot analogue of Dart's `identical(source, target)` in
        // `mapException`. File identity decides (not offsets alone, which
        // live in different coordinate spaces); a URL-only check would
        // false-positive whenever both URLs are `None`.
        {
            let fs_file = source_fs.file();
            let ts_file = target_span.file().unwrap_or(None);
            if FileSource::identical(fs_file, ts_file) {
                let fs_start = source_fs.start_location().offset;
                let ts_start = target_span
                    .start_location()
                    .unwrap_or(SourceLocation {
                        offset: 0,
                        line: 0,
                        column: 0,
                    })
                    .offset;
                let fs_end = source_fs.end_location().offset;
                let ts_end = target_span
                    .end_location()
                    .unwrap_or(SourceLocation {
                        offset: 0,
                        line: 0,
                        column: 0,
                    })
                    .offset;
                if fs_start == ts_start && fs_end == ts_end {
                    return error.clone();
                }
            }
        }

        let has_expression = self.has_expression_between(&source.start, &source.end);

        let remapped_span =
            SourceSpanWithContext::from_file_span(&source_fs).unwrap_or(source.clone());

        if !has_expression {
            SassError::Format {
                message: error.message().into(),
                span: remapped_span,
                original_source,
                cause: None,
                loaded_urls: vec![],
            }
        } else {
            SassError::MultiSpan {
                message: error.message().into(),
                span: remapped_span,
                primary_label: Some(String::new()),
                secondary: vec![(source.clone(), "error in interpolated output".into())],
                original_source,
                cause: None,
                loaded_urls: vec![],
                trace: Default::default(),
            }
        }
    }

    fn span_err_into_sass(&self, e: SpanError) -> SassResult<Span<'parse>> {
        match e {
            SpanError::Sass(e) => Err(e),
            SpanError::Argument(msg) | SpanError::Range(msg) => Err(Box::new(SassError::Script {
                message: msg,
                argument_name: None,
            })),
        }
    }

    /// Maps a location in the generated string to its original source.
    ///
    /// When the location points at un-interpolated text this returns the
    /// corresponding location in the source file; when it points at text
    /// generated from interpolation it returns the full span of that
    /// interpolated expression.
    fn map_location(&self, target: SourceLocation) -> SassResult<MapLocationResult<'parse>> {
        if self.interpolation.contents.is_empty() {
            let interp_span = self.interpolation.span()?;
            return Ok(MapLocationResult::Span(interp_span));
        }

        let index = self.index_in_contents(&target);
        if let InterpolationPart::Expression(expr) = &self.interpolation.contents[index] {
            let expr_span = expr.span()?;
            return Ok(MapLocationResult::Span(expr_span));
        }

        let interp_span = self.interpolation.span()?;
        let previous_offset = if index == 0 {
            interp_span.start_location().offset
        } else {
            let prev_expr_span = match &self.interpolation.contents[index - 1] {
                InterpolationPart::Expression(e) => e.span()?,
                _ => {
                    return Err(Box::new(SassError::Script {
                        message: "Expected expression before text element".into(),
                        argument_name: None,
                    }))
                }
            };
            self.expand_interpolation_span_right(prev_expr_span.end_location().offset)?
        };

        let offset_in_string = if index == 0 {
            target.offset
        } else {
            target.offset - self.target_offsets[index - 1]
        };

        let line_starts = match interp_span.file() {
            Some(f) => f.line_starts(),
            None => &[0],
        };
        let location = source_location_from_offset(line_starts, previous_offset + offset_in_string);
        Ok(MapLocationResult::Location(location))
    }

    /// Whether the generated-output range [`start`, `end`] falls within or
    /// across any `Expression` contents of this interpolation. Mirrors Dart
    /// `mapException` (interpolation_map.dart:62-72): when it does, the mapped
    /// error becomes a MultiSpan with a secondary "error in interpolated output"
    /// pointing at the generated output.
    pub fn has_expression_between(&self, start: &SourceLocation, end: &SourceLocation) -> bool {
        if self.interpolation.contents.is_empty() {
            return false;
        }
        let start_index = self.index_in_contents(start);
        let end_index = self.index_in_contents(end);
        (start_index..=end_index.min(self.interpolation.contents.len().saturating_sub(1))).any(
            |i| {
                matches!(
                    self.interpolation.contents[i],
                    InterpolationPart::Expression(_)
                )
            },
        )
    }

    /// Returns the index in `interpolation.contents` at which `target` points.
    fn index_in_contents(&self, target: &SourceLocation) -> usize {
        for (i, &offset) in self.target_offsets.iter().enumerate() {
            if target.offset < offset {
                return i;
            }
        }
        self.interpolation.contents.len().saturating_sub(1)
    }

    /// Given the start of a [`FileSpan`] covering an interpolated expression,
    /// returns the offset of the interpolation's opening `#`.
    ///
    /// Note that this can be tricked by a `#{` that appears within a
    /// single-line comment before the expression, but since it's only used
    /// for error reporting that's probably fine.
    fn expand_interpolation_span_left(&self, start_offset: usize) -> SassResult<usize> {
        let interp_span = self.interpolation.span()?;
        let source = match interp_span.file() {
            Some(f) => f.text(),
            None => return Ok(start_offset.saturating_sub(2)),
        };
        if source.is_empty() {
            return Ok(start_offset.saturating_sub(2));
        }

        let mut i = start_offset as isize - 1;
        while i >= 0 {
            let idx = i as usize;
            if idx >= source.len() {
                break;
            }
            let prev = source.as_bytes()[idx];
            i -= 1;
            if prev == b'{' {
                if i >= 0 && source.as_bytes().get(i as usize) == Some(&b'#') {
                    break;
                }
            } else if prev == b'/' && i >= 0 {
                let second = match source.as_bytes().get(i as usize) {
                    Some(&b) => b,
                    None => break,
                };
                i -= 1;
                if second == b'*' {
                    while let Some(&b) = source.as_bytes().get(i as usize) {
                        let char_byte = b;
                        i -= 1;
                        if char_byte != b'*' {
                            continue;
                        }
                        while i >= 0
                            && source
                                .as_bytes()
                                .get(i as usize)
                                .is_some_and(|&b| b == b'*')
                        {
                            i -= 1;
                        }
                        if i >= 0
                            && source
                                .as_bytes()
                                .get(i as usize)
                                .is_some_and(|&b| b == b'/')
                        {
                            break;
                        }
                    }
                }
            }
        }

        Ok(i as usize)
    }

    /// Given the end of a [`FileSpan`] covering an interpolated expression,
    /// returns the offset of the interpolation's closing `}`.
    fn expand_interpolation_span_right(&self, end_offset: usize) -> SassResult<usize> {
        let interp_span = self.interpolation.span()?;
        let source = match interp_span.file() {
            Some(f) => f.text(),
            None => return Ok(end_offset + 1),
        };
        if source.is_empty() {
            return Ok(end_offset + 1);
        }

        let mut i = end_offset;
        while i < source.len() {
            let next = source.as_bytes()[i];
            i += 1;
            if next == b'}' {
                break;
            }
            if next == b'/' && i < source.len() {
                let second = source.as_bytes()[i];
                i += 1;
                if second == b'/' {
                    while i < source.len() {
                        let ch = source.as_bytes()[i];
                        i += 1;
                        if ch == b'\n' || ch == b'\r' || ch == b'\x0C' {
                            break;
                        }
                    }
                } else if second == b'*' {
                    while let Some(&b) = source.as_bytes().get(i) {
                        let char_byte = b;
                        i += 1;
                        if char_byte != b'*' {
                            continue;
                        }
                        while i < source.len()
                            && source.as_bytes().get(i).is_some_and(|&b| b == b'*')
                        {
                            i += 1;
                        }
                        if i < source.len() && source.as_bytes().get(i).is_some_and(|&b| b == b'/')
                        {
                            i += 1;
                            break;
                        }
                    }
                }
            }
        }

        Ok(i)
    }
}

/// Creates a `SourceLocation` with line/column computed from the given offset
/// within the source text, using a pre-built line-starts cache for O(log n)
/// lookup.
///
/// Slightly incorrect when the source holds unnecessary escapes, matching
/// Dart's documented `_mapLocation` caveat; unlikely enough to not reparse
/// for (see the Dart comment there).
fn source_location_from_offset(line_starts: &[usize], offset: usize) -> SourceLocation {
    let line = match line_starts.binary_search(&offset.saturating_add(1)) {
        Ok(pos) => pos.saturating_sub(1),
        Err(pos) => pos.saturating_sub(1),
    };
    let col = offset.saturating_sub(line_starts[line]);
    SourceLocation {
        offset,
        line,
        column: col,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_variable::VariableExpression;
    use crate::common::source_span_file_source::FileSource;
    use crate::url::SassUrl;
    use bumpalo::Bump;

    #[test]
    fn test_new_validation_ok() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "x", None);
        let span = FileSpan::new(Some(fs), 0, 1);
        let interp =
            Interpolation::new(vec![InterpolationPart::Text("x".into())], vec![None], span)
                .unwrap();
        assert!(InterpolationMap::new(interp, vec![]).is_ok());
    }

    #[test]
    fn test_new_validation_error() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "x", None);
        let span = FileSpan::new(Some(fs), 0, 1);
        let interp =
            Interpolation::new(vec![InterpolationPart::Text("x".into())], vec![None], span)
                .unwrap();
        let err = InterpolationMap::new(interp, vec![1]).unwrap_err();
        assert!(err.message.contains("0 targetOffsets"));
    }

    #[test]
    fn test_map_span_empty_contents() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "hello", None);
        let span = FileSpan::new(Some(fs), 0, 5);
        let interp = Interpolation::new(vec![], vec![], span).unwrap();
        let m = InterpolationMap::new(interp, vec![]).unwrap();

        let gen_url = SassUrl::parse("generated:///output").unwrap();
        let gen_fs = FileSource::new_in(&arena, "output", Some(gen_url));
        let gen_span = Span::File(FileSpan::new(Some(gen_fs), 2, 4));

        assert!(!m.is_mapped(&gen_span).unwrap());
        let mapped = m.map_span(&gen_span).unwrap();
        if let Span::File(fs) = mapped {
            assert_eq!(fs.start_location().offset, 0);
            assert_eq!(fs.end_location().offset, 5);
        } else {
            panic!("expected File span");
        }
    }

    #[test]
    fn test_map_span_with_expression() {
        let arena = Bump::new();
        let source = "a.#{expr}.b";
        let fs = FileSource::new_in(&arena, source, None);
        let file_span = FileSpan::new(Some(fs), 0, 11);

        let var_span = FileSpan::new(Some(fs), 4, 8); // "expr"
        let hash_span = FileSpan::new(Some(fs), 2, 9); // "#{expr}"
        let expr = Expression::Variable(VariableExpression::new("expr".into(), var_span, None));
        let interp = Interpolation::new(
            vec![
                InterpolationPart::Text("a.".into()),
                InterpolationPart::Expression(Box::new(expr)),
                InterpolationPart::Text(".b".into()),
            ],
            vec![None, Some(hash_span), None],
            file_span,
        )
        .unwrap();

        let m = InterpolationMap::new(interp, vec![2, 7]).unwrap();

        let gen_url = SassUrl::parse("generated:///output").unwrap();
        let gen_fs = FileSource::new_in(&arena, "a.VALUE.b", Some(gen_url));
        let gen_span = Span::File(FileSpan::new(Some(gen_fs), 2, 7));

        assert!(!m.is_mapped(&gen_span).unwrap());
        let mapped = m.map_span(&gen_span).unwrap();
        if let Span::File(mapped_fs) = mapped {
            assert_eq!(mapped_fs.start_location().offset, 2);
            assert_eq!(mapped_fs.end_location().offset, 9);
        } else {
            panic!("expected File span");
        }
    }

    #[test]
    fn test_is_mapped_same_file() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "source", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let interp = Interpolation::plain("source".into(), span);
        let m = InterpolationMap::new(interp, vec![]).unwrap();

        let same_fs_span = Span::File(FileSpan::new(Some(fs), 0, 1));
        assert!(m.is_mapped(&same_fs_span).unwrap());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_span_already_mapped_returns_as_is() {
        // Same `FileSource` allocation: Dart `_isMapped` (`identical`) holds,
        // so `map_span` returns the span as-is (no double-mapping).
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "source", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let interp = Interpolation::plain("source".into(), span);
        let m = InterpolationMap::new(interp, vec![]).unwrap();
        let target = Span::File(FileSpan::new(Some(fs), 1, 3));
        let mapped = m.map_span(&target).unwrap();
        match mapped {
            Span::File(fs) => {
                assert_eq!(fs.start_location().offset, 1);
                assert_eq!(fs.end_location().offset, 3);
            }
            _ => panic!("already-mapped span must round-trip unchanged"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_is_mapped_distinct_allocation_same_content() {
        // Two `FileSource` allocations with identical content (`url: None`,
        // same text) are structurally `==` but never `identical`: the target
        // lives in generated-output space and must still be mapped.
        let arena = Bump::new();
        let fs_a = FileSource::new_in(&arena, "source", None);
        let span = FileSpan::new(Some(fs_a), 0, 6);
        let interp = Interpolation::plain("source".into(), span);
        let m = InterpolationMap::new(interp, vec![]).unwrap();

        let fs_b = FileSource::new_in(&arena, "source", None);
        assert_eq!(Some(fs_a), Some(fs_b), "setup: allocations compare equal");
        let target = Span::File(FileSpan::new(Some(fs_b), 1, 3));
        assert!(
            !m.is_mapped(&target).unwrap(),
            "distinct allocation must not count as mapped"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_span_distinct_allocation_maps() {
        // Empty-contents interpolation in allocation A; target in a separate
        // allocation B with identical content. Correct behavior (Dart
        // `identical` guard): the span runs through `map_location` to the
        // interpolation span `0..5` in A, not returned as-is (`2..4` in B).
        let arena = Bump::new();
        let fs_a = FileSource::new_in(&arena, "hello", None);
        let span = FileSpan::new(Some(fs_a), 0, 5);
        let interp = Interpolation::new(vec![], vec![], span).unwrap();
        let m = InterpolationMap::new(interp, vec![]).unwrap();

        let fs_b = FileSource::new_in(&arena, "hello", None);
        let target = Span::File(FileSpan::new(Some(fs_b), 2, 4));
        let mapped = m.map_span(&target).unwrap();
        match mapped {
            Span::File(mapped_fs) => {
                assert_eq!(mapped_fs.start_location().offset, 0);
                assert_eq!(mapped_fs.end_location().offset, 5);
                assert!(
                    matches!(mapped_fs.file(), Some(f) if std::ptr::eq(f, fs_a)),
                    "mapped span must live in the interpolation's allocation"
                );
            }
            _ => panic!("expected File span"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_exception_none_url_maps_to_multispan() {
        // Error snapshot with `source_url: None` against an interpolation
        // whose span URL is also `None`: a URL comparison (`None == None`)
        // would wrongly treat every such error as already-mapped. Correct
        // behavior maps generated offsets `2..7` to `2..9` (`#{expr}`) and,
        // since the range covers an Expression, yields a MultiSpan with the
        // "error in interpolated output" secondary.
        let arena = Bump::new();
        let source = "a.#{expr}.b";
        let fs_a = FileSource::new_in(&arena, source, None);
        let file_span = FileSpan::new(Some(fs_a), 0, 11);

        let var_span = FileSpan::new(Some(fs_a), 4, 8); // "expr"
        let hash_span = FileSpan::new(Some(fs_a), 2, 9); // "#{expr}"
        let expr = Expression::Variable(VariableExpression::new("expr".into(), var_span, None));
        let interp = Interpolation::new(
            vec![
                InterpolationPart::Text("a.".into()),
                InterpolationPart::Expression(Box::new(expr)),
                InterpolationPart::Text(".b".into()),
            ],
            vec![None, Some(hash_span), None],
            file_span,
        )
        .unwrap();
        let m = InterpolationMap::new(interp, vec![2, 7]).unwrap();

        let gen_fs = FileSource::new_in(&arena, "a.VALUE.b", None);
        let ctx =
            SourceSpanWithContext::from_file_span(&FileSpan::new(Some(gen_fs), 2, 7)).unwrap();
        assert_eq!(ctx.source_url, None);
        let err = SassError::Format {
            message: "boom".into(),
            span: ctx,
            original_source: None,
            cause: None,
            loaded_urls: vec![],
        };
        let out = m.map_exception(&err);
        match out {
            SassError::MultiSpan {
                span, secondary, ..
            } => {
                assert_eq!(span.start.offset, 2);
                assert_eq!(span.end.offset, 9);
                assert_eq!(secondary.len(), 1);
                assert_eq!(secondary[0].1, "error in interpolated output");
            }
            other => panic!("unmapped error must become MultiSpan, got {other:?}"),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn test_map_exception_empty_interpolation_remaps() {
        // Truly empty interpolation (`contents: vec![]`): the owned
        // `SourceSpanWithContext` snapshot carries no `FileSource` ref, so
        // file identity is unrecoverable here — even a genuinely-mapped
        // error at `1..3` remaps to the interpolation span `0..6` rather
        // than returning unchanged (Dart checks `_isMapped` on this branch,
        // which needs the live `SourceFile`).
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "source", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let interp = Interpolation::new(vec![], vec![], span).unwrap();
        let m = InterpolationMap::new(interp, vec![]).unwrap();
        let ctx = SourceSpanWithContext::from_file_span(&FileSpan::new(Some(fs), 1, 3)).unwrap();
        let err = SassError::Format {
            message: "boom".into(),
            span: ctx,
            original_source: None,
            cause: None,
            loaded_urls: vec![],
        };
        let out = m.map_exception(&err);
        match out {
            SassError::Format { span, .. } => {
                assert_eq!(span.start.offset, 0);
                assert_eq!(span.end.offset, 6);
            }
            other => panic!("expected remapped Format, got {other:?}"),
        }
    }
}
