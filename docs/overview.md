# Getting the big picture: skim, outline counts, import layers, search within

Added 2026-11-10 on top of the map, goal explorer and trails. Everything here works in a
source-only index, without a project map and without Lean; nothing is computed before the
first frame, and nothing runs on the UI thread except laying out the rows on screen.

| Key | What | Where the data comes from |
|---|---|---|
| `S` | **Skim**: statements of a file, directory, namespace or map group, proofs left out | the index (byte extents, docs); source-only: the header text read from the file |
| `o` | **Outline column and order**: declarations, theorems, proof lines, sorry, last modified | per-file counts recorded when the index is built, summed per directory |
| `I` | **Import layers**: files by import depth, endpoints, hubs; `p` asks why a file imports a module | `import` lines (source-only) or `.olean` headers (snapshot), read into memory on first use |
| `f` / `F` | **Search within** the selected directory or the current namespace / everywhere again | the name index (a namespace is a contiguous range; files are a set of module ids) |
| `1`, then `j`/`k` | **Mentions**: the occurrences of the selected identifier are highlighted in the centre pane | positions recorded by the same lexical pass that finds the mentions |

Scripted forms: `slim skim DIR <MODULE|DIR>` (`--namespace NS`, `--group ID`),
`slim imports DIR [FROM TO]`, `slim search DIR WORDS --within FILES | --namespace NS`.

## Skim (`S`)

`S` skims what is selected: a directory or file in the outline, in the map's file tree or in
the import view; a group of the project map; otherwise the file of the declaration at hand.
Each declaration is its statement, the lead of its documentation (first paragraph, cut) and
how many lines were folded away. `j`/`k` move by statement, `Enter` opens one (Backspace
returns to the same statement), `S` switches between the file and the namespace of the
selected declaration, `Esc` closes.

* **Source-only:** the statement is the text from the declaration keyword to `:=`, `where` or
  the first `|`, exactly as written (a statement written over several lines is flowed into the
  pane unless it contains a comment). It is not elaborated, and modifiers and attributes before
  the keyword are not shown. When a file changed since it was indexed, the stored byte offsets
  are not applied to it: names only, with a warning.
* **Semantic:** the elaborated signature from the snapshot, as Lean printed it.
* **A file is read from top to bottom.** Its doc sections (`/-! … -/`) are kept where they
  stand: each Markdown heading with the lead of the text under it, and how many lines were
  folded. In a source-only index the `variable` and `open` lines its statements are written
  under are kept too, with the `end` of the scope that holds them. Only the text between the
  declarations is lexed for this. A namespace or map-group skim picks declarations out of
  their files and shows neither; a semantic index reads the prose of a single-file skim only
  (its other skims do not open the files).
* **Bounded:** 1,500 declarations and 150 files per skim; a statement longer than 1,600 bytes
  is cut. Each of these is said on screen when it happens.

## Outline counts (`o`)

The scan records per file: theorems, definitions (`def`, `abbrev`, `opaque`), types
(`structure`, `class`, `inductive`), instances, axioms, how many of those have a doc comment,
declarations with a `sorry`/`admit` token, lines, and **proof lines** (for each theorem, the
lines from the end of its statement to the end of the declaration). The outline sums them per
directory. `o` cycles the right-hand column and sorts by it; selecting a node in the map's
"Files and namespaces" section shows every count.

* Semantic indexes count user-written declarations of the snapshot; "sorry" there means
  *declarations whose dependency closure reaches `sorryAx`*; proof lines are not recorded.
* Counts are sizes. There is no percent proved, and a file with no `sorry` token proves nothing.
* The outline is still built whole on the query worker when first opened (about 0.3 s for
  CFSG's 230k files); it is not loaded lazily per directory.

## Import layers (`I`)

* **Depth** of a file: the longest chain of local imports below it (0: it imports nothing from
  the project). A directory shows the range over its files that declare something, and
  directories are listed from the bottom of the import order to the top.
* **Endpoints**: files with declarations that no other *declaring* file imports. Import-only
  umbrella files do not count as importers, otherwise one file importing everything would hide
  every endpoint. Final results tend to be here; so does unused code.
* **Hubs**: the 40 files most often imported directly.
* **`p`**: a shortest chain of import lines from the selected file to a module given by name,
  `Dir/File.lean` path or unambiguous end of a name. The graph is in memory and the search is
  exhaustive: the answers are a path, "does not import it", or "no such module".
* The detail pane of a directory groups what it imports and what imports it by sibling
  directory, and external imports by first name component.

These are build dependencies read from import lines. They are not mathematical dependencies,
and the view says so. In a source-only index module names are paths, so modules under a Lake
`srcDir` or of other packages show as external. Files on or above an import cycle are counted
and their depth is a lower bound.

## Search within (`f`, `F`)

`f` restricts search to the directory or file selected in an outline, else to the namespace of
the declaration at hand; pressed again it widens one level, and past the top it clears. `F`, or
Backspace in the empty search box, searches everywhere. The scope is always shown under the
search box. Inside a scope an empty query lists what is there. Documentation search is scoped
after ranking: the scoped part of the 600 best-ranked text matches.

## Mentions highlight

In a source-only index, moving through the Mentions tab (`1`) highlights every place the
selected identifier is written in the declaration: in the statement, in the source excerpt and
in the whole-file view (`s`), and scrolls the first place into view. The footer says how often
and on which lines. Up to 200 places per identifier are recorded. It is the same lexical match
as the tab itself: the identifier as a token, not what Lean resolved. Semantic indexes show
Lean's edges in tab 1, which carry no source positions, so nothing is highlighted there.

## Index format 2

Per-file counts are new columns of `modules`, and the name index now stores the module of
every entry, so the index format went from 1 to 2. An older index is derived data and is never
read as if it were current: a source-only index is rebuilt when the project is opened (CFSG:
about 20 s); a semantic index is imported again from the snapshot kept in the cache, and when
that snapshot is gone the source-only index is shown with a message.

## Measured

2026-11-10, release build, 2 CPUs under `research-safe-run`, warm page cache, one run of
`slim bench --index` per project with the final binary (raw output in
`docs/bench/overview-*.md`). Milliseconds, p50 / p95.

| | HaagerupIzumi (1,435 files, 15,390 candidates) | CFSG (229,089 files, 2.36 M candidates) |
|---|---|---|
| name search, whole index (unchanged path) | 0.14 / 0.58 | 4.6 / 30.5 |
| name search within a namespace | 0.05 / 0.52 | 0.03 / 0.28 |
| name search within files | 0.06 / 0.32 | 8.9 / 17.6 |
| outline tree with per-directory counts | 1.2 | 301 |
| skim of a file | 0.16 / 0.63 | 0.14 / 0.30 |
| skim of a directory (bounded) | 11.0 / 11.3 | 10.1 / 13.4 |
| skim of a namespace | 14.3 / 15.0 | 2.7 / 15.9 (max 85) |
| mentions with positions | 0.16 / 0.36 | 0.24 / 0.52 |
| import graph: read and analyse (once per session) | 1.2 | 326 |
| imports of a directory, grouped | 0.00 / 0.06 | 0.01 / 1.7 (max 42: the whole project) |
| import path | < 0.01 | 1.4 / 3.7 |

CFSG cold scan with the new counts: 20.3 s, max RSS 808 MiB, index 652.8 MiB + 162.3 MiB name
index (one run; the previous format measured 18.7 s, 805 MiB, 650 + 153 MiB on another day, so
the difference in time is not established). The import graph of CFSG holds about 83 MiB by its
own estimate; `slim imports` on CFSG peaks at 182 MiB resident.

**Not measured:** a before/after comparison of scan time and of the browser's memory with the
same session; frame times of the new views on CFSG; the semantic path beyond the demo snapshot.

## Reading a statement in its scope

Added on 2026-11-11 for reading a large, sparsely documented project (the YM repository:
9,564 files, 6% of its declarations documented, a small human-written layer of checks and
worked examples on top of a machine-written one).

* **In scope.** Under a statement, the `variable` and `open` lines in force where the
  declaration stands. They are read from the text *between* the declarations of its file (the
  extents of the declarations are in the index), with the section or namespace each command
  ends with. Asked for after the declaration is shown, so opening a declaration takes as long
  as before.
* **Mentions follow Lean's lookup.** A name is tried under the enclosing namespaces, innermost
  first; failing those, as written and under each opened namespace that exists in the index.
  A declaration is listed once, however it is spelled and however often; of several
  declarations with the same name the one in the nearest file is listed, with `+N` for the
  others. Names and namespaces looked up are remembered while the index stays open.
* **Cards.** With the right pane focused, the selected name shows where it is written, its
  statement and the lead of its documentation under the declaration being read (under the
  list when the terminal has no room for both panes).
* **Prose in skims and in the outline.** See "Skim" above; an expanded file in the outline
  lists the headings of its doc sections among its declarations.

Two defects of the scanner were found on the way and fixed, so a source-only index written
by an older slim is built again when its project is opened (YM: under a second; CFSG: about
20 s):

* a module doc that directly follows the imports was dropped (the next doc block of the file,
  if any, was recorded as its module doc). On YM the files with a recorded module doc went
  from 832 to 7,711 of 9,564;
* in a structure, the identifiers after a field name were recorded as further fields
  (`order_le m : …` gave a field `m`); they are binders, as Lean 4.33.1 confirms. On YM this
  removed 5,666 candidates, and the scan reports 894 ambiguous names instead of 4,633.

Measured on 2026-11-11 with the build before this work and the build after it, on the same
index, 2 CPUs under `research-safe-run`, warm page cache, one `slim bench --index` run each
(raw output in `docs/bench/scope.md`). Milliseconds, p50 / p95, before → after.

| | YM (9,564 files, 119,730 candidates) | CFSG (229,089 files, 2.36 M candidates) |
|---|---|---|
| declaration detail | 0.06 / 0.07 → 0.05 / 0.06 | 0.06 / 0.06 → 0.06 / 0.06 |
| declarations of a module (outline) | 0.03 / 0.05 → 0.03 / 0.04 | 0.03 / 0.10 → 0.03 / 0.10 |
| name search, whole index | 0.34 / 4.5 → 0.30 / 3.9 | 4.6 / 30.0 → 4.8 / 28.3 |
| skim of a file | 0.12 / 0.18 → 0.13 / 0.18 | 0.14 / 0.35 → 0.16 / 0.39 |
| skim of a directory (bounded) | 1.7 / 7.9 → 2.1 / 9.5 | 9.7 / 12.9 → 10.9 / 14.4 |
| skim of a namespace | 0.87 / 6.6 → 0.85 / 6.3 | 2.3 / 17.7 → 2.3 / 18.1 |
| mentions with positions | 0.18 / 0.41 → 0.30 / 1.1 (max 0.85 → 2.6) | 0.21 / 0.58 → 0.31 / 1.4 (max 1.8 → 3.9) |
| scope of a declaration (new) | 0.10 / 0.13 | 0.09 / 0.11 |
| card of a declaration (new) | 0.04 / 0.04 | 0.04 / 0.04 |

Two paths are slower. A directory skim now also reads the prose and the scope lines of its
files (about 0.4 ms more on YM, 1.2 ms on CFSG). A mention lookup on a declaration picked at
random costs about half as much again at the median and two to three times as much at p95: it
reads the `open` lines of the file and tries each name under the opened namespaces too. The
benchmark picks declarations all over the project, which is the worst case for the name memo;
moving through one file was not measured separately. Everything else is within the noise of
two runs.

Scans with 2 threads into an empty cache: YM 0.80, 0.72, 0.72 s before and 0.72, 0.71, 0.71 s
after; CFSG 21.5, 20.6 s before and 19.5, 19.4 s after (the runs of one build after the
other's, so the difference on CFSG is not established). Index sizes: YM 30.0 → 31.0 MiB (module docs
now recorded), CFSG 652.6 → 652.4 MiB. Peak resident memory of the benchmark process: YM
54 → 57 MiB, CFSG 534 → 526 MiB (one run each).

**Not measured:** frame times with the card and the scope section on screen; the semantic
path beyond the demo snapshot; a session that stays in one file.

## Tests

Unit tests for the import graph, skim, scoped search, the outline order, marks and the views;
`tests/overview.rs` checks counts, the import graph, skim, scoped search and occurrence
positions on the demo library against values recomputed in the test; two pseudo-terminal
sessions (`tests/pty.rs`) drive the features by keyboard and cover old index formats.
