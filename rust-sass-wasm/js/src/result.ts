// Result normalization: the wasm `WasmCompileResult` → the public `CompileResult`
// (loadedUrls become `URL` instances; `sourceMap` is absent when not produced).

import { WasmCompileResult } from './types';

export interface CompileResult {
  css: string;
  sourceMap?: unknown;
  loadedUrls: URL[];
}

export function normalizeResult(result: WasmCompileResult): CompileResult {
  return {
    css: result.css,
    loadedUrls: result.loadedUrls.map((url) => new URL(url)),
    ...(result.sourceMap !== undefined ? { sourceMap: result.sourceMap } : {}),
  };
}
