#!/bin/bash
# tools/bump-version.sh <new-version> — set the release version repo-wide.
#
# <new-version> is full semver with an optional prerelease suffix, e.g.
# `1.104.0` or `1.104.0-alpha1`. Run from anywhere; operates on the repo root.
#
# Split model (see docs/release.md):
# - FULL version (as given): registry manifests + cargo versions — the
#   identity that gets published.
# - BASE version (suffix stripped): reported strings (SASS_VERSION,
#   COMPILER_VERSION, handshake fixture, version assertions) — what Dart
#   would report, so hosts and --version parsers keep seeing exact values.
# Everything else (docs prose, oracle pins, historical markers, 0.0.0
# sources, lockfiles) is triaged by hand from the sweep report below.
set -eu

die() {
  echo "bump-version: $*" >&2
  exit 1
}

[ $# -eq 1 ] || die "usage: tools/bump-version.sh <new-version> (e.g. 1.104.0-alpha1, 1.104.0)"
NEW="$1"
BASE="${NEW%%-*}"

[[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] \
  || die "'$NEW' is not valid semver (want X.Y.Z[-prerelease])"

cd "$(dirname "$0")/.."

OLD=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
[ -n "$OLD" ] || die "could not detect current version from Cargo.toml"
BASE_OLD="${OLD%%-*}"
[ "$NEW" != "$OLD" ] || die "already at $OLD (nothing to do)"

# Portable in-place sed (GNU vs BSD/macOS).
if sed --version >/dev/null 2>&1; then
  sedi() { sed -i "$@"; }
else
  sedi() { sed -i '' "$@"; }
fi

# Replace, failing loudly if the anchor pattern stopped matching (drift).
rep() {
  file="$1"
  pattern="$2"
  replacement="$3"
  if ! grep -q "$pattern" "$file"; then
    die "$file: pattern not found (drift?): $pattern"
  fi
  # Escape `&` (whole-match in sed replacements); backslashes pass through.
  replacement="${replacement//&/\\&}"
  sedi "s/$pattern/$replacement/g" "$file"
  echo "  updated $file"
}

echo "bump-version: $OLD -> $NEW (base $BASE_OLD -> $BASE)"
echo "== full version =="
rep Cargo.toml "version = \"$OLD\"" "version = \"$NEW\""
rep embedded-host-node-rust/package.dist.json "\"$OLD\"" "\"$NEW\""
rep rust-sass-wasm/js/package.json "\"version\": \"$OLD\"" "\"version\": \"$NEW\""
echo "== base version =="
rep rust-sass-cli/src/options.rs "SASS_VERSION: &str = \"$BASE_OLD\"" "SASS_VERSION: &str = \"$BASE\""
rep rust-sass-wasm/js/src/version.ts "SASS_VERSION = '$BASE_OLD'" "SASS_VERSION = '$BASE'"
rep rust-sass-embedded/src/lib.rs "COMPILER_VERSION: &str = \"$BASE_OLD\"" "COMPILER_VERSION: &str = \"$BASE\""
rep rust-sass-embedded/src/lib.rs "\\\\\"compilerVersion\\\\\": \\\\\"$BASE_OLD\\\\\"" "\\\\\"compilerVersion\\\\\": \\\\\"$BASE\\\\\""
rep rust-sass-embedded/src/lib.rs "\\\\\"implementationVersion\\\\\": \\\\\"$BASE_OLD\\\\\"" "\\\\\"implementationVersion\\\\\": \\\\\"$BASE\\\\\""
rep rust-sass-wasm/js/src/wasm/cli.test.ts "$BASE_OLD compiled with" "$BASE compiled with"

echo "== sweep report (triage by hand — code must be clean, historical prose stays) =="
git grep -F -l "$OLD" -- . || true

echo "== next steps =="
echo "  1. Add a CHANGELOG.md entry (human prose, not scripted)."
echo "  2. The next cargo invocation re-syncs Cargo.lock path entries silently."
if [[ "$NEW" == *-* ]]; then
  echo "  3. Prerelease: commit, push, wait for CI, then tag v$NEW — CI"
  echo "     publishes registries under \`next\` (needs trusted publishing"
  echo "     live). The very first release is manual instead: no tag, pull"
  echo "     CI artifacts, publish by hand — see docs/release.md."
else
  echo "  3. Final: commit, push, wait for CI, then tag v$NEW and push the tag;"
  echo "     see the release runbook in docs/release.md."
fi
