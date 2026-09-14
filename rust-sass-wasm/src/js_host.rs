// JS host glue: importer, logger, and custom-function callbacks that bridge
// from rust-sass into JS via wasm-bindgen.
//
// The dual sync/async build follows the rust-sass-embedded pattern: trait
// impls are written once under `#[maybe_async]` (methods return
// `LocalBoxFuture`, `Box::pin(async move {...})` bodies; the sync build strips
// the futures); callback builders and methods whose bodies genuinely differ
// (sync must THROW on Promise returns, async must AWAIT them) are written as
// two `#[cfg]`-gated variants so only one is typechecked per build.
//
// dart-source: lib/src/importer/js_to_dart/*.dart + lib/src/js/compile.dart (functions)
// go-source: go/embedded/importer_host.go + go/embedded/function.go (via rust-sass-embedded)

use crate::error::span_to_js;
use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;

use js_sys::{Array, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue};

use rust_sass::callable::{BuiltInCallable, Callable, CallableKind};
use rust_sass::common::exception::{SassError, SassResult, Trace};
use rust_sass::eval::importer::filesystem::FilesystemImporter;
use rust_sass::eval::importer::result::ImporterResult;
use rust_sass::eval::importer::{CanonicalizeContext, UserImporter};
use rust_sass::logger::{Logger, NoOpWarnLogger};
use rust_sass::parse::stylesheet::{CssState, SassIndentState, Syntax};
use rust_sass::url::SassUrl;
use rust_sass::value::Value;
use rust_sass::Bump;

#[cfg(feature = "async")]
use futures::future::LocalBoxFuture;
#[cfg(feature = "async")]
use rust_sass::callable::AsyncBuiltInCallback;
#[cfg(not(feature = "async"))]
use rust_sass::callable::SyncBuiltInCallback;
#[cfg(feature = "async")]
use wasm_bindgen_futures::JsFuture;

use crate::marshaller::{
    script, value_from_js_with_accessed, value_to_js, HostFunctionBuilder, MarshallingContext,
};

pub(crate) fn js_error_to_script(e: JsValue) -> Box<SassError> {
    let message = if let Some(s) = e.as_string() {
        s
    } else if let Ok(m) = Reflect::get(&e, &JsValue::from_str("message")) {
        m.as_string().unwrap_or_else(|| format!("{e:?}"))
    } else {
        format!("{e:?}")
    };
    script(message)
}

pub(crate) fn is_promise(v: &JsValue) -> bool {
    v.is_instance_of::<js_sys::Promise>()
}

/// Whether [v] is an instance of the global `URL` constructor, matching
/// Dart's `isJSUrl` (lib/src/js/utils.dart). Evaluates
/// `URL[Symbol.hasInstance](v)` — i.e. `v instanceof URL`.
fn is_js_url(v: &JsValue) -> bool {
    if v.is_null() || v.is_undefined() {
        return false;
    }
    let url_ctor = match Reflect::get(&js_sys::global(), &JsValue::from_str("URL")) {
        Ok(u) if u.is_function() => u,
        _ => return false,
    };
    let has_instance = match Reflect::get(&url_ctor, &js_sys::Symbol::has_instance()) {
        Ok(h) if h.is_function() => h,
        _ => return false,
    };
    let f: js_sys::Function = match has_instance.dyn_into() {
        Ok(f) => f,
        Err(_) => return false,
    };
    f.call1(&url_ctor, v)
        .map(|r| r.is_truthy())
        .unwrap_or(false)
}

/// The string form of a JS URL (its `href`), or a plain string, as needed by
/// `sourceMapUrl`. Returns None for anything else.
fn js_url_to_string(v: &JsValue) -> Option<String> {
    if let Some(s) = v.as_string() {
        return Some(s);
    }
    if is_js_url(v) {
        if let Ok(href) = Reflect::get(v, &JsValue::from_str("href")) {
            if let Some(s) = href.as_string() {
                return Some(s);
            }
        }
    }
    None
}

/// Roughly matches Dart's `jsType` (lib/src/js/utils.dart): `typeof` for
/// primitives, `constructor.name` for objects, else `"object"`.
fn js_type_name(v: &JsValue) -> String {
    if v.is_undefined() {
        return "undefined".into();
    }
    if v.is_null() {
        return "null".into();
    }
    if v.as_string().is_some() {
        return "string".into();
    }
    if v.as_f64().is_some() {
        return "number".into();
    }
    if v.as_bool().is_some() {
        return "boolean".into();
    }
    if v.is_function() {
        return "function".into();
    }
    if let Ok(ctor) = Reflect::get(v, &JsValue::from_str("constructor")) {
        if let Ok(name) = Reflect::get(&ctor, &JsValue::from_str("name")) {
            if let Some(s) = name.as_string() {
                return s;
            }
        }
    }
    "object".into()
}

/// Parses a syntax string the way Dart's `parseSyntax` does
/// (lib/src/js/utils.dart:267).
fn syntax_from_str(s: &str) -> SassResult<Syntax> {
    match s {
        "scss" => Ok(Syntax::Scss),
        "indented" => Ok(Syntax::Sass(SassIndentState {
            current_indentation: 0,
            next_indentation: None,
            next_indentation_end: None,
            indent_spaces: None,
        })),
        "css" => Ok(Syntax::Css(CssState {
            disallowed_function_names: HashSet::new(),
        })),
        other => Err(script(format!("Unknown syntax \"{other}\"."))),
    }
}

/// Stringifies an arbitrary JS value for an error message — mirrors the JS
/// `String(v)` constructor (§12 #18): strings pass through, URLs become their
/// `[object URL]`-style repr, other objects `[object Object]`, etc.
fn js_str_repr(v: &JsValue) -> String {
    if let Some(s) = v.as_string() {
        return s;
    }
    if let Ok(string_ctor) = Reflect::get(&js_sys::global(), &JsValue::from_str("String")) {
        if let Ok(f) = string_ctor.dyn_into::<js_sys::Function>() {
            if let Ok(r) = f.call1(&JsValue::UNDEFINED, v) {
                if let Some(s) = r.as_string() {
                    return s;
                }
            }
        }
    }
    format!("{v:?}")
}

fn call_js(f: &js_sys::Function, arg: &JsValue) -> SassResult<JsValue> {
    f.call1(&JsValue::UNDEFINED, arg)
        .map_err(js_error_to_script)
}

/// Calls `f` with the elements of `args` as separate arguments (the JS
/// importer signature is `(url, context)`, not a single args array).
fn call_js_spread(f: &js_sys::Function, args: &Array) -> SassResult<JsValue> {
    f.apply(&JsValue::UNDEFINED, args)
        .map_err(js_error_to_script)
}

// ==== Custom importers =======================================================

/// A JS URL importer (`{canonicalize, load}`).
pub struct JsImporter {
    canonicalize: js_sys::Function,
    load: js_sys::Function,
    non_canonical_schemes: Vec<String>,
}

impl fmt::Debug for JsImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsImporter").finish_non_exhaustive()
    }
}

impl JsImporter {
    pub fn new(
        canonicalize: js_sys::Function,
        load: js_sys::Function,
        non_canonical_schemes: Vec<String>,
    ) -> Self {
        JsImporter {
            canonicalize,
            load,
            non_canonical_schemes,
        }
    }
}

fn canonicalize_args(url: &SassUrl, context: &mut CanonicalizeContext) -> Array {
    let args = Array::new();
    // Use the Display form (strips the internal `sass-relative:` wrapper for
    // relative URLs) so the JS importer sees the URL as written, matching Dart.
    args.push(&JsValue::from_str(&url.to_string()));
    let ctx_obj = Object::new();
    let _ = Reflect::set(
        &ctx_obj,
        &JsValue::from_str("fromImport"),
        &JsValue::from_bool(context.from_import),
    );
    // Only mark the containing URL as accessed when one is actually present
    // (and thus passed to JS): a JS importer may read it, so the result is
    // context-sensitive and must not be globally cached (sass/dart-sass#2208).
    if context.containing_url_without_marking().is_some() {
        if let Some(containing) = context.containing_url() {
            let _ = Reflect::set(
                &ctx_obj,
                &JsValue::from_str("containingUrl"),
                // Use the Display form (§15.4 #1); the wire stays a string and the
                // shim converts it to a URL instance for the user-visible context.
                &JsValue::from_str(&containing.to_string()),
            );
        }
    }
    args.push(&ctx_obj.into());
    args
}

/// Validates the result of a JS `canonicalize` call. Matches Dart
/// (`lib/src/importer/js_to_dart/sync.dart:39-56`): null → fallback; a JS
/// `URL` → its string form; anything else (after the Promise check in the
/// caller) → error.
fn parse_canonicalize_result(result: &JsValue) -> SassResult<Option<SassUrl>> {
    if result.is_null() || result.is_undefined() {
        return Ok(None);
    }
    if is_js_url(result) {
        let s = js_url_to_string(result).unwrap_or_default();
        return SassUrl::parse(&s)
            .map(Some)
            .map_err(|_| script(format!("The importer returned an invalid URL \"{s}\".")));
    }
    Err(script("The canonicalize() method must return a URL."))
}

/// Validates the result of a JS `load` call. Matches Dart
/// (`lib/src/importer/js_to_dart/sync.dart:58-98`): null → fallback; the
/// `contents` field must be a string (else ArgumentError), and `syntax` must
/// be present (else "must return an object with contents and syntax fields.").
fn parse_load_result(result: &JsValue) -> SassResult<Option<ImporterResult>> {
    if result.is_null() || result.is_undefined() {
        return Ok(None);
    }
    let contents_val =
        Reflect::get(result, &JsValue::from_str("contents")).unwrap_or(JsValue::UNDEFINED);
    let contents = match contents_val.as_string() {
        Some(s) => s,
        None => {
            return Err(script(format!(
                "Invalid argument (contents): must be a string but was: {}",
                js_type_name(&contents_val)
            )));
        }
    };
    let syntax = match get_js_str_opt(result, "syntax")? {
        Some(s) => syntax_from_str(&s)?,
        None => {
            return Err(script(
                "The load() function must return an object with contents and syntax fields.",
            ));
        }
    };
    // `sourceMapUrl` must be an absolute URL (Dart: `jsToDartUrl` =
    // `Uri.parse`, then `ImporterResult` throws `ArgumentError.value(url,
    // 'sourceMapUrl', 'must be absolute')` when the scheme is empty — an
    // object/string without a scheme, e.g. `{}` or `"foo"`, produces the
    // "must be absolute" error, not a type error).
    let source_map_url = match Reflect::get(result, &JsValue::from_str("sourceMapUrl")) {
        Ok(v) if !(v.is_undefined() || v.is_null()) => {
            let s = js_str_repr(&v);
            match SassUrl::parse(&s) {
                Ok(u) if !u.scheme().is_empty() => Some(u),
                _ => {
                    return Err(script(format!(
                        "Invalid argument (sourceMapUrl): must be absolute, was {s}"
                    )));
                }
            }
        }
        _ => None,
    };
    Ok(Some(ImporterResult::new(contents, syntax, source_map_url)?))
}
#[rust_sass_macros::maybe_async]
impl UserImporter for JsImporter {
    #[cfg(feature = "async")]
    fn canonicalize<'a>(
        &'a self,
        url: &'a SassUrl,
        context: &'a mut CanonicalizeContext,
    ) -> LocalBoxFuture<'a, SassResult<Option<SassUrl>>> {
        let f = self.canonicalize.clone();
        let args = canonicalize_args(url, context);
        Box::pin(async move {
            let result = call_js_spread(&f, &args)?;
            let result = if is_promise(&result) {
                let p = result
                    .dyn_into::<js_sys::Promise>()
                    .map_err(|_| script("expected a Promise"))?;
                JsFuture::from(p).await.map_err(js_error_to_script)?
            } else {
                result
            };
            parse_canonicalize_result(&result)
        })
    }

    #[cfg(not(feature = "async"))]
    fn canonicalize<'a>(
        &'a self,
        url: &'a SassUrl,
        context: &'a mut CanonicalizeContext,
    ) -> LocalBoxFuture<'a, SassResult<Option<SassUrl>>> {
        let f = self.canonicalize.clone();
        let args = canonicalize_args(url, context);
        Box::pin(async move {
            let result = call_js_spread(&f, &args)?;
            if is_promise(&result) {
                return Err(script("The canonicalize() function can't return a Promise for synchronous compile functions."));
            }
            parse_canonicalize_result(&result)
        })
    }

    #[cfg(feature = "async")]
    fn load<'a>(
        &'a self,
        url: &'a SassUrl,
    ) -> LocalBoxFuture<'a, SassResult<Option<ImporterResult>>> {
        let f = self.load.clone();
        let arg = JsValue::from_str(&url.to_string());
        Box::pin(async move {
            let result = call_js(&f, &arg)?;
            let result = if is_promise(&result) {
                let p = result
                    .dyn_into::<js_sys::Promise>()
                    .map_err(|_| script("expected a Promise"))?;
                JsFuture::from(p).await.map_err(js_error_to_script)?
            } else {
                result
            };
            parse_load_result(&result)
        })
    }

    #[cfg(not(feature = "async"))]
    fn load<'a>(
        &'a self,
        url: &'a SassUrl,
    ) -> LocalBoxFuture<'a, SassResult<Option<ImporterResult>>> {
        let f = self.load.clone();
        let arg = JsValue::from_str(&url.to_string());
        Box::pin(async move {
            let result = call_js(&f, &arg)?;
            if is_promise(&result) {
                return Err(script(
                    "The load() function can't return a Promise for synchronous compile functions.",
                ));
            }
            parse_load_result(&result)
        })
    }

    fn is_non_canonical_scheme(&self, scheme: &str) -> bool {
        self.non_canonical_schemes.iter().any(|s| s == scheme)
    }
}

/// A JS file importer (`{findFileUrl}`). Mirrors Dart's `JSToDartFileImporter`
/// (lib/src/importer/js_to_dart/file.dart) and the embedded
/// `importer_file.rs`: `file:` URLs are handled by a local
/// `FilesystemImporter`; other schemes go to JS `findFileUrl` and must come
/// back as `file:` URLs.
pub struct JsFileImporter {
    find_file_url: js_sys::Function,
    fs: FilesystemImporter,
}

impl fmt::Debug for JsFileImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsFileImporter").finish_non_exhaustive()
    }
}

impl JsFileImporter {
    pub fn new(find_file_url: js_sys::Function, io: Rc<dyn rust_sass::io::Io>) -> Self {
        // `new_no_load_path`, not `new_cwd` (which sets a deprecated load path
        // that emits a spurious FS_IMPORTER_CWD deprecation) — same call as
        // the embedded crate's FileImporter.
        JsFileImporter {
            find_file_url,
            fs: FilesystemImporter::new_no_load_path(io),
        }
    }
}

#[rust_sass_macros::maybe_async]
impl UserImporter for JsFileImporter {
    #[cfg(feature = "async")]
    fn canonicalize<'a>(
        &'a self,
        url: &'a SassUrl,
        context: &'a mut CanonicalizeContext,
    ) -> LocalBoxFuture<'a, SassResult<Option<SassUrl>>> {
        if url.is_file() {
            let fs = &self.fs;
            return Box::pin(async move { fs.canonicalize(url, context, &NoOpWarnLogger).await });
        }
        let f = self.find_file_url.clone();
        let fs = self.fs.clone();
        let args = canonicalize_args(url, context);
        Box::pin(async move {
            let result = call_js_spread(&f, &args)?;
            let result = if is_promise(&result) {
                let p = result
                    .dyn_into::<js_sys::Promise>()
                    .map_err(|_| script("expected a Promise"))?;
                JsFuture::from(p).await.map_err(js_error_to_script)?
            } else {
                result
            };
            parse_file_url_result(&result, url, &fs, context).await
        })
    }

    #[cfg(not(feature = "async"))]
    fn canonicalize<'a>(
        &'a self,
        url: &'a SassUrl,
        context: &'a mut CanonicalizeContext,
    ) -> LocalBoxFuture<'a, SassResult<Option<SassUrl>>> {
        if url.is_file() {
            let fs = &self.fs;
            return Box::pin(async move { fs.canonicalize(url, context, &NoOpWarnLogger).await });
        }
        let f = self.find_file_url.clone();
        let fs = self.fs.clone();
        let args = canonicalize_args(url, context);
        Box::pin(async move {
            let result = call_js_spread(&f, &args)?;
            if is_promise(&result) {
                return Err(script("The findFileUrl() function can't return a Promise for synchron compile functions."));
            }
            parse_file_url_result(&result, url, &fs, context).await
        })
    }

    #[rust_sass_macros::maybe_async]
    fn load<'a>(
        &'a self,
        url: &'a SassUrl,
    ) -> LocalBoxFuture<'a, SassResult<Option<ImporterResult>>> {
        let fs = &self.fs;
        Box::pin(async move { fs.load(url).await })
    }

    fn is_non_canonical_scheme(&self, scheme: &str) -> bool {
        scheme != "file"
    }
}

/// Validates the result of a JS `findFileUrl` call (Dart file.dart:25-51):
/// null → fallback; a `file:` URL → canonicalized via [fs]; anything else →
/// Dart-exact error.
#[rust_sass_macros::maybe_async]
async fn parse_file_url_result(
    result: &JsValue,
    original_url: &SassUrl,
    fs: &FilesystemImporter,
    context: &mut CanonicalizeContext,
) -> SassResult<Option<SassUrl>> {
    if result.is_null() || result.is_undefined() {
        return Ok(None);
    }
    if !is_js_url(result) {
        return Err(script("The findFileUrl() method must return a URL."));
    }
    let s = js_url_to_string(result).unwrap_or_default();
    let file_url =
        SassUrl::parse(&s).map_err(|_| script("The findFileUrl() method must return a URL."))?;
    if file_url.scheme() != "file" {
        return Err(script(format!(
            "The findFileUrl() must return a URL with scheme file://, was \"{original_url}\"."
        )));
    }
    fs.canonicalize(&file_url, context, &NoOpWarnLogger).await
}

// ==== Logger =================================================================

/// A JS logger (`{warn, debug}`) mirroring Dart's `JSToDartLogger`
/// (lib/src/logger/js_to_dart.dart): when the JS side defines the method it is
/// called with the plain message; when it is ABSENT the full terminal block is
/// rendered (via `stderr::render_warning`/`render_debug`, honoring
/// `color`/`alertColor` and `ascii`/`alertAscii`) and forwarded to the
/// `consoleWarn`/`consoleDebug` sink functions (docs/ref/wasm.md, "Extension options"),
/// falling back to `console.warn`/`console.error`. `unicode` selects the glyph
/// set (`!ascii`).
pub struct JsLogger {
    warn: Option<js_sys::Function>,
    debug: Option<js_sys::Function>,
    console_warn: Option<js_sys::Function>,
    console_debug: Option<js_sys::Function>,
    color: bool,
    unicode: bool,
    io: Rc<dyn rust_sass::io::Io>,
}

impl fmt::Debug for JsLogger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsLogger")
            .field("color", &self.color)
            .field("unicode", &self.unicode)
            .finish_non_exhaustive()
    }
}

impl JsLogger {
    pub fn new(
        warn: Option<js_sys::Function>,
        debug: Option<js_sys::Function>,
        console_warn: Option<js_sys::Function>,
        console_debug: Option<js_sys::Function>,
        color: bool,
        unicode: bool,
        io: Rc<dyn rust_sass::io::Io>,
    ) -> Self {
        JsLogger {
            warn,
            debug,
            console_warn,
            console_debug,
            color,
            unicode,
            io,
        }
    }

    /// Routes a fully-rendered block to the `consoleWarn` sink when present,
    /// else the `console.warn` fallback (stderr in Node).
    fn emit_warning(&self, block: &str) {
        if let Some(sink) = &self.console_warn {
            let _ = sink.call1(&JsValue::UNDEFINED, &JsValue::from_str(block));
        } else {
            web_sys::console::warn_1(&JsValue::from_str(block));
        }
    }

    /// Routes a fully-rendered debug block to the `consoleDebug` sink when
    /// present, else the `console.error` fallback (`console.debug` writes to
    /// stdout in Node; the default `@debug` output must go to stderr).
    fn emit_debug(&self, block: &str) {
        if let Some(sink) = &self.console_debug {
            let _ = sink.call1(&JsValue::UNDEFINED, &JsValue::from_str(block));
        } else {
            web_sys::console::error_1(&JsValue::from_str(block));
        }
    }

    /// Renders (or falls back to the plain message on a rendering failure) the
    /// full warning/deprecation block for the console fallback path.
    fn render_console_warning(
        &self,
        deprecation: Option<&rust_sass::deprecation::Deprecation>,
        message: &str,
        span: Option<&rust_sass::common::span::Span<'_>>,
        stack: &str,
    ) -> String {
        match rust_sass::logger::stderr::render_warning(
            self.color,
            self.unicode,
            self.io.as_ref(),
            deprecation,
            message,
            span,
            stack,
        ) {
            Ok(block) => block,
            // A highlight can fail when the span lacks source text; fall back
            // to the raw message rather than dropping the warning.
            Err(_) => message.to_string(),
        }
    }
}

/// Marshals an optional logger `Span` into the JS `SourceSpan` shape
/// (`sass/js-api-doc/logger/source_span.d.ts`), mirroring `error::span_to_js`
/// (the JS API SourceSpan = Dart FileSpan: start/end locations, url, text,
/// context). Returns `undefined` when there is no span (e.g. `@warn`, which
/// Dart calls with `span: null` — js-api-spec logger.test.ts asserts
/// `span` toBeUndefined for `@warn`).
fn logger_span_to_js(span: Option<&rust_sass::common::span::Span<'_>>) -> JsValue {
    let Some(span) = span else {
        return JsValue::UNDEFINED;
    };
    match rust_sass::common::source_span_span_with_context::SourceSpanWithContext::from_span(span) {
        Ok(ctx) => span_to_js(&ctx),
        Err(_) => JsValue::UNDEFINED,
    }
}

impl Logger for JsLogger {
    fn warn<'a>(
        &self,
        message: &str,
        span: Option<&rust_sass::common::span::Span<'a>>,
        trace: Option<&Trace>,
    ) {
        let stack = trace
            .map(|t| t.format(self.io.as_ref()))
            .unwrap_or_default();
        if let Some(warn) = &self.warn {
            let opts = Object::new();
            let _ = Reflect::set(
                &opts,
                &JsValue::from_str("deprecation"),
                &JsValue::from_bool(false),
            );
            let span = logger_span_to_js(span);
            if !span.is_undefined() {
                let _ = Reflect::set(&opts, &JsValue::from_str("span"), &span);
            }
            let _ = Reflect::set(
                &opts,
                &JsValue::from_str("stack"),
                &JsValue::from_str(&stack),
            );
            let _ = warn.call2(
                &JsValue::UNDEFINED,
                &JsValue::from_str(message),
                &opts.into(),
            );
        } else {
            // No JS `warn` — render the full warning block (Dart's
            // `Logger.stderr(color)` fallback) and forward it to the
            // `consoleWarn` sink (or `console.warn`, stderr in Node). Colors/
            // glyphs follow `color`/`alertColor` and `ascii`/`alertAscii`.
            let block = self.render_console_warning(None, message, span, &stack);
            self.emit_warning(&block);
        }
    }

    fn debug<'a>(&self, message: &str, span: Option<&rust_sass::common::span::Span<'a>>) {
        if let Some(debug) = &self.debug {
            let opts = Object::new();
            let span = logger_span_to_js(span);
            if !span.is_undefined() {
                let _ = Reflect::set(&opts, &JsValue::from_str("span"), &span);
            }
            let _ = debug.call2(
                &JsValue::UNDEFINED,
                &JsValue::from_str(message),
                &opts.into(),
            );
        } else {
            // No JS `debug` — forward the rendered block to the `consoleDebug`
            // sink (or `console.error`, stderr in Node — `console.debug` would
            // write to stdout).
            let block = rust_sass::logger::stderr::render_debug(
                self.color,
                self.io.as_ref(),
                message,
                span,
            );
            self.emit_debug(&block);
        }
    }

    fn warn_deprecation<'a>(
        &self,
        message: &str,
        span: Option<&rust_sass::common::span::Span<'a>>,
        deprecation: &'static rust_sass::deprecation::Deprecation,
        trace: Option<&Trace>,
    ) -> SassResult<()> {
        let stack = trace
            .map(|t| t.format(self.io.as_ref()))
            .unwrap_or_default();
        if let Some(warn) = &self.warn {
            let opts = Object::new();
            let _ = Reflect::set(
                &opts,
                &JsValue::from_str("deprecation"),
                &JsValue::from_bool(true),
            );
            let _ = Reflect::set(
                &opts,
                &JsValue::from_str("deprecationType"),
                &JsValue::from_str(deprecation.id),
            );
            let span = logger_span_to_js(span);
            if !span.is_undefined() {
                let _ = Reflect::set(&opts, &JsValue::from_str("span"), &span);
            }
            let _ = Reflect::set(
                &opts,
                &JsValue::from_str("stack"),
                &JsValue::from_str(&stack),
            );
            let _ = warn.call2(
                &JsValue::UNDEFINED,
                &JsValue::from_str(message),
                &opts.into(),
            );
        } else {
            // No JS `warn` — forward the rendered deprecation block to the
            // `consoleWarn` sink (or `console.warn`, stderr in Node), matching
            // the default Dart logger.
            let block = self.render_console_warning(Some(deprecation), message, span, &stack);
            self.emit_warning(&block);
        }
        Ok(())
    }
}

// ==== Custom functions =======================================================

fn split_signature(signature: &str) -> SassResult<(String, String)> {
    let open = signature.find('(').ok_or_else(|| {
        script(format!(
            "options.functions: \"{signature}\" is missing \"(\""
        ))
    })?;
    if !signature.ends_with(')') {
        return Err(script(format!(
            "Invalid signature \"{signature}\": expected \")\"."
        )));
    }
    let name = &signature[..open];
    // Dart's `ScssParser.parseSignature` is strict: `identifier()` then
    // `_parameterList()`, with no whitespace allowed before the identifier or
    // between it and the `(` (`' foo()'` and `'foo ()'` both throw).
    if name.trim() != name {
        return Err(script(format!(
            "Invalid signature \"{signature}\": Expected identifier."
        )));
    }
    let params = &signature[open + 1..signature.len() - 1];
    Ok((name.to_string(), params.to_string()))
}

fn args_to_js<'parse>(
    ctx: &MarshallingContext<'parse>,
    args: &[Value<'parse>],
) -> SassResult<Array> {
    let arr = Array::new();
    for a in args {
        arr.push(&value_to_js(ctx, a)?);
    }
    Ok(arr)
}

fn read_accessed_argument_lists(envelope: &JsValue) -> SassResult<Vec<u32>> {
    let v = match Reflect::get(envelope, &JsValue::from_str("accessedArgumentLists")) {
        Ok(v) => v,
        Err(_) => return Ok(Vec::new()),
    };
    if v.is_undefined() || v.is_null() {
        return Ok(Vec::new());
    }
    let arr = Array::from(&v);
    let mut out = Vec::with_capacity(arr.length() as usize);
    for i in 0..arr.length() {
        out.push(
            arr.get(i)
                .as_f64()
                .ok_or_else(|| script("accessedArgumentLists entries must be numbers"))?
                as u32,
        );
    }
    Ok(out)
}

/// Reads the `{value, accessedArgumentLists}` envelope returned by the shim's
/// wrapped JS function and converts it to a Sass value.
fn parse_function_result<'compile: 'parse, 'parse: 'compile>(
    ctx: &MarshallingContext<'parse>,
    arena: &'compile Bump,
    envelope: &JsValue,
) -> SassResult<Value<'parse>> {
    let accessed = read_accessed_argument_lists(envelope)?;
    let value = Reflect::get(envelope, &JsValue::from_str("value"))
        .map_err(|e| script(format!("function result is missing a value: {e:?}")))?;
    let host_builder = host_function_builder(ctx.clone());
    value_from_js_with_accessed(ctx, arena, &value, &accessed, &*host_builder)
}

/// Builds a `BuiltInCallable` whose callback marshals its arguments to JS,
/// calls `js_fn`, and marshals the result back. Two `#[cfg]`-gated variants so
/// only one is typechecked per build.
#[cfg(not(feature = "async"))]
pub fn make_host_callable<'compile: 'parse, 'parse>(
    signature: &str,
    js_fn: js_sys::Function,
    ctx: MarshallingContext<'parse>,
    arena: &'compile Bump,
) -> SassResult<BuiltInCallable<'compile, 'parse>> {
    let (name, params) = split_signature(signature)?;
    let cb: SyncBuiltInCallback<'compile, 'parse> = Rc::new(move |_config, _state, args, arena| {
        let js_args = args_to_js(&ctx, &args)?;
        let result = call_js(&js_fn, &js_args)?;
        if is_promise(&result) {
            return Err(script(
                "can't return a Promise for synchronous compile functions",
            ));
        }
        parse_function_result(&ctx, arena, &result)
    });
    Ok(BuiltInCallable::function(&name, &params, "", arena, cb))
}

#[cfg(feature = "async")]
// `redundant_locals`: the `let arena = arena;` rebind inside is deliberate —
// it pins the captured `&'compile Bump` against the per-call `&'a Bump`.
#[allow(clippy::redundant_locals)]
pub fn make_host_callable<'compile: 'parse, 'parse>(
    signature: &str,
    js_fn: js_sys::Function,
    ctx: MarshallingContext<'parse>,
    arena: &'compile Bump,
) -> SassResult<BuiltInCallable<'compile, 'parse>> {
    let (name, params) = split_signature(signature)?;
    let arena = arena; // captured &'compile Bump; the per-call &'a Bump is the wrong lifetime
    let cb: AsyncBuiltInCallback<'compile, 'parse> = Rc::new(
        move |_config,
              _state,
              args,
              _call_arena|
              -> LocalBoxFuture<'_, SassResult<Value<'parse>>> {
            let ctx = ctx.clone();
            let js_fn = js_fn.clone();
            let js_args = match args_to_js(&ctx, &args) {
                Ok(a) => a,
                Err(e) => return Box::pin(async move { Err(e) }),
            };
            Box::pin(async move {
                let result = call_js(&js_fn, &js_args)?;
                let result = if is_promise(&result) {
                    let p = result
                        .dyn_into::<js_sys::Promise>()
                        .map_err(|_| script("expected a Promise"))?;
                    JsFuture::from(p).await.map_err(js_error_to_script)?
                } else {
                    result
                };
                parse_function_result(&ctx, arena, &result)
            })
        },
    );
    Ok(BuiltInCallable::function_async(
        &name, &params, "", arena, cb,
    ))
}

/// Builds a host-defined `Callable` for a JS-created `SassFunction(signature,
/// callback)` value (the wire format's `hostFunction`).
pub fn host_function_builder<'compile: 'parse, 'parse>(
    ctx: MarshallingContext<'parse>,
) -> Box<HostFunctionBuilder<'compile, 'parse>> {
    Box::new(
        move |signature: &str, callback: &JsValue, arena: &'compile Bump| {
            let js_fn = callback
                .clone()
                .dyn_into::<js_sys::Function>()
                .map_err(|_| script("host function callback is not a function"))?;
            let callable = make_host_callable(signature, js_fn, ctx.clone(), arena)?;
            Ok(Callable::new(arena, CallableKind::BuiltIn(callable)))
        },
    )
}

// ==== js_sys helpers =========================================================

fn get_js_str_opt(obj: &JsValue, key: &str) -> SassResult<Option<String>> {
    let v = Reflect::get(obj, &JsValue::from_str(key))
        .map_err(|e| script(format!("Failed to read \"{key}\": {e:?}")))?;
    if v.is_undefined() || v.is_null() {
        Ok(None)
    } else {
        js_url_to_string(&v)
            .ok_or_else(|| script(format!("Expected \"{key}\" to be a string")))
            .map(Some)
    }
}
