# Milestone 2: implementation note

Written at the start of milestone 2 (2026-11-10) and updated when it finished. It records what
was found in the repository, what was reused, and what was added. Usage is in `docs/map.md`;
status and measurements are in `docs/handoff-milestone-2.md` and `docs/benchmarks.md`.

## Baseline (milestone 1, commit 370f01c)

* `cargo test --release`: 96 tests passed (79 unit, 8 fixture, 5 lifecycle, 4 pseudo-terminal)
  before any change.
* UI model: one UI thread that only renders and handles input, blocking on one channel; a query
  worker (read-only SQLite connection + user store, coalescing superseded requests), a search
  worker (in-memory name index) and a job runner. Responses carry generation numbers.
* Index: SQLite schema 1 (`decls`, `modules`, `edges(src, kind, dst)` with a reverse index,
  axioms), one file per project and mode under the cache directory. Semantic snapshots
  (`slim-snapshot`, format 1) are produced by the existing version-pinned Lean exporter;
  the bundled demo snapshot is real Lean 4.33.1 data.
* Reading data: `userdata.sqlite` with bookmarks, one unnamed trail per project, sessions.
* Start-up and search on the local CFSG source-only index were re-measured with the milestone-1
  binary as the regression baseline (`docs/bench/m2-views.md`).

## Decisions

| Question | Decision | Why |
|---|---|---|
| Where do shareable annotations live? | `lean-view.toml` in the project root, schema 1, read-only for slim | hand-editable and reviewable; a tool that never writes the file cannot clobber a malformed one or lose hand edits |
| Where do personal trails live? | the existing `userdata.sqlite`, schema version 2 | the plan asks to adapt the existing persistence layer; SQLite gives atomic writes and safe concurrent instances |
| A semantic-snapshot contract? | the existing `slim-snapshot` format 1 and exporter are kept as the contract; nothing new was written | they already provide resolved identities, kinds, pretty-printed types, source ranges, separately classified type/value/constructor references, coverage and provenance, and are tested on five toolchains |
| One graph view or several? | three outline views over shared identities | a project is neither a directory tree nor one dependency tree |
| How are relation kinds kept apart? | one `EdgeLabel` per edge, carried from the query to every row and to the detail pane | no code path can show an edge without its evidence |
| TOML parser | the `toml` crate 1.2.1 (already in the local cargo cache; built offline) | strict typed parsing with field errors; no hand-written parser |
| What opens first? | the map, when a valid map exists and no earlier session is resumed | a new reader should meet the stated objectives first; `/` is the search |

## Modules added or changed

| Module | Role |
|---|---|
| `src/map.rs` | sidecar schema, validation with field-level issues, the validated model, resolution of names against an index (resolved / ambiguous / missing with rename hints), staleness, candidate paper references, `map init` skeleton |
| `src/relations.rs` | typed direct relations (curated, lexical, Lean) with paging and hidden counts; generic bounded, cancellable breadth-first search; connection queries |
| `src/userdata.rs` | user-data schema 2: named trails, stops (note, question, state, fingerprint), questions, per-project preferences; migration from schema 1; stop status against an index; Markdown export; atomic file writes |
| `src/app/views.rs` | state and input of the map, goal and trail views; history integration; selection by key |
| `src/app/render_views.rs` | outline rows (only the visible ones are rendered) and the detail pane |
| `src/app/worker.rs` | new requests: map load and resolution, view detail, goal children, reading operations; connection search on its own short-lived thread |
| `src/app/state.rs`, `render.rs`, `mod.rs` | view switching, global keys, search results from the map, help with unavailable actions, session |
| `src/main.rs`, `src/cli.rs` | `slim map init|check`, `trail list`, `--map`, `--trail` |
| `src/paths.rs` | reading the git commit from `.git` as text |

## Data kept apart

1. Regenerable: indexes and snapshots (cache directory).
2. Shareable: `lean-view.toml` (project root; written only by people, or by `map init -o` into a
   file that does not exist yet).
3. Personal: trails, questions, states, goal (user data directory; exported only on request).

## Identity and staleness

* A declaration reference is the Lake package (project key) plus the fully qualified Lean name.
  Path, line and SHA-256 are locators recorded alongside.
* Private declarations keep Lean's mangled name (`_private.<Module>.0.…`), which embeds the
  module path; exports say so. Anonymous source-only candidates have a descriptive label, not
  Lean's generated name, and are flagged as such by the browser.
* A stop or annotation with a recorded fingerprint is stale when the indexed file differs. A git
  commit is shown in exports but is never the freshness test, because a worktree can be dirty.
* A semantic snapshot older than the sources is flagged by the existing freshness check
  (global badge and per-declaration "SOURCE CHANGED"); the views repeat that warning and never
  describe snapshot data as verified against the current source.
