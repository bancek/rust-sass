#!/bin/sh
# sassc smoke: compile a string via the sassc CLI linked against our libsass.
# Usage: smoke.sh <sassc-binary>
set -eu
printf '$color: red; .foo { color: $color; }\n' > /tmp/sassc-smoke.scss
out="$("$1" --style expanded /tmp/sassc-smoke.scss)"
want='.foo {
  color: red;
}'
if [ "$out" != "$want" ]; then
    echo "got: $out" >&2
    exit 1
fi
echo "sassc SMOKE-OK"
