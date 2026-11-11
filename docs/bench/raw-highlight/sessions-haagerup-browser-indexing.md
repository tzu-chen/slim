## haagerup: syntax + semantic while the browser's own indexing job runs (same 2 CPUs) — run 1
first frame 3 ms; search ready after 3 ms; peak RSS 28 MiB; mode source
CPU 0.70 s; read 22.2 MiB (from storage 0.0 MiB); written 2.25 MiB (to storage 0.18 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 447 | 0.34 | 0.66 | 1.93 | 5.66 |
| frame bytes (KiB) | 447 | 4.13 | 11.73 | 21.10 | 43.95 |
| highlight: file asked to spans on the UI thread | 14 | 0.27 | 3.60 | 4.15 | 4.15 |
| highlight: open to first spans | 14 | 0.02 | 2.37 | 2.37 | 2.37 |
| input event to frame written | 301 | 0.36 | 0.88 | 2.51 | 5.67 |
| source: file asked to text on the UI thread | 14 | 0.40 | 2.86 | 7.41 | 7.41 |
| ui event handling | 543 | 0.00 | 0.03 | 0.49 | 2.65 |
| worker: detail | 14 | 0.23 | 0.56 | 0.89 | 0.89 |
| worker: full source | 14 | 0.04 | 0.16 | 0.35 | 0.35 |
| worker: lexical mentions | 14 | 0.15 | 0.18 | 0.19 | 0.19 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.04 | 7.12 | 7.12 | 7.12 |

## haagerup: syntax + semantic while the browser's own indexing job runs (same 2 CPUs) — run 2
first frame 4 ms; search ready after 8 ms; peak RSS 28 MiB; mode source
CPU 0.73 s; read 24.7 MiB (from storage 0.0 MiB); written 2.41 MiB (to storage 0.37 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 445 | 0.38 | 1.00 | 3.12 | 7.91 |
| frame bytes (KiB) | 445 | 4.13 | 11.73 | 21.10 | 43.95 |
| highlight: file asked to spans on the UI thread | 14 | 0.29 | 2.42 | 2.62 | 2.62 |
| highlight: open to first spans | 14 | 0.02 | 2.35 | 4.33 | 4.33 |
| input event to frame written | 291 | 0.40 | 1.22 | 2.37 | 7.91 |
| source: file asked to text on the UI thread | 14 | 0.61 | 1.16 | 5.27 | 5.27 |
| ui event handling | 550 | 0.00 | 0.02 | 0.36 | 2.42 |
| worker: detail | 14 | 0.25 | 0.86 | 1.19 | 1.19 |
| worker: full source | 14 | 0.05 | 0.17 | 0.60 | 0.60 |
| worker: lexical mentions | 14 | 0.15 | 0.17 | 0.17 | 0.17 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.04 | 2.83 | 2.83 | 2.83 |

## haagerup: syntax + semantic while the browser's own indexing job runs (same 2 CPUs) — run 3
first frame 1 ms; search ready after 10 ms; peak RSS 27 MiB; mode source
CPU 0.67 s; read 21.6 MiB (from storage 0.0 MiB); written 2.40 MiB (to storage 0.31 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 446 | 0.36 | 0.77 | 2.72 | 7.46 |
| frame bytes (KiB) | 446 | 4.19 | 11.69 | 21.10 | 43.95 |
| highlight: file asked to spans on the UI thread | 14 | 0.33 | 2.36 | 6.25 | 6.25 |
| highlight: open to first spans | 14 | 0.02 | 1.91 | 6.26 | 6.26 |
| input event to frame written | 290 | 0.38 | 1.00 | 3.13 | 7.46 |
| source: file asked to text on the UI thread | 14 | 0.43 | 0.95 | 4.82 | 4.82 |
| ui event handling | 550 | 0.00 | 0.02 | 0.33 | 2.75 |
| worker: detail | 14 | 0.21 | 0.39 | 0.88 | 0.88 |
| worker: full source | 14 | 0.03 | 0.16 | 0.36 | 0.36 |
| worker: lexical mentions | 14 | 0.15 | 0.20 | 0.50 | 0.50 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.04 | 7.65 | 7.65 | 7.65 |

language-server processes of this user before the sessions: 3, after: 3 (equal means the browser's servers are gone)
semantic records in the cache copy: 1435 before, 1354 after the sessions
  run: cancelled: {"attempted":9,"complete":9,"detail":"","elapsed_s":28.975331085,"failed":0,"fresh":15,"lean_version":"Lean (version 4.33.1, x86_64-unknown-linux-gnu, commit 819816b2e0a3bf405af45ae5c7af2491d8f5bee6, Release)","lingering_children":0,"outcome":"cancelled","partial":0,"peak_tree_rss_mib":3924,"remaining":1411,"restarts":0,"shutdown_clean":false,"shutdown_ms":27,"skipped":0,"total":1435}
  run: cancelled: {"attempted":10,"complete":10,"detail":"","elapsed_s":29.044133164,"failed":0,"fresh":5,"lean_version":"Lean (version 4.33.1, x86_64-unknown-linux-gnu, commit 819816b2e0a3bf405af45ae5c7af2491d8f5bee6, Release)","lingering_children":0,"outcome":"cancelled","partial":0,"peak_tree_rss_mib":3964,"remaining":1420,"restarts":0,"shutdown_clean":false,"shutdown_ms":30,"skipped":0,"total":1435}
  run: cancelled: {"attempted":4,"complete":4,"detail":"","elapsed_s":28.985144661,"failed":0,"fresh":1,"lean_version":"Lean (version 4.33.1, x86_64-unknown-linux-gnu, commit 819816b2e0a3bf405af45ae5c7af2491d8f5bee6, Release)","lingering_children":0,"outcome":"cancelled","partial":0,"peak_tree_rss_mib":4449,"remaining":1430,"restarts":0,"shutdown_clean":false,"shutdown_ms":28,"skipped":0,"total":1435}
