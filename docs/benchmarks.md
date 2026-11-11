# Measured performance

Measured 2026-11-10 on the development machine. Every run used the release build
(`cargo build --release`, thin LTO) and was executed through the machine's resource guard
(`research-safe-run --threads 2`), i.e. **pinned to 2 CPUs** with a cgroup memory limit, on a
shared desktop that was otherwise lightly loaded. Raw reports: `docs/bench/*.md` (regenerate
with the commands shown).

* CPU: AMD Ryzen 9 7950X (16 cores / 32 threads; 2 available to the runs)
* RAM: 62.5 GiB; Fedora 44, kernel 7.2.11; NVMe; page cache warm ("warm" = files cached)
* Repetitions: 5 (synthetic) or 3 (real indexes) per query shape; p50/p95/max over all samples

## Targets vs. results

| Target (from the brief) | Synthetic 100k decls / 962k edges | CFSG (source-only, 2.36 M) | Stacks (source-only, 0.78 M) |
|---|---|---|---|
| warm open to a usable screen < ~1 s | first frame 1–2 ms after process start; index + metadata + name index 6.8 ms p50 | first frame 1–2 ms; search ready 175–197 ms after process start (3 TUI runs) | name index ready ~115 ms |
| warm search p95 < 50 ms | 3.2 ms | 30.5 ms (p50 4.8 ms) — see note | 8.4 ms (p50 1.8 ms) |
| warm direct-neighbour p95 < 50 ms | dependencies 0.14 ms; dependents 0.95 ms incl. hubs with 26,218 dependents (max 5.2 ms) | n/a (no semantic edges) | n/a |
| responsive during ingestion | search + dependents while a second 1 M-edge import runs: p95 3.0 ms | UI stays interactive during the initial scan (progress screen); `q` cancels it — the pseudo-terminal test asserts exit within 4 s on a 4,000-file tree (CFSG not timed) | — |
| idle CPU ≈ 0 | ≤ 2 clock ticks (20 ms) of CPU in 3 s idle (pseudo-terminal test; the loop blocks, no timers) | same code path | — |

Note on CFSG search p95: queries whose precise tiers find fewer than 40 names fall through to the
fuzzy tier, which stops at its 30 ms budget on 2.36 M names. The target is met, but this is the
dominant cost; see limitations.

## Scrolling (j/k held down)

Measured with `SLIM_TIMING_LOG` while tmux sent 80 `j` then 80 `k` at 40 keys/s to a 120×34
window (scenario script used for this report: search results; then the CFSG outline).

| | before (preview on every key) | after (debounced preview, atomic frames) |
|---|---|---|
| frames drawn for 160 keys (CFSG results) | 482 | 178 |
| bytes written per frame, p95 | 2.3 KiB (CFSG), 3.4 KiB (demo) | 0.6 KiB (CFSG), 0.9 KiB (demo) |
| detail + right-pane loads during the scroll | 161 + 161 | 3 + 3 (when the cursor rests) |
| render + write per frame, p95 | 0.5 ms | 0.5 ms |
| CFSG outline (229k modules): tree build | on the UI thread | 281 ms on the worker; UI handling ≤ 0.33 ms |

Every frame is written with one `write` inside synchronized-update markers (DEC mode 2026), so
terminals that support it never show half a frame.

## Synthetic snapshot (back end only)

`slim bench --full --report docs/bench/synthetic-100k.md` — deterministic generator (seed 42,
SplitMix64), 100,012 declarations in 1,000 modules, 962,176 typed edges with a heavy-tailed
target distribution (largest fan-in 26,218), 30 % with docstrings, standard/custom/`sorryAx`
axioms; header marked `"synthetic": true`. It tests the index and browser back end only, not
Lean extraction or mathematical usefulness.

| | |
|---|---|
| snapshot | 54.0 MiB JSON Lines, generated in 0.15 s |
| cold import (2-pass validation, batched load, SCC axiom analysis, FTS5, indexes, name index, atomic install) | **4.85 s** |
| index size | 67.1 MiB SQLite + 4.7 MiB name index |
| peak RSS of the benchmark process | 187 MiB |

| warm path (ms) | n | p50 | p95 | max |
|---|---|---|---|---|
| open index + metadata + modules + name index load | 5 | 6.78 | 7.56 | 7.56 |
| name search incl. fetching the visible page (20 query shapes) | 150 | 0.16 | 3.23 | 4.42 |
| documentation full-text search | 150 | 0.12 | 2.32 | 33.28 |
| direct dependencies (3 groups, ≤100 each, with counts) | 550 | 0.11 | 0.14 | 0.27 |
| direct dependents (incl. 10 hubs, largest fan-in 26218) | 550 | 0.10 | 0.95 | 5.20 |
| direct dependents, filtered (local theorems only) | 550 | 0.11 | 1.01 | 5.83 |
| declaration detail | 550 | 0.07 | 0.08 | 0.17 |
| axiom evidence with witness paths | 550 | 0.55 | 0.82 | 1.07 |
| search + dependents while another import runs | 9479 | 0.26 | 3.04 | 4.77 |

Smoke variant (default `slim bench`, 10k / 100k, ~1 s): `docs/bench/synthetic-10k-smoke.md`.

## Real projects (source-only)

Both are local checkouts made by the user; slim only read their `*.lean` files.

| | CFSG | Stacks |
|---|---|---|
| revision | `69cb972f587a3ecc035e092253167ff2e303c962` | `5ed82d14634e8d9576bb844c7bd4e3da66d450e7` |
| toolchain pinned by the project | `leanprover/lean4:v4.35.0-rc3` | `leanprover/lean4:v4.35.0` |
| files / bytes | 229,089 / 2.06 GiB | 62,975 / 963 MiB |
| declaration candidates | 2,364,132 (12,634 ambiguous names) | 777,887 (9,321 ambiguous names) |
| partially parsed files | 29 | 11 |
| cold scan, 2 threads (`slim scan --force`) | **18.7 s** wall, max RSS 805 MiB | **10.8 s** |
| index on disk | 650 MiB + 153 MiB name index | 536 MiB + 56 MiB name index |
| freshness check (stat all files, hash only on mismatch) | 2.3 s | — |

Warm paths (`slim bench --index …/source.sqlite --repeat 3`):

| warm path (ms) | CFSG p50 / p95 / max | Stacks p50 / p95 / max |
|---|---|---|
| open index + metadata + name index load | 252 / 271 / 271 | 115 / 128 / 128 |
| name search incl. page fetch (68 query shapes) | 4.8 / 30.5 / 31.1 | 1.8 / 8.4 / 30.8 |
| documentation full-text search | 0.14 / 0.42 / 1.02 | 1.33 / 34.5 / 42.4 |
| declaration detail | 0.05 / 0.06 / 0.11 | 0.05 / 0.06 / 0.12 |
| module outline (declarations of a module) | 0.03 / 0.07 / 3.3 | 0.03 / 0.06 / 0.13 |

Not measured: semantic export of CFSG or Stacks (no build artifacts; CFSG's toolchain is not
installed), so there is **no mathlib-scale validation of the semantic path**. The semantic path
was measured only on the fixture: export 0.3–0.4 s per toolchain (166–299 local and
1,675–1,827 library declarations, 24–27 k edges), import 0.05–0.08 s.

## Reproducing

```sh
cargo build --release
./target/release/slim bench                       # smoke, ~1 s
./target/release/slim bench --full --report r.md  # 100k / 1M, ~12 s, < 400 MiB
./target/release/slim scan PATH --threads 2        # cold source index
./target/release/slim bench --index ~/.cache/slim/projects/<key>/source.sqlite
SLIM_TIMING_LOG=t.jsonl ./target/release/slim PATH   # first-frame / search-ready times
cargo test --release --test pty                        # idle CPU and cleanup
```

## Milestone 2: map, goal explorer and trails

Measured 2026-11-10, same machine and guard (`research-safe-run --threads 2`, 2 CPUs), release
build, page cache warm. Scripted sessions in a pseudo-terminal (`scripts/measure-views.py`, keys
40 per second into a 120x34 terminal), three runs each with fresh user data; the numbers are the
range over the three runs, from slim's own timing log. Raw tables, scenarios and exactly what
was opened: `docs/bench/m2-views.md`. Cold-cache runs were **not** measured.

The plan's engineering targets were warm first paint under 300 ms and ordinary selection/expand
actions under 50 ms at p95.

| | demo (semantic, bundled map) | CFSG (source-only, 2.36 M candidates) | HaagerupIzumi (source-only, 15,390) |
|---|---|---|---|
| first frame after process start | 3–4 ms | 2 ms | 2–3 ms |
| map file read + names resolved (worker) | 1.6–1.7 ms | 1.0–6.7 ms | 1.3–2.3 ms |
| rename hints for a missing name (worker, map already shown) | 1.4–2.3 ms | 290–331 ms | 3.0–6.2 ms |
| UI thread per event (selection, expand, mode change), p95 / max | 0.03–0.05 / 0.57 ms | 0.03 / 0.45 ms | 0.02 / 0.40 ms |
| frame render + write, p95 / max | 0.47–0.49 / 1.9 ms | 0.42–0.44 / 1.1 ms | 0.44–0.46 / 2.2 ms |
| detail for the selected declaration (worker), p95 | 0.39–0.46 ms | 0.36–0.42 ms | 0.37–0.84 ms |
| goal explorer: one node's direct relations (worker), p95 | 0.93–1.09 ms (curated, lexical, Lean) | 0.38–0.44 ms (curated, lexical; n = 4) | 0.38–0.47 ms (n = 3) |
| generated file outline, first expansion (worker) | not opened in this script | 347–357 ms (230,935 modules) | 1.5–2.0 ms |
| peak RSS | 11 MiB | 484–488 MiB | 17–18 MiB |

Both targets are met in every run. Two things are not instantaneous on CFSG, and both run on the
query worker while the UI stays responsive: the rename-hint scan (one pass over 2.36 M names per
missing reference, for at most 20 references, and no new scan starts once 400 ms have been spent) and the
first build of the file outline, which milestone 1 already had.

The scratch maps used for CFSG and HaagerupIzumi were generated skeletons with a few pointers to
existing names; they measure the software, not the projects. No semantic snapshot exists for
either project, so Lean dependencies were not exercised at that scale on real data.

### Regression check against milestone 1

The same search session (`/`, query, Enter, 20 x `j`, 20 x `k`, tabs) with both binaries:

| | milestone 1 | milestone 2 |
|---|---|---|
| CFSG: first frame / name index ready | 1 ms / 192–200 ms | 2–3 ms / 189–194 ms |
| CFSG: UI per event p95 · frame p95 | 0.10–0.16 ms · 0.47–0.49 ms | 0.10–0.11 ms · 0.47–0.49 ms |
| CFSG: peak RSS | 438–439 MiB | 438–440 MiB |
| demo: first frame / name index ready | 2–3 ms / 2–4 ms | 2–3 ms / 3–4 ms |
| demo: UI per event p95 · frame p95 | 0.01–0.02 ms · 0.48–0.50 ms | 0.05–0.06 ms · 0.48–0.55 ms |
| demo: peak RSS | 11 MiB | 12 MiB |

No regression outside run-to-run noise. On the demo the UI spends a few hundredths of a
millisecond more per event because search results now also match the project map.

### Scale (SYNTHETIC data)

`tests/map_fixture.rs::synthetic_100k_with_a_hub_stays_paged_and_bounded`: the deterministic
generator (seed 7; the index is marked synthetic) with 100,000 declarations and about 400,000
edges, plus one adversarial hub with 60,000 outgoing edges. Three runs on 2 CPUs: the worst of
five consecutive 50-row pages of the hub took 3 ms; a connection query from the hub stopped at
its 20,000-declaration limit after 4 ms and reported "truncated". These numbers say nothing
about Lean extraction or about any real project.
