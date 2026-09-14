import { describe, expect, it } from 'vitest';

import { Adapter, WireValue } from './adapter';
import { deprecations } from './deprecations';
import { Value } from './value/index';
import { SassArgumentList } from './value/argument-list';
import { sassFalse, sassTrue } from './value/boolean';
import { SassColor } from './value/color';
import { SassFunction } from './value/function';
import { SassList } from './value/list';
import { SassMap } from './value/map';
import { SassMixin } from './value/mixin';
import { SassNumber } from './value/number';
import { sassNull } from './value/null';
import { SassString } from './value/string';

describe('Adapter.valueFromWasm', () => {
  it('maps boolean/null/string/number wire values', () => {
    const a = new Adapter();
    expect(a.valueFromWasm({ type: 'boolean', value: true })).toBe(sassTrue);
    expect(a.valueFromWasm({ type: 'boolean', value: false })).toBe(sassFalse);
    expect(a.valueFromWasm({ type: 'null' })).toBe(sassNull);

    const s = a.valueFromWasm({
      type: 'string',
      text: 'x',
      quoted: true,
    }) as SassString;
    expect(s).toBeInstanceOf(SassString);
    expect(s.text).toBe('x');
    expect(s.hasQuotes).toBe(true);

    const n = a.valueFromWasm({
      type: 'number',
      value: 3,
      numeratorUnits: ['px'],
      denominatorUnits: [],
    }) as SassNumber;
    expect(n).toBeInstanceOf(SassNumber);
    expect(n.value).toBe(3);
    expect(n.numeratorUnits.toArray()).toEqual(['px']);
  });

  it('maps color wire values including missing channels', () => {
    const a = new Adapter();
    const c = a.valueFromWasm({
      type: 'color',
      space: 'rgb',
      channel1: 1,
      channel2: 2,
      channel3: 3,
      alpha: 0.5,
      missing: [false, false, false, false],
    }) as SassColor;
    expect(c).toBeInstanceOf(SassColor);
    expect(c.space).toBe('rgb');
    expect(c.alpha).toBe(0.5);
    expect(c.isChannelMissing('red')).toBe(false);

    const m = a.valueFromWasm({
      type: 'color',
      space: 'oklch',
      channel1: 50,
      channel2: 10,
      channel3: 30,
      alpha: undefined,
      missing: [false, false, false, true],
    }) as SassColor;
    expect(m.space).toBe('oklch');
    expect(m.isChannelMissing('alpha')).toBe(true);
  });

  it('maps list and map wire values', () => {
    const a = new Adapter();
    const l = a.valueFromWasm({
      type: 'list',
      separator: 'comma',
      hasBrackets: true,
      contents: [{ type: 'boolean', value: true }],
    }) as SassList;
    expect(l).toBeInstanceOf(SassList);
    expect(l.separator).toBe(',');
    expect(l.hasBrackets).toBe(true);
    expect(l.asList.size).toBe(1);

    const m = a.valueFromWasm({
      type: 'map',
      entries: [
        {
          key: { type: 'string', text: 'k', quoted: false },
          value: {
            type: 'number',
            value: 1,
            numeratorUnits: [],
            denominatorUnits: [],
          },
        },
      ],
    }) as SassMap;
    expect(m).toBeInstanceOf(SassMap);
    expect(m.contents.size).toBe(1);
  });

  it('tracks argument lists with their id and compile context', () => {
    const a = new Adapter();
    const list = a.valueFromWasm({
      type: 'argumentList',
      id: 3,
      separator: 'comma',
      hasBrackets: false,
      contents: [],
      keywords: {},
    }) as SassArgumentList;
    expect(list).toBeInstanceOf(SassArgumentList);
    expect(list.id).toBe(3);
    expect(list.compileContext).toBe(a.compileContext);
  });

  it('maps function and mixin ids', () => {
    const a = new Adapter();
    const f = a.valueFromWasm({ type: 'function', id: 1 });
    expect(f).toBeInstanceOf(SassFunction);
    expect((f as SassFunction).id).toBe(1);

    const m = a.valueFromWasm({ type: 'mixin', id: 2 });
    expect(m).toBeInstanceOf(SassMixin);
    expect((m as SassMixin).id).toBe(2);
  });

  it('rejects hostFunction values coming from wasm', () => {
    const a = new Adapter();
    expect(() =>
      a.valueFromWasm({
        type: 'hostFunction',
        signature: 'f()',
        callback: () => {},
      }),
    ).toThrowError(/hostFunction values are only valid from JS to Rust/);
  });
});

describe('Adapter.valueToWasm', () => {
  it('maps value classes to wire values', () => {
    const a = new Adapter();
    expect(a.valueToWasm(new SassString('x', { quotes: false })).value).toEqual(
      {
        type: 'string',
        text: 'x',
        quoted: false,
      },
    );
    expect(
      a.valueToWasm(
        new SassNumber(3, { numeratorUnits: ['px'], denominatorUnits: ['s'] }),
      ).value,
    ).toEqual({
      type: 'number',
      value: 3,
      numeratorUnits: ['px'],
      denominatorUnits: ['s'],
    });
    expect(a.valueToWasm(sassNull).value).toEqual({ type: 'null' });
    expect(a.valueToWasm(sassTrue).value).toEqual({
      type: 'boolean',
      value: true,
    });
    expect(
      a.valueToWasm(
        new SassList([sassTrue], { separator: '/', brackets: false }),
      ).value,
    ).toEqual({
      type: 'list',
      separator: 'slash',
      hasBrackets: false,
      contents: [{ type: 'boolean', value: true }],
    });
  });

  it('maps colors to wire values', () => {
    const a = new Adapter();
    const wire = a.valueToWasm(
      new SassColor({ red: 255, green: 0, blue: 0, alpha: 1, space: 'rgb' }),
    ).value as Extract<WireValue, { type: 'color' }>;
    expect(wire.type).toBe('color');
    expect(wire.space).toBe('rgb');
    expect(wire.missing).toEqual([false, false, false, false]);
  });

  it('maps function references by id or signature', () => {
    const a = new Adapter();
    const byId = a.valueFromWasm({ type: 'function', id: 4 });
    expect(a.valueToWasm(byId).value).toEqual({ type: 'function', id: 4 });

    const host = new SassFunction('f($x)', () => sassNull);
    const wire = a.valueToWasm(host).value as Extract<
      WireValue,
      { type: 'hostFunction' }
    >;
    expect(wire.type).toBe('hostFunction');
    expect(wire.signature).toBe('f($x)');
  });

  it('throws on non-Value returns', () => {
    const a = new Adapter();
    expect(() => a.valueToWasm('not a value' as unknown as Value)).toThrowError(
      /is not a sass\.Value\./,
    );
  });
});

describe('Adapter cross-compilation ownership', () => {
  it('sends an argument list from another compilation by value (id 0)', () => {
    const a = new Adapter();
    const foreign = new Adapter().valueFromWasm({
      type: 'argumentList',
      id: 7,
      separator: 'comma',
      hasBrackets: false,
      contents: [
        { type: 'number', value: 1, numeratorUnits: [], denominatorUnits: [] },
      ],
      keywords: { k: { type: 'boolean', value: true } },
    }) as SassArgumentList;
    const wire = a.valueToWasm(foreign).value as Extract<
      WireValue,
      { type: 'argumentList' }
    >;
    expect(wire.id).toBe(0);
    expect(wire.contents).toEqual([
      { type: 'number', value: 1, numeratorUnits: [], denominatorUnits: [] },
    ]);
    expect(wire.keywords).toEqual({ k: { type: 'boolean', value: true } });
  });

  it('sends an argument list from the same compilation by id', () => {
    const a = new Adapter();
    const list = a.valueFromWasm({
      type: 'argumentList',
      id: 7,
      separator: 'comma',
      hasBrackets: false,
      contents: [],
      keywords: {},
    }) as SassArgumentList;
    expect(a.valueToWasm(list).value).toMatchObject({
      type: 'argumentList',
      id: 7,
    });
  });

  it('throws for a compiler function from another compilation', () => {
    const a = new Adapter();
    const foreign = new Adapter().valueFromWasm({ type: 'function', id: 4 });
    expect(() => a.valueToWasm(foreign)).toThrowError(
      /does not belong to this compilation/,
    );
  });

  it('throws for a compiler mixin from another compilation', () => {
    const a = new Adapter();
    const foreign = new Adapter().valueFromWasm({ type: 'mixin', id: 3 });
    expect(() => a.valueToWasm(foreign)).toThrowError(
      /does not belong to this compilation/,
    );
  });

  it('still sends same-compilation function and host functions', () => {
    const a = new Adapter();
    const byId = a.valueFromWasm({ type: 'function', id: 4 });
    expect(a.valueToWasm(byId).value).toEqual({ type: 'function', id: 4 });
    const host = new SassFunction('f($x)', () => sassNull);
    expect(a.valueToWasm(host).value).toMatchObject({ type: 'hostFunction' });
  });
});

describe('Adapter argument-list keyword-access tracking', () => {
  it('reports the ids of argument lists whose keywords were accessed', () => {
    const a = new Adapter();
    const list = a.valueFromWasm({
      type: 'argumentList',
      id: 5,
      separator: 'comma',
      hasBrackets: false,
      contents: [],
      keywords: {},
    }) as SassArgumentList;
    void list.keywords;
    const result = a.valueToWasm(list);
    expect(result.value).toEqual({
      type: 'argumentList',
      id: 5,
      separator: 'comma',
      hasBrackets: false,
      contents: [],
      keywords: {},
    });
    expect(result.accessedArgumentLists).toEqual([5]);
  });

  it('does not report argument lists whose keywords were not accessed', () => {
    const a = new Adapter();
    const list = a.valueFromWasm({
      type: 'argumentList',
      id: 5,
      separator: 'comma',
      hasBrackets: false,
      contents: [],
      keywords: {},
    }) as SassArgumentList;
    expect(a.valueToWasm(list).accessedArgumentLists).toEqual([]);
  });

  it('does not report host-constructed argument lists', () => {
    const a = new Adapter();
    const fresh = new SassArgumentList([], { kw: sassTrue }, ',');
    void fresh.keywords;
    expect(a.valueToWasm(fresh).accessedArgumentLists).toEqual([]);
  });
});

describe('Adapter.wrapFunction', () => {
  it('throws on Promise returns in sync mode', () => {
    const a = new Adapter();
    const wrapped = a.wrapFunction(async () => sassNull, true);
    expect(() => wrapped([])).toThrowError(
      "can't return a Promise for synchronous compile functions",
    );
  });

  it('awaits Promise returns in async mode', async () => {
    const a = new Adapter();
    const wrapped = a.wrapFunction(async () => sassNull, false);
    const result = await wrapped([]);
    expect(result.value).toEqual({ type: 'null' });
  });
});

describe('Adapter.wrapImporter', () => {
  it('passes canonicalize results raw and load a URL instance', () => {
    const a = new Adapter();
    const importer = {
      canonicalize: (url: string) =>
        url === 'theme' ? new URL('theme:index.scss') : null,
      load: (url: string | URL) =>
        String(url) === 'theme:index.scss'
          ? { contents: '$c: red;', syntax: 'scss' as const }
          : null,
    };
    const wrapped = a.wrapImporter(importer, true) as {
      canonicalize: (url: string, ctx: unknown) => unknown;
      load: (url: string) => unknown;
    };
    // canonicalize results pass through raw — Rust validates instanceof URL
    // and produces the Dart-exact error messages.
    expect(wrapped.canonicalize('theme', { fromImport: false })).toBeInstanceOf(
      URL,
    );
    expect(
      (wrapped.canonicalize('theme', { fromImport: false }) as URL).toString(),
    ).toBe('theme:index.scss');
    expect(wrapped.canonicalize('other', { fromImport: false })).toBeNull();
    // load receives a URL instance per the js-api-doc and returns the raw
    // result object (Rust validates contents/syntax).
    const result = wrapped.load('theme:index.scss') as {
      contents: string;
      syntax: string;
    };
    expect(result.contents).toBe('$c: red;');
    expect(result.syntax).toBe('scss');
    expect(wrapped.load('theme:other')).toBeNull();
  });

  it('passes findFileUrl results raw', () => {
    const a = new Adapter();
    const wrapped = a.wrapImporter(
      {
        findFileUrl: (url: string) =>
          url === 'theme' ? new URL('file:///theme/_index.scss') : null,
      },
      true,
    ) as { findFileUrl: (url: string, ctx: unknown) => unknown };
    expect(
      (wrapped.findFileUrl('theme', { fromImport: false }) as URL).toString(),
    ).toBe('file:///theme/_index.scss');
    expect(wrapped.findFileUrl('other', { fromImport: false })).toBeNull();
  });

  it('exposes containingUrl as a URL instance in the importer context', () => {
    const a = new Adapter();
    const contexts: Array<{ fromImport: boolean; containingUrl: URL | null }> =
      [];
    const wrapped = a.wrapImporter(
      {
        canonicalize: (url: string, ctx: { containingUrl: URL | null }) => {
          contexts.push(ctx);
          return null;
        },
        load: () => null,
      },
      true,
    ) as {
      canonicalize: (url: string, ctx: { containingUrl?: string }) => unknown;
    };
    wrapped.canonicalize('x', {
      fromImport: true,
      containingUrl: 'file:///a.scss',
    });
    wrapped.canonicalize('y', { fromImport: false });
    expect(contexts[0]).toEqual({
      fromImport: true,
      containingUrl: new URL('file:///a.scss'),
    });
    expect(contexts[0].containingUrl).toBeInstanceOf(URL);
    expect(contexts[1]).toEqual({ fromImport: false, containingUrl: null });
  });
});

describe('Adapter.wrapLogger', () => {
  it('maps a string deprecationType id to the full deprecations entry', () => {
    const a = new Adapter();
    let received: { deprecationType?: unknown } | undefined;
    const wrapped = a.wrapLogger(
      {
        warn: (message: string, options: { deprecationType?: unknown }) => {
          received = options;
        },
      },
      true,
    ) as { warn: (message: string, options: unknown) => void };
    wrapped.warn('oh no', {
      deprecation: true,
      deprecationType: 'new-global',
      stack: '',
    });
    expect(received!.deprecationType).toBe(deprecations['new-global']);
  });

  it('passes non-string deprecationType and plain warn options through unchanged', () => {
    const a = new Adapter();
    const original = { deprecation: false, stack: 'at foo' };
    let received: unknown;
    const wrapped = a.wrapLogger(
      {
        warn: (_message: string, options: unknown) => {
          received = options;
        },
      },
      true,
    ) as { warn: (message: string, options: unknown) => void };
    wrapped.warn('plain', original);
    expect(received).toEqual(original);
    expect(received).toBe(original);
  });

  it('maps a string span.url to a URL instance', () => {
    const a = new Adapter();
    let received: unknown;
    const wrapped = a.wrapLogger(
      {
        warn: (_message: string, options: unknown) => {
          received = options;
        },
      },
      true,
    ) as { warn: (message: string, options: unknown) => void };
    const span = {
      start: { offset: 0, line: 0, column: 0 },
      end: { offset: 11, line: 0, column: 11 },
      url: 'file:///style.scss',
      text: '@debug heck',
    };
    wrapped.warn('heck', { deprecation: false, span });
    const out = received as { span: { url?: unknown } };
    expect(out.span.url).toBeInstanceOf(URL);
    expect(String(out.span.url)).toBe('file:///style.scss');
  });

  it('passes a null/absent span.url through as undefined', () => {
    const a = new Adapter();
    let received: unknown;
    const wrapped = a.wrapLogger(
      {
        warn: (_message: string, options: unknown) => {
          received = options;
        },
      },
      true,
    ) as { warn: (message: string, options: unknown) => void };
    const span = {
      start: { offset: 0, line: 0, column: 0 },
      end: { offset: 3, line: 0, column: 3 },
      url: null,
      text: 'abc',
    };
    wrapped.warn('heck', {
      deprecation: true,
      deprecationType: 'new-global',
      span,
    });
    const out = received as {
      deprecationType?: unknown;
      span: { url?: unknown };
    };
    expect(out.span.url).toBeUndefined();
    expect(out.deprecationType).toBe(deprecations['new-global']);
  });

  it('maps a span.url in debug options too', () => {
    const a = new Adapter();
    let received: unknown;
    const wrapped = a.wrapLogger(
      {
        debug: (_message: string, options: unknown) => {
          received = options;
        },
      },
      true,
    ) as { debug: (message: string, options: unknown) => void };
    wrapped.debug('dbg', {
      span: { url: 'file:///style.scss', text: '@debug heck' },
    });
    expect((received as { span: { url?: unknown } }).span.url).toBeInstanceOf(
      URL,
    );
  });

  it('passes debug through unchanged', () => {
    const a = new Adapter();
    let received: unknown;
    const wrapped = a.wrapLogger(
      {
        debug: (_message: string, options: unknown) => {
          received = options;
        },
      },
      true,
    ) as { debug: (message: string, options: unknown) => void };
    wrapped.debug('dbg', {});
    expect(received).toEqual({});
  });

  it('awaits Promise returns in async mode', async () => {
    const a = new Adapter();
    let resolved = false;
    const wrapped = a.wrapLogger(
      {
        warn: async (message: string, options: Record<string, unknown>) => {
          await Promise.resolve();
          resolved = options.deprecationType === deprecations['new-global'];
        },
      },
      false,
    ) as { warn: (message: string, options: unknown) => Promise<void> };
    await wrapped.warn('oh no', { deprecationType: 'new-global' });
    expect(resolved).toBe(true);
  });

  it('returns undefined when the logger has neither warn nor debug', () => {
    expect(new Adapter().wrapLogger({}, true)).toBeUndefined();
  });
});
