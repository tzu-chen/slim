# Milestone 2 handoff

Branch `milestone-2`, on top of `milestone-1` (commits are local; nothing was pushed).
Checked 2026-11-10. Usage: `docs/map.md`. Decisions and modules: `docs/milestone-2-notes.md`.

## What works

* **Architecture map** (`M`): a validated sidecar `lean-view.toml` (schema 1) with chosen main
  results, nested and ordered groups, landmarks with several placements and one annotation,
  purposes and explanations, paper references with provenance, limitations, an external
  foundations section, an unresolved-references section with rename hints that are never
  applied, and the file/namespace tree labelled as generated organization. Without a sidecar the
  generated outline and everything else still work. slim never writes the sidecar.
* **Detail pane**: purpose or "not annotated", the formal statement (wrapping, or sideways
  scrolling with `w`), placements, curated prerequisites and dependents, the contribution to the
  pinned goal, authored and candidate paper references, evidence and freshness with their origin.
* **Goal explorer** (`A`, `*` pins): lazy unfolding of curated, lexical and Lean-derived
  relations, each edge labelled; uses / used-by direction; local/external, generated and
  statement/proof filters with hidden counts and a reveal key; path-local cycle detection,
  repeat markers, paging; bounded, cancellable connection queries with four distinct outcomes.
* **Reading trails** (`T`): several named trails, stops on declarations or map groups with an
  explanation, a question and a state, reorder/remove/resume, standalone questions, shared
  trails from the map copied on request, Markdown export with project identity, git commit and
  stale/unresolved stops. Milestone-1 trails are migrated.
* **Integration**: search results include map groups and landmarks; `M` on a declaration reveals
  and cycles its placements; views are history steps; the goal stays in the header; `?` lists
  every key; one pane at a time below 80 columns; `slim map init|check`
  and `slim trail list|export`.
* **Semantic increment**: no new exporter was needed. The existing snapshot format and
  version-pinned exporter are the import contract; the goal explorer reads Lean edges from the
  real Lean 4.33.1 demo snapshot, split into statement and proof/body references. Tests compare
  them with the fixture's expected edges.

## How to try it

```sh
cargo build --release
./target/release/slim demo            # opens on the map; ? for keys
```

Follow "Ten minutes with the map" in the README. The sample map is
`fixtures/demo/lean-view.toml`; the shared trail in it is the sample trail. Real terminal
captures of each step are in `docs/screenshots/m2-*.txt` (`scripts/capture-screens.sh
--views-only` regenerates them).

For a project of your own: `slim map init DIR -o DIR/lean-view.toml`, edit, `slim map
check DIR`, then `slim DIR` and `r` in the map after each edit.

## Tests executed

`cargo test --release`, last run 2026-11-10: **135 passed, 0 failed** (milestone 1 had 96).

* 109 unit tests. New: map parsing and validation (nesting, order, multiple placements, cycles
  in group parents, duplicate ids, unknown fields with the field named, unsupported and missing
  schema versions, a malformed file left byte-for-byte unchanged, a symlinked sidecar refused,
  Unicode and quoted names), paper references and literal candidates, curated relation direction
  and cycles, the generated skeleton; trails (order, notes, questions, states across reopen,
  migration from version 1, a newer store refused and untouched, two store handles interleaving,
  atomic export writes); bounded search (shortest path, cycles, a 100,000-successor hub,
  time limit, cancellation); the three views driven by key events (lazy unfolding, back
  references, repeats, paging, hidden counts, stale answers dropped, relation kinds never
  silently skipped, connection outcomes, history across views, selection kept across reloads,
  stale and unresolved stops); rendering of the views at 9 sizes from 140x40 down to 20x5 in 3
  themes, ASCII and Unicode, both index modes, with Unicode and hostile text.
* 7 map-fixture tests on real data: the sample map against the Lean 4.33.1 snapshot and against
  a source-only scan; Lean edges equal to the fixture's expected edges with statement and
  proof labels; curated, lexical and Lean labels in the mixed view; connection queries (path
  through a generated auxiliary, exhaustive no-path, direction, truncation, cancellation,
  unavailable); trails across an uncommitted edit and a rename with re-indexing, and their
  export; a labelled synthetic 100,000-declaration index with a 60,000-edge hub.
* 6 pseudo-terminal tests (2 new) that read the emulated screen: a keyboard-only session on the
  demo (objective, statement, goal, curated then Lean relations, add a stop, browser and back,
  trails, help, quit, restart with goal/trail/view restored, command-line export); a 60-column
  source-only project with a malformed map (browser usable, field error shown, file untouched,
  hand fix and reload, single-pane toggle, candidate reference, Lean kind unavailable).
* 8 fixture and 5 lifecycle tests from milestone 1, unchanged and passing.

Not automated: the measurement sessions (`scripts/measure-views-run.sh`), the screenshots, and
anything involving a human reader.

## What was measured

`docs/benchmarks.md` (section "Milestone 2") and the raw report `docs/bench/m2-views.md`. In
short, on 2 CPUs with a warm cache: first frame 2–4 ms on the demo, CFSG and HaagerupIzumi
indexes; UI work per selection or expand event at most 0.05 ms at p95 and 0.6 ms worst case;
map load and resolution 1–7 ms; no regression against the milestone-1 binary on the same search
session. Cold starts were not measured.

## Acceptance checklist

| Item | Status |
|---|---|
| V1 usable without sidecar, snapshot, Lean or AI service | met; tested (source-only project with a broken map, and with none) |
| Map: nested groups, multiple placements, landmarks, purpose, unresolved references | met; tested on the fixture and in the UI |
| Map, statement and source views keep context at wide and narrow sizes | met for the tested sizes; source view is the milestone-1 browser, one key away |
| Goal view never presents lexical or curated edges as Lean-derived | met; every edge carries its label, tested |
| Repeated nodes, cycles and large fan-out stay finite and navigable | met; unit tests and a synthetic hub. Not exercised on a real large semantic snapshot (none exists here) |
| Trails and notes survive restart and re-indexing, stale references visible | met; tested |
| Paper references keep source evidence, no invented bibliography | met; tested |
| A real semantic fixture works | met with the existing Lean 4.33.1 fixture and exporter; **no new exporter and no larger real snapshot** |
| No automatic execution, upload, hidden network or runaway worker | met; opening reads files only, the connection thread is bounded and joined, existing lifecycle tests pass |
| Tests and measurements recorded honestly | this document, `docs/benchmarks.md`, `docs/limitations.md` |
| A new reader can follow the fixture's main argument and save a route without the docs | **only partly evidenced**: a scripted keyboard session does it and the screens were inspected, but no human new reader was timed |

## Known gaps

See `docs/limitations.md` ("Map, goal explorer and trails"). The ones that matter most:

1. The map is edited outside slim. There is no in-app annotation editor and no accept/reject
   dialog for agent suggestions (the plan allowed an external-edit-and-reload workflow; that is
   what exists). Fingerprints are pasted by hand.
2. No curated map exists for a real project. CFSG and HaagerupIzumi were opened with generated
   skeletons for measurement only; nobody wrote a map of their mathematics, and none was made up.
3. Lean dependencies in the goal explorer were exercised on the 166-declaration demo snapshot
   and on synthetic data, not on a Mathlib-scale snapshot (milestone 1's open gap).
4. Changing a filter or relation kind rebuilds the goal outline from the root.
5. The ten-minute criterion was not tested with a person.

## Next steps

1. Have someone who knows one real project write its map, and time a new reader on it.
2. Add an in-app "annotate" action that writes a *patch file* next to the map (reviewed and
   merged by a person) rather than editing the map in place, plus `map stamp` to fill
   `source_sha256`.
3. Produce one Mathlib-scale semantic snapshot (milestone-1 next step 1) and re-measure the goal
   explorer and connection queries on it.
4. Keep the unfolded goal outline across filter changes by re-requesting expanded nodes by name.
