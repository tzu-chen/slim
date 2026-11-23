#!/bin/sh
# Records the README's short animation: slim on the VP != VNP proof of
# https://github.com/anthropics/formal-math, from the start page to the main
# theorem's statement unfolded, the contents by dependence and why a lemma is there.
#
# Usage: scripts/record-demo.sh FORMAL_MATH_DIR [dark|light|both]
#
# tmux is used only as a terminal emulator on a private socket. The project is
# copied to the scratch directory /tmp/slim, so that the screens show neither this
# checkout nor a home directory; the cache and the reader's data are scratch too.
# Each step is one frame, captured with its colours, drawn by
# scripts/ansi-screenshot.py and held for the time given; ffmpeg makes the GIF.
# Needs tmux, ffmpeg, python3 with Pillow and fontTools, and the fonts that
# ansi-screenshot.py names.
set -eu
cd "$(dirname "$0")/.."
BIN=$PWD/target/release/slim
SRC=${1:?usage: scripts/record-demo.sh FORMAL_MATH_DIR [dark|light|both]}
WHICH=${2:-both}
OUT=assets/screenshots
SOCK=slim-record
WORK=/tmp/slim
COLS=120
ROWS=32
T="tmux -L $SOCK"
rm -rf "$WORK" && mkdir -p "$WORK/formal-math"
cleanup() { $T kill-server 2>/dev/null || true; [ -n "${KEEP:-}" ] || rm -rf "$WORK"; }
trap cleanup EXIT
cp -a "$SRC/vp-vnp" "$WORK/formal-math/vp-vnp"
rm -rf "$WORK/formal-math/vp-vnp/.lake"
export SLIM_CACHE_DIR=$WORK/cache
P=$WORK/formal-math/vp-vnp
"$BIN" graph build "$P" >/dev/null 2>&1

record() { # theme
  th=$1
  F=$WORK/frames-$th; mkdir -p "$F"; : > "$F/list.txt"
  n=0
  $T kill-session -t rec 2>/dev/null || true
  $T new-session -d -s rec -x "$COLS" -y "$ROWS" "env SLIM_DATA_DIR=$WORK/data-$th COLORTERM=truecolor $BIN $P --theme lean-$th --no-mouse"
  sleep 2
  # frame SECONDS: capture what is on screen now and hold it that long
  frame() {
    n=$((n + 1)); f=$(printf '%s/%03d' "$F" "$n")
    $T capture-pane -e -N -p -t rec > "$f.ans"
    $T capture-pane -p -t rec > "$f.txt"
    printf "file '%s.png'\nduration %s\n" "$f" "$1" >> "$F/list.txt"
  }
  key() { $T send-keys -t rec "$1"; sleep "${2:-0.7}"; }

  frame 1.8                       # the start page: what the project ends in
  key Enter 1.2; frame 0.9        # the main theorem
  key u 1; frame 1.3              # its statement, one name unfolded
  key u 1; frame 2.4              # ... down to Mathlib's vocabulary
  key '*' 0.6                     # pin it as the goal
  key D 1.2; frame 1.0            # contents by dependence, from the goal
  key Right 0.9; frame 0.8
  key j 0.2; key Right 0.9; frame 0.8
  key j 0.2; key Right 0.9; frame 0.8
  key j 0.2; key Right 0.9; frame 1.6
  key W 1.2; frame 3.2            # why the selected lemma is there
  key Escape 0.2; key q 0.3
  # ffmpeg's concat reader wants the last file named once more.
  printf "file '%s.png'\n" "$(printf '%s/%03d' "$F" "$n")" >> "$F/list.txt"
  [ -n "${DRY:-}" ] && return 0
  for a in "$F"/*.ans; do python3 scripts/ansi-screenshot.py "$a" "${a%.ans}.png"; done
  ffmpeg -loglevel error -y -f concat -safe 0 -i "$F/list.txt" -fps_mode vfr \
    -vf "split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" \
    "$OUT/reading-$th.gif"
  echo "wrote $OUT/reading-$th.gif ($(du -h "$OUT/reading-$th.gif" | cut -f1), $n frames)"
}
case "$WHICH" in
  both) record dark; record light ;;
  dark | light) record "$WHICH" ;;
  *) echo "theme: dark, light or both" >&2; exit 2 ;;
esac
