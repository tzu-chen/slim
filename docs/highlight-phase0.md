# Highlighting: implementation note (phase 0)

Written 2026-11-10 before the measurements in `docs/bench/highlight.md` were evaluated. It
records what was inspected, what was decided, and the budgets the results are held to.
Usage and design are in `docs/highlighting.md`.

## What exists and where highlighting attaches

slim (crate `slim` 0.1.0, commit `3433d31` on branch `milestone-2` when this work
started) is a read-only browser over a per-project SQLite index. The UI thread blocks on one
channel; a query worker owns the index connection and a small source-file cache; a search
worker and a job runner do the rest (`docs/design.md` §2).

* Source text reaches the screen in two places: the whole-file source view
  (`render::draw_source`, from `QueryResp::FullSource`) and the excerpt at the end of the
  declaration view (`render::detail_lines`, from `DetailBundle.excerpt`). Both wrap long
  lines with `text::wrap_line` after `text::sanitize`; there is no horizontal scrolling, no
  in-file search and no selection in the source view. The only overlay is the background of
  the current declaration's lines.
* Extension points chosen: both bundles now carry the SHA-256 of the text they show; a new
  worker thread answers "spans for this path" with immutable snapshots; the two render
  functions ask `App::hl_rows` for styled rows of a line and fall back to the old plain rows.
  `text::wrap_line` was split into `wrap_ranges` so styled text wraps identically. Nothing
  else in navigation, history, search, the map views or the index changed.
* The existing job runner runs one job at a time (freshness check, rescan, export). Semantic
  indexing can run for hours, so it has its own thread and its own cancel/pause flags and
  never blocks those jobs. It reuses `process.rs` (process groups, `PR_SET_PDEATHSIG`,
  limits, registry for the panic hook) and the export's sandbox detection.
* Position utilities for UTF-8, code points and UTF-16 already existed (`text.rs`); the
  semantic decoder adds a strict variant that refuses positions inside a character.

## Projects and what may run

| | HaagerupIzumi | CFSG |
|---|---|---|
| Checkout | `~/Codes/haagerup/lean` (Lake project inside the `haagerup` repository) | `~/Codes/CFSG` |
| Revision | `b4b618f2abff`, with uncommitted changes (23 changed or untracked entries under `lean/` at the time; the repository is being worked on) | `69cb972f587a`, clean |
| Local Lean sources | 1,435 files, 30.04 MiB, digest `7b8560d751981f21`¹ | 229,089 files, 2,059.15 MiB, digest `7987aa9f2370fe5f`¹ |
| `lean-toolchain` | `leanprover/lean4:v4.33.1` — installed | `leanprover/lean4:v4.35.0-rc3` — **not installed** (4.35.0 is, which is a different toolchain) |
| Dependencies | Mathlib `0df444a360` and its dependencies present under `.lake/packages`, built | manifest names Mathlib `b63f6e8a`, TauCeti and others; **nothing fetched** (`.lake` does not exist) |
| Build | `.lake/build` present: 1,383 `.olean` files for 1,435 sources | none |
| `lake-manifest.json` SHA-256 (first 16) | `99e9d2b033f57d36` | `329012ba8bf96e7b` |
| Lake configuration | `lakefile.toml` (data) | `lakefile.toml` (data) |

¹ SHA-256 over the sorted list of (relative path, SHA-256 of the file), first 16 hex digits.
A git commit alone does not identify either tree.

Authorised and done: reading both trees; for HaagerupIzumi, running its own installed
toolchain's language server on its existing build, in the sandbox (read-only project, no
network), through the machine's resource guard, writing only slim's cache.

Not authorised, therefore not done: installing `v4.35.0-rc3`, fetching CFSG's dependencies,
building CFSG. Semantic indexing of CFSG is consequently impossible here, and its
large-project semantic gate stays open (`docs/bench/highlight.md`). CFSG is measured for
everything that needs only text. HaagerupIzumi's own rules for running Lean (two CPUs, a
memory guard) are met by the guard invocation used; the stack-size shim of its `safe_lean.py`
is not needed because the address-space cap was raised explicitly for the server (thread
stack reservations are virtual; the binding limit is the 6,144 MiB cgroup memory limit).

## Machine

AMD Ryzen 9 7950X (16 cores, 32 threads), 62.5 GiB RAM, Fedora 44, kernel 7.2.11, ext4 on
NVMe (Sabrent Rocket 4.0 1 TB). A shared desktop: every measured run goes through
`research-safe-run` pinned to two CPUs, and other jobs of the owner run at the same time.
Rust 1.99.1, release profile (`debug = 1`, thin LTO, 4 codegen units). Terminals: a
120×34 pseudo-terminal for the scripted sessions (also 80×24 and 200×60 during the resize
steps), tmux 3.7c for screenshots, a headless 120×34 and 200×60 buffer for render timing.
The page cache was warm throughout; nothing was done to empty system caches.

## Decisions

* **Baseline engine:** a port of vscode-textmate on Oniguruma, running the Lean extension's
  grammars unchanged; Giallo not adopted for a licensing reason (EUPL-1.2), syntect and
  tree-sitter-lean for fidelity reasons. Details and the licence review:
  `docs/highlighting.md`, `THIRD-PARTY-NOTICES.md`. New linked dependency: `onig` 6.5.3 (MIT)
  with bundled Oniguruma 6.9.10 (BSD-2-Clause). slim stays Apache-2.0.
* **Semantic extraction:** a small LSP client for `lake serve` of the pinned toolchain, one
  watchdog process reused for all files, one open document at a time by default. No Lean
  fork, no copied compiler internals, no invented command-line flags.
* **Supported toolchains:** Lean 4.16.0, 4.21.0, 4.28.0, 4.33.1 and 4.35.0 (the versions the
  exporter is tested on), each verified end to end on the fixture; others in 4.16+ only
  with `--allow-untested-toolchain`.
* **Storage:** a separate SQLite file per project in the cache directory.
* **Reference theme:** VS Code Dark+ first, Light+ second, the two high-contrast themes, and
  any VS Code theme file.

## Baseline before the change

The build of commit `3433d31` (`target/m2-target`, same profile) runs the same scripted
sessions as the new build; its numbers are the first rows of every table in
`docs/bench/highlight.md`. It has no "input event to frame" measurement of its own (that
timer was added with this work), so it is compared on frame time, per-event UI time, CPU
time and memory.

## Budgets

Adopted unchanged from the plan, before any variant was measured; none was adjusted
afterwards.

| | Budget |
|---|---|
| Warm scrolling and selection, event to frame | p95 ≤ 16 ms, p99 ≤ 33 ms |
| Warm file or definition jump | p95 ≤ 50 ms |
| Opening a file whose spans are cached on disk | normally ≤ 150 ms, with plain text first for outliers |
| Start-up with caches present | ≤ 250 ms added to the first usable screen; no whole-project validation before the first frame |
| Highlighting overhead on warm interactions | ≤ 10 ms added at p95 |
| Highlight worker's file cache | 128 MiB by default, configurable |
| Cancelling the indexing job | about 2 s when the server stops on request; SIGKILL after 1.5 s otherwise; no owned process left |
| Idle with a coloured view | no polling; CPU use over a minute indistinguishable from the build before |
| "Severe" (would justify proposing less) | p95 above 100 ms for normally immediate interactions, repeated stalls above 250 ms, unbounded memory growth, or navigation made unusable by indexing |

Semantic indexing itself has no latency budget: it is reported with its wall time, coverage
and resource use. Its resource budget for the runs here: one guard slot, two CPUs, 6,144 MiB
of memory, 240 s per file.
