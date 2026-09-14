import { defineConfig } from 'vitest/config';

// Bench suites (`js/src/wasm/perf.test.ts`) only run when RUN_BENCH=1 (see the
// `test:bench` script); they compile the 1.87MB huge.scss / bootstrap, which
// needs the larger V8 stack the old standalone scripts passed via
// `node --stack_size=2048`, and are serialized to de-noise timings.
const bench = process.env.RUN_BENCH === '1';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['js/src/**/*.test.ts'],
    ...(bench
      ? {
          execArgv: ['--stack_size=2048'],
          fileParallelism: false,
          testTimeout: 600_000,
        }
      : {}),
  },
});
