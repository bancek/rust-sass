import { describe, expect, it } from 'vitest';
import * as sass from 'sass';
import * as fs from 'node:fs';
import * as p from 'node:path';

import { Exception, compile, compileString } from '../index';
import { artifactsPresent, tempDir } from './helpers';

describe.skipIf(!artifactsPresent)('errors bridge contract', () => {
  it('throws an Exception for a syntax error', () => {
    try {
      compileString('a { b: }');
      expect.unreachable('should have thrown');
    } catch (e) {
      expect(e).toBeInstanceOf(Exception);
      expect(typeof (e as Exception).sassMessage).toBe('string');
      expect((e as Exception).sassMessage.length).toBeGreaterThan(0);
    }
  });

  it('exposes Exception span.url as a URL instance', () => {
    const url = new URL('file:///style.scss');
    try {
      compileString('a { b: }', { url });
      expect.unreachable('should have thrown');
    } catch (e) {
      const span = (e as Exception).span as { url?: unknown };
      expect(span.url).toBeInstanceOf(URL);
      expect(String(span.url)).toBe('file:///style.scss');
    }
  });

  it('renders sassStack like sass for a nested runtime error', () => {
    const source =
      '@use "sass:meta"; @include meta.apply(meta.get-function("nope"));';
    let oursStack: string | undefined;
    let theirsStack: string | undefined;
    try {
      compileString(source, {});
    } catch (e) {
      oursStack = (e as Exception).sassStack;
    }
    try {
      sass.compileString(source, {});
    } catch (e) {
      theirsStack = (e as Exception).sassStack;
    }
    expect(oursStack).toBe(theirsStack);
    expect(oursStack).toMatch(/root stylesheet\n$/);
  });

  it('renders sassStack with file paths for a file-based compile', () => {
    const dir = tempDir();
    fs.writeFileSync(p.join(dir, 'input.scss'), '@use "sub";\n.a { b: c; }');
    fs.writeFileSync(
      p.join(dir, '_sub.scss'),
      '@mixin foo { @error "boom" } @include foo;',
    );
    const entry = p.join(dir, 'input.scss');
    let oursStack: string | undefined;
    let theirsStack: string | undefined;
    try {
      compileString('a {b: c}'); // warm the artifact
      oursStack = compile(entry).css;
    } catch (e) {
      oursStack = (e as Exception).sassStack;
    }
    try {
      theirsStack = sass.compile(entry).css;
    } catch (e) {
      theirsStack = (e as Exception).sassStack;
    }
    expect(oursStack).toBe(theirsStack);
    expect(oursStack).toContain(p.join(dir, '_sub.scss'));
    expect(oursStack).toContain(p.join(dir, 'input.scss'));
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it('renders a "root stylesheet" sassStack for a syntax error (parity)', () => {
    const grab = (fn: () => unknown): string => {
      try {
        fn();
      } catch (e) {
        return (e as Exception).sassStack;
      }
      throw new Error('expected a throw');
    };
    expect(grab(() => compileString('a {b:'))).toBe(
      grab(() => sass.compileString('a {b:')),
    );
    const url = new URL('file:///style.scss');
    expect(grab(() => compileString('a {b:', { url }))).toBe(
      grab(() => sass.compileString('a {b:', { url })),
    );
  });
});
