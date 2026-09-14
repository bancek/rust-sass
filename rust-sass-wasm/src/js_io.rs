// A JS-delegate-backed `Io` for the wasm bridge. The delegate is a purpose-built
// typed JS object (see `js/src/io/node-fs.ts`) — NOT a mirror of the Rust `Io`
// trait — exposing only the primitives the bridge needs:
//
//   readFile(path): Uint8Array          fileExists/dirExists/linkExists(path): bool
//   readDir(path): string[]             (all entries, incl. dirs — for canonicalize)
//   currentDir(): string                isWindows()/isMacOS()/supportsAnsiEscapes(): bool
//
// Every operation may fail; on failure the delegate THROWS (or, for the async
// delegate, rejects with) an `IoError`-shaped error `{message, kind, path?}`.
// `JsIo` maps that shape onto Rust `IoError`, preserving the original
// message/kind/path (docs/ref/wasm.md, "Io bridge").
//
// Sync/async: the sync build invokes the (sync) delegate directly; the async
// build awaits the (always-Promise) delegate via `JsFuture`. `readDir` is a
// SYNC delegate method on both delegates (used only by the synchronous
// `real_case_path` recursion). Written once with `#[maybe_async]`.
//
// dart-source: lib/src/io/js.dart (Node behavior)

#[cfg(feature = "async")]
use crate::js_host::is_promise;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::rc::Rc;

use js_sys::{Array, Reflect};
use wasm_bindgen::{JsCast, JsValue};
#[cfg(feature = "async")]
use wasm_bindgen_futures::JsFuture;

use rust_sass::io::{clean_path, real_case_path, Io, IoError, IoErrorKind};

#[cfg(feature = "async")]
use futures::future::LocalBoxFuture;

pub struct JsIo {
    delegate: JsValue,
    real_case_cache: Rc<RefCell<HashMap<String, String>>>,
}

impl fmt::Debug for JsIo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsIo").finish_non_exhaustive()
    }
}

fn io_error(msg: impl Into<String>, kind: IoErrorKind, path: &str) -> IoError {
    IoError {
        message: msg.into(),
        kind,
        path: if path.is_empty() {
            None
        } else {
            Some(path.to_string())
        },
    }
}

fn js_str_repr(v: &JsValue) -> String {
    if let Some(s) = v.as_string() {
        return s;
    }
    if let Ok(m) = Reflect::get(v, &JsValue::from_str("message")) {
        if let Some(s) = m.as_string() {
            return s;
        }
    }
    format!("{v:?}")
}

fn get_str(v: &JsValue, key: &str) -> Option<String> {
    Reflect::get(v, &JsValue::from_str(key))
        .ok()
        .and_then(|m| m.as_string())
}

/// Maps a JS error value (a thrown `IoError`-shaped object, a rejection, or an
/// arbitrary throw) onto the Rust `IoError`, preserving message/kind/path.
fn io_error_from_js(e: &JsValue, fallback_path: &str) -> IoError {
    let message = js_str_repr(e);
    let kind = match get_str(e, "kind").as_deref() {
        Some("NotFound") => IoErrorKind::NotFound,
        Some("Permission") => IoErrorKind::Permission,
        Some("AlreadyExists") => IoErrorKind::AlreadyExists,
        _ => IoErrorKind::Other,
    };
    let path = get_str(e, "path").or_else(|| {
        if fallback_path.is_empty() {
            None
        } else {
            Some(fallback_path.to_string())
        }
    });
    IoError {
        message,
        kind,
        path,
    }
}

impl JsIo {
    pub fn new(delegate: JsValue) -> Self {
        JsIo {
            delegate,
            real_case_cache: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    /// A clone sharing the delegate and the `real_case_cache` (used by the
    /// per-call `Box::pin` futures).
    fn share(&self) -> JsIo {
        JsIo {
            delegate: self.delegate.clone(),
            real_case_cache: self.real_case_cache.clone(),
        }
    }

    /// Invokes a delegate method and returns the raw `JsValue`. Sync delegate:
    /// the value (or a synchronously thrown `IoError`). Async delegate: for fs
    /// methods this is the Promise (await it with [`JsIo::call_await`]).
    fn call(&self, method: &str, args: &[&JsValue]) -> Result<JsValue, IoError> {
        let f = Reflect::get(&self.delegate, &JsValue::from_str(method)).map_err(|_| {
            io_error(
                format!("io delegate is missing a {method}() method"),
                IoErrorKind::Other,
                method,
            )
        })?;
        if !f.is_function() {
            return Err(io_error(
                format!("io delegate has no {method}() method"),
                IoErrorKind::Other,
                method,
            ));
        }
        let f: js_sys::Function = f.dyn_into().unwrap();
        let array = Array::new();
        for a in args {
            array.push(a);
        }
        f.apply(&JsValue::UNDEFINED, &array)
            .map_err(|e| io_error_from_js(&e, ""))
    }

    /// Invokes a delegate method, awaiting the Promise it returns (async build
    /// only). Sync-returning delegate methods (e.g. `readDir`) pass through.
    #[cfg(feature = "async")]
    async fn call_await(&self, method: &str, args: &[&JsValue]) -> Result<JsValue, IoError> {
        let v = self.call(method, args)?;
        if !is_promise(&v) {
            return Ok(v);
        }
        let p = v.dyn_into::<js_sys::Promise>().map_err(|_| {
            io_error(
                "expected a Promise from the io delegate",
                IoErrorKind::Other,
                method,
            )
        })?;
        JsFuture::from(p)
            .await
            .map_err(|e| io_error_from_js(&e, ""))
    }

    fn read_dir_entries(&self, dir: &str) -> Option<Vec<String>> {
        self.call("readDir", &[&JsValue::from_str(dir)])
            .ok()
            .map(|v| {
                Array::from(&v)
                    .iter()
                    .filter_map(|e| e.as_string())
                    .collect()
            })
    }
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[rust_sass_macros::maybe_async]
impl Io for JsIo {
    fn read_file<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<Vec<u8>, IoError>> {
        let this = self.share();
        let p = path_arg(path);
        Box::pin(async move {
            #[cfg(feature = "async")]
            let v = this
                .call_await("readFile", &[&JsValue::from_str(&p)])
                .await?;
            #[cfg(not(feature = "async"))]
            let v = this.call("readFile", &[&JsValue::from_str(&p)])?;
            Ok(js_sys::Uint8Array::from(v).to_vec())
        })
    }

    fn file_exists<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<bool, IoError>> {
        let this = self.share();
        let p = path_arg(path);
        Box::pin(async move {
            #[cfg(feature = "async")]
            let v = this
                .call_await("fileExists", &[&JsValue::from_str(&p)])
                .await?;
            #[cfg(not(feature = "async"))]
            let v = this.call("fileExists", &[&JsValue::from_str(&p)])?;
            v.as_bool()
                .ok_or_else(|| io_error("fileExists must return a boolean", IoErrorKind::Other, &p))
        })
    }

    fn dir_exists<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<bool, IoError>> {
        let this = self.share();
        let p = path_arg(path);
        Box::pin(async move {
            #[cfg(feature = "async")]
            let v = this
                .call_await("dirExists", &[&JsValue::from_str(&p)])
                .await?;
            #[cfg(not(feature = "async"))]
            let v = this.call("dirExists", &[&JsValue::from_str(&p)])?;
            v.as_bool()
                .ok_or_else(|| io_error("dirExists must return a boolean", IoErrorKind::Other, &p))
        })
    }

    fn link_exists<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<bool, IoError>> {
        let this = self.share();
        let p = path_arg(path);
        Box::pin(async move {
            #[cfg(feature = "async")]
            let v = this
                .call_await("linkExists", &[&JsValue::from_str(&p)])
                .await?;
            #[cfg(not(feature = "async"))]
            let v = this.call("linkExists", &[&JsValue::from_str(&p)])?;
            v.as_bool()
                .ok_or_else(|| io_error("linkExists must return a boolean", IoErrorKind::Other, &p))
        })
    }

    /// Matches Dart's `io.canonicalize` + `DefaultIo.canonicalize`: absolute +
    /// lexical `clean_path`, then on case-insensitive filesystems
    /// `real_case_path` (which preserves symlink directory names — `/var` stays
    /// `/var`). Uses the delegate `readDir` primitive; there is NO delegate
    /// `canonicalize` method (docs/ref/wasm.md, "Io bridge").
    fn canonicalize<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<String, IoError>> {
        let this = self.share();
        let p = path_arg(path);
        Box::pin(async move {
            let cwd = this.current_dir();
            let abs = if Path::new(&p).is_absolute() {
                p
            } else {
                Path::new(&cwd).join(&p).to_string_lossy().into_owned()
            };
            let normalized = clean_path(&abs);
            if this.is_macos() || this.is_windows() {
                let mut cache = this.real_case_cache.borrow_mut();
                let read_dir = |dir: &str| this.read_dir_entries(dir);
                Ok(real_case_path(&normalized, &mut cache, read_dir))
            } else {
                Ok(normalized)
            }
        })
    }

    fn current_dir(&self) -> String {
        self.call("currentDir", &[])
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "/".to_string())
    }

    fn is_windows(&self) -> bool {
        self.call("isWindows", &[])
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    fn is_macos(&self) -> bool {
        self.call("isMacOS", &[])
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    fn supports_ansi_escapes(&self) -> bool {
        self.call("supportsAnsiEscapes", &[])
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }
}

/// Sync-expansion only: every test body below calls the sync `Io` surface
/// directly, so the module cannot compile in async mode. Async JsIo behavior
/// is covered by the lib/embedded async suites and the TS-side gates instead.
#[cfg(all(test, not(feature = "async")))]
mod tests {
    use super::*;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen_test::wasm_bindgen_test;

    use js_sys::Object;

    fn set(obj: &Object, key: &str, val: &JsValue) {
        let _ = Reflect::set(obj, &JsValue::from_str(key), val);
    }

    /// A plain (state-free) JS function taking one string argument.
    fn fn1(arg: &str, body: &str) -> js_sys::Function {
        js_sys::Function::new_with_args(arg, body)
    }

    /// A delegate backing the minimal `Io` surface with deterministic
    /// readDir/isMacOS/currentDir values. All methods are plain JS functions
    /// (no `Closure`s — those would be dropped when this returns).
    fn minimal_delegate() -> Object {
        let obj = Object::new();
        set(
            &obj,
            "readFile",
            &js_sys::Function::new_with_args("p", "return new Uint8Array([104,105]);"),
        );
        set(&obj, "fileExists", &fn1("p", "return p === '/x';"));
        set(&obj, "dirExists", &fn1("p", "return p === '/d';"));
        set(&obj, "linkExists", &fn1("p", "return p === '/l';"));
        set(
            &obj,
            "readDir",
            &js_sys::Function::new_with_args("p", "return [p + '/a', p + '/B'];"),
        );
        set(
            &obj,
            "currentDir",
            &js_sys::Function::new_no_args("return '/cwd';"),
        );
        set(
            &obj,
            "isWindows",
            &js_sys::Function::new_no_args("return false;"),
        );
        set(
            &obj,
            "isMacOS",
            &js_sys::Function::new_no_args("return true;"),
        );
        set(
            &obj,
            "supportsAnsiEscapes",
            &js_sys::Function::new_no_args("return false;"),
        );
        obj
    }

    #[wasm_bindgen_test]
    fn delegates_the_minimal_io_surface() {
        let io = JsIo::new(minimal_delegate().into());

        assert_eq!(io.read_file(Path::new("/x")).unwrap(), vec![104, 105]);
        assert!(io.file_exists(Path::new("/x")).unwrap());
        assert!(!io.file_exists(Path::new("/nope")).unwrap());
        assert!(io.dir_exists(Path::new("/d")).unwrap());
        assert!(io.link_exists(Path::new("/l")).unwrap());
        assert_eq!(io.current_dir(), "/cwd");
        assert!(!io.is_windows());
        assert!(io.is_macos());
        assert!(!io.supports_ansi_escapes());
    }

    #[wasm_bindgen_test]
    fn canonicalize_is_lexical_and_case_corrects_via_read_dir() {
        let obj = minimal_delegate();
        // Case-correct each component: `/foo/bAr` -> entries list `/foo/BAR`.
        set(
            &obj,
            "readDir",
            &js_sys::Function::new_with_args(
                "p",
                "return p === '/foo' ? ['/foo/BAR'] : [p + '/foo'];",
            ),
        );
        let io = JsIo::new(obj.into());
        // macOS gate (isMacOS true) -> real_case_path runs; the component
        // `bAr` matches the `/foo/BAR` entry case-insensitively and is replaced
        // with the on-disk name `BAR`.
        let result = io.canonicalize(Path::new("/foo/bAr")).unwrap();
        assert_eq!(result, "/foo/BAR");
    }

    #[wasm_bindgen_test]
    fn canonicalize_preserves_symlink_dir_names() {
        let obj = minimal_delegate();
        // Simulate `/var` being a symlink whose on-disk listing keeps `var`:
        // the root listing returns `/var` (not `/private/var`).
        set(
            &obj,
            "readDir",
            &js_sys::Function::new_with_args(
                "p",
                "if (p === '/') return ['/var']; return [p + '/folders'];",
            ),
        );
        let io = JsIo::new(obj.into());
        let result = io.canonicalize(Path::new("/var/folders/x.scss")).unwrap();
        assert_eq!(result, "/var/folders/x.scss");
    }

    #[wasm_bindgen_test]
    fn canonicalize_without_macos_is_lexical_only() {
        let obj = minimal_delegate();
        set(
            &obj,
            "isMacOS",
            &js_sys::Function::new_no_args("return false;"),
        );
        set(
            &obj,
            "isWindows",
            &js_sys::Function::new_no_args("return false;"),
        );
        let io = JsIo::new(obj.into());
        let result = io.canonicalize(Path::new("/foo/./bar/../baz")).unwrap();
        assert_eq!(result, "/foo/baz");
    }

    #[wasm_bindgen_test]
    fn maps_thrown_io_error_shapes() {
        // A delegate whose readFile throws an IoError-shaped object.
        let obj = Object::new();
        let err_obj = js_sys::Object::new();
        set(&err_obj, "message", &JsValue::from_str("boom"));
        set(&err_obj, "kind", &JsValue::from_str("Permission"));
        set(&err_obj, "path", &JsValue::from_str("/x"));
        let err: JsValue = err_obj.clone().into();
        let throw = Closure::wrap(Box::new(move |_: JsValue| -> Result<JsValue, JsValue> {
            Err(err.clone())
        })
            as Box<dyn FnMut(JsValue) -> Result<JsValue, JsValue>>);
        set(&obj, "readFile", throw.as_ref());

        let io = JsIo::new(obj.into());
        let e = io.read_file(Path::new("/x")).unwrap_err();
        assert_eq!(e.message, "boom");
        assert!(e.is_permission());
        assert_eq!(e.path.as_deref(), Some("/x"));
    }

    #[wasm_bindgen_test]
    fn missing_method_errors() {
        let io = JsIo::new(Object::new().into());
        let err = io.read_file(Path::new("/x")).unwrap_err();
        assert!(err.message.contains("readFile"));
    }
}
