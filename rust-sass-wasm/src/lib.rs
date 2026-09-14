mod console;
mod error;
mod js_host;
mod js_io;
mod marshaller;
mod options;
mod result;

use std::rc::Rc;

use js_sys::Reflect;
use rust_sass::{compile, io::VirtualIo, Bump};
use wasm_bindgen::prelude::*;

/// `#[maybe_async]` runs before `#[wasm_bindgen]` (outermost first): in the
/// sync build the binding is converted to a plain fn (exported as a direct
/// JS call), in the async build it stays an async fn (exported as a Promise).
/// The mode follows the unified feature graph — same macros, same flags as
/// `rust-sass` — so this compiles correctly whether or not a sync consumer
/// forced `rust-sass/is_sync` elsewhere in the build.
/// Full entry point: `compileString(source, options) -> {css, sourceMap?,
/// loadedUrls}`. Sync in the sync build, Promise in the async build. Throws a
/// JS data object (see `error::sass_error_to_js`) on failure; the JS shim wraps
/// it in the `Exception` class. The filesystem delegate comes from
/// `options.io` (an optional JS filesystem delegate; Node injects it). When
/// absent an empty `VirtualIo` is used (browser: fs access throws, matching
/// Dart).
#[rust_sass_macros::maybe_async]
#[wasm_bindgen(js_name = compileString)]
pub async fn compile_string_js(source: &str, options: &JsValue) -> Result<JsValue, JsValue> {
    let arena = Bump::new();
    let io = make_io(options);
    let hl = error::highlight_from_options(options);
    let ctx = marshaller::MarshallingContext::new();
    let opts = match options::parse_options(&arena, options, io.clone(), ctx) {
        Ok(o) => o,
        Err(e) => return Err(error::sass_error_to_js(&e, &*io, hl)),
    };
    match compile::compile_string(source, io.clone(), opts, &arena).await {
        Ok(r) => {
            result::compile_result_to_js(&r).map_err(|e| error::sass_error_to_js(&e, &*io, hl))
        }
        Err(e) => Err(error::sass_error_to_js(&e, &*io, hl)),
    }
}

/// `compile(path, options)` — reads the file at `path` via the `options.io`
/// delegate (Node injects it), then compiles it. Same sync/async duality as
/// `compileString`.
#[rust_sass_macros::maybe_async]
#[wasm_bindgen(js_name = compile)]
pub async fn compile_js(path: &str, options: &JsValue) -> Result<JsValue, JsValue> {
    let arena = Bump::new();
    let io = make_io(options);
    let hl = error::highlight_from_options(options);
    let ctx = marshaller::MarshallingContext::new();
    let opts = match options::parse_options(&arena, options, io.clone(), ctx) {
        Ok(o) => o,
        Err(e) => return Err(error::sass_error_to_js(&e, &*io, hl)),
    };
    match compile::compile(path, io.clone(), opts, &arena).await {
        Ok(r) => {
            result::compile_result_to_js(&r).map_err(|e| error::sass_error_to_js(&e, &*io, hl))
        }
        Err(e) => Err(error::sass_error_to_js(&e, &*io, hl)),
    }
}

/// `compile_bytes(source, options)` — compiles raw source bytes. Invalid UTF-8
/// is validated here and rendered as Dart's exact `Invalid UTF-8.` error (same
/// shape as the path-based `compile`), so the JS CLI can pass file/stdin bytes
/// straight through. Returns the normal result / enriched error object — no
/// CLI-specific packaging (docs/ref/wasm.md, "Node CLI and export surface"). The logger is whatever
/// `options` built (`JsLogger` with `consoleWarn`/`consoleDebug` sinks, or
/// `QuietLogger` for `--quiet`).
#[rust_sass_macros::maybe_async]
#[wasm_bindgen(js_name = compile_bytes)]
pub async fn compile_bytes_js(source: &[u8], options: &JsValue) -> Result<JsValue, JsValue> {
    let arena = Bump::new();
    let io = make_io(options);
    let hl = error::highlight_from_options(options);
    let ctx = marshaller::MarshallingContext::new();
    let opts = match options::parse_options(&arena, options, io.clone(), ctx) {
        Ok(o) => o,
        Err(e) => return Err(error::sass_error_to_js(&e, &*io, hl)),
    };
    let source = match compile::decode_source_utf8(source, opts.url.clone(), &arena) {
        Ok(s) => s,
        Err(e) => return Err(error::sass_error_to_js(&e, &*io, hl)),
    };
    match compile::compile_string(source, io.clone(), opts, &arena).await {
        Ok(r) => {
            result::compile_result_to_js(&r).map_err(|e| error::sass_error_to_js(&e, &*io, hl))
        }
        Err(e) => Err(error::sass_error_to_js(&e, &*io, hl)),
    }
}

/// Resolves the filesystem delegate from the JS options object (`options.io`):
/// an optional JS filesystem delegate (Node injects one in the shim; a user
/// may pass a custom one for browser/http fs), else an empty `VirtualIo`.
pub(crate) fn make_io(options: &JsValue) -> Rc<dyn rust_sass::io::Io> {
    let delegate = if options.is_undefined() || options.is_null() {
        JsValue::UNDEFINED
    } else {
        Reflect::get(options, &JsValue::from_str("io")).unwrap_or(JsValue::UNDEFINED)
    };
    if !delegate.is_undefined() && !delegate.is_null() {
        Rc::new(js_io::JsIo::new(delegate))
    } else {
        Rc::new(VirtualIo::new())
    }
}

/// Primes the wall clock for wasm (std time panics on wasm32). Call once
/// before compiling, e.g. `set_time_now(Date.now())`.
#[wasm_bindgen]
pub fn set_time_now(millis: f64) {
    // The atomic clock only exists on wasm32 (native wraps SystemTime).
    #[cfg(target_arch = "wasm32")]
    rust_sass::common::time::SassTime::set_wasm_now(millis as i64);
    #[cfg(not(target_arch = "wasm32"))]
    let _millis = millis;
}

/// Sync-expansion only: every test body below drives the sync entry points
/// directly, so the module cannot compile in async mode. Async behavior is
/// covered by the lib/embedded async suites and the TS-side gates instead.
#[cfg(all(test, not(feature = "async")))]
mod tests {
    use super::*;
    use js_sys::{Object, Reflect};
    use std::cell::RefCell;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn get(obj: &js_sys::Object, key: &str) -> JsValue {
        Reflect::get(obj, &JsValue::from_str(key)).unwrap()
    }

    fn set(obj: &Object, key: &str, val: &JsValue) {
        let _ = Reflect::set(obj, &JsValue::from_str(key), val);
    }

    /// Compiles raw bytes through the new unified `compile_bytes` entry point
    /// (sync artifact), returning the result / thrown error object.
    fn compile_bytes(source: &[u8], options: &JsValue) -> Result<JsValue, JsValue> {
        compile_bytes_js(source, options)
    }

    fn opts_with_source_map() -> JsValue {
        let opts = Object::new();
        set(&opts, "sourceMap", &JsValue::from_bool(true));
        opts.into()
    }

    /// An options object carrying a `consoleWarn`/`consoleDebug` sink that
    /// appends its raw argument to [captured].
    fn sink_opts(captured: &Rc<RefCell<String>>, key: &str) -> Object {
        let c = captured.clone();
        let sink = wasm_bindgen::closure::Closure::wrap(Box::new(move |block: JsValue| {
            c.borrow_mut().push_str(&block.as_string().unwrap())
        }) as Box<dyn FnMut(JsValue)>);
        let opts = Object::new();
        set(&opts, key, sink.as_ref());
        // Keep the Closure alive for the duration of the enclosing call: it is
        // stored as a plain JS function on the options object, which Rust reads
        // by handle (no ownership). Leaking is fine in tests.
        std::mem::forget(sink);
        opts
    }

    fn ascii_opts() -> JsValue {
        let opts = Object::new();
        set(&opts, "ascii", &JsValue::from_bool(true));
        opts.into()
    }

    #[wasm_bindgen_test]
    fn warnings_route_to_console_warn_sink_byte_exact() {
        let captured: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        let opts = sink_opts(&captured, "consoleWarn");
        let v = compile_bytes(b"@warn \"hi\";\na { b: c; }\n", &opts.into()).unwrap();
        assert_eq!(
            captured.borrow().as_str(),
            "WARNING: hi\n    - 1:1  root stylesheet\n\n"
        );
        let obj = js_sys::Object::from(v);
        assert_eq!(
            get(&obj, "css").as_string().as_deref(),
            Some("a {\n  b: c;\n}")
        );
    }

    #[wasm_bindgen_test]
    fn debug_routes_to_console_debug_sink_byte_exact() {
        let captured: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        let opts = sink_opts(&captured, "consoleDebug");
        let v = compile_bytes(b"@debug 42;\na { b: c; }\n", &opts.into()).unwrap();
        assert_eq!(captured.borrow().as_str(), "-:1 DEBUG: 42\n");
        let obj = js_sys::Object::from(v);
        assert_eq!(
            get(&obj, "css").as_string().as_deref(),
            Some("a {\n  b: c;\n}")
        );
    }

    #[wasm_bindgen_test]
    fn silent_marker_suppresses_sink_calls() {
        let captured: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        let opts = sink_opts(&captured, "consoleWarn");
        let logger = Object::new();
        set(&logger, "__sassSilent", &JsValue::from_bool(true));
        set(&opts, "logger", &logger.into());
        let v = compile_bytes(b"@warn \"hi\";\na { b: c; }\n", &opts.into()).unwrap();
        assert_eq!(captured.borrow().as_str(), "");
        let obj = js_sys::Object::from(v);
        assert_eq!(
            get(&obj, "css").as_string().as_deref(),
            Some("a {\n  b: c;\n}")
        );
    }

    #[wasm_bindgen_test]
    fn syntax_error_object_shape() {
        let v = compile_bytes(b"a { b: ; }\n", &ascii_opts()).unwrap_err();
        let obj = js_sys::Object::from(v);
        assert_eq!(
            get(&obj, "formatted").as_string().as_deref(),
            Some("Error: Expected expression.\n  ,\n1 | a { b: ; }\n  |        ^\n  '\n  - 1:8  root stylesheet")
        );
        assert_eq!(
            get(&obj, "sassMessage").as_string().as_deref(),
            Some("Expected expression.")
        );
        assert_eq!(
            get(&obj, "sassStack").as_string().as_deref(),
            Some("- 1:8  root stylesheet\n")
        );
        let span = get(&obj, "span");
        let start = Reflect::get(&span, &JsValue::from_str("start")).unwrap();
        assert_eq!(
            Reflect::get(&start, &JsValue::from_str("line"))
                .unwrap()
                .as_f64(),
            Some(0.0)
        );
        let css = get(&obj, "errorCss").as_string().unwrap();
        assert!(css.contains("body::before"));
        assert!(css.contains("Error: Expected expression."));
    }

    #[wasm_bindgen_test]
    fn ascii_glyphs_respected() {
        let v = compile_bytes(b"a { b: ; }\n", &ascii_opts()).unwrap_err();
        let formatted = js_sys::Object::from(v);
        let s = get(&formatted, "formatted").as_string().unwrap();
        assert!(s.contains("'\n"));
        assert!(!s.contains('╷'));

        let v = compile_bytes(b"a { b: ; }\n", &JsValue::UNDEFINED).unwrap_err();
        let formatted = js_sys::Object::from(v);
        let s = get(&formatted, "formatted").as_string().unwrap();
        assert!(s.contains('╵'));
        assert!(s.contains('│'));
    }

    #[wasm_bindgen_test]
    fn success_with_source_map() {
        let v = compile_bytes(b"a { b: c; }\n", &opts_with_source_map()).unwrap();
        let obj = js_sys::Object::from(v);
        let sm = get(&obj, "sourceMap");
        assert_eq!(
            Reflect::get(&sm, &JsValue::from_str("version"))
                .unwrap()
                .as_f64(),
            Some(3.0)
        );
    }

    #[wasm_bindgen_test]
    fn no_source_map_by_default() {
        let v = compile_bytes(b"a { b: c; }\n", &JsValue::UNDEFINED).unwrap();
        let obj = js_sys::Object::from(v);
        assert!(get(&obj, "sourceMap").is_undefined());
    }

    #[wasm_bindgen_test]
    fn invalid_utf8_error_byte_exact() {
        // Mirrors spec/libsass-closed-issues/issue_2446: "$" then an invalid
        // byte. Dart renders `Invalid UTF-8.` with the caret under the first
        // invalid byte and the lossy line (U+FFFD). `ascii: true` reproduces
        // the old CLI's `--no-unicode` ASCII framing.
        let src = b"$\xff:D&(22#222222%0:/2222-2%22%222/2-2%22%2222-2%22%22)/22";
        let opts = Object::new();
        set(&opts, "url", &JsValue::from_str("file:///input.scss"));
        set(&opts, "ascii", &JsValue::from_bool(true));
        let v = compile_bytes(src, &opts.into()).unwrap_err();
        let obj = js_sys::Object::from(v);
        assert_eq!(
            get(&obj, "sassMessage").as_string().as_deref(),
            Some("Invalid UTF-8.")
        );
        let formatted = get(&obj, "formatted").as_string().unwrap();
        assert!(
            formatted.starts_with("Error: Invalid UTF-8.\n  ,\n1 | $\u{FFFD}:D&(22#222222%0:"),
            "formatted: {}",
            formatted
        );
        assert!(formatted.contains("\n  |  ^\n"), "formatted: {}", formatted);
        assert!(
            formatted.ends_with("  input.scss 1:2  root stylesheet"),
            "formatted: {}",
            formatted
        );
        let css = get(&obj, "errorCss").as_string().unwrap();
        assert!(css.contains("Invalid UTF-8."));
    }

    #[wasm_bindgen_test]
    fn valid_utf8_unaffected() {
        let v = compile_bytes(b"a { b: c; }", &JsValue::UNDEFINED).unwrap();
        let obj = js_sys::Object::from(v);
        assert_eq!(
            get(&obj, "css").as_string().as_deref(),
            Some("a {\n  b: c;\n}")
        );
    }

    #[wasm_bindgen_test]
    fn selector_error_uses_ascii_glyphs_with_ascii() {
        // `assert_selector` bakes the parse error into the Script error message
        // using config.unicode. `ascii: true` (→ `opts.unicode = false`) must
        // yield ASCII glyphs throughout `formatted`.
        let src = "@use \"sass:selector\";\na {b: selector.append(\".c\", \"&\")}";
        let v = compile_bytes(src.as_bytes(), &ascii_opts()).unwrap_err();
        let obj = js_sys::Object::from(v);
        let formatted = get(&obj, "formatted").as_string().unwrap();
        assert!(
            formatted.contains("  ,\n1 | &\n  | ^\n  '"),
            "formatted: {}",
            formatted
        );
        assert!(
            !formatted.contains('╷'),
            "formatted has unicode glyphs: {}",
            formatted
        );
    }

    #[wasm_bindgen_test]
    fn selector_error_uses_unicode_glyphs_by_default() {
        let src = "@use \"sass:selector\";\na {b: selector.append(\".c\", \"&\")}";
        let v = compile_bytes(src.as_bytes(), &JsValue::UNDEFINED).unwrap_err();
        let obj = js_sys::Object::from(v);
        let formatted = get(&obj, "formatted").as_string().unwrap();
        assert!(
            formatted.contains("  ╷\n1 │ &\n  │ ^\n  ╵"),
            "formatted: {}",
            formatted
        );
    }

    #[wasm_bindgen_test]
    fn extend_media_query_error_uses_ascii_glyphs() {
        // `extend::span_message` bakes the "From ... of <url>:" prefix using the
        // ExtendState's unicode flag. Needs a url on the entry so the span
        // renders the "From ... of input.scss:" prefix.
        let src = ".example {\n  padding-left: 2rem;\n  padding-right: 2rem;\n}\n\
                   @media screen and (min-width:768px) {\n  #footer {\n    .row {\n      @extend .example;\n    }\n  }\n}\n";
        let opts = Object::new();
        set(&opts, "url", &JsValue::from_str("file:///input.scss"));
        set(&opts, "ascii", &JsValue::from_bool(true));
        let v = compile_bytes(src.as_bytes(), &opts.into()).unwrap_err();
        let obj = js_sys::Object::from(v);
        let formatted = get(&obj, "formatted").as_string().unwrap();
        assert!(
            formatted.contains(
                "From line 1, column 1 of input.scss: \n  ,\n1 | .example {\n  | ^^^^^^^^^\n  '"
            ),
            "formatted: {}",
            formatted
        );
        assert!(
            !formatted.contains('╷'),
            "formatted has unicode glyphs: {}",
            formatted
        );
    }

    #[wasm_bindgen_test]
    fn error_object_carries_error_css() {
        let v = compile_bytes(b"a { b: ; }\n", &ascii_opts()).unwrap_err();
        let obj = js_sys::Object::from(v);
        let css = get(&obj, "errorCss").as_string().unwrap();
        assert!(css.contains("body::before"));
    }

    #[wasm_bindgen_test]
    fn runtime_error_object_shape() {
        // A runtime (eval) error carries sassMessage, a real span, and a
        // rendered formatted string mentioning the loaded entry url.
        let src = "@use \"sass:meta\"; @include meta.apply(meta.get-function(\"nope\"));";
        let opts = Object::new();
        set(&opts, "url", &JsValue::from_str("file:///style.scss"));
        let v = compile_bytes(src.as_bytes(), &opts.into()).unwrap_err();
        let obj = js_sys::Object::from(v);
        assert_eq!(
            get(&obj, "sassMessage").as_string().as_deref(),
            Some("Function not found: \"nope\"")
        );
        let formatted = get(&obj, "formatted").as_string().unwrap();
        assert!(
            formatted.contains("Function not found"),
            "formatted: {}",
            formatted
        );
        let stack = get(&obj, "sassStack").as_string().unwrap();
        assert!(stack.ends_with("root stylesheet\n"), "stack: {}", stack);
        let span = get(&obj, "span");
        let start = Reflect::get(&span, &JsValue::from_str("start")).unwrap();
        assert!(Reflect::get(&start, &JsValue::from_str("offset"))
            .unwrap()
            .as_f64()
            .is_some());
        let css = get(&obj, "errorCss").as_string().unwrap();
        assert!(css.contains("body::before"));
    }
}
