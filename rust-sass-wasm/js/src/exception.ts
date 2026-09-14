// `Exception` — the public error class (js-api-doc / lib/src/js/exception.dart).
// Built from the plain data object Rust throws (docs/ref/wasm.md, "Approach" D8). The wasm entry
// points return `Result<JsValue, JsValue>`; wasm-bindgen throws the `Err` value
// (a plain data object) which the public functions catch and wrap here.

import { ExceptionData } from './types';

export class Exception extends Error {
  readonly sassMessage: string;
  readonly sassStack: string;
  readonly span: unknown;
  readonly loadedUrls: URL[];

  constructor(data: ExceptionData) {
    super((data.formatted ?? data.sassMessage ?? '').replace(/^Error: /, ''));
    this.name = 'Exception';
    this.sassMessage = data.sassMessage;
    this.sassStack = data.sassStack ?? '';
    this.span = normalizeSpan(data.span);
    this.loadedUrls = (data.loadedUrls ?? []).map((url) => new URL(url));
  }
}

/** Converts the wire `span.url` string to a `URL` instance (Dart-exact:
 * `SourceSpan.url?: URL` — lib/src/js/source_span.dart `dartToJSUrl`). */
function normalizeSpan(span: unknown): unknown {
  if (!span || typeof span !== 'object') return span;
  const s = span as { url?: unknown };
  if (typeof s.url !== 'string') return span;
  return { ...s, url: new URL(s.url) };
}

export function isErrorData(e: unknown): e is ExceptionData {
  return (
    typeof e === 'object' &&
    e !== null &&
    typeof (e as { sassMessage?: unknown }).sassMessage === 'string'
  );
}

/** Wraps a wasm-thrown value in an `Exception` when it is error data. */
export function wrapException(e: unknown): never {
  if (isErrorData(e)) throw new Exception(e);
  throw e;
}
