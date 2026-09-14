// Resolve/assert gate (runs first): the assembled wrapper must resolve the
// LOCAL platform package to a real rust-sass binary. Without this, stock
// compiler-path.js would silently fall back to Dart Sass (same version,
// byte-identical output) and every test below would pass against the WRONG
// engine — a silent false-green. Fail loudly instead, and log the binary.
import {createRequire} from 'node:module';
import * as fs from 'node:fs';
import {fileURLToPath} from 'node:url';
import * as p from 'node:path';
import {triple as platform} from '../detect-musl.mjs';

const require = createRequire(import.meta.url);

const triple = platform();
const platformPkg = `sass-embedded-rust-${triple}`;
// Stock resolution probes `dart-sass/sass` (+ `.bat` on Windows) — mirror it
// exactly so the gate exercises the same path the wrapper uses at runtime.
const entry =
  process.platform === 'win32' ? 'dart-sass/sass.bat' : 'dart-sass/sass';
let binary;
try {
  binary = require.resolve(`${platformPkg}/${entry}`);
} catch {
  throw new Error(
    `platform package unresolvable: ${platformPkg} — run npm run build first`,
  );
}
if (!binary.endsWith(`platform/${triple}/${entry}`)) {
  throw new Error(
    `resolved binary is outside our platform package ` +
      `(expected …/platform/${triple}/${entry}): ${binary}`,
  );
}
if (!fs.existsSync(binary)) {
  throw new Error(`resolved binary missing: ${binary}`);
}
const {compilerCommand} = require('../dist/lib/src/compiler-path.js');
if (!compilerCommand[0].endsWith(`platform/${triple}/${entry}`)) {
  throw new Error(
    `wrapper compilerCommand points at the wrong engine: ${compilerCommand[0]}`,
  );
}
console.log(`[gate] compiler binary: ${binary}`);
console.log(`[gate] compilerCommand: ${JSON.stringify(compilerCommand)}`);

export const RUST_SASS_BINARY = binary;
