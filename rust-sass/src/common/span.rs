// Copyright 2023 Google LLC. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/util/lazy_file_span.dart + lib/src/util/multi_span.dart
// go-source: go/sasscommon/util_lazy_file_span.go + go/sasscommon/util_multi_span.go

use crate::url::SassUrl;
use std::cell::Cell;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Error;
use std::fmt::Formatter;
use std::rc::Rc;

use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::{FileSpan, SourceLocation};
use crate::common::source_span_file_source::FileSource;
use crate::common::source_span_highlighter::HighlightOptions;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::common::span_error::SpanError;

use crate::io::Io;

/// A wrapper for [`FileSpan`] that defers an expensive creation process until
/// the span is actually needed.
///
/// Mirrors Dart's `LazyFileSpan`: every accessor resolves (and caches) the
/// underlying span on first use via `builder`. `Clone` shares the builder
/// without forcing resolution; once resolved the clone carries only the
/// cached span.
pub struct LazyFileSpan<'parse> {
    // Deferred builder with shared ownership; the `Cell`+`Rc` shape is the
    // lazy-cache semantics, not incidental complexity.
    #[allow(clippy::type_complexity)]
    builder: Cell<Option<Rc<dyn Fn() -> SassResult<FileSpan<'parse>> + 'parse>>>,
    cached: Cell<Option<FileSpan<'parse>>>,
}

impl<'parse> LazyFileSpan<'parse> {
    /// Creates a new [`LazyFileSpan`] that defers calling `builder` until the
    /// underlying span is needed.
    ///
    /// Mirrors Dart's `LazyFileSpan(builder)`.
    pub fn new<F>(builder: F) -> Self
    where
        F: Fn() -> SassResult<FileSpan<'parse>> + 'parse,
    {
        LazyFileSpan {
            builder: Cell::new(Some(Rc::new(builder))),
            cached: Cell::new(None),
        }
    }

    fn get(&self) -> SassResult<FileSpan<'parse>> {
        if let Some(fs) = self.cached.get() {
            return Ok(fs);
        }
        let builder = self.builder.take().ok_or_else(|| SassError::Script {
            message: "LazyFileSpan builder already consumed without caching result.".into(),
            argument_name: None,
        })?;
        let fs = builder()?;
        self.builder.set(Some(builder));
        self.cached.set(Some(fs));
        Ok(fs)
    }
}

impl<'parse> Clone for LazyFileSpan<'parse> {
    fn clone(&self) -> Self {
        if let Some(fs) = self.cached.get() {
            return LazyFileSpan {
                builder: Cell::new(None),
                cached: Cell::new(Some(fs)),
            };
        }
        let builder_rc = self.builder.take().expect("no builder for clone");
        let clone = Rc::clone(&builder_rc);
        self.builder.set(Some(builder_rc));
        LazyFileSpan {
            builder: Cell::new(Some(clone)),
            cached: Cell::new(None),
        }
    }
}

impl<'parse> Debug for LazyFileSpan<'parse> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.get() {
            Ok(fs) => f.debug_tuple("LazyFileSpan").field(&fs).finish(),
            Err(e) => f.debug_tuple("LazyFileSpan").field(&e).finish(),
        }
    }
}

/// A [`FileSpan`] wrapper with secondary spans attached, so that
/// [`Span::message`] can forward to multi-span rendering.
///
/// Mirrors Dart's `MultiSpan`: used to transparently support multi-span
/// messages where only single spans are expected (such as logger
/// invocations). Where backwards compatibility is not a concern, separate
/// multi-span APIs are preferred over this type. All span accessors delegate
/// to the primary span; `highlight`/`message` render the primary plus
/// secondaries, and `expand`/`subspan` rebuild the wrapper around the new
/// primary so the labels survive.
pub struct MultiSpan<'parse> {
    primary: Box<Span<'parse>>,
    primary_label: String,
    secondary_spans: Vec<(FileSpan<'parse>, String)>,
}

impl<'parse> MultiSpan<'parse> {
    /// Creates a [`MultiSpan`] with `primary` as the highlighted span,
    /// `primary_label` as its label, and `secondary_spans` as the extra
    /// labelled points of reference.
    ///
    /// Mirrors Dart's `MultiSpan(primary, primaryLabel, secondarySpans)`.
    pub fn new(
        primary: Span<'parse>,
        primary_label: String,
        secondary_spans: Vec<(FileSpan<'parse>, String)>,
    ) -> Self {
        MultiSpan {
            primary: Box::new(primary),
            primary_label,
            secondary_spans,
        }
    }

    fn with_primary(&self, new_primary: FileSpan<'parse>) -> Span<'parse> {
        Span::Multi(MultiSpan {
            primary: Box::new(Span::File(new_primary)),
            primary_label: self.primary_label.clone(),
            secondary_spans: self.secondary_spans.clone(),
        })
    }
}

impl<'parse> Clone for MultiSpan<'parse> {
    fn clone(&self) -> Self {
        MultiSpan {
            primary: self.primary.clone(),
            primary_label: self.primary_label.clone(),
            secondary_spans: self.secondary_spans.clone(),
        }
    }
}

impl<'parse> Debug for MultiSpan<'parse> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiSpan")
            .field("primary_label", &self.primary_label)
            .field("secondary_spans", &self.secondary_spans)
            .finish()
    }
}

/// A span that is either resolved ([`Span::File`]), lazily computed
/// ([`Span::Lazy`]), or carrying secondary spans ([`Span::Multi`]).
///
/// Rust has no `FileSpan` subclassing, so Dart's `LazyFileSpan`/`MultiSpan`
/// wrappers become variants of this union. Every accessor delegates through
/// to the (resolved) primary span; [`Span::file_span`] resolves `Lazy` on
/// demand. `Interpolation.span` is the one AST-node span typed as `Span`
/// rather than `FileSpan`, matching Dart's `LazyFileSpan` stored inside
/// `Interpolation`.
#[derive(Debug)]
pub enum Span<'parse> {
    File(FileSpan<'parse>),
    Lazy(LazyFileSpan<'parse>),
    Multi(MultiSpan<'parse>),
}

impl<'parse> Clone for Span<'parse> {
    fn clone(&self) -> Self {
        match self {
            Span::File(fs) => Span::File(*fs),
            Span::Lazy(l) => Span::Lazy(l.clone()),
            Span::Multi(m) => Span::Multi(m.clone()),
        }
    }
}

impl<'parse> Span<'parse> {
    /// Returns the source text covered by this span.
    ///
    /// Mirrors Dart's `SourceSpan.text`.
    pub fn text(&self) -> Result<String, SpanError> {
        match self {
            Span::File(fs) => Ok(fs.text().to_string()),
            Span::Lazy(l) => Ok(l.get()?.text().to_string()),
            Span::Multi(m) => m.primary.text(),
        }
    }

    pub fn start_location(&self) -> Result<SourceLocation, SpanError> {
        match self {
            Span::File(fs) => Ok(fs.start_location()),
            Span::Lazy(l) => Ok(l.get()?.start_location()),
            Span::Multi(m) => m.primary.start_location(),
        }
    }

    pub fn end_location(&self) -> Result<SourceLocation, SpanError> {
        match self {
            Span::File(fs) => Ok(fs.end_location()),
            Span::Lazy(l) => Ok(l.get()?.end_location()),
            Span::Multi(m) => m.primary.end_location(),
        }
    }

    pub fn source_url(&self) -> Result<Option<SassUrl>, SpanError> {
        match self {
            Span::File(fs) => Ok(fs.source_url().cloned()),
            Span::Lazy(l) => Ok(l.get()?.source_url().cloned()),
            Span::Multi(m) => m.primary.source_url(),
        }
    }

    pub fn file(&self) -> Result<Option<&'parse FileSource<'parse>>, SpanError> {
        match self {
            Span::File(fs) => Ok(fs.file()),
            Span::Lazy(l) => Ok(l.get()?.file()),
            Span::Multi(m) => m.primary.file(),
        }
    }

    /// Resolves this span to a concrete [`FileSpan`], computing the builder
    /// of a [`Span::Lazy`] on demand.
    pub fn file_span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            Span::File(fs) => Ok(*fs),
            Span::Lazy(l) => l.get(),
            Span::Multi(m) => m.primary.file_span(),
        }
    }

    /// The length of this span, in characters.
    ///
    /// Mirrors Dart's `SourceSpan.length`.
    pub fn len(&self) -> Result<usize, SpanError> {
        match self {
            Span::File(fs) => Ok(fs.len()),
            Span::Lazy(l) => Ok(l.get()?.len()),
            Span::Multi(m) => m.primary.len(),
        }
    }

    pub fn is_empty(&self) -> Result<bool, SpanError> {
        self.len().map(|l| l == 0)
    }

    pub fn context(&self) -> Result<String, SpanError> {
        match self {
            Span::File(fs) => Ok(fs.context()),
            Span::Lazy(l) => Ok(l.get()?.context()),
            Span::Multi(m) => m.primary.context(),
        }
    }

    /// Prints the text of this span in a user-friendly way.
    ///
    /// Mirrors Dart's `SourceSpan.highlight`: like [`Span::message`] but
    /// without the file name, line/column numbers, or message. A `Multi`
    /// span forwards to multi-span highlighting with its stored labels.
    pub fn highlight(&self, opts: &HighlightOptions, io: &dyn Io) -> SassResult<String> {
        match self {
            Span::Multi(m) => {
                let primary_ctx = SourceSpanWithContext::from_span(&m.primary)?;
                let secondary: Vec<(SourceSpanWithContext, String)> = m
                    .secondary_spans
                    .iter()
                    .map(|(fs, label)| {
                        SourceSpanWithContext::from_file_span(fs).map(|ctx| (ctx, label.clone()))
                    })
                    .collect::<SassResult<Vec<_>>>()?;
                primary_ctx.highlight_multiple(&m.primary_label, &secondary, opts, io)
            }
            _ => SourceSpanWithContext::from_span(self)?.highlight(opts, io),
        }
    }

    /// Formats `message` in a human-friendly way associated with this span.
    ///
    /// Mirrors Dart's `SourceSpan.message`. A `Multi` span forwards to
    /// multi-span rendering with its stored primary label and secondaries.
    pub fn message(
        &self,
        message: &str,
        opts: &HighlightOptions,
        io: &dyn Io,
    ) -> SassResult<String> {
        match self {
            Span::Multi(m) => {
                let primary_ctx = SourceSpanWithContext::from_span(&m.primary)?;
                let secondary: Vec<(SourceSpanWithContext, String)> = m
                    .secondary_spans
                    .iter()
                    .map(|(fs, label)| {
                        SourceSpanWithContext::from_file_span(fs).map(|ctx| (ctx, label.clone()))
                    })
                    .collect::<SassResult<Vec<_>>>()?;
                primary_ctx.message_multiple(message, &m.primary_label, &secondary, opts, io)
            }
            _ => SourceSpanWithContext::from_span(self)?.message(message, opts, io),
        }
    }

    pub fn string(&self) -> Result<String, SpanError> {
        self.text()
    }

    /// Returns a new span covering both `self` and `other`.
    ///
    /// Mirrors Dart's `FileSpan.expand`: unlike `union`, `other` may be
    /// disjoint, in which case the text between the two is covered as well.
    /// A `Multi` span rebuilds the wrapper around the expanded primary so
    /// its labels survive.
    pub fn expand(&self, other: &Span<'parse>) -> Result<Span<'parse>, SpanError> {
        match self {
            Span::File(fs) => fs.expand(other).map(Span::File),
            Span::Lazy(l) => l.get()?.expand(other).map(Span::File),
            Span::Multi(m) => {
                let expanded = m.primary.expand(other)?;
                match expanded {
                    Span::File(fs) => Ok(m.with_primary(fs)),
                    _ => Ok(expanded),
                }
            }
        }
    }

    /// Returns a subspan `start..end` relative to the beginning of this span.
    ///
    /// Mirrors Dart's `SourceSpanExtension.subspan`: a full-range subspan
    /// returns the span unchanged. A `Multi` span rebuilds the wrapper around
    /// the new primary so its labels survive.
    pub fn subspan(&self, start: usize, end: usize) -> Result<Span<'parse>, SpanError> {
        match self {
            Span::File(fs) => fs.subspan(start, end).map(Span::File),
            Span::Lazy(l) => l.get()?.subspan(start, end).map(Span::File),
            Span::Multi(m) => {
                let sub = m.primary.subspan(start, end)?;
                match sub {
                    Span::File(fs) => Ok(m.with_primary(fs)),
                    _ => Ok(sub),
                }
            }
        }
    }

    pub fn trim_right(&self) -> Result<Span<'parse>, SpanError> {
        match self {
            Span::File(fs) => fs.trim_right().map(Span::File),
            Span::Lazy(l) => l.get()?.trim_right().map(Span::File),
            Span::Multi(m) => m.primary.trim_right(),
        }
    }

    pub fn trim_left(&self) -> Result<Span<'parse>, SpanError> {
        match self {
            Span::File(fs) => fs.trim_left().map(Span::File),
            Span::Lazy(l) => l.get()?.trim_left().map(Span::File),
            Span::Multi(m) => m.primary.trim_left(),
        }
    }

    /// Returns this span with all whitespace trimmed from both sides.
    ///
    /// Mirrors Dart's `SpanExtensions.trim` (`trimLeft().trimRight()`).
    pub fn trim(&self) -> Result<Span<'parse>, SpanError> {
        match self {
            Span::File(fs) => fs.trim().map(Span::File),
            Span::Lazy(l) => l.get()?.trim().map(Span::File),
            Span::Multi(m) => m.primary.trim(),
        }
    }

    /// Returns a span covering the text from the beginning of this span to
    /// the beginning of `inner`.
    ///
    /// Mirrors Dart's `SpanExtensions.before`: fails when `inner` isn't fully
    /// within this span or lives in a different file.
    pub fn before(&self, inner: &Span<'parse>) -> Result<Span<'parse>, SpanError> {
        match self {
            Span::File(fs) => fs.before(inner).map(Span::File),
            Span::Lazy(l) => l.get()?.before(inner).map(Span::File),
            Span::Multi(m) => m.primary.before(inner),
        }
    }

    /// Returns a span covering the text from the end of `inner` to the end of
    /// this span.
    ///
    /// Mirrors Dart's `SpanExtensions.after`: fails when `inner` isn't fully
    /// within this span or lives in a different file.
    pub fn after(&self, inner: &Span<'parse>) -> Result<Span<'parse>, SpanError> {
        match self {
            Span::File(fs) => fs.after(inner).map(Span::File),
            Span::Lazy(l) => l.get()?.after(inner).map(Span::File),
            Span::Multi(m) => m.primary.after(inner),
        }
    }

    /// Returns a span covering the text after this span and before `other`.
    ///
    /// Mirrors Dart's `SpanExtensions.between`: fails when `other` starts
    /// before this span ends or lives in a different file.
    pub fn between(&self, other: &Span<'parse>) -> Result<Span<'parse>, SpanError> {
        match self {
            Span::File(fs) => fs.between(other).map(Span::File),
            Span::Lazy(l) => l.get()?.between(other).map(Span::File),
            Span::Multi(m) => m.primary.between(other),
        }
    }

    /// Whether this span contains `target` within its inclusive
    /// `[start, end]` range.
    ///
    /// Mirrors Dart's `SpanExtensions.contains`: spans in different files
    /// never contain each other (returns `false` rather than failing).
    pub fn contains(&self, target: &Span<'parse>) -> Result<bool, SpanError> {
        match self {
            Span::File(fs) => fs.contains(target),
            Span::Lazy(l) => l.get()?.contains(target),
            Span::Multi(m) => m.primary.contains(target),
        }
    }

    /// Returns a subspan excluding an initial at-rule and any whitespace
    /// after it.
    ///
    /// Mirrors Dart's `SpanExtensions.withoutInitialAtRule`.
    pub fn without_initial_at_rule(&self) -> Result<Span<'parse>, SpanError> {
        match self {
            Span::File(fs) => fs.without_initial_at_rule().map(Span::File),
            Span::Lazy(l) => l.get()?.without_initial_at_rule().map(Span::File),
            Span::Multi(m) => m.primary.without_initial_at_rule(),
        }
    }

    /// Returns the span of the quoted text at the start of this span.
    ///
    /// The span must start with `"` or `'`. Mirrors Dart's
    /// `SpanExtensions.initialQuoted`.
    pub fn initial_quoted(&self) -> Result<Span<'parse>, SpanError> {
        match self {
            Span::File(fs) => fs.initial_quoted().map(Span::File),
            Span::Lazy(l) => l.get()?.initial_quoted().map(Span::File),
            Span::Multi(m) => m.primary.initial_quoted(),
        }
    }

    /// Returns the span of the identifier at the start of this span.
    ///
    /// When `include_leading` is greater than 0, that many additional
    /// characters are included before looking for the identifier. Mirrors
    /// Dart's `SpanExtensions.initialIdentifier`.
    pub fn initial_identifier(&self, include_leading: usize) -> Result<Span<'parse>, SpanError> {
        match self {
            Span::File(fs) => fs.initial_identifier(include_leading).map(Span::File),
            Span::Lazy(l) => l.get()?.initial_identifier(include_leading).map(Span::File),
            Span::Multi(m) => m.primary.initial_identifier(include_leading),
        }
    }

    /// Returns a subspan excluding the identifier at the start of this span.
    ///
    /// Mirrors Dart's `SpanExtensions.withoutInitialIdentifier`.
    pub fn without_initial_identifier(&self) -> Result<Span<'parse>, SpanError> {
        match self {
            Span::File(fs) => fs.without_initial_identifier().map(Span::File),
            Span::Lazy(l) => l.get()?.without_initial_identifier().map(Span::File),
            Span::Multi(m) => m.primary.without_initial_identifier(),
        }
    }

    /// Returns a subspan excluding a namespace and `.` at the start of this
    /// span.
    ///
    /// Mirrors Dart's `SpanExtensions.withoutNamespace` (note: unlike Dart,
    /// no error is raised when there is no `.` — the identifier-stripped span
    /// is returned as-is).
    pub fn without_namespace(&self) -> Result<Span<'parse>, SpanError> {
        match self {
            Span::File(fs) => fs.without_namespace().map(Span::File),
            Span::Lazy(l) => l.get()?.without_namespace().map(Span::File),
            Span::Multi(m) => m.primary.without_namespace(),
        }
    }
    pub fn to_display_string(&self) -> SassResult<String> {
        Ok(self.string()?)
    }
}

impl<'parse> Display for Span<'parse> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(Error),
        }
    }
}

impl<'parse> From<FileSpan<'parse>> for Span<'parse> {
    fn from(fs: FileSpan<'parse>) -> Self {
        Span::File(fs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::{FileSpan, SourceLocation};
    use crate::common::source_span_file_source::FileSource;
    use crate::url::SassUrl;
    use bumpalo::Bump;
    use std::cell::Cell;

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

    fn test_url(url_str: &str) -> SassUrl {
        SassUrl::parse(url_str).unwrap()
    }

    #[test]
    fn test_lazy_not_called_until_get() {
        let arena = Bump::new();
        let fs = test_source(&arena, "x");
        let called = Rc::new(Cell::new(0));
        let called2 = Rc::clone(&called);
        let lazy = LazyFileSpan::new(move || {
            called2.set(called2.get() + 1);
            Ok(FileSpan::new(Some(fs), 0, 1))
        });
        assert_eq!(called.get(), 0);
        let _ = Span::Lazy(lazy).text();
        assert_eq!(called.get(), 1);
    }

    #[test]
    fn test_lazy_caches_result() {
        let arena = Bump::new();
        let fs = test_source(&arena, "x");
        let called = Rc::new(Cell::new(0));
        let called2 = Rc::clone(&called);
        let lazy = LazyFileSpan::new(move || {
            called2.set(called2.get() + 1);
            Ok(FileSpan::new(Some(fs), 0, 1))
        });
        let span = Span::Lazy(lazy);
        let _ = span.text();
        let _ = span.text();
        let _ = span.text();
        assert_eq!(called.get(), 1);
    }

    #[test]
    fn test_lazy_delegates_text() {
        let arena = Bump::new();
        let fs = test_source(&arena, "hello");
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 0, 5)));
        let span = Span::Lazy(lazy);
        assert_eq!(span.text().unwrap(), "hello");
    }

    #[test]
    fn test_lazy_file_span_file() {
        let arena = Bump::new();
        let fs = test_source(&arena, "a");
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 0, 1)));
        let span = Span::Lazy(lazy);
        assert_eq!(span.file().unwrap(), Some(fs));
    }

    #[test]
    fn test_lazy_delegates_start_location() {
        let arena = Bump::new();
        let fs = test_source(&arena, "hello\n");
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 0, 5)));
        let span = Span::Lazy(lazy);
        let loc = span.start_location().unwrap();
        assert_eq!(
            loc,
            SourceLocation {
                offset: 0,
                line: 0,
                column: 0
            }
        );
    }

    #[test]
    fn test_lazy_file_span_end_location() {
        let arena = Bump::new();
        let fs = test_source(&arena, "hello\n");
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 0, 5)));
        let span = Span::Lazy(lazy);
        let loc = span.end_location().unwrap();
        assert_eq!(
            loc,
            SourceLocation {
                offset: 5,
                line: 0,
                column: 5
            }
        );
    }

    #[test]
    fn test_lazy_file_span_length() {
        let arena = Bump::new();
        let fs = test_source(&arena, "hello");
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 2, 7)));
        let span = Span::Lazy(lazy);
        assert_eq!(span.len().unwrap(), 5);
    }

    #[test]
    fn test_lazy_file_span_source_url() {
        let u = test_url("file:///test.scss");
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "a", Some(u.clone()));
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 0, 1)));
        let span = Span::Lazy(lazy);
        assert_eq!(
            span.source_url().unwrap().as_ref().map(|u| u.as_str()),
            Some("file:///test.scss")
        );
    }

    #[test]
    fn test_lazy_file_span_context() {
        let arena = Bump::new();
        let fs = test_source(&arena, "body { }\n");
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 0, 8)));
        let span = Span::Lazy(lazy);
        assert!(!span.context().unwrap().is_empty());
    }

    #[test]
    fn test_lazy_file_span_string() {
        let arena = Bump::new();
        let fs = test_source(&arena, "hello");
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 0, 5)));
        let span = Span::Lazy(lazy);
        assert_eq!(span.string().unwrap(), "hello");
    }

    #[test]
    fn test_lazy_delegates_expand() {
        let arena = Bump::new();
        let fs = test_source(&arena, "..........\n");
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 3, 8)));
        let span = Span::Lazy(lazy);
        let other = Span::File(FileSpan::new(Some(fs), 0, 10));
        let expanded = span.expand(&other).unwrap();
        assert_eq!(expanded.start_location().unwrap().offset, 0);
        assert_eq!(expanded.end_location().unwrap().offset, 10);
    }

    #[test]
    fn test_lazy_delegates_subspan() {
        let arena = Bump::new();
        let fs = test_source(&arena, "abcde");
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 0, 5)));
        let span = Span::Lazy(lazy);
        let sub = span.subspan(1, 4).unwrap();
        assert_eq!(sub.text().unwrap(), "bcd");
    }

    #[test]
    fn test_lazy_delegates_before() {
        let arena = Bump::new();
        let fs = test_source(&arena, "abcdef");
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 0, 6)));
        let span = Span::Lazy(lazy);
        let sub = Span::File(FileSpan::new(Some(fs), 3, 6));
        let before_span = span.before(&sub).unwrap();
        assert_eq!(before_span.text().unwrap(), "abc");
    }

    #[test]
    fn test_lazy_file_span_trim_right() {
        let arena = Bump::new();
        let fs = test_source(&arena, "abc  ");
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 0, 5)));
        let span = Span::Lazy(lazy);
        let trimmed = span.trim_right().unwrap();
        assert_eq!(trimmed.text().unwrap(), "abc");
    }

    #[test]
    fn test_lazy_file_span_after() {
        let arena = Bump::new();
        let fs = test_source(&arena, "abcdef");
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 0, 6)));
        let span = Span::Lazy(lazy);
        let sub = Span::File(FileSpan::new(Some(fs), 0, 3));
        let after_span = span.after(&sub).unwrap();
        assert_eq!(after_span.text().unwrap(), "def");
    }

    #[test]
    fn test_lazy_file_span_between() {
        let arena = Bump::new();
        let fs = test_source(&arena, "abcdef");
        let a = Span::File(FileSpan::new(Some(fs), 1, 2));
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 4, 5)));
        let b = Span::Lazy(lazy);
        let between_span = a.between(&b).unwrap();
        assert_eq!(between_span.text().unwrap(), "cd");
    }

    #[test]
    fn test_lazy_passed_to_contains() {
        let arena = Bump::new();
        let fs = test_source(&arena, "abcdef");
        let inner = Span::File(FileSpan::new(Some(fs), 2, 4));
        let lazy = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 0, 6)));
        let span = Span::Lazy(lazy);
        assert!(span.contains(&inner).unwrap());
    }

    #[test]
    fn test_lazy_as_argument_to_before() {
        let arena = Bump::new();
        let fs = test_source(&arena, "abcdef");
        let parent = Span::File(FileSpan::new(Some(fs), 0, 6));
        let lazy_sub = LazyFileSpan::new(move || Ok(FileSpan::new(Some(fs), 2, 4)));
        let sub = Span::Lazy(lazy_sub);
        let before_span = parent.before(&sub).unwrap();
        assert_eq!(before_span.text().unwrap(), "ab");
    }

    #[test]
    fn test_multispan_delegates_text() {
        let arena = Bump::new();
        let fs = test_source(&arena, "hello");
        let primary = Span::File(FileSpan::new(Some(fs), 0, 5));
        let multi = Span::Multi(MultiSpan::new(primary, "label".into(), vec![]));
        assert_eq!(multi.text().unwrap(), "hello");
    }

    #[test]
    fn test_multispan_expand_preserves_metadata() {
        let arena = Bump::new();
        let fs = test_source(&arena, "..........");
        let primary = Span::File(FileSpan::new(Some(fs), 3, 8));
        let secondary = vec![(FileSpan::new(Some(fs), 0, 1), "note".into())];
        let multi = Span::Multi(MultiSpan::new(primary, "primary".into(), secondary.clone()));
        let other = Span::File(FileSpan::new(Some(fs), 0, 10));
        let expanded = multi.expand(&other).unwrap();
        assert_eq!(expanded.start_location().unwrap().offset, 0);
        assert_eq!(expanded.end_location().unwrap().offset, 10);
        match expanded {
            Span::Multi(ref m) => {
                assert_eq!(m.primary_label, "primary");
                assert_eq!(m.secondary_spans.len(), 1);
            }
            _ => panic!("expected Multi variant"),
        }
    }
}
