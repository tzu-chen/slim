# slim design

Checked 2026-11-10. Versions in this document are the ones actually built and run.

## 1. Reconnaissance (bounded) and what it decided

| Source | Checked | What it decided |
|---|---|---|
| Lean toolchain sources installed by elan: `Lean/Environment.lean`, `Lean/Util/CollectAxioms.lean`, `Lean/DocString*.lean`, `Lean/DeclarationRange.lean`, `Lean/Meta/Instances.lean`, `Lean/Class.lean`, `Lean/ScopedEnvExtension.lean`, `Lean/Setup.lean`, `Init/Meta*.lean` for v4.16.0, v4.21.0, v4.28.0, v4.30.0-rc2, v4.32.0, v4.33.1, v4.35.0 | read locally | exporter APIs per version (below); identifier character classes for the lexer; the exact relation `collectAxioms` follows |
| Lean reference, *Elaboration and Compilation* (lean-lang.org/doc/reference/latest) | fetched | `.olean` files are memory images ("represented just as they are in memory … can be directly memory-mapped"): loading untrusted ones is a trust decision; dependencies' initialization code runs in the interpreter during import; `.ilean` contents "may change at any release" |
| doc-gen4 (github.com/leanprover/doc-gen4) | fetched | Apache-2.0; runs as a Lake facet in a nested `docbuild` project on a *built* library, pinned per Lean release. It documents declarations but does not export typed statement/proof edges or axiom paths, and requires evaluating the project's Lake configuration. Not used; nothing copied. |
| import-graph (github.com/leanprover-community/import-graph) | fetched | Apache-2.0; module-level import graphs via `lake exe graph` on a built project. slim gets module imports from the `.olean` headers (semantic) or the source headers (source-only) instead. Nothing copied. |
| vscode-lean4 / LSP `References` | not used | live mode (LSP) is out of scope for this milestone; `.ilean` is version-specific and incomplete as a proof-dependency source |
| bubblewrap 0.12.0, systemd 25x user scopes | probed on this host | sandbox and cgroup limits are detected at run time and described in the plan |

No code was copied from any of these projects. The exporter re-implements two small predicates
whose behaviour is documented in Lean's sources (internal-name detection and the axiom-collection
relation); Lean 4 is Apache-2.0.

### Exporter API facts per version (from the toolchain sources)

| | 4.16 | 4.21 | ≥ 4.28 (4.33.1, 4.35.0) |
|---|---|---|---|
| `importModules` | `(imports) (opts) (trustLevel) (leakEnv)`: always runs extensions' import hooks | adds `loadExts := false`, `level := .private` | same as 4.21 |
| `Import` fields | `module`, `runtimeOnly` | `module`, `importAll`, `isExported` | + `isMeta` |
| `ModuleIdx` | `def ModuleIdx := Nat` | same | `@[expose] def` |
| `collectAxioms` | does not follow axiom types | same | follows axiom types; 4.33 reads precomputed per-module data when extensions are loaded |
| `native_decide` axioms | `Lean.ofReduceBool` | `Lean.ofReduceBool` | 4.28: `+ Lean.trustCompiler`; 4.33+: one auxiliary axiom per use (`<decl>._native.native_decide.ax_n_m`) |
| identifier letters | Greek, Coptic, letterlike, … | + `ⱼ` subscript | + Latin-1 Supplement, Latin Extended-A |

Hence three compatibility shims (`exporter/compat/{legacy,mid,modern}`) selected by version, all
sharing `exporter/Main.lean`; and a version-aware lexer profile.

## 2. Architecture

Three components with clear boundaries:

1. **Browser** (`src/app`, Rust + Ratatui 0.30.2 / crossterm 0.29.0). Reads a local index and
   source files. Never starts Lean. The UI thread only renders and handles input; it blocks on one
   channel (no polling, no redraw timer) and talks to workers:
   * query worker: read-only SQLite connection + user-data store; coalesces superseded requests;
   * search worker: in-memory name index; abandons obsolete scans via an atomic generation;
   * job runner: one long task at a time (freshness check, incremental rescan, initial source
     build, semantic export), cancellable, throttled progress events.
   Responses carry generation numbers; stale ones are dropped.
2. **Indexer / importer** (`src/db`, `src/source/index.rs`). SQLite (bundled 3.53.2 via rusqlite
   0.40.2), one file per (project, mode) under the cache directory, schema version 2 (version 2
   added per-file counts to `modules`; see `docs/overview.md`).
   Snapshot import is two-pass and transactional: validation + deterministic id assignment, then
   batched loading with a recorded byte offset (an interrupted import resumes), then analysis
   (degrees, axiom closure, FTS5, secondary indexes, name-index side file), then an atomic rename
   over the previous index, which stays usable until then. Source indexes are built the same way
   and updated per file in one transaction.
3. **Lean semantic exporter** (`exporter/`, Lean). Built per toolchain by `slim export` into
   the cache, run sandboxed on the project's existing build artifacts, emitting plain JSON Lines.
   The Rust driver validates the output and wraps it into a snapshot with provenance.

SQLite was chosen over a custom format because it gives transactions, crash safety, indexed
reverse edges, FTS5 and concurrent readers for free; the one structure SQLite is slow at,
substring/fuzzy name search over millions of names, lives in a separate memory-resident index
(`<index>.names`) tied to the database by a generation id.

## 3. Data modes

**Source-only.** `discover` lists `*.lean` files (skipping hidden/build directories, not following
symlinks out of the root); `scan_source` lexes each file and extracts imports, module docs,
namespaces/sections/mutual blocks, doc comments, attributes, modifiers and declaration
candidates (theorem/lemma/def/abbrev/instance/structure/class/inductive/axiom/opaque/alias,
constructors, structure fields) with byte extents, statement header, and lexical observations
(`sorry`/`admit`, `native_decide`). Issues are recorded per file (unterminated comments/strings,
unbalanced brackets, unrecognised commands); a file with issues is marked `partial`. Duplicate
names are flagged ambiguous; anonymous instances are labelled by their type. The lexer's
identifier classes follow the project's `lean-toolchain` (see table above).

**Semantic snapshot.** Produced by the exporter (or the deterministic benchmark generator, whose
header says `"synthetic": true`).

**Live (LSP).** There is no live mode: the browser never talks to a language server while you
read. The one LSP client (`src/highlight/lsp.rs`) belongs to the explicit semantic-highlight
indexing job (§13), which asks the project's own server for semantic tokens once and caches
them. Position conversion utilities handle UTF-8 bytes, code points (Lean's `Position.column`)
and UTF-16 units (LSP's default `positionEncoding`), with tests.

## 4. Snapshot format (`slim-snapshot`, format 1)

UTF-8 JSON Lines. First line `{"record":"header", "format":"slim-snapshot","format_version":1,
producer, exporter (the exporter's own header: version, compat shim, Lean version and githash,
whether extensions were loaded, olean level), lean {version, githash, toolchain}, project {name,
root_hint, lean_toolchain, lake_manifest_sha256, revision}, scope, created_at, build, isolation,
synthetic}`. Then any number of

* `module`: name, local, path (relative, validated), sha256/size/mtime of the source and mtime of
  the `.olean` at export time, imports `[{module, flags:[public|all|meta|runtime]}]`, doc;
* `decl`: canonical name (Lean's escaped `Name.toString`, private names unmangled only for
  display), kind, module, local, flags, generated (reason), pretty-printed type, binders
  `[{name, info, type}]` + conclusion (local declarations), doc, range and selection range
  (line + code-point column + UTF-16 column), `refs {type, value, ctor}` (canonical names),
  `coverage {type, value}` ∈ complete | unavailable | partial | not_applicable | not_extracted,
  `lean_axioms` (Lean's `collectAxioms`, when validation was requested);

and a final `footer {counts {modules, decls}, complete}`. A missing footer or count mismatch
rejects the snapshot (or imports it explicitly as partial with `--allow-partial`). Names and
texts are bounded; paths must be relative with normal components only; nothing in a snapshot
is executed.

Identity: declarations are deduplicated by canonical name within a snapshot (duplicates are an
error); referenced but undescribed names become persisted *unresolved* nodes (coverage
not_extracted), never dropped. The exporter imports all local modules into **one** environment,
so imported library declarations appear once.

## 5. What an edge means

| Edge | Meaning | Source |
|---|---|---|
| module import | header `import` of a module | `.olean` header (semantic) / source header (source-only) |
| **T** type | the constant occurs in the declaration's elaborated type | `ConstantInfo.type.getUsedConstants` |
| **V** value | the constant occurs in the elaborated value: proof term of a theorem, body of a definition/opaque | `value.getUsedConstants` |
| **C** ctor | inductive type → its constructors | `InductiveVal.ctors` (Lean's axiom collector follows it) |
| lexical mention | an identifier in the source text equals an indexed name (as written or under an enclosing namespace) | source-only, computed on demand, labelled, never stored as an edge |

Source-level resolved reference locations (`.ilean`/LSP) are not implemented. Proof-term edges
include constants chosen by automation (`simp` lemmas, instances); the UI says so and never
presents them as a reading order. Counts are navigation aids.

Coverage is per declaration and edge class: an empty list with incomplete coverage is shown as
"not evidence of no dependencies". Library declarations reachable from the project are exported
with their edges (`--library full`, default) so axiom paths can cross into the library; with
`frontier`/`none` they are marked not_extracted and every declaration that reaches them gets a
coverage-gap marker.

## 6. Assumptions and evidence

The transitive axiom analysis (`src/axioms.rs`) follows exactly the relation of Lean's
`collectAxioms`: T, V and C edges. It computes strongly connected components (iterative Tarjan)
and propagates axiom bitsets in reverse topological order, so cycles (an inductive and its
constructors; unsafe recursion) are handled exactly; a pseudo-axiom marks reachability of a
declaration with incomplete data. For every (declaration, axiom) pair it stores the next hop, so
the UI shows a witness path made of recorded edges. Categories: standard (`propext`,
`Classical.choice`, `Quot.sound`, shown by name, not as errors), `sorryAx` (placeholder),
compiler trust (`Lean.ofReduceBool`, `Lean.trustCompiler`, per-use `native_decide` axioms),
custom axioms, coverage gap.

Validation: the exporter optionally records Lean's own `collectAxioms` per local declaration;
`slim check-axioms` and the test suite compare. For the fixture this was done on all five
tested toolchains (identical; on 4.16/4.21 up to the documented 4.28 rule change, recomputed
under the old rule), and the bundled snapshot is additionally compared with the raw
`#print axioms` output in `fixtures/demo/axiom-report/v4.33.1.txt` (73 names).

The evidence panel keeps distinct: source candidate; semantic declaration extracted (Lean
version, snapshot date); build artifacts present at export (slim does not run or verify the
build); current-source status (SHA-256 match / changed since snapshot / unavailable); explicit
axiom; dependency on `sorryAx`; extraction incomplete; stale snapshot (global badge from the
freshness check, plus per-declaration notes). There is no "proved" badge.

## 7. Search

Display names are ASCII-lower-cased into one contiguous buffer sorted by name; a permutation
sorts by last component. Tiers: exact qualified name; exact last component; prefix; last-component
prefix; word prefix (after `.`/`_`); substring (`memmem` over the whole buffer — a hit cannot
span names because they are newline-separated); separator-insensitive variant (`feit_thompson`
finds `FeitThompson`); all words (multi-word queries); fuzzy subsequence (word-start anchored,
compact span; separators in the query match any word boundary) only when the precise tiers did
not fill a page and nothing matched exactly, under a 30 ms budget. Within a tier: local before
library before unresolved, user-written before generated, then shorter names, then name order.
A bounded heap keeps the top 300; nothing sorts the full index per keystroke. Documentation,
attributes and statements of semantic declarations are in an FTS5 table (bm25). Filters (local
only, generated shown, kind) are applied before ranking and reported in the summary line.

## 8. Performance design

* Warm open does no scanning, migration or graph work: open SQLite, read metadata, load the
  name-index file (one read), render. Freshness is checked in the background (size+mtime as a
  hint, SHA-256 when they differ; `lean-toolchain` and `lake-manifest.json` hashes for dependency
  state).
* Neighbour queries walk the `(src, kind, dst)` primary key or the `(dst, kind, src)` index in
  id order; ids were assigned local-first and name-sorted at import, so index order is a stable
  presentation order and no query sorts large result sets. Lists are bounded with explicit "N
  more" rows; counts beyond 10,000 after filters are reported as lower bounds.
* Source indexing: a bounded thread pool (default min(4, available CPUs)) feeds a single SQLite
  writer through a bounded channel; secondary indexes are built after bulk load.

## 9. Process hygiene

`src/process.rs` owns every child: new session/process group, `PR_SET_PDEATHSIG(SIGKILL)` (with
the parent-race check), `RLIMIT_CORE=0`, `RLIMIT_CPU`, `RLIMIT_FSIZE`, a coarse `RLIMIT_AS`
backstop (Lean reserves large virtual ranges for worker-thread stacks, so the real memory limit
is a cgroup `MemoryMax` via a systemd user scope, used when available and otherwise reported as
unavailable), wall-clock timeout, cancellation, `SIGTERM`→`SIGKILL` of the whole group, and
always waiting for the child. A registry lets the panic hook and shutdown kill remaining groups;
exit warns if any remain. Export runs take a per-user `flock` (one at a time) and use a fresh
private input directory (bound read-only) and output directory (the only writable bind), created
with `create_new` and read back only if they are regular files (`O_NOFOLLOW`), so code running
in the sandbox cannot redirect host-side writes through symlinks.

## 10. Versions of everything

Rust 1.99.1; ratatui 0.30.2; crossterm 0.29.0; rusqlite 0.40.2 (bundled SQLite 3.53.2);
serde_json 1.0.154; toml 1.2.1 (milestone 2, the project map); sha2 0.10.9; memchr 2.8.3; unicode-width 0.2.3; signal-hook 0.3.18;
libc 0.2.191; anyhow 1.0.104; onig 6.5.3 with onig_sys 69.9.3 (bundled Oniguruma 6.9.10; added
for highlighting) (all locked in `Cargo.lock`). Lean toolchains under test: 4.16.0,
4.21.0, 4.28.0, 4.33.1, 4.35.0. bubblewrap 0.12.0. Fedora 44, kernel 7.2.11.

## 11. Colour themes

The default theme, `lean`, takes its colours from lean-lang.org (checked 2026-11-10): the site's
CSS custom properties (`:root` for light, `.dark-theme` for dark) for page colours, and its Verso
code stylesheets (`-verso-data/lean-themed.css`, `lean-dark.css`) for code roles. `lean-light`
and `lean-dark` pick a variant; `lean` picks light only when `COLORFGBG` reports a white terminal
background (7 or 15), otherwise dark. The terminal is not queried (no OSC 11 round trip at
start-up). `SLIM_THEME` sets the theme like `--theme`.

| role (`Palette` field in `src/app/theme.rs`) | light | dark | lean-lang.org source |
|---|---|---|---|
| `bg` | `#F9FBFD` | `#181818` | `--color-bg` |
| `text`, `bar_fg`, `unfocused_fg` | `#333333` | `#EEEEEE` | `--color-text` |
| `dim` | `#5F6B78` | `#90A4AE` | `--color-muted` |
| `accent`, `border_focus`, `selected_bg` (also the active tab), `tag_type` (T) | `#386EE0` | `#3B94FF` | `--color-primary` |
| `heading`, `input` | `#1D4ED8` | `#6AADFE` | `--color-primary-focus` / `--color-primary-light` |
| `selected_fg` | `#FFFFFF` | `#121212` | `--color-text-contrast` / `--color-surface` |
| `border` | `#D1D9E2` | `#444444` | `--color-border-light` |
| `bar_bg` | `#E1E7F0` | `#232838` | `--color-border` / code-block navy |
| `unfocused_bg` | `#D5E1FF` | `#2C3346` | gradient tint / code-block navy |
| `code` | `#24292F` | `#D4D4D4` | code text |
| `decl_name` | `#247A93`\* | `#4EC9B0` | code: constants |
| `kind`, `tag_ctor` (C) | `#8B21B8` | `#C586C0` | code: keywords |
| `tag_value` (V) | `#098155`\* | `#B5CEA8` | code: literals |
| `ok` | `#2F7D4F` | `#6A9955` | code: comments |
| `warn` | `#91680F`\* | `#E7A71D` | warning indicator |
| `danger` | `#E30000`\* | `#FF4545`\* | error indicator |
| `highlight_bg` (current declaration in the source view) | `#EAF0FB`† | `#2B2B2B`† | — |

\* The site colour changed in lightness only (same hue) until it reaches WCAG AA, 4.5:1, on the
background. The site values are `#267F99`, `#098658`, `#E7A71D` and `#FF0000`. On dark, the
selection text is the surface colour because white on `#3B94FF` is only 3.1:1.
† Not a site colour: the light background mixed with about 8 % of the primary blue, and the dark
background lifted about 9 % towards the text colour.

A unit test checks the theme's actual styles: every text role against the background, and the
selection, active tab, unfocused selection, bar and highlighted-line pairs (all ≥ 4.5:1, in
24-bit and in xterm-256 colour). Both themes paint their own background over the whole frame, so
they read the same in a light or dark terminal. A render test asserts that no cell is left at
the terminal's default colours; overlays clear their area and must repaint it. Colours are sent
as 24-bit RGB when the terminal advertises it (`COLORTERM=truecolor|24bit`, or
`SLIM_TRUECOLOR=1|0` to override). Otherwise each text colour becomes the nearest xterm-256
colour that keeps 4.5:1 against its own approximated background. Two roles move this way: light
declaration names become `#005F87`, because the plain nearest colour `#008787` has only 4.36:1,
and dark "ok" becomes `#5FAF5F`. All other colours take the plain nearest colour; for example,
the light primary blue becomes `#5F5FD7`. The older themes (`dark`, `light`, `contrast`, `mono`)
keep the terminal's own background and 16-colour palette, and `NO_COLOR` selects `mono` unless a
theme is given. Every state shown in colour is also shown by a symbol or word.
`scripts/capture-screens.sh --themes-only` renders `docs/screenshots/theme-lean-*.png` from real
tmux captures (`scripts/ansi-screenshot.py`).

## 12. Milestone 2: map, goal explorer, reading trails

Usage and the file format are in `docs/map.md`; decisions and the module list are in
`docs/milestone-2-notes.md`. The points that constrain future changes:

* **Three kinds of data, three places.** Regenerable indexes (cache directory); the shareable
  map `lean-view.toml` (project root, read-only for slim); personal reading data
  (`userdata.sqlite`, schema version 2 in `PRAGMA user_version`).
* **Evidence travels with the edge.** `relations::EdgeLabel` is `Curated`, `Lexical` or `Lean`
  and is attached to every child, connection step and rendered row. The mixed view merges rows
  by declaration and keeps every label.
* **Direction.** "A uses B" runs from a statement to its prerequisite; `Direction::Uses` lists
  B under A. The same enum drives the SQL column choice, so the UI and the queries cannot
  disagree.
* **Unfolding.** The goal outline is an arena of nodes; a child whose name is on the path to the
  root is a back-reference and is never unfolded; a name seen elsewhere gets a link marker only.
  Nothing is computed transitively except the bounded connection search.
* **Workers.** Map loading, name resolution, child pages and reading-data changes run on the
  query worker; a connection search runs on its own short-lived thread with a generation counter
  as its cancellation token, joined on cancel, on completion and at exit. The event loop is
  unchanged: it blocks on one channel.
* **Selection is an identity.** Map rows have string keys (`grp:<id>`, `lm:<group>:<decl>`,
  `obj:<decl>`, …); history entries store the view and the key.

## 13. Syntax and semantic highlighting

Usage, design, protocol, cache, freshness and parity are in `docs/highlighting.md`; what was
inspected and decided first in `docs/highlight-phase0.md`; measurements in
`docs/bench/highlight.md`. The points that constrain future changes:

* **Layers do not know each other.** Baseline spans (TextMate scope stacks), semantic spans
  (Lean's token types with the server's legend) and styles (a theme applied to either) are
  separate data. A theme change touches only styles; a grammar change only baseline spans;
  a toolchain change only the freshness of semantic spans.
* **Spans are byte ranges of exact text.** Every span set is keyed by the SHA-256 of the text
  it was computed for and is applied to nothing else. LSP units are converted once, strictly,
  at extraction; terminal cells exist only in the renderer.
* **The UI thread holds an immutable snapshot** of the file on screen and resolves styles for
  visible lines. Tokenizing, cache reads and freshness checks are on the highlight worker,
  which blocks when idle. Nothing about highlighting runs before the first frame.
* **Only the indexing job starts Lean**, after a typed confirmation, with builds switched off,
  sandboxed, under time and memory limits per file, on its own thread with its own cancel and
  pause flags. A file can fail, time out or be stopped for memory without ending the run; a
  record says which, and coverage is never inferred.
* **Freshness is several facts, not one**: source alignment, extraction coverage, environment,
  imported sources, build relation. Unaligned tokens are never shown; aligned but outdated
  ones only in the labelled stale preview.
* **The highlight cache is its own SQLite file** (`highlight.sqlite`, schema version 1) beside
  the index, which is replaced wholesale on rebuild.

## 14. Overview: skim, outline counts, import layers, scoped search

Usage, definitions and measurements are in `docs/overview.md`. The points that constrain
future changes:

* **Counts are recorded at index time, per file** (`modules.n_thm` … `proof_lines`, written by
  the scan and by the snapshot import), and summed per directory when the outline tree is
  built on the worker. Nothing aggregates the `decls` table while browsing.
* **The import graph is a worker-side, memory-resident structure** (`src/imports.rs`), read on
  first use and dropped when the index is replaced, like the name index of the search worker.
  The UI holds an `Arc` to it and builds rows for open nodes only; per-directory summaries and
  import paths are computed on the worker and addressed by name, not by index.
* **A skim is one bounded worker request** (`src/skim.rs`) whose result the UI lays out once
  per width; the selection is patched onto cached rows at draw time.
* **A search scope is a range or a set**: a namespace is a contiguous range of the sorted name
  buffer; files are a bit set of module ids tested per entry (the name index stores the module
  of every entry since format version 2 of the side file).
* **Occurrences travel with a mention** (`relations::Occurrence`: line and byte range in that
  line) and are drawn as a mark layer above syntax and semantic colours; marks never change
  text or wrapping (`highlight::render::styled_rows_marked`).
