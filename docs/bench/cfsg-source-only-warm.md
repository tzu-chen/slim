# slim measurement on an existing index

- index: ~/.cache/slim/projects/cfsg-78b45ae094c6/source.sqlite (650.3 MiB, mode source)
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
| open index + metadata + name index load (2364132 names) | 3 | 251.55 | 270.59 | 270.59 |
| name search incl. page fetch (68 query shapes) | 204 | 4.78 | 30.51 | 31.06 |
| documentation full-text search | 204 | 0.14 | 0.42 | 1.02 |
| declaration detail | 600 | 0.05 | 0.06 | 0.11 |
| module outline (declarations of the module) | 600 | 0.03 | 0.07 | 3.32 |
| direct dependents (semantic edges; none in source mode) | 600 | 0.02 | 0.02 | 0.03 |

- peak RSS: 498 MiB; name index in memory 153.2 MiB
