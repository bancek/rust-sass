// Build-time assembly for the sass-embedded-rust npm package.
//
// Assembles (never vendors) the genuine embedded-host-node source with a
// one-line `compiler-module.ts` template swap, so the stock resolution logic
// finds OUR platform packages (`sass-embedded-rust-<platform>-<arch>`) while
// sharing zero modules with an app's own `sass-embedded` install.
//
// Steps: compiler binary (dev: `cargo build --release` for the host; CI:
// prebuilt per-platform binaries for all triples) → npm install + compile in
// ../embedded-host-node (in place; its tree stays pristine — dist/ is
// gitignored) → copy dist/ here → apply the template swap → assemble the
// public typings (tsc emits declarations to _types/, never into dist/) →
// assemble the local platform package(s) + link the host one into
// node_modules (same mechanism as registry publish, unpublished locally).
//
// Usage:
//   npm run build                     (host platform, local `cargo build`)
//   npm run build -- --platforms=all  (all 8 triples, needs RUST_SASS_DIST
//                                     pointed at a dir of prebuilt binaries —
//                                     the npm.yml flow; see below)
//
// Prebuilt-binary layout: RUST_SASS_DIST holds one dir per triple
// (`rust-sass-<triple>/`, the CI matrix artifact names — each artifact IS its
// bare binary file), holding that triple's bare compiler binary
// (`rust-sass-<triple>`, `.exe` on Windows). Every triple ships its own
// genuine native binary — musl triples are real musl-linked builds, never
// relabeled gnu binaries — so the ELF-based musl detection in
// detect-musl.mjs (mirroring the host's compiler-module.ts) routes each host
// to its own package.

import {execFileSync} from 'node:child_process';
import * as fs from 'node:fs';
import {createRequire} from 'node:module';
import * as p from 'node:path';
import {fileURLToPath} from 'node:url';
import {triple} from './detect-musl.mjs';

const require = createRequire(import.meta.url);

const pkgDir = p.dirname(fileURLToPath(import.meta.url));
const repoRoot = p.resolve(pkgDir, '..');
const hostDir = p.resolve(repoRoot, 'embedded-host-node');
const distDir = p.resolve(pkgDir, 'dist');

function sh(cmd, args, cwd) {
  execFileSync(cmd, args, {cwd, stdio: 'inherit'});
}

function platformPackageName() {
  return `sass-embedded-rust-${triple()}`;
}

// The 8 shipped triples with their npm manifest coordinates. musl manifests
// say `os: linux` (npm has no musl OS value) plus `libc: musl`, matching the
// upstream `sass-embedded-linux-musl-*` shape. Android/riscv/armv7 triples
// arrive with full upstream parity later.
const TRIPLES = [
  {triple: 'darwin-arm64', os: 'darwin', cpu: 'arm64'},
  {triple: 'darwin-x64', os: 'darwin', cpu: 'x64'},
  {triple: 'linux-x64', os: 'linux', cpu: 'x64'},
  {triple: 'linux-arm64', os: 'linux', cpu: 'arm64'},
  {triple: 'linux-musl-x64', os: 'linux', libc: 'musl', cpu: 'x64'},
  {triple: 'linux-musl-arm64', os: 'linux', libc: 'musl', cpu: 'arm64'},
  {triple: 'win32-x64', os: 'win32', cpu: 'x64'},
  {triple: 'win32-arm64', os: 'win32', cpu: 'arm64'},
];

const modeAll = process.argv.includes('--platforms=all');
const distDir0 = process.env.RUST_SASS_DIST;
if (modeAll && !distDir0) {
  throw new Error('`--platforms=all` needs RUST_SASS_DIST pointed at the prebuilt binaries dir');
}

// Locate the compiler binary for one triple: the bare `rust-sass-<triple>`
// file (`.exe` on Windows) inside its `rust-sass-<triple>/` artifact dir.
// The error lists the dir contents — no guessing when the layout surprises
// us.
function findBinary(t) {
  const base = `rust-sass-${t.triple}${t.triple.startsWith('win32') ? '.exe' : ''}`;
  const file = p.join(distDir0, base, base);
  if (!fs.existsSync(file)) {
    throw new Error(
      `no prebuilt binary ${base}/${base} in ${distDir0} (contents: ${fs.readdirSync(distDir0).join(', ')})`,
    );
  }
  return file;
}

// 1. Compiler binaries: one per requested triple.
const binaries = new Map(); // triple -> binary path
if (modeAll) {
  for (const t of TRIPLES) {
    binaries.set(t.triple, findBinary(t));
  }
} else {
  sh('cargo', ['build', '--release', '-p', 'rust-sass-cli'], repoRoot);
  binaries.set(triple(), p.join(pkgDir, 'platform-tmp-sass'));
  const rustBinary =
    process.platform === 'win32' ? 'rust-sass.exe' : 'rust-sass';
  fs.copyFileSync(
    p.join(repoRoot, 'target', 'release', rustBinary),
    p.join(pkgDir, 'platform-tmp-sass'),
  );
}
for (const [name, binPath] of binaries) {
  if (!fs.existsSync(binPath)) {
    throw new Error(`missing compiler binary for ${name}: ${binPath}`);
  }
}

// 2. Host source deps + build, in place (node_modules/ and dist/ are
// gitignored inside the submodule — its tracked tree stays pristine).
if (!fs.existsSync(p.join(hostDir, 'node_modules'))) {
  // Clean-install when the host tree carries a lockfile, plain install
  // otherwise (our own lockfile pins what we test).
  const ci = fs.existsSync(p.join(hostDir, 'package-lock.json'));
  sh('npm', [ci ? 'ci' : 'install', '--no-audit', '--no-fund'], hostDir);
}
// Vendor sources (protobuf bindings via buf, JS API from the language repo).
// --skip-compiler: we never need the Dart binary; --language-path points at
// our pinned sass submodule (all local, no network, no Dart SDK).
sh('npx', [
  'ts-node',
  './tool/init.ts',
  '--skip-compiler',
  '--language-path',
  p.resolve(repoRoot, 'sass'),
], hostDir);
sh('npm', ['run', 'clean'], hostDir);
sh('npm', ['run', 'compile'], hostDir);

// 3. Copy the built dist/ here and apply the one-line template swap.
fs.rmSync(distDir, {recursive: true, force: true});
fs.cpSync(p.join(hostDir, 'dist'), distDir, {recursive: true});
const moduleJs = p.join(distDir, 'lib', 'src', 'compiler-module.js');
let moduleSrc = fs.readFileSync(moduleJs, 'utf8');
const from = 'sass-embedded-${platform}-${arch}';
const to = 'sass-embedded-rust-${platform}-${arch}';
const hits = moduleSrc.split(from).length - 1;
if (hits !== 1) {
  throw new Error(
    `template swap assertion failed: expected 1 hit, found ${hits} in ${moduleJs}`,
  );
}
moduleSrc = moduleSrc.split(from).join(to);
fs.writeFileSync(moduleJs, moduleSrc);

// Public typings: dist/types/ mirrors lib/src/vendor/sass (minus README),
// plus index.m.d.ts for the .mjs entry — the same assembly upstream
// prepare-release.ts performs (tsc emits declarations to _types/, never
// into dist/, so without this the `types` fields dangle).
// NOTE: vendor/sass is a symlink (init.ts links the language repo's
// js-api-doc); dereference into real files, since npm will not pack
// symlinks escaping the package root.
const vendorTypes = p.join(hostDir, 'lib', 'src', 'vendor', 'sass');
const distTypes = p.join(distDir, 'types');
fs.rmSync(distTypes, {recursive: true, force: true});
fs.cpSync(vendorTypes, distTypes, {recursive: true, dereference: true});
if (fs.lstatSync(distTypes).isSymbolicLink()) {
  throw new Error(`dist/types is a symlink — npm would silently skip it`);
}
fs.rmSync(p.join(distTypes, 'README.md'), {force: true});
fs.copyFileSync(
  p.join(distTypes, 'index.d.ts'),
  p.join(distTypes, 'index.m.d.ts'),
);

// Publish manifest: package.dist.json (the version authority — source
// package.json stays 0.0.0) + LICENSE + README land in dist/, so `npm
// publish ./dist` ships exactly this tree.
fs.copyFileSync(p.join(pkgDir, 'package.dist.json'), p.join(distDir, 'package.json'));
fs.copyFileSync(p.join(repoRoot, 'LICENSE'), p.join(distDir, 'LICENSE'));
fs.copyFileSync(p.join(pkgDir, 'README.md'), p.join(distDir, 'README.md'));

// 4. Assemble the platform package(s) (mirrors the registry layout the
// wrapper resolves: <pkg>/dart-sass/sass).
const wrapperVersion = JSON.parse(fs.readFileSync(p.join(pkgDir, 'package.dist.json'), 'utf8')).version;
const assembledTriples = modeAll ? TRIPLES.map((t) => t.triple) : [triple()];
for (const name of assembledTriples) {
  const tripleBinary = binaries.get(name);
  const t = TRIPLES.find((x) => x.triple === name) ?? {triple: name, os: process.platform === 'win32' ? 'win32' : process.platform, cpu: process.arch};
  const platDir = p.join(pkgDir, 'platform', name);
  const dartSassDir = p.join(platDir, 'dart-sass');
  fs.mkdirSync(dartSassDir, {recursive: true});
  const platPkg = {
    name: `sass-embedded-rust-${name}`,
    version: wrapperVersion,
    description: `rust-sass embedded compiler binary (${name}).`,
    license: 'MIT',
    files: ['dart-sass/**/*'],
    engines: {node: '>=14.0.0'},
    os: [t.os],
    cpu: [t.cpu],
    ...(t.libc ? {libc: t.libc} : {}),
  };
  fs.writeFileSync(p.join(platDir, 'package.json'), JSON.stringify(platPkg, null, 2) + '\n');
  if (name.startsWith('win32')) {
    const exe = p.join(dartSassDir, 'sass.exe');
    fs.copyFileSync(tripleBinary, exe);
    // Stock resolution probes `dart-sass/sass.bat` on Windows.
    fs.writeFileSync(
      p.join(dartSassDir, 'sass.bat'),
      `@echo off\r\n"%~dp0sass.exe" %*\r\n`,
    );
  } else {
    const bin = p.join(dartSassDir, 'sass');
    fs.copyFileSync(tripleBinary, bin);
    fs.chmodSync(bin, 0o755);
  }
}
fs.rmSync(p.join(pkgDir, 'platform-tmp-sass'), {force: true});

// 5. Link the host platform into node_modules (same require.resolve path
// as a registry install; script state, never committed).
const hostPlatDir = p.join(pkgDir, 'platform', triple());
const nmDir = p.join(pkgDir, 'node_modules');
fs.mkdirSync(nmDir, {recursive: true});
const link = p.join(nmDir, platformPackageName());
fs.rmSync(link, {recursive: true, force: true});
fs.symlinkSync(
  p.relative(nmDir, hostPlatDir),
  link,
  process.platform === 'win32' ? 'junction' : 'dir',
);

// 6. Report the resolved binary (also asserted by the test gate). Entry
// mirrors stock resolution (`dart-sass/sass`, + `.bat` on Windows).
const entry =
  process.platform === 'win32' ? 'dart-sass/sass.bat' : 'dart-sass/sass';
const resolved = require.resolve(`${platformPackageName()}/${entry}`);
console.log(`[build] platform package: ${platformPackageName()}`);
console.log(`[build] compiler binary:  ${resolved}`);
