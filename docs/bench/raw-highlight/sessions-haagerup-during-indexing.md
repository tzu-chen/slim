## haagerup: build before highlighting (3433d31) — run 1
first frame 1 ms; search ready after 2 ms; peak RSS 19 MiB; mode source
CPU 0.16 s; read 6.9 MiB (from storage 0.0 MiB); written 1.28 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 323 | 0.42 | 0.57 | - | 1.15 |
| frame bytes (KiB) | 323 | 3.68 | 8.33 | - | 30.60 |
| ui event handling | 384 | 0.00 | 0.01 | - | 0.18 |
| worker: detail | 14 | 0.28 | 0.40 | - | 0.98 |
| worker: full source | 14 | 0.04 | 0.23 | - | 0.38 |
| worker: lexical mentions | 14 | 0.19 | 0.24 | - | 0.24 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | - | 0.00 |
| worker: other | 3 | 0.04 | 4.18 | - | 4.18 |

## haagerup: highlighting off — run 1
first frame 2 ms; search ready after 3 ms; peak RSS 21 MiB; mode source
CPU 0.16 s; read 6.8 MiB (from storage 0.0 MiB); written 1.23 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 322 | 0.40 | 0.56 | 0.83 | 1.38 |
| frame bytes (KiB) | 322 | 3.68 | 8.33 | 10.12 | 30.62 |
| input event to frame written | 280 | 0.44 | 0.59 | 0.70 | 1.06 |
| source: file asked to text on the UI thread | 14 | 0.36 | 0.57 | 0.62 | 0.62 |
| ui event handling | 384 | 0.00 | 0.01 | 0.17 | 0.37 |
| worker: detail | 14 | 0.25 | 0.48 | 0.91 | 0.91 |
| worker: full source | 14 | 0.05 | 0.17 | 0.60 | 0.60 |
| worker: lexical mentions | 14 | 0.18 | 0.21 | 0.23 | 0.23 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.04 | 7.33 | 7.33 | 7.33 |

## haagerup: syntax only, nothing cached on disk (every file tokenized in this session) — run 1
first frame 2 ms; search ready after 4 ms; peak RSS 26 MiB; mode source
CPU 0.27 s; read 7.3 MiB (from storage 0.0 MiB); written 2.14 MiB (to storage 0.56 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 340 | 0.41 | 0.59 | 0.84 | 1.28 |
| frame bytes (KiB) | 340 | 4.70 | 8.63 | 13.24 | 34.46 |
| highlight: file asked to spans on the UI thread | 14 | 0.30 | 2.58 | 3.33 | 3.33 |
| highlight: open to first spans | 14 | 0.02 | 2.22 | 2.22 | 2.22 |
| input event to frame written | 280 | 0.46 | 0.61 | 0.85 | 1.27 |
| source: file asked to text on the UI thread | 14 | 0.31 | 0.46 | 0.58 | 0.58 |
| ui event handling | 402 | 0.00 | 0.01 | 0.11 | 0.46 |
| worker: detail | 14 | 0.25 | 0.37 | 1.14 | 1.14 |
| worker: full source | 14 | 0.04 | 0.23 | 0.36 | 0.36 |
| worker: lexical mentions | 14 | 0.14 | 0.24 | 0.25 | 0.25 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.04 | 5.30 | 5.30 | 5.30 |

## haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) — run 1
first frame 3 ms; search ready after 3 ms; peak RSS 23 MiB; mode source
CPU 0.20 s; read 8.5 MiB (from storage 0.0 MiB); written 2.33 MiB (to storage 0.03 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 338 | 0.42 | 0.69 | 0.93 | 1.27 |
| frame bytes (KiB) | 338 | 7.23 | 11.73 | 21.19 | 43.91 |
| highlight: file asked to spans on the UI thread | 14 | 0.32 | 1.76 | 2.42 | 2.42 |
| highlight: open to first spans | 14 | 0.02 | 2.03 | 2.30 | 2.30 |
| input event to frame written | 280 | 0.47 | 0.73 | 0.97 | 1.27 |
| source: file asked to text on the UI thread | 14 | 0.28 | 0.45 | 0.69 | 0.69 |
| ui event handling | 403 | 0.00 | 0.01 | 0.17 | 0.45 |
| worker: detail | 14 | 0.24 | 0.49 | 0.88 | 0.88 |
| worker: full source | 14 | 0.06 | 0.21 | 0.66 | 0.66 |
| worker: lexical mentions | 14 | 0.14 | 0.22 | 0.25 | 0.25 |
| worker: map load + resolve | 1 | 0.01 | 0.01 | 0.01 | 0.01 |
| worker: other | 3 | 0.04 | 3.71 | 3.71 | 3.71 |

## haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) — run 1
first frame 2 ms; search ready after 8 ms; peak RSS 24 MiB; mode source
CPU 0.19 s; read 9.3 MiB (from storage 0.0 MiB); written 2.32 MiB (to storage 0.03 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 338 | 0.43 | 0.68 | 0.96 | 1.16 |
| frame bytes (KiB) | 338 | 7.23 | 11.73 | 21.19 | 43.91 |
| highlight: file asked to spans on the UI thread | 14 | 0.29 | 2.61 | 2.94 | 2.94 |
| highlight: open to first spans | 14 | 0.03 | 2.07 | 2.27 | 2.27 |
| input event to frame written | 280 | 0.48 | 0.71 | 1.02 | 3.27 |
| source: file asked to text on the UI thread | 14 | 0.30 | 0.49 | 0.60 | 0.60 |
| ui event handling | 403 | 0.00 | 0.01 | 0.15 | 2.86 |
| worker: detail | 14 | 0.21 | 0.36 | 1.05 | 1.05 |
| worker: full source | 14 | 0.05 | 0.18 | 0.38 | 0.38 |
| worker: lexical mentions | 14 | 0.14 | 0.19 | 0.20 | 0.20 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.05 | 6.03 | 6.03 | 6.03 |

## haagerup: syntax only, nothing cached on disk (every file tokenized in this session) — run 1
first frame 2 ms; search ready after 3 ms; peak RSS 26 MiB; mode source
CPU 0.26 s; read 7.4 MiB (from storage 0.0 MiB); written 2.08 MiB (to storage 0.56 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 342 | 0.37 | 0.60 | 0.73 | 1.23 |
| frame bytes (KiB) | 342 | 4.68 | 8.30 | 13.24 | 34.46 |
| highlight: file asked to spans on the UI thread | 14 | 0.31 | 2.50 | 3.49 | 3.49 |
| highlight: open to first spans | 14 | 0.02 | 2.22 | 2.37 | 2.37 |
| input event to frame written | 280 | 0.41 | 0.61 | 0.74 | 1.24 |
| source: file asked to text on the UI thread | 14 | 0.32 | 0.50 | 0.61 | 0.61 |
| ui event handling | 402 | 0.00 | 0.01 | 0.11 | 0.42 |
| worker: detail | 14 | 0.22 | 0.50 | 0.96 | 0.96 |
| worker: full source | 14 | 0.05 | 0.18 | 0.38 | 0.38 |
| worker: lexical mentions | 14 | 0.14 | 0.21 | 0.30 | 0.30 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.04 | 3.83 | 3.83 | 3.83 |

## haagerup: highlighting off — run 1
first frame 2 ms; search ready after 4 ms; peak RSS 20 MiB; mode source
CPU 0.16 s; read 6.8 MiB (from storage 0.0 MiB); written 1.23 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 325 | 0.38 | 0.55 | 0.83 | 1.46 |
| frame bytes (KiB) | 325 | 3.68 | 8.33 | 10.12 | 30.62 |
| input event to frame written | 280 | 0.43 | 0.56 | 0.77 | 1.04 |
| source: file asked to text on the UI thread | 14 | 0.33 | 0.44 | 0.51 | 0.51 |
| ui event handling | 384 | 0.00 | 0.01 | 0.11 | 0.41 |
| worker: detail | 14 | 0.23 | 0.37 | 0.92 | 0.92 |
| worker: full source | 14 | 0.04 | 0.26 | 0.36 | 0.36 |
| worker: lexical mentions | 14 | 0.15 | 0.20 | 0.31 | 0.31 |
| worker: map load + resolve | 1 | 0.01 | 0.01 | 0.01 | 0.01 |
| worker: other | 3 | 0.09 | 4.41 | 4.41 | 4.41 |

## haagerup: build before highlighting (3433d31) — run 1
first frame 1 ms; search ready after 5 ms; peak RSS 18 MiB; mode source
CPU 0.16 s; read 6.7 MiB (from storage 0.0 MiB); written 1.23 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 322 | 0.38 | 0.56 | - | 1.92 |
| frame bytes (KiB) | 322 | 3.71 | 8.33 | - | 30.60 |
| ui event handling | 384 | 0.00 | 0.01 | - | 0.76 |
| worker: detail | 14 | 0.25 | 0.52 | - | 1.02 |
| worker: full source | 14 | 0.05 | 0.22 | - | 0.57 |
| worker: lexical mentions | 14 | 0.18 | 0.23 | - | 0.25 |
| worker: map load + resolve | 1 | 0.01 | 0.01 | - | 0.01 |
| worker: other | 3 | 0.26 | 4.11 | - | 4.11 |

## haagerup: highlighting off — run 1
first frame 2 ms; search ready after 4 ms; peak RSS 21 MiB; mode source
CPU 0.17 s; read 7.4 MiB (from storage 0.0 MiB); written 1.23 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 323 | 0.46 | 0.57 | 0.88 | 1.45 |
| frame bytes (KiB) | 323 | 3.71 | 8.33 | 10.12 | 30.62 |
| input event to frame written | 280 | 0.48 | 0.59 | 0.89 | 3.36 |
| source: file asked to text on the UI thread | 14 | 0.24 | 0.43 | 0.48 | 0.48 |
| ui event handling | 384 | 0.00 | 0.01 | 0.15 | 2.86 |
| worker: detail | 14 | 0.23 | 0.71 | 0.96 | 0.96 |
| worker: full source | 14 | 0.05 | 0.18 | 0.46 | 0.46 |
| worker: lexical mentions | 14 | 0.15 | 0.20 | 0.24 | 0.24 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.09 | 3.11 | 3.11 | 3.11 |

## haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) — run 1
first frame 2 ms; search ready after 5 ms; peak RSS 24 MiB; mode source
CPU 0.19 s; read 8.6 MiB (from storage 0.0 MiB); written 2.34 MiB (to storage 0.03 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 338 | 0.43 | 0.69 | 0.91 | 1.47 |
| frame bytes (KiB) | 338 | 7.32 | 11.73 | 21.19 | 43.91 |
| highlight: file asked to spans on the UI thread | 14 | 0.31 | 1.81 | 2.75 | 2.75 |
| highlight: open to first spans | 14 | 0.02 | 2.10 | 2.38 | 2.38 |
| input event to frame written | 280 | 0.48 | 0.72 | 0.93 | 1.47 |
| source: file asked to text on the UI thread | 14 | 0.24 | 0.39 | 0.44 | 0.44 |
| ui event handling | 403 | 0.00 | 0.01 | 0.14 | 0.64 |
| worker: detail | 14 | 0.24 | 0.48 | 1.00 | 1.00 |
| worker: full source | 14 | 0.04 | 0.17 | 0.39 | 0.39 |
| worker: lexical mentions | 14 | 0.16 | 0.21 | 0.25 | 0.25 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.04 | 6.72 | 6.72 | 6.72 |

## haagerup: build before highlighting (3433d31) — run 1
first frame 3 ms; search ready after 8 ms; peak RSS 18 MiB; mode source
CPU 0.16 s; read 6.8 MiB (from storage 0.0 MiB); written 1.23 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 323 | 0.40 | 0.57 | - | 4.71 |
| frame bytes (KiB) | 323 | 3.68 | 8.33 | - | 30.60 |
| ui event handling | 384 | 0.00 | 0.01 | - | 0.16 |
| worker: detail | 14 | 0.26 | 0.36 | - | 0.88 |
| worker: full source | 14 | 0.05 | 0.22 | - | 0.60 |
| worker: lexical mentions | 14 | 0.15 | 0.21 | - | 0.26 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | - | 0.00 |
| worker: other | 3 | 0.03 | 2.94 | - | 2.94 |

## haagerup: syntax only, nothing cached on disk (every file tokenized in this session) — run 1
first frame 3 ms; search ready after 3 ms; peak RSS 26 MiB; mode source
CPU 0.27 s; read 7.3 MiB (from storage 0.0 MiB); written 2.08 MiB (to storage 0.56 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 341 | 0.41 | 0.61 | 0.98 | 1.43 |
| frame bytes (KiB) | 341 | 4.70 | 8.30 | 13.24 | 34.46 |
| highlight: file asked to spans on the UI thread | 14 | 0.29 | 2.47 | 2.59 | 2.59 |
| highlight: open to first spans | 14 | 0.02 | 2.12 | 2.34 | 2.34 |
| input event to frame written | 280 | 0.48 | 0.63 | 0.99 | 1.43 |
| source: file asked to text on the UI thread | 14 | 0.25 | 0.50 | 0.50 | 0.50 |
| ui event handling | 402 | 0.00 | 0.01 | 0.12 | 0.32 |
| worker: detail | 14 | 0.22 | 0.54 | 0.84 | 0.84 |
| worker: full source | 14 | 0.06 | 0.19 | 0.39 | 0.39 |
| worker: lexical mentions | 14 | 0.14 | 0.21 | 0.23 | 0.23 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.06 | 3.15 | 3.15 | 3.15 |

## haagerup: idle minute, syntax + semantic — run 1
first frame 2 ms; search ready after 3 ms; peak RSS 25 MiB; mode source
CPU 0.22 s; read 9.6 MiB (from storage 0.0 MiB); written 2.33 MiB (to storage 0.03 MiB)
idle for 60 s: 0 ms of CPU time
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 336 | 0.51 | 0.69 | 0.87 | 1.22 |
| frame bytes (KiB) | 336 | 7.32 | 11.73 | 21.19 | 43.91 |
| highlight: file asked to spans on the UI thread | 14 | 0.30 | 1.78 | 3.03 | 3.03 |
| highlight: open to first spans | 14 | 0.03 | 2.05 | 2.96 | 2.96 |
| input event to frame written | 280 | 0.55 | 0.72 | 1.01 | 2.75 |
| source: file asked to text on the UI thread | 14 | 0.32 | 0.56 | 0.61 | 0.61 |
| ui event handling | 404 | 0.00 | 0.01 | 0.16 | 2.37 |
| worker: detail | 14 | 0.22 | 0.54 | 0.89 | 0.89 |
| worker: full source | 14 | 0.04 | 0.21 | 0.52 | 0.52 |
| worker: lexical mentions | 14 | 0.14 | 0.24 | 0.27 | 0.27 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.04 | 4.82 | 4.82 | 4.82 |

## haagerup: idle minute, build before highlighting — run 1
first frame 2 ms; search ready after 2 ms; peak RSS 18 MiB; mode source
CPU 0.16 s; read 6.8 MiB (from storage 0.0 MiB); written 1.23 MiB (to storage 0.00 MiB)
idle for 60 s: 0 ms of CPU time
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 325 | 0.36 | 0.57 | - | 1.02 |
| frame bytes (KiB) | 325 | 3.68 | 8.33 | - | 30.60 |
| ui event handling | 384 | 0.00 | 0.01 | - | 0.18 |
| worker: detail | 14 | 0.28 | 0.38 | - | 0.93 |
| worker: full source | 14 | 0.05 | 0.26 | - | 0.63 |
| worker: lexical mentions | 14 | 0.15 | 0.24 | - | 0.58 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | - | 0.00 |
| worker: other | 3 | 0.04 | 3.20 | - | 3.20 |

