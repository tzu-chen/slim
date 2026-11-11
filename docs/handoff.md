# Milestone 1 handoff

Branch `milestone-1` (commits are local; nothing was pushed). Checked 2026-11-10.

## What actually runs

* `slim demo`: the embedded original library (65 user-written declarations in 7 modules, with
  clearly labelled test fixtures) and its semantic snapshot from Lean 4.33.1. Needs no Lean, no
  network, no Lean compilation. The six-step reading session from the brief was driven
  end to end in a real terminal (tmux used as the terminal emulator) and in a pseudo-terminal test:
  search by name and by doc words → full statement/doc/source → follow a **V** dependency and a
  **T** dependent with explanations → `sorryAx` evidence path → back through history → bookmark.
* `slim DIR` on any local tree: source-only index built in the background with progress
  (cancellable), incremental per-file updates, freshness badge; then semantic export from the
  palette (plan shown, typed `yes`, sandboxed, imported, switched to in place).
* `slim export DIR --import` on built projects, with toolchain-specific exporter builds.
  Ran on the fixture built with **v4.16.0, v4.21.0, v4.28.0, v4.33.1, v4.35.0**, all inside
  bubblewrap with a cgroup memory limit; `check-axioms` agreement with Lean's `collectAxioms` for
  every local declaration on all five (4.16/4.21 modulo Lean's 4.28 rule change, verified).
* Source-only indexing of the user's CFSG checkout (229,089 files, 2.36 M candidates, 18.7 s) and
  Stacks checkout (62,975 files, 0.78 M candidates, 10.8 s); browsing both in the TUI.
* Benchmarks: synthetic 100k / 962k edges and the two real indexes (`docs/benchmarks.md`).
* Colour themes after lean-lang.org's palettes: `lean` (default; dark, or light when `COLORFGBG`
  reports a light terminal), `lean-dark`, `lean-light`; the terminal-colour themes `dark`,
  `light`, `contrast`, `mono` remain (`docs/design.md` §11, `docs/screenshots/theme-lean-*.png`).

## Tests executed

`cargo test --release` — **96 passed, 0 failed** (last run 2026-11-10):

* 79 unit tests (incl. debounced previews, reveal-in-outline, selection kept when rows load above it, stale-row handling, mouse hit-testing): lexer (nested/doc comments, raw and interpolated strings, char literals vs
  primes, Unicode and «quoted» identifiers, per-version identifier rules, BOM/CRLF), scanner
  (namespaces, sections, mutual, constructors/fields, attributes, doc on same line, multi-line
  imports, unbalanced-bracket recovery, unterminated comments), text (sanitising escapes and bidi
  controls, cell widths, wrapping never drops text, byte/code-point/UTF-16 conversions),
  snapshot validation and path traversal, import (id order, axioms, truncated/hostile input
  refused with the previous index untouched, interrupted import resumes), SCC axiom analysis
  (cycles, long chains), name search (ranking, filters, fuzzy, cancellation, persistence and
  corruption), source index (delete/rename/add, touch vs change, cancelled build), process
  control (timeout kills grandchildren and reaps, cancellation, rlimits, drop, bounded stderr),
  export (compat selection, untested/missing toolchains refused, planted symlinks refused), user
  data persistence and Markdown export, UI state (history restores panes/query/scroll, previews,
  session round trip, palette), rendering at 7 sizes × 6 themes × ASCII/Unicode × overlays,
  colour themes (WCAG AA contrast of every Lean-theme role in 24-bit and xterm-256 colour, no cell
  left at the terminal's default colours, `COLORFGBG` light/dark detection), on-screen
  sanitisation, doc reflow, synthetic generator determinism.
* 8 fixture tests against the bundled snapshot: provenance, edge meanings (T/V/C, reverse
  edges, coverage), axiom paths by category (indirect `sorryAx`, macro-hidden sorry, custom
  axiom, compiler trust, standard axioms, none), agreement with Lean's raw `#print axioms` for all
  73 names and with `collectAxioms` for all local declarations, Unicode/private/generated names,
  code-point positions, ranking and doc search, scanner recall vs. Lean and no declarations from
  inside comments, source containment.
* 5 lifecycle tests: stale snapshot (edited, deleted module; toolchain and manifest changes),
  moved checkout (relative paths, reading data keyed by package), renamed declaration,
  version-dependent lexing, hostile paths and terminal escapes in snapshots.
* 4 pseudo-terminal tests: idle CPU ≤ 2 ticks / 3 s and terminal restored on `q`; `SIGTERM`
  restores the terminal; `q` during a large initial scan exits promptly without a staging file;
  `q` during a semantic export (fake toolchain whose `lake` spawns a grandchild) kills and reaps
  the whole process tree.

Not automated: the multi-version export runs (they need the installed toolchains and the
fixture builds in `/tmp/slim-fixture-v*`; commands in `fixtures/demo/axiom-report/README.md`
and `scripts/regen-demo-snapshot.sh`), the CFSG/Stacks measurements, and the screenshots
(`scripts/capture-screens.sh`).

## Known gaps

See `docs/limitations.md`. The important ones: no semantic export was run on a Mathlib-scale
project (no build artifacts here; CFSG's toolchain `v4.35.0-rc3` is not installed), so its cost
with the default full library closure is unknown; source-only mode cannot see macro-generated
declarations and has no "used by"; fuzzy search on multi-million-name indexes is time-budgeted;
no LSP/`.ilean` integration; Linux only.

## Next best extensions

1. **Mathlib-scale semantic export**: have the exporter summarise library declarations beyond the
   project frontier (per-declaration axiom sets with next-hop witnesses, computed once with
   sharing) instead of exporting every library edge, then measure on a built Mathlib project.
2. **Independent replay as evidence**: export with `lean4export` and check with an external type
   checker such as nanoda (cloned in `~/Codes/nanoda`), recording "replayed by checker X at
   fingerprint Y, permitted axioms Z" as its own evidence line, distinct from "build artifacts
   present".
3. **Tags and blueprints**: Stacks declarations carry `@[stacks TAG]`, `@[ega …]`, `@[sga …]`
   attributes (already stored and full-text searchable in source mode); a tag index and a "Tag"
   view would map informal references to declarations without claiming equivalence.
4. `.ilean`/LSP integration for source-level reference locations (version-pinned, labelled).
5. Comparing two snapshots of the same project (added/removed declarations, changed statements,
   changed axiom sets).

## Commands to start with

```sh
cargo build --release
./target/release/slim demo
./target/release/slim ~/Codes/CFSG          # source-only, index already in the cache
./target/release/slim ~/Codes/Stacks
cargo test --release
```
