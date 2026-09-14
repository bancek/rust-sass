// CompileResult -> JS object: {css, sourceMap?, loadedUrls}.
//
// dart-source: lib/src/js/compile_result.dart

use js_sys::{Array, Object, Reflect};
use wasm_bindgen::JsValue;

use rust_sass::common::exception::SassResult;
use rust_sass::compile::result::CompileResult;

use crate::marshaller::script;

pub fn compile_result_to_js(result: &CompileResult<'_, '_>) -> SassResult<JsValue> {
    let obj = Object::new();
    let _ = Reflect::set(
        &obj,
        &JsValue::from_str("css"),
        &JsValue::from_str(result.css()),
    );

    if let Some(map) = result.source_map() {
        let json = String::from_utf8(map.json()?)
            .map_err(|e| script(format!("source map is not valid UTF-8: {e}")))?;
        let parsed = js_sys::JSON::parse(&json)
            .map_err(|e| script(format!("Failed to parse source map JSON: {e:?}")))?;
        let _ = Reflect::set(&obj, &JsValue::from_str("sourceMap"), &parsed);
    }

    let urls = Array::new();
    for u in result.loaded_urls() {
        urls.push(&JsValue::from_str(u.as_str()));
    }
    let _ = Reflect::set(&obj, &JsValue::from_str("loadedUrls"), &urls);

    Ok(obj.into())
}

/// Sync-expansion only: every test body below drives the sync entry points
/// directly, so the module cannot compile in async mode. Async behavior is
/// covered by the lib/embedded async suites and the TS-side gates instead.
#[cfg(all(test, not(feature = "async")))]
mod tests {
    use super::*;
    use rust_sass::compile;
    use rust_sass::compile::options::CompileOptions;
    use rust_sass::io::VirtualIo;
    use rust_sass::Bump;
    use std::rc::Rc;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn get(obj: &js_sys::Object, key: &str) -> JsValue {
        Reflect::get(obj, &JsValue::from_str(key)).unwrap()
    }

    #[wasm_bindgen_test]
    fn compile_result_shape() {
        let arena = Bump::new();
        let io: Rc<dyn rust_sass::io::Io> = Rc::new(VirtualIo::new());
        let opts = CompileOptions::new(&arena);
        let result = compile::compile_string("a { b: c; }", io, opts, &arena).unwrap();
        let v = compile_result_to_js(&result).unwrap();
        let obj = js_sys::Object::from(v);
        assert_eq!(get(&obj, "css").as_string().unwrap(), "a {\n  b: c;\n}");
        assert!(get(&obj, "sourceMap").is_undefined());
        assert!(Array::from(&get(&obj, "loadedUrls")).length() == 0);
    }

    #[wasm_bindgen_test]
    fn compile_result_with_source_map() {
        let arena = Bump::new();
        let io: Rc<dyn rust_sass::io::Io> = Rc::new(VirtualIo::new());
        let mut opts = CompileOptions::new(&arena);
        opts.source_map = true;
        let result = compile::compile_string("a { b: c; }", io, opts, &arena).unwrap();
        let v = compile_result_to_js(&result).unwrap();
        let obj = js_sys::Object::from(v);
        let sm = get(&obj, "sourceMap");
        assert_eq!(
            Reflect::get(&sm, &JsValue::from_str("version"))
                .unwrap()
                .as_f64(),
            Some(3.0)
        );
    }
}
