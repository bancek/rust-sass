// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

//! Serializes the CSS output tree (or a single value) to CSS text.
//!
//! Dart's `_SerializeVisitor` is split here into [`SerializeState`] (plain
//! formatting flags) plus free `visit_*_impl` functions taking
//! `(&mut SourceMapBuffer, &mut SerializeState)` — the two-level dispatch
//! described in `docs/ref/serialize.md`. Inside [`for_node`] callbacks only
//! the buffer and state are live, so dispatch goes through manual `match`
//! helpers rather than `accept()`.

// dart-source: lib/src/visitor/serialize.dart (serialize, serializeValue, serializeSelector, _SerializeVisitor utilities, OutputStyle (Nested, Compact: not present in Dart — libsass-compat extensions), LineFeed, SerializeResult)
// go-source: go/value/visitor_serialize.go

use crate::common::span_error::SpanError;
use crate::serialize::css::visit_css_node_impl;
use std::fmt::Write;

use crate::ast::css::node::CssNode;
use crate::ast::css::stylesheet::CssStylesheet;
use crate::ast::css::visitor::CssVisitor;
use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::common::span::Span;
use crate::source_map_buffer::SourceMapBuffer;
use crate::sourcemap::{Builder, SingleMapping};
use crate::value::Value;

pub mod calc;
pub mod color;
pub mod css;
pub mod list;
pub mod number;
pub mod selector;
pub mod string;
pub mod value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The generated CSS style: one declaration per line, as few bytes as
/// possible, or a libsass-compatible output style.
pub enum OutputStyle {
    /// Standard multi-line style (`.sidebar {\n  width: 100px;\n}`).
    Expanded,
    /// Minified single-line style (`.sidebar{width:100px}`).
    Compressed,
    /// Libsass NESTED style (`a {\n  b: 3; }` — glued `; }` closer,
    /// source-depth indentation, blank line between top-level blocks).
    /// No Dart counterpart (Dart only has expanded/compressed); serializer
    /// only, shares the Expanded arm everywhere except the block closer.
    Nested,
    /// Libsass COMPACT style (`a { b: 3; }` — one top-level block per
    /// line, declarations space-separated, glued `; }` closer, blank line
    /// between top-level blocks, zero indentation). No Dart counterpart;
    /// serializer only, shares the Expanded arm for value spellings and
    /// the Compressed arm for indentation suppression.
    Compact,
}

#[derive(Debug, Clone, Copy)]
/// A line-feed sequence used between emitted lines.
pub struct LineFeed {
    /// Short name (`lf`, `crlf`, …); Dart's `LineFeed.name`.
    pub name: &'static str,
    /// Bytes emitted for one line break; Dart's `LineFeed.text`.
    pub text: &'static str,
}

/// The default line feed (LF).
pub const LINE_FEED_LF: LineFeed = LineFeed {
    name: "lf",
    text: "\n",
};

/// CRLF line feed (node-sass `crlf`, libsass C-API `"\r\n"`).
pub const LINE_FEED_CRLF: LineFeed = LineFeed {
    name: "crlf",
    text: "\r\n",
};

/// CR line feed (node-sass `cr`, libsass C-API `"\r"`).
pub const LINE_FEED_CR: LineFeed = LineFeed {
    name: "cr",
    text: "\r",
};

/// LFCR line feed (node-sass `lfcr`, libsass C-API `"\n\r"`).
pub const LINE_FEED_LFCR: LineFeed = LineFeed {
    name: "lfcr",
    text: "\n\r",
};

#[derive(Debug, Clone, Copy)]
/// Formatting flags for one serialization run: indentation level, output
/// style, inspect mode, quoting, line feed, and indent shape.
///
/// Mirrors Dart's `_SerializeVisitor` scalar fields (`_indentation`,
/// `_style`, `_inspect`, `_quote`, `_indentCharacter`, `_indentWidth`,
/// `_lineFeed`); the buffer lives separately so callbacks can borrow both.
pub struct SerializeState {
    /// Current nesting depth (Dart's `_indentation`).
    pub indentation: u32,
    /// Expanded vs compressed output (Dart's `_style`).
    pub style: OutputStyle,
    /// Emit `/* line N, path */` comments (libsass C-API seam; no Dart).
    pub source_comments: bool,
    /// Unambiguous SCSS representation instead of valid CSS (Dart's `_inspect`).
    pub inspect: bool,
    /// Whether quoted strings keep their quotes (Dart's `_quote`).
    pub quote: bool,
    /// Line-break text (Dart's `_lineFeed`).
    pub line_feed: LineFeed,
    /// Space or tab per level (Dart's `_indentCharacter`).
    pub indent_char: char,
    /// Spaces/tabs per level, 0–10 (Dart's `_indentWidth`).
    pub indent_width: u32,
}

impl SerializeState {
    /// Creates expanded-output state with default 2-space indentation.
    pub fn new(quote: bool, inspect: bool) -> Self {
        SerializeState {
            indentation: 0,
            style: OutputStyle::Expanded,
            source_comments: false,
            inspect,
            quote,
            line_feed: LINE_FEED_LF,
            indent_char: ' ',
            indent_width: 2,
        }
    }

    /// Comma separator for the current style (`","` vs `", "`).
    pub fn comma_sep(&self) -> &'static str {
        match self.style {
            OutputStyle::Compressed => ",",
            OutputStyle::Expanded | OutputStyle::Nested | OutputStyle::Compact => ", ",
        }
    }

    fn write_byte_times(buf: &mut SourceMapBuffer<'_>, b: u8, times: usize) {
        for _ in 0..times {
            buf.write_char(b as char).unwrap();
        }
    }

    /// Writes indentation from the current level unless compressed.
    pub fn write_indentation(&self, buf: &mut SourceMapBuffer<'_>) {
        if matches!(self.style, OutputStyle::Expanded | OutputStyle::Nested) {
            Self::write_byte_times(
                buf,
                self.indent_char as u8,
                (self.indentation * self.indent_width) as usize,
            );
        }
    }

    /// Writes a line feed unless emitting compressed CSS.
    pub fn write_line_feed(&self, buf: &mut SourceMapBuffer<'_>) {
        if matches!(
            self.style,
            OutputStyle::Expanded | OutputStyle::Nested | OutputStyle::Compact
        ) {
            write!(buf, "{}", self.line_feed.text).unwrap();
        }
    }

    /// Intra-block child separator: a space in COMPACT (one top-level block
    /// per line), a line feed in Expanded/Nested, nothing in Compressed.
    /// Libsass `append_optional_linefeed` (`emitter.cpp:247-255`); kept
    /// separate from [`SerializeState::write_line_feed`] (which also feeds
    /// inter-block/top-level separation, where COMPACT needs a real LF).
    pub fn write_block_break(&self, buf: &mut SourceMapBuffer<'_>) {
        match self.style {
            OutputStyle::Compact => {
                buf.write_char(' ').unwrap();
            }
            OutputStyle::Expanded | OutputStyle::Nested => {
                write!(buf, "{}", self.line_feed.text).unwrap();
            }
            OutputStyle::Compressed => {}
        }
    }

    /// Child separator for supports, keyframe, and non-`@font-face` at-rule
    /// blocks: always a line feed (with raw indent in COMPACT, where
    /// [`SerializeState::write_indentation`] is a no-op). Libsass
    /// `append_special_linefeed` (`emitter.cpp:238-245`), called from
    /// `output.cpp:197` (keyframes), `:230` (supports), `:293` (at-rules).
    pub fn write_special_linefeed(&self, buf: &mut SourceMapBuffer<'_>) {
        if matches!(self.style, OutputStyle::Compact) {
            write!(buf, "{}", self.line_feed.text).unwrap();
            // Raw indent at the (already incremented) child level, bypassing
            // `write_indentation` (a no-op in COMPACT by design).
            self.write_raw_indentation(buf);
        } else {
            self.write_line_feed(buf);
        }
    }

    /// Writes the current indentation unconditionally (bypasses the
    /// COMPACT suppression in [`SerializeState::write_indentation`]).
    fn write_raw_indentation(&self, buf: &mut SourceMapBuffer<'_>) {
        Self::write_byte_times(
            buf,
            self.indent_char as u8,
            (self.indentation * self.indent_width) as usize,
        );
    }

    /// Indentation for reindented custom-property continuation lines
    /// (`write_with_indent` in `css.rs`): identical to
    /// [`SerializeState::write_indentation`] except it also emits in
    /// COMPACT, so declaration values match Expanded byte-for-byte.
    pub fn write_reindented_indentation(&self, buf: &mut SourceMapBuffer<'_>) {
        self.write_raw_indentation(buf);
    }

    /// Writes a space unless the style is compressed.
    pub fn write_optional_space(&self, buf: &mut SourceMapBuffer<'_>) {
        if matches!(
            self.style,
            OutputStyle::Expanded | OutputStyle::Nested | OutputStyle::Compact
        ) {
            buf.write_char(' ').unwrap();
        }
    }

    /// Extra indent levels for a hoisted node in NESTED output (the
    /// evaluator's `tabs` stamp, mirroring libsass's `indentation += tabs`
    /// bracketing around the node's whole subtree). Zero unless Nested.
    pub fn nested_tabs(&self, tabs: u32) -> u32 {
        match self.style {
            OutputStyle::Nested => tabs,
            OutputStyle::Compressed | OutputStyle::Expanded | OutputStyle::Compact => 0,
        }
    }

    /// Runs `cb` with indentation increased one level.
    pub fn with_indent<'parse, F>(
        &mut self,
        buf: &mut SourceMapBuffer<'parse>,
        cb: F,
    ) -> SassResult<()>
    where
        F: FnOnce(&mut Self, &mut SourceMapBuffer<'parse>) -> SassResult<()>,
    {
        self.indentation += 1;
        let result = cb(self, buf);
        self.indentation -= 1;
        result
    }

    /// Runs `cb` with indentation suppressed (for trailing comments).
    pub fn without_indent<'parse, F>(
        &mut self,
        buf: &mut SourceMapBuffer<'parse>,
        cb: F,
    ) -> SassResult<()>
    where
        F: FnOnce(&mut Self, &mut SourceMapBuffer<'parse>) -> SassResult<()>,
    {
        let saved = self.indentation;
        self.indentation = 0;
        let result = cb(self, buf);
        self.indentation = saved;
        result
    }
}

/// The serializer handle: output buffer plus formatting state.
///
/// Thin wrapper — real logic lives in the free `visit_*_impl` functions so
/// [`for_node`] callbacks (which only hold `&mut` buffer + state) can
/// dispatch without `accept()`.
pub struct SerializeVisitor<'parse> {
    /// Output buffer (plain or source-mapping); Dart's `_buffer`.
    pub buffer: SourceMapBuffer<'parse>,
    /// Formatting flags (Dart's scalar `_SerializeVisitor` fields).
    pub inner: SerializeState,
}

impl<'parse> SerializeVisitor<'parse> {
    /// Plain (source-map-free) serializer for values and tests.
    pub fn new_plain(quote: bool, inspect: bool) -> Self {
        SerializeVisitor {
            buffer: SourceMapBuffer::new_plain(),
            inner: SerializeState::new(quote, inspect),
        }
    }

    /// Consumes the visitor and returns the emitted text.
    pub fn into_string(self) -> String {
        self.buffer.into_string()
    }

    // No `_logger` field: Dart keeps one on `_SerializeVisitor` but only for
    // statement-level serialization (retained there to avoid deprecation
    // churn); expression/value serialization never reads it.

    /// Comma separator for the current style.
    pub fn comma_sep(&self) -> &'static str {
        self.inner.comma_sep()
    }

    /// Writes current indentation to the buffer.
    pub fn write_indentation(&mut self) {
        self.inner.write_indentation(&mut self.buffer)
    }

    /// Writes a line feed unless compressed.
    pub fn write_line_feed(&mut self) {
        self.inner.write_line_feed(&mut self.buffer)
    }

    /// Writes a space unless compressed.
    pub fn write_optional_space(&mut self) {
        self.inner.write_optional_space(&mut self.buffer)
    }

    /// Whether `node` is omitted from output (inspect shows everything;
    /// compressed additionally hides loud comments).
    pub fn is_invisible(&self, node: &CssNode<'parse>) -> bool {
        if self.inner.inspect {
            return false;
        }
        if matches!(self.inner.style, OutputStyle::Compressed) {
            return node.is_invisible_hiding_comments();
        }
        node.is_invisible()
    }

    /// Whether a comment `node` following `previous_span` sits on the same line
    /// (emitted inline after a space rather than on its own line).
    ///
    /// `previous_span` may be a sibling or the parent when `node` is the first
    /// visible child. Short-circuits in compressed mode, where whitespace is
    /// collapsed anyway.
    pub fn is_trailing_comment(
        &self,
        node: &CssNode<'parse>,
        previous_span: &FileSpan<'parse>,
    ) -> SassResult<bool> {
        if matches!(
            self.inner.style,
            OutputStyle::Compressed | OutputStyle::Nested | OutputStyle::Compact
        ) {
            return Ok(false);
        }
        let CssNode::Comment(_) = node else {
            return Ok(false);
        };
        let node_span = node.span()?;
        if node_span.source_url() != previous_span.source_url() {
            return Ok(false);
        }

        if !previous_span
            .contains(&Span::File(node_span))
            .map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: e.to_string(),
                    argument_name: None,
                }),
            })?
        {
            let node_start = node_span.start_location();
            let previous_end = previous_span.end_location();
            return Ok(node_start.line == previous_end.line);
        }

        let node_start = node_span.start_location();
        let previous_start = previous_span.start_location();
        let search_from = node_start.offset.saturating_sub(previous_start.offset + 1);
        if search_from == 0 && node_start.offset <= previous_start.offset + 1 {
            return Ok(false);
        }
        let search_text = previous_span.text();
        if search_text.is_empty() {
            return Ok(false);
        }
        let search_text = &search_text[..search_from.min(search_text.len())];
        let end_offset = search_text.rfind('{').map_or(0, |i| i + 1);
        let end_line = previous_start.line
            + search_text[..end_offset]
                .chars()
                .filter(|&c| c == '\n')
                .count();
        Ok(node_start.line == end_line)
    }
}

/// Writes each item with `sep` between them (Dart's `_writeBetween`).
pub(crate) fn write_between<'parse, T>(
    buf: &mut SourceMapBuffer<'parse>,
    items: &[T],
    sep: &str,
    mut cb: impl FnMut(&mut SourceMapBuffer<'parse>, &T) -> SassResult<()>,
) -> SassResult<()> {
    let mut first = true;
    for item in items {
        if first {
            first = false;
        } else {
            write!(buf, "{}", sep).unwrap();
        }
        cb(buf, item)?;
    }
    Ok(())
}

/// Runs `cb` and associates all text it writes with `node`'s span
/// (Dart's `_for`, for source-map tracking).
pub(crate) fn for_node<'parse, N: AstNode<'parse>>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    node: &N,
    cb: impl FnOnce(&mut SourceMapBuffer<'parse>, &mut SerializeState) -> SassResult<()>,
) -> SassResult<()> {
    let span = node.span()?;
    buf.for_span(&span, |inner_buf| cb(inner_buf, state))
}

pub(crate) fn is_invisible(state: &SerializeState, node: &CssNode<'_>) -> bool {
    if state.inspect {
        return false;
    }
    if matches!(state.style, OutputStyle::Compressed) {
        return node.is_invisible_hiding_comments();
    }
    node.is_invisible()
}

/// Whether `node` needs a `;` after it: childless at-rules, declarations,
/// and imports — never rules, comments, or parent blocks with children.
pub(crate) fn requires_semicolon(node: &CssNode<'_>) -> bool {
    match node {
        CssNode::AtRule(r) => r.childless,
        CssNode::StyleRule(_)
        | CssNode::KeyframeBlock(_)
        | CssNode::MediaRule(_)
        | CssNode::SupportsRule(_) => false,
        CssNode::Comment(_) => false,
        CssNode::Declaration(_) | CssNode::Import(_) => true,
        CssNode::Stylesheet(_) => false,
    }
}

/// Emits a parent's children in a `{...}` block, one indented child per line
/// (trailing comments stay on the previous line); Dart's `_visitChildren`.
/// In COMPACT the intra-block separator is a space (one-line blocks).
pub(crate) fn visit_children<'parse>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    children: &[CssNode<'parse>],
    parent_span: FileSpan<'parse>,
) -> SassResult<()> {
    visit_children_impl(buf, state, children, parent_span, false)
}

/// Emits a parent's children with every child on its own line in all styles
/// (COMPACT included): supports, keyframe, and non-`@font-face` at-rule
/// blocks. Libsass `append_special_linefeed` (`output.cpp:197,230,293`).
pub(crate) fn visit_children_special<'parse>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    children: &[CssNode<'parse>],
    parent_span: FileSpan<'parse>,
) -> SassResult<()> {
    visit_children_impl(buf, state, children, parent_span, true)
}

fn visit_children_impl<'parse>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    children: &[CssNode<'parse>],
    parent_span: FileSpan<'parse>,
    special: bool,
) -> SassResult<()> {
    buf.write_char('{').unwrap();
    let mut pre_previous: Option<&CssNode<'parse>> = None;
    let mut previous: Option<&CssNode<'parse>> = None;
    for child in children {
        if is_invisible(state, child) {
            continue;
        }
        if let Some(prev) = previous {
            if requires_semicolon(prev) {
                buf.write_char(';').unwrap();
            }
        }
        let prev_span = match previous {
            Some(p) => p.span()?,
            None => parent_span,
        };
        let is_trailing = is_trailing_comment(state, child, &prev_span)?;
        if is_trailing {
            state.write_optional_space(buf);
            state.without_indent(buf, |state, buf| visit_css_node_impl(buf, state, child))?;
        } else {
            // The separator runs inside `with_indent` so the COMPACT
            // special linefeed indents at the child level; separator bytes
            // never depend on the level, so Expanded/Nested/Compressed
            // output is unchanged. The first child always takes the plain
            // block break (libsass emits the special linefeed only *between*
            // children: `if (i < L - 1)` in `output.cpp:197,230,293`).
            let first = previous.is_none();
            state.with_indent(buf, |state, buf| {
                if special && !first {
                    state.write_special_linefeed(buf);
                } else {
                    state.write_block_break(buf);
                }
                visit_css_node_impl(buf, state, child)
            })?;
        }
        pre_previous = previous;
        previous = Some(child);
    }
    if let Some(prev) = previous {
        if requires_semicolon(prev) && !matches!(state.style, OutputStyle::Compressed) {
            buf.write_char(';').unwrap();
        }
        let is_trailing = is_trailing_comment(state, prev, &parent_span)?;
        if pre_previous.is_none() && is_trailing {
            state.write_optional_space(buf);
        } else {
            // The Compact fork: Expanded re-indents the `}` onto its own
            // line; Nested and Compact glue it (`; }`). Compressed writes
            // nothing (both helpers are no-ops there).
            match state.style {
                OutputStyle::Compressed => {}
                OutputStyle::Expanded => {
                    state.write_line_feed(buf);
                    state.write_indentation(buf);
                }
                OutputStyle::Nested | OutputStyle::Compact => {
                    buf.write_char(' ').unwrap();
                }
            }
        }
    }
    buf.write_char('}').unwrap();
    Ok(())
}

// Free-function twin of `SerializeVisitor::is_trailing_comment` for use in
// `for_node`-style contexts holding only `state`. Walks back from just
// before `node` to the parent's `{` (safer than a forward search, which could
// hit unrelated braces; imports can nest identical spans, hence the
// `search_from == 0` guard).
fn is_trailing_comment(
    state: &SerializeState,
    node: &CssNode<'_>,
    previous_span: &FileSpan<'_>,
) -> SassResult<bool> {
    // Nested keeps comments on their own lines like libsass (no trailing
    // inlining); Compact joins it (comments space-join via the block
    // break, so inlining is unobservable); see `nested_block_separated`
    // in `css.rs`.
    if matches!(
        state.style,
        OutputStyle::Compressed | OutputStyle::Nested | OutputStyle::Compact
    ) {
        return Ok(false);
    }
    let CssNode::Comment(_) = node else {
        return Ok(false);
    };
    let node_span = node.span()?;
    if node_span.source_url() != previous_span.source_url() {
        return Ok(false);
    }

    if !previous_span
        .contains(&Span::File(node_span))
        .map_err(|e| match e {
            SpanError::Sass(e) => e,
            _ => Box::new(SassError::Script {
                message: e.to_string(),
                argument_name: None,
            }),
        })?
    {
        let node_start = node_span.start_location();
        let previous_end = previous_span.end_location();
        return Ok(node_start.line == previous_end.line);
    }

    let node_start = node_span.start_location();
    let previous_start = previous_span.start_location();
    let search_from = node_start.offset.saturating_sub(previous_start.offset + 1);
    if search_from == 0 && node_start.offset <= previous_start.offset + 1 {
        return Ok(false);
    }
    let search_text = previous_span.text();
    if search_text.is_empty() {
        return Ok(false);
    }
    let search_slice = &search_text[..search_from.min(search_text.len())];
    let end_offset = search_slice.rfind('{').map_or(0, |i| i + 1);
    let end_line = previous_start.line
        + search_slice[..end_offset]
            .chars()
            .filter(|&c| c == '\n')
            .count();
    Ok(node_start.line == end_line)
}

/// Converts `value` to CSS, erroring on values with no plain-CSS form
/// (maps, functions, mixins, `()`). Set `quote` to `false` to emit quoted
/// strings without quotes. Dart's `serializeValue`.
pub fn serialize_value(value: &Value<'_>, quote: bool) -> SassResult<String> {
    let mut visitor = SerializeVisitor::new_plain(quote, false);
    value.accept(&mut visitor)?;
    Ok(visitor.into_string())
}

/// Like [`serialize_value`], but emits the unambiguous inspect representation
/// (valid SCSS, possibly not valid CSS). Dart's `serializeValue(inspect: true)`.
pub fn serialize_value_inspect(value: &Value<'_>) -> SassResult<String> {
    let mut visitor = SerializeVisitor::new_plain(true, true);
    value.accept(&mut visitor)?;
    Ok(visitor.into_string())
}

// ===========================================================================
// SerializeResult, SerializeOptions, serialize()
// (Dart's `serialize` function + `SerializeResult` typedef)
// ===========================================================================

/// The result of converting a CSS AST to CSS text: the CSS plus an optional
/// source map (`None` when source mapping was disabled).
#[derive(Debug)]
pub struct SerializeResult {
    pub css: String,
    pub source_map: Option<SingleMapping>,
}

/// Options controlling CSS serialization (style, indentation, line feeds,
/// charset/BOM prefix, source maps).
pub struct SerializeOptions {
    pub style: OutputStyle,
    /// Emit libsass-style `/* line N, path */` source comments before each
    /// style rule (no Dart counterpart; C-API seam only).
    pub source_comments: bool,
    pub inspect: bool,
    pub use_spaces: bool,
    pub indent_width: u32,
    pub line_feed: LineFeed,
    pub charset: bool,
    pub source_map: bool,
    pub include_source_map_sources: bool,
}

impl Default for SerializeOptions {
    fn default() -> Self {
        SerializeOptions {
            style: OutputStyle::Expanded,
            source_comments: false,
            inspect: false,
            use_spaces: true,
            indent_width: 2,
            line_feed: LINE_FEED_LF,
            charset: true,
            source_map: false,
            include_source_map_sources: false,
        }
    }
}

/// Converts a CSS stylesheet to CSS text, adding a `@charset` declaration
/// (expanded) or BOM (compressed) when non-ASCII output needs it, and an
/// optional source map. `indent_width` must be 0–10. Dart's `serialize`.
pub fn serialize(
    stylesheet: &CssStylesheet<'_>,
    opts: &SerializeOptions,
) -> SassResult<SerializeResult> {
    // Range check mirrors Dart's `RangeError.checkValueInInterval(indentWidth, 0, 10)`.
    if opts.indent_width > 10 {
        return Err(Box::new(SassError::Script {
            message: format!("must be between 0 and 10, was {}", opts.indent_width),
            argument_name: Some("indentWidth".into()),
        }));
    }

    let buffer = if opts.source_map {
        let builder = Builder::new(String::new());
        SourceMapBuffer::new_mapping(builder)
    } else {
        SourceMapBuffer::new_plain()
    };

    let state = SerializeState {
        indentation: 0,
        style: opts.style,
        source_comments: opts.source_comments,
        inspect: opts.inspect,
        quote: true,
        line_feed: opts.line_feed,
        indent_char: if opts.use_spaces { ' ' } else { '\t' },
        indent_width: opts.indent_width,
    };

    let mut visitor = SerializeVisitor {
        buffer,
        inner: state,
    };

    visitor.visit_css_stylesheet(stylesheet)?;

    let css = visitor.buffer.as_string().to_string();

    let prefix = if opts.charset && css.chars().any(|c| c as u32 > 0x7F) {
        match opts.style {
            OutputStyle::Compressed => "\u{FEFF}",
            OutputStyle::Expanded | OutputStyle::Nested | OutputStyle::Compact => {
                "@charset \"UTF-8\";\n"
            }
        }
    } else {
        ""
    };

    let source_map = if opts.source_map {
        Some(
            visitor
                .buffer
                .build_source_map(prefix, opts.include_source_map_sources)?,
        )
    } else {
        None
    };

    Ok(SerializeResult {
        css: format!("{prefix}{css}"),
        source_map,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::css::comment::CssComment;
    use crate::ast::css::node::CssNode;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::common::source_span_file_source::FileSource;
    use crate::compile::{compile_string, CompileOptions};
    use crate::io::{DefaultIo, Io};
    use crate::url::SassUrl;
    use bumpalo::Bump;
    use std::rc::Rc;

    fn test_io() -> Rc<dyn Io> {
        Rc::new(DefaultIo::new())
    }

    #[rust_sass_macros::maybe_async]
    async fn compile_nested(source: &str) -> String {
        let arena = bumpalo::Bump::new();
        let opts = CompileOptions {
            style: OutputStyle::Nested,
            ..CompileOptions::new(&arena)
        };
        compile_string(source, test_io(), opts, &arena)
            .await
            .unwrap()
            .css()
            .to_string()
    }

    fn empty_stylesheet() -> CssStylesheet<'static> {
        CssStylesheet::new(vec![], BOGUS_SPAN)
    }

    fn stylesheet_with_comment(comment: &str) -> CssStylesheet<'static> {
        let comment_node = CssNode::Comment(CssComment::new(comment.to_string(), BOGUS_SPAN));
        CssStylesheet::new(vec![comment_node], BOGUS_SPAN)
    }

    fn preserved_comment(text: &str) -> CssNode<'static> {
        CssNode::Comment(CssComment {
            text: text.to_string(),
            is_preserved: true,
            span: BOGUS_SPAN,
            is_group_end: false,
        })
    }

    #[test]
    fn test_serialize_empty() {
        let result = serialize(&empty_stylesheet(), &SerializeOptions::default()).unwrap();
        assert_eq!(result.css, "");
        assert!(result.source_map.is_none());
    }

    #[test]
    fn test_serialize_source_map_enabled() {
        let stylesheet = stylesheet_with_comment("/* hello */");
        let opts = SerializeOptions {
            source_map: true,
            ..SerializeOptions::default()
        };
        let result = serialize(&stylesheet, &opts).unwrap();
        assert!(result.source_map.is_some());
    }

    #[test]
    fn test_serialize_source_map_disabled() {
        let result = serialize(&empty_stylesheet(), &SerializeOptions::default()).unwrap();
        assert!(result.source_map.is_none());
    }

    #[test]
    fn test_serialize_source_map_include_sources_toggle() {
        // A file-backed comment so `source_files` is populated and
        // sourcesContent can actually be emitted.
        let arena = Bump::new();
        let text = "/* hello */";
        let fs = FileSource::new_in(
            &arena,
            text,
            Some(SassUrl::parse("file:///input.scss").unwrap()),
        );
        let span = FileSpan::new(Some(fs), 0, text.len());
        let comment = CssNode::Comment(CssComment::new(text.to_string(), span));
        let stylesheet = CssStylesheet::new(vec![comment], span);

        let with = SerializeOptions {
            source_map: true,
            include_source_map_sources: true,
            ..SerializeOptions::default()
        };
        let result = serialize(&stylesheet, &with).unwrap();
        let with_sources = result.source_map.unwrap();
        assert_eq!(with_sources.sources_content.len(), 1);

        let without = SerializeOptions {
            source_map: true,
            ..SerializeOptions::default()
        };
        let result = serialize(&stylesheet, &without).unwrap();
        let without_sources = result.source_map.unwrap();
        assert!(without_sources.sources_content.is_empty());
    }

    #[test]
    fn test_serialize_compressed() {
        let stylesheet = stylesheet_with_comment("/* hello */");
        let opts = SerializeOptions {
            style: OutputStyle::Compressed,
            ..SerializeOptions::default()
        };
        let result = serialize(&stylesheet, &opts).unwrap();
        // Unpreserved comments are excluded in compressed mode
        assert_eq!(result.css.trim(), "");
    }

    #[test]
    fn test_serialize_expanded_comment() {
        let stylesheet = stylesheet_with_comment("/* hello */");
        let result = serialize(&stylesheet, &SerializeOptions::default()).unwrap();
        assert!(result.css.contains("/* hello */"));
    }

    #[test]
    fn test_line_feed_consts() {
        // The four node-sass linefeeds (adapter-mapped by exact bytes).
        assert_eq!((LINE_FEED_LF.name, LINE_FEED_LF.text), ("lf", "\n"));
        assert_eq!((LINE_FEED_CRLF.name, LINE_FEED_CRLF.text), ("crlf", "\r\n"));
        assert_eq!((LINE_FEED_CR.name, LINE_FEED_CR.text), ("cr", "\r"));
        assert_eq!((LINE_FEED_LFCR.name, LINE_FEED_LFCR.text), ("lfcr", "\n\r"));
    }

    #[test]
    fn test_serialize_charset_expanded() {
        // Non-ASCII chars in CSS output should trigger @charset
        // Use a unicode character directly in the comment
        let stylesheet = stylesheet_with_comment("/* café */");
        let result = serialize(&stylesheet, &SerializeOptions::default()).unwrap();
        assert!(
            result.css.starts_with("@charset \"UTF-8\";\n"),
            "expanded non-ASCII CSS should have @charset, got: {:?}",
            result.css
        );
    }

    #[test]
    fn test_serialize_charset_compressed() {
        let stylesheet = CssStylesheet::new(vec![preserved_comment("/* café */")], BOGUS_SPAN);
        let opts = SerializeOptions {
            style: OutputStyle::Compressed,
            ..SerializeOptions::default()
        };
        let result = serialize(&stylesheet, &opts).unwrap();
        assert!(
            result.css.starts_with("\u{FEFF}"),
            "compressed non-ASCII CSS should have BOM, got: {:?}",
            result.css
        );
    }

    #[test]
    fn test_serialize_charset_all_ascii() {
        let stylesheet = stylesheet_with_comment("/* hello */");
        let result = serialize(&stylesheet, &SerializeOptions::default()).unwrap();
        assert!(
            !result.css.starts_with("@charset"),
            "all-ASCII CSS should not have @charset, got: {:?}",
            result.css
        );
    }

    #[test]
    fn test_serialize_charset_disabled() {
        let stylesheet = stylesheet_with_comment("/* café */");
        let opts = SerializeOptions {
            charset: false,
            ..SerializeOptions::default()
        };
        let result = serialize(&stylesheet, &opts).unwrap();
        assert!(
            !result.css.starts_with("@charset") && !result.css.starts_with("\u{FEFF}"),
            "charset=false should suppress prefix, got: {:?}",
            result.css
        );
    }

    #[test]
    fn test_serialize_indent_width_too_large() {
        let opts = SerializeOptions {
            indent_width: 11,
            ..SerializeOptions::default()
        };
        let err = serialize(&empty_stylesheet(), &opts).unwrap_err();
        match *err {
            SassError::Script {
                message,
                argument_name,
            } => {
                assert_eq!(message, "must be between 0 and 10, was 11");
                assert_eq!(argument_name, Some("indentWidth".into()));
            }
            _ => panic!("expected SassError::Script, got {:?}", err),
        }
    }

    #[test]
    fn test_serialize_indent_width_max_valid() {
        let opts = SerializeOptions {
            indent_width: 10,
            ..SerializeOptions::default()
        };
        let result = serialize(&empty_stylesheet(), &opts).unwrap();
        assert_eq!(result.css, "");
    }

    // Libsass NESTED goldens: expected bytes captured from upstream
    // `sassc -t nested` (libsass 3.6.6), hardcoded here — never recomputed
    // from the serializer under test. `sassc` appends a trailing newline on
    // stdout; `serialize()` emits none, so goldens exclude it.

    #[rust_sass_macros::maybe_test]
    async fn test_nested_basic_glued_closer() {
        // Canonical NESTED shape: trailing `;` kept, closer glued (`; }`).
        // (Bind before assert: `.await` inside macro args is invisible to
        // the sync-build await-stripper.)
        let css = compile_nested("$x:1+2;a{b:$x}").await;
        assert_eq!(css, "a {\n  b: 3; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_sibling_blank_line() {
        // Blank line between top-level blocks (reuses `is_group_end`).
        let css = compile_nested("a{b:c}d{e:f}").await;
        assert_eq!(css, "a {\n  b: c; }\n\nd {\n  e: f; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_charset_expanded_prefix() {
        // Nested joins Expanded for the charset prefix (BOM is compressed-only).
        let arena = bumpalo::Bump::new();
        let opts = CompileOptions {
            style: OutputStyle::Nested,
            ..CompileOptions::new(&arena)
        };
        let result = compile_string("a{content:\"café\"}", test_io(), opts, &arena).await;
        let css = result.unwrap().css().to_string();
        assert!(
            css.starts_with("@charset \"UTF-8\";\n"),
            "nested non-ASCII CSS should have @charset, got: {css:?}"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_two_bubbled_medias_blank_separated() {
        // Two indent-0 bubbled medias blank-separate (media closers
        // schedule it); the empty origin rule is dropped.
        let css = compile_nested("a{@media x{b:c};@media y{d:e}}").await;
        assert_eq!(
            css,
            "@media x {\n  a {\n    b: c; } }\n\n@media y {\n  a {\n    d: e; } }"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_keyframes_blank_separated() {
        // Keyframes bubbles carry no tabs but blank-separate like rules.
        let css = compile_nested("a{b:c;@keyframes k{from{x:y}}}").await;
        assert_eq!(
            css,
            "a {\n  b: c; }\n\n@keyframes k {\n  from {\n    x: y; } }"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_childless_at_rule_blank_separated() {
        // A childless at-rule blank-separates from the preceding rule but
        // schedules none after itself.
        let css = compile_nested("a{b:c}@foo bar;c{d:e}").await;
        assert_eq!(css, "a {\n  b: c; }\n\n@foo bar;\nc {\n  d: e; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_rule_after_media_blank_separated() {
        let css = compile_nested("@media x{a{b:c}}d{e:f}").await;
        assert_eq!(css, "@media x {\n  a {\n    b: c; } }\n\nd {\n  e: f; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_rule_then_comment_blank_separated() {
        // Comments never collapse the pending blank (but never schedule
        // one either — see the loud-comment golden for the reverse).
        let css = compile_nested("a{b:c}/* hi */").await;
        assert_eq!(css, "a {\n  b: c; }\n\n/* hi */");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_props_empty_rule_no_tabs() {
        // A props-less origin rule contributes nothing: merged rule and
        // bubble both render at indent 0 (blank-separated).
        let css = compile_nested("a{d{e:f};@media x{g:h}}").await;
        assert_eq!(css, "a d {\n  e: f; }\n\n@media x {\n  a {\n    g: h; } }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_compressed_closer_unchanged() {
        // Regression pin: the touched closer arm leaves compressed alone.
        let arena = bumpalo::Bump::new();
        let opts = CompileOptions {
            style: OutputStyle::Compressed,
            ..CompileOptions::new(&arena)
        };
        let result = compile_string("a{b:c}", test_io(), opts, &arena).await;
        let css = result.unwrap().css().to_string();
        assert_eq!(css, "a{b:c}");
    }
}
