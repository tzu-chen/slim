# slim measurement on an existing index

- index: target/ov-cache/projects/cfsg-78b45ae094c6/source.sqlite (652.8 MiB, mode source)
- project_name: cfsg
- source_root: ~/Codes/CFSG
- project_lean_toolchain: leanprover/lean4:v4.35.0-rc3
- file_count: 229089
- decl_count: 2364132
- CPU: AMD Ryzen 9 7950X 16-Core Processor; CPUs available to this process: 2
- RAM: 62.5 GiB; kernel 7.2.11-200.fc44.x86_64
- build: release

| warm path (ms) | n | p50 | p95 | max |
|---|---|---|---|---|
| open index + metadata + name index load (2364132 names) | 3 | 184.37 | 200.64 | 200.64 |
| name search incl. page fetch (68 query shapes) | 204 | 4.40 | 30.33 | 30.73 |
| documentation full-text search | 204 | 0.15 | 0.45 | 1.09 |
| declaration detail | 600 | 0.05 | 0.06 | 0.12 |
| module outline (declarations of the module) | 600 | 0.03 | 0.08 | 3.54 |
| direct dependents (semantic edges; none in source mode) | 600 | 0.02 | 0.03 | 0.05 |
| outline tree with per-directory counts (233767 nodes) | 3 | 300.58 | 326.43 | 326.43 |
| name search within a namespace incl. page fetch | 204 | 0.03 | 0.28 | 0.48 |
| name search within files (a file, its directory, a top-level directory; 41 scopes) incl. page fetch | 204 | 8.90 | 17.63 | 28.14 |
| resolving a file scope to module ids (once per scope) | 41 | 0.02 | 1.02 | 14.77 |
| skim of a file (statements read from the source) | 120 | 0.14 | 0.30 | 0.44 |
| skim of a directory (bounded: 1500 declarations, 150 files; 1174 declarations on average here) | 120 | 10.08 | 13.44 | 24.55 |
| skim of a namespace | 120 | 2.73 | 15.90 | 84.82 |
| lexical mentions of a declaration, with the position of every occurrence | 120 | 0.24 | 0.52 | 0.93 |
| import graph: read, depths, directory tree (229089 local files, 1343264 import lines between them) | 3 | 325.60 | 344.89 | 344.89 |
| imports of a directory, grouped (whole project and its first-level directories) | 270 | 0.01 | 1.70 | 42.49 |
| import path between two files (exhaustive search) | 192 | 1.41 | 3.69 | 5.21 |

- import graph: 728 endpoints, 7943 import-only files, deepest chain 619, about 82.8 MiB in memory
- peak RSS: 526 MiB before the overview measurements, 526 MiB after; name index in memory 162.3 MiB
