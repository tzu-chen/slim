# Highlighting handoff

Branch `milestone-2`, one commit on top of `3433d31` (local; nothing was
pushed). Checked 2026-11-10. Plan: `lean-view-highlight-plan.md`. Use and design:
`docs/highlighting.md`. First inspection and budgets: `docs/highlight-phase0.md`.
Measurements: `docs/bench/highlight.md`. Licences: `THIRD-PARTY-NOTICES.md`.

## What works

* **Syntax colouring identical to VS Code's.** The Lean extension's TextMate grammars
  (vscode-lean4 0.0.240), run by a port of vscode-textmate on Oniguruma, resolved against
  VS Code's Dark+, Light+ and high-contrast themes or a theme file. Checked against VS Code's
  tokenizer on 4.22 million lines (all of HaagerupIzumi, all of its Mathlib, 5,768 CFSG
  files): the same scope stack and the same colour for every byte. Works in every mode,
  for any file, with no process started.
* **Semantic colouring from Lean itself.** An explicit, confirmed, sandboxed job runs the
  project's pinned language server, takes its semantic tokens per file after a verified
  completion condition, and caches them. Afterwards browsing reads the cache. On
  HaagerupIzumi: 1,419 of 1,435 files complete, 16 failed with their reasons; an independent
  second client returned identical tokens for all 26 files sampled.
* **In the browser**: coloured source view and declaration excerpt; `F4` mode, `F5` syntax
  theme, `F6` details; palette commands to build the index (plan, typed `yes`), refresh the
  file on screen, pause, resume, cancel, and toggle the stale preview; progress in the title
  bar; the title of the source pane always says what the colours are.
* **From the shell**: `slim highlight build | status | clean | dump | bench`, and
  `slim export --with-highlight`.
* **Freshness kept apart**: source alignment, coverage, environment, imported sources and
  build relation are separate facts, shown separately; unaligned tokens are never applied.
* **Process hygiene**: only slim's own process tree is ever signalled; cancel, quit,
  Ctrl-C, a killed browser, a crashed server and a memory-hungry file are each tested.

## Supported versions

Grammar: vscode-lean4 0.0.240. Tokenizer behaviour: vscode-textmate 9.3.2. Themes and
semantic style rules: VS Code 1.142.0. Lean servers tested end to end on the fixture:
4.16.0, 4.21.0, 4.28.0, 4.33.1, 4.35.0 (`scripts/test-highlight-toolchains.sh`); others from
4.16 on only with `--allow-untested-toolchain`. Linux only.

## Tests executed

`cargo test --release`: 205 tests pass (169 unit tests, 8 in `tests/highlight.rs`, 8 in `tests/pty.rs`, 5 lifecycle, 7 map fixture, 8 semantic fixture; 110 unit and 6 pseudo-terminal tests existed before). The highlighting tests need `python3` for the stand-in
server; the real-server test ran here (Lean 4.33.1 and a built `fixtures/demo` are present).
`scripts/test-highlight-toolchains.sh`: all five toolchains ok.

## Measurements (details and raw data in `docs/bench/highlight.md`)

| | |
|---|---|
| Key to written frame, syntax + semantic, HaagerupIzumi | p50 0.50 ms, p95 0.68 ms, p99 0.95 ms (budget 16 / 33 ms) |
| The same on CFSG (syntax only exists there) | p50 0.41 ms, p95 0.66 ms, p99 0.87 ms |
| The same while the browser's own indexing job runs on the same two CPUs | p50 0.38 ms, p95 1.00 ms, p99 2.51 ms, slowest frame 7.9 ms |
| File asked → coloured | p95 1.8 to 2.5 ms on HaagerupIzumi; on CFSG 2.2 ms from the disk cache, 11.9 ms tokenizing |
| Frame time before → after | p95 0.54 → 0.67 ms; resident memory 18 → 24 MiB; idle CPU 0 ms per minute both |
| Tokenizer | 8.5 MiB/s on HaagerupIzumi, 13.0 MiB/s on the CFSG sample, one thread |
| Semantic index of HaagerupIzumi | first pass 1 h 43 min on two CPUs, 3.9 s per file at the median; cache 7.2 MiB; rerun with nothing changed 0.6 s; one file 3.3 s |
| Cancelling | 27 to 30 ms until no server process is left |

No lower-fidelity mode was added, because nothing came near the "severe" criteria.

## What is partial or open

1. **CFSG has no semantic tokens.** Its toolchain (`v4.35.0-rc3`) is not installed and it is
   not built; installing, fetching and building were not authorised and slim never does
   them. This acceptance gate of the plan is open. Everything text-only was measured on CFSG.
2. **16 HaagerupIzumi files have no tokens**: 4 with stale imports (rebuild the project,
   then `slim highlight build ~/Codes/haagerup/lean --retry`), 5 tool or draft files that
   are not part of the built library, 4 draft files stopped for memory at 4.3 GiB of heap,
   and 3 whose Lean worker crashes in the server for a reason not established.
3. **The first indexing pass was stopped by the machine's resource guard** when one draft file
   took the server to its 6,144 MiB limit. The finished files were kept. Before running
   again the job was changed to stop such a file itself at 70 % of the limit; the following
   passes stayed below the limit. Worth knowing when choosing `--memory-mib`.
4. **Not compared with a running VS Code window.** The syntax layer is compared with VS
   Code's tokenizer and themes by machine; the semantic style rules were ported from VS
   Code's code. A look at "Developer: Inspect Editor Tokens and Scopes" on a few files would
   close discrepancy 11 of `docs/highlighting.md`.
5. **Resident memory in a stress test** (18,147 file opens) grows by 193 MiB with the default
   128 MiB cache budget; the accounted cache itself stays at 128.0 MiB.
6. **Not built**: in-file search, selection and horizontal scrolling in the source view;
   automatic re-indexing on change; automatic pausing while the reader types; colours in
   statements and documentation.
7. **Not measured**: allocation counts, a cold page cache, remote terminals, more than two
   CPUs for indexing, old toolchains on a large project.

## Where things are

| | |
|---|---|
| `src/highlight/` | `textmate` (grammar interpreter), `baseline`, `theme`, `semantic`, `lsp`, `indexer`, `codec`, `store`, `env`, `service`, `render`, `cli`, `bench` |
| `src/app/hl.rs` | settings, snapshot, the job in the UI, style conversion calls; `render.rs` uses `App::hl_rows` |
| `assets/` | the bundled grammars and themes |
| `fixtures/highlight/` | original fixture files and VS Code's reference output for them |
| `tests/highlight.rs`, `tests/support/fake_lean_server.py`, additions in `tests/pty.rs` | end-to-end tests |
| `scripts/` | `textmate-reference.mjs`, `compare-textmate.py`, `regen-highlight-reference.sh`, `check-semantic-tokens.py`, `test-highlight-toolchains.sh`, `measure-highlight-run.sh`, `measure-highlight-indexing.sh`, `summarize-highlight-bench.py` |

Changes to existing code are small: `text.rs` (wrapping split so styled text wraps the same),
`process.rs` (piped children, process-tree and cgroup readings, limits that respect an outer
guard), `app/worker.rs` (text hashes in two bundles), `app/render.rs` (the two places that
draw source lines, the title, two overlays), `app/state.rs` and `app/mod.rs` (commands, keys,
events, the worker's lifetime, one more timing), `app/theme.rs` (syntax colour conversion),
`export.rs` (`--with-highlight`), `main.rs` (`info` mentions the cache).

## State left on this machine

* `~/.cache/slim/projects/HaagerupIzumi-0775d1c6f2b6/highlight.sqlite`: the semantic index
  described above (7.2 MiB). `slim ~/Codes/haagerup/lean` uses it at once; `slim
  highlight clean ~/Codes/haagerup/lean` removes it.
* Nothing was written into `~/Codes/haagerup` or `~/Codes/CFSG`.
* Scratch under `target/` (reference tooling in `target/hl-ref*`, measurement copies of the
  caches in `target/measure`, the comparison build in `target/m2-*`): ignored by git, safe to
  delete.
