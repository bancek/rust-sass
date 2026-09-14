# Module: `embedded-host-node-rust`

The `sass-embedded-rust` npm package: the genuine `sass-embedded` host API
backed by the rust-sass embedded compiler instead of Dart Sass.

## Why this exists (and why not a patch)

`sass-embedded` resolves its compiler binary in `compiler-path.js` with no
public knob (no env var, no option). The dev-time `require.cache` patch worked
locally but mutates a process-global singleton: importing `sass-embedded-rust`
alongside `sass-embedded` would silently rewire the app's host too. Scoped
patching is impossible, save/restore races with concurrent compilations, and
worker/process proxies can't carry user closures (importers, functions).
So the release strategy assembles the host into **our own module namespace**:
zero shared modules with an app's `sass-embedded`, coexistence by construction.
Rejected alternatives: vendoring `dist/` (153 files committed), npm
`overrides` for the platform peer (install-time existence requirement plus a
static 19-entry list that can't condition on platform), vitest `alias` (wrong
layer — externalized CJS bypasses vite resolution), relying on the stock
`sass.js` fallback (same-version false-green risk — hence the resolve gate).

## Build-time assembly (nothing vendored)

`embedded-host-node` submodule @ tag `1.104.0` (`f6e991a8`), full clone:

1. The compiler binary: dev builds it locally (`cargo build --release -p
   rust-sass-cli` for the host); CI feeds prebuilt per-platform binaries via
   `npm run build -- --platforms=all` with `RUST_SASS_DIST` (one dir per
   triple holding its bare `rust-sass-<triple>` matrix artifact).
2. `npm ci` + `npm run compile` inside `./embedded-host-node`, in place
   (`node_modules/` and `dist/` are gitignored there — the tracked tree stays
   pristine; run `clean` first for determinism).
3. Copy its `dist/` to `embedded-host-node-rust/dist/` and apply the one-line
   template swap in `dist/lib/src/compiler-module.js` (`` `sass-embedded-${platform}-${arch}` ``
   → `` `sass-embedded-rust-…` ``; asserted to hit exactly once). This is the
   ONLY fork point in host source (verified: the sole functional
   `sass-embedded-` reference; musl/arch detection untouched).
   `compiler-path.ts` stays stock; unsupported platforms keep the upstream
   `sass.js` fallback (documented behavior, not relied upon — see the gate).
   Public typings follow: `dist/types/` mirrors `lib/src/vendor/sass` (minus
   README) plus `index.m.d.ts` — the same assembly upstream
   `prepare-release.ts` performs, since tsc emits declarations to `_types/`,
   never into `dist/`. The vendor tree is a symlink, so the copy dereferences
   into real files (npm silently drops escaping symlinks from packs) with an
   explicit symlink guard. The published manifest is `package.dist.json`
   (the version authority — source `package.json` stays `0.0.0`), plus
   LICENSE + README.
4. Assemble the platform package(s) (`package.json` with `os`/`cpu` mirrored
   from the real platform manifests — musl triples add `libc: musl`, matching
   upstream — + `dart-sass/sass` binary, exec bit preserved; Windows ships
   `sass.bat` + `sass.exe` per the resolution order) and link the host one
   into `node_modules/` (script state, never committed — same
   `require.resolve` path as a registry install). Every triple ships its own
   genuine native binary: musl triples are real musl-linked builds, never
   relabeled gnu binaries.
5. `test/test.mjs` (migrated harness, repointed at `../dist`, behind the
   `test/gate.mjs` resolve gate) + `test/coexistence.mjs` (both engines, one
   process) run against the assembly.

`index.mjs` delegates to the CJS `index.js` in host dist, so the single patched
file covers both import styles.

## Coexistence + resolve gate

`test/gate.mjs` runs first in every suite: it asserts the platform specifier
resolves, the binary exists and is a `rust-sass` binary, and the wrapper's
`compilerCommand` points at it — failing loudly with the binary path logged.
Without this gate, a missing platform package would fall back to Dart Sass
(same version, byte-identical output) and the suite would pass against the
wrong engine.

## Binary distribution

- **v1 (now):** wrapper + the 8 per-platform packages (big 6 + musl)
  assembled by the build script; local dev links its own platform,
  unpublished. Registry publish = `release.yml` CI matrix (8 genuine native
  binaries in the `build-native` job) + `publish` job (wrapper with exact-pinned `optionalDependencies`
  via `package.dist.json`, host pattern). Android/riscv/armv7 triples arrive
  with full upstream parity later.
- Wrapper version tracks dart-sass from `1.104.0` via `package.dist.json`
  (source `package.json` stays `0.0.0`); host runtime deps exact-pinned
  (proven with host 1.104.0), lockfile committed.

## Update procedure

Bump the submodule pin → rebuild → rerun harness + coexistence + tarball
validation (`npm pack` both, scratch-dir install, smoke). Touch the patch only
if upstream refactors the two `compiler-path` consumers (`compiler/sync.js`,
`compiler/async.js`). Version bumps ride the re-sync train (`upstream.md`,
`COMPILER_VERSION`, `version.ts`, wrapper + platform rebuild).

Rebuild hygiene: `embedded-host-node/node_modules/` predates the pin move
(the build script only installs when it is absent), so a stale tree breaks
the build with toolchain drift (observed: TS 5.9.3 vs required TS 6) — and
its gitignored `package-lock.json` goes stale the same way. Delete
`node_modules/` (and the ignored lockfile, regenerated on install) inside
the submodule before rebuilding after a pin move; the tracked tree stays
pristine either way.

## File mapping

| Dart/JS                                                                | Rust                                                                           |
| ---------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| `embedded-host-node` submodule (`sass/embedded-host-node` @ `1.104.0`) | `embedded-host-node-rust/` (build script, patch application, manifests, tests) |
| `sass-embedded` npm host (dev-only peer)                               | coexistence differential target                                                |
