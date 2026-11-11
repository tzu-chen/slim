## haagerup: build before highlighting (3433d31) — run 1
first frame 1 ms; search ready after 4 ms; peak RSS 18 MiB; mode source
CPU 0.15 s; read 6.8 MiB (from storage 0.0 MiB); written 1.23 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 326 | 0.34 | 0.54 | - | 1.02 |
| frame bytes (KiB) | 326 | 3.67 | 8.33 | - | 30.60 |
| ui event handling | 384 | 0.00 | 0.01 | - | 0.19 |
| worker: detail | 14 | 0.21 | 0.45 | - | 0.94 |
| worker: full source | 14 | 0.04 | 0.18 | - | 0.37 |
| worker: lexical mentions | 14 | 0.15 | 0.20 | - | 0.22 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | - | 0.00 |
| worker: other | 3 | 0.03 | 4.93 | - | 4.93 |

## haagerup: highlighting off — run 1
first frame 2 ms; search ready after 3 ms; peak RSS 21 MiB; mode source
CPU 0.17 s; read 6.9 MiB (from storage 0.0 MiB); written 1.23 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 324 | 0.45 | 0.58 | 0.84 | 1.04 |
| frame bytes (KiB) | 324 | 3.67 | 8.33 | 10.12 | 30.62 |
| input event to frame written | 280 | 0.48 | 0.59 | 0.86 | 1.04 |
| source: file asked to text on the UI thread | 14 | 0.33 | 0.43 | 0.47 | 0.47 |
| ui event handling | 384 | 0.00 | 0.01 | 0.15 | 0.44 |
| worker: detail | 14 | 0.21 | 0.38 | 0.91 | 0.91 |
| worker: full source | 14 | 0.05 | 0.19 | 0.38 | 0.38 |
| worker: lexical mentions | 14 | 0.14 | 0.15 | 0.22 | 0.22 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.28 | 3.58 | 3.58 | 3.58 |

## haagerup: syntax only, nothing cached on disk (every file tokenized in this session) — run 1
first frame 2 ms; search ready after 3 ms; peak RSS 26 MiB; mode source
CPU 0.25 s; read 7.3 MiB (from storage 0.0 MiB); written 2.08 MiB (to storage 0.56 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 342 | 0.36 | 0.58 | 0.84 | 1.06 |
| frame bytes (KiB) | 342 | 4.70 | 8.30 | 13.24 | 34.46 |
| highlight: file asked to spans on the UI thread | 14 | 0.28 | 3.39 | 3.46 | 3.46 |
| highlight: open to first spans | 14 | 0.02 | 2.20 | 2.24 | 2.24 |
| input event to frame written | 280 | 0.38 | 0.60 | 0.84 | 1.06 |
| source: file asked to text on the UI thread | 14 | 0.32 | 0.49 | 0.65 | 0.65 |
| ui event handling | 402 | 0.00 | 0.01 | 0.15 | 0.28 |
| worker: detail | 14 | 0.22 | 0.36 | 0.88 | 0.88 |
| worker: full source | 14 | 0.04 | 0.25 | 0.40 | 0.40 |
| worker: lexical mentions | 14 | 0.15 | 0.24 | 0.62 | 0.62 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.03 | 5.35 | 5.35 | 5.35 |

## haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) — run 1
first frame 2 ms; search ready after 3 ms; peak RSS 24 MiB; mode source
CPU 0.17 s; read 8.6 MiB (from storage 0.0 MiB); written 2.33 MiB (to storage 0.03 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 337 | 0.38 | 0.65 | 0.74 | 1.18 |
| frame bytes (KiB) | 337 | 7.28 | 11.78 | 21.19 | 43.91 |
| highlight: file asked to spans on the UI thread | 14 | 0.30 | 2.12 | 2.70 | 2.70 |
| highlight: open to first spans | 14 | 0.02 | 1.95 | 2.35 | 2.35 |
| input event to frame written | 280 | 0.40 | 0.67 | 0.84 | 1.19 |
| source: file asked to text on the UI thread | 14 | 0.30 | 0.45 | 0.45 | 0.45 |
| ui event handling | 404 | 0.00 | 0.01 | 0.14 | 0.70 |
| worker: detail | 14 | 0.23 | 0.38 | 0.85 | 0.85 |
| worker: full source | 14 | 0.04 | 0.22 | 0.39 | 0.39 |
| worker: lexical mentions | 14 | 0.14 | 0.21 | 0.22 | 0.22 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.04 | 2.93 | 2.93 | 2.93 |

## haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) — run 1
first frame 2 ms; search ready after 3 ms; peak RSS 24 MiB; mode source
CPU 0.19 s; read 8.6 MiB (from storage 0.0 MiB); written 2.33 MiB (to storage 0.03 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 341 | 0.45 | 0.67 | 0.99 | 1.55 |
| frame bytes (KiB) | 341 | 7.23 | 11.78 | 21.19 | 43.91 |
| highlight: file asked to spans on the UI thread | 14 | 0.31 | 2.00 | 2.69 | 2.69 |
| highlight: open to first spans | 14 | 0.03 | 2.07 | 2.63 | 2.63 |
| input event to frame written | 280 | 0.50 | 0.68 | 0.99 | 1.56 |
| source: file asked to text on the UI thread | 14 | 0.30 | 0.48 | 0.77 | 0.77 |
| ui event handling | 404 | 0.00 | 0.01 | 0.16 | 0.43 |
| worker: detail | 14 | 0.25 | 0.41 | 0.93 | 0.93 |
| worker: full source | 14 | 0.05 | 0.31 | 0.40 | 0.40 |
| worker: lexical mentions | 14 | 0.18 | 0.24 | 0.64 | 0.64 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.03 | 5.48 | 5.48 | 5.48 |

## haagerup: syntax only, nothing cached on disk (every file tokenized in this session) — run 1
first frame 2 ms; search ready after 8 ms; peak RSS 26 MiB; mode source
CPU 0.27 s; read 7.3 MiB (from storage 0.0 MiB); written 2.08 MiB (to storage 0.56 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 340 | 0.45 | 0.59 | 0.81 | 1.29 |
| frame bytes (KiB) | 340 | 4.70 | 8.30 | 13.24 | 34.46 |
| highlight: file asked to spans on the UI thread | 14 | 0.31 | 2.51 | 2.65 | 2.65 |
| highlight: open to first spans | 14 | 0.02 | 2.36 | 2.51 | 2.51 |
| input event to frame written | 280 | 0.47 | 0.63 | 0.83 | 1.29 |
| source: file asked to text on the UI thread | 14 | 0.32 | 0.53 | 0.77 | 0.77 |
| ui event handling | 402 | 0.00 | 0.01 | 0.16 | 0.41 |
| worker: detail | 14 | 0.23 | 0.41 | 1.17 | 1.17 |
| worker: full source | 14 | 0.05 | 0.27 | 0.50 | 0.50 |
| worker: lexical mentions | 14 | 0.16 | 0.22 | 0.61 | 0.61 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.03 | 8.23 | 8.23 | 8.23 |

## haagerup: highlighting off — run 1
first frame 2 ms; search ready after 4 ms; peak RSS 21 MiB; mode source
CPU 0.17 s; read 6.8 MiB (from storage 0.0 MiB); written 1.23 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 324 | 0.41 | 0.55 | 0.75 | 1.26 |
| frame bytes (KiB) | 324 | 3.68 | 8.33 | 10.12 | 30.62 |
| input event to frame written | 280 | 0.43 | 0.61 | 0.75 | 1.26 |
| source: file asked to text on the UI thread | 14 | 0.43 | 0.68 | 0.73 | 0.73 |
| ui event handling | 384 | 0.00 | 0.01 | 0.16 | 0.47 |
| worker: detail | 14 | 0.26 | 0.46 | 1.19 | 1.19 |
| worker: full source | 14 | 0.04 | 0.33 | 0.42 | 0.42 |
| worker: lexical mentions | 14 | 0.18 | 0.21 | 0.28 | 0.28 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.04 | 5.60 | 5.60 | 5.60 |

## haagerup: build before highlighting (3433d31) — run 1
first frame 3 ms; search ready after 3 ms; peak RSS 18 MiB; mode source
CPU 0.17 s; read 6.7 MiB (from storage 0.0 MiB); written 1.23 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 321 | 0.40 | 0.53 | - | 1.07 |
| frame bytes (KiB) | 321 | 3.71 | 8.45 | - | 30.60 |
| ui event handling | 384 | 0.00 | 0.01 | - | 0.27 |
| worker: detail | 14 | 0.27 | 0.39 | - | 0.99 |
| worker: full source | 14 | 0.05 | 0.19 | - | 0.41 |
| worker: lexical mentions | 14 | 0.15 | 0.23 | - | 0.23 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | - | 0.00 |
| worker: other | 3 | 0.04 | 3.99 | - | 3.99 |

## haagerup: highlighting off — run 1
first frame 2 ms; search ready after 3 ms; peak RSS 21 MiB; mode source
CPU 0.17 s; read 6.8 MiB (from storage 0.0 MiB); written 1.28 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 322 | 0.43 | 0.56 | 0.84 | 1.35 |
| frame bytes (KiB) | 322 | 3.70 | 8.33 | 10.12 | 30.62 |
| input event to frame written | 280 | 0.45 | 0.61 | 0.85 | 1.35 |
| source: file asked to text on the UI thread | 14 | 0.35 | 0.56 | 0.64 | 0.64 |
| ui event handling | 384 | 0.00 | 0.01 | 0.16 | 0.44 |
| worker: detail | 14 | 0.25 | 0.45 | 1.21 | 1.21 |
| worker: full source | 14 | 0.05 | 0.24 | 0.53 | 0.53 |
| worker: lexical mentions | 14 | 0.16 | 0.23 | 0.24 | 0.24 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.04 | 4.19 | 4.19 | 4.19 |

## haagerup: syntax + semantic, spans and semantic tokens cached on disk (new process each run) — run 1
first frame 2 ms; search ready after 8 ms; peak RSS 24 MiB; mode source
CPU 0.20 s; read 8.6 MiB (from storage 0.0 MiB); written 2.33 MiB (to storage 0.03 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 339 | 0.49 | 0.67 | 0.99 | 2.41 |
| frame bytes (KiB) | 339 | 7.23 | 11.78 | 21.19 | 43.91 |
| highlight: file asked to spans on the UI thread | 14 | 0.31 | 2.42 | 2.73 | 2.73 |
| highlight: open to first spans | 14 | 0.03 | 2.06 | 2.51 | 2.51 |
| input event to frame written | 280 | 0.51 | 0.72 | 0.95 | 1.59 |
| source: file asked to text on the UI thread | 14 | 0.35 | 0.58 | 0.64 | 0.64 |
| ui event handling | 404 | 0.00 | 0.01 | 0.15 | 0.41 |
| worker: detail | 14 | 0.23 | 0.51 | 1.14 | 1.14 |
| worker: full source | 14 | 0.05 | 0.22 | 0.48 | 0.48 |
| worker: lexical mentions | 14 | 0.16 | 0.18 | 0.20 | 0.20 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.03 | 4.50 | 4.50 | 4.50 |

## haagerup: build before highlighting (3433d31) — run 1
first frame 2 ms; search ready after 6 ms; peak RSS 19 MiB; mode source
CPU 0.17 s; read 6.7 MiB (from storage 0.0 MiB); written 1.23 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 322 | 0.41 | 0.55 | - | 1.30 |
| frame bytes (KiB) | 322 | 3.68 | 8.33 | - | 30.60 |
| ui event handling | 384 | 0.00 | 0.01 | - | 0.18 |
| worker: detail | 14 | 0.24 | 0.50 | - | 1.07 |
| worker: full source | 14 | 0.05 | 0.29 | - | 0.54 |
| worker: lexical mentions | 14 | 0.17 | 0.21 | - | 0.27 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | - | 0.00 |
| worker: other | 3 | 0.03 | 4.34 | - | 4.34 |

## haagerup: syntax only, nothing cached on disk (every file tokenized in this session) — run 1
first frame 2 ms; search ready after 3 ms; peak RSS 26 MiB; mode source
CPU 0.27 s; read 7.9 MiB (from storage 0.0 MiB); written 2.08 MiB (to storage 0.56 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 340 | 0.38 | 0.57 | 0.74 | 1.25 |
| frame bytes (KiB) | 340 | 4.69 | 8.30 | 13.24 | 34.46 |
| highlight: file asked to spans on the UI thread | 14 | 0.31 | 2.50 | 2.77 | 2.77 |
| highlight: open to first spans | 14 | 0.02 | 2.34 | 2.60 | 2.60 |
| input event to frame written | 280 | 0.43 | 0.61 | 0.78 | 3.12 |
| source: file asked to text on the UI thread | 14 | 0.42 | 0.69 | 0.73 | 0.73 |
| ui event handling | 402 | 0.00 | 0.01 | 0.27 | 2.83 |
| worker: detail | 14 | 0.25 | 0.46 | 1.11 | 1.11 |
| worker: full source | 14 | 0.05 | 0.36 | 0.41 | 0.41 |
| worker: lexical mentions | 14 | 0.18 | 0.22 | 0.24 | 0.24 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.03 | 4.88 | 4.88 | 4.88 |

## haagerup: idle minute, syntax + semantic — run 1
first frame 3 ms; search ready after 5 ms; peak RSS 24 MiB; mode source
CPU 0.19 s; read 8.6 MiB (from storage 0.0 MiB); written 2.33 MiB (to storage 0.03 MiB)
idle for 60 s: 0 ms of CPU time
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 337 | 0.45 | 0.65 | 0.96 | 1.42 |
| frame bytes (KiB) | 337 | 7.28 | 11.78 | 21.19 | 43.91 |
| highlight: file asked to spans on the UI thread | 14 | 0.31 | 1.92 | 2.53 | 2.53 |
| highlight: open to first spans | 14 | 0.02 | 2.03 | 2.60 | 2.60 |
| input event to frame written | 280 | 0.50 | 0.68 | 0.96 | 1.42 |
| source: file asked to text on the UI thread | 14 | 0.27 | 0.47 | 0.59 | 0.59 |
| ui event handling | 404 | 0.00 | 0.01 | 0.17 | 1.81 |
| worker: detail | 14 | 0.21 | 0.52 | 1.14 | 1.14 |
| worker: full source | 14 | 0.04 | 0.20 | 0.57 | 0.57 |
| worker: lexical mentions | 14 | 0.15 | 0.23 | 2.88 | 2.88 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.28 | 5.36 | 5.36 | 5.36 |

## haagerup: idle minute, build before highlighting — run 1
first frame 5 ms; search ready after 6 ms; peak RSS 19 MiB; mode source
CPU 0.17 s; read 7.9 MiB (from storage 0.0 MiB); written 1.28 MiB (to storage 0.00 MiB)
idle for 60 s: 0 ms of CPU time
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 322 | 0.39 | 0.54 | - | 1.25 |
| frame bytes (KiB) | 322 | 3.68 | 8.33 | - | 30.60 |
| ui event handling | 384 | 0.00 | 0.01 | - | 2.05 |
| worker: detail | 14 | 0.24 | 0.41 | - | 1.22 |
| worker: full source | 14 | 0.05 | 0.24 | - | 0.41 |
| worker: lexical mentions | 14 | 0.16 | 0.22 | - | 0.24 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | - | 0.00 |
| worker: other | 3 | 0.04 | 3.16 | - | 3.16 |

