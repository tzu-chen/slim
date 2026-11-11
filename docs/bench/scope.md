# Reading a statement in its scope: measurements

2026-11-11, release builds, 2 CPUs under `research-safe-run`, warm page cache. The same
index (built by the new scanner) is measured with the build before this work (commit 2926fa7)
and with the build after it, one `slim bench --index` run each. The summary is in
`docs/overview.md`.

## ym, before

### slim measurement on an existing index

- index: target/scope-cache/projects/YM-33018b5bb173/source.sqlite (31.0 MiB, mode source)
- project_name: YM
- source_root: ~/Codes/YM
- file_count: 9564
- decl_count: 119730
- CPU: AMD Ryzen 9 7950X 16-Core Processor; CPUs available to this process: 2
- RAM: 62.5 GiB; kernel 7.2.11-200.fc44.x86_64
- build: release

| warm path (ms) | n | p50 | p95 | max |
|---|---|---|---|---|
| open index + metadata + name index load (119730 names) | 5 | 6.53 | 10.68 | 10.68 |
| name search incl. page fetch (68 query shapes) | 340 | 0.34 | 4.46 | 5.27 |
| documentation full-text search | 340 | 0.10 | 0.24 | 0.44 |
| declaration detail | 1000 | 0.06 | 0.07 | 0.18 |
| module outline (declarations of the module) | 1000 | 0.03 | 0.05 | 0.09 |
| direct dependents (semantic edges; none in source mode) | 1000 | 0.03 | 0.03 | 0.09 |
| outline tree with per-directory counts (10326 nodes) | 5 | 10.86 | 11.45 | 11.45 |
| name search within a namespace incl. page fetch | 340 | 0.00 | 0.03 | 0.06 |
| name search within files (a file, its directory, a top-level directory; 41 scopes) incl. page fetch | 340 | 0.29 | 0.52 | 0.93 |
| resolving a file scope to module ids (once per scope) | 41 | 0.01 | 0.02 | 0.57 |
| skim of a file (statements read from the source) | 120 | 0.12 | 0.18 | 0.20 |
| skim of a directory (bounded: 1500 declarations, 150 files; 417 declarations on average here) | 120 | 1.68 | 7.94 | 12.14 |
| skim of a namespace | 118 | 0.87 | 6.55 | 14.41 |
| lexical mentions of a declaration, with the position of every occurrence | 120 | 0.18 | 0.41 | 0.85 |
| import graph: read, depths, directory tree (9564 local files, 0 import lines between them) | 5 | 10.70 | 13.12 | 13.12 |
| imports of a directory, grouped (whole project and its first-level directories) | 10 | 1.65 | 1.77 | 1.77 |
| import path between two files (exhaustive search) | 320 | 0.00 | 0.00 | 0.00 |

- import graph: 9552 endpoints, 10 import-only files, deepest chain 0, about 4.9 MiB in memory
- peak RSS: 45 MiB before the overview measurements, 54 MiB after; name index in memory 6.4 MiB

## ym, after

### slim measurement on an existing index

- index: target/scope-cache/projects/YM-33018b5bb173/source.sqlite (31.0 MiB, mode source)
- project_name: YM
- source_root: ~/Codes/YM
- file_count: 9564
- decl_count: 119730
- CPU: AMD Ryzen 9 7950X 16-Core Processor; CPUs available to this process: 2
- RAM: 62.5 GiB; kernel 7.2.11-200.fc44.x86_64
- build: release

| warm path (ms) | n | p50 | p95 | max |
|---|---|---|---|---|
| open index + metadata + name index load (119730 names) | 5 | 5.75 | 9.15 | 9.15 |
| name search incl. page fetch (68 query shapes) | 340 | 0.30 | 3.89 | 4.61 |
| documentation full-text search | 340 | 0.09 | 0.21 | 0.29 |
| declaration detail | 1000 | 0.05 | 0.06 | 0.26 |
| module outline (declarations of the module) | 1000 | 0.03 | 0.04 | 0.10 |
| direct dependents (semantic edges; none in source mode) | 1000 | 0.02 | 0.03 | 0.08 |
| outline tree with per-directory counts (10326 nodes) | 5 | 10.34 | 12.16 | 12.16 |
| name search within a namespace incl. page fetch | 340 | 0.00 | 0.03 | 0.06 |
| name search within files (a file, its directory, a top-level directory; 41 scopes) incl. page fetch | 340 | 0.32 | 0.51 | 0.96 |
| resolving a file scope to module ids (once per scope) | 41 | 0.01 | 0.02 | 0.58 |
| skim of a file (statements read from the source) | 120 | 0.13 | 0.18 | 0.24 |
| skim of a directory (bounded: 1500 declarations, 150 files; 417 declarations on average here) | 120 | 2.06 | 9.53 | 14.76 |
| skim of a namespace | 118 | 0.85 | 6.31 | 14.84 |
| lexical mentions of a declaration, with the position of every occurrence | 120 | 0.30 | 1.11 | 2.62 |
| scope of a declaration (its variable and open lines; asked after the declaration is shown) | 120 | 0.10 | 0.13 | 0.16 |
| card of a mentioned declaration (statement and lead of its documentation) | 120 | 0.04 | 0.04 | 0.07 |
| import graph: read, depths, directory tree (9564 local files, 0 import lines between them) | 5 | 10.82 | 13.01 | 13.01 |
| imports of a directory, grouped (whole project and its first-level directories) | 10 | 1.68 | 1.74 | 1.74 |
| import path between two files (exhaustive search) | 320 | 0.00 | 0.00 | 0.00 |

- import graph: 9552 endpoints, 10 import-only files, deepest chain 0, about 4.9 MiB in memory
- peak RSS: 45 MiB before the overview measurements, 57 MiB after; name index in memory 6.4 MiB

## cfsg, before

### slim measurement on an existing index

- index: target/scope-cache/projects/cfsg-78b45ae094c6/source.sqlite (652.4 MiB, mode source)
- project_name: cfsg
- source_root: ~/Codes/CFSG
- project_lean_toolchain: leanprover/lean4:v4.35.0-rc3
- file_count: 229089
- decl_count: 2362652
- CPU: AMD Ryzen 9 7950X 16-Core Processor; CPUs available to this process: 2
- RAM: 62.5 GiB; kernel 7.2.11-200.fc44.x86_64
- build: release

| warm path (ms) | n | p50 | p95 | max |
|---|---|---|---|---|
| open index + metadata + name index load (2362652 names) | 5 | 154.78 | 165.51 | 165.51 |
| name search incl. page fetch (68 query shapes) | 340 | 4.60 | 30.03 | 30.75 |
| documentation full-text search | 340 | 0.13 | 0.66 | 12.34 |
| declaration detail | 1000 | 0.06 | 0.06 | 0.16 |
| module outline (declarations of the module) | 1000 | 0.03 | 0.10 | 3.34 |
| direct dependents (semantic edges; none in source mode) | 1000 | 0.03 | 0.03 | 0.11 |
| outline tree with per-directory counts (233767 nodes) | 5 | 293.28 | 306.31 | 306.31 |
| name search within a namespace incl. page fetch | 340 | 0.02 | 1.03 | 6.23 |
| name search within files (a file, its directory, a top-level directory; 41 scopes) incl. page fetch | 340 | 8.10 | 16.70 | 26.57 |
| resolving a file scope to module ids (once per scope) | 41 | 0.02 | 1.00 | 14.15 |
| skim of a file (statements read from the source) | 120 | 0.14 | 0.35 | 6.22 |
| skim of a directory (bounded: 1500 declarations, 150 files; 1227 declarations on average here) | 120 | 9.70 | 12.94 | 16.35 |
| skim of a namespace | 120 | 2.26 | 17.66 | 54.80 |
| lexical mentions of a declaration, with the position of every occurrence | 120 | 0.21 | 0.58 | 1.77 |
| import graph: read, depths, directory tree (229089 local files, 1343264 import lines between them) | 5 | 307.70 | 341.76 | 341.76 |
| imports of a directory, grouped (whole project and its first-level directories) | 450 | 0.01 | 1.57 | 40.16 |
| import path between two files (exhaustive search) | 320 | 1.29 | 3.36 | 4.56 |

- import graph: 728 endpoints, 7943 import-only files, deepest chain 619, about 82.8 MiB in memory
- peak RSS: 534 MiB before the overview measurements, 534 MiB after; name index in memory 162.2 MiB

## cfsg, after

### slim measurement on an existing index

- index: target/scope-cache/projects/cfsg-78b45ae094c6/source.sqlite (652.4 MiB, mode source)
- project_name: cfsg
- source_root: ~/Codes/CFSG
- project_lean_toolchain: leanprover/lean4:v4.35.0-rc3
- file_count: 229089
- decl_count: 2362652
- CPU: AMD Ryzen 9 7950X 16-Core Processor; CPUs available to this process: 2
- RAM: 62.5 GiB; kernel 7.2.11-200.fc44.x86_64
- build: release

| warm path (ms) | n | p50 | p95 | max |
|---|---|---|---|---|
| open index + metadata + name index load (2362652 names) | 5 | 157.61 | 170.36 | 170.36 |
| name search incl. page fetch (68 query shapes) | 340 | 4.79 | 28.31 | 30.72 |
| documentation full-text search | 340 | 0.13 | 0.67 | 11.90 |
| declaration detail | 1000 | 0.06 | 0.06 | 0.12 |
| module outline (declarations of the module) | 1000 | 0.03 | 0.10 | 3.34 |
| direct dependents (semantic edges; none in source mode) | 1000 | 0.03 | 0.03 | 0.15 |
| outline tree with per-directory counts (233767 nodes) | 5 | 285.55 | 308.65 | 308.65 |
| name search within a namespace incl. page fetch | 340 | 0.02 | 1.02 | 6.33 |
| name search within files (a file, its directory, a top-level directory; 41 scopes) incl. page fetch | 340 | 8.68 | 16.61 | 26.87 |
| resolving a file scope to module ids (once per scope) | 41 | 0.02 | 1.00 | 14.18 |
| skim of a file (statements read from the source) | 120 | 0.16 | 0.39 | 5.66 |
| skim of a directory (bounded: 1500 declarations, 150 files; 1227 declarations on average here) | 120 | 10.87 | 14.38 | 15.89 |
| skim of a namespace | 120 | 2.29 | 18.11 | 60.12 |
| lexical mentions of a declaration, with the position of every occurrence | 120 | 0.31 | 1.44 | 3.94 |
| scope of a declaration (its variable and open lines; asked after the declaration is shown) | 120 | 0.09 | 0.11 | 0.15 |
| card of a mentioned declaration (statement and lead of its documentation) | 120 | 0.04 | 0.04 | 0.08 |
| import graph: read, depths, directory tree (229089 local files, 1343264 import lines between them) | 5 | 316.18 | 342.77 | 342.77 |
| imports of a directory, grouped (whole project and its first-level directories) | 450 | 0.01 | 1.63 | 41.13 |
| import path between two files (exhaustive search) | 320 | 1.35 | 3.50 | 5.14 |

- import graph: 728 endpoints, 7943 import-only files, deepest chain 619, about 82.8 MiB in memory
- peak RSS: 526 MiB before the overview measurements, 526 MiB after; name index in memory 162.2 MiB
