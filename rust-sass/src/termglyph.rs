// Copyright (c) 2017, the Dart project authors.  Please see the AUTHORS file
// for details. All rights reserved. Use of this source code is governed by a
// BSD-style license that can be found in the LICENSE file.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: (external) package:term_glyph (term_glyph.dart + lib/src/generated/glyph_set.dart, v1.2.2 per docs/upstream.md)
// go-source: go/termglyph/termglyph.go

//! Box-drawing glyphs in both ASCII and Unicode.
//!
//! Ports the subset of Dart's `package:term_glyph` this compiler uses: the
//! `GlyphSet` selector plus the box-drawing getters needed by the span
//! highlighter. Dart provides this as a class so individual chunks of code
//! can choose between `asciiGlyphs` and `unicodeGlyphs`; the global
//! `glyphs`/`ascii` switch and the getters nothing references (`bullet`,
//! arrows, corners, tees) are omitted (rule 4).
//!
//! Glyph values verified against `term_glyph` 1.2.2
//! (`unicode_glyph_set.dart` / `ascii_glyph_set.dart`).
//!
//! Mirrors Dart's `package:term_glyph`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GlyphSet {
    #[default]
    /// Unicode box-drawing glyphs (Dart's `unicodeGlyphs`).
    Unicode,
    /// Plain-ASCII fallbacks (Dart's `asciiGlyphs`).
    Ascii,
}

impl GlyphSet {
    /// A vertical line that can be used to draw a box.
    pub fn vertical_line(&self) -> &'static str {
        match self {
            GlyphSet::Unicode => "│",
            GlyphSet::Ascii => "|",
        }
    }

    /// A horizontal line that can be used to draw a box.
    pub fn horizontal_line(&self) -> &'static str {
        match self {
            GlyphSet::Unicode => "─",
            GlyphSet::Ascii => "-",
        }
    }

    /// The bottom half of a vertical box line.
    pub fn down_end(&self) -> &'static str {
        match self {
            GlyphSet::Unicode => "╷",
            GlyphSet::Ascii => ",",
        }
    }

    /// The top half of a vertical box line.
    pub fn up_end(&self) -> &'static str {
        match self {
            GlyphSet::Unicode => "╵",
            GlyphSet::Ascii => "'",
        }
    }

    /// The upper left-hand corner of a box.
    pub fn top_left_corner(&self) -> &'static str {
        match self {
            GlyphSet::Unicode => "┌",
            GlyphSet::Ascii => ",",
        }
    }

    /// The lower left-hand corner of a box.
    pub fn bottom_left_corner(&self) -> &'static str {
        match self {
            GlyphSet::Unicode => "└",
            GlyphSet::Ascii => "'",
        }
    }

    /// An intersection of vertical and horizontal box lines.
    pub fn cross(&self) -> &'static str {
        match self {
            GlyphSet::Unicode => "┼",
            GlyphSet::Ascii => "+",
        }
    }

    /// A bold horizontal line that can be used to draw a box.
    pub fn horizontal_line_bold(&self) -> &'static str {
        match self {
            GlyphSet::Unicode => "━",
            GlyphSet::Ascii => "=",
        }
    }

    /// Returns `unicode` if `self` supports Unicode glyphs and `ascii`
    /// otherwise (Dart's `GlyphSet.glyphOrAscii`).
    pub fn glyph_or_ascii<'parse>(&self, unicode: &'parse str, ascii: &'parse str) -> &'parse str {
        match self {
            GlyphSet::Unicode => unicode,
            GlyphSet::Ascii => ascii,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unicode_vertical() {
        assert_eq!(GlyphSet::Unicode.vertical_line(), "│");
    }

    #[test]
    fn test_ascii_vertical() {
        assert_eq!(GlyphSet::Ascii.vertical_line(), "|");
    }

    #[test]
    fn test_unicode_horizontal() {
        assert_eq!(GlyphSet::Unicode.horizontal_line(), "─");
    }

    #[test]
    fn test_ascii_horizontal() {
        assert_eq!(GlyphSet::Ascii.horizontal_line(), "-");
    }

    #[test]
    fn test_unicode_corner() {
        assert_eq!(GlyphSet::Unicode.top_left_corner(), "┌");
    }

    #[test]
    fn test_ascii_corner() {
        assert_eq!(GlyphSet::Ascii.top_left_corner(), ",");
    }

    #[test]
    fn test_glyph_or_ascii_unicode() {
        assert_eq!(GlyphSet::Unicode.glyph_or_ascii("┌", "/"), "┌");
    }

    #[test]
    fn test_glyph_or_ascii_ascii() {
        assert_eq!(GlyphSet::Ascii.glyph_or_ascii("┌", "/"), "/");
    }

    #[test]
    fn test_default_is_unicode() {
        assert_eq!(GlyphSet::default(), GlyphSet::Unicode);
    }
}
