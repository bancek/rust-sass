// Copyright (c) 2014, the Dart project authors.  Please see the AUTHORS file
// for details. All rights reserved. Use of this source code is governed by a
// BSD-style license that can be found in the LICENSE file.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: (external) package:source_span/lib/src/highlighter.dart
// go-source: go/sasscommon/source_span_highlighter.go

use std::collections::HashMap;
use std::fmt::Write;

use crate::common::exception::SassResult;
use crate::common::pretty_uri::pretty_uri;
use crate::common::source_span_span_with_context::find_line_start;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::io::Io;
use crate::termglyph::GlyphSet;

const ANSI_RED: &str = "\x1b[31m";
const ANSI_BLUE: &str = "\x1b[34m";
const ANSI_RESET: &str = "\x1b[0m";
/// The number of spaces rendered for each hard tab in span text.
///
/// Tabs would break caret alignment, so they are expanded (Dart
/// `Highlighter._spacesPerTab` in `highlighter.dart`).
const SPACES_PER_TAB: usize = 4;

/// A span plus its display role within one highlight pass.
///
/// Matches Dart: `_Highlight` (`highlighter.dart`) — the section of the
/// source to highlight, whether it is the primary span (drawn with `^` and
/// the primary color), and the label written inline next to it.
#[derive(Debug, Clone, PartialEq)]
struct Highlight {
    span: SourceSpanWithContext,
    is_primary: bool,
    label: Option<String>,
}

/// One rendered source line with the highlights that cover it.
///
/// Matches Dart: `_Line` (`highlighter.dart`) — the line text without its
/// trailing newline, the 0-based line number, the display URL of the file it
/// came from, and the highlights covering it.
#[derive(Debug, Clone)]
struct Line {
    text: String,
    number: usize,
    url: String,
    highlights: Vec<Highlight>,
}

/// How the highlighted text is colored.
///
/// Matches Dart: the `color` argument of the `Highlighter` constructors in
/// `highlighter.dart` — a string selects an ANSI escape, `true` selects the
/// default color, `false`/`null` disables color. Rust shift: the `bool`
/// cases become [`HighlightColor::Default`]/[`HighlightColor::None`], and
/// the multi-span constructor's separate primary/secondary colors travel on
/// [`HighlightOptions::primary_color`]/`secondary_color` instead.
#[derive(Debug, Clone)]
pub enum HighlightColor {
    None,
    Default,
    Custom(String),
}

/// Display options for [`Highlighter`].
///
/// Bundles Dart's per-constructor `color`/`primaryColor`/`secondaryColor`
/// arguments with the [`GlyphSet`] (Dart reads `glyph.ascii` globally; Rust
/// threads it explicitly) and the [`Io`] handle used by `pretty_uri` when
/// grouping lines by file.
pub struct HighlightOptions {
    pub color: HighlightColor,
    pub primary_color: String,
    pub secondary_color: String,
    pub glyphs: GlyphSet,
}

impl Default for HighlightOptions {
    fn default() -> Self {
        HighlightOptions {
            color: HighlightColor::None,
            primary_color: String::new(),
            secondary_color: String::new(),
            glyphs: GlyphSet::default(),
        }
    }
}

/// Renders a chunk of source text with one or more spans highlighted.
///
/// Matches Dart: `Highlighter` (`package:source_span/lib/src/highlighter.dart`).
/// Construct with [`Highlighter::new`] (single span) or
/// [`Highlighter::new_multiple`] (primary plus labeled secondary spans),
/// then call [`Highlighter::highlight`] for the rendered string. Rust shift:
/// `color` travels on [`HighlightOptions`] instead of constructor arguments,
/// and rendering takes `&mut self` into an internal buffer.
pub struct Highlighter {
    lines: Vec<Line>,
    primary_color: Option<String>,
    secondary_color: Option<String>,
    padding_before_sidebar: usize,
    max_multiline_spans: usize,
    multiple_files: bool,
    buf: String,
    glyphs: GlyphSet,
}

impl Highlighter {
    /// Creates a highlighter for a single `span`.
    ///
    /// Matches Dart: `Highlighter(span, {color})` (`highlighter.dart`) — a
    /// string `color` is an ANSI escape, `Default` is the default red,
    /// `None` disables color.
    pub fn new(
        span: &SourceSpanWithContext,
        opts: &HighlightOptions,
        io: &dyn Io,
    ) -> SassResult<Self> {
        let primary_color = match &opts.color {
            HighlightColor::None => None,
            HighlightColor::Default => Some(ANSI_RED.to_string()),
            HighlightColor::Custom(s) => Some(s.clone()),
        };
        let normalized = normalize_span(span)?;
        let lines = collate_lines(
            &[Highlight {
                span: normalized,
                is_primary: true,
                label: None,
            }],
            io,
        )?;

        Self::build(lines, primary_color, None, opts)
    }

    /// Creates a highlighter for `primary` plus labeled secondary spans.
    ///
    /// Matches Dart: `Highlighter.multiple` (`highlighter.dart`) — the
    /// primary span renders with `primary_color` (default red) and secondary
    /// spans with `secondary_color` (default blue); both are ignored unless
    /// `color` selects coloring.
    pub fn new_multiple(
        primary: &SourceSpanWithContext,
        primary_label: &str,
        secondary: &[(SourceSpanWithContext, String)],
        opts: &HighlightOptions,
        io: &dyn Io,
    ) -> SassResult<Self> {
        let (primary_color, secondary_color) = match &opts.color {
            HighlightColor::None => (None, None),
            HighlightColor::Default => (
                if opts.primary_color.is_empty() {
                    Some(ANSI_RED.to_string())
                } else {
                    Some(opts.primary_color.clone())
                },
                if opts.secondary_color.is_empty() {
                    Some(ANSI_BLUE.to_string())
                } else {
                    Some(opts.secondary_color.clone())
                },
            ),
            HighlightColor::Custom(s) => (Some(s.clone()), None),
        };
        let primary_hl = Highlight {
            span: normalize_span(primary)?,
            is_primary: true,
            label: Some(primary_label.replace("\r\n", "\n")),
        };
        let mut all: Vec<Highlight> = vec![primary_hl];
        for (span, label) in secondary {
            all.push(Highlight {
                span: normalize_span(span)?,
                is_primary: false,
                label: Some(label.replace("\r\n", "\n")),
            });
        }
        let lines = collate_lines(&all, io)?;

        Self::build(lines, primary_color, secondary_color, opts)
    }

    fn build(
        lines: Vec<Line>,
        primary_color: Option<String>,
        secondary_color: Option<String>,
        opts: &HighlightOptions,
    ) -> SassResult<Self> {
        if lines.is_empty() {
            return Ok(Highlighter {
                lines: vec![],
                primary_color,
                secondary_color,
                padding_before_sidebar: 1,
                max_multiline_spans: 0,
                multiple_files: false,
                buf: String::new(),
                glyphs: opts.glyphs,
            });
        }
        let max_line_num = lines.last().unwrap().number + 1;
        let line_digits = max_line_num.to_string().len();
        let contig = contiguous(&lines);
        let mut pad = 1 + line_digits;
        if !contig && pad < 4 {
            pad = 4;
        }
        let mut max_multi = 0;
        for line in &lines {
            let count = line
                .highlights
                .iter()
                .filter(|hl| is_multiline(&hl.span))
                .count();
            if count > max_multi {
                max_multi = count;
            }
        }
        let multiple_files = if lines.len() < 2 {
            false
        } else {
            let first = &lines[0].url;
            lines[1..].iter().any(|l| l.url != *first)
        };
        Ok(Highlighter {
            lines,
            primary_color,
            secondary_color,
            padding_before_sidebar: pad,
            max_multiline_spans: max_multi,
            multiple_files,
            buf: String::new(),
            glyphs: opts.glyphs,
        })
    }

    /// Returns the highlighted span text.
    ///
    /// Matches Dart: `Highlighter.highlight()` (`highlighter.dart`).
    pub fn highlight(&mut self) -> SassResult<String> {
        if self.lines.is_empty() {
            return Ok(String::new());
        }

        let first_url = self.lines[0].url.clone();
        self.write_file_start(&first_url);

        let mut highlights_by_column: Vec<Option<Highlight>> = vec![None; self.max_multiline_spans];

        for i in 0..self.lines.len() {
            if i > 0 {
                let last_url = self.lines[i - 1].url.clone();
                let cur_url = self.lines[i].url.clone();
                if last_url != cur_url {
                    self.write_sidebar_end(self.glyphs.up_end());
                    self.buf.push('\n');
                    self.write_file_start(&cur_url);
                } else if self.lines[i - 1].number + 1 != self.lines[i].number {
                    self.write_sidebar_text("...");
                    self.buf.push('\n');
                }
            }

            let line = self.lines[i].clone();
            {
                let highlights = &line.highlights;
                for hl in highlights.iter().rev() {
                    if is_multiline(&hl.span)
                        && hl.span.start.line == line.number
                        && is_only_whitespace(
                            &line.text[..char_boundary_at_or_before(
                                &line.text,
                                hl.span.start.column.min(line.text.len()),
                            )],
                        )
                    {
                        replace_first_null(&mut highlights_by_column, hl.clone());
                    }
                }
            }

            self.write_sidebar_line(line.number + 1);
            self.buf.push(' ');
            self.write_multiline_highlights(&line.text, line.number, &highlights_by_column, None);
            if self.max_multiline_spans > 0 {
                self.buf.push(' ');
            }

            let primary = line.highlights.iter().find(|hl| hl.is_primary);

            let primary_color = self.primary_color.clone();
            if let Some(primary) = primary {
                let start_col = if primary.span.start.line == line.number {
                    primary.span.start.column
                } else {
                    0
                };
                let end_col = if primary.span.end.line == line.number {
                    primary.span.end.column
                } else {
                    // Char (not byte) length: `end_col` feeds
                    // `write_highlighted_text`, whose slicing is char-based
                    // like Dart's `substring`.
                    line.text.chars().count()
                };
                self.write_highlighted_text(
                    &line.text,
                    start_col,
                    end_col,
                    primary_color.as_deref(),
                );
            } else {
                self.write_text(&line.text);
            }
            self.buf.push('\n');

            if let Some(primary) = line.highlights.iter().find(|hl| hl.is_primary) {
                let primary = primary.clone();
                let pc = self.primary_color.clone();
                let sc = self.secondary_color.clone();
                self.write_indicator(
                    &line.text,
                    line.number,
                    &primary,
                    &line.highlights,
                    &mut highlights_by_column,
                    pc.as_deref(),
                    sc.as_deref(),
                )?;
            }
            for hl in &line.highlights {
                if hl.is_primary {
                    continue;
                }
                let hl = hl.clone();
                let pc = self.primary_color.clone();
                let sc = self.secondary_color.clone();
                self.write_indicator(
                    &line.text,
                    line.number,
                    &hl,
                    &line.highlights,
                    &mut highlights_by_column,
                    pc.as_deref(),
                    sc.as_deref(),
                )?;
            }
        }

        self.write_sidebar_end(self.glyphs.up_end());
        Ok(std::mem::take(&mut self.buf))
    }

    fn write_file_start(&mut self, url: &str) {
        // `\x00`-prefixed urls are synthetic keys for sources without a URL;
        // they must render like an empty URL (plain sidebar, no `┌──>` header).
        if !self.multiple_files || url.is_empty() || url.starts_with('\x00') {
            self.write_sidebar_end(self.glyphs.down_end());
        } else {
            self.write_sidebar_end(self.glyphs.top_left_corner());
            let blue = ANSI_BLUE;
            let hl = self.glyphs.horizontal_line();
            self.colorize(
                |h| {
                    for _ in 0..2 {
                        h.buf.push_str(hl);
                    }
                    h.buf.push('>');
                },
                Some(blue),
            );
            write!(self.buf, " {url}").unwrap();
        }
        self.buf.push('\n');
    }

    fn write_text(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\t' {
                for _ in 0..SPACES_PER_TAB {
                    self.buf.push(' ');
                }
            } else {
                self.buf.push(ch);
            }
        }
    }

    fn write_sidebar(&mut self) {
        self.write_sidebar_internal(None, "", "");
    }

    fn write_sidebar_line(&mut self, line_no: usize) {
        self.write_sidebar_internal(Some(line_no), "", "");
    }

    fn write_sidebar_text(&mut self, text: &str) {
        self.write_sidebar_internal(None, text, "");
    }

    fn write_sidebar_end(&mut self, end: &str) {
        self.write_sidebar_internal(None, "", end);
    }

    fn write_sidebar_internal(&mut self, line: Option<usize>, text: &str, end: &str) {
        let text = if let Some(n) = line {
            n.to_string()
        } else {
            text.to_string()
        };
        let end = if end.is_empty() {
            self.glyphs.vertical_line()
        } else {
            end
        };
        let blue = ANSI_BLUE;
        let pad = self.padding_before_sidebar;
        self.colorize(
            |h| {
                write!(h.buf, "{text:<pad$}{end}").unwrap();
            },
            Some(blue),
        );
    }

    fn colorize<F, T>(&mut self, f: F, color: Option<&str>)
    where
        F: FnOnce(&mut Self) -> T,
    {
        let do_color = self.primary_color.is_some() && color.is_some();
        if do_color {
            self.buf.push_str(color.unwrap());
        }
        f(self);
        if do_color {
            self.buf.push_str(ANSI_RESET);
        }
    }

    fn colorize_int<F>(&mut self, f: F, color: Option<&str>) -> usize
    where
        F: FnOnce(&mut Self) -> usize,
    {
        let do_color = self.primary_color.is_some() && color.is_some();
        if do_color {
            self.buf.push_str(color.unwrap());
        }
        let result = f(self);
        if do_color {
            self.buf.push_str(ANSI_RESET);
        }
        result
    }

    fn write_multiline_highlights(
        &mut self,
        line_text: &str,
        line_number: usize,
        columns: &[Option<Highlight>],
        current: Option<&Highlight>,
    ) {
        let mut opened_on_this_line = false;
        let mut opened_color: Option<String> = None;
        let primary_color = self.primary_color.clone();
        let secondary_color = self.secondary_color.clone();
        let glyphs = self.glyphs;

        let current_color = current.and_then(|c| {
            if c.is_primary {
                primary_color.clone()
            } else {
                secondary_color.clone()
            }
        });

        for col_hl in columns {
            let start_line = col_hl.as_ref().map(|h| h.span.start.line);
            let end_line = col_hl.as_ref().map(|h| h.span.end.line);

            let is_current = current.is_some() && col_hl.as_ref() == current;
            let hl_color = col_hl.as_ref().and_then(|hl| {
                if hl.is_primary {
                    primary_color.clone()
                } else {
                    secondary_color.clone()
                }
            });

            if is_current {
                let glyph = if start_line == Some(line_number) {
                    glyphs.top_left_corner()
                } else {
                    glyphs.bottom_left_corner()
                };
                self.colorize(|h| h.buf.push_str(glyph), current_color.as_deref());
            } else if col_hl.is_none() {
                if opened_on_this_line {
                    let hline = glyphs.horizontal_line();
                    self.colorize(|h| h.buf.push_str(hline), opened_color.as_deref());
                } else {
                    self.buf.push(' ');
                }
            } else {
                let hl = col_hl.as_ref().unwrap();
                let vertical = if opened_on_this_line {
                    glyphs.cross()
                } else {
                    glyphs.vertical_line()
                };
                if current.is_some() {
                    self.buf.push_str(vertical);
                } else if start_line == Some(line_number) {
                    let glyph = if opened_on_this_line {
                        glyphs.glyph_or_ascii("┬", "/")
                    } else {
                        glyphs.glyph_or_ascii("┌", "/")
                    };
                    self.colorize(|h| h.buf.push_str(glyph), opened_color.as_deref());
                    opened_on_this_line = true;
                    if opened_color.is_none() {
                        opened_color = hl_color.clone();
                    }
                } else if end_line == Some(line_number)
                    && hl.span.end.column == line_text.chars().count()
                {
                    let glyph = if hl.label.is_none() {
                        glyphs.glyph_or_ascii("└", "\\")
                    } else {
                        vertical
                    };
                    self.buf.push_str(glyph);
                } else {
                    self.colorize(|h| h.buf.push_str(vertical), opened_color.as_deref());
                }
            }
        }
    }

    fn write_highlighted_text(
        &mut self,
        text: &str,
        start_column: usize,
        end_column: usize,
        color: Option<&str>,
    ) {
        // Columns are char-based (Dart `SourceFile.span` columns count UTF-16
        // units; Rust counts chars — identical for BMP text); byte-slicing
        // with them panics mid-codepoint on non-ASCII lines. Snap to char
        // boundaries instead.
        let start = char_boundary_at_or_before(text, start_column.min(text.len()));
        let end = char_boundary_at_or_before(text, end_column.min(text.len()));
        let start = start.min(end);
        self.write_text(&text[..start]);
        self.colorize(|h| h.write_text(&text[start..end]), color);
        self.write_text(&text[end..]);
    }

    // Upstream-highlighter threading (Dart `highlighter.dart` shape);
    // packing into a params struct would diverge from the port.
    #[allow(clippy::too_many_arguments)]
    fn write_indicator(
        &mut self,
        line_text: &str,
        line_number: usize,
        highlight: &Highlight,
        _all_highlights: &[Highlight],
        columns: &mut [Option<Highlight>],
        primary_color: Option<&str>,
        secondary_color: Option<&str>,
    ) -> SassResult<()> {
        let color = if highlight.is_primary {
            primary_color
        } else {
            secondary_color
        };

        if !is_multiline(&highlight.span) {
            self.write_sidebar();
            self.buf.push(' ');
            let cols_snapshot = columns.to_vec();
            self.write_multiline_highlights(
                line_text,
                line_number,
                &cols_snapshot,
                Some(highlight),
            );
            if self.max_multiline_spans > 0 {
                self.buf.push(' ');
            }
            let ul_start = highlight.span.start.column;
            let ul_end = highlight.span.end.column;
            let is_primary = highlight.is_primary;
            let glyphs = self.glyphs;
            let underline_len = self.colorize_int(
                |h| {
                    let before = h.buf.len();
                    let chr = if is_primary {
                        "^"
                    } else {
                        glyphs.horizontal_line_bold()
                    };
                    h.write_underline(line_text, ul_start, ul_end, chr);
                    h.buf.len() - before
                },
                color,
            );
            self.write_label(
                highlight,
                columns,
                underline_len,
                primary_color,
                secondary_color,
            );
            return Ok(());
        }

        let start_loc = highlight.span.start;
        let end_loc = highlight.span.end;

        if start_loc.line == line_number {
            if highlight_in_column(columns, highlight) {
                return Ok(());
            }
            replace_first_null(columns, highlight.clone());
            self.write_sidebar();
            self.buf.push(' ');
            let cols_snapshot = columns.to_vec();
            self.write_multiline_highlights(
                line_text,
                line_number,
                &cols_snapshot,
                Some(highlight),
            );
            self.colorize(|h| h.write_arrow(line_text, start_loc.column, true), color);
            self.buf.push('\n');
            return Ok(());
        }

        if end_loc.line == line_number {
            let covers_whole_line = end_loc.column == line_text.chars().count();
            if covers_whole_line && highlight.label.is_none() {
                replace_with_null(columns, highlight);
                return Ok(());
            }
            self.write_sidebar();
            self.buf.push(' ');
            let cols_snapshot = columns.to_vec();
            self.write_multiline_highlights(
                line_text,
                line_number,
                &cols_snapshot,
                Some(highlight),
            );
            let glyphs = self.glyphs;
            let underline_len = self.colorize_int(
                |h| {
                    let before = h.buf.len();
                    if covers_whole_line {
                        for _ in 0..3 {
                            h.buf.push_str(glyphs.horizontal_line());
                        }
                    } else {
                        let col = if end_loc.column == 0 {
                            0
                        } else {
                            end_loc.column - 1
                        };
                        h.write_arrow(line_text, col, false);
                    }
                    h.buf.len() - before
                },
                color,
            );
            self.write_label(
                highlight,
                columns,
                underline_len,
                primary_color,
                secondary_color,
            );
            replace_with_null(columns, highlight);
        }

        Ok(())
    }

    fn write_underline(
        &mut self,
        line_text: &str,
        start_column: usize,
        end_column: usize,
        character: &str,
    ) {
        // Columns are char-based (Dart `substring` indices are UTF-16 units);
        // convert to char counts before tab-adjustment so multibyte text
        // yields the right caret width (byte counts over-count).
        let start_chars = line_text.chars().count().min(start_column);
        let end_chars = line_text.chars().count().min(end_column);
        let (start_chars, end_chars) = (
            start_chars.min(end_chars),
            end_chars.max(start_chars).max(start_chars),
        );
        let start_byte = char_boundary_at_or_before(line_text, start_column.min(line_text.len()));
        let end_byte = char_boundary_at_or_before(line_text, end_column.min(line_text.len()));
        let (start_byte, end_byte) = (
            start_byte.min(end_byte),
            end_byte.max(start_byte).max(start_byte),
        );
        let tabs_before = count_tabs(&line_text[..start_byte]);
        let tabs_inside = count_tabs(&line_text[start_byte..end_byte]);
        let start_col = start_chars + tabs_before * (SPACES_PER_TAB - 1);
        let end_col = end_chars + (tabs_before + tabs_inside) * (SPACES_PER_TAB - 1);
        let width = (end_col - start_col).max(1);
        for _ in 0..start_col {
            self.buf.push(' ');
        }
        for _ in 0..width {
            self.buf.push_str(character);
        }
    }

    fn write_arrow(&mut self, line_text: &str, column: usize, beginning: bool) {
        let col = char_boundary_at_or_before(line_text, column.min(line_text.len()));
        let tab_count = if beginning {
            count_tabs(&line_text[..col])
        } else {
            let end = if col < line_text.len() { col + 1 } else { col };
            count_tabs(&line_text[..end])
        };
        let total = 1 + col + tab_count * (SPACES_PER_TAB - 1);
        let hl = self.glyphs.horizontal_line();
        for _ in 0..total {
            self.buf.push_str(hl);
        }
        self.buf.push('^');
    }

    fn write_label(
        &mut self,
        highlight: &Highlight,
        columns: &[Option<Highlight>],
        underline_length: usize,
        primary_color: Option<&str>,
        secondary_color: Option<&str>,
    ) {
        let label = match &highlight.label {
            Some(l) => l,
            None => {
                self.buf.push('\n');
                return;
            }
        };
        let label_lines: Vec<&str> = label.split('\n').collect();
        let color = if highlight.is_primary {
            primary_color
        } else {
            secondary_color
        };
        self.colorize(
            |h| {
                write!(h.buf, " {}", label_lines[0]).unwrap();
            },
            color,
        );
        self.buf.push('\n');

        for text in &label_lines[1..] {
            self.write_sidebar();
            self.buf.push(' ');
            for col_hl in columns {
                if col_hl.is_none() || col_hl.as_ref() == Some(highlight) {
                    self.buf.push(' ');
                } else {
                    self.buf.push_str(self.glyphs.vertical_line());
                }
            }
            for _ in 0..underline_length {
                self.buf.push(' ');
            }
            self.colorize(
                |h| {
                    write!(h.buf, " {text}").unwrap();
                },
                color,
            );
            self.buf.push('\n');
        }
    }
}

// --- Normalization ---
//
// Matches Dart: `_Highlight._normalize*` (`highlighter.dart`) — spans are
// normalized before collation so the renderer only handles canonical shapes
// (context containing the text, `\n` newlines, no redundant trailing
// newline, end-of-line spans pulled back onto the last line).

/// Normalizes `span` for highlighting (context, newlines, trailing newline,
/// end-of-line in that order).
fn normalize_span(span: &SourceSpanWithContext) -> SassResult<SourceSpanWithContext> {
    let s = normalize_context(span)?;
    let s = normalize_newlines(&s)?;
    let s = normalize_trailing_newline(&s)?;
    normalize_end_of_line(&s)
}

/// Rebuilds the context when it does not contain the span text.
///
/// Matches Dart: `_normalizeContext` (`highlighter.dart`) — falls back to
/// using the span text as its own context with recomputed end line/column.
fn normalize_context(span: &SourceSpanWithContext) -> SassResult<SourceSpanWithContext> {
    let text = &span.text;
    if find_line_start(&span.context, text, span.start.column).is_some() {
        return Ok(span.clone());
    }
    SourceSpanWithContext::new(
        SourceLocation {
            offset: span.start.offset,
            line: 0,
            column: 0,
        },
        SourceLocation {
            offset: span.end.offset,
            line: text.chars().filter(|&c| c == '\n').count(),
            column: last_line_length(text),
        },
        text.clone(),
        text.clone(),
        span.source_url.clone(),
    )
}

/// Rewrites `\r\n` to `\n` in the span text and context.
///
/// Matches Dart: `_normalizeNewlines` (`highlighter.dart`) — end offsets
/// shift left by one per converted `\r\n` pair.
fn normalize_newlines(span: &SourceSpanWithContext) -> SassResult<SourceSpanWithContext> {
    let text = &span.text;
    if !text.contains("\r\n") {
        return Ok(span.clone());
    }
    let mut end_offset = span.end.offset;
    let text_bytes = text.as_bytes();
    for i in 0..text_bytes.len().saturating_sub(1) {
        if text_bytes[i] == b'\r' && text_bytes[i + 1] == b'\n' {
            end_offset -= 1;
        }
    }
    let context = span.context.clone();
    let new_context = context.replace("\r\n", "\n");
    let new_text = text.replace("\r\n", "\n");
    SourceSpanWithContext::new(
        span.start,
        SourceLocation {
            offset: end_offset,
            line: span.end.line,
            column: span.end.column,
        },
        new_text,
        new_context,
        span.source_url.clone(),
    )
}

/// Drops a redundant trailing newline from the context (and the span text
/// when the text runs to the end of the context).
///
/// Matches Dart: `_normalizeTrailingNewline` (`highlighter.dart`).
fn normalize_trailing_newline(span: &SourceSpanWithContext) -> SassResult<SourceSpanWithContext> {
    let context = &span.context;
    if !context.ends_with('\n') {
        return Ok(span.clone());
    }
    if span.text.ends_with("\n\n") {
        return Ok(span.clone());
    }
    let new_context: String = context[..context.len() - 1].to_string();
    let mut new_text = span.text.clone();
    let mut new_start = span.start;
    let mut new_end = span.end;

    if span.text.ends_with('\n') && is_text_at_end_of_context(span) {
        new_text = span.text[..span.text.len() - 1].to_string();
        if new_text.is_empty() {
            new_end = new_start;
        } else {
            new_end = SourceLocation {
                offset: span.end.offset - 1,
                line: span.end.line.saturating_sub(1),
                column: last_line_length(&new_context),
            };
            if span.start.offset == span.end.offset {
                new_start = new_end;
            }
        }
    }
    SourceSpanWithContext::new(
        new_start,
        new_end,
        new_text,
        new_context,
        span.source_url.clone(),
    )
}

/// Pulls an end-of-line span end back onto the last text line.
///
/// Matches Dart: `_normalizeEndOfLine` (`highlighter.dart`) — a span ending
/// at column 0 of a later line ends at the last column of the previous line
/// instead (no-op for single-line spans).
fn normalize_end_of_line(span: &SourceSpanWithContext) -> SassResult<SourceSpanWithContext> {
    if span.end.column != 0 {
        return Ok(span.clone());
    }
    if span.end.line == span.start.line {
        return Ok(span.clone());
    }
    let text = &span.text[..span.text.len().saturating_sub(1)];
    let new_context = if span.context.ends_with('\n') {
        span.context[..span.context.len() - 1].to_string()
    } else {
        span.context.clone()
    };
    SourceSpanWithContext::new(
        span.start,
        SourceLocation {
            offset: span.end.offset.saturating_sub(1),
            line: span.end.line.saturating_sub(1),
            column: last_line_length(text),
        },
        text.to_string(),
        new_context,
        span.source_url.clone(),
    )
}

// --- Collation ---
//
// Matches Dart: `Highlighter._collateLines` (`highlighter.dart`) — collects
// the source lines from every highlight's context and associates each line
// with the highlights covering it. Rust shift: spans without a URL get
// `\x00`-prefixed synthetic group keys (Dart uses opaque `Object()` keys).

/// Collects the source lines covering `highlights`, grouped by file with the
/// primary span's file first.
fn collate_lines(highlights: &[Highlight], io: &dyn Io) -> SassResult<Vec<Line>> {
    // Group highlights by URL. The primary's file group renders first, then the
    // rest in insertion order (Dart renders the primary span's file first); the
    // HashMap is only used for lookup so the order is deterministic.
    let mut group_of: HashMap<String, usize> = HashMap::new();
    let mut groups: Vec<Vec<&Highlight>> = Vec::new();
    let mut group_urls: Vec<String> = Vec::new();
    let mut primary_group = 0usize;
    let mut no_url_idx: usize = 0;
    for (i, hl) in highlights.iter().enumerate() {
        let url = match &hl.span.source_url {
            Some(u) => pretty_uri(u, io),
            None => {
                let key = format!("\x00{no_url_idx}");
                no_url_idx += 1;
                key
            }
        };
        let idx = match group_of.get(&url) {
            Some(&i) => i,
            None => {
                let i = groups.len();
                groups.push(Vec::new());
                group_urls.push(url.clone());
                group_of.insert(url.clone(), i);
                i
            }
        };
        if i == 0 {
            primary_group = idx;
        }
        groups[idx].push(hl);
    }

    if primary_group != 0 && !groups.is_empty() {
        let group = groups.remove(primary_group);
        let url = group_urls.remove(primary_group);
        groups.insert(0, group);
        group_urls.insert(0, url);
    }

    let mut all_lines: Vec<Line> = Vec::new();
    for (group_idx, hls) in groups.into_iter().enumerate() {
        let url = &group_urls[group_idx];
        let mut sorted: Vec<&Highlight> = hls;
        sorted.sort_by_key(|h| h.span.start.offset);

        let mut lines: Vec<Line> = Vec::new();
        for hl in &sorted {
            let context = &hl.span.context;
            let span_text = &hl.span.text;
            let start_column = hl.span.start.column;

            let resolved_context = if context.is_empty() {
                span_text.as_str()
            } else {
                context.as_str()
            };
            let resolved_text = if context.is_empty() {
                ""
            } else {
                span_text.as_str()
            };
            let line_start =
                find_line_start(resolved_context, resolved_text, start_column).unwrap_or(0);

            let lines_before = resolved_context[..line_start]
                .chars()
                .filter(|&c| c == '\n')
                .count();
            let start_line = hl.span.start.line.saturating_sub(lines_before);
            let mut line_number = start_line;
            for line_text in resolved_context.split('\n') {
                if lines.is_empty() || line_number > lines.last().unwrap().number {
                    lines.push(Line {
                        text: line_text.to_string(),
                        number: line_number,
                        url: url.clone(),
                        highlights: vec![],
                    });
                }
                line_number += 1;
            }
        }

        let mut active: Vec<&Highlight> = Vec::new();
        let mut hl_idx = 0;
        for line_info in &mut lines {
            let cur_number = line_info.number;
            active.retain(|h| h.span.end.line >= cur_number);
            while hl_idx < sorted.len() {
                if sorted[hl_idx].span.start.line > cur_number {
                    break;
                }
                active.push(sorted[hl_idx]);
                hl_idx += 1;
            }
            for a in &active {
                line_info.highlights.push((*a).clone());
            }
        }

        all_lines.extend(lines);
    }

    Ok(all_lines)
}

// --- Helpers ---

use crate::common::file_span::SourceLocation;

/// Snaps a byte index down to the nearest char boundary (identity when
/// already on one). Columns arriving here are char-based counts, which only
/// coincide with byte indices on ASCII text — without snapping, slicing
/// panics mid-codepoint on non-ASCII lines.
fn char_boundary_at_or_before(text: &str, mut idx: usize) -> usize {
    idx = idx.min(text.len());
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn count_tabs(text: &str) -> usize {
    text.chars().filter(|&c| c == '\t').count()
}

fn is_only_whitespace(text: &str) -> bool {
    text.chars().all(|c| c == ' ' || c == '\t')
}

fn is_multiline(span: &SourceSpanWithContext) -> bool {
    span.start.line != span.end.line
}

/// Length of the last line in chars (Dart `_lastLineLength` counts UTF-16
/// units; the byte-based version over-counts multibyte chars, stretching
/// end columns and carets on non-ASCII lines).
fn last_line_length(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let len = text.chars().count();
    let chars: Vec<char> = text.chars().collect();
    if chars[len - 1] == '\n' {
        if len == 1 {
            return 0;
        }
        match chars[..len - 1].iter().rposition(|&c| c == '\n') {
            Some(pos) => len - pos - 1,
            None => len,
        }
    } else {
        match chars.iter().rposition(|&c| c == '\n') {
            Some(pos) => len - pos - 1,
            None => len,
        }
    }
}

fn is_text_at_end_of_context(span: &SourceSpanWithContext) -> bool {
    let context = &span.context;
    let text = &span.text;
    if let Some(ls) = find_line_start(context, text, span.start.column) {
        // Dart compares against `context.length` in UTF-16 units and
        // `span.length` (end-start offsets, unit-based); the byte-based
        // version over-counts multibyte chars. Compare in chars.
        context[..ls].chars().count() + span.start.column + span.text.chars().count()
            == context.chars().count()
    } else {
        false
    }
}

fn highlight_equal(a: &Highlight, b: &Highlight) -> bool {
    a.is_primary == b.is_primary && a.span.start == b.span.start && a.span.end == b.span.end
}

fn highlight_in_column(columns: &[Option<Highlight>], hl: &Highlight) -> bool {
    for col_hl in columns.iter().flatten() {
        if highlight_equal(col_hl, hl) {
            return true;
        }
    }
    false
}

fn replace_first_null(columns: &mut [Option<Highlight>], element: Highlight) {
    for slot in columns.iter_mut() {
        if slot.is_none() {
            *slot = Some(element);
            return;
        }
    }
}

fn replace_with_null(columns: &mut [Option<Highlight>], element: &Highlight) {
    for slot in columns.iter_mut() {
        if let Some(ref existing) = slot {
            if highlight_equal(existing, element) {
                *slot = None;
                return;
            }
        }
    }
}

fn contiguous(lines: &[Line]) -> bool {
    for i in 0..lines.len().saturating_sub(1) {
        if lines[i].number + 1 != lines[i + 1].number && lines[i].url == lines[i + 1].url {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::FileSpan;
    use crate::common::file_span::SourceLocation;
    use crate::common::source_span_file_source::FileSource;
    use crate::io::VirtualIo;
    use crate::url::SassUrl;
    use bumpalo::Bump;

    fn hl_io() -> VirtualIo {
        VirtualIo::with_cwd("/home/user")
    }

    fn ascii_opts() -> HighlightOptions {
        HighlightOptions {
            glyphs: GlyphSet::Ascii,
            ..Default::default()
        }
    }

    fn color_opts() -> HighlightOptions {
        HighlightOptions {
            color: HighlightColor::Default,
            glyphs: GlyphSet::Ascii,
            ..Default::default()
        }
    }

    fn no_color_opts() -> HighlightOptions {
        HighlightOptions {
            color: HighlightColor::None,
            glyphs: GlyphSet::Ascii,
            ..Default::default()
        }
    }

    fn hl_from_file(text: &str, start: usize, end: usize) -> String {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, text, None);
        let ssc =
            SourceSpanWithContext::from_file_span(&FileSpan::new(Some(fs), start, end)).unwrap();
        Highlighter::new(&ssc, &ascii_opts(), &hl_io())
            .unwrap()
            .highlight()
            .unwrap()
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
        SourceSpanWithContext::new(start, end, text.into(), context.into(), None).unwrap()
    }

    fn ssc_url(
        start: SourceLocation,
        end: SourceLocation,
        text: &str,
        context: &str,
        url: &str,
    ) -> SourceSpanWithContext {
        let u = if url.starts_with("file://") || url.starts_with("http") {
            SassUrl::parse(url).unwrap()
        } else {
            SassUrl::parse(&format!("file:///{url}")).unwrap()
        };
        SourceSpanWithContext::new(start, end, text.into(), context.into(), Some(u)).unwrap()
    }

    fn hl(span: &SourceSpanWithContext, opts: &HighlightOptions) -> String {
        Highlighter::new(span, opts, &hl_io())
            .unwrap()
            .highlight()
            .unwrap()
    }

    fn hl_multi(
        primary: &SourceSpanWithContext,
        label: &str,
        secondary: &[(SourceSpanWithContext, String)],
        opts: &HighlightOptions,
    ) -> String {
        Highlighter::new_multiple(primary, label, secondary, opts, &hl_io())
            .unwrap()
            .highlight()
            .unwrap()
    }

    #[test]
    fn test_points_to_span() {
        let s = ssc(loc(4, 0, 4), loc(7, 0, 7), "bar", "foo bar baz");
        let want = ["  ,", "1 | foo bar baz", "  |     ^^^", "  '"].join("\n");
        assert_eq!(hl(&s, &ascii_opts()), want);
    }

    #[test]
    fn test_no_url() {
        let s = ssc(loc(4, 0, 4), loc(7, 0, 7), "bar", "foo bar baz");
        let want = ["  ,", "1 | foo bar baz", "  |     ^^^", "  '"].join("\n");
        assert_eq!(hl(&s, &ascii_opts()), want);
    }

    #[test]
    fn test_single_line_file() {
        let s = ssc(loc(0, 0, 0), loc(7, 0, 7), "foo bar", "foo bar");
        let want = ["  ,", "1 | foo bar", "  | ^^^^^^^", "  '"].join("\n");
        assert_eq!(hl(&s, &ascii_opts()), want);
    }

    #[test]
    fn test_includes_trailing_newline() {
        let s = ssc(loc(8, 0, 8), loc(12, 0, 12), "baz\n", "foo bar baz\n");
        let want = ["  ,", "1 | foo bar baz", "  |         ^^^", "  '"].join("\n");
        assert_eq!(hl(&s, &ascii_opts()), want);
    }

    #[test]
    fn test_multiline() {
        let s = ssc(
            loc(4, 0, 4),
            loc(34, 2, 7),
            "bar baz\nwhiz bang boom\nzip zap ",
            "foo bar baz\nwhiz bang boom\nzip zap zop\n",
        );
        let want = [
            "  ,",
            "1 |   foo bar baz",
            "  | ,-----^",
            "2 | | whiz bang boom",
            "3 | | zip zap zop",
            "  | '-------^",
            "  '",
        ]
        .join("\n");
        assert_eq!(hl(&s, &ascii_opts()), want);
    }

    #[test]
    fn test_full_first_line() {
        let s = ssc(
            loc(0, 0, 0),
            loc(34, 2, 7),
            "foo bar baz\nwhiz bang boom\nzip zap ",
            "foo bar baz\nwhiz bang boom\nzip zap zop\n",
        );
        let want = [
            "  ,",
            "1 | / foo bar baz",
            "2 | | whiz bang boom",
            "3 | | zip zap zop",
            "  | '-------^",
            "  '",
        ]
        .join("\n");
        assert_eq!(hl(&s, &ascii_opts()), want);
    }

    #[test]
    fn test_begins_at_end_of_line() {
        let s = ssc(
            loc(11, 0, 11),
            loc(34, 2, 7),
            "\nwhiz bang boom\nzip zap ",
            "foo bar baz\nwhiz bang boom\nzip zap zop\n",
        );
        let want = [
            "  ,",
            "1 |   foo bar baz",
            "  | ,------------^",
            "2 | | whiz bang boom",
            "3 | | zip zap zop",
            "  | '-------^",
            "  '",
        ]
        .join("\n");
        assert_eq!(hl(&s, &ascii_opts()), want);
    }

    #[test]
    fn test_ends_at_beginning_of_line() {
        let out = hl_from_file("foo bar baz\nwhiz bang boom\nzip zap zop\n", 4, 28);
        let want = [
            "  ,",
            "1 |   foo bar baz",
            "  | ,-----^",
            "2 | | whiz bang boom",
            "3 | | zip zap zop",
            "  | '-^",
            "  '",
        ]
        .join("\n");
        assert_eq!(out, want);
    }

    #[test]
    fn test_full_last_line_at_end_of_file() {
        let out = hl_from_file("foo bar baz\nwhiz bang boom\nzip zap zop\n", 4, 39);
        let want = [
            "  ,",
            "1 |   foo bar baz",
            "  | ,-----^",
            "2 | | whiz bang boom",
            "3 | \\ zip zap zop",
            "  '",
        ]
        .join("\n");
        assert_eq!(out, want);
    }

    #[test]
    fn test_full_last_line() {
        let out = hl_from_file("foo bar baz\nwhiz bang boom\nzip zap zop\n", 4, 27);
        let want = [
            "  ,",
            "1 |   foo bar baz",
            "  | ,-----^",
            "2 | \\ whiz bang boom",
            "  '",
        ]
        .join("\n");
        assert_eq!(out, want);
    }

    #[test]
    fn test_full_last_line_no_trailing_newline() {
        let out = hl_from_file("foo bar baz\nwhiz bang boom\nzip zap zop", 4, 26);
        let want = [
            "  ,",
            "1 |   foo bar baz",
            "  | ,-----^",
            "2 | \\ whiz bang boom",
            "  '",
        ]
        .join("\n");
        assert_eq!(out, want);
    }

    #[test]
    fn test_tabs_before_span() {
        let s = ssc(loc(4, 0, 4), loc(7, 0, 7), "bar", "foo\tbar baz");
        let want = ["  ,", "1 | foo    bar baz", "  |        ^^^", "  '"].join("\n");
        assert_eq!(hl(&s, &ascii_opts()), want);
    }

    #[test]
    fn test_tabs_within_span() {
        let s = ssc(
            loc(4, 0, 4),
            loc(11, 0, 11),
            "bar\tbaz",
            "foo bar\tbaz bang",
        );
        let want = [
            "  ,",
            "1 | foo bar    baz bang",
            "  |     ^^^^^^^^^^",
            "  '",
        ]
        .join("\n");
        assert_eq!(hl(&s, &ascii_opts()), want);
    }

    #[test]
    fn test_multiple_separate() {
        let p = ssc_url(
            loc(17, 1, 5),
            loc(21, 1, 9),
            "bang",
            "whiz bang boom",
            "file:///file1.txt",
        );
        let s1 = ssc_url(
            loc(4, 0, 4),
            loc(7, 0, 7),
            "bar",
            "foo bar baz",
            "file:///file1.txt",
        );
        let s2 = ssc_url(
            loc(31, 2, 4),
            loc(34, 2, 7),
            "zap",
            "zip zap zop",
            "file:///file1.txt",
        );
        let secondary = vec![(s1, "three".into()), (s2, "two".into())];
        let want = [
            "  ,",
            "1 | foo bar baz",
            "  |     === three",
            "2 | whiz bang boom",
            "  |      ^^^^ one",
            "3 | zip zap zop",
            "  |     === two",
            "  '",
        ]
        .join("\n");
        assert_eq!(hl_multi(&p, "one", &secondary, &ascii_opts()), want);
    }

    #[test]
    fn test_multiple_same_file_url() {
        let p = ssc_url(
            loc(17, 1, 5),
            loc(21, 1, 9),
            "bang",
            "whiz bang boom",
            "file:///file1.txt",
        );
        let s = ssc_url(
            loc(4, 0, 4),
            loc(7, 0, 7),
            "bar",
            "foo bar baz",
            "file:///file1.txt",
        );
        let secondary = vec![(s, "three".into())];
        let want = [
            "  ,",
            "1 | foo bar baz",
            "  |     === three",
            "2 | whiz bang boom",
            "  |      ^^^^ one",
            "  '",
        ]
        .join("\n");
        assert_eq!(hl_multi(&p, "one", &secondary, &ascii_opts()), want);
    }

    #[test]
    fn test_multiple_same_line() {
        let s1 = ssc(loc(4, 0, 4), loc(7, 0, 7), "bar", "foo bar baz");
        let s2 = ssc(loc(8, 0, 8), loc(11, 0, 11), "baz", "foo bar baz");
        let secondary = vec![(s2, "z".into())];
        let out = hl_multi(&s1, "bar", &secondary, &ascii_opts());
        assert!(out.contains("bar") && out.contains("z"));
    }

    #[test]
    fn test_multiple_multiline() {
        let p = ssc_url(
            loc(4, 0, 4),
            loc(34, 2, 7),
            "bar baz\nwhiz bang boom\nzip zap ",
            "foo bar baz\nwhiz bang boom\nzip zap zop\n",
            "file:///f.txt",
        );
        let s = ssc_url(
            loc(14, 0, 2),
            loc(18, 0, 6),
            "iz b",
            "whiz bang boom",
            "file:///f.txt",
        );
        let secondary = vec![(s, "inner".into())];
        let out = hl_multi(&p, "outer", &secondary, &ascii_opts());
        assert!(out.contains("inner") && out.contains("outer"));
    }

    #[test]
    fn test_multiple_different_sources() {
        let s1 = ssc(loc(2, 0, 2), loc(3, 0, 3), "b", "a b c");
        let s2 = ssc(loc(2, 0, 2), loc(3, 0, 3), "y", "x y z");
        let secondary = vec![(s2, "note".into())];
        let out = hl_multi(&s1, "primary", &secondary, &ascii_opts());
        assert!(out.contains("primary") && out.contains("note"));
    }

    #[test]
    fn test_with_color() {
        let s = ssc(loc(4, 0, 4), loc(7, 0, 7), "bar", "foo bar baz");
        let out = hl(&s, &color_opts());
        assert!(out.contains("\x1b[31m"));
        assert!(out.contains("\x1b[0m"));
    }

    #[test]
    fn test_no_color() {
        let s = ssc(loc(4, 0, 4), loc(7, 0, 7), "bar", "foo bar baz");
        let out = hl(&s, &no_color_opts());
        assert!(!out.contains("\x1b["));
    }

    #[test]
    fn test_custom_color() {
        let s = ssc(loc(4, 0, 4), loc(7, 0, 7), "bar", "foo bar baz");
        let opts = HighlightOptions {
            color: HighlightColor::Custom("\x1b[35m".into()),
            glyphs: GlyphSet::Ascii,
            ..Default::default()
        };
        let out = hl(&s, &opts);
        assert!(out.contains("\x1b[35m"));
    }

    #[test]
    fn test_unicode_glyphs() {
        let s = ssc(
            loc(4, 0, 4),
            loc(34, 2, 7),
            "bar baz\nwhiz bang boom\nzip zap ",
            "foo bar baz\nwhiz bang boom\nzip zap zop\n",
        );
        let out = hl(&s, &HighlightOptions::default());
        assert!(out.contains("│"));
        assert!(out.contains("┌"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_non_ascii_caret_width() {
        // Plan-adjacent (U31 spans): char-based columns must yield char-wide
        // carets on non-ASCII lines — byte counts over-count (14 vs 15 here)
        // and byte slicing panics mid-codepoint (pre-fix abort on ☃ lines).
        // CLI-verified byte-identical vs Dart on `@error "café ☃";`.
        let text = "@error \"café ☃\"";
        let s = ssc(loc(0, 0, 0), loc(15, 0, 15), text, "@error \"café ☃\";\n");
        let out = hl(&s, &HighlightOptions::default());
        let carets = out.lines().find(|l| l.chars().any(|c| c == '^')).unwrap();
        assert_eq!(
            carets.chars().filter(|&c| c == '^').count(),
            15,
            "caret row must be 15 wide, got: {carets:?}"
        );
    }
}
