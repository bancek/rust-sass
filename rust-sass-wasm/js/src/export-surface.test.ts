import * as fs from 'node:fs';
import * as p from 'node:path';
import { describe, expect, it } from 'vitest';

import * as sass from './index';

// The full modern export surface from docs/ref/wasm.md ("Node CLI and export surface")
// `ListSeparator` and the option/result interfaces). js-api-spec loads this
// package via `require(<dir>)`, so every key must resolve at runtime.
describe('export surface', () => {
  it('exposes every §16.4 runtime export', () => {
    // Type-only names (e.g. `ListSeparator`) are elided in the compiled `dist`
    // but may surface as `undefined`-valued keys under vite's esbuild transform;
    // filter those out so the assertion matches the resolvable package.
    const keys = Object.keys(sass)
      .filter((key) => sass[key as keyof typeof sass] !== undefined)
      .sort();
    expect(keys).toEqual(
      [
        'AsyncCompiler',
        'CalculationInterpolation',
        'CalculationOperation',
        'Compiler',
        'Exception',
        'Logger',
        'NodePackageImporter',
        'SassArgumentList',
        'SassBoolean',
        'SassCalculation',
        'SassColor',
        'SassFunction',
        'SassList',
        'SassMap',
        'SassMixin',
        'SassNumber',
        'SassString',
        'Value',
        'Version',
        'compile',
        'compileAsync',
        'compileString',
        'compileStringAsync',
        'deprecations',
        'info',
        'initAsyncCompiler',
        'initCompiler',
        'sassFalse',
        'sassNull',
        'sassTrue',
      ].sort(),
    );
  });

  it('exports Logger.silent and a working deprecations map', () => {
    expect(sass.Logger.silent).toBeDefined();
    expect(typeof sass.Logger.silent.warn).toBe('function');
    expect(Object.keys(sass.deprecations).length).toBeGreaterThanOrEqual(29);
  });

  it('exposes Version.parse', () => {
    expect(typeof sass.Version.parse).toBe('function');
  });
});

const distMjs = p.join(__dirname, '..', 'dist', 'index.mjs');
const distCjs = p.join(__dirname, '..', 'dist', 'index.js');
const distPresent = fs.existsSync(distMjs) && fs.existsSync(distCjs);

// The `exports` map routes `import` to `index.mjs` and `require` to
// `index.js` (docs/ref/wasm.md, "Node CLI and export surface"). Both must expose the same runtime surface.
describe.skipIf(!distPresent)('ESM entry (dist/index.mjs)', () => {
  it('re-exports the same named surface as the CJS entry', async () => {
    const mjs = await import(distMjs);
    const cjs = await import(distCjs);
    // Filter `default` (CJS-ESM interop artifact) and type-only keys.
    const keysOf = (mod: Record<string, unknown>) =>
      Object.keys(mod)
        .filter((key) => key !== 'default' && mod[key] !== undefined)
        .sort();
    expect(keysOf(mjs as Record<string, unknown>)).toEqual(
      keysOf(cjs as Record<string, unknown>),
    );
    expect(
      keysOf(mjs as Record<string, unknown>).length,
    ).toBeGreaterThanOrEqual(29);
  });
});
