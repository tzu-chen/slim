#!/bin/sh
# Rebuilds the bundled demo snapshot from real compiler data.
# The fixture is self-authored, reviewed code with no dependencies; it is
# built with the pinned toolchain's own lake binary (no elan switching, no
# network), exported in the bubblewrap sandbox, and validated against
# Lean's own axiom report.
#
# A snapshot records where its project was (`root_hint`), so the bundled one
# is exported from a copy of the fixture at a fixed neutral path, not from
# this checkout: the file does not depend on who regenerates it.
set -eu
cd "$(dirname "$0")/.."
TC_NAME=$(cat fixtures/demo/lean-toolchain)
TC="${ELAN_HOME:-$HOME/.elan}/toolchains/$(echo "$TC_NAME" | sed 's|/|--|; s|:|---|')"
[ -x "$TC/bin/lake" ] || { echo "toolchain $TC_NAME is not installed ($TC)"; exit 1; }
cargo build --release
# The in-repo copy is built too: tests and screenshots use it.
(cd fixtures/demo && PATH="$TC/bin:/usr/bin:/bin" "$TC/bin/lake" build)
NEUTRAL=/tmp/slim/fixtures/demo
rm -rf "$NEUTRAL" && mkdir -p "$NEUTRAL"
(cd fixtures/demo && tar -cf - --exclude=.lake --exclude=snapshot --exclude=axiom-report .) | (cd "$NEUTRAL" && tar -xpf -)
(cd "$NEUTRAL" && PATH="$TC/bin:/usr/bin:/bin" "$TC/bin/lake" build)
VER=$(echo "$TC_NAME" | sed 's|.*:||')
OUT="fixtures/demo/snapshot/slim_demo.lean-$VER.jsonl"
./target/release/slim export "$NEUTRAL" --yes --out "$OUT"
IDX=$(mktemp -d)/check.sqlite
./target/release/slim import "$OUT" --index "$IDX" --source-root fixtures/demo
./target/release/slim check-axioms --index "$IDX"
rm -rf /tmp/slim/fixtures
rmdir /tmp/slim 2>/dev/null || true
echo "wrote $OUT"
