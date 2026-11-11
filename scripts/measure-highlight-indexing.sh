#!/bin/bash
# Browsing while the browser's own semantic indexing job runs, on the same
# two CPUs and under the same memory limit (docs/bench/highlight.md).
#
#   scripts/measure-highlight-indexing.sh [RUNS]
#
# The session starts the job from the command palette (plan, typed `yes`),
# waits for the server to come up, runs the navigation of
# scripts/measure-highlight-run.sh, and quits, which cancels the job. It
# works on a copy of the caches (target/measure/cache-indexing); the files
# the job finishes meanwhile are written to that copy only.
set -u
cd "$(dirname "$0")/.."
RUNS=${1:-3}
M=$PWD/target/measure
DIR=$HOME/Codes/haagerup/lean; KEY=HaagerupIzumi-0775d1c6f2b6
G="$HOME/.local/bin/research-safe-run --threads 2 --memory-mib 6144 --limit-mib 49152 --wait --"
mkdir -p "$M/bin"; cp target/release/slim "$M/bin/slim-indexing"
rm -rf "$M/cache-indexing"; mkdir -p "$M/cache-indexing/projects/$KEY"
cp "$M/cache/projects/$KEY"/* "$M/cache-indexing/projects/$KEY/"
# Give the job something to do: forget the tokens of one directory in the copy.
python3 - "$M/cache-indexing/projects/$KEY/highlight.sqlite" <<'PY'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
n = db.execute("delete from semantic where path like 'HaagerupIzumi/Analytic/%'").rowcount
db.commit()
print(f"removed {n} semantic records from the cache copy so that the job has work")
PY
OUT=$M/highlight-haagerup-browser-indexing.md; : > "$OUT"
START="Esc Colon highlighting: Space build ~0.5 Enter ~4 yes Enter ~6"
TRACE="Slash CtrlU HasResidue.hasResidueGen ~0.8 Enter ~0.6 s ~0.8 j*60 k*30 End ~0.5 Home ~0.3 \
Slash CtrlU A_zero_zero ~0.8 Enter ~0.6 s ~0.8 j*30 \
Slash CtrlU s2opIdx ~0.8 Enter ~0.6 s ~0.8 j*30 End ~0.8 k*20 Home ~0.3 \
Slash CtrlU add_zero ~0.8 Enter ~0.6 s ~0.8 j*20 \
LBracket ~0.3 LBracket ~0.3 LBracket ~0.3 RBracket ~0.3 RBracket ~0.3 RBracket ~0.3 LBracket ~0.2 RBracket ~0.2 LBracket ~0.2 RBracket ~0.2 \
Resize:80x24 j*10 Resize:200x60 j*10 Resize:120x34 k*20 F5 ~0.3 F5 ~0.3 j*10 F6 ~0.5 Esc"
BEFORE=$(pgrep -u "$USER" -fc "bin/lake serve")
SLIM_CACHE_DIR=$M/cache-indexing SLIM_HL_CGROUP=none $G python3 scripts/measure-views.py \
  "haagerup: syntax + semantic while the browser's own indexing job runs (same 2 CPUs)" "$M/bin/slim-indexing" "$START $TRACE" \
  --runs "$RUNS" --settle 3 -- "$DIR" --highlight semantic >> "$OUT" 2>>"$M/err.log"
sleep 1
AFTER=$(pgrep -u "$USER" -fc "bin/lake serve")
echo "language-server processes of this user before the sessions: $BEFORE, after: $AFTER (equal means the browser's servers are gone)" >> "$OUT"
python3 - "$M/cache/projects/$KEY/highlight.sqlite" "$M/cache-indexing/projects/$KEY/highlight.sqlite" >> "$OUT" <<'PY'
import sqlite3, sys
n = [sqlite3.connect(p).execute("select count(*) from semantic").fetchone()[0] for p in sys.argv[1:3]]
runs = sqlite3.connect(sys.argv[2]).execute("select outcome, summary from run order by id desc limit 3").fetchall()
print(f"semantic records in the cache copy: {n[0]} before, {n[1]} after the sessions")
for o, s in runs:
    print(f"  run: {o}: {s}")
PY
echo "wrote $OUT"
