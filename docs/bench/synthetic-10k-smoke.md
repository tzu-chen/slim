# slim benchmark

Synthetic snapshot: 10000 declarations, ~100000 edges, seed 42. Synthetic data tests the index and browser back end only.

- CPU: AMD Ryzen 9 7950X 16-Core Processor; CPUs available to this process: 2
- RAM: 62.5 GiB; kernel 7.2.11-200.fc44.x86_64
- build: release

- generated snapshot: 10012 declarations, 96457 edges, 5.2 MiB in 0.01 s
- cold import (validate, load, axiom analysis, indexes, FTS, name index): 0.21 s; 96460 edges, 5 axioms; index 6.4 MiB + name index 0.5 MiB; peak RSS so far 21 MiB

| warm path (ms) | n | p50 | p95 | max |
|---|---|---|---|---|
| open index + metadata + modules + name index load | 5 | 0.69 | 0.77 | 0.77 |
| name search incl. fetching the visible page (20 query shapes) | 150 | 0.02 | 0.47 | 0.83 |
| documentation full-text search | 150 | 0.03 | 0.32 | 3.28 |
| direct dependencies (3 groups, ≤100 each, with counts) | 550 | 0.11 | 0.13 | 0.30 |
| direct dependents (incl. 10 hubs, largest fan-in 4941) | 550 | 0.10 | 0.32 | 0.94 |
| direct dependents, filtered (local theorems only) | 550 | 0.11 | 0.37 | 1.14 |
| declaration detail | 550 | 0.07 | 0.07 | 0.24 |
| axiom evidence with witness paths | 550 | 0.48 | 0.71 | 0.85 |
| search + dependents while another import runs | 1750 | 0.12 | 0.36 | 0.49 |

- peak RSS of the benchmark process: 33 MiB
- name index in memory: 0.5 MiB for 10012 entries
- total benchmark time: 1.0 s
