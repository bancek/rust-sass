import { describe, expect, it } from 'vitest';
import * as sass from 'sass';

import {
  compileString,
  SassArgumentList,
  SassNumber,
  SassString,
  sassNull,
} from '../index';
import { artifactsPresent } from './helpers';

describe.skipIf(!artifactsPresent)('function signature validation', () => {
  it('rejects signatures with whitespace (parity)', () => {
    for (const sig of [' foo()', 'foo ()', 'foo() ', 'foo(', '$foo()', '']) {
      expect(() =>
        compileString('', { functions: { [sig]: () => sassNull } }),
      ).toThrow();
    }
  });
});

describe.skipIf(!artifactsPresent)('custom functions parity with sass', () => {
  it('matches a number-returning function', () => {
    const source = 'a { b: double(5); }';
    expect(
      compileString(source, {
        functions: {
          'double($n)': (args) =>
            new SassNumber(args[0].assertNumber().value * 2),
        },
      }).css,
    ).toBe(
      sass.compileString(source, {
        functions: {
          'double($n)': (args) =>
            new sass.SassNumber(args[0].assertNumber().value * 2),
        },
      }).css,
    );
  });

  it('matches string + rest-args functions', () => {
    const source = 'a { b: greet(World); c: sum(1, 2, 3); }';
    const ours = compileString(source, {
      functions: {
        'greet($name)': (args) =>
          new SassString(`hello ${args[0].assertString().text}`, {
            quotes: false,
          }),
        'sum($args...)': (args) => {
          let total = 0;
          for (const v of args[0].asList) total += v.assertNumber().value;
          return new SassNumber(total);
        },
      },
    });
    const theirs = sass.compileString(source, {
      functions: {
        'greet($name)': (args) =>
          new sass.SassString(`hello ${args[0].assertString().text}`, {
            quotes: false,
          }),
        'sum($args...)': (args) => {
          let total = 0;
          for (const v of args[0].asList) total += v.assertNumber().value;
          return new sass.SassNumber(total);
        },
      },
    });
    expect(ours.css).toBe(theirs.css);
  });
});

describe.skipIf(!artifactsPresent)('custom functions bridge contract', () => {
  it('rejects Promise returns from synchronous compiles', () => {
    expect(() =>
      compileString('a { b: f(); }', {
        functions: { 'f()': async () => new SassNumber(1) },
      }),
    ).toThrowError("can't return a Promise for synchronous compile functions");
  });
});

describe.skipIf(!artifactsPresent)('cross-compilation value ownership', () => {
  it('passes a captured argument list by value into another compilation', () => {
    let saved: unknown;
    compileString('a { b: store(1, 2); }', {
      functions: {
        'store($args...)': (args) => {
          saved = args[0];
          return new SassNumber(1);
        },
      },
    });
    const source = '@use "sass:list"; a { b: list.length(use-saved()); }';
    const ours = compileString(source, {
      functions: { 'use-saved()': () => saved as SassArgumentList },
    });
    const theirs = (() => {
      let theirsSaved: unknown;
      sass.compileString('a { b: store(1, 2); }', {
        functions: {
          'store($args...)': (args) => {
            theirsSaved = args[0];
            return new sass.SassNumber(1);
          },
        },
      });
      return sass.compileString(source, {
        functions: { 'use-saved()': () => theirsSaved as sass.Value },
      });
    })();
    expect(ours.css).toBe(theirs.css);
  });

  // NOTE: a compiler `SassFunction` never reaches JS via custom-function args
  // (function values are serialized as `SassString`, matching the official
  // `sass` package), so the "does not belong to this compilation" throw is only
  // reachable through a manually-constructed value — covered by the colocated
  // `adapter.test.ts` cross-compilation unit tests.
});
