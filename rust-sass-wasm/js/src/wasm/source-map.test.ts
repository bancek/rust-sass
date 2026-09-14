import { describe, expect, it } from 'vitest';
import * as sass from 'sass';

import { compileString } from '../index';
import { artifactsPresent } from './helpers';

describe.skipIf(!artifactsPresent)('source map bridge contract', () => {
  it('omits sourcesContent by default', () => {
    // Matches `sass`: sourcesContent is only embedded when
    // sourceMapIncludeSources is true.
    const input = '$c: #123456;\n.foo { color: $c; width: 10px; }\n';
    const result = compileString(input, { sourceMap: true });
    const sourcesContent = (
      result.sourceMap as { sourcesContent?: string[] } | undefined
    )?.sourcesContent;
    expect(sourcesContent).toBeUndefined();
  });

  it('embeds sourcesContent with sourceMapIncludeSources', () => {
    const input = '$c: #123456;\n.foo { color: $c; width: 10px; }\n';
    const result = compileString(input, {
      sourceMap: true,
      sourceMapIncludeSources: true,
    });
    const sourcesContent = (
      result.sourceMap as { sourcesContent?: string[] } | undefined
    )?.sourcesContent;
    expect(sourcesContent).toBeDefined();
    expect(sourcesContent?.[0]).toBe(input);
  });
});

describe.skipIf(!artifactsPresent)('source map parity with sass', () => {
  it('has structurally valid fields', () => {
    const ours = compileString('a {b: c}', { sourceMap: true }).sourceMap as {
      version: unknown;
      sources: unknown;
      names: unknown;
      mappings: unknown;
    };
    expect(typeof ours.version).toBe('number');
    expect(Array.isArray(ours.sources)).toBe(true);
    expect(Array.isArray(ours.names)).toBe(true);
    expect(typeof ours.mappings).toBe('string');
  });

  it('matches sass exactly (data URL + value mappings)', () => {
    const input = '$c: #123456;\n.foo {\n  color: $c;\n  width: 10px;\n}\n';
    const ours = compileString(input, { sourceMap: true }).sourceMap;
    const theirs = sass.compileString(input, { sourceMap: true }).sourceMap;
    expect(ours).toEqual(theirs);
    // Dart-exact `sources` data URL: `data:;charset=utf-8,` (no MIME type) with
    // Dart's restricted percent-encoding, and value mappings for declarations
    expect((ours as { sources: string[] }).sources).toEqual([
      'data:;charset=utf-8,$c:%20%23123456;%0A.foo%20%7B%0A%20%20color:%20$c;%0A%20%20width:%2010px;%0A%7D%0A',
    ]);
    expect((ours as { mappings: string }).mappings).toBe('AACA;EACE,OAFE;EAGF');
  });
});
