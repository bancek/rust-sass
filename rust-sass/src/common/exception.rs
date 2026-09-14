// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/exception.dart (+ external package:string_scanner SpanScanner error + package:stack_trace Frame/Trace)
// go-source: go/sasscommon/exception.go

use crate::common::core_errors::ArgumentError;
use crate::common::core_errors::RangeError;
use crate::common::core_errors::StateError;
use crate::common::core_errors::UnsupportedError;
use crate::common::source_span_highlighter::HighlightOptions;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::Write;
use std::mem;
use std::ops::Deref;

use crate::url::SassUrl;
use thiserror::Error;

use crate::common::file_span::{FileSpan, SourceLocation};
use crate::common::pretty_uri::pretty_uri;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::common::span_error::SpanError;

use crate::io::Io;

/// The single error type raised by Sass.
///
/// Each variant folds one or more Dart exception classes from
/// `exception.dart` into one enum: [`SassError::Sass`] is Dart's
/// `SassException`, `Runtime` is `SassRuntimeException`, `Format` is
/// `SassFormatException` (it also carries the failed span plus the original
/// source text, as `SourceSpanFormatException` does), `MultiSpan` covers the
/// three `MultiSpan*` classes, `Script` is `SassScriptException`, and
/// `MultiSpanScript` is `MultiSpanSassScriptException`.
///
/// Internal errors borrow their spans zero-copy; at the public boundary the
/// spans are copied into owned [`SourceSpanWithContext`] values so no arena
/// lifetime leaks out. See `docs/ref/common.md`.
#[derive(Debug, Error, Clone)]
pub enum SassError {
    /// An error raised by SassScript that has no span yet.
    ///
    /// Mirrors Dart's `SassScriptException`: it is caught by the internals
    /// and converted into a [`SassError::Runtime`] with a source span and a
    /// stack trace. `argument_name` is the name of the Sass function argument
    /// that triggered the error; when set it is included in the message as
    /// `"$name: message"`.
    Script {
        message: String,
        argument_name: Option<String>,
    },

    /// An error thrown while evaluating a stylesheet.
    ///
    /// Mirrors Dart's `SassRuntimeException`: a [`SassError::Sass`] plus the
    /// Sass stack trace at the point the error was thrown.
    Runtime {
        message: String,
        span: SourceSpanWithContext,
        trace: Trace,
        cause: Option<Box<SassError>>,
        loaded_urls: Vec<SassUrl>,
    },

    /// An error thrown when parsing has failed.
    ///
    /// Mirrors Dart's `SassFormatException`: a [`SassError::Sass`] that also
    /// carries the span's start offset and the full original source text (as
    /// `SourceSpanFormatException` does).
    Format {
        message: String,
        span: SourceSpanWithContext,
        original_source: Option<String>,
        cause: Option<Box<SassError>>,
        loaded_urls: Vec<SassUrl>,
    },

    /// An error thrown by Sass.
    ///
    /// Mirrors Dart's `SassException`. `loaded_urls` is the set of canonical
    /// stylesheet URLs loaded before the compilation failed.
    Sass {
        message: String,
        span: SourceSpanWithContext,
        cause: Option<Box<SassError>>,
        loaded_urls: Vec<SassUrl>,
    },

    /// An error with secondary spans attached as points of reference.
    ///
    /// Mirrors Dart's `MultiSpanSassException` / `MultiSpanSassRuntimeException`
    /// / `MultiSpanSassFormatException`. `primary_label` labels the primary
    /// span and `secondary` maps each extra span to its label. An empty
    /// `trace` means "derive a single frame from the span".
    MultiSpan {
        message: String,
        span: SourceSpanWithContext,
        primary_label: Option<String>,
        secondary: Vec<(SourceSpanWithContext, String)>,
        original_source: Option<String>,
        cause: Option<Box<SassError>>,
        loaded_urls: Vec<SassUrl>,
        trace: Trace,
    },

    /// A script-level multi-span error: carries the message, primary label, and
    /// secondary spans, but no primary span yet. The eval call site attaches the
    /// member-use span + trace via `with_member_use_span` (mirrors Dart's
    /// `MultiSpanSassScriptException.withSpan`).
    MultiSpanScript {
        message: String,
        primary_label: Option<String>,
        secondary: Vec<(SourceSpanWithContext, String)>,
        cause: Option<Box<SassError>>,
        loaded_urls: Vec<SassUrl>,
    },
}

impl Display for SassError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SassError::Script {
                message,
                argument_name: Some(name),
            } => write!(f, "${name}: {message}"),
            SassError::Script {
                message,
                argument_name: None,
            } => write!(f, "{message}"),
            SassError::Runtime { message, .. }
            | SassError::Format { message, .. }
            | SassError::Sass { message, .. }
            | SassError::MultiSpan { message, .. } => {
                write!(f, "Error: {message}")
            }
            SassError::MultiSpanScript { message, .. } => write!(f, "{message}"),
        }
    }
}

impl SassError {
    /// Returns the error message.
    ///
    /// Mirrors Dart's `SassException.message` (inherited from
    /// `SourceSpanException`): the raw message without span rendering.
    /// `SassScriptException` with an argument name already folds the
    /// `"$name: "` prefix into the message at construction; see
    /// [`SassError::full_message`].
    pub fn message(&self) -> &str {
        match self {
            SassError::Script { message, .. }
            | SassError::Runtime { message, .. }
            | SassError::Format { message, .. }
            | SassError::Sass { message, .. }
            | SassError::MultiSpan { message, .. }
            | SassError::MultiSpanScript { message, .. } => message,
        }
    }

    /// Returns the user-visible message with the argument name prefix for
    /// `Script` errors: `"$name: message"` when an argument name is set,
    /// otherwise the bare message.
    ///
    /// Mirrors Dart's `SassScriptException(message, [argumentName])`
    /// constructor, which folds the name into the message at construction.
    pub fn full_message(&self) -> String {
        match self {
            SassError::Script {
                message,
                argument_name: Some(name),
            } => format!("${name}: {message}"),
            _ => self.message().to_string(),
        }
    }

    /// Returns the original source text, if any.
    ///
    /// Mirrors Dart's `SourceSpanFormatException.source`: the full contents
    /// of the file that failed to parse. Only [`SassError::Format`] and
    /// [`SassError::MultiSpan`] (when wrapping a format error) carry it.
    pub fn original_source(&self) -> Option<&String> {
        match self {
            SassError::Format {
                original_source, ..
            }
            | SassError::MultiSpan {
                original_source, ..
            } => original_source.as_ref(),
            _ => None,
        }
    }

    /// Returns the span, if this error has one.
    ///
    /// The two script-level variants ([`SassError::Script`] and
    /// [`SassError::MultiSpanScript`]) carry no primary span yet — Dart's
    /// `SassScriptException` "doesn't (yet) have a `FileSpan` associated with
    /// it" — so this returns `None` for them.
    pub fn span(&self) -> Option<&SourceSpanWithContext> {
        match self {
            SassError::Script { .. } => None,
            SassError::Runtime { span, .. }
            | SassError::Format { span, .. }
            | SassError::Sass { span, .. }
            | SassError::MultiSpan { span, .. } => Some(span),
            SassError::MultiSpanScript { .. } => None,
        }
    }

    /// Returns the URLs loaded so far when this error occurred.
    ///
    /// Mirrors Dart's `SassException.loadedUrls`: the canonical stylesheet
    /// URLs loaded in the course of the compilation before it failed. The
    /// unspanned [`SassError::Script`] family carries none.
    pub fn loaded_urls(&self) -> &[SassUrl] {
        match self {
            SassError::Script { .. } => &[],
            SassError::Runtime { loaded_urls, .. }
            | SassError::Format { loaded_urls, .. }
            | SassError::Sass { loaded_urls, .. }
            | SassError::MultiSpan { loaded_urls, .. }
            | SassError::MultiSpanScript { loaded_urls, .. } => loaded_urls,
        }
    }

    /// Returns a copy of this with `loaded_urls` set to the given value.
    ///
    /// Mirrors Dart's `SassException.withLoadedUrls` (evaluate.dart stamps
    /// all `SassException`s at the evaluate boundary; the unspanned
    /// `SassScriptException` family has no URLs to stamp).
    pub fn with_loaded_urls(mut self, loaded_urls: Vec<SassUrl>) -> Self {
        match &mut self {
            SassError::Script { .. } => {}
            SassError::Runtime {
                loaded_urls: urls, ..
            }
            | SassError::Format {
                loaded_urls: urls, ..
            }
            | SassError::Sass {
                loaded_urls: urls, ..
            }
            | SassError::MultiSpan {
                loaded_urls: urls, ..
            }
            | SassError::MultiSpanScript {
                loaded_urls: urls, ..
            } => {
                *urls = loaded_urls;
            }
        }
        self
    }

    /// Full formatted error string with default highlight options.
    ///
    /// Mirrors Dart's `SassException.toString()`: writes `"Error: {message}"`,
    /// then the span highlight, then each trace frame on its own line
    /// indented two spaces. [`SassError::Script`] renders as just the message
    /// (Dart's `SassScriptException.toString`), matching its `Display` impl.
    /// Matches Go: all exception types' Error() method.
    pub fn to_error_string(&self, io: &dyn Io) -> String {
        self.to_error_string_with_options(&Default::default(), io)
    }

    /// Full formatted error string with specified highlight options.
    ///
    /// Same rendering as [`SassError::to_error_string`], with the caller's
    /// [`HighlightOptions`] (color on/off) threaded into the span highlight.
    /// Matches Go: ErrorWithOptions(opts HighlightOptions) on each exception type.
    pub fn to_error_string_with_options(&self, opts: &HighlightOptions, io: &dyn Io) -> String {
        match self {
            SassError::Script { .. } => self.to_string(),
            SassError::MultiSpanScript { .. } => self.to_string(),
            SassError::Sass { message, span, .. } => {
                format_single(message, span, None, None, None, opts, io)
            }
            SassError::Runtime {
                message,
                span,
                trace,
                ..
            } => format_single(message, span, None, None, Some(trace), opts, io),
            SassError::Format { message, span, .. } => {
                format_single(message, span, None, None, None, opts, io)
            }
            SassError::MultiSpan {
                message,
                span,
                primary_label,
                secondary,
                trace,
                ..
            } => format_multiple(
                message,
                span,
                primary_label.as_deref().unwrap_or(""),
                secondary,
                if trace.is_empty() {
                    // Generate single-frame trace from span, matching Go's
                    // MultiSpanSassException.Trace().
                    trace_for_span(span, "root stylesheet")
                } else {
                    trace.clone()
                },
                opts,
                io,
            ),
        }
    }

    /// Renders this error as a CSS stylesheet that displays the error above the
    /// current page using a `body::before` pseudo-element.
    ///
    /// Mirrors Dart's `SassException.toCssString()`; see
    /// [`exception_to_css_string`] for the rendering details.
    ///
    /// Matches Go: each exception type's ToCssString() method.
    pub fn to_css_string(&self, io: &dyn Io) -> String {
        exception_to_css_string_impl(&self.to_error_string(io))
    }

    /// Converts `self` into a [`SassError::MultiSpan`] with `additional_span`
    /// and `label` added as a secondary span.
    ///
    /// Mirrors Dart's `withAdditionalSpan`: when `self` is already a
    /// `MultiSpan` the secondary map gains one entry; otherwise a new
    /// `MultiSpan` is built whose primary span is `self`'s own span (falling
    /// back to `additional_span` for the unspanned script variants) with an
    /// empty primary label and `self` kept as the cause.
    pub fn with_additional_span(
        mut self,
        additional_span: SourceSpanWithContext,
        label: String,
    ) -> Self {
        match &mut self {
            SassError::MultiSpan { secondary, .. } => {
                secondary.push((additional_span, label));
                self
            }
            _ => {
                let default_span = additional_span.clone();
                let default_label = label.clone();
                let placeholder = SassError::Script {
                    message: String::new(),
                    argument_name: None,
                };
                let old = mem::replace(&mut self, placeholder);

                let (message, old_span, original_source, trace, loaded_urls) = match &old {
                    SassError::Runtime {
                        message,
                        span,
                        trace,
                        loaded_urls,
                        ..
                    } => (
                        message.clone(),
                        Some(span.clone()),
                        None,
                        trace.clone(),
                        loaded_urls.clone(),
                    ),
                    SassError::Sass {
                        message,
                        span,
                        loaded_urls,
                        ..
                    } => (
                        message.clone(),
                        Some(span.clone()),
                        None,
                        Trace::default(),
                        loaded_urls.clone(),
                    ),
                    SassError::Format {
                        message,
                        span,
                        original_source,
                        loaded_urls,
                        ..
                    } => (
                        message.clone(),
                        Some(span.clone()),
                        original_source.clone(),
                        Trace::default(),
                        loaded_urls.clone(),
                    ),
                    SassError::Script { message, .. }
                    | SassError::MultiSpanScript { message, .. } => {
                        (message.clone(), None, None, Trace::default(), vec![])
                    }
                    SassError::MultiSpan { .. } => unreachable!(),
                };
                let effective_span = old_span.unwrap_or(default_span);
                self = SassError::MultiSpan {
                    message,
                    span: effective_span,
                    primary_label: None,
                    secondary: vec![(additional_span, default_label)],
                    original_source,
                    cause: Some(Box::new(old)),
                    loaded_urls,
                    trace,
                };
                self
            }
        }
    }

    /// Converts a script-level `MultiSpanScript` into a full `MultiSpan`,
    /// attaching the member-use [span] as the primary span and building the
    /// trace from [trace]. Mirrors Dart's
    /// `MultiSpanSassScriptException.withSpan` + `withTrace`.
    pub fn with_member_use_span(self, span: SourceSpanWithContext, trace: Trace) -> Self {
        match self {
            SassError::MultiSpanScript {
                message,
                primary_label,
                secondary,
                cause,
                loaded_urls,
            } => SassError::MultiSpan {
                message,
                span,
                primary_label,
                secondary,
                original_source: None,
                cause,
                loaded_urls,
                trace,
            },
            other => other,
        }
    }
}

/// Creates a single stack frame from a span and member name.
///
/// Mirrors Dart's `frameForSpan` (`package:stack_trace`): the span's URL
/// (humanized upstream by the evaluator), its 1-based line/column, and the
/// member name. Matches Go: FrameForSpan(span FileSpan, member string).
pub fn frame_for_span(span: &SourceSpanWithContext, member: &str) -> Frame {
    Frame {
        uri: span.source_url.clone(),
        line: span.line(),
        column: span.column(),
        member: member.to_string(),
    }
}

/// Creates the single-frame trace Dart's `SassException.trace` getter returns:
/// `Trace([frameForSpan(span, "root stylesheet")])`.
pub fn trace_for_span(span: &SourceSpanWithContext, member: &str) -> Trace {
    Trace::new(vec![frame_for_span(span, member)])
}

/// Formats a single-span error with highlight and optional trace.
/// Matches Go: ErrorWithOptions pattern shared by SassException,
/// SassFormatException, SassRuntimeException.
fn format_single(
    message: &str,
    span: &SourceSpanWithContext,
    _primary_label: Option<&str>,
    _secondary: Option<&[(SourceSpanWithContext, String)]>,
    trace: Option<&Trace>,
    opts: &HighlightOptions,
    io: &dyn Io,
) -> String {
    let mut buf = format!("Error: {message}\n");
    match span.highlight(opts, io) {
        Ok(hl) => buf.push_str(&hl),
        Err(_) => buf.push_str("[error highlighting source]\n"),
    }
    let trace = match trace {
        Some(t) if !t.is_empty() => t.clone(),
        _ => trace_for_span(span, "root stylesheet"),
    };
    append_trace_lines(&mut buf, &trace, io);
    buf
}

/// Formats a multi-span error with highlight_multiple and optional trace.
/// Matches Go: ErrorWithOptions pattern for MultiSpanSassException,
/// MultiSpanSassRuntimeException.
fn format_multiple(
    message: &str,
    span: &SourceSpanWithContext,
    primary_label: &str,
    secondary: &[(SourceSpanWithContext, String)],
    trace: Trace,
    opts: &HighlightOptions,
    io: &dyn Io,
) -> String {
    let mut buf = format!("Error: {message}\n");
    match span.highlight_multiple(primary_label, secondary, opts, io) {
        Ok(hl) => buf.push_str(&hl),
        Err(_) => buf.push_str("[error highlighting source]\n"),
    }
    append_trace_lines(&mut buf, &trace, io);
    buf
}

fn append_trace_lines(buf: &mut String, trace: &Trace, io: &dyn Io) {
    let trace_str = trace.format(io);
    for frame_line in trace_str.split('\n') {
        if frame_line.is_empty() {
            continue;
        }
        buf.push_str("\n  ");
        buf.push_str(frame_line);
    }
}

impl From<ArgumentError> for SassError {
    fn from(e: ArgumentError) -> Self {
        SassError::Script {
            message: e.message,
            argument_name: None,
        }
    }
}

impl From<ArgumentError> for Box<SassError> {
    fn from(e: ArgumentError) -> Self {
        Box::new(e.into())
    }
}

impl From<RangeError> for Box<SassError> {
    fn from(e: RangeError) -> Self {
        Box::new(SassError::from(SpanError::from(e)))
    }
}

impl From<StateError> for Box<SassError> {
    fn from(e: StateError) -> Self {
        Box::new(SassError::Script {
            message: e.message,
            argument_name: None,
        })
    }
}

impl From<UnsupportedError> for Box<SassError> {
    fn from(e: UnsupportedError) -> Self {
        Box::new(SassError::Script {
            message: e.message,
            argument_name: None,
        })
    }
}

pub type SassResult<T> = Result<T, Box<SassError>>;

/// Scanner-level error carrying a `FileSpan` so the parser can adjust the
/// span (e.g. reposition zero-length "expected" errors at the preceding
/// newline) before converting to [`SassError`].
///
/// Mirrors Dart's `StringScannerException` (which carries `span` + the
/// scanned string; the string here lives in the [`FileSpan`]'s file).
/// Mirrors Go's `ScanError` (go/sasscommon/exception.go:489).
#[derive(Debug)]
pub struct ScanError<'parse> {
    /// The error message.
    pub message: String,
    /// The span of the failure.
    pub span: FileSpan<'parse>,
    /// The chained error, if any.
    pub cause: Option<Box<SassError>>,
}

impl Display for ScanError<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl<'parse> std::error::Error for ScanError<'parse> {}

/// Error type for [`SpanScanner`](crate::common::span_scanner::SpanScanner) operations.
///
/// Carries either a pre-existing [`SassError`] (e.g. from a `LazyFileSpan`
/// builder) or a scanner-created [`ScanError`]. The parser layer converts
/// this into `ParseError` and finally [`SassError`]; see
/// `docs/ref/common.md` for the layering.
#[derive(Debug, Error)]
pub enum SpanScannerError<'parse> {
    #[error(transparent)]
    Sass(Box<SassError>),
    #[error("{0}")]
    Scan(ScanError<'parse>),
}

impl<'parse> From<SassError> for SpanScannerError<'parse> {
    fn from(e: SassError) -> Self {
        SpanScannerError::Sass(Box::new(e))
    }
}

impl<'parse> From<Box<SassError>> for SpanScannerError<'parse> {
    fn from(e: Box<SassError>) -> Self {
        SpanScannerError::Sass(e)
    }
}

pub type SpanScannerResult<'parse, T> = Result<T, SpanScannerError<'parse>>;

// From impl #2: SpanScanner → SassError (for modules outside parse, e.g. css_identifier.rs)
impl From<SpanScannerError<'_>> for SassError {
    fn from(e: SpanScannerError<'_>) -> Self {
        match e {
            SpanScannerError::Sass(s) => *s,
            SpanScannerError::Scan(s) => {
                let fallback = SourceSpanWithContext::new(
                    SourceLocation {
                        offset: 0,
                        line: 0,
                        column: 0,
                    },
                    SourceLocation {
                        offset: 0,
                        line: 0,
                        column: 0,
                    },
                    String::new(),
                    String::new(),
                    None,
                )
                .unwrap_or_else(|_| unreachable!());
                SassError::Format {
                    message: s.message,
                    span: SourceSpanWithContext::from_file_span(&s.span).unwrap_or(fallback),
                    original_source: None,
                    cause: s.cause,
                    loaded_urls: vec![],
                }
            }
        }
    }
}

impl From<SpanScannerError<'_>> for Box<SassError> {
    fn from(e: SpanScannerError<'_>) -> Self {
        Box::new(e.into())
    }
}

impl From<SpanError> for SpanScannerError<'_> {
    fn from(e: SpanError) -> Self {
        match e {
            SpanError::Sass(s) => SpanScannerError::Sass(s),
            SpanError::Argument(msg) | SpanError::Range(msg) => {
                SpanScannerError::Sass(Box::new(SassError::Script {
                    message: msg,
                    argument_name: None,
                }))
            }
        }
    }
}

/// A single stack frame in a Sass trace.
///
/// Mirrors Dart's `Frame` from `package:stack_trace`: the stylesheet URI
/// (rendered via [`pretty_uri`](crate::common::pretty_uri::pretty_uri), `"-"`
/// when unknown), the 1-based line/column, and the member name (e.g.
/// `"root stylesheet"`).
#[derive(Debug, Clone)]
pub struct Frame {
    pub uri: Option<SassUrl>,
    pub line: usize,
    pub column: usize,
    pub member: String,
}

impl Frame {
    /// Returns the frame formatted as `"{library} {line}:{column}"`.
    ///
    /// Mirrors Dart's `Frame.toString()` (`'$lib ${line}:${column}  $member'`
    /// renders the member separately in [`Trace::format`]).
    pub fn location(&self, io: &dyn Io) -> String {
        let lib = self
            .uri
            .as_ref()
            .map(|u| pretty_uri(u, io))
            .unwrap_or_else(|| "-".to_string());
        format!("{lib} {}:{}", self.line, self.column)
    }
}

/// A Sass stack trace: an ordered list of [`Frame`]s.
///
/// Mirrors Dart's `Trace` from `package:stack_trace`. Carried structurally
/// through the evaluator and [`Logger`](crate::logger::Logger); rendered to a
/// string only at the output boundary via [`Trace::format`] (which needs the
/// [`Io`] seam for `prettyUri`, unlike Dart's ambient `Trace.toString()`).
/// Owns its frames, so it never borrows the `'parse` arena.
#[derive(Debug, Clone, Default)]
pub struct Trace {
    pub frames: Vec<Frame>,
}

impl Trace {
    /// Wraps `frames` (outermost → innermost, matching Dart).
    pub fn new(frames: Vec<Frame>) -> Self {
        Trace { frames }
    }

    /// Whether this trace has no frames.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Renders the trace as Dart's `Trace.toString()` does: each frame as
    /// `"{location}  {member}"`, with locations padded to the longest one and
    /// frames joined by `\n`. Returns `""` for an empty trace.
    pub fn format(&self, io: &dyn Io) -> String {
        if self.frames.is_empty() {
            return String::new();
        }
        let longest = self
            .frames
            .iter()
            .map(|f| f.location(io).len())
            .max()
            .unwrap_or(0);
        let mut buf = String::new();
        for (i, frame) in self.frames.iter().enumerate() {
            if i > 0 {
                buf.push('\n');
            }
            let loc = frame.location(io);
            let padding = longest - loc.len() + 2;
            write!(buf, "{loc}").unwrap();
            for _ in 0..padding {
                buf.push(' ');
            }
            buf.push_str(&frame.member);
        }
        buf
    }
}

impl Deref for Trace {
    type Target = [Frame];

    fn deref(&self) -> &[Frame] {
        &self.frames
    }
}

impl From<SpanError> for SassError {
    fn from(e: SpanError) -> Self {
        match e {
            SpanError::Sass(e) => *e,
            SpanError::Argument(msg) | SpanError::Range(msg) => SassError::Script {
                message: msg,
                argument_name: None,
            },
        }
    }
}

impl From<Box<SassError>> for SassError {
    fn from(e: Box<SassError>) -> Self {
        *e
    }
}

impl From<SpanError> for Box<SassError> {
    fn from(e: SpanError) -> Self {
        Box::new(e.into())
    }
}

/// Renders a Sass error as a CSS stylesheet that displays the error above the
/// current page using a `body::before` pseudo-element.
///
/// Mirrors Dart's `SassException.toCssString()`: the full error string (with
/// ASCII highlighting forced on, since the user's encoding may not be UTF-8)
/// becomes a `/* ... */` comment — with `*/` sequences replaced by a
/// lookalike that cannot close the comment and CRLF normalized to LF — while
/// non-ASCII characters are hex-escaped in the `content:` string so they
/// render even with wrong HTTP headers.
///
/// Matches Go: sasscommon.exceptionToCssString
pub fn exception_to_css_string(error: &SassError, io: &dyn Io) -> String {
    error.to_css_string(io)
}

/// Pure string transformation — see exception_to_css_string for public wrapper.
///
/// Matches Go: sasscommon.exceptionToCssString(message string) string
fn exception_to_css_string_impl(message: &str) -> String {
    let comment_message = message.replace("*/", "*\u{2215}").replace("\r\n", "\n");

    let mut content_buf = String::new();
    for c in message.chars() {
        if (c as u32) > 0x7F {
            write!(&mut content_buf, "\\{:x} ", c as u32).unwrap();
        } else {
            content_buf.push(c);
        }
    }

    let comment_lines: Vec<&str> = comment_message.split('\n').collect();
    let comment_buf = format!("/* {} */", comment_lines.join("\n * "));

    format!(
        "{comment_buf}\n\nbody::before {{\n  font-family: \"Source Code Pro\", \"SF Mono\", Monaco, Inconsolata, \"Fira Mono\",\n      \"Droid Sans Mono\", monospace, monospace;\n  white-space: pre;\n  display: block;\n  padding: 1em;\n  margin-bottom: 1em;\n  border-bottom: 2px solid black;\n  content: {content_buf};\n}}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::VirtualIo;

    fn test_io() -> VirtualIo {
        VirtualIo::with_cwd("/home/user")
    }

    #[test]
    fn test_full_message_with_argument_name() {
        // Matches Go: SassScriptException.Error() == "$name: message".
        let err = SassError::Script {
            message: "true is not a number.".into(),
            argument_name: Some("x".into()),
        };
        assert_eq!(err.full_message(), "$x: true is not a number.");
    }

    #[test]
    fn test_full_message_without_argument_name() {
        let err = SassError::Script {
            message: "At least one argument must be passed.".into(),
            argument_name: None,
        };
        assert_eq!(err.full_message(), "At least one argument must be passed.");
    }

    #[test]
    fn test_exception_to_css_string_impl_simple() {
        let got = exception_to_css_string_impl("test error");
        let want = [
            "/* test error */",
            "",
            "body::before {",
            "  font-family: \"Source Code Pro\", \"SF Mono\", Monaco, Inconsolata, \"Fira Mono\",",
            "      \"Droid Sans Mono\", monospace, monospace;",
            "  white-space: pre;",
            "  display: block;",
            "  padding: 1em;",
            "  margin-bottom: 1em;",
            "  border-bottom: 2px solid black;",
            "  content: test error;",
            "}",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn test_exception_to_css_string_impl_multiline() {
        let got = exception_to_css_string_impl("line1\nline2");
        let want = vec![
            "/* line1",
            " * line2 */",
            "",
            "body::before {",
            "  font-family: \"Source Code Pro\", \"SF Mono\", Monaco, Inconsolata, \"Fira Mono\",",
            "      \"Droid Sans Mono\", monospace, monospace;",
            "  white-space: pre;",
            "  display: block;",
            "  padding: 1em;",
            "  margin-bottom: 1em;",
            "  border-bottom: 2px solid black;",
            "  content: line1\nline2;",
            "}",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn test_exception_to_css_string_impl_close_comment() {
        let got = exception_to_css_string_impl("hello */ world");
        let want = [
            "/* hello *\u{2215} world */",
            "",
            "body::before {",
            "  font-family: \"Source Code Pro\", \"SF Mono\", Monaco, Inconsolata, \"Fira Mono\",",
            "      \"Droid Sans Mono\", monospace, monospace;",
            "  white-space: pre;",
            "  display: block;",
            "  padding: 1em;",
            "  margin-bottom: 1em;",
            "  border-bottom: 2px solid black;",
            "  content: hello */ world;",
            "}",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn test_exception_to_css_string_impl_crlf() {
        let got = exception_to_css_string_impl("line1\r\nline2");
        let want = vec![
            "/* line1",
            " * line2 */",
            "",
            "body::before {",
            "  font-family: \"Source Code Pro\", \"SF Mono\", Monaco, Inconsolata, \"Fira Mono\",",
            "      \"Droid Sans Mono\", monospace, monospace;",
            "  white-space: pre;",
            "  display: block;",
            "  padding: 1em;",
            "  margin-bottom: 1em;",
            "  border-bottom: 2px solid black;",
            "  content: line1\r\nline2;",
            "}",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn test_exception_to_css_string_impl_non_ascii() {
        let got = exception_to_css_string_impl("caf\u{00e9}");
        let want = [
            "/* caf\u{00e9} */",
            "",
            "body::before {",
            "  font-family: \"Source Code Pro\", \"SF Mono\", Monaco, Inconsolata, \"Fira Mono\",",
            "      \"Droid Sans Mono\", monospace, monospace;",
            "  white-space: pre;",
            "  display: block;",
            "  padding: 1em;",
            "  margin-bottom: 1em;",
            "  border-bottom: 2px solid black;",
            "  content: caf\\e9 ;",
            "}",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn test_exception_to_css_string_impl_real_error() {
        let msg = [
            "Error: expected selector.",
            "  \u{2557}",
            "1 \u{2502} .foo {",
            "  \u{2502}      ^",
            "  \u{2555}",
            "  - 1:6  root stylesheet",
        ]
        .join("\n");
        let got = exception_to_css_string_impl(&msg);
        // Box-drawing chars are > 0x7F so hex-escaped in content,
        // preserved as-is in the comment.
        let want = vec![
            "/* Error: expected selector.",
            " *   \u{2557}",
            " * 1 \u{2502} .foo {",
            " *   \u{2502}      ^",
            " *   \u{2555}",
            " *   - 1:6  root stylesheet */",
            "",
            "body::before {",
            "  font-family: \"Source Code Pro\", \"SF Mono\", Monaco, Inconsolata, \"Fira Mono\",",
            "      \"Droid Sans Mono\", monospace, monospace;",
            "  white-space: pre;",
            "  display: block;",
            "  padding: 1em;",
            "  margin-bottom: 1em;",
            "  border-bottom: 2px solid black;",
            "  content: Error: expected selector.\n  \\2557 \n1 \\2502  .foo {\n  \\2502       ^\n  \\2555 \n  - 1:6  root stylesheet;",
            "}",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn test_exception_to_css_string_impl_empty() {
        let got = exception_to_css_string_impl("");
        let want = [
            "/*  */",
            "",
            "body::before {",
            "  font-family: \"Source Code Pro\", \"SF Mono\", Monaco, Inconsolata, \"Fira Mono\",",
            "      \"Droid Sans Mono\", monospace, monospace;",
            "  white-space: pre;",
            "  display: block;",
            "  padding: 1em;",
            "  margin-bottom: 1em;",
            "  border-bottom: 2px solid black;",
            "  content: ;",
            "}",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn test_exception_to_css_string_impl_quote_chars() {
        let got = exception_to_css_string_impl("\"quoted\"");
        let want = [
            "/* \"quoted\" */",
            "",
            "body::before {",
            "  font-family: \"Source Code Pro\", \"SF Mono\", Monaco, Inconsolata, \"Fira Mono\",",
            "      \"Droid Sans Mono\", monospace, monospace;",
            "  white-space: pre;",
            "  display: block;",
            "  padding: 1em;",
            "  margin-bottom: 1em;",
            "  border-bottom: 2px solid black;",
            "  content: \"quoted\";",
            "}",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn test_exception_to_css_string_impl_backslash() {
        let got = exception_to_css_string_impl("path\\to\\file");
        let want = [
            "/* path\\to\\file */",
            "",
            "body::before {",
            "  font-family: \"Source Code Pro\", \"SF Mono\", Monaco, Inconsolata, \"Fira Mono\",",
            "      \"Droid Sans Mono\", monospace, monospace;",
            "  white-space: pre;",
            "  display: block;",
            "  padding: 1em;",
            "  margin-bottom: 1em;",
            "  border-bottom: 2px solid black;",
            "  content: path\\to\\file;",
            "}",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn test_exception_to_css_string_with_sass_error() {
        // Script Display is just the message (no "Error: " prefix).
        let err = SassError::Script {
            message: "test error".into(),
            argument_name: None,
        };
        let got = exception_to_css_string(&err, &test_io());
        let want = [
            "/* test error */",
            "",
            "body::before {",
            "  font-family: \"Source Code Pro\", \"SF Mono\", Monaco, Inconsolata, \"Fira Mono\",",
            "      \"Droid Sans Mono\", monospace, monospace;",
            "  white-space: pre;",
            "  display: block;",
            "  padding: 1em;",
            "  margin-bottom: 1em;",
            "  border-bottom: 2px solid black;",
            "  content: test error;",
            "}",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn test_exception_to_css_string_with_script_error_prefixed() {
        // Script's Display now includes $name: prefix (matching Go).
        let err = SassError::Script {
            message: "true is not a number.".into(),
            argument_name: Some("x".into()),
        };
        let got = exception_to_css_string(&err, &test_io());
        let want = [
            "/* $x: true is not a number. */",
            "",
            "body::before {",
            "  font-family: \"Source Code Pro\", \"SF Mono\", Monaco, Inconsolata, \"Fira Mono\",",
            "      \"Droid Sans Mono\", monospace, monospace;",
            "  white-space: pre;",
            "  display: block;",
            "  padding: 1em;",
            "  margin-bottom: 1em;",
            "  border-bottom: 2px solid black;",
            "  content: $x: true is not a number.;",
            "}",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn test_frame_location_with_uri() {
        let frame = Frame {
            uri: Some(SassUrl::parse("file:///a.scss").unwrap()),
            line: 1,
            column: 1,
            member: "root stylesheet".to_string(),
        };
        // pretty_uri strips file:// prefix => "/a.scss"
        assert_eq!(frame.location(&test_io()), "/a.scss 1:1");
    }

    #[test]
    fn test_frame_location_nil_uri() {
        let frame = Frame {
            uri: None,
            line: 1,
            column: 7,
            member: "root stylesheet".to_string(),
        };
        assert_eq!(frame.location(&test_io()), "- 1:7");
    }

    #[test]
    fn test_trace_format_single() {
        let trace = Trace::new(vec![Frame {
            uri: None,
            line: 1,
            column: 7,
            member: "root stylesheet".to_string(),
        }]);
        assert_eq!(trace.format(&test_io()), "- 1:7  root stylesheet");
    }

    #[test]
    fn test_trace_format_multiple() {
        let trace = Trace::new(vec![
            Frame {
                uri: None,
                line: 1,
                column: 7,
                member: "root stylesheet".to_string(),
            },
            Frame {
                uri: None,
                line: 123,
                column: 45,
                member: "some other member".to_string(),
            },
        ]);
        // longest = "- 123:45" len=8; "- 1:7" len=5; padding = 8-5+2 = 5
        let want = ["- 1:7     root stylesheet", "- 123:45  some other member"].join("\n");
        assert_eq!(trace.format(&test_io()), want);
    }

    #[test]
    fn test_trace_format_empty() {
        assert_eq!(Trace::default().format(&test_io()), "");
        assert!(Trace::default().is_empty());
    }

    #[test]
    fn test_frame_for_span() {
        let span = SourceSpanWithContext::new(
            SourceLocation {
                offset: 4,
                line: 0,
                column: 4,
            },
            SourceLocation {
                offset: 5,
                line: 0,
                column: 5,
            },
            "c".to_string(),
            "a { color: red; }".to_string(),
            None,
        )
        .unwrap();
        let frame = frame_for_span(&span, "root stylesheet");
        assert_eq!(frame.line, 1);
        assert_eq!(frame.column, 5);
        assert_eq!(frame.member, "root stylesheet");
        assert!(frame.uri.is_none());

        let trace = trace_for_span(&span, "root stylesheet");
        assert_eq!(trace.frames.len(), 1);
        assert_eq!(trace.frames[0].member, "root stylesheet");
    }

    #[test]
    fn test_to_error_string_script() {
        let err = SassError::Script {
            message: "bad".into(),
            argument_name: None,
        };
        assert_eq!(err.to_error_string(&test_io()), "bad");
    }

    #[test]
    fn test_to_error_string_script_with_arg() {
        let err = SassError::Script {
            message: "true is not a number.".into(),
            argument_name: Some("x".into()),
        };
        assert_eq!(err.to_error_string(&test_io()), "$x: true is not a number.");
    }

    #[test]
    fn test_to_error_string_sass_includes_highlight() {
        let span = SourceSpanWithContext::new(
            SourceLocation {
                offset: 4,
                line: 0,
                column: 4,
            },
            SourceLocation {
                offset: 5,
                line: 0,
                column: 5,
            },
            "c".to_string(),
            "a { color: red; }".to_string(),
            None,
        )
        .unwrap();
        let err = SassError::Sass {
            message: "expected selector.".into(),
            span,
            cause: None,
            loaded_urls: vec![],
        };
        let got = err.to_error_string(&test_io());
        assert!(got.starts_with("Error: expected selector.\n"));
        assert!(got.contains("\u{2577}"), "should contain top border");
        assert!(got.contains("  -"), "should contain trace frame");
    }

    #[test]
    fn test_to_css_string_includes_highlight() {
        let span = SourceSpanWithContext::new(
            SourceLocation {
                offset: 4,
                line: 0,
                column: 4,
            },
            SourceLocation {
                offset: 5,
                line: 0,
                column: 5,
            },
            "c".to_string(),
            "a { color: red; }".to_string(),
            None,
        )
        .unwrap();
        let err = SassError::Sass {
            message: "expected selector.".into(),
            span,
            cause: None,
            loaded_urls: vec![],
        };
        let got = err.to_css_string(&test_io());
        assert!(got.contains("/* Error: expected selector."));
        assert!(
            got.contains("\u{2577}"),
            "comment should contain box-drawing"
        );
    }

    #[test]
    fn test_to_css_string_multispan() {
        let span = SourceSpanWithContext::new(
            SourceLocation {
                offset: 4,
                line: 0,
                column: 4,
            },
            SourceLocation {
                offset: 5,
                line: 0,
                column: 5,
            },
            "c".to_string(),
            "a { color: red; }".to_string(),
            None,
        )
        .unwrap();
        let secondary_span = SourceSpanWithContext::new(
            SourceLocation {
                offset: 0,
                line: 0,
                column: 0,
            },
            SourceLocation {
                offset: 1,
                line: 0,
                column: 1,
            },
            "b".to_string(),
            "body { color: blue; }".to_string(),
            None,
        )
        .unwrap();
        let err = SassError::MultiSpan {
            message: "module loop".into(),
            span,
            primary_label: Some("new load".into()),
            secondary: vec![(secondary_span, "original load".into())],
            original_source: None,
            cause: None,
            loaded_urls: vec![],
            trace: Trace::default(),
        };
        let got = err.to_css_string(&test_io());
        assert!(got.contains("/* Error: module loop"));
        assert!(got.contains("new load"));
        assert!(got.contains("original load"));
    }

    #[test]
    fn test_to_error_string_runtime_with_trace() {
        let span = SourceSpanWithContext::new(
            SourceLocation {
                offset: 4,
                line: 0,
                column: 4,
            },
            SourceLocation {
                offset: 5,
                line: 0,
                column: 5,
            },
            "c".to_string(),
            "a { color: red; }".to_string(),
            None,
        )
        .unwrap();
        let err = SassError::Runtime {
            message: "something broke".into(),
            span,
            trace: Trace::new(vec![
                Frame {
                    uri: None,
                    line: 1,
                    column: 7,
                    member: "root stylesheet".to_string(),
                },
                Frame {
                    uri: None,
                    line: 123,
                    column: 45,
                    member: "some other member".to_string(),
                },
            ]),
            cause: None,
            loaded_urls: vec![],
        };
        let got = err.to_error_string(&test_io());
        assert!(got.starts_with("Error: something broke\n"));
        assert!(got.contains("\n  - 1:7     root stylesheet\n"));
        assert!(got.contains("\n  - 123:45  some other member"));
    }
}
