#!/bin/sh
#
# Time "angel --render" on every saved fixture page (rendering only:
# no network, no cache), slowest first.  The budget is about 50 ms
# per page on the household server.
#
# usage: tests/bench.sh [runs]     (default 20)
#

set -u
here=$(cd "$(dirname "$0")/.." && pwd)
angel=$here/target/release/angel
runs=${1:-20}

(cd "$here" && cargo build --release -q) || exit 1

for html in "$here"/tests/fixtures/*/*.html; do
    "$angel" --render "$html" --bench "$runs" 2>&1 >/dev/null |
        sed "s|^$here/tests/fixtures/||"
done | sort -t' ' -k8,8nr
