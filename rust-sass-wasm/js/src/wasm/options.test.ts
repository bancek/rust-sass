import { describe, expect, it } from 'vitest';
import * as sass from 'sass';

import { Exception, Version, compileString } from '../index';
import { deprecations } from '../deprecations';
import { artifactsPresent } from './helpers';

describe.skipIf(!artifactsPresent)('options parity with sass', () => {
  it('emits @charset for non-ASCII output by default and omits it with charset: false', () => {
    const input = 'a {b: あ;}';
    expect(compileString(input).css).toBe(sass.compileString(input).css);
    expect(compileString(input, { charset: false }).css).toBe(
      sass.compileString(input, { charset: false }).css,
    );
  });

  it('compiles the indented and css syntaxes', () => {
    expect(compileString('a\n  b: c', { syntax: 'indented' }).css).toBe(
      sass.compileString('a\n  b: c', { syntax: 'indented' }).css,
    );
    expect(compileString('a {b: c}', { syntax: 'css' }).css).toBe(
      sass.compileString('a {b: c}', { syntax: 'css' }).css,
    );
  });
});

describe.skipIf(!artifactsPresent)('options bridge contract', () => {
  it('throws an Exception for an unrecognized style', () => {
    try {
      compileString('a {b: c}', { style: 'nope' as never });
      expect.unreachable('should have thrown');
    } catch (e) {
      expect(e).toBeInstanceOf(Exception);
    }
  });

  it('throws an Exception for an unrecognized syntax', () => {
    try {
      compileString('a {b: c}', { syntax: 'nope' as never });
      expect.unreachable('should have thrown');
    } catch (e) {
      expect(e).toBeInstanceOf(Exception);
    }
  });
});

describe.skipIf(!artifactsPresent)('importer option bridge contract', () => {
  it('throws Dart-exact nonCanonicalScheme validation error', () => {
    const importer = {
      canonicalize: () => null,
      load: () => null,
      nonCanonicalScheme: 5,
    };
    expect(() => compileString('a {b: c}', { importers: [importer] })).toThrow(
      'nonCanonicalScheme must be a string or list of strings, was "5"',
    );
  });

  it('throws Dart-exact invalid-scheme error', () => {
    const importer = {
      canonicalize: () => null,
      load: () => null,
      nonCanonicalScheme: 'not a scheme!',
    };
    expect(() => compileString('a {b: c}', { importers: [importer] })).toThrow(
      '"not a scheme!" isn\'t a valid URL scheme (for example "file").',
    );
  });
});

describe.skipIf(!artifactsPresent)('deprecation lists parity with sass', () => {
  const source = 'a { $b: c !global; }';

  it('silences a deprecation by id', () => {
    const warn = () => {
      throw new Error('should not warn');
    };
    const ours = compileString(source, {
      silenceDeprecations: ['new-global'],
      logger: { warn },
    });
    const theirs = sass.compileString(source, {
      silenceDeprecations: ['new-global'],
      logger: { warn },
    });
    expect(ours.css).toBe(theirs.css);
  });

  it('throws when a deprecation is fatal by id', () => {
    expect(() =>
      compileString(source, { fatalDeprecations: ['new-global'] }),
    ).toThrow(Exception);
    expect(() =>
      sass.compileString(source, { fatalDeprecations: ['new-global'] }),
    ).toThrow(/!global/);
  });

  it('throws when a deprecation is fatal by Version', () => {
    expect(() =>
      compileString(source, { fatalDeprecations: [new Version(1, 17, 2)] }),
    ).toThrow(Exception);
  });

  it('throws when a deprecation is fatal by Deprecation object', () => {
    expect(() =>
      compileString(source, {
        fatalDeprecations: [{ id: 'new-global', status: 'active' as const }],
      }),
    ).toThrow(Exception);
  });

  it('accepts full deprecations-map objects in the lists', () => {
    const warn = () => {
      throw new Error('should not warn');
    };
    const ours = compileString(source, {
      silenceDeprecations: [deprecations['new-global']],
      logger: { warn },
    });
    const theirs = sass.compileString(source, {
      silenceDeprecations: [deprecations['new-global']],
      logger: { warn },
    });
    expect(ours.css).toBe(theirs.css);

    expect(() =>
      compileString(source, {
        fatalDeprecations: [deprecations['new-global']],
      }),
    ).toThrow(Exception);
    expect(() =>
      compileString(source, {
        futureDeprecations: [deprecations['new-global']],
      }),
    ).not.toThrow();
  });

  it('warns about unknown deprecation ids via the logger', () => {
    const oursWarnings: unknown[][] = [];
    const theirsWarnings: unknown[][] = [];
    compileString('a {b: c}', {
      fatalDeprecations: ['bogus'],
      logger: { warn: (...args: unknown[]) => oursWarnings.push(args) },
    });
    sass.compileString('a {b: c}', {
      fatalDeprecations: ['bogus'],
      logger: { warn: (...args: unknown[]) => theirsWarnings.push(args) },
    });
    expect(oursWarnings.length).toBe(1);
    expect(oursWarnings[0][0]).toBe(theirsWarnings[0][0]);
  });
});
