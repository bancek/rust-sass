#!/bin/bash
# tools/heaptrack-leaks.sh [options] [SPEC_SUBPATH|SPEC_PATH]
#
# Run the spec runner under heaptrack and report which leaked blocks belong to
# rust-sass. See docs/CONTRIBUTING.md ("Debugging a leak-gate failure").
#
# The counting gate (rust-sass-spec --features leak-gate) tells you *that* bytes
# were retained; this script tells you *where* they were allocated. Use the
# debug build: release inlining removes the frames the verdict depends on.
#
# A block is OURS when the stack reaches a `rust-sass/src/...` frame before any
# `rust-sass-spec/src/...` frame, i.e. rust-sass is the closest allocating
# frame. HARNESS when the runner comes first (regex caches, normalisation) or
# every frame is runtime/dependency code. Anything else is UNKNOWN; heaptrack
# truncates deep stacks, so inspect leaks-demangled.txt for those.
#
# Artifacts go to /tmp/rust-sass/<run>/ (one directory per run, never pruned):
#   heap.zst, heap-print.txt, leaks.txt, leaks-demangled.txt, attribution.txt,
#   summary.txt, meta.txt (+ diff.txt with --diff)
set -eu

die() {
  echo "heaptrack-leaks: $*" >&2
  exit 1
}

usage() {
  sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
  cat <<'EOF'

Arguments:
  SPEC_SUBPATH   path under sass-spec/spec to run (default: whole suite)
  SPEC_PATH      absolute path to a spec directory or .hrx file

Options:
  -r, --release        release build (fast, but frames are inlined away)
  -o, --out DIR        artifact directory (default /tmp/rust-sass/heaptrack-<stamp>)
  -d, --diff ZST       also diff against a previous run's heap.zst
  -s, --strict         exit 1 when any leaked block is classified OURS
  -h, --help           show this help
EOF
}

PROFILE=debug
OUT=""
DIFF=""
STRICT=0
SPEC_ARG=""

while [ $# -gt 0 ]; do
  case "$1" in
    -r | --release) PROFILE=release ;;
    -o | --out)
      [ $# -ge 2 ] || die "--out needs a directory"
      OUT="$2"
      shift
      ;;
    -d | --diff)
      [ $# -ge 2 ] || die "--diff needs a previous heap.zst"
      DIFF="$2"
      shift
      ;;
    -s | --strict) STRICT=1 ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*) die "unknown option '$1' (try --help)" ;;
    *)
      [ -z "$SPEC_ARG" ] || die "at most one spec path argument"
      SPEC_ARG="$1"
      ;;
  esac
  shift
done

cd "$(dirname "$0")/.."
REPO=$PWD

for tool in heaptrack heaptrack_print c++filt; do
  command -v "$tool" >/dev/null 2>&1 || die "'$tool' not found on PATH"
done

STAMP=$(date +%Y%m%d-%H%M%S)
[ -n "$OUT" ] || OUT="/tmp/rust-sass/heaptrack-$STAMP"
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)

# --- spec target --------------------------------------------------------------

SPEC_ROOT="$REPO/sass-spec/spec"
[ -d "$SPEC_ROOT" ] || die "sass-spec checkout not found at $SPEC_ROOT"

SPEC_ENV=()
RUN_LABEL="full suite"
if [ -n "$SPEC_ARG" ]; then
  if [ "${SPEC_ARG#/}" != "$SPEC_ARG" ]; then
    [ -e "$SPEC_ARG" ] || die "spec path '$SPEC_ARG' does not exist"
    SPEC_ENV=(SASS_SPEC_PATH="$SPEC_ARG")
  else
    [ -e "$SPEC_ROOT/$SPEC_ARG" ] || die "'$SPEC_ARG' is not under $SPEC_ROOT"
    SPEC_ENV=(SASS_SPEC_PATH="$SPEC_ROOT" SASS_SPEC="$SPEC_ARG")
  fi
  RUN_LABEL="$SPEC_ARG"
fi

# --- build --------------------------------------------------------------------

PROFILE_FLAG=()
BUILD_DIR=target/debug
if [ "$PROFILE" = release ]; then
  PROFILE_FLAG=(--release)
  BUILD_DIR=target/release
fi

echo "heaptrack-leaks: building the $PROFILE runner for $RUN_LABEL" >&2
cargo test -p rust-sass-spec --test runner_test "${PROFILE_FLAG[@]}" --no-run >/dev/null

BIN=$(ls -t "$BUILD_DIR"/deps/runner_test-* 2>/dev/null | grep -v '\.d$' | head -n 1 || true)
[ -n "$BIN" ] && [ -x "$BIN" ] || die "no runner_test binary under $BUILD_DIR/deps"

# --- run ----------------------------------------------------------------------

echo "heaptrack-leaks: heaptracking $BIN (artifacts: $OUT)" >&2
rm -f "$OUT/heap.zst"
env "${SPEC_ENV[@]}" \
  heaptrack -o "$OUT/heap" "$BIN" --ignored >/dev/null 2>"$OUT/heaptrack.log" \
  || die "heaptrack failed (see $OUT/heaptrack.log)"

[ -f "$OUT/heap.zst" ] || die "heaptrack produced no $OUT/heap.zst"

heaptrack_print -l -f "$OUT/heap.zst" >"$OUT/heap-print.txt" 2>&1 \
  || die "heaptrack_print failed (see $OUT/heap-print.txt)"

awk '/MEMORY LEAKS/ { found = 1 } found' "$OUT/heap-print.txt" >"$OUT/leaks.txt"
c++filt <"$OUT/leaks.txt" >"$OUT/leaks-demangled.txt"

# --- classify -----------------------------------------------------------------
#
# `heaptrack_print -l` prints each leak as a size line followed by its
# allocation stack (faulting frame first, then callers). The verdict is decided
# by the first `at <path>` line that names either crate; `heaptrack` truncates
# deep stacks, so some blocks name no crate at all and stay UNKNOWN.

awk '
  function emit() {
    if (size == "") return
    if (verdict == "OURS") {
      printf "%-10s OURS     %s\n", size, where
      n_ours++
    } else if (verdict == "HARNESS") {
      printf "%-10s harness\n", size
    } else {
      printf "%-10s UNKNOWN  %s\n", size, (where == "" ? "-" : where)
      n_unknown++
    }
    size = ""
  }
  # A size line starts a block; the stack follows until the next one.
  /leaked over/ {
    emit()
    size = $1
    verdict = ""
    where = ""
    next
  }
  size == "" { next }
  # Only frame lines carry "at <path>": decide on the first one that is
  # either crate.
  verdict == "" && /^ *at rust-sass\/src\// {
    verdict = "OURS"
    where = $0
    sub(/^ *at /, "", where)
    sub(/:[0-9]+ *$/, "", where)
    next
  }
  verdict == "" && /^ *at rust-sass-spec\/src\// {
    verdict = "HARNESS"
    next
  }
  where == "" && /^ *at rust-sass\/src\// {
    where = $0
    sub(/^ *at /, "", where)
    sub(/:[0-9]+ *$/, "", where)
  }
  END {
    emit()
    printf "%d block(s) OURS, %d UNKNOWN\n", n_ours, n_unknown
  }
' "$OUT/leaks-demangled.txt" >"$OUT/attribution.txt"

{
  echo "# heaptrack leak attribution"
  echo
  echo "run:     $RUN_LABEL"
  echo "profile: $PROFILE"
  echo "binary:  $BIN"
  echo
  grep -E '^total memory leaked:' "$OUT/leaks.txt" 2>/dev/null || echo "total:   (not reported)"
  echo
  cat "$OUT/attribution.txt"
} >"$OUT/summary.txt"

{
  echo "date:      $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "head:      $(git rev-parse HEAD)"
  echo "dirty:     $(git status --porcelain | wc -l) modified path(s)"
  echo "spec:      $RUN_LABEL"
  echo "profile:   $PROFILE"
  echo "runner:    $BIN"
  echo "artifacts: $OUT"
  echo "demangler: c++filt"
} >"$OUT/meta.txt"

# --- optional diff ------------------------------------------------------------

if [ -n "$DIFF" ]; then
  [ -f "$DIFF" ] || die "diff target '$DIFF' does not exist"
  if ! heaptrack_print -f "$OUT/heap.zst" -d "$DIFF" >"$OUT/diff.txt" 2>&1; then
    echo "heaptrack-leaks: heaptrack_print -d failed (see $OUT/diff.txt)" >&2
  fi
fi

# --- report -------------------------------------------------------------------

cat "$OUT/summary.txt"
echo
echo "artifacts: $OUT"

if [ "$STRICT" -eq 1 ] && grep -q ' OURS ' "$OUT/attribution.txt"; then
  echo "heaptrack-leaks: leaked blocks attributable to rust-sass (strict)" >&2
  exit 1
fi
