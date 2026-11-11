#!/bin/bash
# The pseudo-terminal sessions recorded in docs/bench/highlight.md.
#
#   scripts/measure-highlight-run.sh haagerup|cfsg [LABEL_SUFFIX]
#
# Needs: this tree's release build (target/release), the build of the commit
# before highlighting (target/m2-target, see docs/bench/highlight.md), and
# copies of the project's caches under target/measure/cache (made with an
# online SQLite backup; the user's own cache is never touched). Every run
# goes through the machine's resource guard on two CPUs.
set -u
cd "$(dirname "$0")/.."
WHICH=${1:-haagerup}
SUFFIX=${2:-}
M=$PWD/target/measure
G="$HOME/.local/bin/research-safe-run --threads 2 --memory-mib 4096 --limit-mib 8192 --wait --"
# Copies, so that the tree can be rebuilt while sessions run.
mkdir -p "$M/bin"
cp target/release/slim "$M/bin/slim-new"
cp target/m2-target/release/slim "$M/bin/slim-before"
NEW=$M/bin/slim-new
OLD=$M/bin/slim-before
case $WHICH in
  haagerup)
    DIR=$HOME/Codes/haagerup/lean; KEY=HaagerupIzumi-0775d1c6f2b6
    Q1=HasResidue.hasResidueGen; Q2=A_zero_zero; Q3=s2opIdx; Q4=add_zero ;;
  cfsg)
    DIR=$HOME/Codes/CFSG; KEY=cfsg-78b45ae094c6
    Q1=classification_finite_simple_groups; Q2=odd_order_theorem; Q3=ldataP_L0; Q4=sylow ;;
  *) echo "usage: $0 haagerup|cfsg" >&2; exit 2 ;;
esac
OUT=$M/highlight-$WHICH$SUFFIX.md; : > "$OUT"
# One session: open a declaration, its whole file, scroll, jump to the end
# and back, visit three more files (the third is the project's largest),
# (in the source view each time), go back and forward through them, resize,
# change the syntax theme.
TRACE="Slash CtrlU $Q1 ~0.8 Enter ~0.6 s ~0.8 j*60 k*30 End ~0.5 Home ~0.3 \
Slash CtrlU $Q2 ~0.8 Enter ~0.6 s ~0.8 j*30 \
Slash CtrlU $Q3 ~0.8 Enter ~0.6 s ~0.8 j*30 End ~0.8 k*20 Home ~0.3 \
Slash CtrlU $Q4 ~0.8 Enter ~0.6 s ~0.8 j*20 \
LBracket ~0.3 LBracket ~0.3 LBracket ~0.3 RBracket ~0.3 RBracket ~0.3 RBracket ~0.3 LBracket ~0.2 RBracket ~0.2 LBracket ~0.2 RBracket ~0.2 \
Resize:80x24 j*10 Resize:200x60 j*10 Resize:120x34 k*20 F5 ~0.3 F5 ~0.3 j*10"
# Cache directories: with the cached highlight data, and without any.
rm -rf "$M/cache-nohl"; mkdir -p "$M/cache-nohl/projects/$KEY"
cp "$M/cache/projects/$KEY"/source.sqlite* "$M/cache-nohl/projects/$KEY/"
run() { # label, binary, cache dir, extra args...
  local label=$1 bin=$2 cache=$3; shift 3
  SLIM_CACHE_DIR=$cache $G python3 scripts/measure-views.py "$label" "$bin" "$TRACE" --runs 1 --settle 3 "$@" >> "$OUT" 2>>"$M/err.log"
}
v_old()  { run "$WHICH: build before highlighting (3433d31)" $OLD "$M/cache" -- "$DIR"; }
v_off()  { run "$WHICH: highlighting off" $NEW "$M/cache" -- "$DIR" --highlight off; }
v_syn()  { rm -f "$M/cache-nohl/projects/$KEY"/highlight.sqlite*
           run "$WHICH: syntax only, nothing cached on disk (every file tokenized in this session)" $NEW "$M/cache-nohl" -- "$DIR" --highlight syntax; }
v_sem()  { run "$WHICH: syntax + semantic, spans and semantic tokens cached on disk (new process each run)" $NEW "$M/cache" -- "$DIR" --highlight semantic; }
# A first session fills the on-disk span cache for the measured ones.
SLIM_CACHE_DIR=$M/cache $G python3 scripts/measure-views.py "warm-up (not reported)" $NEW "$TRACE" --runs 1 --settle 3 -- "$DIR" > /dev/null 2>>"$M/err.log"
# Three rounds, the order changed each time.
v_old; v_off; v_syn; v_sem
v_sem; v_syn; v_off; v_old
v_off; v_sem; v_old; v_syn
# Idle CPU over a minute in the coloured source view, and before highlighting.
SLIM_CACHE_DIR=$M/cache $G python3 scripts/measure-views.py "$WHICH: idle minute, syntax + semantic" $NEW "$TRACE" --runs 1 --settle 3 --idle 60 -- "$DIR" --highlight semantic >> "$OUT" 2>>"$M/err.log"
SLIM_CACHE_DIR=$M/cache $G python3 scripts/measure-views.py "$WHICH: idle minute, build before highlighting" $OLD "$TRACE" --runs 1 --settle 3 --idle 60 -- "$DIR" >> "$OUT" 2>>"$M/err.log"
echo "wrote $OUT"
