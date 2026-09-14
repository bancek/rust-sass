// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

//! CSS tree serialization: one `visit_*` per [`CssNode`] plus declaration
//! re-indentation, comment/indent scanning, and media-query helpers (Dart's
//! `visitCss*` family, `_writeFoldedValue`, `_writeReindentedValue`,
//! `_minimumIndentation`, `_writeWithIndent`, `_writeImportUrl`,
//! `_visitMediaQuery`).
//!
//! Thin `CssVisitor` wrappers delegate to free `visit_*_impl` functions so
//! [`visit_css_node_impl`] can dispatch by `match` where `accept()` is
//! unavailable.

// dart-source: lib/src/visitor/serialize.dart (visitCssStylesheet, visitCssComment, visitCssAtRule, visitCssMediaRule, visitCssImport, _writeImportUrl, visitCssKeyframeBlock, _visitMediaQuery, visitCssStyleRule, visitCssSupportsRule, visitCssDeclaration, _writeFoldedValue, _writeReindentedValue, _minimumIndentation, _writeWithIndent)
// go-source: go/value/visitor_css.go

use crate::serialize::selector::visit_selector_list_impl;
use crate::serialize::string::visit_quoted_string;
use crate::serialize::value::visit_value_impl;
use crate::serialize::visit_children;
use crate::serialize::visit_children_special;
use crate::util::trim_ascii::trim_ascii_right;
use std::borrow::Cow;
use std::fmt::Write;

use crate::ast::css::at_rule::CssAtRule;
use crate::ast::css::comment::CssComment;
use crate::ast::css::declaration::CssDeclaration;
use crate::ast::css::import::CssImport;
use crate::ast::css::keyframe_block::CssKeyframeBlock;
use crate::ast::css::media_query::CssMediaQuery;
use crate::ast::css::media_rule::CssMediaRule;
use crate::ast::css::node::CssNode;
use crate::ast::css::style_rule::CssStyleRule;
use crate::ast::css::stylesheet::CssStylesheet;
use crate::ast::css::supports_rule::CssSupportsRule;
use crate::ast::css::visitor::CssVisitor;
use crate::common::ast_node::AstNode;
use crate::common::exception::SassError;
use crate::common::file_span::SourceLocation;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::common::span::Span;
use crate::common::SassResult;
use crate::source_map_buffer::SourceMapBuffer;
use crate::value::ValueKind;

use crate::serialize::{
    for_node, is_invisible, requires_semicolon, write_between, OutputStyle, SerializeState,
    SerializeVisitor,
};

impl<'parse> CssVisitor<'parse> for SerializeVisitor<'parse> {
    type Output = ();

    fn visit_css_stylesheet(&mut self, node: &CssStylesheet<'parse>) -> SassResult<()> {
        visit_css_stylesheet_impl(&mut self.buffer, &mut self.inner, node)
    }
    fn visit_css_comment(&mut self, node: &CssComment<'parse>) -> SassResult<()> {
        visit_css_comment_impl(&mut self.buffer, &mut self.inner, node)
    }
    fn visit_css_at_rule(&mut self, node: &CssAtRule<'parse>) -> SassResult<()> {
        visit_css_at_rule_impl(&mut self.buffer, &mut self.inner, node)
    }
    fn visit_css_media_rule(&mut self, node: &CssMediaRule<'parse>) -> SassResult<()> {
        visit_css_media_rule_impl(&mut self.buffer, &mut self.inner, node)
    }
    fn visit_css_import(&mut self, node: &CssImport<'parse>) -> SassResult<()> {
        visit_css_import_impl(&mut self.buffer, &mut self.inner, node)
    }
    fn visit_css_keyframe_block(&mut self, node: &CssKeyframeBlock<'parse>) -> SassResult<()> {
        visit_css_keyframe_block_impl(&mut self.buffer, &mut self.inner, node)
    }
    fn visit_css_style_rule(&mut self, node: &CssStyleRule<'parse>) -> SassResult<()> {
        visit_css_style_rule_impl(&mut self.buffer, &mut self.inner, node)
    }
    fn visit_css_supports_rule(&mut self, node: &CssSupportsRule<'parse>) -> SassResult<()> {
        visit_css_supports_rule_impl(&mut self.buffer, &mut self.inner, node)
    }
    fn visit_css_declaration(&mut self, node: &CssDeclaration<'parse>) -> SassResult<()> {
        visit_css_declaration_impl(&mut self.buffer, &mut self.inner, node)
    }
}

/// Manual `match` dispatch over [`CssNode`] for buffer/state-only contexts
/// (Dart dispatches via `child.accept`; see the module docs).
pub(crate) fn visit_css_node_impl<'parse>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    node: &CssNode<'parse>,
) -> SassResult<()> {
    match node {
        CssNode::Stylesheet(s) => visit_css_stylesheet_impl(buf, state, s),
        CssNode::StyleRule(sr) => visit_css_style_rule_impl(buf, state, sr),
        CssNode::AtRule(r) => visit_css_at_rule_impl(buf, state, r),
        CssNode::Comment(c) => visit_css_comment_impl(buf, state, c),
        CssNode::Declaration(d) => visit_css_declaration_impl(buf, state, d),
        CssNode::Import(i) => visit_css_import_impl(buf, state, i),
        CssNode::KeyframeBlock(k) => visit_css_keyframe_block_impl(buf, state, k),
        CssNode::MediaRule(m) => visit_css_media_rule_impl(buf, state, m),
        CssNode::SupportsRule(s) => visit_css_supports_rule_impl(buf, state, s),
    }
}

// Top-level children: `;` between semicolon-needing siblings, blank line
// after group ends (Expanded and Compact; Nested uses the libsass rule
// instead), trailing comments inline (Dart's `visitCssStylesheet`; Nested
// and Compact keep comments on their own lines like libsass).
fn visit_css_stylesheet_impl<'parse>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    node: &CssStylesheet<'parse>,
) -> SassResult<()> {
    let mut previous: Option<&CssNode<'parse>> = None;
    for child in &node.children {
        if is_invisible(state, child) {
            continue;
        }
        if let Some(prev) = previous {
            if requires_semicolon(prev) {
                buf.write_char(';').unwrap();
            }
            if is_trailing_comment(child, prev, state) {
                state.write_optional_space(buf);
            } else {
                state.write_line_feed(buf);
                match state.style {
                    OutputStyle::Compressed | OutputStyle::Expanded | OutputStyle::Compact => {
                        if prev.is_group_end() {
                            state.write_line_feed(buf);
                        }
                    }
                    OutputStyle::Nested => {
                        if nested_block_separated(prev, child) {
                            state.write_line_feed(buf);
                        }
                    }
                }
            }
        }
        previous = Some(child);
        visit_css_node_impl(buf, state, child)?;
    }
    if let Some(prev) = previous {
        if requires_semicolon(prev) && !matches!(state.style, OutputStyle::Compressed) {
            buf.write_char(';').unwrap();
        }
    }
    Ok(())
}

// Preserved (`/*!`) comments survive compression; source-map/-URL comments
// are always dropped; multi-line comments re-indent to the current level
// (Dart's `visitCssComment`). In COMPACT multi-line comments flatten to one
// line (libsass `comment_to_compact_string`, `util.cpp:224-251`).
fn visit_css_comment_impl<'parse>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    node: &CssComment<'parse>,
) -> SassResult<()> {
    for_node(buf, state, node, |buf, state| {
        if matches!(state.style, OutputStyle::Compressed) && !node.is_preserved {
            return Ok(());
        }
        if node.text.starts_with("/*# sourceMappingURL=") || node.text.starts_with("/*# sourceURL=")
        {
            return Ok(());
        }
        if matches!(state.style, OutputStyle::Compact) && node.text.contains('\n') {
            write!(buf, "{}", comment_to_compact_string(&node.text)).unwrap();
        } else if let Some(min_indent) = minimum_indentation(&node.text) {
            let min_indent = std::cmp::min(min_indent, node.span.start_location().column as i32);
            state.write_indentation(buf);
            write_with_indent(buf, state, &node.text, min_indent);
        } else {
            state.write_indentation(buf);
            write!(buf, "{}", node.text).unwrap();
        }
        Ok(())
    })
}

/// Flattens a multi-line loud comment to one line: after `\n`, swallow
/// ` \t*`, re-emit the first real char prefixed with one space
/// (special-casing `*/`). Single-line input passes through unchanged — the
/// flattened text is returned only when spaces/tabs were swallowed, so a
/// bare `/* a\nb */` keeps its newline (libsass `comment_to_compact_string`,
/// `util.cpp:224-251`).
fn comment_to_compact_string(text: &str) -> Cow<'_, str> {
    let mut flattened = String::new();
    let mut swallowed = 0usize;
    let mut prev = '\0';
    let mut clean = false;
    for ch in text.chars() {
        if clean {
            if ch == '\n' {
                swallowed = 0;
            } else if ch == '\t' || ch == ' ' {
                swallowed += 1;
            } else if ch == '*' {
                // Swallowed without counting (matches libsass).
            } else {
                clean = false;
                flattened.push(' ');
                if prev == '*' && ch == '/' {
                    flattened.push_str("*/");
                } else {
                    flattened.push(ch);
                }
            }
        } else if ch == '\n' {
            clean = true;
        } else {
            flattened.push(ch);
        }
        prev = ch;
    }
    if swallowed > 0 {
        Cow::Owned(flattened)
    } else {
        Cow::Borrowed(text)
    }
}

// `@name value` with the header source-mapped, then children unless
// childless (Dart's `visitCssAtRule`).
fn visit_css_at_rule_impl<'parse>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    node: &CssAtRule<'parse>,
) -> SassResult<()> {
    state.write_indentation(buf);
    for_node(buf, state, node, |buf, _state| {
        buf.write_char('@').unwrap();
        write!(buf, "{}", node.name.value).unwrap();
        if let Some(ref value) = node.value {
            buf.write_char(' ').unwrap();
            write!(buf, "{}", value.value).unwrap();
        }
        Ok(())
    })?;
    if !node.childless {
        state.write_optional_space(buf);
        // Libsass splits at-rule children across lines (`output.cpp:293`)
        // except `@font-face` (declarations stay space-joined like style
        // rules); in COMPACT that is the special linefeed, elsewhere both
        // helpers emit the same LF.
        if node.name.value.as_str() == "font-face" {
            visit_children(buf, state, &node.children, node.span)?;
        } else {
            visit_children_special(buf, state, &node.children, node.span)?;
        }
    }
    Ok(())
}

// `@media` keeps the space before queries except in compressed mode with a
// bare query list (Dart's `visitCssMediaRule`).
fn visit_css_media_rule_impl<'parse>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    node: &CssMediaRule<'parse>,
) -> SassResult<()> {
    // libsass NESTED tabs: shift the whole subtree like `indentation +=
    // tabs` bracketing (safe without a guard: any `?` unwinds the whole
    // serialization, discarding the state).
    let extra = state.nested_tabs(node.tabs);
    state.indentation = state.indentation.saturating_add(extra);
    state.write_indentation(buf);
    for_node(buf, state, node, |buf, state| {
        buf.write_str("@media").unwrap();
        let first_query = &node.queries[0];
        if matches!(
            state.style,
            OutputStyle::Expanded | OutputStyle::Nested | OutputStyle::Compact
        ) || first_query.modifier.is_some()
            || first_query.media_type.is_some()
            || (first_query.conditions.len() == 1 && first_query.conditions[0].starts_with("(not "))
        {
            buf.write_char(' ').unwrap();
        }
        let sep = state.comma_sep();
        write_between(buf, &node.queries, sep, |buf, q| {
            visit_media_query(buf, state, q)
        })?;
        Ok(())
    })?;
    state.write_optional_space(buf);
    visit_children(buf, state, &node.children, node.span)?;
    state.indentation = state.indentation.saturating_sub(extra);
    Ok(())
}

// `@import url modifiers` (Dart's `visitCssImport`).
fn visit_css_import_impl<'parse>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    node: &CssImport<'parse>,
) -> SassResult<()> {
    state.write_indentation(buf);
    for_node(buf, state, node, |buf, state| {
        buf.write_str("@import").unwrap();
        state.write_optional_space(buf);
        for_node(buf, state, &node.url, |buf, state| {
            write_import_url(buf, state, &node.url.value);
            Ok(())
        })?;
        if let Some(ref modifiers) = node.modifiers {
            state.write_optional_space(buf);
            write!(buf, "{}", modifiers.value).unwrap();
        }
        Ok(())
    })
}

// Comma-separated keyframe selectors plus block (Dart's `visitCssKeyframeBlock`).
// Children split across lines like at-rule children (libsass
// `output.cpp:197`); in COMPACT that is the special linefeed.
fn visit_css_keyframe_block_impl<'parse>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    node: &CssKeyframeBlock<'parse>,
) -> SassResult<()> {
    state.write_indentation(buf);
    for_node(buf, state, &node.selector, |buf, state| {
        let sep = state.comma_sep();
        write_between(buf, node.selector.value.as_slice(), sep, |buf, s| {
            write!(buf, "{}", s).unwrap();
            Ok(())
        })
    })?;
    state.write_optional_space(buf);
    visit_children_special(buf, state, &node.children, node.span)?;
    Ok(())
}

// Selector source-mapped as one span, then the rule block
// (Dart's `visitCssStyleRule`).
fn visit_css_style_rule_impl<'parse>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    node: &CssStyleRule<'parse>,
) -> SassResult<()> {
    // libsass NESTED tabs: shift the whole subtree like `indentation +=
    // tabs` bracketing (safe without a guard: any `?` unwinds the whole
    // serialization, discarding the state).
    let extra = state.nested_tabs(node.tabs);
    state.indentation = state.indentation.saturating_add(extra);
    // libsass `/* line N, path */` source comments (`output.cpp:137-144`):
    // 1-based line; filesystem path for `file:` URLs; `"stdin"` when the
    // span has no URL (data compiles mirror libsass's default input path).
    // File-entry paths stay absolute (the serializer has no CWD to
    // relativize against — untested by any gate, documented). Emitted
    // after the tabs shift so the comment shares the rule's indentation
    // (upstream appends it post-accumulation).
    if state.source_comments {
        let span = node.span()?;
        let loc = span.start_location();
        let path = match span.source_url() {
            Some(url) if url.is_file() => {
                url.to_file_path_string().unwrap_or_else(|| url.to_string())
            }
            Some(url) => url.to_string(),
            None => "stdin".to_string(),
        };
        state.write_indentation(buf);
        write!(buf, "/* line {}, {} */", loc.line + 1, path).unwrap();
        state.write_line_feed(buf);
    }
    state.write_indentation(buf);
    let span = node.selector.span()?;
    buf.for_span(&span, |buf| {
        visit_selector_list_impl(buf, state, &node.selector)
    })?;
    state.write_optional_space(buf);
    visit_children(buf, state, &node.children, node.span()?)?;
    state.indentation = state.indentation.saturating_sub(extra);
    Ok(())
}

// `@supports` keeps its space unless compressed output starts with `(`
// (Dart's `visitCssSupportsRule`).
fn visit_css_supports_rule_impl<'parse>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    node: &CssSupportsRule<'parse>,
) -> SassResult<()> {
    // libsass NESTED tabs: shift the whole subtree like `indentation +=
    // tabs` bracketing (safe without a guard: any `?` unwinds the whole
    // serialization, discarding the state).
    let extra = state.nested_tabs(node.tabs);
    state.indentation = state.indentation.saturating_add(extra);
    state.write_indentation(buf);
    for_node(buf, state, node, |buf, state| {
        buf.write_str("@supports").unwrap();
        if !(matches!(state.style, OutputStyle::Compressed)
            && node.condition.value.as_bytes().first() == Some(&b'('))
        {
            buf.write_char(' ').unwrap();
        }
        write!(buf, "{}", node.condition.value).unwrap();
        Ok(())
    })?;
    state.write_optional_space(buf);
    // Supports children split across lines (libsass `output.cpp:230`); in
    // COMPACT that is the special linefeed, elsewhere identical to the
    // plain block break.
    visit_children_special(buf, state, &node.children, node.span)?;
    state.indentation = state.indentation.saturating_sub(extra);
    Ok(())
}

// `name: value`. Values parsed as plain CSS fold/re-indent raw text;
// Sass-script values serialize as values (script errors rethrown as runtime
// errors at the value span; Dart's `visitCssDeclaration`).
fn visit_css_declaration_impl<'parse>(
    buf: &mut SourceMapBuffer<'parse>,
    state: &mut SerializeState,
    node: &CssDeclaration<'parse>,
) -> SassResult<()> {
    state.write_indentation(buf);
    let name_span = node.name.span()?;
    buf.for_span(&name_span, |buf| {
        write!(buf, "{}", node.name.value).unwrap();
        Ok(())
    })?;
    buf.write_char(':').unwrap();
    if !node.parsed_as_sass_script {
        for_node(buf, state, &node.value, |buf, state| match state.style {
            OutputStyle::Compressed => write_folded_value(buf, node),
            OutputStyle::Expanded | OutputStyle::Nested | OutputStyle::Compact => {
                write_reindented_value(buf, state, node)
            }
        })?;
    } else {
        state.write_optional_space(buf);
        match buf.for_span(&node.value_span_for_map, |buf| {
            visit_value_impl(buf, state, &node.value.value)
        }) {
            Err(e) if matches!(*e, SassError::Script { .. }) => {
                let SassError::Script { message, .. } = *e else {
                    unreachable!()
                };
                let val_span = node.value.span()?;
                let span_ctx =
                    SourceSpanWithContext::from_file_span(&val_span).unwrap_or_else(|_| {
                        let loc = SourceLocation {
                            offset: 0,
                            line: 0,
                            column: 0,
                        };
                        SourceSpanWithContext::new(loc, loc, String::new(), String::new(), None)
                            .unwrap()
                    });
                Err(Box::new(SassError::Runtime {
                    message,
                    span: span_ctx,
                    trace: Default::default(),
                    cause: None,
                    loaded_urls: vec![],
                }))
            }
            other => other,
        }?;
    }
    Ok(())
}

// One media query: optional modifier/type plus `and`/`or`-joined conditions,
// with `(not X)` normalized to `not X` (Dart's `_visitMediaQuery`).
fn visit_media_query(
    buf: &mut SourceMapBuffer<'_>,
    state: &SerializeState,
    query: &CssMediaQuery,
) -> SassResult<()> {
    if let Some(ref modifier) = query.modifier {
        write!(buf, "{}", modifier).unwrap();
        buf.write_char(' ').unwrap();
    }
    if let Some(ref media_type) = query.media_type {
        write!(buf, "{}", media_type).unwrap();
        if !query.conditions.is_empty() {
            buf.write_str(" and ").unwrap();
        }
    }
    if let Some((first, rest)) = query.conditions.split_first() {
        if first.starts_with("(not ") {
            buf.write_str("not ").unwrap();
            let inner = &first["(not ".len()..first.len() - 1];
            write!(buf, "{}", inner).unwrap();
        } else {
            let operator = if query.conjunction { "and" } else { "or" };
            write!(buf, "{}", first).unwrap();
            let sep = match state.style {
                OutputStyle::Compressed => format!("{} ", operator),
                OutputStyle::Expanded | OutputStyle::Nested | OutputStyle::Compact => {
                    format!(" {} ", operator)
                }
            };
            for condition in rest {
                write!(buf, "{}", sep).unwrap();
                write!(buf, "{}", condition).unwrap();
            }
        }
    }
    Ok(())
}

/// Compressed `url(...)` unwraps to a bare (possibly re-quoted) URL so the
/// space after `@import` can go; other URLs pass through (Dart's `_writeImportUrl`).
fn write_import_url(buf: &mut SourceMapBuffer<'_>, state: &SerializeState, url: &str) {
    if !matches!(state.style, OutputStyle::Compressed)
        || url.as_bytes().first().is_none_or(|&b| b != b'u')
    {
        write!(buf, "{}", url).unwrap();
        return;
    }
    let url_contents = &url[4..url.len() - 1];
    if let Some(first_byte) = url_contents.as_bytes().first() {
        if *first_byte == b'\'' || *first_byte == b'"' {
            write!(buf, "{}", url_contents).unwrap();
        } else {
            visit_quoted_string(buf, state, url_contents);
        }
    }
}

// Compressed custom-property value: newlines fold to one space, surrounding
// whitespace collapses (Dart's `_writeFoldedValue`).
fn write_folded_value(buf: &mut SourceMapBuffer<'_>, node: &CssDeclaration<'_>) -> SassResult<()> {
    if let ValueKind::String(s) = &*node.value.value {
        let mut chars = s.text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '\n' {
                buf.write_char(ch).unwrap();
                continue;
            }
            buf.write_char(' ').unwrap();
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
        }
    } else {
        write!(buf, "{}", &*node.value.value).unwrap();
    }
    Ok(())
}

// Expanded custom-property value: single-line passes through, trailing
// blank lines compress to one space, otherwise re-indent to the declaration
// column (Dart's `_writeReindentedValue`).
fn write_reindented_value(
    buf: &mut SourceMapBuffer<'_>,
    state: &mut SerializeState,
    node: &CssDeclaration<'_>,
) -> SassResult<()> {
    if let ValueKind::String(s) = &*node.value.value {
        let value = &s.text;
        match minimum_indentation(value) {
            None => {
                write!(buf, "{}", value).unwrap();
            }
            Some(-1) => {
                let trimmed = trim_ascii_right(value, true);
                write!(buf, "{}", trimmed).unwrap();
                buf.write_char(' ').unwrap();
            }
            Some(min_indent) => {
                let min_indentation =
                    std::cmp::min(min_indent, node.name.span()?.start_location().column as i32);
                write_with_indent(buf, state, value, min_indentation);
            }
        }
    } else {
        write!(buf, "{}", &*node.value.value).unwrap();
    }
    Ok(())
}

/// Least-indented non-empty line after the first (`None` = no newlines, `-1`
/// = newlines but nothing indented); Dart's `_minimumIndentation`.
fn minimum_indentation(text: &str) -> Option<i32> {
    // Direct port of Dart's SerializeSerializer._minimumIndentation.
    let mut chars = text.char_indices().peekable();

    // Skip to the first newline.
    let mut hit_newline = false;
    for (_, c) in chars.by_ref() {
        if c == '\n' {
            hit_newline = true;
            break;
        }
    }
    if !hit_newline {
        return None;
    }

    let mut min: Option<i32> = None;
    while let Some(&(_, c)) = chars.peek() {
        if c == ' ' || c == '\t' {
            chars.next();
            continue;
        }
        match c {
            '\n' => {
                chars.next();
            }
            _ => {
                // Column of the first non-whitespace character on this line.
                let line_start = chars.peek().map_or(text.len(), |&(i, _)| i);
                let column =
                    (line_start - (text[..line_start].rfind('\n').map_or(0, |p| p + 1))) as i32;
                min = Some(min.map_or(column, |m: i32| m.min(column)));
                // Skip to the end of the line.
                for (_, c2) in chars.by_ref() {
                    if c2 == '\n' {
                        break;
                    }
                }
            }
        }
    }

    min.or(Some(-1))
}

/// LineScanner-equivalent over `char`s for `_writeWithIndent`.
struct CharScanner<'a> {
    text: &'a str,
    offset: usize,
}

impl<'a> CharScanner<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, offset: 0 }
    }

    fn position(&self) -> usize {
        self.offset
    }

    fn is_done(&self) -> bool {
        self.offset >= self.text.len()
    }

    fn read_char(&mut self) -> Option<char> {
        let c = self.text[self.offset..].chars().next()?;
        self.offset += c.len_utf8();
        Some(c)
    }

    /// Steps back over the last-read char. Dart's `LineScanner` (UTF-16 units)
    /// never splits a code point; the naive `offset -= 1` panics mid-codepoint
    /// on multibyte text. Track the previous boundary instead.
    fn unread_char(&mut self, c: char) {
        self.offset -= c.len_utf8();
    }

    fn substring(&self, start: usize) -> &'a str {
        &self.text[start..self.offset]
    }
}

/// Replaces `minimum_indentation` columns with the current indent on every
/// non-empty line after the first; trailing blank lines become one space
/// (custom properties are whitespace-sensitive). Dart's `_writeWithIndent`.
fn write_with_indent(
    buf: &mut SourceMapBuffer<'_>,
    state: &mut SerializeState,
    text: &str,
    minimum_indentation: i32,
) {
    // Direct port of Dart's SerializeSerializer._writeWithIndent.
    let text = text.replace("\r\n", "\n");
    let mut scanner = CharScanner::new(&text);

    // Write the first line as-is.
    while !scanner.is_done() {
        let next = scanner.read_char();
        if next == Some('\n') {
            break;
        }
        if let Some(c) = next {
            buf.write_char(c).unwrap();
        }
    }

    loop {
        // Scan forward until we hit non-whitespace or the end of [text].
        let mut line_start = scanner.position();
        let mut newlines = 1usize;
        loop {
            if scanner.is_done() {
                // Preserve the fact that whitespace exists (custom properties).
                buf.write_char(' ').unwrap();
                return;
            }
            match scanner.read_char() {
                Some(' ') | Some('\t') => continue,
                Some('\n') => {
                    line_start = scanner.position();
                    newlines += 1;
                }
                Some(c) => {
                    scanner.unread_char(c);
                    break;
                }
                None => {
                    break;
                }
            }
        }

        for _ in 0..newlines {
            buf.write_char('\n').unwrap();
        }
        // Reindented (not suppressed) indentation: declaration values share
        // the Expanded path in COMPACT, so continuation lines must match
        // Expanded byte-for-byte even though `write_indentation` is a no-op
        // there (values are Dart-governed content, not layout).
        state.write_reindented_indentation(buf);
        let excess_start = line_start + minimum_indentation.max(0) as usize;
        write!(buf, "{}", scanner.substring(excess_start)).unwrap();

        // Scan and write until we hit a newline or the end of [text].
        loop {
            if scanner.is_done() {
                return;
            }
            let next = scanner.read_char();
            if next == Some('\n') {
                break;
            }
            if let Some(c) = next {
                buf.write_char(c).unwrap();
            }
        }
    }
}

/// Whether libsass NESTED emits a blank line between top-level `prev` and
/// `cur`: `prev` closed a block at indent 0 — style and supports rules
/// always balance their tabs around the closer, media rules only with
/// `tabs == 0`, at-rules with children, never comments, declarations,
/// imports, or childless at-rules (libsass `Emitter::append_scope_closer`)
/// — and `cur` renders at indent 0 (hoisted blocks with `tabs > 0` collapse
/// the pending linefeed; comments never collapse it).
fn nested_block_separated(prev: &CssNode<'_>, cur: &CssNode<'_>) -> bool {
    let triggers = match prev {
        CssNode::StyleRule(_) | CssNode::SupportsRule(_) => true,
        CssNode::MediaRule(m) => m.tabs == 0,
        CssNode::AtRule(r) => !r.childless,
        CssNode::Comment(_)
        | CssNode::Declaration(_)
        | CssNode::Import(_)
        | CssNode::KeyframeBlock(_)
        | CssNode::Stylesheet(_) => false,
    };
    if !triggers {
        return false;
    }
    match cur {
        CssNode::StyleRule(s) => s.tabs == 0,
        CssNode::MediaRule(m) => m.tabs == 0,
        CssNode::SupportsRule(s) => s.tabs == 0,
        _ => true,
    }
}

fn is_trailing_comment(node: &CssNode<'_>, previous: &CssNode<'_>, state: &SerializeState) -> bool {
    if matches!(
        state.style,
        OutputStyle::Compressed | OutputStyle::Nested | OutputStyle::Compact
    ) {
        return false;
    }
    let CssNode::Comment(_) = node else {
        return false;
    };
    let Ok(node_span) = node.span() else {
        return false;
    };
    let Ok(previous_span) = previous.span() else {
        return false;
    };
    if node_span.source_url() != previous_span.source_url() {
        return false;
    }

    if !previous_span
        .contains(&Span::File(node_span))
        .unwrap_or(false)
    {
        return node_span.start_location().line == previous_span.end_location().line;
    }

    let node_start = node_span.start_location();
    let previous_start = previous_span.start_location();
    let search_from = node_start.offset.saturating_sub(previous_start.offset + 1);
    if search_from == 0 && node_start.offset <= previous_start.offset + 1 {
        return false;
    }
    let search_text = previous_span.text();
    if search_text.is_empty() {
        return false;
    }
    let search_slice = &search_text[..search_from.min(search_text.len())];
    let end_offset = search_slice.rfind('{').map_or(0, |i| i + 1);
    let end_line = previous_start.line
        + search_slice[..end_offset]
            .chars()
            .filter(|&c| c == '\n')
            .count();
    node_start.line == end_line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{compile_string, CompileOptions};
    use crate::io::{DefaultIo, Io};
    use crate::serialize::LINE_FEED_LF;
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

    #[rust_sass_macros::maybe_async]
    async fn compile_compact(source: &str) -> String {
        let arena = bumpalo::Bump::new();
        let opts = CompileOptions {
            style: OutputStyle::Compact,
            ..CompileOptions::new(&arena)
        };
        compile_string(source, test_io(), opts, &arena)
            .await
            .unwrap()
            .css()
            .to_string()
    }

    #[rust_sass_macros::maybe_test]
    async fn test_write_with_indent_multibyte() {
        // `write_with_indent` panicked mid-codepoint
        // (`offset -= 1` unread lands inside a multibyte char). Dart's
        // LineScanner never splits a code point. Repro: loud comment whose
        // second line starts with a multibyte char after indentation.
        let mut state = SerializeState {
            indentation: 1,
            style: OutputStyle::Expanded,
            source_comments: false,
            inspect: false,
            quote: true,
            line_feed: LINE_FEED_LF,
            indent_char: ' ',
            indent_width: 2,
        };
        let mut buf = SourceMapBuffer::new_plain();
        write_with_indent(&mut buf, &mut state, "/* a\n   é */", 0);
        assert_eq!(buf.into_string(), "/* a\n     é */");
    }

    // Libsass NESTED goldens for block-level constructs: expected bytes
    // captured from upstream `sassc -t nested` (libsass 3.6.6), hardcoded
    // here. `sassc`'s trailing stdout newline is excluded (the serializer
    // emits none).

    // (Bind before assert throughout: `.await` inside macro args is
    // invisible to the sync-build await-stripper.)

    #[rust_sass_macros::maybe_test]
    async fn test_nested_two_level_indent() {
        // Nested rules flatten at eval; output depth indents the body.
        let css = compile_nested("a{b{c{d:e}}}").await;
        assert_eq!(css, "a b c {\n  d: e; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_partial_merge_no_tabs() {
        // Merged rule from a props-less origin collects nothing (the
        // origin rule contributes no `+1`, and the merged node is the
        // inner frame's own).
        let css = compile_nested("x{a{b:c}}").await;
        assert_eq!(css, "x a {\n  b: c; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_media_rule() {
        let css = compile_nested("@media screen{a{b:c}}").await;
        assert_eq!(css, "@media screen {\n  a {\n    b: c; } }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_supports_rule() {
        let css = compile_nested("@supports (display:grid){a{b:c}}").await;
        assert_eq!(css, "@supports (display: grid) {\n  a {\n    b: c; } }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_propset() {
        let css = compile_nested("a{b:{c:d}}").await;
        assert_eq!(css, "a {\n  b-c: d; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_loud_comment_preserved() {
        let css = compile_nested("/* hi */a{b:c}").await;
        assert_eq!(css, "/* hi */\na {\n  b: c; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_at_root_hoist_keeps_source_indent() {
        // Hoisted `@at-root` rule keeps its source indent (libsass tabs).
        let css = compile_nested("a{b:c;@at-root d{e:f}}").await;
        assert_eq!(css, "a {\n  b: c; }\n  d {\n    e: f; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_media_bubble_with_declarations() {
        // Bubbled `@media` beside declarations keeps the source indent.
        let css = compile_nested("a{b:c;@media screen{d:e}}").await;
        assert_eq!(
            css,
            "a {\n  b: c; }\n  @media screen {\n    a {\n      d: e; } }"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_supports_bubble_then_rule() {
        // Supports bubbles like media; the following top-level rule is
        // blank-separated (supports closers always schedule it).
        let css = compile_nested("a{b:c;@supports (display:grid){d:e}}f{g:h}").await;
        assert_eq!(
            css,
            "a {\n  b: c; }\n  @supports (display: grid) {\n    a {\n      d: e; } }\n\nf {\n  g: h; }"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_at_root_inside_bubbled_media() {
        // `@at-root` inside bubbled media stays in the media; only the
        // media collects the props-rule `+1` (media boundary blocks).
        let css = compile_nested("a{b:c;@media x{@at-root d{e:f}}}").await;
        assert_eq!(
            css,
            "a {\n  b: c; }\n  @media x {\n    d {\n      e: f; } }"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_merged_rule_and_bubbled_media_tabs() {
        // Merged nested rules collect `+1`; media collects from every
        // enclosing props rule (`+2` here).
        let css = compile_nested("a{b:c;d{e:f;@media x{g:h}}}").await;
        assert_eq!(
            css,
            "a {\n  b: c; }\n  a d {\n    e: f; }\n    @media x {\n      a d {\n        g: h; } }"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_transitive_at_root() {
        // Transitive `@at-root` hoisting accumulates through transparent
        // frames (`e` collects from both props-bearing ancestors).
        let css = compile_nested("a{b:c;@at-root d{x:y;@at-root e{f:g}}}").await;
        assert_eq!(
            css,
            "a {\n  b: c; }\n  d {\n    x: y; }\n    e {\n      f: g; }"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_comment_counts_as_props() {
        // A loud comment alone makes the rule props-bearing (libsass
        // `props` partition); comments render on their own lines.
        let css = compile_nested("a{/*c*/;@media x{d:e}}").await;
        assert_eq!(
            css,
            "a {\n  /*c*/ }\n  @media x {\n    a {\n      d: e; } }"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_trailing_comment_own_line() {
        // libsass never inlines trailing comments (unlike Dart/Expanded).
        let css = compile_nested("a{b:c;/*x*/d:e}").await;
        assert_eq!(css, "a {\n  b: c;\n  /*x*/\n  d: e; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_supports_inside_bubbled_media() {
        // Supports nested in bubbled media stays put (subtree exclusion):
        // the media collects `+1`, everything inside is structural.
        let css = compile_nested("a{b:c;@media x{d:e;@supports (a:b){f:g}}}").await;
        assert_eq!(
            css,
            "a {\n  b: c; }\n  @media x {\n    a {\n      d: e; }\n      @supports (a: b) {\n        a {\n          f: g; } } }"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_media_inside_unknown_at_rule() {
        // Media nested in an unknown at-rule stays nested (subtree
        // exclusion): no tabs, structural indent only; the at-rule
        // blank-separates like other blocks.
        let css = compile_nested("a{b:c;@foo{@media x{d:e}}}").await;
        assert_eq!(
            css,
            "a {\n  b: c; }\n\n@foo {\n  @media x {\n    a {\n      d: e; } } }"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_extend_merged_selectors() {
        // `@extend` merges selectors at the same depth: output depth
        // coincides with source depth, byte-identical to upstream.
        let css = compile_nested("a{b:c}d{@extend a;e:f}").await;
        assert_eq!(css, "a, d {\n  b: c; }\n\nd {\n  e: f; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_nested_bubbled_media_probe() {
        // Differential probe (upstream `sassc -t nested` emits exactly
        // this): bubbled `@media` hoisted above the rule still indents by
        // output depth, which coincides with source depth here.
        let css = compile_nested("a{@media screen{b:c}}").await;
        assert_eq!(css, "@media screen {\n  a {\n    b: c; } }");
    }

    // Libsass COMPACT goldens: one top-level block per line, declarations
    // space-separated, glued `; }` closer, blank line between top-level
    // blocks, zero indentation. Expected bytes captured from upstream
    // `sassc -t compact` (libsass 3.6.6-2-g9bb4); `sassc`'s trailing stdout
    // newline is excluded (the serializer emits none).

    #[rust_sass_macros::maybe_test]
    async fn test_compact_single_declaration() {
        let css = compile_compact("a{color:red}").await;
        assert_eq!(css, "a { color: red; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_multiple_declarations() {
        let css = compile_compact("a{x:y;z:w;q:v}").await;
        assert_eq!(css, "a { x: y; z: w; q: v; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_top_level_siblings_blank_separated() {
        let css = compile_compact("a{x:y}b{p:q}").await;
        assert_eq!(css, "a { x: y; }\n\nb { p: q; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_nested_rules_flatten_to_siblings() {
        // Dart content (nesting flattens) with COMPACT layout. The flattened
        // siblings share one evaluator group, so (like Expanded) they join
        // with a single LF — libsass blanks here, but grouping is
        // Dart-governed content, not layout (NESTED precedent).
        let css = compile_compact("a{x:y;b{p:q}c{r:s}}").await;
        assert_eq!(css, "a { x: y; }\na b { p: q; }\na c { r: s; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_media_children_space_joined() {
        let css = compile_compact("@media x{a{x:y}b{p:q}}").await;
        assert_eq!(css, "@media x { a { x: y; } b { p: q; } }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_supports_children_line_split() {
        // `@supports` children go through `append_special_linefeed`
        // (`output.cpp:230`): LF + raw indent in COMPACT.
        let css = compile_compact("@supports (display:grid){a{x:y}b{p:q}}").await;
        assert_eq!(
            css,
            "@supports (display: grid) { a { x: y; }\n  b { p: q; } }"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_supports_single_child() {
        let css = compile_compact("@supports (a:b){x{p:q}}").await;
        assert_eq!(css, "@supports (a: b) { x { p: q; } }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_keyframes_children_line_split() {
        let css = compile_compact("@keyframes foo{from{x:y}to{p:q}}").await;
        assert_eq!(css, "@keyframes foo { from { x: y; }\n  to { p: q; } }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_keyframe_block_decls_line_split() {
        // Declarations inside a keyframe block also line-split
        // (upstream emits LF + indent, unlike style-rule declarations).
        let css = compile_compact("@keyframes foo{from{x:y;z:w}}").await;
        assert_eq!(css, "@keyframes foo { from { x: y;\n    z: w; } }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_page_decls_line_split() {
        // Non-`@font-face` at-rule children line-split (`output.cpp:293`),
        // even declarations.
        let css = compile_compact("@page{x:y;z:w}").await;
        assert_eq!(css, "@page { x: y;\n  z: w; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_font_face_decls_space_joined() {
        // `@font-face` is exempt from the at-rule line split.
        let css = compile_compact("@font-face{font-family:x;src:url(y)}").await;
        assert_eq!(css, "@font-face { font-family: x; src: url(y); }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_multiline_comment_flattened() {
        let css = compile_compact("/* line1\n * line2\n * line3 */\na{x:y}").await;
        assert_eq!(css, "/* line1 line2 line3 */\na { x: y; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_nested_comment_flattened() {
        let css = compile_compact("a{/* a\n * b\n */x:y}").await;
        assert_eq!(css, "a { /* a b */ x: y; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_comment_newline_without_indent_passthrough() {
        // `comment_to_compact_string` returns the original when no
        // spaces/tabs were swallowed after `\n` (`util.cpp:249-250`).
        let css = compile_compact("/* a\nb */\na{x:y}").await;
        assert_eq!(css, "/* a\nb */\na { x: y; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_custom_property_keeps_newline() {
        // Declaration values join Expanded (reindented, not folded):
        // the embedded newline survives instead of collapsing to a space,
        // and continuation lines match Expanded byte-for-byte (the
        // reindent indent is emitted even though COMPACT suppresses all
        // other indentation).
        let css = compile_compact("a{\n  --x: foo\n    bar;\n  b: c;\n}").await;
        assert_eq!(css, "a { --x: foo\n    bar; b: c; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_multiline_selector_collapses() {
        let css = compile_compact("a,\nb{x:y}").await;
        assert_eq!(css, "a, b { x: y; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_charset_not_bom() {
        let css = compile_compact("a::before{content:\"é\"}").await;
        assert_eq!(css, "@charset \"UTF-8\";\na::before { content: \"é\"; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_trailing_comment_stays_inline() {
        let css = compile_compact("a{x:y;/* trail */}").await;
        assert_eq!(css, "a { x: y; /* trail */ }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_comment_between_decls() {
        let css = compile_compact("a{x:y;/* mid */z:w}").await;
        assert_eq!(css, "a { x: y; /* mid */ z: w; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_comment_only_stylesheet() {
        let css = compile_compact("/* hi */").await;
        assert_eq!(css, "/* hi */");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_empty_rule_emits_nothing() {
        let css = compile_compact("a{}").await;
        assert_eq!(css, "");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_rule_then_comment_blank_separated() {
        let css = compile_compact("a{x:y}/* after */").await;
        assert_eq!(css, "a { x: y; }\n\n/* after */");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_media_and_spacing() {
        let css = compile_compact("@media screen and (min-width:100px){a{x:y}}").await;
        assert_eq!(css, "@media screen and (min-width: 100px) { a { x: y; } }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_combinator_spacing() {
        let css = compile_compact("a>b+c~d{x:y}").await;
        assert_eq!(css, "a > b + c ~ d { x: y; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_number_spelling() {
        let css = compile_compact("a{x:0.5}").await;
        assert_eq!(css, "a { x: 0.5; }");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_compact_matches_expanded_for_custom_property() {
        // COMPACT shares the Expanded declaration-value path, so a
        // whitespace-sensitive custom property renders identically apart
        // from block layout (guards against folded-value regressions).
        let arena = bumpalo::Bump::new();
        let source = "a{\n  --x: foo\n    bar;\n  b: c;\n}";
        let expanded = {
            let opts = CompileOptions::new(&arena);
            compile_string(source, test_io(), opts, &arena)
                .await
                .unwrap()
                .css()
                .to_string()
        };
        let compact = compile_compact(source).await;
        assert!(compact.contains("--x: foo\n"));
        assert!(expanded.contains("--x: foo\n"));
    }

    // Direct unit tests for `comment_to_compact_string` (libsass
    // `util.cpp:224-251`): space/tab/`*` swallowing, the `*/` special
    // case, and the no-swallow passthrough.

    #[test]
    fn test_comment_to_compact_string_flattens() {
        assert_eq!(
            comment_to_compact_string("/* line1\n * line2\n * line3 */"),
            "/* line1 line2 line3 */"
        );
    }

    #[test]
    fn test_comment_to_compact_string_star_slash() {
        assert_eq!(comment_to_compact_string("/* a\n *\n */"), "/* a */");
    }

    #[test]
    fn test_comment_to_compact_string_no_swallow_passthrough() {
        // No spaces/tabs swallowed after `\n` → the original (with its
        // newline) is returned, matching libsass byte-for-byte.
        assert_eq!(comment_to_compact_string("/* a\nb */"), "/* a\nb */");
    }

    #[test]
    fn test_comment_to_compact_string_single_line_passthrough() {
        assert_eq!(comment_to_compact_string("/* hi */"), "/* hi */");
    }
}
