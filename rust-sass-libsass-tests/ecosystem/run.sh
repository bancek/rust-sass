#!/bin/sh
# Ecosystem gate runner: builds each Dockerfile target and reports PASS/FAIL.
#
# Usage:
#   rust-sass-libsass-tests/ecosystem/run.sh            # all bindings
#   rust-sass-libsass-tests/ecosystem/run.sh lua nim    # subset
#
# Each target ends in a compile-string assertion; a failing smoke fails the
# build, so the image receipt only exists on success. See Dockerfile.
set -eu

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ALL="python go ruby perl dotnet java lua php rust nim node sassc"
TARGETS="${*:-$ALL}"
FAIL=""

for t in $TARGETS; do
    if docker build -f "$ROOT/rust-sass-libsass-tests/ecosystem/Dockerfile" \
            --target "$t" -t "rust-sass-ecosystem:$t" "$ROOT" >/tmp/ecosystem-"$t".log 2>&1; then
        echo "PASS $t"
    else
        echo "FAIL $t (see /tmp/ecosystem-$t.log)"
        FAIL="$FAIL $t"
    fi
done

if [ -n "$FAIL" ]; then
    echo "failed:$FAIL" >&2
    exit 1
fi
echo "all green"
