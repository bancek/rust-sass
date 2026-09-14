// `Compiler`/`AsyncCompiler` + `initCompiler`/`initAsyncCompiler`.
//
// Stateless wrappers over the top-level compile functions (a fresh `Adapter`
// and wasm call per compilation — no shared state across compilations, per
// docs/ref/wasm.md, "Approach" D2 "keep stateless wrappers for fidelity"). Constructor behavior
// mirrors `lib/src/js/compiler.dart`: direct construction throws; instances
// come from `initCompiler`/`initAsyncCompiler`; `dispose()` poisons the
// instance; `AsyncCompiler.dispose()` waits for in-flight compilations.

import {
  compile,
  compileAsync,
  compileString,
  compileStringAsync,
  CompileOptions,
  CompileStringOptions,
  CompileResult,
} from './index';

/** Throws if the instance has been disposed (compiler.dart `_throwIfDisposed`). */
function throwIfDisposed(disposed: boolean): void {
  if (disposed) throw new Error('Compiler has already been disposed.');
}

/** Bypasses the throwing constructor (compiler.dart `injectSuperclass`). */
function create<T>(prototype: T, fields: Record<string, unknown>): T {
  return Object.assign(Object.create(prototype as object), fields);
}

export class Compiler {
  protected disposed = false;

  constructor() {
    throw new Error(
      'Compiler can not be directly constructed. Please use `sass.initCompiler()` instead.',
    );
  }

  compile(path: string, options?: CompileOptions): CompileResult {
    throwIfDisposed(this.disposed);
    return compile(path, options);
  }

  compileString(source: string, options?: CompileStringOptions): CompileResult {
    throwIfDisposed(this.disposed);
    return compileString(source, options);
  }

  dispose(): void {
    this.disposed = true;
  }
}

/** `sass.initCompiler()`. */
export function initCompiler(): Compiler {
  return create(Compiler.prototype, { disposed: false });
}

export class AsyncCompiler {
  private disposed = false;
  /** In-flight compilations (error-swallowed), so `dispose()` can await them. */
  private readonly compilations = new Set<Promise<void>>();

  constructor() {
    throw new Error(
      'AsyncCompiler can not be directly constructed. Please use `sass.initAsyncCompiler()` instead.',
    );
  }

  private track(compilation: Promise<unknown>): void {
    const tracked = compilation.then(
      () => undefined,
      () => undefined,
    );
    this.compilations.add(tracked);
    tracked.finally(() => this.compilations.delete(tracked));
  }

  compileAsync(path: string, options?: CompileOptions): Promise<CompileResult> {
    throwIfDisposed(this.disposed);
    const compilation = compileAsync(path, options);
    this.track(compilation);
    return compilation;
  }

  compileStringAsync(
    source: string,
    options?: CompileStringOptions,
  ): Promise<CompileResult> {
    throwIfDisposed(this.disposed);
    const compilation = compileStringAsync(source, options);
    this.track(compilation);
    return compilation;
  }

  async dispose(): Promise<void> {
    this.disposed = true;
    await Promise.all([...this.compilations]);
    return undefined;
  }
}

/** `sass.initAsyncCompiler()`. */
export function initAsyncCompiler(): Promise<AsyncCompiler> {
  return Promise.resolve(
    create(AsyncCompiler.prototype, {
      disposed: false,
      compilations: new Set<Promise<void>>(),
    }),
  );
}
