// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/logger/stderr.dart
// go-source: go/sasslogger/logger_stderr.go

//! The terminal logger. Matches Dart: `StderrLogger` in `logger/stderr.dart`
//! (a `LoggerWithDeprecationType` writing via `printError`).

use crate::common::pretty_uri::pretty_uri;
use crate::common::source_span_highlighter::HighlightColor;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::fmt::Write;
use std::io::Write as IoWrite;
use std::rc::Rc;

use crate::common::exception::{SassResult, Trace};
use crate::common::source_span_highlighter::HighlightOptions;
use crate::common::span::Span;
use crate::deprecation::Deprecation;
use crate::io::Io;
use crate::termglyph::GlyphSet;

use crate::logger::Logger;

const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_BLUE: &str = "\x1b[34m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_RESET: &str = "\x1b[0m";

/// A logger that prints warnings to standard error (or the browser console).
///
/// `color` selects ANSI-colored output; `unicode` selects the [`GlyphSet`] for
/// source-frame highlighting (Dart reads the global `glyph.ascii` instead).
/// `io` supplies `current_dir` for URL prettifying (see `pretty_uri`).
/// Output goes through `write_fn` (default: `stderr`), which makes the
/// rendered text capturable in tests.
pub struct StderrLogger {
    color: bool,
    pub unicode: bool,
    io: Rc<dyn Io>,
    write_fn: Box<dyn Fn(String) + Send>,
}

impl Debug for StderrLogger {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StderrLogger")
            .field("color", &self.color)
            .field("unicode", &self.unicode)
            .finish()
    }
}

impl StderrLogger {
    /// Creates a logger writing to stderr. Matches Dart's
    /// `const StderrLogger({color = false})` (plus explicit `unicode` and the
    /// `Io` needed for URL prettifying).
    pub fn new(color: bool, unicode: bool, io: Rc<dyn Io>) -> Self {
        StderrLogger {
            color,
            unicode,
            io,
            write_fn: stderr_write_fn(),
        }
    }

    /// Creates a logger with an injectable `write_fn` sink (test seam;
    /// Rust-only, no Dart counterpart).
    pub fn new_with_write_fn<F>(color: bool, unicode: bool, io: Rc<dyn Io>, write_fn: F) -> Self
    where
        F: Fn(String) + Send + 'static,
    {
        StderrLogger {
            color,
            unicode,
            io,
            write_fn: Box::new(write_fn),
        }
    }

    // Sends already-rendered text to the sink (Dart's `printError` call at
    // the end of `internalWarn`).
    fn write_out(&self, text: &str) {
        (self.write_fn)(text.to_string());
    }
}

// Default sink: appends to the real stderr. Rust-only (Dart calls the
// `printError` io-function directly).
fn stderr_write_fn() -> Box<dyn Fn(String) + Send> {
    Box::new(|text: String| {
        let _ = std::io::stderr().lock().write_all(text.as_bytes());
    })
}

impl Logger for StderrLogger {
    /// Renders and writes a warning. Matches Dart's `internalWarn` with
    /// `deprecation: None`.
    fn warn<'a>(&self, message: &str, span: Option<&Span<'a>>, trace: Option<&Trace>) {
        let _ = self.internal_warn(message, span, trace, None);
    }

    /// Renders and writes a `@debug` line. Matches Dart's `StderrLogger.debug`
    /// (`prettyUri` location + `Debug` label, written via `printError`).
    fn debug<'a>(&self, message: &str, span: Option<&Span<'a>>) {
        let result = render_debug(self.color, self.io.as_ref(), message, span);
        self.write_out(&result);
    }

    /// Renders and writes a deprecation warning. Matches Dart's
    /// `internalWarn` with `deprecation: Some(..)`.
    fn warn_deprecation<'a>(
        &self,
        message: &str,
        span: Option<&Span<'a>>,
        deprecation: &'static Deprecation,
        trace: Option<&Trace>,
    ) -> SassResult<()> {
        self.internal_warn(message, span, trace, Some(deprecation))
    }
}

impl StderrLogger {
    // Shared `internalWarn` body: stringifies the trace at this render
    // boundary (Dart: `trace.toString()`), renders the full block (header +
    // optional highlight + indented trace), then writes it. Matches Dart's
    // `StderrLogger.internalWarn`.
    fn internal_warn(
        &self,
        msg: &str,
        span: Option<&Span<'_>>,
        trace: Option<&Trace>,
        dep: Option<&Deprecation>,
    ) -> SassResult<()> {
        let stack = trace
            .map(|t| t.format(self.io.as_ref()))
            .unwrap_or_default();
        let block = render_warning(
            self.color,
            self.unicode,
            self.io.as_ref(),
            dep,
            msg,
            span,
            &stack,
        )?;
        self.write_out(&block);
        Ok(())
    }
}

/// Renders a `@debug` line (e.g. `-:1 DEBUG: message\n`), mirroring the Dart
/// `StderrLogger.debug` block: `prettyUri` location (`-` when the span has no
/// source URL), 1-based line, then the `Debug`/`DEBUG` label. `color` bolds
/// the label; span-less messages use the `-:1` pseudo location.
pub fn render_debug(color: bool, io: &dyn Io, message: &str, span: Option<&Span<'_>>) -> String {
    let mut result = String::new();

    if let Some(s) = span {
        let url = match s.source_url() {
            Ok(Some(u)) => pretty_uri(&u, io),
            _ => "-".to_string(),
        };
        let start_loc = s.start_location().ok();
        let line = start_loc.map_or(1, |l| l.line + 1);
        if color {
            let _ = writeln!(
                result,
                "{url}:{line} {ANSI_BOLD}Debug{ANSI_RESET}: {message}"
            );
        } else {
            let _ = writeln!(result, "{url}:{line} DEBUG: {message}");
        }
    } else if color {
        let _ = writeln!(result, "-:1 {ANSI_BOLD}Debug{ANSI_RESET}: {message}");
    } else {
        let _ = writeln!(result, "-:1 DEBUG: {message}");
    }

    result
}

/// Renders a full warning/deprecation block, byte-for-byte the text
/// `StderrLogger` writes: `Warning`/`Deprecation Warning` header (bold yellow
/// when `color`), bracketed deprecation id (hidden for `user-authored`,
/// matching Dart's `showDeprecation`), then the span rendering — bare message
/// without a span, highlight plus trace with span and stack, `on <message>`
/// with span alone — indented trace, and a trailing blank line.
/// `unicode` selects the `GlyphSet`. Exposed as a string-returning helper so
/// non-terminal loggers (e.g. the wasm JS-API default logger forwarding to
/// `console.warn`) produce identical output.
pub fn render_warning(
    color: bool,
    unicode: bool,
    io: &dyn Io,
    dep: Option<&Deprecation>,
    msg: &str,
    span: Option<&Span<'_>>,
    stack: &str,
) -> SassResult<String> {
    let mut result = String::new();
    let show_deprecation = dep.is_some_and(|d| d.id != "user-authored");
    let glyphs = if unicode {
        GlyphSet::default()
    } else {
        GlyphSet::Ascii
    };
    let color_mode = if color {
        HighlightColor::Default
    } else {
        HighlightColor::None
    };
    let opts = HighlightOptions {
        color: color_mode,
        glyphs,
        ..Default::default()
    };

    if color {
        result.push_str(ANSI_YELLOW);
        result.push_str(ANSI_BOLD);
        if dep.is_some() {
            result.push_str("Deprecation ");
        }
        result.push_str("Warning");
        result.push_str(ANSI_RESET);
        if show_deprecation {
            let _ = write!(result, " [{ANSI_BLUE}{}{ANSI_RESET}]", dep.unwrap().id);
        }
    } else {
        if dep.is_some() {
            result.push_str("DEPRECATION ");
        }
        result.push_str("WARNING");
        if show_deprecation {
            let _ = write!(result, " [{}]", dep.unwrap().id);
        }
    }

    match span {
        None => {
            let _ = writeln!(result, ": {msg}");
        }
        Some(s) if !stack.is_empty() => {
            let _ = writeln!(result, ": {msg}\n");
            let hl = s.highlight(&opts, io)?;
            let _ = write!(result, "{hl}");
            let _ = writeln!(result);
        }
        Some(s) => {
            let msg_line = format!("\n{msg}");
            let msg_out = s.message(&msg_line, &opts, io)?;
            let _ = write!(result, " on {msg_out}");
            let _ = writeln!(result);
        }
    }

    if !stack.is_empty() {
        let _ = writeln!(result, "{}", indent(stack, 4));
    }

    result.push('\n');

    Ok(result)
}

// Indents every line of `s` by `n` spaces for the stack-trace block.
// Matches Dart's `indent(trace.toString().trimRight(), 4)` idiom.
fn indent(s: &str, n: usize) -> String {
    let prefix = " ".repeat(n);
    s.split('\n')
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use crate::common::exception::Frame;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::url::SassUrl;
    use bumpalo::Bump;

    use super::*;
    use crate::deprecation::{CALL_STRING, SLASH_DIV, USER_AUTHORED};
    use crate::io::VirtualIo;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    fn capture_logger(color: bool, unicode: bool) -> (StderrLogger, Arc<Mutex<String>>) {
        let output = Arc::new(Mutex::new(String::new()));
        let out = output.clone();
        let io = Rc::new(VirtualIo::with_cwd("/home/user"));
        let l = StderrLogger::new_with_write_fn(color, unicode, io, move |s| {
            out.lock().unwrap().push_str(&s)
        });
        (l, output)
    }

    fn captured(out: &Arc<Mutex<String>>) -> String {
        out.lock().unwrap().clone()
    }

    fn make_span<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
        start: usize,
        end: usize,
    ) -> Span<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        Span::File(FileSpan::new(Some(fs), start, end))
    }

    fn make_span_url<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
        url_str: &str,
        start: usize,
        end: usize,
    ) -> Span<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let u = SassUrl::parse(url_str).unwrap();
        let fs = FileSource::new_in(arena, text, Some(u));
        Span::File(FileSpan::new(Some(fs), start, end))
    }

    #[test]
    fn test_stderr_warn_no_span() {
        let (l, out) = capture_logger(false, true);
        l.warn("test message", None, None);
        let expected = "WARNING: test message\n\n";
        assert_eq!(captured(&out), expected);
    }

    #[test]
    fn test_stderr_warn_with_span() {
        let arena = Bump::new();
        let (l, out) = capture_logger(false, false);
        let span = make_span(&arena, "body { color: red; }\n", 0, 10);
        l.warn("test message", Some(&span), None);
        let expected = concat!(
            "WARNING on line 1, column 1: \n",
            "test message\n",
            "  ,\n",
            "1 | body { color: red; }\n",
            "  | ^^^^^^^^^^\n",
            "  '\n",
            "\n",
        );
        assert_eq!(captured(&out), expected);
    }

    #[test]
    fn test_stderr_warn_with_stack() {
        let arena = Bump::new();
        let (l, out) = capture_logger(false, false);
        let span = make_span(&arena, "body { color: red; }\n", 0, 10);
        let trace = Trace::new(vec![Frame {
            uri: None,
            line: 1,
            column: 1,
            member: "stack trace".to_string(),
        }]);
        l.warn("test message", Some(&span), Some(&trace));
        let expected = concat!(
            "WARNING: test message\n",
            "\n",
            "  ,\n",
            "1 | body { color: red; }\n",
            "  | ^^^^^^^^^^\n",
            "  '\n",
            "    - 1:1  stack trace\n",
            "\n",
        );
        assert_eq!(captured(&out), expected);
    }

    #[test]
    fn test_stderr_warn_deprecation() {
        let (l, out) = capture_logger(false, true);
        assert!(l
            .warn_deprecation("deprecated feature", None, &CALL_STRING, None)
            .is_ok());
        let expected = "DEPRECATION WARNING [call-string]: deprecated feature\n\
                        \n";
        assert_eq!(captured(&out), expected);
    }

    #[test]
    fn test_stderr_warn_deprecation_no_span() {
        let (l, out) = capture_logger(false, true);
        assert!(l.warn_deprecation("msg", None, &SLASH_DIV, None).is_ok());
        let expected = "DEPRECATION WARNING [slash-div]: msg\n\
                        \n";
        assert_eq!(captured(&out), expected);
    }

    #[test]
    fn test_stderr_warn_user_authored() {
        let (l, out) = capture_logger(false, true);
        assert!(l
            .warn_deprecation("msg", None, &USER_AUTHORED, None)
            .is_ok());
        let expected = "DEPRECATION WARNING: msg\n\
                        \n";
        assert_eq!(captured(&out), expected);
    }

    #[test]
    fn test_stderr_debug_no_span() {
        let (l, out) = capture_logger(false, true);
        l.debug("debug msg", None);
        let expected = "-:1 DEBUG: debug msg\n";
        assert_eq!(captured(&out), expected);
    }

    #[test]
    fn test_stderr_debug_with_span() {
        let arena = Bump::new();
        let (l, out) = capture_logger(false, true);
        let span = make_span_url(&arena, "body { }\n", "file:///test.scss", 0, 8);
        l.debug("debug msg", Some(&span));
        let expected = "/test.scss:1 DEBUG: debug msg\n";
        assert_eq!(captured(&out), expected);
    }

    #[test]
    fn test_stderr_color_output() {
        let (l, out) = capture_logger(true, true);
        l.warn("test", None, None);
        let expected = "\x1b[33m\x1b[1mWarning\x1b[0m: test\n\
                        \n";
        assert_eq!(captured(&out), expected);
    }

    #[test]
    fn test_stderr_color_deprecation_output() {
        let (l, out) = capture_logger(true, true);
        assert!(l.warn_deprecation("test", None, &CALL_STRING, None).is_ok());
        let expected =
            "\x1b[33m\x1b[1mDeprecation Warning\x1b[0m [\x1b[34mcall-string\x1b[0m]: test\n\
                        \n";
        assert_eq!(captured(&out), expected);
    }

    #[test]
    fn test_stderr_unicode_glyphs() {
        let arena = Bump::new();
        let (l, out) = capture_logger(false, true);
        let span = make_span(&arena, "body { color: red; }\n", 0, 10);
        l.warn("test", Some(&span), None);
        let expected = concat!(
            "WARNING on line 1, column 1: \n",
            "test\n",
            "  \u{2577}\n",
            "1 \u{2502} body { color: red; }\n",
            "  \u{2502} ^^^^^^^^^^\n",
            "  \u{2575}\n",
            "\n",
        );
        assert_eq!(captured(&out), expected);
    }

    #[test]
    fn test_stderr_ascii_glyphs() {
        let arena = Bump::new();
        let (l, out) = capture_logger(false, false);
        let span = make_span(&arena, "body { color: red; }\n", 0, 10);
        l.warn("test", Some(&span), None);
        let expected = concat!(
            "WARNING on line 1, column 1: \n",
            "test\n",
            "  ,\n",
            "1 | body { color: red; }\n",
            "  | ^^^^^^^^^^\n",
            "  '\n",
            "\n",
        );
        assert_eq!(captured(&out), expected);
    }
}
