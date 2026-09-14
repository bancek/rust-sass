import { describe, expect, it } from 'vitest';

import {
  AsyncCompiler,
  Compiler,
  initAsyncCompiler,
  initCompiler,
} from './compiler';

describe('Compiler', () => {
  it('cannot be constructed directly', () => {
    const Untyped = Compiler as unknown as new () => unknown;
    expect(() => new Untyped()).toThrowError(
      /Compiler can not be directly constructed/,
    );
  });

  it('initCompiler returns a working instance', () => {
    const compiler = initCompiler();
    expect(compiler).toBeInstanceOf(Compiler);
    expect(compiler.compile).toBeTypeOf('function');
    expect(compiler.compileString).toBeTypeOf('function');
    compiler.dispose();
  });

  it('throws after being disposed', () => {
    const compiler = initCompiler();
    compiler.dispose();
    expect(() => compiler.compileString('a {b: c}')).toThrowError(
      'Compiler has already been disposed.',
    );
    expect(() => compiler.compile('/tmp/input.scss')).toThrowError(
      'Compiler has already been disposed.',
    );
  });
});

describe('AsyncCompiler', () => {
  it('cannot be constructed directly', () => {
    const Untyped = AsyncCompiler as unknown as new () => unknown;
    expect(() => new Untyped()).toThrowError(
      /AsyncCompiler can not be directly constructed/,
    );
  });

  it('initAsyncCompiler resolves to an AsyncCompiler', async () => {
    const compiler = await initAsyncCompiler();
    expect(compiler).toBeInstanceOf(AsyncCompiler);
    await compiler.dispose();
  });

  it('throws after being disposed', async () => {
    const compiler = await initAsyncCompiler();
    await compiler.dispose();
    expect(() => compiler.compileStringAsync('a {b: c}')).toThrowError(
      'Compiler has already been disposed.',
    );
    expect(() => compiler.compileAsync('/tmp/input.scss')).toThrowError(
      'Compiler has already been disposed.',
    );
  });
});
