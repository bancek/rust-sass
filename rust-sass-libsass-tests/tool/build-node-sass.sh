#!/bin/sh
# node-sass gate: builds node-sass against OUR libsass and runs the api/cli
# acceptance subset.
#
# Usage:
#   tool/build-node-sass.sh            # provision + build only
#   tool/build-node-sass.sh --run      # provision + build, then run the gate
#
# Environment:
#   PROFILE=release                    # use the release cdylib (default: debug)
#
# GYP QUIRK (probed 2026-09-07 with node-gyp 10.3.1/gyp-next): in
# binding.gyp's `libsass_ext == "yes"` branch the condition matches and
# `libraries: ['<(libsass_library)']` expands, but `cflags_cc` and `ldflags`
# `<(...)` references silently vanish from the generated makefile (verified
# with single-token probes: only `libraries` survives). The `auto` branch
# uses the same dead keys, so pkg-config mode cannot work either (the .pc
# below is still provisioned — other consumers use it). Workaround, no
# submodule edits: headers via $CPLUS_INCLUDE_PATH (clang honors it,
# node-gyp passes env through) and the whole link line smuggled through the
# one working key, `libraries` (space-split into LIBS).
#
# node-sass selects an external libsass through the `libsass_ext` GYP variable
# (binding.gyp:40-70), fed from $LIBSASS_EXT/$LIBSASS_CFLAGS/$LIBSASS_LDFLAGS/
# $LIBSASS_LIBRARY by scripts/build.js:57-61. `yes` passes explicit flags;
# `auto` goes through pkg-config (needs the generated libsass.pc below).
# The prebuilt-binary download is bypassed (install.js honors
# SKIP_SASS_BINARY_DOWNLOAD_FOR_CI) and the source build forced
# (SASS_FORCE_BUILD). All outputs live under target/node-sass-gate/
# (ephemeral, never committed); the binding itself lands in node-sass/vendor/
# (gitignored upstream).
#
# See docs/plans/libsass.md step 9.
set -eu

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PROFILE="${PROFILE:-debug}"
OUT="$ROOT/target/node-sass-gate"
OS="$(uname -s)"

if [ "$PROFILE" = "release" ]; then
    CARGO_PROFILE_FLAG="--release"
else
    CARGO_PROFILE_FLAG=""
fi

# 1. Our library (cdylib).
# shellcheck disable=SC2086
cargo build -p rust-sass-libsass $CARGO_PROFILE_FLAG

# 2. Assemble the drop-in tree: include/ + lib/libsass.{dylib,so} +
# lib/pkgconfig/libsass.pc (the second drop-in path, plan §7 step 9).
LIB_SRC=""
if [ -f "$ROOT/target/$PROFILE/libsass.dylib" ]; then
    LIB_SRC="$ROOT/target/$PROFILE/libsass.dylib"
elif [ -f "$ROOT/target/$PROFILE/libsass.so" ]; then
    LIB_SRC="$ROOT/target/$PROFILE/libsass.so"
else
    echo "error: no shared libsass in $ROOT/target/$PROFILE" >&2
    exit 1
fi
mkdir -p "$OUT/lib/pkgconfig" "$OUT/include"
cp "$LIB_SRC" "$OUT/lib/"
rm -rf "$OUT/include"
cp -r "$ROOT/libsass/include" "$OUT/include"
cat > "$OUT/lib/pkgconfig/libsass.pc" <<EOF
prefix=$OUT
exec_prefix=\${prefix}
libdir=\${prefix}/lib
includedir=\${prefix}/include
Name: libsass
Description: libsass ABI implemented on rust-sass
Version: 3.6.6
Cflags: -I\${includedir}
Libs: -L\${libdir} -lsass
EOF

# Hermeticity (plan G27): cargo sets the dylib's install-name to an absolute
# target/.../deps path, which macOS ld records verbatim. Rewrite the copy's
# ID to @rpath so the binding's rpath below governs.
if [ "$OS" = "Darwin" ]; then
    install_name_tool -id @rpath/libsass.dylib "$OUT/lib/$(basename "$LIB_SRC")"
fi

# 3. Build node-sass against the tree.
if [ ! -d "$ROOT/node-sass/node_modules" ]; then
    echo "error: node-sass/node_modules missing — run 'npm install' in node-sass/ first" >&2
    exit 1
fi
cd "$ROOT/node-sass"
export SKIP_SASS_BINARY_DOWNLOAD_FOR_CI=1
export SASS_FORCE_BUILD=1
export LIBSASS_EXT=yes
export CPLUS_INCLUDE_PATH="$OUT/include"
export LIBSASS_CFLAGS=""
export LIBSASS_LDFLAGS=""
export LIBSASS_LIBRARY="-L$OUT/lib -Wl,-rpath,$OUT/lib -lsass"
node scripts/build.js --force

# 4. Smoke: the binding loads and renders through our library.
node -e "
const sass = require('./lib');
console.log(sass.info.split('\n')[0]);
const out = sass.renderSync({ data: 'a { b: c; }' });
console.log(JSON.stringify(out.css.toString()));
"

if [ "${1:-}" = "--run" ]; then
    # Acceptance subset (plan §7 step 9): full api.js + the non-watch CLI
    # render blocks. Error-text/precision goldens that contradict Dart
    # behavior are triaged, not fixed (plan D1).
    npx mocha test/api.js
    npx mocha test/cli.js --grep "watch|follow" --invert
fi
