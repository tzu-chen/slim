## cfsg: build before highlighting (3433d31) — run 1
first frame 1 ms; search ready after 230 ms; peak RSS 442 MiB; mode source
CPU 3.11 s; read 277.1 MiB (from storage 0.0 MiB); written 0.79 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 323 | 0.38 | 0.58 | - | 3.08 |
| frame bytes (KiB) | 323 | 1.68 | 4.30 | - | 30.90 |
| ui event handling | 400 | 0.00 | 0.01 | - | 5.65 |
| worker: detail | 14 | 0.28 | 0.43 | - | 12.63 |
| worker: full source | 14 | 0.11 | 0.80 | - | 0.85 |
| worker: lexical mentions | 14 | 0.23 | 0.31 | - | 0.32 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | - | 0.00 |
| worker: other | 3 | 5.72 | 98.59 | - | 98.59 |

## cfsg: highlighting off — run 1
first frame 2 ms; search ready after 167 ms; peak RSS 436 MiB; mode source
CPU 2.87 s; read 274.4 MiB (from storage 0.0 MiB); written 0.73 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 327 | 0.42 | 0.55 | 0.73 | 3.46 |
| frame bytes (KiB) | 327 | 1.69 | 4.27 | 9.37 | 30.92 |
| input event to frame written | 280 | 0.45 | 0.59 | 0.83 | 1.44 |
| source: file asked to text on the UI thread | 14 | 0.51 | 1.04 | 1.45 | 1.45 |
| ui event handling | 400 | 0.00 | 0.02 | 0.31 | 0.46 |
| worker: detail | 14 | 0.27 | 0.42 | 8.64 | 8.64 |
| worker: full source | 14 | 0.12 | 0.81 | 0.89 | 0.89 |
| worker: lexical mentions | 14 | 0.19 | 0.26 | 0.29 | 0.29 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.23 | 65.97 | 65.97 | 65.97 |

## cfsg: syntax only, nothing cached on disk (every file tokenized in this session) — run 1
first frame 2 ms; search ready after 169 ms; peak RSS 436 MiB; mode source
CPU 3.04 s; read 281.2 MiB (from storage 0.0 MiB); written 1.12 MiB (to storage 0.28 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 336 | 0.43 | 0.56 | 0.61 | 1.08 |
| frame bytes (KiB) | 336 | 1.77 | 8.25 | 9.49 | 31.20 |
| highlight: file asked to spans on the UI thread | 14 | 0.45 | 11.88 | 24.60 | 24.60 |
| highlight: open to first spans | 14 | 0.03 | 11.72 | 24.36 | 24.36 |
| input event to frame written | 280 | 0.46 | 0.61 | 1.09 | 1.62 |
| source: file asked to text on the UI thread | 14 | 0.49 | 1.48 | 1.70 | 1.70 |
| ui event handling | 415 | 0.00 | 0.03 | 0.37 | 0.92 |
| worker: detail | 14 | 0.29 | 0.46 | 8.60 | 8.60 |
| worker: full source | 14 | 0.13 | 1.03 | 1.12 | 1.12 |
| worker: lexical mentions | 14 | 0.23 | 0.26 | 0.34 | 0.34 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 1.10 | 83.14 | 83.14 | 83.14 |

## cfsg: syntax + semantic, spans and semantic tokens cached on disk (new process each run) — run 1
first frame 2 ms; search ready after 249 ms; peak RSS 443 MiB; mode source
CPU 3.08 s; read 283.3 MiB (from storage 0.0 MiB); written 0.95 MiB (to storage 0.03 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 333 | 0.39 | 0.60 | 0.74 | 1.35 |
| frame bytes (KiB) | 333 | 1.80 | 8.25 | 9.49 | 31.15 |
| highlight: file asked to spans on the UI thread | 14 | 0.33 | 3.19 | 8.58 | 8.58 |
| highlight: open to first spans | 14 | 0.02 | 2.30 | 8.56 | 8.56 |
| input event to frame written | 280 | 0.42 | 0.71 | 1.05 | 3.22 |
| source: file asked to text on the UI thread | 14 | 0.44 | 0.98 | 1.07 | 1.07 |
| ui event handling | 414 | 0.00 | 0.04 | 0.41 | 2.85 |
| worker: detail | 14 | 0.29 | 0.54 | 8.96 | 8.96 |
| worker: full source | 14 | 0.09 | 0.82 | 0.93 | 0.93 |
| worker: lexical mentions | 14 | 0.20 | 0.29 | 0.29 | 0.29 |
| worker: map load + resolve | 1 | 0.01 | 0.01 | 0.01 | 0.01 |
| worker: other | 3 | 0.09 | 87.81 | 87.81 | 87.81 |

## cfsg: syntax + semantic, spans and semantic tokens cached on disk (new process each run) — run 1
first frame 2 ms; search ready after 165 ms; peak RSS 440 MiB; mode source
CPU 2.85 s; read 281.6 MiB (from storage 0.0 MiB); written 0.89 MiB (to storage 0.03 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 337 | 0.38 | 0.59 | 0.77 | 1.26 |
| frame bytes (KiB) | 337 | 1.77 | 8.25 | 9.49 | 31.15 |
| highlight: file asked to spans on the UI thread | 14 | 0.30 | 2.15 | 8.86 | 8.86 |
| highlight: open to first spans | 14 | 0.02 | 2.05 | 8.61 | 8.61 |
| input event to frame written | 280 | 0.41 | 0.66 | 0.87 | 1.26 |
| source: file asked to text on the UI thread | 14 | 0.50 | 0.88 | 1.33 | 1.33 |
| ui event handling | 414 | 0.00 | 0.01 | 0.29 | 0.47 |
| worker: detail | 14 | 0.27 | 0.49 | 8.73 | 8.73 |
| worker: full source | 14 | 0.11 | 0.79 | 0.84 | 0.84 |
| worker: lexical mentions | 14 | 0.19 | 0.25 | 0.26 | 0.26 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.07 | 78.87 | 78.87 | 78.87 |

## cfsg: syntax only, nothing cached on disk (every file tokenized in this session) — run 1
first frame 43 ms; search ready after 167 ms; peak RSS 435 MiB; mode source
CPU 3.03 s; read 277.3 MiB (from storage 0.0 MiB); written 1.12 MiB (to storage 0.28 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 339 | 0.40 | 0.56 | 0.63 | 2.05 |
| frame bytes (KiB) | 339 | 1.73 | 8.25 | 9.49 | 31.20 |
| highlight: file asked to spans on the UI thread | 14 | 0.32 | 13.43 | 24.60 | 24.60 |
| highlight: open to first spans | 14 | 0.03 | 13.13 | 24.59 | 24.59 |
| input event to frame written | 280 | 0.42 | 0.60 | 0.87 | 3.07 |
| source: file asked to text on the UI thread | 14 | 0.48 | 1.00 | 1.46 | 1.46 |
| ui event handling | 415 | 0.00 | 0.02 | 0.40 | 2.82 |
| worker: detail | 14 | 0.29 | 0.45 | 7.56 | 7.56 |
| worker: full source | 14 | 0.09 | 0.76 | 0.78 | 0.78 |
| worker: lexical mentions | 14 | 0.20 | 0.29 | 0.38 | 0.38 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.05 | 47.68 | 47.68 | 47.68 |

## cfsg: highlighting off — run 1
first frame 1 ms; search ready after 173 ms; peak RSS 439 MiB; mode source
CPU 2.93 s; read 274.6 MiB (from storage 0.0 MiB); written 0.78 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 323 | 0.40 | 0.57 | 0.67 | 3.11 |
| frame bytes (KiB) | 323 | 1.70 | 4.31 | 9.37 | 30.92 |
| input event to frame written | 280 | 0.42 | 0.63 | 0.86 | 1.05 |
| source: file asked to text on the UI thread | 14 | 0.43 | 1.01 | 1.20 | 1.20 |
| ui event handling | 400 | 0.00 | 0.01 | 0.27 | 0.58 |
| worker: detail | 14 | 0.27 | 0.64 | 9.14 | 9.14 |
| worker: full source | 14 | 0.08 | 0.76 | 0.77 | 0.77 |
| worker: lexical mentions | 14 | 0.20 | 0.35 | 0.40 | 0.40 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.08 | 84.09 | 84.09 | 84.09 |

## cfsg: build before highlighting (3433d31) — run 1
first frame 2 ms; search ready after 173 ms; peak RSS 437 MiB; mode source
CPU 2.99 s; read 270.5 MiB (from storage 0.0 MiB); written 0.73 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 326 | 0.36 | 0.55 | - | 1.05 |
| frame bytes (KiB) | 326 | 1.68 | 4.27 | - | 30.90 |
| ui event handling | 400 | 0.00 | 0.01 | - | 0.87 |
| worker: detail | 14 | 0.26 | 0.43 | - | 8.98 |
| worker: full source | 14 | 0.11 | 0.80 | - | 0.84 |
| worker: lexical mentions | 14 | 0.19 | 0.26 | - | 0.32 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | - | 0.00 |
| worker: other | 3 | 0.07 | 78.28 | - | 78.28 |

## cfsg: highlighting off — run 1
first frame 2 ms; search ready after 175 ms; peak RSS 437 MiB; mode source
CPU 2.88 s; read 274.4 MiB (from storage 0.0 MiB); written 0.73 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 329 | 0.38 | 0.58 | 0.77 | 1.26 |
| frame bytes (KiB) | 329 | 1.65 | 4.23 | 9.37 | 30.92 |
| input event to frame written | 280 | 0.42 | 0.61 | 0.77 | 1.19 |
| source: file asked to text on the UI thread | 14 | 0.59 | 0.82 | 3.34 | 3.34 |
| ui event handling | 400 | 0.00 | 0.01 | 0.26 | 0.43 |
| worker: detail | 14 | 0.26 | 0.53 | 9.41 | 9.41 |
| worker: full source | 14 | 0.10 | 0.76 | 0.80 | 0.80 |
| worker: lexical mentions | 14 | 0.21 | 0.66 | 2.97 | 2.97 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.07 | 88.41 | 88.41 | 88.41 |

## cfsg: syntax + semantic, spans and semantic tokens cached on disk (new process each run) — run 1
first frame 5 ms; search ready after 177 ms; peak RSS 439 MiB; mode source
CPU 2.93 s; read 277.7 MiB (from storage 0.0 MiB); written 0.89 MiB (to storage 0.03 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 344 | 0.36 | 0.59 | 0.79 | 4.80 |
| frame bytes (KiB) | 344 | 1.69 | 8.25 | 9.49 | 31.15 |
| highlight: file asked to spans on the UI thread | 14 | 0.33 | 2.20 | 8.86 | 8.86 |
| highlight: open to first spans | 14 | 0.03 | 2.06 | 8.61 | 8.61 |
| input event to frame written | 281 | 0.41 | 0.61 | 0.83 | 1.11 |
| source: file asked to text on the UI thread | 14 | 0.49 | 0.86 | 1.44 | 1.44 |
| ui event handling | 414 | 0.00 | 0.01 | 0.28 | 0.40 |
| worker: detail | 14 | 0.28 | 0.47 | 9.17 | 9.17 |
| worker: full source | 14 | 0.09 | 0.81 | 0.82 | 0.82 |
| worker: lexical mentions | 14 | 0.20 | 0.25 | 0.29 | 0.29 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.40 | 76.66 | 76.66 | 76.66 |

## cfsg: build before highlighting (3433d31) — run 1
first frame 1 ms; search ready after 169 ms; peak RSS 440 MiB; mode source
CPU 2.92 s; read 274.3 MiB (from storage 0.0 MiB); written 0.73 MiB (to storage 0.00 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 322 | 0.41 | 0.57 | - | 1.44 |
| frame bytes (KiB) | 322 | 1.70 | 4.24 | - | 30.90 |
| ui event handling | 400 | 0.00 | 0.01 | - | 2.93 |
| worker: detail | 14 | 0.26 | 0.41 | - | 9.39 |
| worker: full source | 14 | 0.10 | 0.89 | - | 1.23 |
| worker: lexical mentions | 14 | 0.23 | 0.30 | - | 0.30 |
| worker: map load + resolve | 1 | 0.01 | 0.01 | - | 0.01 |
| worker: other | 3 | 2.35 | 88.09 | - | 88.09 |

## cfsg: syntax only, nothing cached on disk (every file tokenized in this session) — run 1
first frame 3 ms; search ready after 190 ms; peak RSS 415 MiB; mode source
CPU 3.14 s; read 277.5 MiB (from storage 0.0 MiB); written 1.12 MiB (to storage 0.28 MiB)
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 339 | 0.39 | 0.57 | 0.65 | 1.17 |
| frame bytes (KiB) | 339 | 1.73 | 8.25 | 9.49 | 31.20 |
| highlight: file asked to spans on the UI thread | 14 | 0.29 | 11.90 | 24.73 | 24.73 |
| highlight: open to first spans | 14 | 0.02 | 11.83 | 24.47 | 24.47 |
| input event to frame written | 280 | 0.42 | 0.62 | 0.78 | 1.26 |
| source: file asked to text on the UI thread | 14 | 0.47 | 0.86 | 1.32 | 1.32 |
| ui event handling | 415 | 0.00 | 0.01 | 0.28 | 0.39 |
| worker: detail | 14 | 0.28 | 0.40 | 8.70 | 8.70 |
| worker: full source | 14 | 0.11 | 0.83 | 0.83 | 0.83 |
| worker: lexical mentions | 14 | 0.18 | 0.30 | 1.34 | 1.34 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | 0.00 | 0.00 |
| worker: other | 3 | 0.10 | 98.38 | 98.38 | 98.38 |

## cfsg: idle minute, syntax + semantic — run 1
first frame 7 ms; search ready after 173 ms; peak RSS 441 MiB; mode source
CPU 2.86 s; read 277.7 MiB (from storage 0.0 MiB); written 0.89 MiB (to storage 0.03 MiB)
idle for 60 s: 0 ms of CPU time
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 334 | 0.39 | 0.59 | 0.91 | 3.76 |
| frame bytes (KiB) | 334 | 1.78 | 8.25 | 9.49 | 31.15 |
| highlight: file asked to spans on the UI thread | 14 | 0.29 | 2.16 | 8.95 | 8.95 |
| highlight: open to first spans | 14 | 0.02 | 2.05 | 8.71 | 8.71 |
| input event to frame written | 280 | 0.42 | 0.65 | 0.91 | 1.19 |
| source: file asked to text on the UI thread | 14 | 0.43 | 0.81 | 1.50 | 1.50 |
| ui event handling | 414 | 0.00 | 0.03 | 0.33 | 0.46 |
| worker: detail | 14 | 0.26 | 0.60 | 8.97 | 8.97 |
| worker: full source | 14 | 0.11 | 0.80 | 0.82 | 0.82 |
| worker: lexical mentions | 14 | 0.18 | 0.24 | 0.32 | 0.32 |
| worker: map load + resolve | 1 | 0.01 | 0.01 | 0.01 | 0.01 |
| worker: other | 3 | 0.07 | 71.48 | 71.48 | 71.48 |

## cfsg: idle minute, build before highlighting — run 1
first frame 1 ms; search ready after 168 ms; peak RSS 438 MiB; mode source
CPU 2.87 s; read 270.4 MiB (from storage 0.0 MiB); written 0.73 MiB (to storage 0.00 MiB)
idle for 60 s: 0 ms of CPU time
| measurement | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|
| frame (render + write) | 326 | 0.40 | 0.54 | - | 1.02 |
| frame bytes (KiB) | 326 | 1.68 | 4.24 | - | 30.90 |
| ui event handling | 400 | 0.00 | 0.01 | - | 0.40 |
| worker: detail | 14 | 0.30 | 0.41 | - | 8.57 |
| worker: full source | 14 | 0.11 | 0.78 | - | 0.82 |
| worker: lexical mentions | 14 | 0.18 | 0.26 | - | 0.30 |
| worker: map load + resolve | 1 | 0.00 | 0.00 | - | 0.00 |
| worker: other | 3 | 0.07 | 81.12 | - | 81.12 |

