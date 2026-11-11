#!/bin/sh
# Regenerates fixtures/highlight/reference/*.jsonl with VS Code's tokenizer
# (vscode-textmate + vscode-oniguruma installed under target/hl-ref; see
# scripts/textmate-reference.mjs). Run from the repository root.
set -eu
cd "$(dirname "$0")/.."
files=$(ls fixtures/highlight/*.lean | sort)
for theme in dark_plus light_plus hc_black; do
  # shellcheck disable=SC2086
  node scripts/textmate-reference.mjs --theme assets/themes/$theme.json $files > fixtures/highlight/reference/$theme.jsonl
done
ls -la fixtures/highlight/reference
