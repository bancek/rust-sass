import { afterAll, describe, expect, it } from 'vitest';
import * as sass from 'sass';
import * as fs from 'node:fs';
import * as p from 'node:path';

import {
  Exception,
  compileString,
  initAsyncCompiler,
  initCompiler,
} from '../index';
import { artifactsPresent, tempDir } from './helpers';

describe.skipIf(!artifactsPresent)('Compiler parity with sass', () => {
  it('matches sass and the module root for compileString', () => {
    const input = '$c: #123456;\n.foo { color: $c; width: 10px; }\n';
    const ours = initCompiler().compileString(input).css;
    const theirs = sass.initCompiler().compileString(input).css;
    const root = compileString(input).css;
    expect(ours).toBe(theirs);
    expect(ours).toBe(root);
  });

  it('matches sass for compile(path)', () => {
    const dir = tempDir();
    fs.writeFileSync(p.join(dir, 'input.scss'), '$x: 3px;\n.a { width: $x; }');
    const ours = initCompiler().compile(p.join(dir, 'input.scss')).css;
    const theirs = sass.initCompiler().compile(p.join(dir, 'input.scss')).css;
    expect(ours).toBe(theirs);
    fs.rmSync(dir, { recursive: true, force: true });
  });
});

describe.skipIf(!artifactsPresent)('Compiler bridge contract', () => {
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

  it('allows a second dispose', () => {
    const compiler = initCompiler();
    compiler.dispose();
    expect(() => compiler.dispose()).not.toThrow();
  });

  it('succeeds after a compilation failure', () => {
    const compiler = initCompiler();
    expect(() => compiler.compileString('a')).toThrowError(Exception);
    const result = compiler.compileString('x {y: z}');
    expect(result.css).toBe(sass.compileString('x {y: z}').css);
  });
});

describe.skipIf(!artifactsPresent)('AsyncCompiler parity with sass', () => {
  it('matches sass for compileStringAsync', async () => {
    const input = '$c: #123456;\n.foo { color: $c; width: 10px; }\n';
    const ours = await (await initAsyncCompiler()).compileStringAsync(input);
    const theirs = await (
      await sass.initAsyncCompiler()
    ).compileStringAsync(input);
    expect(ours.css).toBe(theirs.css);
  });
});

describe.skipIf(!artifactsPresent)('AsyncCompiler bridge contract', () => {
  it('resolves initAsyncCompiler and interleaves concurrent compilations', async () => {
    const compiler = await initAsyncCompiler();
    const runs = 10;
    const results = await Promise.all(
      Array(runs)
        .fill(0)
        .map((_, i) => compiler.compileStringAsync(`.a${i} {b: ${i}}`)),
    );
    results.forEach((result, i) => {
      expect(result.css).toBe(sass.compileString(`.a${i} {b: ${i}}`).css);
    });
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

  it('allows a second dispose', async () => {
    const compiler = await initAsyncCompiler();
    await compiler.dispose();
    await expect(compiler.dispose()).resolves.toBeUndefined();
  });

  it('waits for compilations to finish before disposing', async () => {
    let completed = false;
    let triggerComplete: () => void = () => undefined;
    const awaited = new Promise<void>((resolve) => {
      triggerComplete = resolve;
    });
    const importer = {
      canonicalize: async () => new URL('foo:bar'),
      load: async () => {
        await awaited;
        completed = true;
        return { contents: 'x {y: z}', syntax: 'scss' as const };
      },
    };
    const compiler = await initAsyncCompiler();
    const compilation = compiler.compileStringAsync('@use "slow"', {
      importers: [importer],
    });

    const disposalPromise = compiler.dispose();
    expect(completed).toBe(false);
    triggerComplete();

    await disposalPromise;
    expect(completed).toBe(true);
    await expect(compilation).resolves.toBeDefined();
  });
});
