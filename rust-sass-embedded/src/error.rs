// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/embedded/utils.dart + lib/src/embedded/protocol_error.dart
// go-source: go/embedded/utils.go + go/embedded/protocol_error.go

use rust_sass::common::exception::{trace_for_span, SassError};
use rust_sass::common::file_span::SourceLocation;
use rust_sass::common::source_span_highlighter::{HighlightColor, HighlightOptions};
use rust_sass::common::source_span_span_with_context::SourceSpanWithContext;
use rust_sass::io::Io;
use rust_sass::termglyph::GlyphSet;

use crate::embedded_sass::outbound_message::compile_response::CompileFailure;
use crate::embedded_sass::source_span::SourceLocation as ProtoSourceLocation;
use crate::embedded_sass::{ProtocolError, ProtocolErrorType, SourceSpan};

/// The special request ID indicating an error not associated with any
/// specific inbound request.
///
/// Matches Go: `const errorId = 0xffffffff`.
pub const ERROR_ID: u32 = 0xffff_ffff;

/// The request ID used for all outbound (compiler → host) requests.
///
/// Matches Go: `outboundRequestId = 0` in isolate_dispatcher.go.
pub const OUTBOUND_REQUEST_ID: u32 = 0;

/// A `ProtocolError` indicating that a mandatory field was missing.
///
/// Matches Go: `mandatoryError`.
pub fn mandatory_error(field: &str) -> ProtocolError {
    params_error(&format!("Missing mandatory field {field}"))
}

/// A `ProtocolError` indicating that an inbound message's parameters were
/// invalid.
///
/// Matches Go: `paramsError`.
pub fn params_error(message: &str) -> ProtocolError {
    ProtocolError {
        r#type: ProtocolErrorType::Params as i32,
        id: ERROR_ID,
        message: message.into(),
    }
}

/// A `ProtocolError` with type PARSE and the given message.
///
/// Matches Go: `parseError`.
pub fn parse_error(message: &str) -> ProtocolError {
    ProtocolError {
        r#type: ProtocolErrorType::Parse as i32,
        id: 0,
        message: message.into(),
    }
}

/// A `SassError::Script` with no argument name. Used for host-caused errors
/// that surface as compile failures.
pub(crate) fn script(message: &str) -> Box<SassError> {
    Box::new(SassError::Script {
        message: message.into(),
        argument_name: None,
    })
}

/// Maps a `ProtocolErrorType` to a process exit code: 70 for internal
/// compiler errors, 76 for host-caused protocol errors.
pub(crate) fn exit_code_for(pe: &ProtocolError) -> i32 {
    if pe.r#type == ProtocolErrorType::Internal as i32 {
        70
    } else {
        76
    }
}

/// Converts a `SassError` into a `CompileFailure`.
///
/// `highlight` controls the `formatted` rendering (colors / ASCII glyphs) and
/// comes from the `CompileRequest`'s `alert_color`/`alert_ascii`.
///
/// Matches Go: `buildFailureFromError`.
pub fn build_failure(err: &SassError, io: &dyn Io, highlight: &HighlightOptions) -> CompileFailure {
    let message = err.message().to_string();
    let span = err.span().map(protofy_span_with_context);
    // Matches Dart: use the error's own trace, else derive a "root stylesheet"
    // frame from the span; Dart's field ends each frame line with a newline.
    let stack_trace = match err {
        SassError::Runtime { trace, .. } | SassError::MultiSpan { trace, .. }
            if !trace.is_empty() =>
        {
            trace.format(io)
        }
        _ => err
            .span()
            .map(|s| trace_for_span(s, "root stylesheet").format(io))
            .unwrap_or_default(),
    };
    let stack_trace = if stack_trace.is_empty() {
        stack_trace
    } else {
        format!("{stack_trace}\n")
    };
    // Richer than Go's `compileErr.Error()`; the protocol leaves `formatted`
    // unspecified, so any format is acceptable.
    let formatted = err.to_error_string_with_options(highlight, io);
    CompileFailure {
        message,
        span,
        stack_trace,
        formatted,
    }
}

/// Builds `HighlightOptions` from the protocol's `alert_color`/`alert_ascii`.
pub fn highlight_options(alert_color: bool, alert_ascii: bool) -> HighlightOptions {
    HighlightOptions {
        color: if alert_color {
            HighlightColor::Default
        } else {
            HighlightColor::None
        },
        glyphs: if alert_ascii {
            GlyphSet::Ascii
        } else {
            GlyphSet::default()
        },
        ..Default::default()
    }
}

/// Dart's `bogusSpan` (lib/src/util/span.dart): a zero-width span over an empty
/// source, used for errors Dart attaches an empty span to (e.g. invalid host
/// importer schemes: `HostImporter`'s constructor throws with `bogusSpan`).
pub fn bogus_span() -> SourceSpanWithContext {
    SourceSpanWithContext {
        source_url: None,
        start: SourceLocation {
            offset: 0,
            line: 0,
            column: 0,
        },
        end: SourceLocation {
            offset: 0,
            line: 0,
            column: 0,
        },
        text: String::new(),
        context: String::new(),
    }
}

/// Converts a `SourceSpanWithContext` into a protobuf `SourceSpan`.
///
/// Errors hold the owned `SourceSpanWithContext` (the `'parse`-borrowed
/// `Span`/`FileSpan` can't outlive the arena, so it never reaches the wire).
/// Matches Go: `protofySpan`.
pub fn protofy_span_with_context(s: &SourceSpanWithContext) -> SourceSpan {
    let url = s
        .source_url
        .as_ref()
        .map(|u| u.as_str().to_string())
        .unwrap_or_default();
    SourceSpan {
        text: s.text.clone(),
        start: Some(protofy_location(&s.start)),
        end: Some(protofy_location(&s.end)),
        url,
        context: s.context.clone(),
    }
}

fn protofy_location(loc: &rust_sass::common::file_span::SourceLocation) -> ProtoSourceLocation {
    ProtoSourceLocation {
        offset: loc.offset as u32,
        line: loc.line as u32,
        column: loc.column as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_sass::common::exception::{Frame, Trace};
    use rust_sass::common::file_span::SourceLocation;
    use rust_sass::io::DefaultIo;

    fn loc(offset: usize, line: usize, column: usize) -> SourceLocation {
        SourceLocation {
            offset,
            line,
            column,
        }
    }

    fn ssc(text: &str, context: &str) -> SourceSpanWithContext {
        SourceSpanWithContext::new(
            loc(0, 0, 0),
            loc(text.len(), 0, text.len()),
            text.to_string(),
            context.to_string(),
            None,
        )
        .unwrap()
    }

    #[test]
    fn params_error_fields() {
        let pe = params_error("nope");
        assert_eq!(pe.r#type, ProtocolErrorType::Params as i32);
        assert_eq!(pe.id, ERROR_ID);
        assert_eq!(pe.message, "nope");
    }

    #[test]
    fn parse_error_fields() {
        let pe = parse_error("nope");
        assert_eq!(pe.r#type, ProtocolErrorType::Parse as i32);
        assert_eq!(pe.id, 0);
        assert_eq!(pe.message, "nope");
    }

    #[test]
    fn mandatory_error_fields() {
        let pe = mandatory_error("input");
        assert_eq!(pe.r#type, ProtocolErrorType::Params as i32);
        assert_eq!(pe.message, "Missing mandatory field input");
    }

    #[test]
    fn protocol_error_display_is_message() {
        let pe = params_error("boom");
        assert_eq!(pe.to_string(), "boom");
    }

    #[test]
    fn protofy_span_with_context_fields() {
        let s = ssc("foo", "foo bar");
        let expected = SourceSpan {
            text: "foo".into(),
            start: Some(ProtoSourceLocation {
                offset: 0,
                line: 0,
                column: 0,
            }),
            end: Some(ProtoSourceLocation {
                offset: 3,
                line: 0,
                column: 3,
            }),
            url: String::new(),
            context: "foo bar".into(),
        };
        assert_eq!(protofy_span_with_context(&s), expected);
    }

    #[test]
    fn build_failure_runtime() {
        let span = ssc("foo", "foo bar");
        let err = SassError::Runtime {
            message: "oops".into(),
            span,
            trace: Trace::new(vec![Frame {
                uri: None,
                line: 1,
                column: 2,
                member: "top-level".into(),
            }]),
            cause: None,
            loaded_urls: vec![],
        };
        let failure = build_failure(&err, &DefaultIo::new(), &Default::default());
        assert_eq!(failure.message, "oops");
        assert_eq!(
            failure.span,
            Some(SourceSpan {
                text: "foo".into(),
                start: Some(ProtoSourceLocation {
                    offset: 0,
                    line: 0,
                    column: 0,
                }),
                end: Some(ProtoSourceLocation {
                    offset: 3,
                    line: 0,
                    column: 3,
                }),
                url: String::new(),
                context: "foo bar".into(),
            })
        );
        assert_eq!(failure.stack_trace, "- 1:2  top-level\n");
        assert_eq!(
            failure.formatted,
            "Error: oops\n  ╷\n1 │ foo bar\n  │ ^^^\n  ╵\n  - 1:2  top-level"
        );
    }

    #[test]
    fn build_failure_script() {
        let err = SassError::Script {
            message: "script error".into(),
            argument_name: None,
        };
        let failure = build_failure(&err, &DefaultIo::new(), &Default::default());
        assert_eq!(failure.message, "script error");
        assert!(failure.span.is_none());
        assert_eq!(failure.stack_trace, "");
        assert_eq!(failure.formatted, "script error");
    }
}

/// Golden-value tests: `build_failure` output must match the Dart embedded
/// compiler (1.104.0) byte-for-byte. Values captured by
/// `tests/collect_golden.rs` (`--ignored --nocapture`).
#[cfg(test)]
mod golden_tests {
    use super::*;
    use rust_sass::compile::{compile_string, CompileOptions};
    use rust_sass::io::{DefaultIo, Io};
    use rust_sass::logger::QuietLogger;
    use rust_sass::url::SassUrl;
    use rust_sass::Bump;
    use std::rc::Rc;

    struct Golden {
        source: &'static str,
        url: &'static str,
        color: bool,
        ascii: bool,
        message: &'static str,
        // (start.offset, start.line, start.column, end.offset, end.line,
        //  end.column, text, url, context)
        // Golden fixture mirroring the Dart tuple shape; a named struct would
        // be a larger refactor for one test field.
        #[allow(clippy::type_complexity)]
        span: Option<(
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            &'static str,
            &'static str,
            &'static str,
        )>,
        stack_trace: &'static str,
        formatted: &'static str,
    }

    const GOLDENS: &[Golden] = &[
        Golden {
            source: "a { color: }",
            url: "",
            color: false,
            ascii: false,
            message: "Expected expression.",
            span: Some((11, 0, 11, 11, 0, 11, "", "", "a { color: }")),
            stack_trace: "- 1:12  root stylesheet\n",
            formatted: concat!(
                "Error: Expected expression.\n",
                "  ╷\n",
                "1 │ a { color: }\n",
                "  │            ^\n",
                "  ╵\n",
                "  - 1:12  root stylesheet"
            ),
        },
        Golden {
            source: "a { color: }",
            url: "https://example.com/foo.scss",
            color: false,
            ascii: false,
            message: "Expected expression.",
            span: Some((
                11,
                0,
                11,
                11,
                0,
                11,
                "",
                "https://example.com/foo.scss",
                "a { color: }",
            )),
            stack_trace: "https://example.com/foo.scss 1:12  root stylesheet\n",
            formatted: concat!(
                "Error: Expected expression.\n",
                "  ╷\n",
                "1 │ a { color: }\n",
                "  │            ^\n",
                "  ╵\n",
                "  https://example.com/foo.scss 1:12  root stylesheet"
            ),
        },
        Golden {
            source: "a { color: }",
            url: "",
            color: true,
            ascii: false,
            message: "Expected expression.",
            span: Some((11, 0, 11, 11, 0, 11, "", "", "a { color: }")),
            stack_trace: "- 1:12  root stylesheet\n",
            formatted: concat!(
                "Error: Expected expression.\n",
                "\u{1b}[34m  ╷\u{1b}[0m\n",
                "\u{1b}[34m1 │\u{1b}[0m a { color: \u{1b}[31m\u{1b}[0m}\n",
                "\u{1b}[34m  │\u{1b}[0m \u{1b}[31m           ^\u{1b}[0m\n",
                "\u{1b}[34m  ╵\u{1b}[0m\n",
                "  - 1:12  root stylesheet"
            ),
        },
        Golden {
            source: "a { color: }",
            url: "https://example.com/foo.scss",
            color: true,
            ascii: true,
            message: "Expected expression.",
            span: Some((
                11,
                0,
                11,
                11,
                0,
                11,
                "",
                "https://example.com/foo.scss",
                "a { color: }",
            )),
            stack_trace: "https://example.com/foo.scss 1:12  root stylesheet\n",
            formatted: concat!(
                "Error: Expected expression.\n",
                "\u{1b}[34m  ,\u{1b}[0m\n",
                "\u{1b}[34m1 |\u{1b}[0m a { color: \u{1b}[31m\u{1b}[0m}\n",
                "\u{1b}[34m  |\u{1b}[0m \u{1b}[31m           ^\u{1b}[0m\n",
                "\u{1b}[34m  '\u{1b}[0m\n",
                "  https://example.com/foo.scss 1:12  root stylesheet"
            ),
        },
        Golden {
            source: "@error \"boom\"",
            url: "",
            color: false,
            ascii: false,
            message: "\"boom\"",
            span: Some((0, 0, 0, 13, 0, 13, "@error \"boom\"", "", "@error \"boom\"")),
            stack_trace: "- 1:1  root stylesheet\n",
            formatted: concat!(
                "Error: \"boom\"\n",
                "  ╷\n",
                "1 │ @error \"boom\"\n",
                "  │ ^^^^^^^^^^^^^\n",
                "  ╵\n",
                "  - 1:1  root stylesheet"
            ),
        },
        Golden {
            source: "@error \"boom\"",
            url: "https://example.com/foo.scss",
            color: true,
            ascii: false,
            message: "\"boom\"",
            span: Some((
                0,
                0,
                0,
                13,
                0,
                13,
                "@error \"boom\"",
                "https://example.com/foo.scss",
                "@error \"boom\"",
            )),
            stack_trace: "https://example.com/foo.scss 1:1  root stylesheet\n",
            formatted: concat!(
                "Error: \"boom\"\n",
                "\u{1b}[34m  ╷\u{1b}[0m\n",
                "\u{1b}[34m1 │\u{1b}[0m \u{1b}[31m@error \"boom\"\u{1b}[0m\n",
                "\u{1b}[34m  │\u{1b}[0m \u{1b}[31m^^^^^^^^^^^^^\u{1b}[0m\n",
                "\u{1b}[34m  ╵\u{1b}[0m\n",
                "  https://example.com/foo.scss 1:1  root stylesheet"
            ),
        },
        Golden {
            source: "@error \"boom\"",
            url: "",
            color: false,
            ascii: true,
            message: "\"boom\"",
            span: Some((0, 0, 0, 13, 0, 13, "@error \"boom\"", "", "@error \"boom\"")),
            stack_trace: "- 1:1  root stylesheet\n",
            formatted: concat!(
                "Error: \"boom\"\n",
                "  ,\n",
                "1 | @error \"boom\"\n",
                "  | ^^^^^^^^^^^^^\n",
                "  '\n",
                "  - 1:1  root stylesheet"
            ),
        },
        Golden {
            source: "a { b: $nope; }",
            url: "",
            color: false,
            ascii: false,
            message: "Undefined variable.",
            span: Some((7, 0, 7, 12, 0, 12, "$nope", "", "a { b: $nope; }")),
            stack_trace: "- 1:8  root stylesheet\n",
            formatted: concat!(
                "Error: Undefined variable.\n",
                "  ╷\n",
                "1 │ a { b: $nope; }\n",
                "  │        ^^^^^\n",
                "  ╵\n",
                "  - 1:8  root stylesheet"
            ),
        },
        Golden {
            source: "a { b: $nope; }",
            url: "https://example.com/foo.scss",
            color: false,
            ascii: false,
            message: "Undefined variable.",
            span: Some((
                7,
                0,
                7,
                12,
                0,
                12,
                "$nope",
                "https://example.com/foo.scss",
                "a { b: $nope; }",
            )),
            stack_trace: "https://example.com/foo.scss 1:8  root stylesheet\n",
            formatted: concat!(
                "Error: Undefined variable.\n",
                "  ╷\n",
                "1 │ a { b: $nope; }\n",
                "  │        ^^^^^\n",
                "  ╵\n",
                "  https://example.com/foo.scss 1:8  root stylesheet"
            ),
        },
        Golden {
            source: "@use \"nope\";",
            url: "",
            color: false,
            ascii: false,
            message: "Can't find stylesheet to import.",
            span: Some((0, 0, 0, 11, 0, 11, "@use \"nope\"", "", "@use \"nope\";")),
            stack_trace: "- 1:1  root stylesheet\n",
            formatted: concat!(
                "Error: Can't find stylesheet to import.\n",
                "  ╷\n",
                "1 │ @use \"nope\";\n",
                "  │ ^^^^^^^^^^^\n",
                "  ╵\n",
                "  - 1:1  root stylesheet"
            ),
        },
        Golden {
            source: "@media screen { .b { @extend .a; } }",
            url: "https://example.com/foo.scss",
            color: true,
            ascii: false,
            message: concat!(
                "The target selector was not found.\n",
                "Use \"@extend .a !optional\" to avoid this error."
            ),
            span: Some((
                21,
                0,
                21,
                31,
                0,
                31,
                "@extend .a",
                "https://example.com/foo.scss",
                "@media screen { .b { @extend .a; } }",
            )),
            stack_trace: "https://example.com/foo.scss 1:22  root stylesheet\n",
            formatted: concat!(
                "Error: The target selector was not found.\n",
                "Use \"@extend .a !optional\" to avoid this error.\n",
                "\u{1b}[34m  ╷\u{1b}[0m\n",
                "\u{1b}[34m1 │\u{1b}[0m @media screen { .b { \u{1b}[31m@extend .a\u{1b}[0m; } }\n",
                "\u{1b}[34m  │\u{1b}[0m \u{1b}[31m                     ^^^^^^^^^^\u{1b}[0m\n",
                "\u{1b}[34m  ╵\u{1b}[0m\n",
                "  https://example.com/foo.scss 1:22  root stylesheet"
            ),
        },
    ];

    #[rust_sass_macros::maybe_async]
    async fn compile_failure(source: &str, url: &str) -> Box<SassError> {
        let io: Rc<dyn Io> = Rc::new(DefaultIo::new());
        let arena = Bump::new();
        let opts = CompileOptions {
            url: if url.is_empty() {
                None
            } else {
                Some(SassUrl::parse(url).unwrap())
            },
            logger: Some(Rc::new(QuietLogger)),
            ..CompileOptions::new(&arena)
        };
        let result = compile_string(source, io.clone(), opts, &arena).await;
        match result {
            Ok(_) => panic!("expected compilation of {source:?} to fail"),
            Err(e) => e,
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn matches_dart_goldens() {
        let mut failures = Vec::new();
        for g in GOLDENS {
            let err = compile_failure(g.source, g.url).await;
            let failure = build_failure(
                &err,
                &DefaultIo::new(),
                &highlight_options(g.color, g.ascii),
            );
            if failure.message != g.message {
                failures.push(format!(
                    "message for {:?}: {:?} != {:?}",
                    g.source, failure.message, g.message
                ));
            }
            let expected_span =
                g.span
                    .map(|(so, sl, sc, eo, el, ec, text, url, context)| SourceSpan {
                        text: text.into(),
                        start: Some(ProtoSourceLocation {
                            offset: so,
                            line: sl,
                            column: sc,
                        }),
                        end: Some(ProtoSourceLocation {
                            offset: eo,
                            line: el,
                            column: ec,
                        }),
                        url: url.into(),
                        context: context.into(),
                    });
            if failure.span != expected_span {
                failures.push(format!(
                    "span for {:?}:\n  {:#?}\n  != {:#?}",
                    g.source, failure.span, expected_span
                ));
            }
            if failure.stack_trace != g.stack_trace {
                failures.push(format!(
                    "stack_trace for {:?}:\n  {:?}\n  != {:?}",
                    g.source, failure.stack_trace, g.stack_trace
                ));
            }
            if failure.formatted != g.formatted {
                failures.push(format!(
                    "formatted for {:?}:\n  {:?}\n  != {:?}",
                    g.source, failure.formatted, g.formatted
                ));
            }
        }
        if !failures.is_empty() {
            panic!(
                "{} golden mismatches:\n{}",
                failures.len(),
                failures.join("\n\n")
            );
        }
    }
}
