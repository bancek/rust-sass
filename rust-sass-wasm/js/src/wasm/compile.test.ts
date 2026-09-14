import { afterAll, describe, expect, it } from 'vitest';
import * as fs from 'node:fs';
import * as p from 'node:path';
import * as sass from 'sass';

import {
  compile,
  compileAsync,
  compileString,
  compileStringAsync,
  SassNumber,
} from '../index';
import { artifactsPresent, tempDir } from './helpers';

describe.skipIf(!artifactsPresent)('compileString parity with sass', () => {
  it('matches a basic stylesheet', () => {
    const input = '$c: #123456;\n.foo { color: $c; width: 10px; }\n';
    expect(compileString(input).css).toBe(sass.compileString(input).css);
  });

  it('matches the compressed style', () => {
    const input = '.foo { color: red; }\n.bar { color: blue; }';
    expect(compileString(input, { style: 'compressed' }).css).toBe(
      sass.compileString(input, { style: 'compressed' }).css,
    );
  });

  it('resolves loadPaths with exact loadedUrls parity', () => {
    const dir = tempDir();
    fs.writeFileSync(p.join(dir, '_x.scss'), '$c: red;');
    const source = '@use "x";\n.a { color: x.$c; }';
    const ours = compileString(source, { loadPaths: [dir] });
    const theirs = sass.compileString(source, { loadPaths: [dir] });
    expect(ours.css).toBe(theirs.css);
    expect(ours.loadedUrls.map(String)).toEqual(theirs.loadedUrls.map(String));
  });

  it('matches an async compile with async function + importer', async () => {
    const source = '@use "async-imp";\na { b: add(1, 2); c: async-imp.$v; }';
    const importers = [
      {
        canonicalize: async (url: string) =>
          url === 'async-imp' ? new URL('async-imp:index.scss') : null,
        load: async (url: string | URL) =>
          String(url) === 'async-imp:index.scss'
            ? { contents: '$v: 42;', syntax: 'scss' as const }
            : null,
      },
    ];
    const ours = await compileStringAsync(source, {
      functions: {
        'add($a, $b)': async (args: sass.Value[]) =>
          new SassNumber(
            args[0].assertNumber().value + args[1].assertNumber().value,
          ),
      },
      importers,
    });
    const theirs = await sass.compileStringAsync(source, {
      functions: {
        'add($a, $b)': async (args: sass.Value[]) =>
          new sass.SassNumber(
            args[0].assertNumber().value + args[1].assertNumber().value,
          ),
      },
      importers,
    });
    expect(ours.css).toBe(theirs.css);
  });
});

describe.skipIf(!artifactsPresent)('compileString bridge contract', () => {
  it('includes a source map when requested and omits it by default', () => {
    expect(compileString('a {b: c}')).not.toHaveProperty('sourceMap');
    expect(compileString('a {b: c}', { sourceMap: true })).toHaveProperty(
      'sourceMap',
    );
  });

  it('returns loadedUrls as URL objects', () => {
    const result = compileString('$a: 1;\n.b { c: $a; }');
    expect(result.loadedUrls).toBeInstanceOf(Array);
    expect(result.loadedUrls.length).toBe(
      sass.compileString('$a: 1;\n.b { c: $a; }').loadedUrls.length,
    );
    for (const url of result.loadedUrls) expect(url).toBeInstanceOf(URL);
  });
});

describe.skipIf(!artifactsPresent)('compile/compileAsync via node io', () => {
  let dir: string;

  afterAll(() => {
    if (dir) fs.rmSync(dir, { recursive: true, force: true });
  });

  function writeEntry(): string {
    dir = tempDir();
    fs.writeFileSync(p.join(dir, 'input.scss'), '$x: 3px;\n.a { width: $x; }');
    fs.writeFileSync(p.join(dir, '_sub.scss'), 'b {x: y}');
    const entry = p.join(dir, 'main.scss');
    fs.writeFileSync(entry, '@import "sub";\n.a { width: 1px; }');
    return entry;
  }

  it('matches sass for compile(path)', () => {
    const entry = writeEntry();
    expect(compile(entry).css).toBe(sass.compile(entry).css);
  });

  it('matches sass for compileAsync(path)', async () => {
    const entry = writeEntry();
    expect((await compileAsync(entry)).css).toBe(
      (await sass.compileAsync(entry)).css,
    );
  });
});
