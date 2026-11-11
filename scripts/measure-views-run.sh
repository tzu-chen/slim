#!/bin/bash
# The measurement runs recorded in docs/bench/m2-views.md. Needs: release builds of this tree
# (target/release) and of milestone 1 (target/m1-target), copies of the project indexes in
# target/measure/cache, and scratch maps in target/measure (see docs/benchmarks.md).
set -u
cd "$(dirname "$0")/.."
M=target/measure
export SLIM_CACHE_DIR=$PWD/$M/cache
G="$HOME/.local/bin/research-safe-run --threads 2 --memory-mib 4096 --limit-mib 8192 --wait --"
OUT=$M/raw.md; : > $OUT
m() { $G python3 scripts/measure-views.py "$@" >> $OUT 2>>$M/err.log; }
SEARCH="Slash o d d _ o r d e r ~0.8 Enter ~0.6 j*20 k*20 1 ~0.4"
m "CFSG, milestone-1 binary, search session" target/m1-target/release/slim "$SEARCH" -- ~/Codes/CFSG
m "CFSG, milestone-2 binary, same search session (no map)" target/release/slim "$SEARCH" -- ~/Codes/CFSG
DSEARCH="Slash s e a r c h W i t h i n ~0.6 Enter ~0.4 j*20 k*20 1 ~0.3 2 ~0.3 3 ~0.3"
m "demo, milestone-1 binary, search session" target/m1-target/release/slim "$DSEARCH" -- demo
m "demo, milestone-2 binary, same search session" target/release/slim "$DSEARCH" -- demo
DVIEWS="j*14 k*14 j Star ~0.2 A ~0.4 j Space ~0.2 j*3 Space ~0.2 r ~0.3 r ~0.3 j Space ~0.3 j*8 k*8 H ~0.3 r ~0.3 u ~0.3 u r ~0.2 T ~0.3 M ~0.2 j*3 Space j*10 k*10 Tab j*10 Tab"
m "demo, milestone-2, map + goal + trails session (semantic, bundled map)" target/release/slim "$DVIEWS" -- demo
CV="~0.5 j Star ~0.3 j*20 k*20 j*3 Space ~0.3 j*10 k*10 A ~0.5 r ~1.2 j Space ~1.0 j*6 k*6 r ~0.4 M ~0.3 End Space ~2.0 j Space ~0.5 j*30 k*30"
m "CFSG, milestone-2, map + goal session (source-only; generated skeleton + 4 measurement pointers)" target/release/slim "$CV" --settle 3 -- ~/Codes/CFSG --map $PWD/$M/CFSG-measure.toml
m "HaagerupIzumi, milestone-2, map + goal session (source-only; generated skeleton + 3 measurement pointers)" target/release/slim "$CV" -- ~/Codes/haagerup/lean --map $PWD/$M/haagerup-measure.toml
