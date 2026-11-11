#!/bin/bash
# Runs semantic-highlight indexing with the real Lean language server of
# several toolchains on a copy of the demo fixture, and checks the result.
#
#   scripts/test-highlight-toolchains.sh [TOOLCHAIN_VERSION...]
#
# For each version (default: the tested list) that is installed under elan:
# copies fixtures/demo without build output to a scratch directory, pins the
# toolchain, builds the copy with that toolchain's own `lake build` (the
# fixture has no dependencies, so nothing is downloaded), then runs
# `slim highlight build` in the sandbox and compares against what Lean
# reports. Uses the machine's resource guard when it is present. Nothing in
# the repository is modified.
set -u
cd "$(dirname "$0")/.."
BIN=$PWD/target/release/slim
VERSIONS=("$@"); [ ${#VERSIONS[@]} -eq 0 ] && VERSIONS=(v4.16.0 v4.21.0 v4.28.0 v4.33.1 v4.35.0)
GUARD=""; [ -x "$HOME/.local/bin/research-safe-run" ] && GUARD="$HOME/.local/bin/research-safe-run --threads 2 --memory-mib 4096 --limit-mib 49152 --wait --"
WORK=$(mktemp -d /tmp/slim-hl-matrix.XXXXXX)
trap 'rm -rf "$WORK"' EXIT
fail=0
for v in "${VERSIONS[@]}"; do
  tc="leanprover/lean4:$v"; tcdir="$HOME/.elan/toolchains/leanprover--lean4---$v"
  if [ ! -x "$tcdir/bin/lake" ]; then echo "$v: SKIPPED (toolchain not installed)"; continue; fi
  proj=$WORK/$v/demo; mkdir -p "$proj"
  (cd fixtures/demo && tar -c --exclude=.lake --exclude=snapshot --exclude=axiom-report .) | tar -x -C "$proj"
  echo "$tc" > "$proj/lean-toolchain"
  if ! (cd "$proj" && $GUARD "$tcdir/bin/lake" build > "$WORK/$v/build.log" 2>&1); then
    echo "$v: FAILED to build the fixture copy (see $WORK/$v/build.log)"; tail -5 "$WORK/$v/build.log"; fail=1; continue
  fi
  out=$($GUARD "$BIN" highlight build "$proj" --yes --json --cgroup none --cache-dir "$WORK/$v/cache" 2> "$WORK/$v/index.log" | tail -1)
  python3 - "$v" "$out" "$WORK/$v/cache" <<'PY' || fail=1
import json, sqlite3, sys, glob
v, out, cache = sys.argv[1], sys.argv[2], sys.argv[3]
s = json.loads(out)
db = sqlite3.connect(glob.glob(cache + "/projects/*/highlight.sqlite")[0])
rows = db.execute("select path, status, n_tokens, diag_errors, position_encoding, legend from semantic order by path").fetchall()
legend = json.loads(rows[0][5])["types"]
ok = s["outcome"] == "finished" and s["failed"] == 0 and s["partial"] == 0 and s["lingering_children"] == 0 and all(r[1] == "complete" for r in rows)
print(f"{v}: {'ok' if ok else 'FAILED'} — {s['complete']}/{s['total']} complete, {sum(r[2] for r in rows)} tokens, "
      f"encoding {rows[0][4]}, legend {len(legend)} types, {s['elapsed_s']:.1f} s, shutdown {s['shutdown_ms']} ms clean={s['shutdown_clean']}, "
      f"lingering {s['lingering_children']}; {s['lean_version']}")
sys.exit(0 if ok else 1)
PY
done
exit $fail
