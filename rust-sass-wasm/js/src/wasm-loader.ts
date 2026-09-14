// Loads the wasm artifacts lazily per entry point. The sync artifact
// (`pkg-sync`, zero futures) backs the sync compile functions; the async
// artifact (`pkg-async`) backs the async ones.

import { AsyncWasmModule, SyncWasmModule } from './types';

let syncModule: SyncWasmModule | undefined;
let asyncModule: AsyncWasmModule | undefined;

/** The sync artifact (backing `compile`/`compileString`). */
export function loadSyncWasm(): SyncWasmModule {
  if (syncModule === undefined) {
    // Paths are relative to the compiled output (js/dist) and point at the
    // wasm-pack builds. Each artifact is a separate wasm instance with its own
    // clock atomic; prime it at first load so any date-sensitive behavior
    // (e.g. deprecation warnings) uses real time. Done lazily per artifact at
    // first use rather than eagerly at module init to avoid loading both
    // artifacts up front.
    const mod = require('./pkg-sync/rust_sass_wasm.js') as SyncWasmModule;
    mod.set_time_now?.(Date.now());
    syncModule = mod;
  }
  return syncModule;
}

/** The async artifact (backing `compileAsync`/`compileStringAsync`). */
export function loadAsyncWasm(): AsyncWasmModule {
  if (asyncModule === undefined) {
    const mod = require('./pkg-async/rust_sass_wasm.js') as AsyncWasmModule;
    mod.set_time_now?.(Date.now());
    asyncModule = mod;
  }
  return asyncModule;
}
