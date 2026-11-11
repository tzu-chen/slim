# slim benchmark

Synthetic snapshot: 100000 declarations, ~1000000 edges, seed 42. Synthetic data tests the index and browser back end only.

- CPU: AMD Ryzen 9 7950X 16-Core Processor; CPUs available to this process: 2
- RAM: 62.5 GiB; kernel 7.2.11-200.fc44.x86_64
- build: release

- generated snapshot: 100012 declarations, 962173 edges, 54.0 MiB in 0.15 s
- cold import (validate, load, axiom analysis, indexes, FTS, name index): 4.85 s; 962176 edges, 5 axioms; index 67.1 MiB + name index 4.7 MiB; peak RSS so far 142 MiB

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

- peak RSS of the benchmark process: 187 MiB
- name index in memory: 4.7 MiB for 100012 entries
- total benchmark time: 11.2 s
