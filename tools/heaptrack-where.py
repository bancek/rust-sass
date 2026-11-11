#!/usr/bin/env python3
"""Find where a heap allocation recorded by heaptrack came from.

Heaptrack records an allocation stack per *call site*. `heaptrack_print -l`
prints only the leaked blocks; the same `N leaked over M calls from` header is
also a key into the full stack listing, which is what `-l` truncates away for
blocks whose backtrace could not be resolved (the summaries show `$x`).

Usage:
    tools/heaptrack-where.py <heap.zst> <bytes> [<calls>]

Prints every block whose leaked total matches <bytes> (and, when given, its
call count), with the full demangled allocation stack.
"""
import re
import subprocess
import sys

HEADER = re.compile(r"^([0-9.]+[KMG]?)(?:B)? leaked over ([0-9]+) calls from:?$")


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    zst, want_bytes = sys.argv[1], sys.argv[2]
    want_calls = sys.argv[3] if len(sys.argv) > 3 else None

    out = subprocess.run(
        ["heaptrack_print", "-A", "-f", zst],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    out = subprocess.run(
        ["c++filt"], input=out, check=True, capture_output=True, text=True
    ).stdout

    lines = out.splitlines()
    blocks = []
    current = None
    for line in lines:
        m = HEADER.match(line)
        if m:
            current = {"bytes": m.group(1), "calls": m.group(2), "stack": []}
            blocks.append(current)
        elif current is not None:
            current["stack"].append(line)

    def matches(block):
        return block["bytes"] == want_bytes and (
            want_calls is None or block["calls"] == want_calls
        )

    hits = [b for b in blocks if matches(b)]
    if not hits:
        print(
            f"no block with {want_bytes} bytes"
            + (f" and {want_calls} calls" if want_calls else ""),
            file=sys.stderr,
        )
        return 1

    for hit in hits:
        print(f"=== {hit['bytes']} over {hit['calls']} calls")
        for line in hit["stack"]:
            print(line)
        print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
