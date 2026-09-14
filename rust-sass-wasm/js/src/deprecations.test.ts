import * as fs from 'node:fs';
import * as path from 'node:path';

import { afterEach, describe, expect, it, vi } from 'vitest';

import { Version } from './version';
import {
  activeDeprecationOptions,
  deprecations,
  warnForHostSideDeprecation,
} from './deprecations';

// vitest's cwd is `rust-sass-wasm/`; the yaml lives in the `sass` submodule
// checkout at the repository root (`sass/spec/`).
const yamlPath = path.resolve(
  process.cwd(),
  '..',
  'sass',
  'spec',
  'deprecations.yaml',
);

// Parses the specific deprecations.yaml shape (a 2-level document with
// description / dart-sass.status / dart-sass.deprecated / dart-sass.obsolete).
function parseYaml(file: string): Map<
  string,
  {
    description: string | null;
    status: string;
    deprecated?: string;
    obsolete?: string;
  }
> {
  const out = new Map<
    string,
    {
      description: string | null;
      status: string;
      deprecated?: string;
      obsolete?: string;
    }
  >();
  let current: {
    description: string | null;
    status: string;
    deprecated?: string;
    obsolete?: string;
  } | null = null;
  let currentId: string | null = null;
  let inDartSass = false;

  const unquote = (raw: string): string => {
    const trimmed = raw.trim();
    if (trimmed.startsWith('"') && trimmed.endsWith('"')) {
      return trimmed.slice(1, -1).replace(/\\"/g, '"');
    }
    if (trimmed.startsWith("'") && trimmed.endsWith("'")) {
      return trimmed.slice(1, -1).replace(/''/g, "'");
    }
    return trimmed;
  };

  for (const line of file.split('\n')) {
    if (line.trim().startsWith('#')) continue;
    const topLevel = line.match(/^([a-z0-9-]+):\s*$/);
    if (topLevel) {
      currentId = topLevel[1];
      current = { description: null, status: 'active' };
      out.set(currentId, current);
      inDartSass = false;
      continue;
    }
    const nested = line.match(/^ {2}([a-zA-Z0-9-]+):(.*)$/);
    if (nested && current) {
      if (nested[1] === 'dart-sass') {
        inDartSass = true;
      } else if (nested[1] === 'description') {
        current.description = unquote(nested[2]);
      }
      continue;
    }
    const leaf = line.match(/^ {4}([a-zA-Z0-9-]+):(.*)$/);
    if (leaf && current && inDartSass) {
      if (leaf[1] === 'status') {
        current.status = leaf[2].trim();
      } else if (leaf[1] === 'deprecated') {
        current.deprecated = leaf[2].trim();
      } else if (leaf[1] === 'obsolete') {
        current.obsolete = leaf[2].trim();
      }
      continue;
    }
  }
  return out;
}

describe('deprecations map', () => {
  it('matches the official sass/spec/deprecations.yaml exactly', () => {
    const yaml = parseYaml(fs.readFileSync(yamlPath, 'utf8'));
    // The map covers every yaml entry plus the special `user-authored` entry
    // (which the official `sass` package also exports).
    const ours = Object.keys(deprecations).sort();
    const theirs = [...yaml.keys(), 'user-authored'].sort();
    expect(ours).toEqual(theirs);

    for (const [id, yamlEntry] of yaml) {
      const entry = deprecations[id];
      expect(entry.id).toBe(id);
      expect(entry.status).toBe(yamlEntry.status);
      expect(entry.description).toBe(yamlEntry.description);
      expect(entry.deprecatedIn).toEqual(
        yamlEntry.deprecated
          ? new Version(...splitVersion(yamlEntry.deprecated))
          : null,
      );
      expect(entry.obsoleteIn).toEqual(
        yamlEntry.obsolete
          ? new Version(...splitVersion(yamlEntry.obsolete))
          : null,
      );
    }
  });

  it('includes the special user-authored deprecation', () => {
    expect(deprecations['user-authored']).toEqual({
      id: 'user-authored',
      status: 'user',
      description: null,
      deprecatedIn: null,
      obsoleteIn: null,
    });
  });

  it('uses real Version instances for deprecatedIn/obsoleteIn', () => {
    expect(deprecations['new-global'].deprecatedIn).toBeInstanceOf(Version);
    expect(deprecations['css-function-mixin'].obsoleteIn).toBeInstanceOf(
      Version,
    );
    expect(deprecations['css-function-mixin'].deprecatedIn).toBeInstanceOf(
      Version,
    );
  });

  it('contains the entries the value classes need', () => {
    expect(deprecations['color-4-api'].status).toBe('active');
    expect(deprecations['null-alpha'].status).toBe('active');
  });
});

describe('warnForHostSideDeprecation', () => {
  afterEach(() => vi.restoreAllMocks());

  it('emits a prefixed console warning', () => {
    const spy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    warnForHostSideDeprecation('oh no', deprecations['color-4-api']);
    expect(spy).toHaveBeenCalledWith('Deprecation [color-4-api]: oh no');
  });

  it('is silenced by the passed silenceDeprecations option', () => {
    const spy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    warnForHostSideDeprecation('oh no', deprecations['color-4-api'], {
      silenceDeprecations: ['color-4-api'],
    });
    expect(spy).not.toHaveBeenCalled();
  });

  it('throws when made fatal by the passed fatalDeprecations option', () => {
    const spy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    expect(() =>
      warnForHostSideDeprecation('oh no', deprecations['color-4-api'], {
        fatalDeprecations: ['color-4-api'],
      }),
    ).toThrow('Deprecation [color-4-api]: oh no');
    expect(spy).not.toHaveBeenCalled();
  });

  it('is silenced when the id is a Deprecation object', () => {
    const spy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    warnForHostSideDeprecation('oh no', deprecations['color-4-api'], {
      silenceDeprecations: [deprecations['color-4-api']],
    });
    expect(spy).not.toHaveBeenCalled();
  });

  it('consults active compilations when no options are passed', () => {
    const key = Symbol();
    activeDeprecationOptions.set(key, {
      silenceDeprecations: ['color-4-api'],
    });
    try {
      const spy = vi.spyOn(console, 'warn').mockImplementation(() => {});
      warnForHostSideDeprecation('oh no', deprecations['color-4-api']);
      expect(spy).not.toHaveBeenCalled();
    } finally {
      activeDeprecationOptions.delete(key);
    }
  });

  it('is fatal across all active compilations', () => {
    const key = Symbol();
    activeDeprecationOptions.set(key, {
      fatalDeprecations: ['color-4-api'],
    });
    try {
      expect(() =>
        warnForHostSideDeprecation('oh no', deprecations['color-4-api']),
      ).toThrow();
    } finally {
      activeDeprecationOptions.delete(key);
    }
  });
});

function splitVersion(version: string): [number, number, number] {
  const [major, minor, patch] = version.split('.').map(Number);
  return [major, minor, patch];
}
