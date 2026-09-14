# CI

All workflows use `concurrency.cancel-in-progress` on every push.

## `ci.yml` (push to `main` + PRs)

Two jobs. `test` runs on ubuntu, macOS and Windows: `cargo build
-p rust-sass-libsass` then `cargo build --workspace` (fresh checkouts need
libsass first — see `CONTRIBUTING.md` Building), clippy deny gate both legs
(`-D warnings`, sync + `--features async`), unit tests sync + async
(`rust-sass`, `rust-sass-macros`, `rust-sass-embedded`), `glibc-math` check
leg. The multi-OS matrix is deliberate: OS-gated code and OS-sensitive
behavior (fs paths, symlinks, case) are invisible to an ubuntu-only gate, and
the npm matrix jobs only run `cargo build`. `spec` stays ubuntu-only: the
release spec suite (`--release -- --ignored`, needs the `sass-spec`
submodule — checkout uses `submodules: recursive`) needs the LTO release
build, too pricey to triplicate while caches stay disabled. Clippy gate
checklist lives in [`ci-todo.md`](ci-todo.md).

## `npm.yml` (push to `main` + PRs + tags `v*`)

Release + npm pipeline for `sass-embedded-rust`. `mimalloc` links C and each
OS needs its own SDK, so binaries build on native per-arch runners (no
linux-hosted cross path) and one ubuntu job assembles everything downstream:

- Build matrix: 8 jobs (linux-x64/arm64, linux-musl-x64/arm64, darwin-x64/arm64,
  win32-x64/arm64), each `cargo build --release --target <triple> -p
  rust-sass-cli` with default features (sync + embedded + mimalloc;
  `glibc-math` stays wasm-only) and one bare-binary artifact
  (`rust-sass-<triple>`, `archive: false` — downloadable directly, ready for
  human testing).
  No submodules: the CLI build needs workspace crates only. Musl jobs build
  against the musl target with `musl-tools` — genuine musl-linked binaries,
  never relabeled gnu binaries.
- Assemble job (`needs:` the matrix): downloads the 8 binaries (`file`
  listing shows arch/libc per binary) → Alpine smoke for the musl-x64 binary
  (it must actually run on musl; musl-arm64 is smoked natively inside its own
  arm64 build job instead — the x64 assemble runner cannot execute arm64
  without QEMU) → `npm ci` → `npm run build -- --platforms=all` with
  `RUST_SASS_DIST` (assembles the 8 platform packages from the prebuilt
  binaries, plus the `dist/` publish tree; see `embedded-host-node-rust/build.mjs`)
  → `npm test` (resolve gate + harness + coexistence) → pack wrapper + 8
  platform tgzs → scratch-install smoke (`compileString` asserting `3px`,
  the true consumer path with zero registry involvement) → per-file artifacts
  (9 npm tarballs + 8 release archives + checksums, 14-day retention — any run
  is downloadable without a release) → release archives
  (`rust-sass-<version>-<platform>-<arch>.tar.gz`/`.zip`, upstream-style names)
  with Sigstore attestation on tags.
- Tags only: `gh release upload` attaches archives + tarballs to the GitHub
  release. Tags also publish: version guard (tag base vs
  `package.dist.json`), then `npm publish --provenance` of the 8 platform
  packages + `./dist` — under `next` when the tag carries a `-` suffix
  (derived, not gated), `latest` otherwise. Auth is npm trusted publishing
  (OIDC) — no token secret.

Local testing without publishing: `gh release download <tag>` (or the
Actions-artifact download for non-tag runs), then `npm install` the wrapper +
platform tarballs in a scratch dir, exactly like the smoke step.

No version injection: `--version` reports the hardcoded `SASS_VERSION`, which
tracks dart-sass via the version-bump checklist in
[`CONTRIBUTING.md`](CONTRIBUTING.md).

## `wasm.yml` (push to `main` + PRs + tags `v*`)

Release + npm pipeline for `rust-sass-wasm`, the pure-JS/wasm fallback (JS
API + `bin/sass.js` + `pkg-sync`/`pkg-async` bundles; `pkg-web` stays
dev-only). Platform-independent, so a single ubuntu job does everything: Rust
1.92.0 + `wasm32-unknown-unknown` + wasm-pack, `npm ci`,
`npm run build:rust:release` (strips wasm-pack's nested `package.json` /
`.gitignore` / `README.md` / `LICENSE` from each `pkg-*` dir — a nested
`package.json` makes npm treat the dir as a nested package and silently drop
the whole bundle from the tarball), `npm run build:js`, `npm run test:js`
(vitest), pack `js/dist`, scratch-install smoke (`compileString` asserting
`3px` plus `bin/sass.js --version` — dependencies resolve from the registry,
exactly as for a real user), per-file artifact (14-day retention).

Tags only: `gh release upload` attaches the tgz. Tags also publish: version
guard (`js/package.json` vs tag base), then `npm publish --provenance
./js/dist` via npm trusted publishing (same OIDC posture as `npm.yml`) —
under `next` on `-`-suffixed tags, `latest` otherwise.

Manifest notes: `js/package.json` is the published manifest (copied into
`js/dist/` by `build:js`). Its exact-pinned `dependencies` (`immutable`,
`colorjs.io` — the value layer's runtime imports; `tsc` never bundles) ride
the version-bump checklist. `engines` is `>=18.0.0`, set empirically: the
packed tarball was smoke-tested in `node:14/16/18/20/22` containers
(`compileString` + ESM import + `bin/sass.js`) — 14 fails on `colorjs.io`'s
own `||=` syntax, 16 fails on wasm reference types (`externref`, stable from
17), 18+ pass fully.

## `libsass.yml` (push to `main` + PRs + tags `v*`)

Release pipeline for the libsass C-ABI adapter: one self-contained
`rust-sass-libsass-<version>-<triple>.zip` per triple (`include/` with pinned
upstream headers + `lib/` with cargo's native filenames, never renamed —
the import lib embeds the DLL name), attached to `v*` releases. No npm, no
node anywhere in this workflow.

- Build matrix: 8 jobs (same runner/target table as `npm.yml`),
  `cargo build --release -p rust-sass-libsass` + `cargo test
  -p rust-sass-libsass-tests` (rust-impl leg, debug — behavior gate without
  paying LTO twice) + consumer smoke: upstream sassc built with MSVC
  (`cl`/`link` with dash-prefixed flags — git-bash rewrites `/`-prefixed
  args; `link.exe` by absolute path from `cl`'s directory; `shell32.lib`
  for the UTF-8 argv shim) or `tool/build-sassc.sh` on unix, then a stdin
  compile asserting `3px`. musl triples ship build-gated only (a gnu runner
  cannot execute or link-test musl binaries).
- Assemble job (ubuntu, shell only): downloads the 8 lib dirs, stamps
  `sass/version.h` from `version.h.in` with the ABI version from
  `rust-sass-libsass/src/base.rs` (never the submodule's `[NA]`
  placeholder; a header-count guard fails the run if upstream adds one),
  packs the zips + separate checksums file, per-file artifacts (14-day
  retention), attestation + `gh release upload` on tags (version guard: tag
  base vs workspace `Cargo.toml`).

## `crates.yml` (push to `main` + PRs + tags `v*`)

crates.io releases for the 6 publishable workspace crates, in dependency
order: `rust-sass-macros`, `rust-sass-embedded-pb`, `rust-sass`,
`rust-sass-libsass`, `rust-sass-embedded`, `rust-sass-cli` (harness crates —
spec, libsass-tests, pb-gen, wasm — carry `publish = false`). Single ubuntu
job, no matrix: registry artifacts are platform-independent source packages.

Every run lists packaged files per crate (`cargo package --list` — the file
health gate). Tags also publish: version guard (tag base vs
workspace `Cargo.toml`), then real `cargo publish` in order with a retry loop
per dependent crate (the sparse index needs seconds to see the just-published
dep). Prereleases publish as-is (cargo has no channels — opt-in by
resolution). Auth is OIDC trusted publishing — no token anywhere; the one manual
prerequisite is registering a pending trusted publisher per crate name on
crates.io (all 6 verified unclaimed).

Deliberately not `cargo publish --dry-run` for rehearsal: dry-run resolves
path-deps against the index, so every dependent fails pre-first-publish with
"no matching package". `--list` proves file health without the index; the
ordered real publish is the true gate (and post-first-publish, dependents
resolve normally).

## `playground.yml` (push to `main` only, never PRs)

Node 26 + Rust 1.92.0 + `wasm32-unknown-unknown` + wasm-pack, `npm ci`,
`npm run build:playground:release` (release wasm ×3, assemble
`rust-sass-wasm/playground/`), deploy to the `gh-pages` branch via
`peaceiris/actions-gh-pages` (commits only when content changed; needs
`contents: write`).

## Manifest model (`package.dist.json`)

`embedded-host-node-rust/package.dist.json` is the single version that matters
(1.104.0): the assemble job copies it into `dist/` as the published manifest,
platform versions and the tag guard derive from it. Source `package.json`
stays `0.0.0` forever — npm ≥ 11 rejects `npm ci` on lockfiles whose
optionalDeps are unresolvable (our platform packages, unpublished by design),
while `0.0.0` without optionalDeps installs cleanly on every npm version. The
lockfile carries no platform entries (they live only in the dist manifest),
so no lockfile re-sync is needed for platform changes.

## Tags and prereleases

Tags track the pinned dart-sass version: `v1.104.0`, etc. Pushing a `v*` tag
creates the GitHub release with CLI binaries + npm tarballs, and publishes
registries: npm under `next` on `-`-suffixed tags, `latest` otherwise
(crates publish as-is — opt-in by resolution). The full procedure, including
the alpha/final distinction, lives in [`release.md`](release.md); the bump
checklist in [`CONTRIBUTING.md`](CONTRIBUTING.md).

Test the release path with a prerelease suffix (never `latest`):

```sh
git tag v1.104.0-prerelease && git push origin v1.104.0-prerelease
# verify assets on the release page, then clean up:
gh release delete v1.104.0-prerelease --cleanup-tag
git tag -d v1.104.0-prerelease
```

## While the repo is private

- Caches are commented out in all workflows (re-enable on publish day —
  one line each).
- GitHub Pages needs a public repo on the Free plan: once public, set
  Settings → Pages → `gh-pages` branch, `/` root, and the playground goes
  live at `https://bancek.github.io/rust-sass`.
- For npm trusted publishing, add the repo as a trusted publisher on the
  `sass-embedded-rust*` packages at first publish; nothing is parked in
  secrets beforehand.

## Roadmap (full CI)

- [ ] Re-enable caches (`ci.yml`, `npm.yml`, `wasm.yml`, `playground.yml`)
      when public.
- [ ] wasm gates in CI (wasm spec + js-api-spec harnesses; vitest already runs
      in `wasm.yml`).
- [ ] Upstream-harness CLI cross-check (`npm run sass-spec -- --command ../target/release/rust-sass`).
- [ ] Platform parity beyond the big 6 + musl (android/riscv/armv7) — see
      [`ref/sass-embedded-rust.md`](ref/sass-embedded-rust.md) §Binary distribution.
- [ ] Enable trusted publishing: npm (`wasm.yml`, `npm.yml` — register the
      repo for `rust-sass-wasm` + `sass-embedded-rust*`, then uncomment the
      `id-token` permissions) and crates.io (`crates.yml` — pending
      publishers for all 6 crate names).
