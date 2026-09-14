// Perf/parity benchmark suites, run manually (not in `npm run test:js`):
//
//   npm run test:bench            # RUN_BENCH=1 vitest run js/src/wasm/perf.test.ts
//
// These replaced the standalone rust-sass-wasm/{test.js,bench-ab.js,
// bench-alloc.js,bench-bootstrap.js} scripts. Everything goes through the
// public API over the real filesystem (the shim injects the node:fs delegate;
// no VirtualIo) and, for parity, the official `sass` (Dart) and `sass-embedded`
// packages from devDependencies. No wall-clock thresholds — correctness is
// asserted as byte equality of the emitted CSS; timing is reported.
//
// The 1.87MB huge.scss needs the larger V8 stack the old scripts set via
// `node --stack_size=2048`; vitest.config.mts applies it to bench workers.
//
// Requires `npm run build:rust` (pkg-sync/pkg-async) and, for the bootstrap
// suite, the `bootstrap-main` submodule checkout at the repository root.
// `huge.scss` is the tracked `bench/huge.scss` workload (`huge10` is generated
// from it at bench time: 10× concatenation, see docs/CONTRIBUTING.md).
// The `sass-embedded-rust` engine additionally requires `npm run build` in
// `embedded-host-node-rust` (assembled dist + local platform package).

import * as fs from 'node:fs';
import * as p from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import {
  compile,
  compileAsync,
  compileString,
  compileStringAsync,
  Logger,
} from '../index';
import { artifactsPresent, wrapperPresent } from './helpers';

// Only run under `npm run test:bench`; `npm run test:js` skips the suites.
const benchRun = process.env.RUN_BENCH === '1';

// Package root (rust-sass-wasm): this file sits in js/src/wasm/.
const root = p.resolve(
  p.dirname(fileURLToPath(import.meta.url)),
  '..',
  '..',
  '..',
);
const hugePath = p.join(root, '..', 'bench', 'huge.scss');
const bootstrapDir = p.join(root, '..', 'bootstrap-main', 'scss');
const bootstrapEntry = p.join(bootstrapDir, 'bootstrap.scss');

const haveHuge = fs.existsSync(hugePath);
const haveBootstrap = fs.existsSync(bootstrapEntry);

const median = (xs: number[]): number => {
  const sorted = [...xs].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)];
};

const silent = (logger: typeof Logger): { logger: typeof Logger } => ({
  logger: logger.silent,
});

function report(title: string, rows: Array<[string, number]>): void {
  const line = rows
    .map(([label, ms]) => `${label} ${ms.toFixed(0).padStart(5)}ms`)
    .join('   ');
  console.log(`[perf] ${title}: ${line}`);
}

// sass / sass-embedded are CommonJS packages; load them lazily so the default
// `npm run test:js` run never touches them.
const oracleCache = new Map<string, any>();
async function oracle(
  name: 'sass' | 'sass-embedded' | 'sass-embedded-rust',
): Promise<any> {
  let api = oracleCache.get(name);
  if (api === undefined) {
    const mod: any = await import(name);
    api = mod.compileString ? mod : mod.default;
    oracleCache.set(name, api);
  }
  return api;
}

describe.skipIf(!benchRun || !artifactsPresent || !haveHuge)(
  'huge.scss',
  () => {
    const huge = fs.readFileSync(hugePath, 'utf8');
    const RUNS = 5;

    it('produces byte-identical CSS across the sync and async wasm builds', async () => {
      compileString(huge, silent(Logger)); // warm both artifacts once
      await compileStringAsync(huge, silent(Logger));

      let syncCss = '';
      let asyncCss = '';
      const syncMs: number[] = [];
      const asyncMs: number[] = [];
      for (let i = 0; i < RUNS; i++) {
        let t = performance.now();
        syncCss = compileString(huge, silent(Logger)).css;
        syncMs.push(performance.now() - t);

        t = performance.now();
        asyncCss = (await compileStringAsync(huge, silent(Logger))).css;
        asyncMs.push(performance.now() - t);
      }

      expect(asyncCss).toBe(syncCss);
      report('huge.scss sync vs async', [
        ['sync', median(syncMs)],
        ['async', median(asyncMs)],
      ]);
    });

    it('matches sass and sass-embedded byte-for-byte', async () => {
      const sass = await oracle('sass');
      const embedded = await oracle('sass-embedded');
      const sassSilent = { logger: sass.Logger.silent };
      const embeddedSilent = { logger: embedded.Logger.silent };

      compileString(huge, silent(Logger));
      await compileStringAsync(huge, silent(Logger));
      sass.compileString(huge, sassSilent); // warm all engines
      await embedded.compileStringAsync(huge, embeddedSilent);

      const rows: Array<[string, number]> = [];
      const samples = new Map<string, number[]>();
      const record = (label: string, ms: number) => {
        if (!samples.has(label)) samples.set(label, []);
        samples.get(label)!.push(ms);
      };

      let rustCss = '';
      let rustAsyncCss = '';
      let dartCss = '';
      let embeddedCss = '';
      for (let i = 0; i < RUNS; i++) {
        let t = performance.now();
        rustCss = compileString(huge, silent(Logger)).css;
        record('rust-sync', performance.now() - t);

        t = performance.now();
        rustAsyncCss = (await compileStringAsync(huge, silent(Logger))).css;
        record('rust-async', performance.now() - t);

        t = performance.now();
        dartCss = sass.compileString(huge, sassSilent).css;
        record('sass', performance.now() - t);

        t = performance.now();
        embeddedCss = (await embedded.compileStringAsync(huge, embeddedSilent))
          .css;
        record('sass-embedded', performance.now() - t);
      }

      expect(rustCss).toBe(rustAsyncCss);
      expect(rustCss).toBe(dartCss);
      expect(rustCss).toBe(embeddedCss);
      for (const [label, ms] of samples) rows.push([label, median(ms)]);
      report('huge.scss rust vs sass vs sass-embedded', rows);
    });

    it.skipIf(!wrapperPresent)(
      'matches sass-embedded-rust byte-for-byte',
      async () => {
        const wrapper = await oracle('sass-embedded-rust');
        const wrapperSilent = { logger: wrapper.Logger.silent };

        compileString(huge, silent(Logger)); // warm
        wrapper.compileString(huge, wrapperSilent);

        let rustCss = '';
        let wrapperCss = '';
        let wrapperAsyncCss = '';
        const syncMs: number[] = [];
        const wrapperMs: number[] = [];
        const wrapperAsyncMs: number[] = [];
        for (let i = 0; i < RUNS; i++) {
          let t = performance.now();
          rustCss = compileString(huge, silent(Logger)).css;
          syncMs.push(performance.now() - t);

          t = performance.now();
          wrapperCss = wrapper.compileString(huge, wrapperSilent).css;
          wrapperMs.push(performance.now() - t);

          t = performance.now();
          wrapperAsyncCss = (
            await wrapper.compileStringAsync(huge, wrapperSilent)
          ).css;
          wrapperAsyncMs.push(performance.now() - t);
        }

        expect(wrapperCss).toBe(rustCss);
        expect(wrapperAsyncCss).toBe(rustCss);
        report('huge.scss rust vs sass-embedded-rust', [
          ['rust-sync', median(syncMs)],
          ['wrapper', median(wrapperMs)],
          ['wrapper-async', median(wrapperAsyncMs)],
        ]);
      },
    );
  },
);

describe.skipIf(!benchRun || !artifactsPresent || !haveBootstrap)(
  'bootstrap (real fs, loadPaths)',
  () => {
    const RUNS = 3;

    it('compiles over the real filesystem and matches the oracles byte-for-byte', async () => {
      const sass = await oracle('sass');
      const embedded = await oracle('sass-embedded');
      const loadPaths = [bootstrapDir];

      const sassOpts = { loadPaths, logger: sass.Logger.silent };
      const embeddedOpts = { loadPaths, logger: embedded.Logger.silent };
      const rustOpts = { loadPaths, ...silent(Logger) };

      // Warm every engine once.
      compile(bootstrapEntry, rustOpts);
      await compileAsync(bootstrapEntry, rustOpts);
      sass.compile(bootstrapEntry, sassOpts);
      await embedded.compileAsync(bootstrapEntry, embeddedOpts);

      const rows: Array<[string, number]> = [];
      const samples = new Map<string, number[]>();
      const record = (label: string, ms: number) => {
        if (!samples.has(label)) samples.set(label, []);
        samples.get(label)!.push(ms);
      };

      let rustCss = '';
      let rustAsyncCss = '';
      let dartCss = '';
      let embeddedCss = '';
      for (let i = 0; i < RUNS; i++) {
        let t = performance.now();
        rustCss = compile(bootstrapEntry, rustOpts).css;
        record('rust-sync', performance.now() - t);

        t = performance.now();
        rustAsyncCss = (await compileAsync(bootstrapEntry, rustOpts)).css;
        record('rust-async', performance.now() - t);

        t = performance.now();
        dartCss = sass.compile(bootstrapEntry, sassOpts).css;
        record('sass', performance.now() - t);

        t = performance.now();
        embeddedCss = (
          await embedded.compileAsync(bootstrapEntry, embeddedOpts)
        ).css;
        record('sass-embedded', performance.now() - t);
      }

      expect(rustCss).toBe(rustAsyncCss);
      expect(rustCss).toBe(dartCss);
      expect(rustCss).toBe(embeddedCss);
      for (const [label, ms] of samples) rows.push([label, median(ms)]);
      report('bootstrap.scss rust vs sass vs sass-embedded', rows);
    });

    it.skipIf(!wrapperPresent)(
      'matches sass-embedded-rust byte-for-byte',
      async () => {
        const wrapper = await oracle('sass-embedded-rust');
        const wrapperOpts = { loadPaths, logger: wrapper.Logger.silent };
        const rustOpts = { loadPaths, ...silent(Logger) };

        compile(bootstrapEntry, rustOpts); // warm
        wrapper.compile(bootstrapEntry, wrapperOpts);

        let rustCss = '';
        let wrapperCss = '';
        let wrapperAsyncCss = '';
        const syncMs: number[] = [];
        const wrapperMs: number[] = [];
        const wrapperAsyncMs: number[] = [];
        for (let i = 0; i < RUNS; i++) {
          let t = performance.now();
          rustCss = compile(bootstrapEntry, rustOpts).css;
          syncMs.push(performance.now() - t);

          t = performance.now();
          wrapperCss = wrapper.compile(bootstrapEntry, wrapperOpts).css;
          wrapperMs.push(performance.now() - t);

          t = performance.now();
          wrapperAsyncCss = (
            await wrapper.compileAsync(bootstrapEntry, wrapperOpts)
          ).css;
          wrapperAsyncMs.push(performance.now() - t);
        }

        expect(wrapperCss).toBe(rustCss);
        expect(wrapperAsyncCss).toBe(rustCss);
        report('bootstrap.scss rust vs sass-embedded-rust', [
          ['rust-sync', median(syncMs)],
          ['wrapper', median(wrapperMs)],
          ['wrapper-async', median(wrapperAsyncMs)],
        ]);
      },
    );
  },
);
