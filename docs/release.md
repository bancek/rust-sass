# Release runbook

Tags track the dart-sass version (`v1.104.0`); rehearsals add a suffix
(`v1.104.0-alpha0`, `v1.104.0-alpha1`). Mechanics: [`ci.md`](ci.md). Bump
surfaces: [`CONTRIBUTING.md`](CONTRIBUTING.md#version-bump-checklist).

## Bump

`tools/bump-version.sh <version>` — full version to manifests/cargo,
suffix-stripped base to reported strings. Anchored, fails loudly on drift.
Then: triage its sweep report, `CHANGELOG.md` entry, `cargo check`
(re-syncs `Cargo.lock`), commit. Registry writes are tag-gated — pushes
without tags only build/test/pack/upload artifacts.

## First release (alpha0, manual registries)

Bootstraps everything before automation can use it. No tag at any point.

1. Re-enable parked workflows (`*.disabled` back) — need the matrices
   for artifacts.
2. Bump to `1.104.0-alpha0`, CHANGELOG, `cargo check`, commit, push.
3. CI green → `gh run download <run-id> -D alpha-artifacts`.
4. Classic tokens in env (`CARGO_REGISTRY_TOKEN`, `NODE_AUTH_TOKEN`;
   OIDC never serves hand runs). Rotate/delete after.
5. `cargo publish` ×6 from checkout, in order, with waits
   (macros, embedded-pb, rust-sass, libsass, embedded, cli).
6. `npm publish --tag next` from the downloaded tarballs: 8 platforms,
   then wrapper, then wasm. Never omit `--tag` on a prerelease.
7. Optional paper trail: `gh release create v1.104.0-alpha0 --draft
   --prerelease --generate-notes` + upload.
8. Set up trusted publishing now (npm packages exist; crates pending
   publishers) — prerequisite for every flow below.

## CI prerelease (alpha1, `1.105.0-alpha0`, …)

Standing rehearsal mechanism, not a one-off: proves the full automation
before each final. Needs trusted publishing live (preceding step).

1. Bump, CHANGELOG, `cargo check`, commit, push.
2. CI green → tag (e.g. `v1.104.0-alpha1`), push tag.
3. CI attaches assets + publishes: npm under `next` (derived from the
   `-` in the tag), crates as-is (opt-in by resolution), libsass zips
   attached. Release created as **draft** — edit notes, publish when
   ready (email fires then).

## Final release

Same as CI prerelease, minus the suffix:

1. Bump (no suffix), CHANGELOG, `cargo check`, commit, push.
2. CI green → tag `v<version>`, push tag.
3. CI publishes under `latest` (npm default tag) + crates + zips;
   draft → edit → publish.
