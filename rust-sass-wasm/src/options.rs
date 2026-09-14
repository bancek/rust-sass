// Parses the JS options object into a rust-sass `CompileOptions`.
//
// Functions/importers/logger are JS functions (not serde-able), so the options
// object is traversed manually with js_sys. Mirrors `lib/src/js/compile.dart`
// (`_parseFunctions`, `_parseImporter`) validation and defaults.
//
// dart-source: lib/src/js/compile.dart + lib/src/js/compile_options.dart

use std::collections::HashSet;
use std::rc::Rc;

use js_sys::{Array, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue};

use rust_sass::callable::{Callable, CallableKind};
use rust_sass::common::exception::SassResult;
use rust_sass::compile::options::{CompileOptions, OutputStyle};
use rust_sass::deprecation::{self, Deprecation};
use rust_sass::eval::importer::node_package::NodePackageImporter;
use rust_sass::eval::importer::{Importer, ImporterKind};
use rust_sass::io::Io;
use rust_sass::logger::{Logger, QuietLogger};
use rust_sass::parse::stylesheet::{CssState, SassIndentState, Syntax};
use rust_sass::url::SassUrl;
use rust_sass::Bump;

use crate::js_host::{make_host_callable, JsFileImporter, JsImporter, JsLogger};
use crate::marshaller::{script, MarshallingContext};

pub fn parse_options<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    options: &JsValue,
    io: Rc<dyn Io>,
    ctx: MarshallingContext<'parse>,
) -> SassResult<CompileOptions<'compile, 'parse>> {
    let mut opts = CompileOptions::new(arena);
    if options.is_undefined() || options.is_null() {
        return Ok(opts);
    }

    if let Some(style) = get_str_opt(options, "style")? {
        opts.style = match style.as_str() {
            "expanded" => OutputStyle::Expanded,
            "compressed" => OutputStyle::Compressed,
            other => return Err(script(format!("Unknown output style \"{other}\"."))),
        };
    }

    opts.source_map = get_bool_opt(options, "sourceMap")?.unwrap_or(false);
    opts.include_source_map_sources =
        get_bool_opt(options, "sourceMapIncludeSources")?.unwrap_or(false);
    opts.charset = get_bool_opt(options, "charset")?.unwrap_or(true);
    opts.quiet_deps = get_bool_opt(options, "quietDeps")?.unwrap_or(false);
    opts.verbose = get_bool_opt(options, "verbose")?.unwrap_or(false);

    // rust-sass extension knobs (docs/ref/wasm.md, "Extension options"). `alertColor`/
    // `alertAscii` are DECOUPLED: they never write `opts.alert_color`/
    // `opts.alert_ascii` (which would change eval-time message baking). They
    // stay render-time only — the `JsLogger` fallback and error rendering
    // (error.rs `highlight_from_options`) read them directly off the raw
    // options — matching sass.js (lib/src/logger/js_to_dart.dart colors only
    // final output; empirically `alertAscii` leaves baked `╷ │ ╵` Unicode).
    // `ascii` is the explicit eval-time knob: it maps to `opts.unicode =
    // false` so baked selector/extend error highlights render ASCII (what the
    // old CLI `unicode` positional flag did). `color` is the
    // explicit render-color knob. CSS/source-map output is never affected.
    let ascii = get_bool_opt(options, "ascii")?.unwrap_or(false);
    let color = get_bool_opt(options, "color")?.unwrap_or(false);
    let alert_ascii = get_bool_opt(options, "alertAscii")?.unwrap_or(false);
    let alert_color = get_bool_opt(options, "alertColor")?.unwrap_or(false);
    if ascii {
        opts.unicode = false;
    }

    if let Some(load_paths) = get_array_opt(options, "loadPaths")? {
        for i in 0..load_paths.length() {
            let path = load_paths
                .get(i)
                .as_string()
                .ok_or_else(|| script("options.loadPaths entries must be strings"))?;
            opts.load_paths.push(path);
        }
    }

    if let Some(syntax) = get_str_opt(options, "syntax")? {
        opts.syntax = match syntax.as_str() {
            "scss" => Syntax::Scss,
            "indented" => Syntax::Sass(SassIndentState {
                current_indentation: 0,
                next_indentation: None,
                next_indentation_end: None,
                indent_spaces: None,
            }),
            "css" => Syntax::Css(CssState {
                disallowed_function_names: HashSet::new(),
            }),
            other => return Err(script(format!("Unknown syntax \"{other}\"."))),
        };
    }

    if let Some(url) = get_str_opt(options, "url")? {
        opts.url =
            Some(SassUrl::parse(&url).map_err(|_| script(format!("Invalid url: \"{url}\"")))?);
    }

    // Parse the logger FIRST so deprecation-list validation warnings route
    // through it (mirrors Dart: `parseDeprecations` takes a JSToDartLogger`).
    // `Logger.silent` is marshalled by the shim as the `__sassSilent` marker
    // and maps to rust-sass's `QuietLogger`. When options.logger is ABSENT (or
    // lacks `warn`/`debug`) we install a `JsLogger` mirroring Dart's
    // `JSToDartLogger`: methods the JS logger defines get plain messages;
    // absent methods render the full terminal block (colored per
    // `color`/`alertColor`, glyph set per `ascii`/`alertAscii`) and forward to
    // the `consoleWarn`/`consoleDebug` sinks (or the JS console fallback),
    // matching the default Dart logger. Sinks give the CLI/playground a
    // byte-exact destination without Node's added console newline.
    let color = color || alert_color;
    let unicode = !(ascii || alert_ascii);
    let console_warn = get_function_opt(options, "consoleWarn")?;
    let console_debug = get_function_opt(options, "consoleDebug")?;
    let logger: Option<Rc<dyn Logger>> = if let Some(logger) = get_object_opt(options, "logger")? {
        if get(&logger.clone().into(), "__sassSilent").is_truthy() {
            Some(Rc::new(QuietLogger) as Rc<dyn Logger>)
        } else {
            let warn = get_function_opt(&logger, "warn")?;
            let debug = get_function_opt(&logger, "debug")?;
            Some(Rc::new(JsLogger::new(
                warn,
                debug,
                console_warn.clone(),
                console_debug.clone(),
                color,
                unicode,
                io.clone(),
            )) as Rc<dyn Logger>)
        }
    } else {
        Some(Rc::new(JsLogger::new(
            None,
            None,
            console_warn.clone(),
            console_debug.clone(),
            color,
            unicode,
            io.clone(),
        )) as Rc<dyn Logger>)
    };
    opts.logger = logger.clone();

    opts.fatal_deprecations = parse_deprecation_list(options, "fatalDeprecations", true, &logger)?;
    opts.silence_deprecations =
        parse_deprecation_list(options, "silenceDeprecations", false, &logger)?;
    opts.future_deprecations =
        parse_deprecation_list(options, "futureDeprecations", false, &logger)?;

    // options.functions: { [signature]: (args: Value[]) => Value }
    if let Some(functions) = get_object_opt(options, "functions")? {
        let keys = Object::keys(&functions);
        for i in 0..keys.length() {
            let sig = keys
                .get(i)
                .as_string()
                .ok_or_else(|| script("function signature must be a string"))?;
            let f = Reflect::get(&functions, &keys.get(i))
                .map_err(|e| script(format!("failed to read function {sig}: {e:?}")))?;
            let js_fn = f
                .dyn_into::<js_sys::Function>()
                .map_err(|_| script(format!("options.functions: {sig} is not a function")))?;
            let callable = make_host_callable(&sig, js_fn, ctx.clone(), arena)?;
            opts.functions
                .push(Callable::new(arena, CallableKind::BuiltIn(callable)));
        }
    }

    // options.importers: array of URL/file importers (and NodePackageImporter)
    if let Some(importers) = get_array_opt(options, "importers")? {
        for i in 0..importers.length() {
            let importer = importers.get(i);
            let kind = parse_one_importer(&importer, io.clone())?;
            opts.importers.push(Importer::new(arena, kind));
        }
    }

    // CompileStringOptions.importer: a single primary importer (compile.dart:
    // `options?.importer.andThen(_parseImporter)`). Absent/null → keep the
    // core default (NoOp when no url).
    if let Some(importer) = get_opt(options, "importer")? {
        let kind = parse_one_importer(&importer, io.clone())?;
        opts.importer = Importer::new(arena, kind);
    }

    Ok(opts)
}

/// Parses one importer value (a single `importer` or an `importers[]` entry)
/// into an `ImporterKind`, mirroring Dart's `_parseImporter` /
/// `_parseAsyncImporter` (compile.dart:331-364) exactly, including the
/// validation messages. Does NOT handle `undefined`/absent (the caller skips
/// those); a `null` importer is an error.
fn parse_one_importer(importer: &JsValue, io: Rc<dyn Io>) -> SassResult<ImporterKind> {
    // NodePackageImporter instances are tagged by the shim
    // (`{__sassNodePackageImporter: "<entryPointDirectory>"}`).
    if let Some(dir) = get(importer, "__sassNodePackageImporter").as_string() {
        return Ok(ImporterKind::NodePackage(NodePackageImporter::new(
            &dir, io,
        )));
    }

    if importer.is_null() || importer.is_undefined() {
        return Err(script("Importers may not be null."));
    }

    let canonicalize = get_function_opt(importer, "canonicalize")?;
    let load = get_function_opt(importer, "load")?;
    let find_file_url = get_function_opt(importer, "findFileUrl")?;
    if find_file_url.is_some() && (canonicalize.is_some() || load.is_some()) {
        return Err(script("An importer may not have a findFileUrl method as well as canonicalize and load methods."));
    }
    if let Some(find_file_url) = find_file_url {
        return Ok(ImporterKind::User(Rc::new(JsFileImporter::new(
            find_file_url,
            io,
        ))));
    }
    let schemes = parse_non_canonical_schemes(importer)?;
    match (canonicalize, load) {
        (Some(canonicalize), Some(load)) => Ok(ImporterKind::User(Rc::new(JsImporter::new(
            canonicalize,
            load,
            schemes,
        )))),
        _ => Err(script(
            "An importer must have either canonicalize and load methods, or a findFileUrl method.",
        )),
    }
}

/// Parses `nonCanonicalScheme` (string | list of strings | null) and validates
/// each scheme, mirroring `_normalizeNonCanonicalSchemes`
/// (compile.dart:368-379) + `validateUrlScheme`
/// (lib/src/importer/js_to_dart/utils.dart).
fn parse_non_canonical_schemes(importer: &JsValue) -> SassResult<Vec<String>> {
    let schemes = get(importer, "nonCanonicalScheme");
    if schemes.is_undefined() || schemes.is_null() {
        return Ok(Vec::new());
    }
    if let Some(s) = schemes.as_string() {
        validate_url_scheme(&s)?;
        return Ok(vec![s]);
    }
    if schemes.is_array() {
        let arr = Array::from(&schemes);
        let mut out = Vec::new();
        for i in 0..arr.length() {
            let s = arr.get(i).as_string().ok_or_else(|| {
                script(format!(
                    "nonCanonicalScheme must be a string or list of strings, was \"{}\"",
                    js_str_repr(&schemes)
                ))
            })?;
            validate_url_scheme(&s)?;
            out.push(s);
        }
        return Ok(out);
    }
    Err(script(format!(
        "nonCanonicalScheme must be a string or list of strings, was \"{}\"",
        js_str_repr(&schemes)
    )))
}

/// Mirrors `validateUrlScheme` (lib/src/importer/js_to_dart/utils.dart): a
/// scheme must match `^[a-z0-9+.-]+$`.
fn validate_url_scheme(scheme: &str) -> SassResult<()> {
    let valid = !scheme.is_empty()
        && scheme.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'+' | b'.' | b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(script(format!(
            "\"{scheme}\" isn't a valid URL scheme (for example \"file\")."
        )))
    }
}

/// The JS string representation of [v] (`String(v)` in JS), used for the
/// `nonCanonicalScheme` error message.
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

/// Parses one of `fatalDeprecations` / `silenceDeprecations` /
/// `futureDeprecations`. Each entry may be a deprecation id string or a
/// `Deprecation` object; when [support_versions] is true (fatal only) it may
/// also be a `Version` (marshalled by the shim as `{__sassVersion}`). Unknown
/// ids produce a warning via the logger, not an error.
/// Mirrors Dart: `parseDeprecations` (lib/src/js/deprecations.dart:54-82).
fn parse_deprecation_list(
    options: &JsValue,
    key: &str,
    support_versions: bool,
    logger: &Option<Rc<dyn Logger>>,
) -> SassResult<Vec<&'static Deprecation>> {
    let mut out = Vec::new();
    let arr = match get_array_opt(options, key)? {
        Some(a) => a,
        None => return Ok(out),
    };
    for i in 0..arr.length() {
        let item = arr.get(i);
        let id = if let Some(s) = item.as_string() {
            Some(s)
        } else if let Ok(obj) = item.clone().dyn_into::<Object>() {
            let obj: JsValue = obj.into();
            let id = get(&obj, "id").as_string();
            if id.is_some() {
                id
            } else if support_versions {
                match get(&obj, "__sassVersion").as_string() {
                    Some(version) => {
                        out.extend(deprecation::for_version(&version));
                        None
                    }
                    None => None,
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some(id) = id {
            match deprecation::from_id(&id) {
                Some(d) => out.push(d),
                None => {
                    let message = format!("Invalid deprecation \"{id}\".");
                    match logger {
                        Some(l) => l.warn(&message, None, None),
                        None => web_sys::console::warn_1(&JsValue::from_str(&message)),
                    }
                }
            }
        }
    }
    Ok(out)
}

// ==== js_sys helpers =========================================================

fn get(obj: &JsValue, key: &str) -> JsValue {
    Reflect::get(obj, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

/// Returns the value of [key] if present and not null/undefined.
fn get_opt(obj: &JsValue, key: &str) -> SassResult<Option<JsValue>> {
    let v = get(obj, key);
    if v.is_undefined() || v.is_null() {
        Ok(None)
    } else {
        Ok(Some(v))
    }
}

fn get_str_opt(obj: &JsValue, key: &str) -> SassResult<Option<String>> {
    let v = get(obj, key);
    if v.is_undefined() || v.is_null() {
        Ok(None)
    } else {
        v.as_string()
            .ok_or_else(|| script(format!("Expected options.{key} to be a string")))
            .map(Some)
    }
}

fn get_bool_opt(obj: &JsValue, key: &str) -> SassResult<Option<bool>> {
    let v = get(obj, key);
    if v.is_undefined() || v.is_null() {
        Ok(None)
    } else {
        v.as_bool()
            .ok_or_else(|| script(format!("Expected options.{key} to be a boolean")))
            .map(Some)
    }
}

fn get_object_opt(obj: &JsValue, key: &str) -> SassResult<Option<Object>> {
    let v = get(obj, key);
    if v.is_undefined() || v.is_null() {
        Ok(None)
    } else {
        v.dyn_into::<Object>()
            .map(Some)
            .map_err(|_| script(format!("Expected options.{key} to be an object")))
    }
}

fn get_array_opt(obj: &JsValue, key: &str) -> SassResult<Option<Array>> {
    let v = get(obj, key);
    if v.is_undefined() || v.is_null() {
        Ok(None)
    } else {
        Ok(Some(Array::from(&v)))
    }
}

fn get_function_opt(obj: &JsValue, key: &str) -> SassResult<Option<js_sys::Function>> {
    let v = get(obj, key);
    if v.is_undefined() || v.is_null() {
        Ok(None)
    } else {
        v.dyn_into::<js_sys::Function>()
            .map(Some)
            .map_err(|_| script(format!("Expected \"{key}\" to be a function")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_sass::io::VirtualIo;
    use std::cell::RefCell;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn set(obj: &Object, key: &str, val: &JsValue) {
        let _ = Reflect::set(obj, &JsValue::from_str(key), val);
    }

    fn arr(items: &[&str]) -> JsValue {
        let a = Array::new();
        for s in items {
            a.push(&JsValue::from_str(s));
        }
        a.into()
    }

    fn parse<'a>(arena: &'a Bump, opts: &Object) -> SassResult<CompileOptions<'a, 'a>> {
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let options: JsValue = opts.clone().into();
        parse_options(arena, &options, io, MarshallingContext::new())
    }

    fn script_err(r: SassResult<CompileOptions<'_, '_>>) -> String {
        match r {
            Err(e) => match *e {
                rust_sass::common::exception::SassError::Script { message, .. } => message,
                _ => panic!("expected a Script error"),
            },
            _ => panic!("expected a Script error"),
        }
    }

    #[wasm_bindgen_test]
    fn parses_scalar_options() {
        let opts = Object::new();
        set(&opts, "style", &JsValue::from_str("compressed"));
        set(&opts, "sourceMap", &JsValue::from_bool(true));
        set(&opts, "sourceMapIncludeSources", &JsValue::from_bool(true));
        set(&opts, "charset", &JsValue::from_bool(false));
        set(&opts, "quietDeps", &JsValue::from_bool(true));
        set(&opts, "verbose", &JsValue::from_bool(true));
        // Decoupled: alertColor/alertAscii never reach the compile config.
        set(&opts, "alertColor", &JsValue::from_bool(true));
        set(&opts, "alertAscii", &JsValue::from_bool(true));
        set(&opts, "syntax", &JsValue::from_str("css"));
        set(
            &opts,
            "url",
            &JsValue::from_str("https://example.com/input.scss"),
        );
        let arena = Bump::new();
        let o = parse(&arena, &opts).unwrap();
        assert!(matches!(o.style, OutputStyle::Compressed));
        assert!(o.source_map);
        assert!(o.include_source_map_sources);
        assert!(!o.charset);
        assert!(o.quiet_deps);
        assert!(o.verbose);
        assert!(!o.alert_color);
        assert!(!o.alert_ascii);
        assert!(o.unicode);
        assert!(matches!(
            o.syntax,
            rust_sass::parse::stylesheet::Syntax::Css(_)
        ));
        assert_eq!(o.url.unwrap().as_str(), "https://example.com/input.scss");
    }

    #[wasm_bindgen_test]
    fn ascii_extension_disables_unicode_eval_baking() {
        // `ascii: true` → `opts.unicode = false` (the old CLI positional
        // `unicode`); `alertAscii` alone must NOT (decoupled, render-time only).
        let ascii = Object::new();
        set(&ascii, "ascii", &JsValue::from_bool(true));
        let arena = Bump::new();
        assert!(!parse(&arena, &ascii).unwrap().unicode);

        let alert_only = Object::new();
        set(&alert_only, "alertAscii", &JsValue::from_bool(true));
        assert!(parse(&arena, &alert_only).unwrap().unicode);
    }

    #[wasm_bindgen_test]
    fn rejects_unknown_style_and_syntax() {
        let opts = Object::new();
        set(&opts, "style", &JsValue::from_str("nope"));
        let arena = Bump::new();
        assert_eq!(
            script_err(parse(&arena, &opts)),
            "Unknown output style \"nope\"."
        );

        let opts = Object::new();
        set(&opts, "syntax", &JsValue::from_str("nope"));
        assert_eq!(script_err(parse(&arena, &opts)), "Unknown syntax \"nope\".");
    }

    #[wasm_bindgen_test]
    fn parses_deprecation_lists() {
        let opts = Object::new();
        set(&opts, "silenceDeprecations", &arr(&["new-global"]));
        set(&opts, "futureDeprecations", &arr(&["slash-div"]));
        let arena = Bump::new();
        let o = parse(&arena, &opts).unwrap();
        assert_eq!(o.silence_deprecations.len(), 1);
        assert_eq!(o.silence_deprecations[0].id, "new-global");
        assert_eq!(o.future_deprecations.len(), 1);
        assert_eq!(o.future_deprecations[0].id, "slash-div");

        // A Version entry (fatal only) is marshalled as {__sassVersion}.
        let version = Object::new();
        set(&version, "__sassVersion", &JsValue::from_str("1.17.2"));
        let fatal = Array::new();
        fatal.push(&version.into());
        let opts = Object::new();
        set(&opts, "fatalDeprecations", &fatal.into());
        let o = parse(&arena, &opts).unwrap();
        assert!(o.fatal_deprecations.iter().any(|d| d.id == "new-global"));
    }

    #[wasm_bindgen_test]
    fn warns_on_unknown_deprecation_id() {
        // A JS logger whose warn captures the message into Rust state.
        let captured: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let c = captured.clone();
        let warn = Closure::wrap(Box::new(move |msg: JsValue, _opts: JsValue| {
            *c.borrow_mut() = msg.as_string();
            JsValue::UNDEFINED
        }) as Box<dyn FnMut(JsValue, JsValue) -> JsValue>);
        let logger = Object::new();
        let _ = Reflect::set(&logger, &JsValue::from_str("warn"), warn.as_ref());
        let opts = Object::new();
        set(&opts, "logger", &logger.into());
        set(&opts, "fatalDeprecations", &arr(&["bogus"]));
        let arena = Bump::new();
        let o = parse(&arena, &opts).unwrap();
        assert!(o.fatal_deprecations.is_empty());
        assert_eq!(
            captured.borrow().as_deref(),
            Some("Invalid deprecation \"bogus\".")
        );
    }

    #[wasm_bindgen_test]
    fn logger_silent_marker_builds_a_logger() {
        let silent = Object::new();
        set(&silent, "__sassSilent", &JsValue::from_bool(true));
        let opts = Object::new();
        set(&opts, "logger", &silent.into());
        let arena = Bump::new();
        let o = parse(&arena, &opts).unwrap();
        assert!(o.logger.is_some());
    }

    #[wasm_bindgen_test]
    fn parses_importers() {
        let canonicalize = js_sys::Function::new_no_args("return null;");
        let load = js_sys::Function::new_no_args("return null;");
        let arena = Bump::new();

        let url_importer = Object::new();
        let _ = Reflect::set(
            &url_importer,
            &JsValue::from_str("canonicalize"),
            &canonicalize,
        );
        let _ = Reflect::set(&url_importer, &JsValue::from_str("load"), &load);
        let _ = Reflect::set(
            &url_importer,
            &JsValue::from_str("nonCanonicalScheme"),
            &JsValue::from_str("db"),
        );
        let importers = Array::new();
        importers.push(&url_importer.into());
        let opts = Object::new();
        set(&opts, "importers", &importers.into());
        let o = parse(&arena, &opts).unwrap();
        assert_eq!(o.importers.len(), 1);
        assert!(matches!(o.importers[0].kind(), ImporterKind::User(_)));

        // findFileUrl importer
        let find = js_sys::Function::new_no_args("return null;");
        let file_importer = Object::new();
        let _ = Reflect::set(&file_importer, &JsValue::from_str("findFileUrl"), &find);
        let importers = Array::new();
        importers.push(&file_importer.into());
        let opts = Object::new();
        set(&opts, "importers", &importers.into());
        let o = parse(&arena, &opts).unwrap();
        assert!(matches!(o.importers[0].kind(), ImporterKind::User(_)));

        // NodePackageImporter tag
        let npi = Object::new();
        set(
            &npi,
            "__sassNodePackageImporter",
            &JsValue::from_str("/tmp/entry"),
        );
        let importers = Array::new();
        importers.push(&npi.into());
        let opts = Object::new();
        set(&opts, "importers", &importers.into());
        let o = parse(&arena, &opts).unwrap();
        assert!(matches!(
            o.importers[0].kind(),
            ImporterKind::NodePackage(_)
        ));

        // single importer option maps to opts.importer
        let url_importer = Object::new();
        let _ = Reflect::set(
            &url_importer,
            &JsValue::from_str("canonicalize"),
            &canonicalize,
        );
        let _ = Reflect::set(&url_importer, &JsValue::from_str("load"), &load);
        let opts = Object::new();
        set(&opts, "importer", &url_importer.into());
        let o = parse(&arena, &opts).unwrap();
        assert!(matches!(o.importer.kind(), ImporterKind::User(_)));
    }

    #[wasm_bindgen_test]
    fn rejects_invalid_importers() {
        let arena = Bump::new();
        let importers = Array::new();
        importers.push(&JsValue::NULL);
        let opts = Object::new();
        set(&opts, "importers", &importers.into());
        assert_eq!(
            script_err(parse(&arena, &opts)),
            "Importers may not be null."
        );

        let canonicalize = js_sys::Function::new_no_args("return null;");
        let load = js_sys::Function::new_no_args("return null;");
        let find = js_sys::Function::new_no_args("return null;");
        let both = Object::new();
        let _ = Reflect::set(&both, &JsValue::from_str("canonicalize"), &canonicalize);
        let _ = Reflect::set(&both, &JsValue::from_str("load"), &load);
        let _ = Reflect::set(&both, &JsValue::from_str("findFileUrl"), &find);
        let importers = Array::new();
        importers.push(&both.into());
        let opts = Object::new();
        set(&opts, "importers", &importers.into());
        assert_eq!(
            script_err(parse(&arena, &opts)),
            "An importer may not have a findFileUrl method as well as canonicalize and load methods."
        );

        let neither = Object::new();
        let importers = Array::new();
        importers.push(&neither.into());
        let opts = Object::new();
        set(&opts, "importers", &importers.into());
        assert_eq!(
            script_err(parse(&arena, &opts)),
            "An importer must have either canonicalize and load methods, or a findFileUrl method."
        );
    }

    #[wasm_bindgen_test]
    fn validates_non_canonical_schemes() {
        let arena = Bump::new();
        let canonicalize = js_sys::Function::new_no_args("return null;");
        let load = js_sys::Function::new_no_args("return null;");
        let make = |schemes: &JsValue| {
            let importer = Object::new();
            let _ = Reflect::set(&importer, &JsValue::from_str("canonicalize"), &canonicalize);
            let _ = Reflect::set(&importer, &JsValue::from_str("load"), &load);
            let _ = Reflect::set(&importer, &JsValue::from_str("nonCanonicalScheme"), schemes);
            let importers = Array::new();
            importers.push(&importer.into());
            let opts = Object::new();
            set(&opts, "importers", &importers.into());
            parse(&arena, &opts)
        };

        // String scheme is fine.
        assert!(make(&JsValue::from_str("db")).is_ok());
        // List of schemes is fine.
        let list = Array::new();
        list.push(&JsValue::from_str("db"));
        list.push(&JsValue::from_str("sass"));
        assert!(make(&list.into()).is_ok());
        // Non-string/list → error.
        assert_eq!(
            script_err(make(&JsValue::from_f64(5.0))),
            "nonCanonicalScheme must be a string or list of strings, was \"5\""
        );
        // Invalid scheme → error.
        assert_eq!(
            script_err(make(&JsValue::from_str("not a scheme!"))),
            "\"not a scheme!\" isn't a valid URL scheme (for example \"file\")."
        );
    }
}
