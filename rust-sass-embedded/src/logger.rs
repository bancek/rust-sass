// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/embedded/logger.dart
// go-source: go/embedded/logger.go

use std::fmt;
use std::rc::Rc;

use rust_sass::common::exception::{SassResult, Trace};
use rust_sass::common::pretty_uri::pretty_uri;
use rust_sass::common::source_span_span_with_context::SourceSpanWithContext;
use rust_sass::common::span::Span;
use rust_sass::deprecation::{Deprecation, USER_AUTHORED};
use rust_sass::logger::Logger;

use crate::compilation::HostContext;
use crate::embedded_sass::outbound_message;
use crate::embedded_sass::{LogEventType, OutboundMessage, SourceSpan};
use crate::error::{highlight_options, protofy_span_with_context};
use crate::packet::write_message_shared;

/// Sends log messages as `LogEvent`s to the host.
///
/// Holds only `'static` data (`Rc<HostContext>`), so it can be stored as
/// `Rc<dyn Logger>` (a `'static` trait object), matching `EvalConfig.logger`.
///
/// Matches Dart: `EmbeddedLogger`.
pub struct EmbeddedLogger {
    ctx: Rc<HostContext>,
    color: bool,
    ascii: bool,
}

impl EmbeddedLogger {
    pub fn new(ctx: Rc<HostContext>, color: bool, ascii: bool) -> Self {
        EmbeddedLogger { ctx, color, ascii }
    }

    fn internal_warn(
        &self,
        message: &str,
        span: Option<&Span>,
        trace: Option<&Trace>,
        deprecation: Option<&'static Deprecation>,
    ) {
        let stack = trace
            .map(|t| t.format(self.ctx.io.as_ref()))
            .unwrap_or_default();
        let formatted = self.format_warning(message, span, &stack, deprecation);
        let event = outbound_message::LogEvent {
            r#type: if deprecation.is_some() {
                LogEventType::DeprecationWarning as i32
            } else {
                LogEventType::Warning as i32
            },
            message: message.to_string(),
            span: protofy_span(span),
            stack_trace: if stack.is_empty() {
                String::new()
            } else {
                format!("{stack}\n")
            },
            formatted,
            deprecation_type: deprecation.map(|d| d.id.to_string()),
        };
        let msg = OutboundMessage {
            message: Some(outbound_message::Message::LogEvent(event)),
        };
        let _ = write_message_shared(&self.ctx.writer, self.ctx.compilation_id, &msg);
    }

    fn format_warning(
        &self,
        message: &str,
        span: Option<&Span>,
        stack: &str,
        dep: Option<&'static Deprecation>,
    ) -> String {
        let opts = highlight_options(self.color, self.ascii);
        let mut result = String::new();
        let show_deprecation = dep.is_some() && dep.unwrap() != &USER_AUTHORED;

        if self.color {
            result.push_str("\u{1b}[33m\u{1b}[1m");
            if dep.is_some() {
                result.push_str("Deprecation ");
            }
            result.push_str("Warning\u{1b}[0m");
            if show_deprecation {
                result.push_str(&format!(" [\u{1b}[34m{}\u{1b}[0m]", dep.unwrap().id));
            }
        } else {
            if dep.is_some() {
                result.push_str("DEPRECATION ");
            }
            result.push_str("WARNING");
            if show_deprecation {
                result.push_str(&format!(" [{}]", dep.unwrap().id));
            }
        }

        let ssc = span
            .and_then(|s| s.file_span().ok())
            .and_then(|fs| SourceSpanWithContext::from_file_span(&fs).ok());
        match (&ssc, stack.is_empty()) {
            (None, _) => {
                result.push_str(&format!(": {message}\n"));
            }
            (Some(s), false) => {
                result.push_str(&format!(": {message}\n\n"));
                if let Ok(hl) = s.highlight(&opts, self.ctx.io.as_ref()) {
                    result.push_str(&hl);
                    result.push('\n');
                }
            }
            (Some(s), true) => {
                result.push_str(" on ");
                if let Ok(m) = s.message(&format!("\n{message}"), &opts, self.ctx.io.as_ref()) {
                    result.push_str(&m);
                }
                result.push('\n');
            }
        }
        if !stack.is_empty() {
            result.push_str(&indent(stack.trim_end_matches([' ', '\t', '\n', '\r']), 4));
            result.push('\n');
        }
        result
    }

    fn format_debug(&self, message: &str, span: Option<&Span>) -> String {
        let ssc = span
            .and_then(|s| s.file_span().ok())
            .and_then(|fs| SourceSpanWithContext::from_file_span(&fs).ok());
        let Some(s) = ssc else {
            return format!("{message}\n");
        };
        let url = s
            .source_url
            .as_ref()
            .map(|u| pretty_uri(u, self.ctx.io.as_ref()))
            .unwrap_or_else(|| "-".to_string());
        let label = if self.color {
            "\u{1b}[1mDebug\u{1b}[0m"
        } else {
            "DEBUG"
        };
        format!("{url}:{} {label}: {message}\n", s.start.line + 1)
    }
}

impl fmt::Debug for EmbeddedLogger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedLogger").finish_non_exhaustive()
    }
}

impl Logger for EmbeddedLogger {
    fn warn<'a>(&self, message: &str, span: Option<&Span<'a>>, trace: Option<&Trace>) {
        self.internal_warn(message, span, trace, None);
    }

    fn debug<'a>(&self, message: &str, span: Option<&Span<'a>>) {
        let formatted = self.format_debug(message, span);
        let event = outbound_message::LogEvent {
            r#type: LogEventType::Debug as i32,
            message: message.to_string(),
            span: protofy_span(span),
            stack_trace: String::new(),
            formatted,
            deprecation_type: None,
        };
        let msg = OutboundMessage {
            message: Some(outbound_message::Message::LogEvent(event)),
        };
        let _ = write_message_shared(&self.ctx.writer, self.ctx.compilation_id, &msg);
    }

    fn warn_deprecation<'a>(
        &self,
        message: &str,
        span: Option<&Span<'a>>,
        deprecation: &'static Deprecation,
        trace: Option<&Trace>,
    ) -> SassResult<()> {
        self.internal_warn(message, span, trace, Some(deprecation));
        Ok(())
    }
}

fn protofy_span(span: Option<&Span>) -> Option<SourceSpan> {
    span.and_then(|s| s.file_span().ok())
        .and_then(|fs| SourceSpanWithContext::from_file_span(&fs).ok())
        .map(|ssc| protofy_span_with_context(&ssc))
}

fn indent(s: &str, count: usize) -> String {
    if s.is_empty() {
        return String::new();
    }
    let prefix = " ".repeat(count);
    format!("{prefix}{}", s.replace('\n', &format!("\n{prefix}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation::test_host;
    use rust_sass::common::file_span::FileSpan;
    use rust_sass::common::source_span_file_source::FileSource;
    use rust_sass::deprecation::CALL_STRING;
    use rust_sass::Bump;

    fn logger(color: bool, ascii: bool) -> EmbeddedLogger {
        #[cfg(feature = "async")]
        let (_tx, rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        #[cfg(not(feature = "async"))]
        let (_tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let host = test_host(rx);
        EmbeddedLogger::new(host, color, ascii)
    }

    fn span<'compile>(arena: &'compile Bump) -> Span<'compile> {
        let file = FileSource::new_in(arena, "a {\n  b: c;\n}", None);
        Span::File(FileSpan::new(Some(file), 0, 9))
    }

    #[test]
    fn format_warning_no_span() {
        let l = logger(false, false);
        assert_eq!(l.format_warning("msg", None, "", None), "WARNING: msg\n");
        assert_eq!(
            l.format_warning("msg", None, "", Some(&CALL_STRING)),
            "DEPRECATION WARNING [call-string]: msg\n"
        );
    }

    #[test]
    fn format_warning_color() {
        let l = logger(true, false);
        assert_eq!(
            l.format_warning("msg", None, "", None),
            "\u{1b}[33m\u{1b}[1mWarning\u{1b}[0m: msg\n"
        );
        assert_eq!(
            l.format_warning("msg", None, "", Some(&CALL_STRING)),
            "\u{1b}[33m\u{1b}[1mDeprecation Warning\u{1b}[0m [\u{1b}[34mcall-string\u{1b}[0m]: msg\n"
        );
    }

    #[test]
    fn format_warning_user_authored_omits_id() {
        let l = logger(false, false);
        assert_eq!(
            l.format_warning("msg", None, "", Some(&USER_AUTHORED)),
            "DEPRECATION WARNING: msg\n"
        );
    }

    #[test]
    fn format_warning_with_span_and_trace() {
        let l = logger(false, false);
        let arena = Bump::new();
        let s = span(&arena);
        let formatted = l.format_warning("msg", Some(&s), "trace line", None);
        assert!(formatted.starts_with("WARNING: msg\n\n"), "{formatted:?}");
        assert!(formatted.ends_with("\n    trace line\n"), "{formatted:?}");
    }

    #[test]
    fn format_warning_no_span_with_trace() {
        // Dart (span == null): `writeln(': $message')` then
        // `writeln(indent(trace.trimRight(), 4))` — no blank line between.
        let l = logger(false, false);
        let formatted = l.format_warning("msg", None, "trace line", None);
        assert_eq!(formatted, "WARNING: msg\n    trace line\n");
    }

    #[test]
    fn format_warning_on_span() {
        let l = logger(false, false);
        let arena = Bump::new();
        let s = span(&arena);
        let formatted = l.format_warning("msg", Some(&s), "", None);
        assert!(formatted.starts_with("WARNING on "), "{formatted:?}");
        assert!(formatted.contains("line 1, column 1"), "{formatted:?}");
        assert!(formatted.ends_with('\n'), "{formatted:?}");
    }

    #[test]
    fn format_debug_no_span() {
        let l = logger(false, false);
        assert_eq!(l.format_debug("msg", None), "msg\n");
    }

    #[test]
    fn format_debug_with_span() {
        let l = logger(false, false);
        let arena = Bump::new();
        let s = span(&arena);
        let formatted = l.format_debug("msg", Some(&s));
        assert_eq!(formatted, "-:1 DEBUG: msg\n");

        let l = logger(true, false);
        assert_eq!(
            l.format_debug("msg", Some(&s)),
            "-:1 \u{1b}[1mDebug\u{1b}[0m: msg\n"
        );
    }
}
