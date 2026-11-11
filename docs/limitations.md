# Known limitations (milestone 1)

## Source-only mode is lexical

* Lean syntax is extensible. Declarations created by macros, custom commands, `notation`,
  `deriving`, `initialize`, `alias ⟨a, b⟩ := …` or attribute handlers are not detected. Unknown
  column-0 commands after a blank line end the current extent and are counted per file (`partial`).
* Names are namespace-qualified as written (`_root_` handled); `open` does not affect them.
  Anonymous instances get a descriptive label (`instance : Foo Nat`), not Lean's generated name.
  Duplicate full names (often `private` helpers in different files) are flagged ambiguous.
* Extents are approximate: a declaration ends at the next recognised command head. Unindented
  tactic proofs that contain a blank line followed by a column-0 word can end early.
* Structure fields are recognised on lines of the form `name … : T`; binder-style field groups and
  field docstrings are not. Constructor docstrings are not attached.
* Module names are paths relative to the project root; a `srcDir` in the Lake configuration is
  not honoured (imports to such modules then show as external).
* Lexical observations are reported as such: a `sorry` token is evidence of a placeholder in the
  text; its absence proves nothing. There is no "used by" in this mode.
* On CFSG 29 of 229,089 files and on Stacks 11 of 62,975 files are marked partially parsed
  (mostly `end` names that do not match the scanner's scope stack).

## Semantic mode

* The exporter needs existing build artifacts; slim does not build projects. It was tested
  end to end only on the 65-declaration fixture, on five toolchains. **It has not been run on a
  Mathlib-based project here**: CFSG pins `v4.35.0-rc3` (not installed) and neither CFSG nor
  Stacks has build artifacts (Stacks would need a Mathlib cache download and a long build; CFSG
  states a build of about a day). Time, memory and snapshot size at Mathlib scale are therefore
  unmeasured; the default `--library full` exports the whole reachable library closure with
  edges, which for Mathlib may be several million edges. `--library frontier` bounds this at the
  price of coverage-gap markers.
* `--pp notation` (default) loads environment extensions after `enableInitializersExecution`, i.e.
  runs the imported modules' initializers and attribute handlers inside the sandbox. `--pp raw`
  avoids all project code but prints types without library notation. The legacy shim
  (Lean 4.16–4.20) always loads extensions.
* Theorem bodies of module-system files are loaded at `OLeanLevel.private`. Behaviour when only
  public `.olean` parts are present is untested.
* Instance/class detection reads the instance and class extensions' module entries; generated
  declarations are recognised by Lean's predicates (`isAuxRecursor`, `isNoConfusion`,
  recursors) and name patterns (`_`-prefixed components, `match_N`, `proof_N`, `eq_N`, …). Old
  toolchains (4.16/4.21) also expose compiler artifacts (`_cstage2`, `_closed_N`); they are
  marked generated and referenced IR-only names become unresolved nodes.
* Source-level reference locations (`.ilean`, LSP) are not used; edges come from elaborated
  terms only.
* Staleness is per module (content hash) plus a global badge; modules that *import* a changed
  module are not individually marked. Refreshing semantic data is a full re-export, not an
  incremental elaboration. `lake-manifest.json` and `lean-toolchain` hashes are coarse signals of
  dependency state (path dependencies edited in place are not detected).
* When a source file changed after the snapshot, excerpts show the current file with an explicit
  warning that lines may not correspond.

## Search and browsing

* Fuzzy matching on very large indexes is best effort: it runs only when precise tiers found
  few results and stops after 30 ms; on CFSG (2.36 M names) it may not reach every name and is
  biased towards alphabetically early names when cut off. p95 name-search latency on CFSG is
  ~30 ms for that reason (p50 ~5 ms).
* Documentation full-text search is bm25-ranked; on the docstring-heavy Stacks project its p95 is
  ~34 ms.
* The name index uses ~65 bytes per name in memory (153 MiB for CFSG). The CFSG source index is
  650 MiB on disk; no attempt was made to compact it.
* Transitive views are a bounded count plus on-demand tree expansion (depth ≤ 8); there is no
  graph drawing (deliberately).
* Mouse support covers clicking rows and tabs and the wheel; it is on by default, so plain
  drag-selection of text needs Shift in most terminals (`--no-mouse` turns capture off). The
  smallest tab labels (narrow panes) are one-character click targets.
* The default `lean` theme detects a light terminal only from `COLORFGBG`. Terminals that do not
  set it get the dark variant, because the terminal is not queried. The Lean themes paint their
  own background, which hides terminal transparency or background images; `--theme dark|light`
  keep the terminal's colours. Without `COLORTERM=truecolor|24bit` the colours are approximated
  in xterm-256; text keeps 4.5:1, but hues shift (the light theme's blue becomes a slightly more
  violet `#5F5FD7`).
* "Open in editor" (`--editor CMD`) starts the editor detached with `+LINE FILE` and no shell; it
  is not treated as owned work and is not killed on quit.

## Platform and process hygiene

* Linux only was tested (Fedora 44, kernel 7.2). Process hygiene uses Linux features
  (`PR_SET_PDEATHSIG`, bubblewrap user namespaces, systemd user scopes for cgroup limits).
  Without bubblewrap the semantic export is refused unless explicitly run unsandboxed.
* `RLIMIT_AS` is only a coarse backstop because Lean reserves large virtual ranges; the real
  memory limit is the cgroup when a systemd user scope is available (detected and reported).
* UI→worker channels are unbounded but coalesced (only the newest request of each kind is
  served); the scanner's result channel is bounded.

## Measurements

* The 100k/1M benchmark is synthetic: it measures the index and browser back end, not Lean
  extraction or usefulness. UI responsiveness during ingestion is measured as query latency while
  an import runs concurrently, and idle CPU in a pseudo-terminal test; frame times beyond the
  first frame were not separately profiled.
* Real-project numbers (CFSG, Stacks) are source-only.

## Map, goal explorer and trails (milestone 2)

* **The map is curation.** Groups, objectives, purposes and curated arguments are what someone
  wrote in `lean-view.toml`. slim checks that names resolve; it does not check that an
  explanation is true, that an objective is the main theorem, or that a curated step is used.
* **slim does not edit the map.** There is no in-app editor and no accept/reject dialog for
  suggestions: the workflow is to edit the file and reload (`r`). `source_sha256` fingerprints
  are pasted by hand from `slim map check --fingerprints`.
* **Source mentions are lexical.** An identifier counts when it equals an indexed name as written
  or under an enclosing namespace. Dot notation on a variable (`g.Reach`), `open`, notation and
  macros are not seen; a match can be the wrong declaration. Mentions run outward only: there is
  no lexical "mentioned by".
* **Lean dependencies need a semantic snapshot**, and none was produced for a Mathlib-scale
  project (see above). In a source-only index the Lean relation kind and Lean connection queries
  say they are unavailable. Proof terms often go through Lean-generated auxiliaries (the demo's
  main theorem does): they are hidden by default, counted, and shown with `G` or `H`.
* **Connection queries are bounded**: 200,000 declarations or 1.5 s for Lean edges, 400
  declarations for source mentions. "Truncated" means no conclusion. A path is one shortest path,
  not all of them.
* **Rename hints** are declarations with the same last name component, for at most 20 missing
  names; no new scan starts once 400 ms have been spent, so later missing names get no hint. On
  CFSG one scan takes about 0.3 s on the worker (the map is already on screen).
* **Changing a filter or relation kind rebuilds the goal outline** from the goal: the unfolded
  state and the selection inside it are not kept. Selections in the map and in trails are kept
  by identity.
* **Stop staleness compares index fingerprints**: a stop is stale when the file the index saw
  differs from the one recorded with the stop. A file edited since the last index refresh is
  caught by the freshness check, not per stop.
* **Private and anonymous declarations.** A private name embeds its module path and breaks when
  the file moves. Anonymous instances in source-only indexes have a descriptive label rather
  than Lean's generated name, so a stop or landmark on one does not carry over to a snapshot.
* **Group stops** resolve against the map that is loaded now; without the map they are shown as
  unresolved (and kept).
* **Narrow terminals**: below 80 columns one pane is shown at a time; below 30x8 slim only
  says the terminal is too small. Rows are cut with an ellipsis; the detail pane has the full
  text. Tested sizes are listed in the render tests; no mobile client is implied.
* **Mouse** support in the new views is click-to-select, click-again-to-open and the wheel.
* **Not measured**: cold-cache start-up; the views on a real semantic snapshot larger than the
  166-declaration demo; maps with thousands of landmarks (resolution is one indexed lookup per
  name, measured only up to 43 landmarks).

## Highlighting

Design, parity definition and the discrepancy log are in `docs/highlighting.md`; measurements
and the gates that stayed open in `docs/bench/highlight.md`.

* **Where colours appear**: the whole-file source view and the declaration's own lines in the
  excerpt of the declaration view. Statements, documentation and the map, goal and trail views
  are not syntax-coloured.
* **Semantic tokens exist only for files the indexing job processed**: local project files.
  Sources of dependencies (`.lake/packages`) and of Lean itself get syntax colours only. The
  demo opened with `slim demo` has no project directory, so the job cannot be started
  there; open `fixtures/demo` as a project instead.
* **What Lean's semantic tokens are**: keywords (including the project's own), local
  variables, projections, function names in recursive definitions, and the tokens of Verso
  documentation. Lean does not classify theorems versus definitions, types versus terms or
  namespaces, and neither does slim.
* **Other languages inside comments**: a fenced block of another language in a comment gets
  the block's own scopes but is not tokenized as that language (VS Code does that only when
  that language's extension is installed).
* **Lines longer than 20,000 UTF-16 units are not tokenized**, as in VS Code; they keep the
  style in force at their start.
* **CFSG has no semantic tokens here**: its pinned toolchain is not installed and it is not
  built; slim installs and builds nothing.
* **`lakefile.lean` projects in the sandbox** need their compiled Lake configuration to be
  current, because the project is read-only there.
* **Older servers and overlapping tokens**: Lean before 4.28 can return overlapping semantic
  tokens. slim keeps the first, drops the overlapping one, counts it and records the file
  as partial with that reason. This did not occur on the fixture with 4.16 or 4.21.
* **Freshness of dependencies** is by `lake-manifest.json` and the revisions in it, not by
  hashing dependency sources. Inputs read by metaprograms from outside the project are not
  tracked. Unsaved editor buffers are not seen.
* **A moved project** gets a new cache directory (the key includes the root), so its
  highlight cache has to be built again; relative paths inside a cache do not help there.
* **One very large file** can exceed the worker's memory budget by itself while it is shown
  (the budget evicts other files, not the current one). Files over 64 MiB are not displayed
  at all (existing limit); files over 4 MiB are skipped by the indexing job unless
  `--max-file-mib` is raised.
* **Memory figures for the Lean server** in the progress display are resident sizes summed
  over its processes: pages shared between them (memory-mapped `.olean` files) are counted
  once per process, so the sum overstates what the cgroup limit sees.
* **Not present**: in-file search, selection and horizontal scrolling in the source view
  (so their interplay with colours is undefined rather than tested); diagnostics display;
  delta token updates; range requests; automatic re-indexing when files change; pausing the
  indexing job automatically while the reader types.
* **Not measured**: allocation counts; a cold operating-system page cache; SSH or Mosh
  latency; terminals other than the pseudo-terminal and tmux; VS Code screenshots for a
  side-by-side comparison (the comparison is by machine-readable scopes and styles instead,
  and the semantic style rules were read from VS Code's bundle, not observed in its editor).

## Overview features (skim, outline counts, import layers, search within)

Meanings, bounds and measurements are in `docs/overview.md`. What matters most:

* **Everything source-only here is lexical.** A skimmed statement is header text as written,
  counts are counts of scanner candidates, a `sorry` count is a count of tokens, and import
  depth comes from `import` lines. Declarations made by macros are not counted or skimmed.
* **Import layers are build order.** Depth, endpoints and hubs describe files importing files;
  nothing says a result depends mathematically on what its file imports. Endpoints include
  unused and draft files. Module names are paths, so a Lake `srcDir` shows as external.
* **Skims are bounded** (1,500 declarations, 150 files) and say when they stopped. Modifiers and
  attributes written before the keyword are not shown. Keys such as `m`, `a` and `*` still act
  on the open declaration, not on the statement selected in a skim: open it first.
* **The outline is not lazy**: it is built whole on the worker when first opened (about 0.3 s
  on CFSG). Changing the order sorts the children of the open nodes on the UI thread.
* **The import graph is held in memory** once the import view was opened (about 83 MiB by its
  own estimate on CFSG; the `imports` command peaks at 182 MiB there), until the index changes.
* **Search within files** on a very large index reads the whole name buffer and filters
  (CFSG: 8.9 ms p50, 17.6 ms p95); within a namespace it reads only that range. Scoped
  documentation search filters the 600 best-ranked matches, so a rare in-scope match can be
  missed.
* **The Mentions highlight is the lexical match**, in source-only indexes only; semantic edges
  carry no source positions. At most 200 places per identifier.
* **Index format 2**: older indexes are rebuilt (source) or imported again from the cached
  snapshot (semantic); without that snapshot a semantic index has to be imported by hand.
* **Not measured**: scan time and browser memory before and after on the same day; the new
  views on a real semantic snapshot larger than the demo; a person using them.
