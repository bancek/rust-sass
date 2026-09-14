// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/interpolation_buffer.dart
// go-source: go/value/sass_interpolation_buffer.go

use std::fmt;
use std::fmt::Write;

use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::common::span::Span;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::interpolation::{Interpolation, InterpolationPart};

/// A buffer that iteratively builds up an [`Interpolation`].
///
/// Add text using [`write`](Self::write) and related methods, and
/// [`Expression`]s using [`add`](Self::add). Once that's done, call
/// [`interpolation`](Self::interpolation) to build the result.
pub struct InterpolationBuffer<'parse> {
    /// The buffer that accumulates plain text.
    text: String,
    /// The contents of the [`Interpolation`] so far.
    ///
    /// This contains [`InterpolationPart::Text`]s and
    /// [`InterpolationPart::Expression`]s.
    contents: Vec<InterpolationPart<'parse>>,
    /// The spans of the expressions in [`contents`](Self::contents).
    ///
    /// These spans cover from the beginning of `#{` through `}`, rather than
    /// just the expressions themselves.
    spans: Vec<Option<FileSpan<'parse>>>,
}

impl<'parse> InterpolationBuffer<'parse> {
    /// Creates an empty buffer.
    pub fn new() -> Self {
        InterpolationBuffer {
            text: String::new(),
            contents: Vec::new(),
            spans: Vec::new(),
        }
    }

    /// Appends text to the buffer (the `StringSink::write` leg).
    pub fn write(&mut self, s: &str) {
        self.text.push_str(s);
    }

    /// Joins objects with the given separator and appends them.
    ///
    /// When `separator` is `None`, objects are concatenated directly (Dart's
    /// default `separator` is `""`).
    pub fn write_all(&mut self, objects: &[String], separator: Option<&str>) {
        let sep = separator.unwrap_or("");
        for (i, obj) in objects.iter().enumerate() {
            if i > 0 {
                self.text.push_str(sep);
            }
            self.text.push_str(obj);
        }
    }

    /// Appends a single character (the `StringSink::writeCharCode` leg).
    pub fn write_char_code(&mut self, ch: char) {
        self.text.push(ch);
    }

    /// Appends text followed by a newline (the `StringSink::writeln` leg).
    pub fn writeln(&mut self, s: &str) {
        self.text.push_str(s);
        self.text.push('\n');
    }

    /// Returns whether this buffer has no contents.
    pub fn is_empty(&self) -> bool {
        self.contents.is_empty() && self.text.is_empty()
    }

    /// Returns the substring of the buffer string after the last interpolation.
    pub fn trailing_string(&self) -> &str {
        &self.text
    }

    /// Empties this buffer.
    ///
    /// Note: Dart also clears the text buffer; this clears `contents`,
    /// `spans`, and the pending text alike.
    pub fn clear(&mut self) {
        self.contents.clear();
        self.spans.clear();
        self.text.clear();
    }

    /// Adds `expression` to this buffer.
    ///
    /// The `span` should cover from the beginning of `#{` through `}`.
    pub fn add(&mut self, expression: Expression<'parse>, span: FileSpan<'parse>) {
        self.flush_text();
        self.contents
            .push(InterpolationPart::Expression(Box::new(expression)));
        self.spans.push(Some(span));
    }

    /// Adds the contents of `interpolation` to this buffer.
    ///
    /// Leading/trailing text merges into the pending text buffer so the
    /// result never holds two adjacent `Text` parts.
    pub fn add_interpolation(&mut self, interpolation: &Interpolation<'parse>) {
        if interpolation.contents.is_empty() {
            return;
        }

        let mut skip = 0;

        // If first element is text, prepend to our text buffer
        if let InterpolationPart::Text(first) = &interpolation.contents[0] {
            self.text.push_str(first);
            skip = 1;
        }

        self.flush_text();

        // Add remaining elements
        for i in skip..interpolation.contents.len() {
            match &interpolation.contents[i] {
                InterpolationPart::Text(s) => {
                    self.contents.push(InterpolationPart::Text(s.clone()));
                }
                InterpolationPart::Expression(expr) => {
                    self.contents
                        .push(InterpolationPart::Expression(expr.clone()));
                }
            }
        }
        for i in skip..interpolation.spans.len() {
            self.spans.push(interpolation.spans[i]);
        }

        // If last element is a string, move it to text buffer
        if let Some(InterpolationPart::Text(_)) = self.contents.last() {
            if let Some(InterpolationPart::Text(s)) = self.contents.pop() {
                self.spans.pop();
                self.text.push_str(&s);
            }
        }
    }

    /// Flushes the pending text into `contents` if non-empty.
    pub fn flush_text(&mut self) {
        if self.text.is_empty() {
            return;
        }
        let s = std::mem::take(&mut self.text);
        self.contents.push(InterpolationPart::Text(s));
        self.spans.push(None);
    }

    /// Creates an [`Interpolation`] with the given `span` from the contents
    /// of this buffer.
    ///
    /// Pending text is appended as a final `Text` part; a lone `Text` part
    /// collapses to [`Interpolation::plain`].
    pub fn interpolation(
        &self,
        span: impl Into<Span<'parse>>,
    ) -> SassResult<Interpolation<'parse>> {
        let span = span.into();
        let mut contents = self.contents.clone();
        let mut spans = self.spans.clone();
        if !self.text.is_empty() {
            contents.push(InterpolationPart::Text(self.text.clone()));
            spans.push(None);
        }

        if contents.len() == 1 {
            if let InterpolationPart::Text(s) = &contents[0] {
                return Ok(Interpolation::plain(s.clone(), span));
            }
        }

        Ok(Interpolation::new(contents, spans, span)?)
    }
}

impl<'parse> Default for InterpolationBuffer<'parse> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'parse> InterpolationBuffer<'parse> {
    /// Renders the buffered contents with expressions wrapped in `#{...}`.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        for part in &self.contents {
            match part {
                InterpolationPart::Text(s) => write!(buf, "{s}").unwrap(),
                InterpolationPart::Expression(expr) => {
                    write!(buf, "#{{{}}}", Expression::to_display_string(expr)?).unwrap()
                }
            }
        }
        write!(buf, "{}", self.text).unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for InterpolationBuffer<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::expression_variable::VariableExpression;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    fn test_span<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
        start: usize,
        end: usize,
    ) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), start, end)
    }

    #[test]
    fn test_empty() {
        let buf = InterpolationBuffer::new();
        assert!(buf.is_empty());
        assert_eq!(buf.trailing_string(), "");
    }

    #[test]
    fn test_write() {
        let mut buf = InterpolationBuffer::new();
        buf.write("hello");
        assert!(!buf.is_empty());
        assert_eq!(buf.trailing_string(), "hello");
    }

    #[test]
    fn test_write_char_code() {
        let mut buf = InterpolationBuffer::new();
        buf.write_char_code('A');
        assert_eq!(buf.trailing_string(), "A");
    }

    #[test]
    fn test_writeln() {
        let mut buf = InterpolationBuffer::new();
        buf.writeln("hello");
        assert_eq!(buf.trailing_string(), "hello\n");
    }

    #[test]
    fn test_write_all() {
        let mut buf = InterpolationBuffer::new();
        buf.write_all(&["a".into(), "b".into(), "c".into()], Some(","));
        assert_eq!(buf.trailing_string(), "a,b,c");
    }

    #[test]
    fn test_write_all_no_sep() {
        let mut buf = InterpolationBuffer::new();
        buf.write_all(&["a".into(), "b".into(), "c".into()], None);
        assert_eq!(buf.trailing_string(), "abc");
    }

    #[test]
    fn test_clear() {
        let mut buf = InterpolationBuffer::new();
        buf.write("hello");
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.trailing_string(), "");
    }

    #[test]
    fn test_add_expression() {
        let arena = Bump::new();
        let expr_span = test_span(&arena, "#{$expr}", 0, 7);
        let var_span = test_span(&arena, "$expr", 1, 6);
        let expr = Expression::Variable(VariableExpression::new("expr".into(), var_span, None));

        let mut buf = InterpolationBuffer::new();
        buf.add(expr, expr_span);
        assert!(!buf.is_empty());
    }

    #[test]
    fn test_add_interpolation() {
        let arena = Bump::new();
        let source = "a.#{$expr}.b";
        let interp_span = test_span(&arena, source, 0, 12);
        let var_span = test_span(&arena, source, 4, 9);
        let expr_span = test_span(&arena, source, 2, 10);
        let expr = Expression::Variable(VariableExpression::new("expr".into(), var_span, None));
        let interp = Interpolation::new(
            vec![InterpolationPart::Expression(Box::new(expr))],
            vec![Some(expr_span)],
            interp_span,
        )
        .unwrap();

        let mut buf = InterpolationBuffer::new();
        buf.write("a.");
        buf.add_interpolation(&interp);
        assert!(!buf.is_empty());
    }

    #[test]
    fn test_add_interpolation_empty() {
        let arena = Bump::new();
        let span = test_span(&arena, "", 0, 0);
        let interp = Interpolation::new(vec![], vec![], span).unwrap();

        let mut buf = InterpolationBuffer::new();
        buf.add_interpolation(&interp);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_interpolation_plain() {
        let arena = Bump::new();
        let span = test_span(&arena, "hello", 0, 5);

        let mut buf = InterpolationBuffer::new();
        buf.write("hello");
        let interp = buf.interpolation(span).unwrap();

        assert!(interp.is_plain());
        assert_eq!(interp.as_plain(), Some("hello"));
    }

    #[test]
    fn test_interpolation_mixed() {
        let arena = Bump::new();
        let source = "a.#{$expr}.b";
        let file_span = test_span(&arena, source, 0, 12);
        let var_span = test_span(&arena, source, 4, 9);
        let expr_span = test_span(&arena, source, 2, 10);
        let expr = Expression::Variable(VariableExpression::new("expr".into(), var_span, None));

        let mut buf = InterpolationBuffer::new();
        buf.write("a.");
        buf.add(expr, expr_span);
        buf.write(".b");

        let interp = buf.interpolation(file_span).unwrap();
        assert!(!interp.is_plain());
        assert_eq!(interp.contents.len(), 3);
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let source = "a.#{$expr}.b";
        let var_span = test_span(&arena, source, 4, 9);
        let expr_span = test_span(&arena, source, 2, 10);
        let expr = Expression::Variable(VariableExpression::new("expr".into(), var_span, None));

        let mut buf = InterpolationBuffer::new();
        buf.write("a.");
        buf.add(expr, expr_span);
        buf.write(".b");

        let result = format!("{buf}");
        assert_eq!(result, "a.#{$expr}.b");
    }

    #[test]
    fn test_flush_text_behavior() {
        let arena = Bump::new();
        let source = "before#{$expr}after";
        let file_span = test_span(&arena, source, 0, 17);
        let var_span = test_span(&arena, source, 7, 12);
        let expr_span = test_span(&arena, source, 6, 13);
        let expr = Expression::Variable(VariableExpression::new("expr".into(), var_span, None));

        let mut buf = InterpolationBuffer::new();
        buf.write("before");
        buf.add(expr, expr_span);
        buf.write("after");

        let interp = buf.interpolation(file_span).unwrap();
        assert_eq!(interp.contents.len(), 3);
        // First should be "before"
        match &interp.contents[0] {
            InterpolationPart::Text(s) => assert_eq!(s, "before"),
            _ => panic!("expected Text"),
        }
        // Last should be "after"
        match &interp.contents[2] {
            InterpolationPart::Text(s) => assert_eq!(s, "after"),
            _ => panic!("expected Text"),
        }
    }
}
