import { describe, expect, it, vi } from 'vitest';
import * as sass from 'sass';

import { Logger, SassNumber, compileString } from '../index';
import { deprecations } from '../deprecations';
import { artifactsPresent } from './helpers';

describe.skipIf(!artifactsPresent)('logger parity with sass', () => {
  it('passes the @warn message to a custom logger', () => {
    const source = '@warn "hello";\na { b: c; }';
    const oursWarnings: unknown[][] = [];
    const theirsWarnings: unknown[][] = [];
    const ours = compileString(source, {
      logger: { warn: (...args: unknown[]) => oursWarnings.push(args) },
    });
    const theirs = sass.compileString(source, {
      logger: { warn: (...args: unknown[]) => theirsWarnings.push(args) },
    });
    expect(oursWarnings.length).toBe(1);
    expect(theirsWarnings.length).toBe(1);
    expect(oursWarnings[0][0] as string).toBe(theirsWarnings[0][0] as string);
    expect(ours.css).toBe(theirs.css);
  });

  it('delivers deprecationType as the full deprecations entry', () => {
    const source = 'a { $b: c !global; }';
    let oursType: unknown;
    let theirsType: unknown;
    compileString(source, {
      logger: {
        warn: (_message: string, options: { deprecationType?: unknown }) => {
          oursType = options.deprecationType;
        },
      },
    });
    sass.compileString(source, {
      logger: {
        warn: (_message: string, options: { deprecationType?: unknown }) => {
          theirsType = options.deprecationType;
        },
      },
    });
    expect(oursType).toBe(deprecations['new-global']);
    const ours = oursType as {
      id: string;
      status: string;
      description: string;
    };
    const theirs = theirsType as {
      id: string;
      status: string;
      description: string;
    };
    expect(ours.id).toBe(theirs.id);
    expect(ours.status).toBe(theirs.status);
    expect(ours.description).toBe(theirs.description);
  });

  it('passes a span to the logger for @debug', () => {
    const source = '@debug heck';
    const spans: unknown[] = [];
    compileString(source, {
      logger: {
        debug: (_message: string, options: { span?: unknown }) => {
          spans.push(options.span);
        },
      },
    });
    expect(spans.length).toBe(1);
    const span = spans[0] as {
      start: { line: number; column: number };
      end: { line: number; column: number };
      text: string;
    };
    expect(span.start.line).toBe(0);
    expect(span.start.column).toBe(0);
    expect(span.end.line).toBe(0);
    expect(span.end.column).toBe(11);
  });

  it('passes a span for a deprecation warning', () => {
    const source = '* > { --foo: bar }';
    let span: unknown;
    compileString(source, {
      logger: {
        warn: (_message: string, options: { span?: unknown }) => {
          span = options.span;
        },
      },
    });
    const s = span as { start: { column: number }; end: { column: number } };
    expect(s.start.column).toBe(0);
    expect(s.end.column).toBe(3);
  });

  it('passes no span for @warn', () => {
    const source = '@mixin foo {@warn heck} @include foo;';
    let span: unknown = 'unset';
    compileString(source, {
      logger: {
        warn: (_message: string, options: { span?: unknown }) => {
          span = options.span;
        },
      },
    });
    expect(span).toBeUndefined();
  });

  it('passes span.url as a URL instance', () => {
    const url = new URL('file:///style.scss');
    let oursUrl: unknown;
    let theirsUrl: unknown;
    const logger = {
      debug: (_message: string, options: { span?: { url?: unknown } }) => {
        oursUrl = options.span?.url;
      },
    };
    compileString('@debug heck', { url, logger });
    sass.compileString('@debug heck', {
      url,
      logger: {
        debug: (_message: string, options: { span?: { url?: unknown } }) => {
          theirsUrl = options.span?.url;
        },
      },
    });
    expect(oursUrl).toBeInstanceOf(URL);
    expect(String(oursUrl)).toBe(String(theirsUrl));
  });
});

describe.skipIf(!artifactsPresent)('Logger.silent bridge contract', () => {
  it('suppresses warnings and debug output', () => {
    const source = '@warn "hello";\n@debug "world";\na { b: c; }';
    expect(() =>
      compileString(source, { logger: Logger.silent }),
    ).not.toThrow();
    expect(sass.compileString(source, { logger: sass.Logger.silent }).css).toBe(
      compileString(source, { logger: Logger.silent }).css,
    );
  });
});

describe.skipIf(!artifactsPresent)('default no-logger output', () => {
  it('emits @warn and @debug to stderr by default', () => {
    const warnSpy = vi
      .spyOn(console, 'warn')
      .mockImplementation(() => undefined);
    const errorSpy = vi
      .spyOn(console, 'error')
      .mockImplementation(() => undefined);
    try {
      compileString('@warn heck; @debug hello;');
      expect(warnSpy).toHaveBeenCalledWith(
        'WARNING: heck\n    - 1:1  root stylesheet\n\n',
      );
      expect(errorSpy).toHaveBeenCalledWith('-:1 DEBUG: hello\n');
    } finally {
      warnSpy.mockRestore();
      errorSpy.mockRestore();
    }
  });

  it('does not fall back to the console when a logger provides callbacks', () => {
    const warnSpy = vi
      .spyOn(console, 'warn')
      .mockImplementation(() => undefined);
    const errorSpy = vi
      .spyOn(console, 'error')
      .mockImplementation(() => undefined);
    try {
      compileString('@warn heck;\n@debug hello;', {
        logger: { warn() {}, debug() {} },
      });
      expect(warnSpy).not.toHaveBeenCalled();
      expect(errorSpy).not.toHaveBeenCalled();
    } finally {
      warnSpy.mockRestore();
      errorSpy.mockRestore();
    }
  });
});

describe.skipIf(!artifactsPresent)(
  'alertColor / alertAscii bridge contract',
  () => {
    const hasAnsi = (s: string) => /\x1b\[/.test(s);

    it('colors the default-logger warning block when alertColor is true', () => {
      const warnSpy = vi
        .spyOn(console, 'warn')
        .mockImplementation(() => undefined);
      try {
        compileString('@warn heck;', { alertColor: true });
        expect(warnSpy).toHaveBeenCalledTimes(1);
        const text = String(warnSpy.mock.calls[0][0]);
        expect(text).toContain('Warning');
        expect(hasAnsi(text)).toBe(true);
      } finally {
        warnSpy.mockRestore();
      }
    });

    it('keeps the default warning block plain when alertColor is not set', () => {
      const warnSpy = vi
        .spyOn(console, 'warn')
        .mockImplementation(() => undefined);
      try {
        compileString('@warn heck;');
        const text = String(warnSpy.mock.calls[0][0]);
        expect(text).toBe('WARNING: heck\n    - 1:1  root stylesheet\n\n');
        expect(hasAnsi(text)).toBe(false);
      } finally {
        warnSpy.mockRestore();
      }
    });

    it('still passes a plain message to a user warn with alertColor true', () => {
      const messages: string[] = [];
      compileString('@warn heck;', {
        alertColor: true,
        logger: { warn: (m: string) => messages.push(m) },
      });
      expect(messages).toEqual(['heck']);
    });

    it('colors the thrown error message with alertColor true', () => {
      let plain: string | undefined;
      let colored: string | undefined;
      try {
        compileString('a { b: ; }');
      } catch (e) {
        plain = (e as Error).message;
      }
      try {
        compileString('a { b: ; }', { alertColor: true });
      } catch (e) {
        colored = (e as Error).message;
      }
      expect(plain).toBeDefined();
      expect(colored).toBeDefined();
      expect(hasAnsi(colored!)).toBe(true);
      expect(hasAnsi(plain!)).toBe(false);
    });

    it('uses ASCII frame glyphs with alertAscii true', () => {
      let msg: string | undefined;
      try {
        compileString('a { b: ; }', { alertColor: true, alertAscii: true });
      } catch (e) {
        msg = (e as Error).message;
      }
      expect(msg).not.toContain('\u2502');
      const plain = msg!.replace(/\x1b\[[0-9;]*m/g, '');
      expect(plain).toContain('\n1 | a { b: ; }');
    });
  },
);

describe.skipIf(!artifactsPresent)('host-side deprecations', () => {
  const colorDepSource = 'a { b: fn(red); }';
  const colorDepFn = {
    'fn($color)': (args: import('../value/index').Value[]) =>
      new SassNumber(args[0].assertColor().red),
  };

  it('emits a warning when not silenced (parity)', () => {
    const warnSpy = vi
      .spyOn(console, 'warn')
      .mockImplementation(() => undefined);
    try {
      compileString(colorDepSource, { functions: colorDepFn });
      expect(warnSpy).toHaveBeenCalledWith(
        expect.stringContaining('color-4-api'),
      );
    } finally {
      warnSpy.mockRestore();
    }
  });

  it('emits no warning when silenced in the current compilation', () => {
    const warnSpy = vi
      .spyOn(console, 'warn')
      .mockImplementation(() => undefined);
    try {
      compileString(colorDepSource, {
        silenceDeprecations: ['color-4-api'],
        functions: colorDepFn,
      });
      expect(warnSpy).not.toHaveBeenCalled();
    } finally {
      warnSpy.mockRestore();
    }
  });

  it('throws when made fatal in the current compilation', () => {
    expect(() =>
      compileString(colorDepSource, {
        fatalDeprecations: ['color-4-api'],
        functions: colorDepFn,
      }),
    ).toThrow();
  });

  it('matches the sass deprecation warning text', () => {
    const calls: string[] = [];
    const warnSpy = vi
      .spyOn(console, 'warn')
      .mockImplementation((m: string) => calls.push(String(m)));
    try {
      compileString(colorDepSource, { functions: colorDepFn });
    } finally {
      warnSpy.mockRestore();
    }
    expect(calls).toHaveLength(1);
    expect(calls[0]).toBe(
      'Deprecation [color-4-api]: `red` is deprecated, use `channel` instead.\n' +
        'More info: https://sass-lang.com/d/color-4-api',
    );
  });
});
