# Highlighting: measurements on HaagerupIzumi and CFSG

Measured 2026-11-10. What was decided beforehand, the machine, the two checkouts and the
budgets are in `docs/highlight-phase0.md`; design and parity evidence in
`docs/highlighting.md`. Raw session reports and benchmark outputs are in
`docs/bench/raw-highlight/`.

## Result in short

* **Browsing with full highlighting is as fast as browsing without it.** In scripted
  sessions on the real projects, the time from a key to the written frame is about 0.5 ms
  at the median and about 1 ms at the 99th percentile with syntax and semantic colours, on
  HaagerupIzumi and on CFSG; the slowest single frame in any session took 3.4 ms, or 7.9 ms
  with the browser's own indexing job on the same two CPUs.
  The budgets were 16 ms (p95) and 33 ms (p99). Nothing was reduced, and there is no
  lower-fidelity default.
* **The baseline layer matches VS Code's tokenizer on 4.22 million lines** of these projects
  and their Mathlib (no differing byte), at 7 to 13 MiB/s on one thread.
* **HaagerupIzumi's semantic index:** 1,419 of its 1,435 local files have complete semantic tokens (99.4 % of the source bytes; 430,023 tokens in a 7.2 MiB cache file), 16 are recorded as failed with their reasons, none is partial. The first pass took 1 h 43 min on two CPUs (3.9 s per file at the median); an independent second client got identical tokens for all 26 files sampled.
* **CFSG's semantic index: not built; this gate is open.** Its pinned toolchain
  (`v4.35.0-rc3`) is not installed, its dependencies are not fetched and it is not built;
  slim does none of these, and they were not authorised. Everything that needs only text
  was measured on CFSG; nothing about Lean's semantic tokens on CFSG is claimed.
* Indexing in the background does not disturb reading: with the browser's own indexing job
  on the same two CPUs the event-to-frame time was 0.38 ms at the median and 2.5 ms at p99
  (3.1 ms in the worst run), and the job stopped 27 to 30 ms after quitting with no process
  left.

## Method

* Builds: this tree (`cargo build --release`) and, for comparison, commit `3433d31`, the
  state before highlighting, built the same way into `target/m2-target`.
* Every run goes through `research-safe-run --threads 2` (two pinned CPUs, a cgroup memory
  limit). The machine is a shared desktop; other jobs of its owner ran at the same time.
* **Sessions** (`scripts/measure-highlight-run.sh`, driver `scripts/measure-views.py`): the
  real program in a 120×34 pseudo-terminal, keys sent 25 ms apart. One session opens a
  declaration, shows its whole file, scrolls 60 lines down and 30 up, jumps to the end and
  back, does the same in three more files (the third is the project's largest), goes back
  and forward through the four files ten times, resizes to 80×24, 200×60 and back while
  scrolling, and changes the syntax theme twice: about 400 events and 330 frames. Numbers
  come from slim's own timing log (`SLIM_TIMING_LOG`): *event → frame* is from
  receiving a key or resize to the end of writing the frame that shows its effect; *frame*
  is rendering plus writing one frame; *file asked → spans* is from asking the highlight
  worker for a file to its first spans arriving on the UI thread; *file asked → text* the
  same for the text itself. Each variant ran three times in a different order each round
  (old, off, syntax, semantic; then reversed; then off, semantic, old, syntax), after one
  unreported warm-up session. Cells give the median of the three runs and, where they
  differ, the range.
* Variants: the build before highlighting; this build with `--highlight off`; syntax only
  with no highlight cache on disk (the cache file is deleted before each run, so every file
  is tokenized in the session); syntax + semantic with spans and semantic tokens cached on
  disk.
* **What "cold" and "warm" mean here.** Each session is a new process, so slim's
  in-memory caches start empty every time; a file's first visit in a session reads the cache
  file or tokenizes, later visits are warm in memory. The operating system's page cache was
  warm in all runs: nothing was done to empty it (that needs root and would disturb the
  owner's other work), so no cold-storage figure exists. Lean's build cache plays no part in
  browsing.
* The sessions read copies of the projects' caches (`target/measure/cache`, taken with
  SQLite's online backup), never the owner's cache directly.
* **Headless benchmark** (`slim highlight bench DIR`): one thread, in process, no
  terminal: tokenizer throughput over real files, the highlight worker's file-open path for
  several classes of files, frame rendering into a buffer, and the cache's memory bound.
* Not measured: allocation counts (no allocation profiler was run); a cold page cache;
  a remote terminal; time spent by a terminal emulator drawing glyphs.

## The baseline tokenizer on real files (headless)

### HaagerupIzumi, every local file

Project `HaagerupIzumi`: 1435 local Lean files, 30.0 MiB; measured 1435 files (all).

* Tokenizing: 30.0 MiB, 452005 lines, 8071797 tokens in 3.55 s: **8.5 MiB/s, 127287 lines/s** on one thread.
* Per file (ms, p50 / p95 / p99 / max): whole file 0.20 / 16.48 / 20.54 / 72.70; first 240 lines (a viewport and its margin) 0.20 / 2.00 / 2.72 / 8.28.
* Slowest files: `HaagerupIzumi/C8/Lattice/S2Data.lean` (427 KiB) 73 ms; `HaagerupIzumi/C8/Lattice/P4DataZ1.lean` (267 KiB) 30 ms; `HaagerupIzumi/C8/Lattice/P4DataP1.lean` (217 KiB) 24 ms; `HaagerupIzumi/C8/Lattice/P4DataM2.lean` (210 KiB) 23 ms; `HaagerupIzumi/C8/Lattice/P4SubP0.lean` (287 KiB) 23 ms.

| opening a file through the worker's code path | n | first answer ms: p50 / p95 / p99 / max |
|---|---|---|
| alternating between two files 200 times | 200 | 0.01 / 0.01 / 0.01 / 0.31 |
| files with notation/macros, top of file, cold | 11 | 1.60 / 2.31 / 2.31 / 2.31 |
| largest files, jump to the last line, cold | 20 | 23.03 / 31.96 / 78.77 / 78.77 |
| largest files, jump to the last line, new worker with spans cached on disk | 20 | 0.41 / 0.71 / 1.72 / 1.72 |
| largest files, top of file, cold | 20 | 2.48 / 4.64 / 4.64 / 4.64 |
| ordinary files again, warm in memory | 200 | 0.02 / 0.02 / 0.02 / 0.02 |
| ordinary files, top of file, cold | 200 | 0.27 / 0.39 / 0.41 / 0.55 |

* Long navigation: 1435 files opened one after another and tokenized completely in 3.6 s; worker cache peak 109.4 MiB with a 128 MiB budget (1435 files still cached at the end); process RSS 60 → 134 MiB.

| headless frame of the source view | off | syntax | syntax + semantic |
|---|---|---|---|
| 120x34, 1326 frames: p50 / p95 / p99 / max ms | 0.22 / 0.23 / 0.23 / 0.30 | 0.25 / 0.39 / 0.58 / 0.77 | 0.25 / 0.38 / 0.58 / 0.75 |
| 200x60, 1326 frames: p50 / p95 / p99 / max ms | 0.41 / 0.42 / 0.43 / 0.64 | 0.51 / 0.62 / 0.80 / 1.22 | 0.51 / 0.62 / 0.80 / 1.25 |

* Semantic cache: 1419 complete, 0 partial, 16 failed, 0 skipped records (430023 tokens, 2.1 MiB of token blobs). Reading and parsing a record: 0.02 / 0.02 / 0.03 / 0.05 ms; decoding all its chunks: 0.00 / 0.02 / 0.03 / 0.10 ms; token blob size p50 / p95 / max: 229 / 6098 / 51293 bytes.

### CFSG, a sample

Project `cfsg`: 229089 local Lean files, 2059.1 MiB; measured 6049 files (a deterministic sample: every k-th file by path plus the 20 largest).

* Tokenizing: 115.4 MiB, 1331779 lines, 6401342 tokens in 8.90 s: **13.0 MiB/s, 149639 lines/s** on one thread.
* Per file (ms, p50 / p95 / p99 / max): whole file 0.80 / 3.49 / 7.39 / 515.18; first 240 lines (a viewport and its margin) 0.80 / 1.67 / 1.85 / 71.49.
* Slowest files: `Certificates/Data/NT44Model/Inputs.lean` (4650 KiB) 515 ms; `CFSG/Sporadic/Schreier/J1CertLit.lean` (2246 KiB) 266 ms; `CFSG/Sporadic/Bridge/M22Data.lean` (3917 KiB) 187 ms; `CFSG/Sporadic/Bridge/J2Data.lean` (4301 KiB) 155 ms; `CFSG/Sporadic/Bridge/M12Data.lean` (4363 KiB) 151 ms.

| opening a file through the worker's code path | n | first answer ms: p50 / p95 / p99 / max |
|---|---|---|
| alternating between two files 200 times | 200 | 0.01 / 0.01 / 0.01 / 0.87 |
| files with notation/macros, top of file, cold | 50 | 1.28 / 1.77 / 1.88 / 1.88 |
| largest files, jump to the last line, cold | 20 | 32.69 / 276.95 / 547.12 / 547.12 |
| largest files, jump to the last line, new worker with spans cached on disk | 20 | 3.02 / 6.26 / 9.77 / 9.77 |
| largest files, top of file, cold | 20 | 8.71 / 74.72 / 75.15 / 75.15 |
| ordinary files again, warm in memory | 200 | 0.03 / 0.03 / 0.03 / 0.03 |
| ordinary files, top of file, cold | 200 | 0.83 / 0.99 / 1.04 / 1.06 |

* Long navigation: 6049 files opened one after another and tokenized completely in 8.1 s; worker cache peak 128.1 MiB with a 128 MiB budget (3572 files still cached at the end); process RSS 355 → 356 MiB.

| headless frame of the source view | off | syntax | syntax + semantic |
|---|---|---|---|
| 120x34, 1354 frames: p50 / p95 / p99 / max ms | 0.22 / 1.57 / 1.97 / 3.57 | 0.24 / 1.57 / 2.04 / 3.48 | 0.24 / 1.57 / 1.97 / 3.45 |
| 200x60, 1354 frames: p50 / p95 / p99 / max ms | 0.40 / 1.55 / 1.85 / 4.04 | 0.46 / 1.58 / 1.92 / 3.34 | 0.47 / 1.58 / 1.92 / 3.24 |

* Semantic cache: no semantic cache for this project.

The slowest CFSG files are megabyte-sized data files. Opening one at its top costs under
0.1 s even uncached; a jump to the end of an uncached one waits for the whole file (0.5 s for
the 4.6 MiB worst case) with the text shown plain meanwhile, and after that the spans come
from disk in about 10 ms. The 6.5 MiB file with 692,627-character lines is tokenized in
0.1 s, because such lines are not tokenized (as in VS Code).

Memory on a long navigation, in a fresh process that does nothing else
(`highlight bench --navigation-only --repeat 3`: the 6,049 sampled files opened and tokenized
to the end, three times over, 18,147 opens):

| worker budget | peak worker cache | files cached at the end | process RSS before → after rounds 1, 2, 3 | per open ms p50 / p95 / p99 / max |
|---|---|---|---|---|
| 128 MiB (default) | 128.0 MiB | 2348 | 76 → 247, 267, 269 MiB | 0.93 / 1.86 / 2.17 / 77.2 |
| 64 MiB | 64.0 MiB | 835 | 76 → 199, 197, 197 MiB | 0.86 / 1.75 / 2.05 / 85.4 |
| 32 MiB | 32.0 MiB | 463 | 76 → 128, 145, 145 MiB | 0.86 / 1.78 / 2.01 / 82.7 |

The worker's cache stays at its budget whatever the corpus size, and the process's resident
memory levels off after the first round. Resident memory grows by more than the accounted
cache: about 50 MiB plus 1.1 times the budget in this stress (the file being shown and the
eight most recent source texts are outside the budget, and the allocator keeps freed
memory). The 76 MiB at the start is the list of CFSG's 229,089 files that this benchmark
holds, not something the browser has. In the scripted sessions below, highlighting adds 5 to
8 MiB to the browser's resident memory.

### Mathlib (dependency sources), a sample

Project `mathlib`: 8708 local Lean files, 94.4 MiB; measured 2193 files (a deterministic sample: every k-th file by path plus the 20 largest).

* Tokenizing: 24.8 MiB, 616134 lines, 1744277 tokens in 3.38 s: **7.4 MiB/s, 182364 lines/s** on one thread.
* Per file (ms, p50 / p95 / p99 / max): whole file 0.92 / 4.84 / 8.13 / 41.66; first 240 lines (a viewport and its margin) 0.92 / 1.49 / 1.70 / 1.91.
* Slowest files: `Mathlib.lean` (443 KiB) 42 ms; `Mathlib/Tactic/Translate/Core.lean` (66 KiB) 10 ms; `Mathlib/MeasureTheory/Integral/SetToL1.lean` (76 KiB) 10 ms; `Mathlib/MeasureTheory/Integral/IntegralEqImproper.lean` (75 KiB) 10 ms; `Mathlib/MeasureTheory/Measure/MeasureSpace.lean` (72 KiB) 10 ms.

| opening a file through the worker's code path | n | first answer ms: p50 / p95 / p99 / max |
|---|---|---|
| alternating between two files 200 times | 200 | 0.02 / 0.02 / 0.02 / 1.02 |
| files with notation/macros, top of file, cold | 50 | 1.22 / 1.45 / 1.61 / 1.61 |
| largest files, jump to the last line, cold | 20 | 9.75 / 10.41 / 44.68 / 44.68 |
| largest files, jump to the last line, new worker with spans cached on disk | 20 | 0.18 / 0.23 / 0.91 / 0.91 |
| largest files, top of file, cold | 20 | 1.50 / 1.95 / 2.02 / 2.02 |
| ordinary files again, warm in memory | 200 | 0.03 / 0.04 / 0.04 / 0.04 |
| ordinary files, top of file, cold | 200 | 0.98 / 1.17 / 1.23 / 1.31 |

* Long navigation: 2193 files opened one after another and tokenized completely in 3.3 s; worker cache peak 46.9 MiB with a 128 MiB budget (2193 files still cached at the end); process RSS 26 → 82 MiB.

| headless frame of the source view | off | syntax | syntax + semantic |
|---|---|---|---|
| 120x34, 1685 frames: p50 / p95 / p99 / max ms | 0.22 / 0.23 / 0.23 / 0.30 | 0.23 / 0.49 / 0.64 / 0.93 | 0.23 / 0.49 / 0.64 / 0.94 |
| 200x60, 1685 frames: p50 / p95 / p99 / max ms | 0.37 / 0.39 / 0.40 / 0.44 | 0.42 / 0.78 / 1.27 / 1.45 | 0.42 / 0.80 / 1.27 / 1.49 |

* Semantic cache: no semantic cache for this project.

(Files under directories starting with `Mathlib` in HaagerupIzumi's `.lake/packages/mathlib`.
Dependencies have no semantic tokens; this is the syntax layer only.)

## Sessions in a pseudo-terminal

### HaagerupIzumi, nothing else of slim running

| session | runs | first frame ms | search ready ms | CPU s | peak RSS MiB | read MiB | written MiB |
|---|---|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | 3 | 2 (1–3) | 4 (3–6) | 0.17 (0.15–0.17) | 18 (18–19) | 6.7 (6.7–6.8) | 1.23 |
| haagerup: highlighting off | 3 | 2 | 3 (3–4) | 0.17 | 21 | 6.8 (6.8–6.9) | 1.23 (1.23–1.28) |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | 3 | 2 | 3 (3–8) | 0.27 (0.25–0.27) | 26 | 7.3 (7.3–7.9) | 2.08 |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 3 | 2 | 3 (3–8) | 0.19 (0.17–0.20) | 24 | 8.6 | 2.33 |
| haagerup: idle minute, syntax + semantic | 1 | 3 | 5 | 0.19 | 24 | 8.6 | 2.33 |
| haagerup: idle minute, build before highlighting | 1 | 5 | 6 | 0.17 | 19 | 7.9 | 1.28 |

**event → frame** (ms; `input event to frame written`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | – | not recorded by this build | | | |
| haagerup: highlighting off | 280 | 0.45 (0.43–0.48) | 0.61 (0.59–0.61) | 0.85 (0.75–0.86) | 1.26 (1.04–1.35) |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | 280 | 0.43 (0.38–0.47) | 0.61 (0.60–0.63) | 0.83 (0.78–0.84) | 1.29 (1.06–3.12) |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 280 | 0.50 (0.40–0.51) | 0.68 (0.67–0.72) | 0.95 (0.84–0.99) | 1.56 (1.19–1.59) |

**frame** (ms; `frame (render + write)`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | 326 | 0.40 (0.34–0.41) | 0.54 (0.53–0.55) | – | 1.07 (1.02–1.30) |
| haagerup: highlighting off | 324 | 0.43 (0.41–0.45) | 0.56 (0.55–0.58) | 0.84 (0.75–0.84) | 1.26 (1.04–1.35) |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | 342 | 0.38 (0.36–0.45) | 0.58 (0.57–0.59) | 0.81 (0.74–0.84) | 1.25 (1.06–1.29) |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 337 | 0.45 (0.38–0.49) | 0.67 (0.65–0.67) | 0.99 (0.74–0.99) | 1.55 (1.18–2.41) |

**UI per event** (ms; `ui event handling`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | 384 | 0.00 | 0.01 | – | 0.19 (0.18–0.27) |
| haagerup: highlighting off | 384 | 0.00 | 0.01 | 0.16 (0.15–0.16) | 0.44 (0.44–0.47) |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | 402 | 0.00 | 0.01 | 0.16 (0.15–0.27) | 0.41 (0.28–2.83) |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 404 | 0.00 | 0.01 | 0.15 (0.14–0.16) | 0.43 (0.41–0.70) |

**file asked → spans** (ms; `highlight: file asked to spans on the UI thread`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | – | not recorded by this build | | | |
| haagerup: highlighting off | – | does not apply | | | |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | 14 | 0.31 (0.28–0.31) | 2.51 (2.50–3.39) | 2.77 (2.65–3.46) | 2.77 (2.65–3.46) |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 14 | 0.31 (0.30–0.31) | 2.12 (2.00–2.42) | 2.70 (2.69–2.73) | 2.70 (2.69–2.73) |

**jump further down in a file not yet tokenized that far → spans** (ms; `highlight: lines further down asked to spans on the UI thread`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | – | did not occur | | | |
| haagerup: highlighting off | – | did not occur | | | |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | – | did not occur | | | |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | – | did not occur | | | |

**file asked → text** (ms; `source: file asked to text on the UI thread`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | – | not recorded by this build | | | |
| haagerup: highlighting off | 14 | 0.35 (0.33–0.43) | 0.56 (0.43–0.68) | 0.64 (0.47–0.73) | 0.64 (0.47–0.73) |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | 14 | 0.32 (0.32–0.42) | 0.53 (0.49–0.69) | 0.73 (0.65–0.77) | 0.73 (0.65–0.77) |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 14 | 0.30 (0.30–0.35) | 0.48 (0.45–0.58) | 0.64 (0.45–0.77) | 0.64 (0.45–0.77) |

**Idle minute**

* haagerup: idle minute, syntax + semantic: 0 ms of CPU time in 60 s
* haagerup: idle minute, build before highlighting: 0 ms of CPU time in 60 s

### HaagerupIzumi, while the semantic indexing run of the next section was in progress

The indexing job was a separate process on two other CPUs (its own guard slot).

| session | runs | first frame ms | search ready ms | CPU s | peak RSS MiB | read MiB | written MiB |
|---|---|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | 3 | 1 (1–3) | 5 (2–8) | 0.16 | 18 (18–19) | 6.8 (6.7–6.9) | 1.23 (1.23–1.28) |
| haagerup: highlighting off | 3 | 2 | 4 (3–4) | 0.16 (0.16–0.17) | 21 (20–21) | 6.8 (6.8–7.4) | 1.23 |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | 3 | 2 (2–3) | 3 (3–4) | 0.27 (0.26–0.27) | 26 | 7.3 (7.3–7.4) | 2.08 (2.08–2.14) |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 3 | 2 (2–3) | 5 (3–8) | 0.19 (0.19–0.20) | 24 (23–24) | 8.6 (8.5–9.3) | 2.33 (2.32–2.34) |
| haagerup: idle minute, syntax + semantic | 1 | 2 | 3 | 0.22 | 25 | 9.6 | 2.33 |
| haagerup: idle minute, build before highlighting | 1 | 2 | 2 | 0.16 | 18 | 6.8 | 1.23 |

**event → frame** (ms; `input event to frame written`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | – | not recorded by this build | | | |
| haagerup: highlighting off | 280 | 0.44 (0.43–0.48) | 0.59 (0.56–0.59) | 0.77 (0.70–0.89) | 1.06 (1.04–3.36) |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | 280 | 0.46 (0.41–0.48) | 0.61 (0.61–0.63) | 0.85 (0.74–0.99) | 1.27 (1.24–1.43) |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 280 | 0.48 (0.47–0.48) | 0.72 (0.71–0.73) | 0.97 (0.93–1.02) | 1.47 (1.27–3.27) |

**frame** (ms; `frame (render + write)`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | 323 | 0.40 (0.38–0.42) | 0.57 (0.56–0.57) | – | 1.92 (1.15–4.71) |
| haagerup: highlighting off | 322 | 0.40 (0.38–0.46) | 0.56 (0.55–0.57) | 0.83 (0.83–0.88) | 1.45 (1.38–1.46) |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | 340 | 0.41 (0.37–0.41) | 0.60 (0.59–0.61) | 0.84 (0.73–0.98) | 1.28 (1.23–1.43) |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 338 | 0.43 (0.42–0.43) | 0.69 (0.68–0.69) | 0.93 (0.91–0.96) | 1.27 (1.16–1.47) |

**UI per event** (ms; `ui event handling`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | 384 | 0.00 | 0.01 | – | 0.18 (0.16–0.76) |
| haagerup: highlighting off | 384 | 0.00 | 0.01 | 0.15 (0.11–0.17) | 0.41 (0.37–2.86) |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | 402 | 0.00 | 0.01 | 0.11 (0.11–0.12) | 0.42 (0.32–0.46) |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 403 | 0.00 | 0.01 | 0.15 (0.14–0.17) | 0.64 (0.45–2.86) |

**file asked → spans** (ms; `highlight: file asked to spans on the UI thread`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | – | not recorded by this build | | | |
| haagerup: highlighting off | – | does not apply | | | |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | 14 | 0.30 (0.29–0.31) | 2.50 (2.47–2.58) | 3.33 (2.59–3.49) | 3.33 (2.59–3.49) |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 14 | 0.31 (0.29–0.32) | 1.81 (1.76–2.61) | 2.75 (2.42–2.94) | 2.75 (2.42–2.94) |

**jump further down in a file not yet tokenized that far → spans** (ms; `highlight: lines further down asked to spans on the UI thread`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | – | did not occur | | | |
| haagerup: highlighting off | – | did not occur | | | |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | – | did not occur | | | |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | – | did not occur | | | |

**file asked → text** (ms; `source: file asked to text on the UI thread`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: build before highlighting (3433d31) | – | not recorded by this build | | | |
| haagerup: highlighting off | 14 | 0.33 (0.24–0.36) | 0.44 (0.43–0.57) | 0.51 (0.48–0.62) | 0.51 (0.48–0.62) |
| haagerup: syntax only, nothing cached on disk (every file tokenized in this session) | 14 | 0.31 (0.25–0.32) | 0.50 (0.46–0.50) | 0.58 (0.50–0.61) | 0.58 (0.50–0.61) |
| haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 14 | 0.28 (0.24–0.30) | 0.45 (0.39–0.49) | 0.60 (0.44–0.69) | 0.60 (0.44–0.69) |

**Idle minute**

* haagerup: idle minute, syntax + semantic: 0 ms of CPU time in 60 s
* haagerup: idle minute, build before highlighting: 0 ms of CPU time in 60 s

### HaagerupIzumi, with the browser's own indexing job running on the same two CPUs

`scripts/measure-highlight-indexing.sh`: the session starts the job from the palette (plan,
typed `yes`), waits six seconds for the server, runs the same navigation, opens the details
overlay and quits, which cancels the job.

| session | runs | first frame ms | search ready ms | CPU s | peak RSS MiB | read MiB | written MiB |
|---|---|---|---|---|---|---|---|
| haagerup: syntax + semantic while the browser's own indexing job runs (same 2 CPUs) | 3 | 3 (1–4) | 8 (3–10) | 0.70 (0.67–0.73) | 28 (27–28) | 22.2 (21.6–24.7) | 2.40 (2.25–2.41) |

**event → frame** (ms; `input event to frame written`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: syntax + semantic while the browser's own indexing job runs (same 2 CPUs) | 301 | 0.38 (0.36–0.40) | 1.00 (0.88–1.22) | 2.51 (2.37–3.13) | 7.46 (5.67–7.91) |

**frame** (ms; `frame (render + write)`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: syntax + semantic while the browser's own indexing job runs (same 2 CPUs) | 447 | 0.36 (0.34–0.38) | 0.77 (0.66–1.00) | 2.72 (1.93–3.12) | 7.46 (5.66–7.91) |

**UI per event** (ms; `ui event handling`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: syntax + semantic while the browser's own indexing job runs (same 2 CPUs) | 543 | 0.00 | 0.02 (0.02–0.03) | 0.36 (0.33–0.49) | 2.65 (2.42–2.75) |

**file asked → spans** (ms; `highlight: file asked to spans on the UI thread`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: syntax + semantic while the browser's own indexing job runs (same 2 CPUs) | 14 | 0.29 (0.27–0.33) | 2.42 (2.36–3.60) | 4.15 (2.62–6.25) | 4.15 (2.62–6.25) |

**jump further down in a file not yet tokenized that far → spans** (ms; `highlight: lines further down asked to spans on the UI thread`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: syntax + semantic while the browser's own indexing job runs (same 2 CPUs) | – | did not occur | | | |

**file asked → text** (ms; `source: file asked to text on the UI thread`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| haagerup: syntax + semantic while the browser's own indexing job runs (same 2 CPUs) | 14 | 0.43 (0.40–0.61) | 1.16 (0.95–2.86) | 5.27 (4.82–7.41) | 5.27 (4.82–7.41) |

In the three sessions the job finished 4, 10 and 9 files (the cache copy had the tokens of 104 files removed beforehand so that there was work), each run ended as `cancelled` with no failed file, the server tree was gone 27, 30 and 28 ms after the quit was noticed, and no process was left. The extra reading (22 MiB instead of 9 MiB) and CPU time (0.70 s instead of 0.19 s) are the job hashing the project's sources to decide what is current.

### CFSG

| session | runs | first frame ms | search ready ms | CPU s | peak RSS MiB | read MiB | written MiB |
|---|---|---|---|---|---|---|---|
| cfsg: build before highlighting (3433d31) | 3 | 1 (1–2) | 173 (169–230) | 2.99 (2.92–3.11) | 440 (437–442) | 274.3 (270.5–277.1) | 0.73 (0.73–0.79) |
| cfsg: highlighting off | 3 | 2 (1–2) | 173 (167–175) | 2.88 (2.87–2.93) | 437 (436–439) | 274.4 (274.4–274.6) | 0.73 (0.73–0.78) |
| cfsg: syntax only, nothing cached on disk (every file tokenized in this session) | 3 | 3 (2–43) | 169 (167–190) | 3.04 (3.03–3.14) | 435 (415–436) | 277.5 (277.3–281.2) | 1.12 |
| cfsg: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 3 | 2 (2–5) | 177 (165–249) | 2.93 (2.85–3.08) | 440 (439–443) | 281.6 (277.7–283.3) | 0.89 (0.89–0.95) |
| cfsg: idle minute, syntax + semantic | 1 | 7 | 173 | 2.86 | 441 | 277.7 | 0.89 |
| cfsg: idle minute, build before highlighting | 1 | 1 | 168 | 2.87 | 438 | 270.4 | 0.73 |

**event → frame** (ms; `input event to frame written`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| cfsg: build before highlighting (3433d31) | – | not recorded by this build | | | |
| cfsg: highlighting off | 280 | 0.42 (0.42–0.45) | 0.61 (0.59–0.63) | 0.83 (0.77–0.86) | 1.19 (1.05–1.44) |
| cfsg: syntax only, nothing cached on disk (every file tokenized in this session) | 280 | 0.42 (0.42–0.46) | 0.61 (0.60–0.62) | 0.87 (0.78–1.09) | 1.62 (1.26–3.07) |
| cfsg: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 280 | 0.41 (0.41–0.42) | 0.66 (0.61–0.71) | 0.87 (0.83–1.05) | 1.26 (1.11–3.22) |

**frame** (ms; `frame (render + write)`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| cfsg: build before highlighting (3433d31) | 323 | 0.38 (0.36–0.41) | 0.57 (0.55–0.58) | – | 1.44 (1.05–3.08) |
| cfsg: highlighting off | 327 | 0.40 (0.38–0.42) | 0.57 (0.55–0.58) | 0.73 (0.67–0.77) | 3.11 (1.26–3.46) |
| cfsg: syntax only, nothing cached on disk (every file tokenized in this session) | 336 | 0.40 (0.39–0.43) | 0.56 (0.56–0.57) | 0.63 (0.61–0.65) | 1.17 (1.08–2.05) |
| cfsg: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 333 | 0.38 (0.36–0.39) | 0.59 (0.59–0.60) | 0.77 (0.74–0.79) | 1.35 (1.26–4.80) |

**UI per event** (ms; `ui event handling`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| cfsg: build before highlighting (3433d31) | 400 | 0.00 | 0.01 | – | 2.93 (0.87–5.65) |
| cfsg: highlighting off | 400 | 0.00 | 0.01 (0.01–0.02) | 0.27 (0.26–0.31) | 0.46 (0.43–0.58) |
| cfsg: syntax only, nothing cached on disk (every file tokenized in this session) | 415 | 0.00 | 0.02 (0.01–0.03) | 0.37 (0.28–0.40) | 0.92 (0.39–2.82) |
| cfsg: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 414 | 0.00 | 0.01 (0.01–0.04) | 0.29 (0.28–0.41) | 0.47 (0.40–2.85) |

**file asked → spans** (ms; `highlight: file asked to spans on the UI thread`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| cfsg: build before highlighting (3433d31) | – | not recorded by this build | | | |
| cfsg: highlighting off | – | does not apply | | | |
| cfsg: syntax only, nothing cached on disk (every file tokenized in this session) | 14 | 0.32 (0.29–0.45) | 11.90 (11.88–13.43) | 24.60 (24.60–24.73) | 24.60 (24.60–24.73) |
| cfsg: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 14 | 0.33 (0.30–0.33) | 2.20 (2.15–3.19) | 8.86 (8.58–8.86) | 8.86 (8.58–8.86) |

**jump further down in a file not yet tokenized that far → spans** (ms; `highlight: lines further down asked to spans on the UI thread`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| cfsg: build before highlighting (3433d31) | – | did not occur | | | |
| cfsg: highlighting off | – | did not occur | | | |
| cfsg: syntax only, nothing cached on disk (every file tokenized in this session) | – | did not occur | | | |
| cfsg: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | – | did not occur | | | |

**file asked → text** (ms; `source: file asked to text on the UI thread`)

| session | n per run | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| cfsg: build before highlighting (3433d31) | – | not recorded by this build | | | |
| cfsg: highlighting off | 14 | 0.51 (0.43–0.59) | 1.01 (0.82–1.04) | 1.45 (1.20–3.34) | 1.45 (1.20–3.34) |
| cfsg: syntax only, nothing cached on disk (every file tokenized in this session) | 14 | 0.48 (0.47–0.49) | 1.00 (0.86–1.48) | 1.46 (1.32–1.70) | 1.46 (1.32–1.70) |
| cfsg: syntax + semantic, spans and semantic tokens cached on disk (new process each run) | 14 | 0.49 (0.44–0.50) | 0.88 (0.86–0.98) | 1.33 (1.07–1.44) | 1.33 (1.07–1.44) |

**Idle minute**

* cfsg: idle minute, syntax + semantic: 0 ms of CPU time in 60 s
* cfsg: idle minute, build before highlighting: 0 ms of CPU time in 60 s

CFSG has no semantic tokens, so its "syntax + semantic" sessions show syntax colours read
from the disk cache. Its resident memory is that of the 2.36-million-name search index,
unchanged by highlighting.

## The semantic indexing run on HaagerupIzumi

Command (under the resource guard: two CPUs, 6,144 MiB of memory, one guard slot):
`slim highlight build ~/Codes/haagerup/lean --yes --cgroup none --file-timeout 240`,
Lean 4.33.1's `lake serve` in bubblewrap 0.12.0 (no network, project read-only), one open
document at a time. The project's sources did not change during the runs.

| | |
|---|---|
| Local Lean files | 1,435 (30.0 MiB); 1,383 of them have build artifacts |
| First pass | 6,169.7 s wall (1 h 42 min 50 s): 1,426 files opened, 1,411 complete, 15 failed, 3 already current from an earlier three-file trial, 6 not reached |
| How the first pass ended | stopped by the machine's resource guard: one draft file outside the built library drove the server to the 6,144 MiB limit within 25 s. The 1,411 finished files were already published; the server tree was gone 216 ms later; nothing had to be repaired. |
| Change made before running again | the job now watches the server's cgroup and closes a file whose elaboration takes the heap to 70 % of the limit, recording it as failed for that reason; a fatal header error is recorded with Lake's reason; a server that dies because the job is being stopped no longer marks the open file failed |
| Second pass (`--retry`: the 15 failed and the 6 unreached) | 118.4 s: 5 complete, 16 failed; guard's peak 5,355 MiB, no limit hit. A third pass of 59 s re-recorded 7 draft files with their exact reasons (peak 4,634 MiB). |
| **Final state** | **1,419 complete, 0 partial, 16 failed, 0 skipped**; complete coverage for 29.8 of 30.0 MiB of local source (99.4 %); every record current (`slim highlight status`) |
| Tokens | 430,023 (in the 27-file cross-check sample: 53.6 % `variable`, 42.6 % `keyword`, 3.9 % `property`) |
| Cache | 7.2 MiB on disk, of which 2.1 MiB token blobs (blob size p50 229 bytes, p95 6.1 KiB, largest 50 KiB) |
| Time per file (first pass, complete files, from opening to publishing) | p50 3.9 s, p90 7.2 s, p95 8.8 s, p99 12.5 s, max 33.8 s; their sum is 6,108 s of the 6,170 s wall time |
| Where the time goes | waiting for the server to elaborate: every file is elaborated from scratch against the compiled imports. Deciding what is current takes 0.6 s for the whole project; reading a record back takes 0.02 ms and decoding it 0.02 ms (headless benchmark above). |
| Memory | by the guard's cgroup accounting (what the limit applies to): 2,467 MiB peak on a 40-file sample, 5,355 MiB in the second pass (which includes the files stopped for memory at about 4.3 GiB of heap). The progress display's sum of resident sizes over the server's 5 to 6 processes was 3,447 MiB at the median and 4,418 MiB at p95; it counts shared pages once per process. The server's watchdog process alone holds about 1.3 GiB. |
| Two open documents instead of one | 40 files of one directory: 118.0 s with one document, 92.9 s with two (21 % less) on the same two CPUs; guard's peak 2,467 → 2,591 MiB. One stays the default. |
| Running it again with nothing changed | 0.6 s: all 1,435 files hashed, their import closures compared, nothing opened |
| Refreshing one file (`--refresh --files …`) | 3.3 s in all, 2.8 s of it the file |
| Cancelling | from the browser: 27, 30 and 28 ms until no process of the server tree was left (three sessions); the scripted tests bound Ctrl-C at 2.5 s including SIGKILL escalation |
| Agreement with an independent client | `scripts/check-semantic-tokens.py` on every 45th file of three directories: 26 files, 12,891 tokens, identical; the 27th file has no stored tokens (one of the 16 below). `docs/bench/raw-highlight/semantic-crosscheck.txt` |

The 16 files without tokens, as recorded:

| Files | Recorded reason | Comment |
|---|---|---|
| 4 in `HaagerupIzumi/NumberTheory` | imports are out of date and must be rebuilt; Lake named the stale imports | the project's build is behind its sources there; slim does not build |
| 3 drafts in `tools/lattice_export/drafts/p4ctl` | `lake setup-file --no-build` failed | they import modules that are not built |
| 2 in `tools/dependency_graph` | fatal error: unknown module prefix `ResultGraph` | a separate tool, not part of the library |
| 4 drafts `tools/lattice_export/drafts/p4ctl/P4Sum*.lean` | stopped for memory at 4.3 to 4.5 GiB of heap (limit 6,144 MiB) | never built by the project either; they need more memory than the budget used here |
| `C8/Lattice/PCTLink.lean`, `C8/Theta/LatticeData.lean`, `tools/w6_drafts/LatticeData.lean` | the server reported that the file's worker crashed ("likely due to a stack overflow or a bug") | reproducible for these three; two other files that crashed in the first pass completed on retry. A stack limit of 1 GB instead of the default did not help. Cause not established (the 48 GiB address-space cap of this setup is a candidate). |

`slim highlight build … --retry` tries them again; the first four become complete once
the project is rebuilt.

## Against the budgets

| Budget (set beforehand) | Measured | |
|---|---|---|
| Warm scrolling and selection, event to frame: p95 ≤ 16 ms, p99 ≤ 33 ms | HaagerupIzumi with syntax + semantic: p95 0.68 ms, p99 0.95 ms (worst run 0.72 and 0.99; slowest frame 1.6 ms). CFSG: p95 0.66 ms, p99 0.87 ms. With the browser's own indexing job on the same CPUs: p95 1.00 ms, p99 2.51 ms (worst run 1.22 and 3.13), slowest 7.9 ms. | met |
| Warm file or definition jump: p95 ≤ 50 ms | file asked → spans: HaagerupIzumi p95 1.8 to 2.5 ms, max 3.5 ms; CFSG p95 2.2 ms with spans on disk, 11.9 ms when tokenizing, max 24.7 ms. The text itself arrives in 0.3 to 1.5 ms (3.3 ms once). | met |
| Opening a file whose spans are cached on disk: ≤ 150 ms | new process each session: at most 2.9 ms (HaagerupIzumi) and 8.9 ms (CFSG, including its 6.5 MiB file). Headless, the largest files opened at their last line from disk: at most 1.5 ms and 10.6 ms. | met |
| The same without a cache (not a budget; reported) | HaagerupIzumi's largest file, jump to its end: 74 ms. CFSG's largest data files: 34 ms at the median, 272 ms at p95, 537 ms for the worst (4.6 MiB), during which the text is shown plain. In the sessions a jump never waited: files were tokenized to their end before the key arrived. | as designed |
| Start-up: ≤ 250 ms added | first frame 2 ms (1 to 2 ms before); search ready after 3 ms on HaagerupIzumi and about 173 ms on CFSG in every variant | met |
| Highlighting overhead on warm interactions: ≤ 10 ms at p95 | frame time p95 0.54 → 0.67 ms (HaagerupIzumi), 0.57 → 0.59 ms (CFSG); headless 0.22 → 0.39 ms per 120×34 frame | met |
| Highlight worker's file cache: 128 MiB | accounted cache held at 128.0 MiB over 18,147 opens of CFSG files. Resident memory of that stress process grew by 193 MiB (121 MiB with a 64 MiB budget, 69 MiB with 32 MiB). In the sessions highlighting added 5 to 8 MiB (18 → 24 to 26 MiB). | the cache bound is met; as a resident-memory figure the default is exceeded in the stress test, and is reported as such |
| Cancelling the indexing job: about 2 s, nothing left | 27 to 30 ms from the browser; 216 ms when the first pass was stopped from outside; no process left in any run | met |
| Idle: no polling | 0 ms of CPU time in 60 s in a coloured source view, as before highlighting | met |

Nothing met the "severe" criteria (p95 above 100 ms for immediate interactions, repeated
stalls above 250 ms, unbounded memory, navigation made unusable by indexing), so the
optimisation sequence of the plan was not needed beyond its first step, which the design
already takes (no tokenizing, decoding or style resolution of anything but visible lines on
the UI thread), and no lower-fidelity mode was proposed or added.

## Open gates and gaps

* **CFSG, semantic tokens: open.** Not indexed, for the reasons above. Unknown therefore: how
  long CFSG's 229,089 files take to index (by HaagerupIzumi's rate of about 4 s per file on two
  CPUs it would be on the order of ten days, but CFSG's files and toolchain are different and
  this is not a measurement), how large its cache would be, and whether `v4.35.0-rc3`'s server
  behaves like 4.35.0's. What it needs: the toolchain installed, the dependencies fetched, the
  project built, and then `slim highlight build ~/Codes/CFSG --allow-untested-toolchain`
  (resumable; `--files DIR` to start with a part).
* **HaagerupIzumi: 16 files without tokens** (table above), three of them for a worker crash
  whose cause is not established.
* **No comparison against a running VS Code window**: the baseline is compared with VS Code's
  tokenizer and theme files byte for byte; the semantic style rules were ported from VS Code's
  code and not confirmed in its editor (`docs/highlighting.md`, discrepancy 11).
* **Older toolchains on a large project**: 4.16 to 4.35 were exercised on the nine-file
  fixture only.
* **Resident memory under the stress test** exceeds the 128 MiB figure (previous section).
* **Not measured**: allocation counts; a cold page cache; a remote terminal; the paint time of
  a terminal emulator; the indexing job with more than two CPUs.

## Reproducing

```sh
cargo build --release
# the state before highlighting, for comparison
mkdir -p target/m2-src && git archive 3433d31 | tar -x -C target/m2-src
(cd target/m2-src && CARGO_TARGET_DIR=../m2-target cargo build --release)

# headless
slim highlight bench ~/Codes/haagerup/lean --report bench-haagerup.json
slim highlight bench ~/Codes/CFSG --max-files 6000 --report bench-cfsg.json
slim highlight bench ~/Codes/CFSG --max-files 6000 --navigation-only --repeat 3
scripts/summarize-highlight-bench.py --bench bench-haagerup.json

# sessions (copies of the caches go to target/measure/cache first; see the script)
scripts/measure-highlight-run.sh haagerup
scripts/measure-highlight-run.sh cfsg
scripts/measure-highlight-indexing.sh
scripts/summarize-highlight-bench.py target/measure/highlight-haagerup.md

# the semantic index (explicit; prints its plan). Under this machine's resource guard:
research-safe-run --memory-mib 6144 --limit-mib 49152 --threads 2 -- \
  slim highlight build ~/Codes/haagerup/lean --yes --cgroup none --file-timeout 240 --json
slim highlight status ~/Codes/haagerup/lean --failures

# parity with VS Code's tokenizer, and the independent semantic-token check
npm install --prefix target/hl-ref vscode-textmate vscode-oniguruma
node scripts/textmate-reference.mjs --theme assets/themes/dark_plus.json --list files.txt > ref.jsonl
slim highlight dump --syntax-theme dark-plus --list files.txt > ours.jsonl
scripts/compare-textmate.py ref.jsonl ours.jsonl --default-fg '#d4d4d4'
scripts/check-semantic-tokens.py ~/Codes/haagerup/lean HIGHLIGHT.sqlite FILE.lean...
scripts/test-highlight-toolchains.sh
```

The address-space limit of 48 GiB in the indexing command is deliberate: the Lean server
reserves about 1 GiB of address space per thread and maps the imports' `.olean` files; the
binding limit is the 6,144 MiB of memory.
