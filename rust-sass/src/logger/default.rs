// Copyright 2026 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/logger/default.dart
// go-source: go/sasslogger/logger_default.go

//! The default logger: a [`StderrLogger`] with capability-detected color.
//!
//! Matches Dart: `DefaultLogger` in `logger/default.dart` (a
//! `LoggerWithDeprecationType` delegating to a lazily-initialized global
//! `StderrLogger(color: supportsAnsiEscapes)`). Rust cannot lazily share one
//! global instance (no global `Io`), so [`new_default_logger`] constructs a
//! fresh `StderrLogger` per call instead.

use std::io::IsTerminal;
use std::rc::Rc;

use crate::io::Io;

use crate::logger::StderrLogger;

/// Whether stderr supports ANSI escapes: `false` for `TERM=dumb`, `true` for
/// `COLORTERM=truecolor`/`24bit`, else a terminal probe. Rust-only free
/// function (Dart reads `stdout.supportsAnsiEscapes` via `io.dart`; here
/// stderr is probed since that is where warnings go).
pub fn supports_ansi_escapes() -> bool {
    if std::env::var("TERM").is_ok_and(|v| v == "dumb") {
        return false;
    }
    if let Ok(ct) = std::env::var("COLORTERM") {
        if ct == "truecolor" || ct == "24bit" {
            return true;
        }
    }
    std::io::stderr().is_terminal()
}

/// Creates the logger used when no other is selected: a [`StderrLogger`] with
/// color from [`supports_ansi_escapes`] and caller-chosen `unicode`.
/// Matches Dart's `Logger.defaultLogger`.
pub fn new_default_logger(unicode: bool, io: Rc<dyn Io>) -> StderrLogger {
    StderrLogger::new(supports_ansi_escapes(), unicode, io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::common::span::Span;
    use crate::io::VirtualIo;
    use crate::logger::Logger;
    use bumpalo::Bump;
    use std::sync::{Arc, Mutex};

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

    fn make_logger(unicode: bool) -> (StderrLogger, Arc<Mutex<String>>) {
        let output = Arc::new(Mutex::new(String::new()));
        let out = output.clone();
        let io = Rc::new(VirtualIo::new());
        let l = StderrLogger::new_with_write_fn(false, unicode, io, move |s| {
            out.lock().unwrap().push_str(&s)
        });
        (l, output)
    }

    #[test]
    fn test_new_default_logger_unicode_true() {
        let arena = Bump::new();
        let (l, out) = make_logger(true);
        let span = make_span(&arena, "a { b: c; }\n", 0, 1);
        l.warn("test", Some(&span), None);
        let expected = concat!(
            "WARNING on line 1, column 1: \n",
            "test\n",
            "  \u{2577}\n",
            "1 \u{2502} a { b: c; }\n",
            "  \u{2502} ^\n",
            "  \u{2575}\n",
            "\n",
        );
        assert_eq!(out.lock().unwrap().clone(), expected);
    }

    #[test]
    fn test_new_default_logger_unicode_false() {
        let arena = Bump::new();
        let (l, out) = make_logger(false);
        let span = make_span(&arena, "a { b: c; }\n", 0, 1);
        l.warn("test", Some(&span), None);
        let expected = concat!(
            "WARNING on line 1, column 1: \n",
            "test\n",
            "  ,\n",
            "1 | a { b: c; }\n",
            "  | ^\n",
            "  '\n",
            "\n",
        );
        assert_eq!(out.lock().unwrap().clone(), expected);
    }
}
