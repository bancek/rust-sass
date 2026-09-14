// Coexistence: the genuine `sass-embedded` host (Dart binary) and this
// package (rust-sass binary) in ONE process must not interfere — the whole
// reason the release strategy assembles host code into our namespace instead
// of monkey-patching the shared module. Also doubles as an API-level
// Dart-parity check: both engines must emit byte-identical CSS.
//
// Both binaries are resolved as platform-package subpaths (data packages have
// no `exports` map, so deep resolution is allowed) — never via host
// internals, which are not exported.
import './gate.mjs';
import {createRequire} from 'node:module';
import {triple as platform} from '../detect-musl.mjs';

import * as ours from '../dist/lib/index.mjs';
import * as genuine from 'sass-embedded';

const require = createRequire(import.meta.url);

const triple = platform();
const entry =
  process.platform === 'win32' ? 'dart-sass/sass.bat' : 'dart-sass/sass';
const oursBinary = require.resolve(`sass-embedded-rust-${triple}/${entry}`);
const genuineBinary = require.resolve(`sass-embedded-${triple}/${entry}`);
console.log(`[coexist] genuine binary: ${genuineBinary}`);
console.log(`[coexist] ours binary:    ${oursBinary}`);

let failed = 0;
function check(name, cond) {
  console.log(`${cond ? 'PASS' : 'FAIL'} ${name}`);
  if (!cond) failed++;
}

check('binaries differ per engine', oursBinary !== genuineBinary);
check(
  'our binary is the local platform build',
  oursBinary.endsWith(`platform/${triple}/${entry}`),
);
check(
  'genuine binary is the Dart compiler',
  genuineBinary.includes(`sass-embedded-${triple}`) &&
    !genuineBinary.includes(`platform/${triple}`),
);

const input = 'a {b: 1px + 2px}';
const expected = 'a {\n  b: 3px;\n}';
const dartCss = genuine.compileString(input).css;
const rustCss = ours.compileString(input).css;
check('genuine host compiles', dartCss === expected);
check('our host compiles', rustCss === expected);
check('byte-identical across engines', dartCss === rustCss);

if (failed > 0) {
  console.error(`[coexist] ${failed} check(s) failed`);
  process.exitCode = 1;
}
