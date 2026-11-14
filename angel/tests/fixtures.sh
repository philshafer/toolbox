#!/bin/sh
#
# Render every saved fixture page through "angel --render" into a
# work directory, then compare against tests/saved/ when that exists.
# This script never writes to tests/saved/; accepting output is a
# manual step.
#
# usage: tests/fixtures.sh [work-dir]     (default: target/fixtures)
#

set -u
here=$(cd "$(dirname "$0")/.." && pwd)
work=${1:-$here/target/fixtures}
angel=$here/target/release/angel
saved=$here/tests/saved

(cd "$here" && cargo build --release -q) || exit 1
mkdir -p "$work"

fail=0
total=0
for html in "$here"/tests/fixtures/*/*.html; do
    site=$(basename "$(dirname "$html")")
    stem=$(basename "$html" .html)
    out="$work/$site-$stem.html"
    total=$((total + 1))
    # --explain writes the rendering decision (article / headlines /
    # ...) and its measurements next to the output.
    if ! "$angel" --render "$html" --explain > "$out" 2> "$work/$site-$stem.explain"; then
        echo "FAIL render $site/$stem"
        fail=$((fail + 1))
        continue
    fi
    if [ -d "$saved" ]; then
        if [ ! -f "$saved/$site-$stem.html" ]; then
            echo "NEW  $site/$stem"
        elif ! cmp -s "$saved/$site-$stem.html" "$out"; then
            echo "DIFF $site/$stem"
            fail=$((fail + 1))
        fi
    fi
done

echo "$total rendered into $work, $fail failed"
[ "$fail" -eq 0 ]
