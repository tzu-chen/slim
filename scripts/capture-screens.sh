#!/bin/sh
# Captures real terminal screens of slim for the documentation.
# tmux is used only as a terminal emulator on a private socket; every
# session is killed at the end.
# Usage: scripts/capture-screens.sh [CFSG_DIR]    (all screens)
#        scripts/capture-screens.sh --themes-only (the colour theme PNGs)
#        scripts/capture-screens.sh --views-only  (map, goal explorer and trails)
#        scripts/capture-screens.sh --highlight-only (syntax and semantic colours)
#        scripts/capture-screens.sh --ym YM_DIR      (the README walkthrough, on a checkout of
#                                                     https://github.com/yang-mills-group/YM)
# The demo ships a project map and opens on it, so searches start with `/`.
#
# Screens show paths (the project in the title bar, the demo's cache). So that
# they do not show this checkout or a home directory, everything the screens
# name lives under the fixed scratch directory /tmp/slim: user data, the cache
# and a copy of fixtures/demo. Give YM_DIR and CFSG_DIR at neutral paths too.
set -eu
cd "$(dirname "$0")/.."
BIN=./target/release/slim
OUT=docs/screenshots
SOCK=slim-capture
WORK=/tmp/slim
rm -rf "$WORK" && mkdir -p "$WORK/fixtures"
DATA=$WORK/data
export SLIM_CACHE_DIR=$WORK/cache
DEMO=$WORK/fixtures/demo
cp -a fixtures/demo "$DEMO"
T="tmux -L $SOCK"
# The confirmation screen names the toolchain, which lives in the home
# directory of whoever runs this: the home directory is shown as /home/someone.
redact() { for f in "$OUT"/*.txt; do sed -i "s|$HOME|/home/someone|g" "$f"; done; }
cleanup() { $T kill-server 2>/dev/null || true; rm -rf "$WORK"; redact; }
trap cleanup EXIT
mkdir -p "$OUT"

run() { # name cols rows cmd...  (fresh user data per session: no restored state)
  name=$1; cols=$2; rows=$3; shift 3
  mkdir -p "$DATA/$name"
  $T new-session -d -s "$name" -x "$cols" -y "$rows" "env SLIM_DATA_DIR=$DATA/$name $*"
  sleep 1.5
}
keys() { $T send-keys -t "$1" "$2"; sleep "${3:-0.6}"; }
snap() { $T capture-pane -p -t "$1" > "$OUT/$2.txt"; }

# Syntax and semantic highlighting, on the demo library as a project
# directory. Semantic colours need Lean 4.33.1 and fixtures/demo built with
# it (`slim highlight build` runs here, into a scratch cache); without
# them the screens show syntax colours only.
highlight() {
  HC="$DATA/hl-cache"
  $BIN highlight build "$DEMO" --yes --quiet --cache-dir "$HC" >/dev/null 2>&1 || echo "note: no semantic tokens (Lean 4.33.1 or the fixture build is missing): syntax colours only"
  for th in lean-dark lean-light; do
    run "hl-$th" 150 40 "COLORTERM=truecolor $BIN $DEMO --cache-dir $HC --theme $th"
    sleep 1
    keys "hl-$th" / 0.2; keys "hl-$th" "searchWithin_succ" 0.8; keys "hl-$th" Enter 1; keys "hl-$th" s 1.2
    $T capture-pane -e -N -p -t "hl-$th" > "$DATA/hl-$th.ans"
    python3 scripts/ansi-screenshot.py "$DATA/hl-$th.ans" "$OUT/highlight-source-$th.png"
    if [ "$th" = lean-dark ]; then
      snap "hl-$th" highlight-1-source
      keys "hl-$th" F6 0.6; snap "hl-$th" highlight-2-details; keys "hl-$th" Escape 0.3
      keys "hl-$th" : 0.3; keys "hl-$th" "highlighting: build" 0.5; keys "hl-$th" Enter 2.5; snap "hl-$th" highlight-3-confirm
      keys "hl-$th" Escape 0.4
    fi
    keys "hl-$th" q 0.5
  done
}
if [ "${1:-}" = "--highlight-only" ]; then highlight; echo "captured: highlight-* in $OUT"; exit 0; fi

# The README walkthrough: the YM repository with the example map (source-only,
# scanned into a scratch cache; nothing is written into the checkout).
ym() {
  run y1 110 32 "$BIN $1 --map examples/YM/lean-view.toml"
  sleep 3
  keys y1 j 0.2; keys y1 j 0.8; snap y1 ym-1-map
  keys y1 "*" 0.4; keys y1 A 0.8; keys y1 j 0.8; snap y1 ym-2-goal
  keys y1 M 0.5; keys y1 j 0.2; keys y1 j 0.2; keys y1 j 0.2; keys y1 j 0.4; keys y1 S 1; snap y1 ym-3-skim
  keys y1 Escape 0.3; keys y1 T 0.8; snap y1 ym-4-trails
  keys y1 q 0.5
}
if [ "${1:-}" = "--ym" ]; then ym "$2"; echo "captured: ym-*.txt in $OUT"; exit 0; fi

# Map, goal explorer and reading trails (milestone 2), on the bundled demo.
views() {
  run v1 100 30 "$BIN demo"
  snap v1 m2-1-map
  # Relations > Reflexive-transitive closure > head (placed in two groups).
  keys v1 j 0.2; keys v1 j 0.2; keys v1 j 0.2; keys v1 j 0.3; keys v1 Space 0.4; keys v1 j 0.2; keys v1 j 0.3; keys v1 Space 0.4
  keys v1 j 0.2; keys v1 j 0.2; keys v1 j 0.2; keys v1 j 0.6
  snap v1 m2-2-map-landmark
  keys v1 Home 0.3; keys v1 j 0.4 # the first objective
  keys v1 "*" 0.4; keys v1 A 0.6; keys v1 j 0.2; keys v1 j 0.2; keys v1 j 0.2; keys v1 j 0.3; keys v1 Space 0.5; keys v1 j 0.5
  snap v1 m2-3-goal-curated
  keys v1 a 0.3; keys v1 "one step is a chain" 0.3; keys v1 Enter 0.5
  keys v1 r 0.4; keys v1 r 0.6; keys v1 Home 0.3
  snap v1 m2-4-goal-lean
  keys v1 c 0.3; keys v1 "Demo.Rel.ReflTransGen.trans" 0.3; keys v1 Enter 1; keys v1 j 0.2; keys v1 j 0.4
  snap v1 m2-5-connection
  keys v1 T 0.6; keys v1 j 0.2; keys v1 j 0.2; keys v1 Enter 0.8; keys v1 j 0.2; keys v1 s 0.3; keys v1 j 0.2; keys v1 s 0.3; keys v1 s 0.3; keys v1 j 0.2; keys v1 j 0.2; keys v1 j 0.2; keys v1 j 0.2; keys v1 j 0.2; keys v1 j 0.5
  snap v1 m2-6-trails
  keys v1 q 0.5

  run v2 60 20 "$BIN demo --ascii"
  snap v2 m2-7-narrow-map
  keys v2 j 0.4; keys v2 Tab 0.4
  snap v2 m2-8-narrow-detail
  keys v2 q 0.5

  # Source-only, no map: a scan of the demo sources (no snapshot, no Lean).
  SRC="$DATA/src-only"; mkdir -p "$SRC"; cp -r fixtures/demo/SlimDemo fixtures/demo/SlimDemo.lean fixtures/demo/lakefile.toml "$SRC/"
  run v3 100 30 "env SLIM_CACHE_DIR=$DATA/cache $BIN $SRC"
  sleep 1
  keys v3 Escape 0.2; keys v3 M 0.5; keys v3 j 0.2; keys v3 j 0.2; keys v3 j 0.2; keys v3 j 0.2; keys v3 Space 0.8; keys v3 j 0.3; keys v3 Space 0.5; keys v3 j 0.4
  snap v3 m2-9-source-only-generated-outline
  keys v3 q 0.5
}
if [ "${1:-}" = "--views-only" ]; then views; echo "captured: m2-*.txt in $OUT"; exit 0; fi

# Colour themes: ANSI captures (-e, -N keeps trailing cells) rendered to PNG
# by scripts/ansi-screenshot.py (needs Pillow and fontTools).
for th in lean-dark lean-light; do
  run "$th" 120 34 "COLORTERM=truecolor $BIN demo --theme $th"
  keys "$th" / 0.2; keys "$th" "searchWithin_correct" 0.8; keys "$th" Enter 1; keys "$th" 3 0.6; keys "$th" j 0.4
  $T capture-pane -e -N -p -t "$th" > "$DATA/$th.ans"
  python3 scripts/ansi-screenshot.py "$DATA/$th.ans" "$OUT/theme-$th.png"
  keys "$th" q 0.5
done
if [ "${1:-}" = "--themes-only" ]; then echo "captured: theme PNGs in $OUT"; exit 0; fi

run s1 100 30 "$BIN demo"
keys s1 / 0.2; keys s1 "decision procedure" 1; snap s1 demo-1-search-docs
keys s1 C-u 0.2; keys s1 "searchWithin_correct" 0.8; keys s1 Enter 1; snap s1 demo-2-declaration
keys s1 1 0.6; snap s1 demo-3-uses
keys s1 3 0.6; keys s1 j 0.3; snap s1 demo-4-axioms
keys s1 2 0.6; snap s1 demo-5-used-by
keys s1 "?" 0.5; snap s1 demo-6-help
keys s1 q 0.5; keys s1 q 0.5

run s2 80 24 "$BIN demo"
keys s2 / 0.2; keys s2 "reach_trans" 0.8; keys s2 Enter 1; snap s2 demo-7-80x24
keys s2 Tab 0.3; keys s2 Tab 0.3; keys s2 3 0.6; snap s2 demo-8-80x24-right
keys s2 q 0.5

run s3 100 30 "$BIN demo --ascii --theme contrast"
keys s3 / 0.2; keys s3 "oracle_axiom" 0.8; keys s3 Enter 1; snap s3 demo-9-ascii
keys s3 q 0.5

if [ "${1:-}" != "" ] && [ -d "$1" ]; then
  run s4 120 32 "$BIN $1"
  sleep 2
  keys s4 / 0.2; keys s4 "odd_order_theorem" 1; keys s4 Enter 1.5; snap s4 cfsg-source-only
  keys s4 Escape 0.2; keys s4 O 1.5; snap s4 cfsg-outline
  keys s4 q 0.5
fi
views
highlight
echo "captured: $(ls "$OUT" | wc -l) screens in $OUT"
