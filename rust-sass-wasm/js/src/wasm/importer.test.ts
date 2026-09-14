import { describe, expect, it } from 'vitest';
import * as sass from 'sass';
import * as fs from 'node:fs';
import * as p from 'node:path';
import { pathToFileURL } from 'node:url';

import { compile, compileString } from '../index';
import { artifactsPresent, tempDir } from './helpers';

const importer = {
  canonicalize(url: string) {
    return url === 'theme' ? new URL('theme:theme.scss') : null;
  },
  load(url: string | URL) {
    return String(url) === 'theme:theme.scss'
      ? { contents: '$c: red;', syntax: 'scss' as const }
      : null;
  },
};

describe.skipIf(!artifactsPresent)('importer parity with sass', () => {
  it('resolves a @use through canonicalize + load', () => {
    const source = '@use "theme";\n.foo { color: theme.$c; }';
    expect(compileString(source, { importers: [importer] }).css).toBe(
      sass.compileString(source, { importers: [importer] }).css,
    );
  });

  it('passes containingUrl as a URL instance for relative loads', () => {
    const source = '@use "theme";\n.foo { color: theme.$c; }';
    let oursContainingUrl: unknown;
    let theirsContainingUrl: unknown;
    const oursImporter = {
      ...importer,
      canonicalize: (url: string, ctx: { containingUrl: URL | null }) => {
        oursContainingUrl = ctx.containingUrl;
        return importer.canonicalize(url);
      },
    };
    const theirsImporter = {
      ...importer,
      canonicalize: (url: string, ctx: { containingUrl: URL | null }) => {
        theirsContainingUrl = ctx.containingUrl;
        return importer.canonicalize(url);
      },
    };
    const url = new URL('file:///style.scss');
    expect(compileString(source, { url, importers: [oursImporter] }).css).toBe(
      sass.compileString(source, { url, importers: [theirsImporter] }).css,
    );
    expect(oursContainingUrl).toBeInstanceOf(URL);
    expect(theirsContainingUrl).toBeInstanceOf(URL);
    expect(String(oursContainingUrl)).toBe(String(theirsContainingUrl));
  });

  it('accepts a URL instance for load sourceMapUrl', () => {
    const withSourceMapUrl = {
      ...importer,
      load: (url: string | URL) => ({
        contents: '$c: red;',
        syntax: 'scss' as const,
        sourceMapUrl: new URL('file:///style.scss.map'),
      }),
    };
    const source = '@use "theme";\n.foo { color: theme.$c; }';
    expect(() =>
      compileString(source, { importers: [withSourceMapUrl] }),
    ).not.toThrow();
    expect(
      sass.compileString(source, { importers: [withSourceMapUrl] }).css,
    ).toBe(compileString(source, { importers: [withSourceMapUrl] }).css);
  });

  it('supports the single importer option', () => {
    const source = '@use "theme";\n.foo { color: theme.$c; }';
    expect(compileString(source, { importer: importer }).css).toBe(
      sass.compileString(source, { importer: importer }).css,
    );
  });

  it('resolves a @use through a findFileUrl file importer', () => {
    const dir = tempDir();
    fs.writeFileSync(p.join(dir, '_index.scss'), '$c: red;');
    const fileImporter = {
      findFileUrl(url: string) {
        if (url !== 'theme') return null;
        return new URL(pathToFileURL(p.join(dir, '_index.scss')));
      },
    };
    const source = '@use "theme";\n.foo { color: theme.$c; }';
    const ours = compileString(source, { importers: [fileImporter] });
    const theirs = sass.compileString(source, { importers: [fileImporter] });
    expect(ours.css).toBe(theirs.css);
  });
});

describe.skipIf(!artifactsPresent)('importer bridge contract', () => {
  it('throws for a null importer', () => {
    expect(() =>
      compileString('a {b: c}', { importers: [null as never] }),
    ).toThrow('Importers may not be null.');
  });

  it('throws when findFileUrl coexists with canonicalize/load', () => {
    const bad = {
      canonicalize: () => new URL('x:x'),
      load: () => null,
      findFileUrl: () => null,
    };
    expect(() => compileString('a {b: c}', { importers: [bad] })).toThrow(
      'An importer may not have a findFileUrl method as well as canonicalize and load methods.',
    );
  });

  it('throws when an importer has neither canonicalize/load nor findFileUrl', () => {
    expect(() => compileString('a {b: c}', { importers: [{}] })).toThrow(
      'An importer must have either canonicalize and load methods, or a findFileUrl method.',
    );
  });

  it('throws when findFileUrl returns a non-URL', () => {
    const bad = { findFileUrl: () => 'theme' as never };
    expect(() => compileString('@use "theme";', { importers: [bad] })).toThrow(
      'The findFileUrl() method must return a URL.',
    );
  });

  it('throws Dart-exact message when findFileUrl returns a non-file URL', () => {
    const bad = { findFileUrl: () => new URL('https://example.com/x.scss') };
    expect(() => compileString('@use "x";', { importers: [bad] })).toThrow(
      'The findFileUrl() must return a URL with scheme file://, was "x".',
    );
  });

  it('throws when canonicalize returns a non-URL', () => {
    const bad = {
      canonicalize: () => 'x' as never,
      load: () => null,
    };
    expect(() => compileString('@use "x";', { importers: [bad] })).toThrow(
      'The canonicalize() method must return a URL.',
    );
  });

  it('throws Dart-exact message when canonicalize returns a Promise synchronously', () => {
    const bad = {
      canonicalize: async () => null,
      load: () => null,
    };
    expect(() => compileString('@use "x";', { importers: [bad] })).toThrow(
      "The canonicalize() function can't return a Promise for synchronous compile functions.",
    );
  });

  it('throws Dart-exact message when load contents is not a string', () => {
    const bad = {
      canonicalize: () => new URL('x:x'),
      load: () => ({ contents: 5, syntax: 'scss' }),
    };
    expect(() => compileString('@use "x";', { importers: [bad] })).toThrow(
      'Invalid argument (contents): must be a string but was: number',
    );
  });

  it('throws Dart-exact message when load result lacks syntax', () => {
    const bad = {
      canonicalize: () => new URL('x:x'),
      load: () => ({ contents: 'a' }),
    };
    expect(() => compileString('@use "x";', { importers: [bad] })).toThrow(
      'The load() function must return an object with contents and syntax fields.',
    );
  });

  it('throws Dart-exact message when sourceMapUrl is not absolute', () => {
    const bad = {
      canonicalize: () => new URL('u:x'),
      load: () => ({ contents: '', syntax: 'scss', sourceMapUrl: {} }),
    };
    expect(() => compileString('@use "x";', { importers: [bad] })).toThrow(
      'Invalid argument (sourceMapUrl): must be absolute',
    );
  });

  it('imports the same relative url from different base urls as different files', () => {
    const dir = tempDir();
    fs.mkdirSync(p.join(dir, 'sub1', 'sub2'), { recursive: true });
    fs.writeFileSync(
      p.join(dir, 'main.scss'),
      '@use "sub1/test"; @use "sub1/sub2/test" as test2',
    );
    fs.writeFileSync(p.join(dir, 'sub1', 'test.scss'), '@use "y"');
    fs.writeFileSync(p.join(dir, 'sub1', 'x.scss'), 'x { from: sub1; }');
    fs.writeFileSync(p.join(dir, 'sub1', 'sub2', 'test.scss'), '@use "y"');
    fs.writeFileSync(
      p.join(dir, 'sub1', 'sub2', 'x.scss'),
      'x { from: sub2; }',
    );
    try {
      const findFileUrl = (url: string, ctx: { containingUrl: URL | null }) =>
        url === 'y' ? new URL('x.scss', ctx.containingUrl!) : null;
      const calls: string[] = [];
      const ours = compile(p.join(dir, 'main.scss'), {
        importers: [
          {
            findFileUrl: (url, ctx) => {
              calls.push(String(ctx.containingUrl));
              return findFileUrl(url, ctx);
            },
          },
        ],
      });
      expect(ours.css).toBe(
        sass.compile(p.join(dir, 'main.scss'), {
          importers: [{ findFileUrl }],
        }).css,
      );
      expect(calls).toHaveLength(2);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
