# slim measurement on an existing index

- index: ~/.cache/slim/projects/Stacks-e1c607a14c1c/source.sqlite (536.1 MiB, mode source)
- project_name: Stacks
- source_root: ~/Codes/Stacks
- project_lean_toolchain: leanprover/lean4:v4.35.0
- file_count: 62975
- decl_count: 777887
- CPU: AMD Ryzen 9 7950X 16-Core Processor; CPUs available to this process: 2
- RAM: 62.5 GiB; kernel 7.2.11-200.fc44.x86_64
- build: release

| warm path (ms) | n | p50 | p95 | max |
|---|---|---|---|---|
| open index + metadata + name index load (777887 names) | 3 | 114.56 | 127.82 | 127.82 |
| name search incl. page fetch (68 query shapes) | 204 | 1.84 | 8.40 | 30.78 |
| documentation full-text search | 204 | 1.33 | 34.49 | 42.41 |
| declaration detail | 600 | 0.05 | 0.06 | 0.12 |
| module outline (declarations of the module) | 600 | 0.03 | 0.06 | 0.13 |
| direct dependents (semantic edges; none in source mode) | 600 | 0.02 | 0.02 | 0.04 |

- peak RSS: 206 MiB; name index in memory 56.0 MiB
