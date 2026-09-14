// SassError -> JS data object. The shim wraps this in the `Exception` class.
//
// dart-source: lib/src/js/exception.dart (throwNodeException)

use js_sys::{Array, Object, Reflect};
use wasm_bindgen::JsValue;

use rust_sass::common::exception::{trace_for_span, SassError, Trace};
use rust_sass::common::source_span_highlighter::{HighlightColor, HighlightOptions};
use rust_sass::common::source_span_span_with_context::SourceSpanWithContext;
use rust_sass::io::Io;
use rust_sass::termglyph::GlyphSet;

/// Rendering prefs for the thrown `Exception.formatted` string. Mirrors the
/// reference `alertColor`/`alertAscii` (explicit-only: no tty detection; the
/// caller resolves `undefined` if it wants environment behavior). `ascii`
/// forces ASCII frame glyphs (`true`) vs the Unicode default (`false`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MessageHighlight {
    pub color: bool,
    pub ascii: bool,
}

/// Reads the rendering flags off a raw JS options object, for error paths
/// that run before (or without) a fully parsed `CompileOptions`. Both the
/// public `alertColor`/`alertAscii` and the rust-sass extension knobs
/// `color`/`ascii` are honored (the extension key wins when both are set —
/// see docs/ref/wasm.md, "Extension options"). These are render-time only: they color the
/// thrown `formatted` string and never feed the compile config.
pub fn highlight_from_options(options: &JsValue) -> MessageHighlight {
    MessageHighlight {
        color: read_bool(options, "color") || read_bool(options, "alertColor"),
        ascii: read_bool(options, "ascii") || read_bool(options, "alertAscii"),
    }
}

fn read_bool(options: &JsValue, key: &str) -> bool {
    match Reflect::get(options, &JsValue::from_str(key)) {
        Ok(v) => v.as_bool().unwrap_or(false),
        Err(_) => false,
    }
}

fn highlight_options(hl: &MessageHighlight) -> HighlightOptions {
    HighlightOptions {
        color: if hl.color {
            HighlightColor::Default
        } else {
            HighlightColor::None
        },
        glyphs: if hl.ascii {
            GlyphSet::Ascii
        } else {
            GlyphSet::default()
        },
        ..Default::default()
    }
}

pub fn sass_error_to_js(err: &SassError, io: &dyn Io, hl: MessageHighlight) -> JsValue {
    let obj = Object::new();
    let _ = Reflect::set(
        &obj,
        &JsValue::from_str("sassMessage"),
        &JsValue::from_str(err.message()),
    );
    let _ = Reflect::set(
        &obj,
        &JsValue::from_str("sassStack"),
        &JsValue::from_str(&stack_trace(err, io)),
    );
    if let Some(span) = err.span() {
        let _ = Reflect::set(&obj, &JsValue::from_str("span"), &span_to_js(span));
    }
    let urls = Array::new();
    for u in err.loaded_urls() {
        urls.push(&JsValue::from_str(u.as_str()));
    }
    let _ = Reflect::set(&obj, &JsValue::from_str("loadedUrls"), &urls);
    // `formatted` mirrors `throwNodeException`'s
    // `exception.toString(color: color)` (minus the shim's `Error: ` prefix
    // strip): when `alertColor`/`color` is set the frame highlight carries ANSI
    // codes, and `alertAscii`/`ascii` selects ASCII vs Unicode glyphs.
    let _ = Reflect::set(
        &obj,
        &JsValue::from_str("formatted"),
        &JsValue::from_str(&err.to_error_string_with_options(&highlight_options(&hl), io)),
    );
    // `errorCss` — the error stylesheet driving the CLI's `--error-css` dest
    // writes (docs/ref/wasm.md, "Node CLI and export surface"). Harmless extra property for the JS API; js-api-spec
    // ignores extras.
    let _ = Reflect::set(
        &obj,
        &JsValue::from_str("errorCss"),
        &JsValue::from_str(&err.to_css_string(io)),
    );
    obj.into()
}

/// Renders the Sass stack trace for the JS `sassStack` field. Matches Dart's
/// `exception.trace.toString()` (`package:stack_trace` `Trace.toString()`:
/// locations right-padded to align members, `<uri> <line>:<col>  <member>` per
/// line, each followed by a trailing newline). Reuses the core's
/// `Trace::format` (13k-spec-validated prettyUri paths + 1-based line:col) and
/// appends the trailing newline Dart's `Trace.toString()` emits.
///
/// `Runtime`/`MultiSpan` carry a real trace; `Format`/`Sass` (parse-level)
/// errors have only a span — Dart still attaches a single "root stylesheet"
/// frame for them, which the core's `format_single` also synthesizes for the
/// formatted message. Script errors without a span → empty string.
pub(crate) fn stack_trace(err: &SassError, io: &dyn Io) -> String {
    let trace: Option<Trace> = match err {
        SassError::Runtime { trace, .. } => Some(trace.clone()),
        SassError::MultiSpan { trace, .. } => Some(trace.clone()),
        SassError::Format { span, .. } | SassError::Sass { span, .. } => {
            Some(trace_for_span(span, "root stylesheet"))
        }
        _ => None,
    };
    let Some(trace) = trace else {
        return String::new();
    };
    let mut out = trace.format(io);
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

pub(crate) fn span_to_js(span: &SourceSpanWithContext) -> JsValue {
    let obj = Object::new();
    let _ = Reflect::set(
        &obj,
        &JsValue::from_str("start"),
        &location_to_js(span.start),
    );
    let _ = Reflect::set(&obj, &JsValue::from_str("end"), &location_to_js(span.end));
    let url = match &span.source_url {
        Some(u) => JsValue::from_str(u.as_str()),
        None => JsValue::NULL,
    };
    let _ = Reflect::set(&obj, &JsValue::from_str("url"), &url);
    let _ = Reflect::set(
        &obj,
        &JsValue::from_str("text"),
        &JsValue::from_str(&span.text),
    );
    let _ = Reflect::set(
        &obj,
        &JsValue::from_str("context"),
        &JsValue::from_str(&span.context),
    );
    obj.into()
}

fn location_to_js(loc: rust_sass::common::file_span::SourceLocation) -> JsValue {
    let obj = Object::new();
    let _ = Reflect::set(
        &obj,
        &JsValue::from_str("offset"),
        &JsValue::from_f64(loc.offset as f64),
    );
    let _ = Reflect::set(
        &obj,
        &JsValue::from_str("line"),
        &JsValue::from_f64(loc.line as f64),
    );
    let _ = Reflect::set(
        &obj,
        &JsValue::from_str("column"),
        &JsValue::from_f64(loc.column as f64),
    );
    obj.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_sass::common::exception::{trace_for_span, Frame, SassError, Trace};
    use rust_sass::common::file_span::SourceLocation;
    use rust_sass::common::source_span_span_with_context::SourceSpanWithContext;
    use rust_sass::io::VirtualIo;
    use rust_sass::url::SassUrl;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn get(obj: &js_sys::Object, key: &str) -> JsValue {
        Reflect::get(obj, &JsValue::from_str(key)).unwrap()
    }

    #[wasm_bindgen_test]
    fn format_error_has_root_stylesheet_stack() {
        let io = VirtualIo::new();
        let span = SourceSpanWithContext {
            source_url: None,
            start: SourceLocation {
                offset: 0,
                line: 0,
                column: 5,
            },
            end: SourceLocation {
                offset: 1,
                line: 0,
                column: 6,
            },
            text: "b:".into(),
            context: "a {b:".into(),
        };
        let err = SassError::Format {
            message: "Expected expression.".into(),
            span: span.clone(),
            original_source: None,
            cause: None,
            loaded_urls: vec![],
        };
        // A url-less format error (compileString without url) renders the
        // "root stylesheet" frame with a `-` location, matching `sass`
        // (`- 1:6  root stylesheet\n`).
        let v = sass_error_to_js(&err, &io, MessageHighlight::default());
        let obj = js_sys::Object::from(v);
        assert_eq!(
            get(&obj, "sassStack").as_string().as_deref(),
            Some("- 1:6  root stylesheet\n")
        );
        // The same synthesis as the core's format_single fallback:
        assert_eq!(
            trace_for_span(&span, "root stylesheet").format(&io),
            "- 1:6  root stylesheet"
        );
    }

    #[wasm_bindgen_test]
    fn script_error_shape() {
        let io = VirtualIo::new();
        let err = SassError::Script {
            message: "boom".into(),
            argument_name: None,
        };
        let v = sass_error_to_js(&err, &io, MessageHighlight::default());
        let obj = js_sys::Object::from(v);
        assert_eq!(
            get(&obj, "sassMessage").as_string().as_deref(),
            Some("boom")
        );
        assert_eq!(get(&obj, "sassStack").as_string().as_deref(), Some(""));
        assert!(get(&obj, "span").is_undefined());
        assert!(Array::from(&get(&obj, "loadedUrls")).length() == 0);
        assert!(get(&obj, "formatted").as_string().unwrap().contains("boom"));
    }

    #[wasm_bindgen_test]
    fn runtime_error_shape_with_stack() {
        let io = VirtualIo::new();
        let err = SassError::Runtime {
            message: "boom".into(),
            span: SourceSpanWithContext {
                source_url: SassUrl::parse("file:///a.scss").ok(),
                start: SourceLocation {
                    offset: 0,
                    line: 0,
                    column: 0,
                },
                end: SourceLocation {
                    offset: 1,
                    line: 0,
                    column: 1,
                },
                text: "a".into(),
                context: "a { b: c; }".into(),
            },
            trace: Trace::new(vec![Frame {
                uri: Some(SassUrl::parse("file:///a.scss").unwrap()),
                line: 1,
                column: 2,
                member: "foo".into(),
            }]),
            cause: None,
            loaded_urls: vec![SassUrl::parse("file:///a.scss").unwrap()],
        };
        let v = sass_error_to_js(&err, &io, MessageHighlight::default());
        let obj = js_sys::Object::from(v);
        assert_eq!(
            get(&obj, "sassStack").as_string().as_deref(),
            Some("a.scss 1:2  foo\n")
        );
        let span = get(&obj, "span");
        let start = Reflect::get(&span, &JsValue::from_str("start")).unwrap();
        assert_eq!(
            Reflect::get(&start, &JsValue::from_str("line"))
                .unwrap()
                .as_f64(),
            Some(0.0)
        );
        let urls = get(&obj, "loadedUrls");
        assert_eq!(Array::from(&urls).length(), 1);
    }

    #[wasm_bindgen_test]
    fn multi_frame_stack_aligns_members() {
        let io = VirtualIo::new();
        let err = SassError::Runtime {
            message: "boom".into(),
            span: SourceSpanWithContext {
                source_url: None,
                start: SourceLocation {
                    offset: 0,
                    line: 0,
                    column: 0,
                },
                end: SourceLocation {
                    offset: 1,
                    line: 0,
                    column: 1,
                },
                text: "a".into(),
                context: "a { b: c; }".into(),
            },
            trace: Trace::new(vec![
                Frame {
                    uri: Some(SassUrl::parse("file:///a.scss").unwrap()),
                    line: 1,
                    column: 2,
                    member: "foo".into(),
                },
                Frame {
                    uri: Some(SassUrl::parse("file:///b.scss").unwrap()),
                    line: 10,
                    column: 3,
                    member: "bar".into(),
                },
            ]),
            cause: None,
            loaded_urls: vec![],
        };
        let v = sass_error_to_js(&err, &io, MessageHighlight::default());
        let obj = js_sys::Object::from(v);
        assert_eq!(
            get(&obj, "sassStack").as_string().as_deref(),
            Some("a.scss 1:2   foo\nb.scss 10:3  bar\n")
        );
    }
}
