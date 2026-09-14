#!/usr/bin/env node
// The `sass` CLI entry point (compiled to `dist/bin/sass.js`). Mirrors the
// rust-sass-cli main.rs flow: flag parse → usage errors exit 64 → compile_all →
// the max exit code (0 success, 65 Sass error, 66 I/O error).
//
// dart-source: bin/sass.dart

import { compileAll } from '../cli/compile';
import { HELP_TEXT, VERSION_TEXT, UsageError, parseArgs } from '../cli/args';

/** Runs the CLI against [argv] (excluding node + script) and returns the exit code. */
export function runCli(argv: string[]): number {
  if (argv.includes('--help') || argv.includes('-h')) {
    process.stdout.write(HELP_TEXT);
    return 64;
  }
  if (argv.includes('--version')) {
    process.stdout.write(`${VERSION_TEXT}\n`);
    return 0;
  }
  let opts;
  try {
    opts = parseArgs(argv);
  } catch (e) {
    if (e instanceof UsageError) {
      process.stdout.write(`${e.message}\n\n`);
      process.stdout.write(HELP_TEXT);
      return 64;
    }
    throw e;
  }
  return compileAll(opts);
}

// `process.exitCode`, not `process.exit()`: exit() would terminate before
// pending async stdout pipe writes flush, truncating large CSS outputs (the
// sass-spec runner spawns this bin with piped stdio).
process.exitCode = runCli(process.argv.slice(2));
