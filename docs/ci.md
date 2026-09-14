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

## `release.yml` (push to `main` + PRs + tags `v*`)

Single release pipeline: native binaries + libsass adapter + wasm bundles
build in parallel, then one `publish` job owns every write (draft-release
creation, asset uploads, registry publishes). Build jobs never touch
registries or releases; `crates.yml` stays separate (no release interplay —
registry source packages only).

- Build matrix (`build-native`): 8 jobs (linux-x64/arm64,
  linux-musl-x64/arm64, darwin-x64/arm64, win32-x64/arm64), each building BOTH
  the CLI binary (`cargo build --release --target <triple> -p rust-sass-cli`
  with default features: sync + embedded + mimalloc; `glibc-math` stays
  wasm-only) and the libsass adapter (`-p rust-sass-libsass`) in two separate
  build steps (so the red step names the failing crate), plus the contract
  suite and the sassc consumer smoke on non-musl triples. `mimalloc` links C
  and each OS needs its own SDK, so binaries build on native per-arch runners
  (no linux-hosted cross path). Musl jobs build against the musl target with
  `musl-tools` — genuine musl-linked binaries, never relabeled gnu binaries
  (musl-arm64 is smoked natively inside its own cell; musl-x64 gets an Alpine
  smoke in `publish`). Each cell uploads two single-file artifacts (the bare
  CLI binary `rust-sass-<triple>`, downloadable directly and ready for human
  testing; the adapter libs `libsass-<triple>`). No submodules except
  libsass+sassc on smoke-tested triples.
- Wasm job (`build-wasm`): single ubuntu job for `rust-sass-wasm`, the
  pure-JS/wasm fallback (JS API + `bin/sass.js` + `pkg-sync`/`pkg-async`
  bundles; `pkg-web` stays dev-only). Rust 1.92.0 + `wasm32-unknown-unknown`
  + wasm-pack, `npm ci`, `npm run build:rust:release` (strips wasm-pack's
  nested `package.json` / `.gitignore` / `README.md` / `LICENSE` from each
  `pkg-*` dir — a nested `package.json` makes npm treat the dir as a nested
  package and silently drop the whole bundle from the tarball), `npm run
  build:js`, `npm run test:js` (vitest), pack `js/dist`, scratch-install
  smoke (`compileString` asserting `3px` plus `bin/sass.js --version` —
  dependencies resolve from the registry, exactly as for a real user),
  per-file artifact (14-day retention). Needs workspace crates + the `sass`
  language-spec submodule only.
- Publish job (`needs:` both builds, ubuntu, shell only for the libsass
  half): downloads the 8 CLI binaries + adapter libs → `npm ci` → assembly
  (`--platforms=all` from the prebuilt binaries, plus the `dist/` publish
  tree; see `embedded-host-node-rust/build.mjs`) → `npm test` (resolve gate
  + harness + coexistence) → pack wrapper + 8 platform tgzs → scratch-install
  smoke (the true consumer path with zero registry involvement) → CLI release
  archives (`rust-sass-<version>-<platform>-<arch>.tar.gz`/`.zip`,
  upstream-style names) → libsass zips (pinned upstream headers + native
  libs with cargo's native filenames, never renamed — the import lib embeds
  the DLL name; `sass/version.h` stamped from `base.rs`, never the
  submodule's `[NA]` placeholder; musl archives are static-only since rustc
  drops `cdylib` there; a header-count guard fails the run if upstream adds
  one) → three version guards (tag vs `package.dist.json` /
  `js/package.json` / workspace `Cargo.toml`, full-vs-full so prereleases
  pass) → single draft-release creation (`--prerelease` on `-`-suffixed
  tags, `--latest` otherwise; skipped when re-running) → Sigstore attestation
  of both archive sets → one `gh release upload` of everything → npm publish
  of the 8 platform packages, then the wrapper, then the wasm dist — under
  `next` when the tag carries a `-` suffix (derived, not gated), `latest`
  otherwise, via npm trusted publishing (OIDC, no token secret). Per-file
  artifacts (14-day retention) make any run downloadable without a release.

Local testing without publishing: `gh release download <tag>` (or the
Actions-artifact download for non-tag runs), then `npm install` the wrapper +
platform tarballs in a scratch dir, exactly like the smoke step.

No version injection: `--version` reports the hardcoded `SASS_VERSION`, which
tracks dart-sass via the version-bump checklist in
[`CONTRIBUTING.md`](CONTRIBUTING.md).

Manifest notes: `js/package.json` is the published wasm manifest (copied
into `js/dist/` by `build:js`). Its exact-pinned `dependencies`
(`immutable`, `colorjs.io` — the value layer's runtime imports; `tsc` never
bundles) ride the version-bump checklist. `engines` is `>=18.0.0`, set
empirically: the packed tarball was smoke-tested in `node:14/16/18/20/22`
containers (`compileString` + ESM import + `bin/sass.js`) — 14 fails on
`colorjs.io`'s own `||=` syntax, 16 fails on wasm reference types
(`externref`, stable from 17), 18+ pass fully.

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

- [ ] Re-enable caches (`ci.yml`, `release.yml`, `playground.yml`)
      when public.
- [ ] wasm gates in CI (wasm spec + js-api-spec harnesses; vitest already runs
      in `release.yml`).
- [ ] Upstream-harness CLI cross-check (`npm run sass-spec -- --command ../target/release/rust-sass`).
- [ ] Platform parity beyond the big 6 + musl (android/riscv/armv7) — see
      [`ref/sass-embedded-rust.md`](ref/sass-embedded-rust.md) §Binary distribution.
- [ ] Enable trusted publishing: npm (`release.yml` — register the
      repo for `rust-sass-wasm` + `sass-embedded-rust*`, then uncomment the
      `id-token` permissions) and crates.io (`crates.yml` — pending
      publishers for all 6 crate names).
