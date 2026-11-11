# Milestone 2: scripted sessions in a pseudo-terminal (raw report)

Measured 2026-11-10 on the development machine (AMD Ryzen 9 7950X, 62.5 GiB RAM, Fedora 44,
kernel 7.2.11, NVMe), release build (`cargo build --release`, thin LTO), every run through
`research-safe-run --threads 2 --memory-mib 4096`, i.e. **pinned to 2 CPUs**, page cache warm.
Driver: `scripts/measure-views.py` (keys sent 25 ms apart to a 120x34 pseudo-terminal; numbers
come from slim's own `SLIM_TIMING_LOG`; peak RSS is `VmHWM` before quitting). Three runs
per scenario, each with fresh user data. Cold-cache runs were not measured (dropping the page
cache needs root).

Indexes: the bundled demo snapshot (166 local declarations, Lean 4.33.1); a copy of the existing
source-only index of the local CFSG checkout (commit 69cb972f5, clean worktree; 2,364,132
declaration candidates in 230,935 modules; 804 MiB index + name index); a copy of the existing
source-only index of the local HaagerupIzumi checkout (`~/Codes/haagerup/lean`, commit b4b618f
with uncommitted changes in the worktree; 15,390 candidates in 1,679 modules). Neither project
has a semantic snapshot or a `lean-view.toml`. For the map sessions a starting map was generated
with `slim map init` into a scratch directory and 3-4 pointers to existing declaration names
(and one deliberately missing name) were appended, to exercise resolution; these scratch maps
say nothing about the projects and are not part of the repository.

Key scripts (`scripts/measure-views-run.sh`):

* search session: `/`, a query, Enter, 20 x `j`, 20 x `k`, `1` (and `2`, `3` on the demo);
* demo views: 14 x `j`/`k` in the map, pin a goal, goal explorer with expansions, relation kinds
  (curated, mentions, Lean), `H`, direction changes, trails view, map expansion and detail
  scrolling;
* CFSG / HaagerupIzumi views: pin the pointer as goal, 20 x `j`/`k`, expansions, goal explorer in
  curated and mentions mode, then the generated file outline (`End`, Space) and 30 x `j`/`k`.

"ui event handling" is the time the UI thread spends per event (selection and expand actions
included); "frame" is render plus write of one frame; "worker: ..." are query-thread times.
"worker: map load + resolve" is reading the map file and resolving its names; "worker: map
rename hints" is the later scan for declarations with the same last name component as a missing
name (the map is already on screen while it runs).

## CFSG, milestone-1 binary, search session — run 1
first frame 1 ms; search ready after 200 ms; peak RSS 438 MiB; mode source
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 68 | 0.37 | 0.49 | 0.66 |
| frame bytes (KiB) | 68 | 0.04 | 2.88 | 13.31 |
| ui event handling | 69 | 0.00 | 0.12 | 2.85 |
| worker: detail | 1 | 0.35 | 0.35 | 0.35 |
| worker: lexical mentions | 1 | 0.20 | 0.20 | 0.20 |
| worker: other | 3 | 3.30 | 111.17 | 111.17 |

## CFSG, milestone-1 binary, search session — run 2
first frame 1 ms; search ready after 199 ms; peak RSS 439 MiB; mode source
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 68 | 0.36 | 0.47 | 0.50 |
| frame bytes (KiB) | 68 | 0.04 | 2.88 | 13.31 |
| ui event handling | 69 | 0.00 | 0.10 | 2.84 |
| worker: detail | 1 | 0.30 | 0.30 | 0.30 |
| worker: lexical mentions | 1 | 0.17 | 0.17 | 0.17 |
| worker: other | 3 | 0.08 | 77.21 | 77.21 |

## CFSG, milestone-1 binary, search session — run 3
first frame 1 ms; search ready after 192 ms; peak RSS 438 MiB; mode source
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 65 | 0.36 | 0.47 | 1.10 |
| frame bytes (KiB) | 65 | 0.04 | 2.88 | 13.31 |
| ui event handling | 69 | 0.00 | 0.16 | 2.85 |
| worker: detail | 1 | 0.28 | 0.28 | 0.28 |
| worker: lexical mentions | 1 | 0.18 | 0.18 | 0.18 |
| worker: other | 3 | 0.08 | 79.74 | 79.74 |

## CFSG, milestone-2 binary, same search session (no map) — run 1
first frame 3 ms; search ready after 194 ms; peak RSS 439 MiB; mode source
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 67 | 0.38 | 0.48 | 0.56 |
| frame bytes (KiB) | 67 | 0.04 | 2.88 | 13.31 |
| ui event handling | 69 | 0.00 | 0.11 | 2.84 |
| worker: detail | 1 | 0.34 | 0.34 | 0.34 |
| worker: lexical mentions | 1 | 0.23 | 0.23 | 0.23 |
| worker: map load + resolve | 1 | 0.02 | 0.02 | 0.02 |
| worker: other | 3 | 0.08 | 88.87 | 88.87 |

## CFSG, milestone-2 binary, same search session (no map) — run 2
first frame 3 ms; search ready after 191 ms; peak RSS 438 MiB; mode source
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 68 | 0.38 | 0.49 | 1.07 |
| frame bytes (KiB) | 68 | 0.04 | 2.88 | 13.31 |
| ui event handling | 70 | 0.00 | 0.10 | 0.15 |
| worker: detail | 1 | 0.33 | 0.33 | 0.33 |
| worker: lexical mentions | 1 | 0.18 | 0.18 | 0.18 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.50 | 77.28 | 77.28 |

## CFSG, milestone-2 binary, same search session (no map) — run 3
first frame 2 ms; search ready after 189 ms; peak RSS 440 MiB; mode source
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 69 | 0.37 | 0.47 | 0.55 |
| frame bytes (KiB) | 69 | 0.04 | 2.88 | 13.31 |
| ui event handling | 70 | 0.00 | 0.11 | 2.85 |
| worker: detail | 1 | 0.28 | 0.28 | 0.28 |
| worker: lexical mentions | 1 | 0.17 | 0.17 | 0.17 |
| worker: map load + resolve | 1 | 0.01 | 0.01 | 0.01 |
| worker: other | 3 | 0.40 | 91.64 | 91.64 |

## demo, milestone-1 binary, search session — run 1
first frame 3 ms; search ready after 4 ms; peak RSS 11 MiB; mode semantic
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 76 | 0.37 | 0.48 | 1.14 |
| frame bytes (KiB) | 76 | 2.72 | 3.30 | 13.31 |
| ui event handling | 81 | 0.00 | 0.01 | 0.03 |
| worker: axiom evidence | 1 | 0.11 | 0.11 | 0.11 |
| worker: detail | 1 | 0.40 | 0.40 | 0.40 |
| worker: neighbours | 2 | 0.40 | 0.40 | 0.40 |
| worker: other | 3 | 0.03 | 0.22 | 0.22 |

## demo, milestone-1 binary, search session — run 2
first frame 2 ms; search ready after 4 ms; peak RSS 11 MiB; mode semantic
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 78 | 0.37 | 0.48 | 0.57 |
| frame bytes (KiB) | 78 | 2.70 | 3.30 | 13.31 |
| ui event handling | 81 | 0.00 | 0.02 | 0.42 |
| worker: axiom evidence | 1 | 0.11 | 0.11 | 0.11 |
| worker: detail | 1 | 0.36 | 0.36 | 0.36 |
| worker: neighbours | 2 | 0.37 | 0.37 | 0.37 |
| worker: other | 3 | 0.02 | 0.18 | 0.18 |

## demo, milestone-1 binary, search session — run 3
first frame 2 ms; search ready after 2 ms; peak RSS 11 MiB; mode semantic
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 78 | 0.37 | 0.50 | 0.70 |
| frame bytes (KiB) | 78 | 2.70 | 3.30 | 13.31 |
| ui event handling | 81 | 0.00 | 0.01 | 0.03 |
| worker: axiom evidence | 1 | 0.12 | 0.12 | 0.12 |
| worker: detail | 1 | 0.45 | 0.45 | 0.45 |
| worker: neighbours | 2 | 0.33 | 0.33 | 0.33 |
| worker: other | 3 | 0.02 | 0.18 | 0.18 |

## demo, milestone-2 binary, same search session — run 1
first frame 3 ms; search ready after 3 ms; peak RSS 12 MiB; mode semantic
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 80 | 0.38 | 0.48 | 0.51 |
| frame bytes (KiB) | 80 | 2.70 | 3.70 | 13.32 |
| ui event handling | 83 | 0.00 | 0.05 | 0.08 |
| worker: axiom evidence | 1 | 0.12 | 0.12 | 0.12 |
| worker: detail | 1 | 0.33 | 0.33 | 0.33 |
| worker: map load + resolve | 1 | 1.56 | 1.56 | 1.56 |
| worker: map rename hints | 1 | 1.54 | 1.54 | 1.54 |
| worker: neighbours | 2 | 0.35 | 0.35 | 0.35 |
| worker: other | 3 | 0.07 | 0.17 | 0.17 |

## demo, milestone-2 binary, same search session — run 2
first frame 3 ms; search ready after 3 ms; peak RSS 12 MiB; mode semantic
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 77 | 0.39 | 0.52 | 0.55 |
| frame bytes (KiB) | 77 | 2.72 | 3.70 | 13.32 |
| ui event handling | 83 | 0.00 | 0.05 | 0.53 |
| worker: axiom evidence | 1 | 0.16 | 0.16 | 0.16 |
| worker: detail | 1 | 0.32 | 0.32 | 0.32 |
| worker: map load + resolve | 1 | 1.52 | 1.52 | 1.52 |
| worker: map rename hints | 1 | 2.34 | 2.34 | 2.34 |
| worker: neighbours | 2 | 0.36 | 0.36 | 0.36 |
| worker: other | 3 | 0.06 | 0.14 | 0.14 |

## demo, milestone-2 binary, same search session — run 3
first frame 2 ms; search ready after 4 ms; peak RSS 12 MiB; mode semantic
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 78 | 0.39 | 0.55 | 0.79 |
| frame bytes (KiB) | 78 | 2.72 | 3.70 | 13.32 |
| ui event handling | 83 | 0.00 | 0.06 | 0.39 |
| worker: axiom evidence | 1 | 0.13 | 0.13 | 0.13 |
| worker: detail | 1 | 0.32 | 0.32 | 0.32 |
| worker: map load + resolve | 1 | 2.07 | 2.07 | 2.07 |
| worker: map rename hints | 1 | 1.31 | 1.31 | 1.31 |
| worker: neighbours | 2 | 0.56 | 0.56 | 0.56 |
| worker: other | 3 | 0.05 | 0.18 | 0.18 |

## demo, milestone-2, map + goal + trails session (semantic, bundled map) — run 1
first frame 3 ms; search ready after 3 ms; peak RSS 11 MiB; mode semantic
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 143 | 0.35 | 0.48 | 0.55 |
| frame bytes (KiB) | 143 | 2.63 | 4.35 | 13.32 |
| ui event handling | 150 | 0.01 | 0.05 | 0.57 |
| worker: goal children | 11 | 0.36 | 0.93 | 0.93 |
| worker: map load + resolve | 1 | 1.62 | 1.62 | 1.62 |
| worker: map rename hints | 1 | 1.43 | 1.43 | 1.43 |
| worker: other | 6 | 0.12 | 0.22 | 0.22 |
| worker: view detail | 26 | 0.30 | 0.46 | 0.49 |

## demo, milestone-2, map + goal + trails session (semantic, bundled map) — run 2
first frame 4 ms; search ready after 5 ms; peak RSS 11 MiB; mode semantic
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 140 | 0.36 | 0.47 | 1.91 |
| frame bytes (KiB) | 140 | 2.66 | 4.35 | 13.32 |
| ui event handling | 150 | 0.01 | 0.05 | 0.45 |
| worker: goal children | 11 | 0.41 | 1.09 | 1.09 |
| worker: map load + resolve | 1 | 1.71 | 1.71 | 1.71 |
| worker: map rename hints | 1 | 2.31 | 2.31 | 2.31 |
| worker: other | 6 | 0.11 | 0.16 | 0.16 |
| worker: view detail | 26 | 0.32 | 0.40 | 0.42 |

## demo, milestone-2, map + goal + trails session (semantic, bundled map) — run 3
first frame 3 ms; search ready after 3 ms; peak RSS 11 MiB; mode semantic
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 143 | 0.34 | 0.49 | 0.73 |
| frame bytes (KiB) | 143 | 2.67 | 4.35 | 13.32 |
| ui event handling | 150 | 0.01 | 0.03 | 0.29 |
| worker: goal children | 11 | 0.40 | 1.03 | 1.03 |
| worker: map load + resolve | 1 | 1.69 | 1.69 | 1.69 |
| worker: map rename hints | 1 | 2.01 | 2.01 | 2.01 |
| worker: other | 6 | 0.14 | 0.24 | 0.24 |
| worker: view detail | 26 | 0.30 | 0.39 | 0.42 |

## CFSG, milestone-2, map + goal session (source-only; generated skeleton + 4 measurement pointers) — run 1
first frame 2 ms; search ready after 210 ms; peak RSS 484 MiB; mode source
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 179 | 0.31 | 0.44 | 1.06 |
| frame bytes (KiB) | 179 | 1.60 | 3.48 | 13.31 |
| ui event handling | 186 | 0.01 | 0.03 | 0.39 |
| worker: goal children | 4 | 0.36 | 0.41 | 0.41 |
| worker: map load + resolve | 1 | 6.72 | 6.72 | 6.72 |
| worker: map rename hints | 1 | 330.68 | 330.68 | 330.68 |
| worker: other | 5 | 0.07 | 23.09 | 23.09 |
| worker: outline tree | 1 | 346.90 | 346.90 | 346.90 |
| worker: view detail | 21 | 0.26 | 0.36 | 0.46 |

## CFSG, milestone-2, map + goal session (source-only; generated skeleton + 4 measurement pointers) — run 2
first frame 2 ms; search ready after 186 ms; peak RSS 488 MiB; mode source
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 183 | 0.31 | 0.43 | 0.63 |
| frame bytes (KiB) | 183 | 1.58 | 3.48 | 13.31 |
| ui event handling | 186 | 0.01 | 0.03 | 0.28 |
| worker: goal children | 4 | 0.44 | 0.44 | 0.44 |
| worker: map load + resolve | 1 | 0.95 | 0.95 | 0.95 |
| worker: map rename hints | 1 | 295.93 | 295.93 | 295.93 |
| worker: other | 5 | 0.07 | 20.84 | 20.84 |
| worker: outline tree | 1 | 348.40 | 348.40 | 348.40 |
| worker: view detail | 21 | 0.26 | 0.41 | 0.49 |

## CFSG, milestone-2, map + goal session (source-only; generated skeleton + 4 measurement pointers) — run 3
first frame 2 ms; search ready after 189 ms; peak RSS 487 MiB; mode source
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 180 | 0.30 | 0.42 | 0.74 |
| frame bytes (KiB) | 180 | 1.60 | 3.48 | 13.31 |
| ui event handling | 186 | 0.01 | 0.03 | 0.45 |
| worker: goal children | 4 | 0.36 | 0.38 | 0.38 |
| worker: map load + resolve | 1 | 1.14 | 1.14 | 1.14 |
| worker: map rename hints | 1 | 289.80 | 289.80 | 289.80 |
| worker: other | 5 | 0.07 | 22.87 | 22.87 |
| worker: outline tree | 1 | 356.91 | 356.91 | 356.91 |
| worker: view detail | 21 | 0.27 | 0.42 | 0.61 |

## HaagerupIzumi, milestone-2, map + goal session (source-only; generated skeleton + 3 measurement pointers) — run 1
first frame 3 ms; search ready after 4 ms; peak RSS 17 MiB; mode source
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 165 | 0.32 | 0.45 | 0.61 |
| frame bytes (KiB) | 165 | 0.59 | 3.44 | 13.31 |
| ui event handling | 169 | 0.01 | 0.02 | 0.17 |
| worker: goal children | 3 | 0.16 | 0.47 | 0.47 |
| worker: map load + resolve | 1 | 1.72 | 1.72 | 1.72 |
| worker: map rename hints | 1 | 6.15 | 6.15 | 6.15 |
| worker: other | 5 | 0.11 | 6.18 | 6.18 |
| worker: outline tree | 1 | 1.96 | 1.96 | 1.96 |
| worker: view detail | 5 | 0.29 | 0.84 | 0.84 |

## HaagerupIzumi, milestone-2, map + goal session (source-only; generated skeleton + 3 measurement pointers) — run 2
first frame 2 ms; search ready after 3 ms; peak RSS 17 MiB; mode source
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 165 | 0.31 | 0.44 | 2.15 |
| frame bytes (KiB) | 165 | 0.59 | 3.44 | 13.31 |
| ui event handling | 169 | 0.01 | 0.02 | 0.29 |
| worker: goal children | 3 | 0.13 | 0.46 | 0.46 |
| worker: map load + resolve | 1 | 1.30 | 1.30 | 1.30 |
| worker: map rename hints | 1 | 2.98 | 2.98 | 2.98 |
| worker: other | 5 | 0.06 | 3.29 | 3.29 |
| worker: outline tree | 1 | 1.54 | 1.54 | 1.54 |
| worker: view detail | 5 | 0.26 | 0.50 | 0.50 |

## HaagerupIzumi, milestone-2, map + goal session (source-only; generated skeleton + 3 measurement pointers) — run 3
first frame 2 ms; search ready after 4 ms; peak RSS 18 MiB; mode source
| measurement | n | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| frame (render + write) | 164 | 0.30 | 0.46 | 1.02 |
| frame bytes (KiB) | 164 | 0.59 | 3.52 | 13.31 |
| ui event handling | 169 | 0.01 | 0.02 | 0.40 |
| worker: goal children | 3 | 0.13 | 0.38 | 0.38 |
| worker: map load + resolve | 1 | 2.27 | 2.27 | 2.27 |
| worker: map rename hints | 1 | 4.46 | 4.46 | 4.46 |
| worker: other | 5 | 0.08 | 3.79 | 3.79 |
| worker: outline tree | 1 | 1.79 | 1.79 | 1.79 |
| worker: view detail | 5 | 0.29 | 0.37 | 0.37 |

