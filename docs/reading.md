# Reading a large proof: the reference graph, the contract view and the reading views

Added on 2026-11-21. It was built and measured on the Lean formalization of the Riemann
hypothesis that Levent Alpöge announced on 20 November 2026 (175,187 files, 1.13 M
declarations, 17.4 M lines, no docstring and no comment anywhere), and then tried on the
proofs of [anthropics/formal-math](https://github.com/anthropics/formal-math), which the
guide's session uses. Nothing here is specific to a project.

Three facts about such a project decide what helps a reader:

1. **There is no prose.** Everything comes from names, statements and structure, or from
   annotation added afterwards.
2. **It speaks in named propositions.** An upper-level theorem reads
   `(h : GalStar.Row0b fun h rc => Zero.LLC.IsRecT h rc) : EveryCharacter`. Each name is a
   short definition a file or two away.
3. **The top is narrow and the bottom is enormous.** A few dozen declarations lead from the
   endpoint to the places where the mass is.

slim's answer is in four parts: a **reference graph** of the whole project; views that read
it (**start page**, **contents by dependence**, **named statements**, **why is this here**); a
**contract view** that unfolds a statement's vocabulary in place; and **overlays**, by which
other people's reading comes in and a reader's own goes out.

## The reference graph

```sh
slim graph build <PROJECT_DIR> [--artifacts DIR] [--lexical] [--threads N]   # scans first if there is no index
slim graph status <PROJECT_DIR>
slim graph show <PROJECT_DIR> <NAME>      # what a declaration refers to, and what refers to it
```

One file beside the index (`<index>.graph`), built on request, read whole into memory by the
browser. For every declaration of the index it holds the declarations of the index it refers
to, and whether each reference stands in the statement, after it, or both. Building reads
files and runs nothing.

Where the edges come from is decided per file, recorded, and shown wherever they are used:

| Label | Source | What it is not |
|---|---|---|
| `[ref]` Lean references | the `.ilean` file Lake wrote for the module: every constant written in it, as the elaborator resolved it (notation, dot notation and anonymous constructors included) | the kernel's dependency relation: what Lean found without anything being written (instances, the default `simp` set) is in no `.ilean` |
| `[lex]` source mentions | identifiers that equal an indexed name under Lean's lookup rules (enclosing namespaces, `open` lines in force, `_root_`), with the names a declaration binds itself left out | resolved by Lean |
| `[lean]` Lean dependencies | the T/V/C edges of a semantic index | |

An `.ilean` is used for a file only when it is of the text shown: by Lake's source hash when
the `.trace` records one; else when the `.ilean` is the one the trace names (its hash), is not
older than the file, and the declarations it records stand where it says. Otherwise the file
is read lexically, and `graph status` says how many files went which way and why.

**Artifacts from elsewhere.** A project that was never built here can use the artifacts of a
published build cache: `--artifacts DIR` names a directory laid out like a Lake build
directory (`DIR/lib/lean/**/*.ilean`). It is used for packages that have no build directory
of their own, and remembered in a file `artifacts` beside the index, so the Mentions and
Used by tabs find it too. slim downloads nothing: getting such a directory is the reader's
step.

A graph belongs to the index it was built from: after a rescan that changed anything it is
not used, and the commands say to build it again.

### Endpoints, reach, dominators

Everything below is computed in memory for a set of roots, in this order of preference: the
goal pinned with `*`; the `[[objective]]` entries of the project map; the theorems of the
files that no declaring file imports (directly or through files that only import), when
there are at most 32. A library with hundreds of last files has no endpoints until a goal is
pinned, and the views say so; a directory that holds several Lake packages is told which
ones to open instead.

A project checked with Comparator restates its theorems in a challenge file, `sorry` for
proofs, under the names its solution file proves. A theorem with a `sorry` token is not an
endpoint when another declaration of its name has none, and wherever a name is looked up
(a goal, a map entry, a command's argument) the declaration without `sorry` is meant.

* **Reach.** The declarations a chain of references leads to from a root.
* **Chain.** One shortest chain from a root to a declaration: a path, not a reading order,
  and not a proof that each step is needed.
* **Dominators.** A declaration's parent in the dominator tree is the nearest declaration
  that every chain from the roots passes through. The size of its subtree is what the roots
  reach *only* through it. What several branches share hangs under the point where they meet.

On the RH archive (8.1 M edges) reach and dominators take 0.4 s.

## The views

| Key | Command line | View |
|---|---|---|
| `H` | | **Start here**: the endpoints with their statements, who chose them, and the ways in. A project without a map opens on it. |
| `D` | `slim contents DIR [NAME] [--depth N] [--limit N]` | **Contents by dependence**: the dominator tree, largest subtree first. `Space`/`→` unfolds, `←` goes up, `Enter` opens. A theorem whose whole proof is one name is shown with it (`allChi = everyCharacter`). |
| `N` | `slim ledger DIR [--namespace NS] [--limit N]` | **Named statements**: every definition of sort `Prop`, the theorem whose type is exactly that name (`by …`; `⊢` on the command line), how many declarations refer to it, and its depth under the endpoints (`·`: not reached). `p` opens the proof, `f` restricts to the namespace of the selected row (again: wider), `F` clears. |
| `W` | `slim why DIR NAME` | **Why is this here**: a shortest chain from an endpoint to the current declaration, the declarations every chain passes through, and how much is reached only through it. |

All take `--goal NAME` on the command line (repeatable) in place of the pinned goal.
Captured screens: `docs/screenshots/fm-1-start.txt` to `fm-6-search.txt`
(`scripts/capture-screens.sh --fm FORMAL_MATH_DIR`).

In the **declaration view** a line under the declaration's place says where it stands:

```
[ref] LFunction_ne_zero_of_half_lt_re → allChi → … → Comparison₂.formB → Deduction.zeroFree_of_rpow_window · step 10 · 87 reached only through it · W why
```

In the **goal explorer** (`A`) the same numbers stand on the right of each row and order the
rows, heaviest first (except in a curated argument, whose order is its author's); a relay
shows what it hands over to.

**Used by** (tab `2`) falls back to the graph when the project's artifacts are too large to
read in one go (the limit is 96 MiB; the RH archive has 3.9 GiB), provided the graph's edges
are Lean's.

## The contract view

`u` in the declaration view unfolds the project's names in the statement, one level per
press; `U` folds one back. From the command line: `slim contract DIR NAME [--depth N]`.

```
theorem Deduction.zeroFree_of_rpow_window  …/LandauTheorems/ZeroFreeOfRpowWindow.lean:11
  given (F : CharFamily)
        │ structure CharFamily where  (…/DedekindLFunctions/CharFamily.lean:24)
        │   ι : Type
        │   …
        {θ δ C : ℝ}
        (hθ : 0 < θ)
        (h : ∀ Y₀ : ℝ, ∃ Y : ℝ, Y₀ ≤ Y ∧ ∀ y ∈ Set.Icc Y (Y ^ (1 + δ)), Gm F y ≤ C * y ^ θ)
        │ def Gm (F : CharFamily) (y : ℝ) : ℝ := ∑ i, F.a i * (Gchi (F.χ i) y).re  (…)
  then  ZeroFree F θ
        │ def ZeroFree (F : CharFamily) (θ : ℝ) : Prop :=  (…)
        │   ∀ i, ∀ s : ℂ, θ < s.re → DirichletCharacter.LFunction (F.χ i) s ≠ 0
```

* The header is split at its top-level binder groups and the first top-level colon: *given*
  each group, *then* what follows (*is*, for a definition). Implicit and instance groups are
  dimmed.
* Under each part stand the definitions of the project names written in it, as they are
  written (bounded at 14 lines each), each on a rail with the file and line it comes from.
  A name unfolded further up is referred to, not repeated; what the depth or the budget (40
  definitions) leaves folded is listed.
* Unfolding follows *what a declaration states*: the statement of a theorem, all of a
  definition, structure or inductive type. Fields and constructors come with their
  structure. Library names are never unfolded.
* Which name a word stands for comes from Lean's references when the build artifacts are of
  the text (the last line says so), else from the lexical lookup.

`slim dossier DIR NAME [-o FILE.md] [--depth N] [--from BASE_URL]` writes a page about one
declaration in Markdown: the contract (three levels by default), the chain from an endpoint,
what its proof refers to with their statements, and what refers to it. `--from` is where the
files can be read online (`https://github.com/ORG/REPO/blob/REVISION`); places become links.
It is a page to send to someone who will never install anything.

## Search

* **Statements are searched as text** in a source-only index too (scanner version 4): the
  header of each declaration without its keyword and name, up to 2,000 bytes. A library
  name is a way in: `riemannZeta` occurs in 159 of the RH archive's 175,187 files.
* **Rank.** With a graph, matches of one tier are ordered by reach (reached from the
  endpoints first, nearer first), by how many declarations refer to them, and named
  statements before others. The tiers themselves (exact, prefix, …) are unchanged.
* **Reach.** Matches the endpoints do not reach are dimmed and marked `·`. `C` cycles all /
  reached / not reached (`--reached`, `--unreached` on the command line).
* **Components.** A line above the list counts the matches by component: the top-level
  directories, with any that holds more than two fifths of everything divided again.
* **No match.** A query that no name contains says so before listing fuzzy near-spellings.

## Overlays

```sh
slim map import <PROJECT_DIR> --from MODEL.json [-o lean-view.toml]
slim map init <PROJECT_DIR> --landmarks [--top N] [--goal NAME]
slim gloss list|export <PROJECT_DIR> [-o FILE.toml]
slim gloss import <PROJECT_DIR> <FILE.toml> [--replace]
```

* `map import` converts a published reading model into a project map. Known inputs: the
  model of the riemann-rev reading project (`schema: "riemann-rev-model/1"`) and the
  `reading.json` of its proof atlas. Strata and components become groups, annotated nodes
  become landmarks (objectives, for exports), edges become curated relations, itineraries
  become shared trails, and the first routes become an argument for the first objective.
  Every entry is `origin = "imported"` and cites its source; what the model records about
  checking is kept as its words under *limitations*, never as a badge. A short name in the
  model is accepted when it is the end of exactly one indexed name, and reported. The file
  is never overwritten. slim reads a local file: fetching the model is the reader's step.
* `map init --landmarks` appends two generated groups: the declarations with the largest
  dominator subtrees (relays left out), and the theorems the endpoints reach whose statement
  names nothing of the project (they read with the libraries' vocabulary alone), most used
  first. Both are `origin = "generated"`: nobody chose them.
* `gloss export` writes the reader's glosses and trails as map entries; `gloss import` takes
  the `purpose` of each `[[landmark]]` of such a file as a gloss, keeping the reader's own
  unless `--replace`. `,` in the browser glosses the *namespace* of the current declaration
  (what `ZF` stands for); it is shown above every declaration below it.

## Editions

```sh
slim diff <OLD_PROJECT_DIR> <NEW_PROJECT_DIR> [--limit N] [--map FILE]
```

Two source-only indexes compared by full name: unchanged, moved, proof changed with the
statement the same, statement or definition changed, removed, added. Text is compared with
white space collapsed; modules by their Lake names, so an archive unpacked one directory
deeper is the same edition. Glosses, bookmarks and map entries are then sorted into those
that stand and those to read again.

## Smaller changes

* **Imports resolve through Lake's module names.** A package below the opened root, or one
  with a `srcDir`, used to have an empty import graph (`import A.B` did not find
  `lean/src/A/B.lean`). Index names are unchanged (`lean.src.A.B`: maps, trails and
  bookmarks keep working); the Lake name is recorded beside them and shown in the
  declaration view. A file added later repoints the `import` lines that meant it.
* **Local names are not mentions.** A hypothesis `key` or a `have main` is no longer listed
  as the declaration `key` or `main`; `_root_.A.b` is read as `A.b`.
* **A directory skim starts with the files that say what it is about**: `Statement*.lean`,
  then `Defs`, `Definitions`, `Basic`, then the rest by name.

## Measured (2026-11-21, the Riemann hypothesis archive, 4 threads, warm page cache, one run each)

| | |
|---|---|
| source index, scanner 4 | 14.7 s; 583 MiB (377 MiB before statements were made searchable) |
| its `.ilean` files, from the community's published build cache | 5.5 GB in an artifact directory |
| `slim graph build` from Lean references | 8.1 s; 8,126,029 edges; 80 MiB file; 682 MiB peak |
| `slim graph build --lexical` | 7.9 s; 5.7 M edges |
| files with usable `.ilean` | 175,170 of 175,189 (17 rejected: declarations not where the `.ilean` says; 2 outside the package) |
| reached from the five endpoints | 835,670 of 1,133,440 declarations; 640,075 of 818,756 theorems (the community's kernel export: 647,902) |
| reach and dominators | 0.4 s |
| browser with the graph loaded | 586 MiB resident |
| name search | 1.5 ms |

On `formal-math/vp-vnp` (324 files, 5,397 declarations, not built, 2026-11-23): scan and
lexical graph together in under a second; 22,604 edges; 4,396 declarations reached from
the five theorems of `Solution.lean`.

Not measured: frame times of the new views; a second project of the first one's size; the
lexical fallback against Lean's references file by file.

## Limits

* The reference graph is not the kernel's. Instances, coercions and `simp` lemmas that Lean
  inserted without their being written are missing, so reach is a lower bound (on RH, 98.8 %
  of the theorems the kernel export reaches). Only 344 of 8,199 instances are reached.
* A use is given to the innermost indexed declaration whose text holds it. A reference to a
  name the index does not hold (`Foo.mk`, `foo._proof_1`) is given to the declaration it
  belongs to; 835 on RH could not be placed and were left out.
* With several roots, what all of them use hangs under none of them: the top of the contents
  lists it beside the roots.
* The relay test is textual (`:= name`); a proof that is one application with arguments is
  not a relay. Reading an application as a rule ("by `f`, from `a`, `b`, `c`") is not done:
  the goal explorer lists what a proof names, heaviest first.
* The contract splits headers lexically. A binder group that is not bracketed, a `where`
  structure header and a pattern-matching definition are shown as written.
* `diff` needs both editions scanned, and compares text: a renamed bound variable is a change.
* The reach filter, rank and weights follow the project's endpoints, not a goal pinned in the
  browser; the reading views and the position line follow the pinned goal.
* Lemmas used only inside their own file are not folded in skims.
