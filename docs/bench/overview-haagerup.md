# slim measurement on an existing index

- index: target/ov-cache/projects/HaagerupIzumi-0775d1c6f2b6/source.sqlite (5.7 MiB, mode source)
- project_name: HaagerupIzumi
- source_root: ~/Codes/haagerup/lean
- project_lean_toolchain: leanprover/lean4:v4.33.1
- file_count: 1435
- decl_count: 15390
- CPU: AMD Ryzen 9 7950X 16-Core Processor; CPUs available to this process: 2
- RAM: 62.5 GiB; kernel 7.2.11-200.fc44.x86_64
- build: release

| warm path (ms) | n | p50 | p95 | max |
|---|---|---|---|---|
| open index + metadata + name index load (15390 names) | 3 | 0.93 | 1.45 | 1.45 |
| name search incl. page fetch (68 query shapes) | 204 | 0.14 | 0.58 | 0.92 |
| documentation full-text search | 204 | 0.04 | 0.20 | 0.31 |
| declaration detail | 600 | 0.05 | 0.06 | 0.10 |
| module outline (declarations of the module) | 600 | 0.04 | 0.25 | 0.55 |
| direct dependents (semantic edges; none in source mode) | 600 | 0.02 | 0.03 | 0.04 |
| outline tree with per-directory counts (1462 nodes) | 3 | 1.17 | 1.58 | 1.58 |
| name search within a namespace incl. page fetch | 204 | 0.05 | 0.52 | 0.66 |
| name search within files (a file, its directory, a top-level directory; 27 scopes) incl. page fetch | 204 | 0.06 | 0.32 | 0.73 |
| resolving a file scope to module ids (once per scope) | 27 | 0.01 | 0.09 | 0.10 |
| skim of a file (statements read from the source) | 120 | 0.16 | 0.63 | 1.17 |
| skim of a directory (bounded: 1500 declarations, 150 files; 1199 declarations on average here) | 120 | 10.95 | 11.27 | 11.37 |
| skim of a namespace | 120 | 14.34 | 15.01 | 16.26 |
| lexical mentions of a declaration, with the position of every occurrence | 120 | 0.16 | 0.36 | 0.61 |
| import graph: read, depths, directory tree (1435 local files, 5845 import lines between them) | 3 | 1.19 | 1.24 | 1.24 |
| imports of a directory, grouped (whole project and its first-level directories) | 75 | 0.00 | 0.06 | 0.08 |
| import path between two files (exhaustive search) | 192 | 0.00 | 0.00 | 0.01 |

- import graph: 89 endpoints, 4 import-only files, deepest chain 28, about 0.5 MiB in memory
- peak RSS: 15 MiB before the overview measurements, 18 MiB after; name index in memory 0.9 MiB
