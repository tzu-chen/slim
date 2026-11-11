# slim user guide

The [README](../README.md) covers building slim and starting a demo. This guide has the rest:
guided sessions, what each view shows, your own projects, what slim runs and what it never
runs, supported toolchains and every command. Paths are relative to the repository root.
Captured screens of everything described here are in `docs/screenshots/` (plain text, produced
by `scripts/capture-screens.sh`).

Contents: [a first session on YM](#a-first-session-on-a-real-project-ym) ·
[the bundled demo](#the-bundled-demo) · [the demo's map](#the-demos-map) ·
[the big picture](#the-big-picture-in-a-source-only-project) ·
[your own projects](#your-own-projects) · [colours](#syntax-and-semantic-colours) ·
[trust](#trust-and-execution-boundaries) · [toolchains](#supported-lean-toolchains) ·
[commands](#commands) · [other documents](#other-documents)

## What slim is for

slim is for reading: find a theorem, read its complete statement, documentation and source,
follow what it depends on and what depends on it, see *why* each edge exists, inspect
assumptions with evidence paths, and come back without losing your place. Editing and
developing proofs stays in VS Code and its Lean extension.

slim is not a proof checker. It never claims a theorem is "proved"; it shows evidence and
where that evidence came from.

For understanding the **architecture** of a formalization there are three more views
(`docs/map.md`): an architecture map of chosen main results, mathematical components and
landmarks (`M`), a goal explorer that unfolds what a pinned theorem rests on while keeping
curated explanations, lexical mentions and Lean-derived dependencies apart (`A`), and reading
trails with notes, questions and reading states (`T`).

For a **first look at a large project** without a map or a Lean build (`docs/overview.md`):
`S` skims a file, directory, namespace or map group as statements with no proofs; `o` turns
the outline's right column into theorems, proof lines, `sorry` tokens or last-modified and
sorts by it; `I` shows files by import depth with the endpoints and hubs of the import order;
`f` searches within the selected directory or the current namespace.

## A first session on a real project: YM

This session uses the public [YM repository](https://github.com/yang-mills-group/YM), which collects pure
Yang–Mills submissions in Lean 4 together with a group's independent checks and assessments
(9,564 Lean files), and the project map for it that ships with slim,
`examples/YM/lean-view.toml`.

```sh
git clone https://github.com/yang-mills-group/YM.git      # about 160 MB; nothing is built
slim YM --map examples/YM/lean-view.toml                  # from slim's checkout: ./target/release/slim …
```

The first start scans the `.lean` files as text (about a second on two cores) and writes a
source-only index to `~/.cache/slim`, never into the checkout. It runs no Lean and no project
code. The title bar says `SOURCE-ONLY (lexical, not elaborated)`: declarations are candidates
found by a scanner, and there are no Lean-derived dependency edges in this mode.

**About this map.** A project map is a plain TOML file that names main results, groups
declarations into components and explains routes through them (`docs/map.md`). The YM map was
drafted by an AI assistant from the repository's own documents and has not been accepted by
anyone: every entry is marked `origin = "ai-suggestion"`, and slim shows that on each one
(`[suggestion]`, "AI suggestion (not accepted)"). It is a reading aid and an example of the
format, not a statement about the mathematics. It was written against revision `55ba9fc`
(`git -C YM checkout 55ba9fc` if the repository has moved on and names no longer resolve).

1. **The map.** slim opens on it: three main results, the mathematical components, and two
   *unresolved references*. Those two are honest: `YMReview.Acceptance.combined_acceptance` and
   `YMReview.OPE.constructed` are each declared twice in the repository (the checked solution
   and a reference statement), and a source-only index cannot tell which one Lean means.
   Press `j` twice to select *Submitted joint continuum construction*; the right pane shows its
   purpose, an explanation, and the statement as written in the source.

   (This screen: `docs/screenshots/ym-1-map.txt`.)

2. **Pin a goal and unfold its argument.** `*` pins the selected theorem as the goal, `A`
   opens the goal explorer on the map's curated argument for it. `j` moves through the steps;
   the right pane keeps the path from the goal and says why each edge is shown. `Space`
   unfolds a step. Every row is tagged `[cur]`: this is the map's explanation, not a
   dependency computed by Lean. (`r` switches to source mentions, `[lex]`; Lean dependencies
   need a semantic snapshot and say so here.)

   (This screen: `docs/screenshots/ym-2-goal.txt`.)

3. **Read a declaration.** `Enter` opens the selected step in the declaration browser:
   statement, documentation, evidence and the source excerpt. `s` shows the whole file with
   syntax colours, `Esc` returns. Tab `1` (Mentions) lists identifiers in its text that match
   indexed names; moving through that list highlights where each one is written.
   `Backspace` goes back to the goal explorer, and keeps going back through everything you
   visited.

4. **Skim a component.** `M` returns to the map. Move to *Independent review: combined
   acceptance (A7)* (`j` four times) and press `S`: the 42 landmark statements of that group
   and its subgroups, each with the lead of its documentation and the number of proof lines
   folded away. `j`/`k` move by statement, `Enter` opens one, `Esc` closes the skim.
   `S` works the same on a file or a directory.

5. **Follow a reading trail.** `T` lists five trails shipped with the map. Select the first,
   *First reading: statements, one witness, scope*, and press `Enter`: slim copies it
   into your own data, where `s` marks a stop read, understood or to revisit, `n` adds your
   note and `E` exports the trail as Markdown. Your trails, bookmarks (`m`) and pinned goal
   are kept in `~/.local/share/slim`, outside the project, and are there when you come back.

6. **Search.** `/` then `combined_acceptance`: the results list the map's entries for that name
   and both declarations of it. `Esc` leaves the search box and `j`/`k` move over the results;
   the line under the title shows the file of each. The one under `checks/` is the solution;
   the one under `verification/` is the reference statement, and slim flags the `sorry` token
   in its text. With a directory selected in the outline (`O`), `f` restricts the search to it
   and `F` lifts the restriction.

7. **Sizes and import order.** `O` shows the files; `o` switches the right-hand column between
   declarations, theorems, proof lines, `sorry` tokens and last-modified, and sorts by it.
   The import view (`I`) needs module names that match the `import` lines, so for YM open the
   Lake package itself rather than the repository root:

   ```sh
   slim YM/submissions/openai-20261028/submission
   ```

   Then `I` lists the directories from the bottom of the import order to the top (9,477 files,
   deepest chain 519), the endpoints and the hubs, and `p` on a file explains why it imports a
   given module. This is build order read from import lines, not mathematical dependency.

`?` lists every key, `:` opens the command palette, `q` quits. To check a map without the
interface: `slim map check YM --map examples/YM/lean-view.toml`.

For a semantic index of a project you have built (Lean's own dependency edges and axiom paths),
see "Your own projects" below. To write a map for your own project, start from
`slim map init DIR` and `docs/map.md`.

## The bundled demo

`slim demo` opens a miniature library whose semantic snapshot (real output of Lean 4.33.1) is
embedded in the binary: it shows what a source-only session cannot, namely Lean-derived
dependencies and axiom evidence, with **no Lean installation, no network and no Lean
compilation**.

```sh
./target/release/slim demo
```

The demo ships a project map (`fixtures/demo/lean-view.toml`), so it opens on the architecture
map; the map tour is in the next section. For the declaration browser, in order (each step takes
seconds):

1. **Search.** Press `/`: the cursor is in the search box. Type `reach` (names), or `decision procedure`
   (words from documentation). Exact qualified names rank first, then exact names, prefixes, word
   prefixes, substrings and fuzzy matches; the line under the box says how many matched and what
   the active filters hid.
2. **Read.** Clear the box (`Ctrl-U`), type `searchWithin_correct`, press `Enter`. The centre shows
   the full elaborated statement (wrapped, never truncated; `i` hides implicit arguments), the
   documentation, the evidence panel and the source excerpt (`s` shows the whole file).
3. **Follow a dependency.** Press `1` (Uses). Rows are tagged **T** (occurs in the statement),
   **V** (occurs in the proof term/body) or **C** (constructor). The footer explains the selected
   arrow. Select `Demo.Graph.Reach` with `j`/`k` and press `Enter`.
4. **Follow a dependent.** Press `2` (Used by), pick a row, `Enter`. The breadcrumb line shows the
   module › namespace › name and the route you took.
5. **Inspect an assumption.** Go back with `[` until you are at `searchWithin_correct`, press `3`
   (Axioms): `sorryAx` is reached via `searchWithin_complete` (a deliberately unfinished *test
   fixture*), although the theorem's own text contains no `sorry`. Each step is a recorded edge;
   the last line confirms the set equals Lean's own `collectAxioms` report. Try also
   `reach_iff_bounded_search` (custom axiom) and `sample_search_native` (compiler trust).
6. **Return and bookmark.** `[` / `]` move through history (restoring panes, query and scroll).
   `m` bookmarks the current declaration, `a` adds it to your reading trail with a note, `B`
   lists bookmarks, `T` opens your trails, `E` exports a trail as Markdown. They are kept outside
   the project and survive restarts (your last position is restored too).

## The demo's map

`slim demo` opens on the **architecture map**: main results chosen by the map's author,
mathematical components with landmark declarations, external foundations, and unresolved
references (the demo has one on purpose). The goal explorer of step 2 looks like
`docs/screenshots/m2-3-goal-curated.txt`.

1. `j` to the first main result. The detail pane shows its purpose, the formal statement, where
   the map places it, and evidence with its origin (authored, lexical, Lean snapshot).
2. `*` pins it as the **goal**, `A` opens the **goal explorer** on the curated argument. `Space`
   unfolds a step; `Enter` opens a declaration in the browser, `Backspace` returns.
3. `r` switches the relation kind: source mentions `[lex]`, Lean dependencies `[T]`/`[V]`, or all
   of them with every edge labelled. `u` flips the direction (uses / used by). Hidden external
   and generated declarations are counted; `H` shows them.
4. `c` shows a **connection** from the goal to another declaration: a bounded search that
   answers with a path, "no path in this data", "truncated" or "unavailable".
5. `a` adds the selected declaration to your **reading trail** with a note. `T` lists your trails
   and the shared trail shipped with the map (`Enter` starts your copy); `s` marks a stop read,
   understood or to revisit; `E` exports the trail as Markdown with stale or unresolved stops
   called out.

Everything here works in source-only mode except the Lean relation kind, which says that it is
unavailable. Without a map, `M` shows the generated file and namespace outline, labelled as
generated organization. To start a map for your own project:

```sh
slim map init ~/path/to/project -o ~/path/to/project/lean-view.toml   # never overwrites
slim map check ~/path/to/project                                      # field errors, unresolved names
```

The map is a plain TOML file that slim reads and never rewrites; `docs/map.md` documents it.

## The big picture in a source-only project

No map, no snapshot, no Lean: these work from the source-only index alone (`docs/overview.md`).

1. A project without a map opens on the **outline** in the left pane (`O` returns to it from
   anywhere, `/` searches). `o` cycles its right-hand column and order: declarations, theorems,
   proof lines, `sorry` tokens, last modified. A directory shows the sum over its files.
2. `S` on a directory or file **skims** it: each declaration's statement and the lead of its
   documentation, proofs left out, with how many lines were folded. `j`/`k` move by statement,
   `Enter` opens one, `S` switches to its namespace, `Esc` closes.
3. `I` shows the **import layers**: directories from the bottom of the import order to the top,
   endpoints (files with declarations that nothing declaring imports), hubs, and with `p` a
   chain of import lines that explains why a file imports a module. These are build
   dependencies read from `import` lines, labelled as such.
4. `f` **searches within** the selected directory or the current namespace (again: one level
   wider; `F`: everywhere). An empty query lists the scope.
5. In the Mentions tab (`1`), moving through the list highlights where the selected identifier
   is written, in the declaration view and in the whole file.

Counts are sizes, not progress: there is no percent proved.

`?` shows every key and what every arrow and badge means; `:` opens the command palette; `q`
quits. `O` shows the current declaration in the module outline, and Enter on a module in the
Module tab (`4`) expands and selects it there. The mouse works too (click selects, a second click
opens, tabs are clickable, the wheel scrolls the pane under the pointer; Shift+drag still selects
text in most terminals; `--no-mouse` turns it off). Holding `j`/`k` only moves the highlight; the
declaration under the cursor loads when you pause.

Colours follow lean-lang.org's light and dark palettes
(`docs/screenshots/theme-lean-dark.png`, `theme-lean-light.png`). The default `--theme lean`
paints its own background. It is dark unless `COLORFGBG` says the terminal is light; choose with
`--theme lean-light|lean-dark` or `SLIM_THEME`. `--theme dark|light|contrast|mono` keep the
terminal's own colours. `--ascii` and `NO_COLOR` are supported.

## Your own projects

```sh
slim ~/path/to/project        # browse (source-only until you build a semantic index)
slim scan ~/path/to/project   # build/refresh the source-only index without the UI
slim info ~/path/to/project   # provenance, coverage, freshness
slim search ~/path/to/project normal subgroup
```

**Source-only mode** (the default for any directory) reads `*.lean` files as text: a conservative
lexer (nested comments, strings incl. raw and interpolated, Unicode identifiers as each Lean
version defines them, `«quoted»` names) finds imports, namespaces, doc comments, attributes and
declaration candidates. It never evaluates `lakefile.lean`, runs Lake, macros or project code. It
draws **no theorem dependencies**; the "Mentions" tab is an explicitly labelled lexical guess. An
absent `sorry` token is reported as proving nothing. Large trees work: the CFSG checkout
(229,089 files, 2.06 GiB, 2.36 M candidates) indexes in about 19 s on two cores and reopens in a
few milliseconds (see `docs/benchmarks.md`).

**Semantic mode** comes from a snapshot exported from a *built* project by slim's small,
version-pinned Lean exporter:

```sh
lake build                                      # you build the project yourself, if you trust it
slim export ~/path/to/project --import      # prints exactly what will run, asks you to type yes
slim ~/path/to/project                      # now opens the semantic index
```

or from inside the browser: `:` → "Semantic export…". The snapshot records the Lean version,
exporter version and compatibility shim, project identity, git revision, `lake-manifest.json`
hash, SHA-256 of every local source file, extraction scope, sandbox and limits.

Snapshots can be imported elsewhere: `slim import SNAPSHOT.jsonl --source-root DIR` (the
directory you approve for showing source; snapshot paths can never leave it).

## Reading the labels

The screens say where a fact comes from in a few words and leave the explanation to this
section and to the help overlay (`?`).

* **lexical** (`[lex]`, the Mentions tab, source-only statements, `sorry` tokens, import lines):
  read from the text of the files without running Lean. A mention is an identifier that equals
  an indexed name; open namespaces, local variables, notation and macros are ignored, so Lean
  may resolve it to something else, or to nothing. Mentions run outward only: there is no
  "mentioned by" (text search with `/` is the way to look for one). A source-only index has no
  dependency edges at all; the Used by, Axioms and Lean-dependency views need a semantic
  snapshot (`slim export`, see above).
* **curated** (`[cur]`, the map): written by a person in `lean-view.toml` and not checked by
  Lean. Missing annotations are normal. Objectives are never inferred, and a name the index
  cannot resolve is never re-attached to a similar one: edit the file and press `r` in the map.
  `slim map init <PROJECT_DIR>` prints a starting file derived from the directories.
* **Lean-derived** (`[T]` statement, `[V]` proof term or body, `[C]` constructor): constants in
  the elaborated terms of a semantic snapshot. Proof terms include what automation chose, so
  these edges are not a reading order, and "coverage incomplete" means missing edges are not
  evidence of no dependency.
* **import layers**: build order, not mathematics. Module names are paths from the project
  root, so in a source-only index the files of a Lake `srcDir` show as external unless slim is
  opened on that directory.
* **skim**: statements without proofs. In a source-only index a statement is its header text
  as written.

## Syntax and semantic colours

Source is coloured as in VS Code with the Lean extension: the extension's own TextMate
grammar, run by a port of VS Code's tokenizer and resolved against VS Code's default themes
(Dark+, Light+, high contrast, or your own theme file). This needs only the text: it works in
every mode, for every file, and starts nothing. On 4.2 million lines of real Lean (all of
HaagerupIzumi, all of its Mathlib, a sample of CFSG) it gives the same scope stack and the
same colour as VS Code's tokenizer for every byte (`docs/highlighting.md`).

A captured screen is in `docs/screenshots/highlight-1-source.txt`, and with its colours in
`docs/screenshots/highlight-source-lean-dark.png`: `fixture_trust_me` on line 55, a tactic the
demo library defines, is coloured as a keyword only because Lean said so.

On top of that, slim can show **Lean's own semantic tokens**: keywords that the project's
notation and macros introduce, local variables, projections. Those come from the project's
pinned Lean language server, so extracting them is an explicit job:

```sh
slim highlight build ~/path/to/project    # prints exactly what will run, asks you to type yes
slim highlight status ~/path/to/project   # what is cached and how fresh it is; runs nothing
```

or `:` → "Semantic highlighting: build index…" in the browser, where it runs in the
background while you read, shows its progress in the title bar and can be paused or
cancelled. It never builds, fetches or installs anything; a file whose imports are not built
is recorded as failed, with the reason. The tokens are cached per file; afterwards browsing
reads the cache and starts no Lean process. `F4` switches between syntax + semantic, syntax
only and off, `F5` changes the syntax theme, and `F6` says, for the file on screen, each
thing separately: whether the cached tokens belong to exactly these bytes, how much of the
file the server processed, whether the toolchain and configuration are still the recorded
ones, whether an imported module changed since, and what was known about the build.
Outdated semantic colours are not shown unless you ask for the labelled stale preview.

## Trust and execution boundaries

* **Browsing is read-only and offline.** Opening a demo, a project directory or an index reads
  files and a local SQLite index; it starts no Lean, Lake, LSP, editor, hook or network access.
  Indexes and reading data live in `~/.cache/slim` and `~/.local/share/slim`
  (override: `--cache-dir`, `SLIM_CACHE_DIR`, `SLIM_DATA_DIR`), never in the project.
* **The project map is only read.** `lean-view.toml` is parsed as data (nothing in it is
  executed) and never rewritten; a malformed file is reported and left as it is. The one command
  that writes a map, `slim map init -o FILE`, refuses to overwrite. No model, API key or
  network is involved in the map, the goal explorer or trails.
* **`highlight build` runs the project's Lean language server**, only after you type `yes`
  (in the shell or in the browser), and trust is not remembered. The server elaborates the
  files, which runs the project's macros, tactics and elaborators. It runs in the same kind
  of bubblewrap sandbox (no network, project and toolchain read-only, nothing writable),
  under a cgroup memory limit and a time limit per file, with the toolchain named in
  `lean-toolchain` as installed, and with builds switched off (`dependencyBuildMode: never`).
  Cancelling or quitting stops the whole process tree and keeps the finished files.
* **Only `export` and `highlight build` run anything**, only after you type `yes`, and trust is not remembered.
  It builds slim's own exporter (embedded in the binary) with the project's exact, already
  installed toolchain, then runs it on the project's *existing* `.olean` files. slim never
  builds your project, runs `lake update`, fetches caches or installs toolchains.
* Loading `.olean` files means trusting them (they are memory images), and the default
  `--pp notation` mode also runs the imported modules' initializers and attribute handlers so that
  types print with notation. So the exporter runs in a **bubblewrap sandbox**: no network,
  read-only project and toolchain, private `/tmp`, cleared environment, only a fresh per-run
  output directory writable; plus a cgroup memory/task limit (systemd user scope), CPU time,
  output size, wall clock, no core dumps, and `PR_SET_PDEATHSIG`. If no sandbox is available the
  export is refused; `--sandbox none --trust-project` is an explicit per-run decision, never a
  fallback. `--pp raw` (Lean ≥ 4.21) runs no project code at all, at the cost of printing types
  without library notation. One export runs at a time.
* Snapshots are untrusted data: framing, sizes, names and paths are validated; graph walks are
  iterative and bounded; text is sanitised before display (escape sequences, bidi controls).
* On quit, `SIGTERM`/`SIGHUP`/`SIGINT` or panic, slim restores the terminal, cancels
  background work and kills and reaps its child process groups (tested in a pseudo-terminal).

A successful build, an exported snapshot, or an empty axiom list is **not** an independent kernel
replay, and says nothing about whether an informal mathematical statement was formalised
faithfully.

## Supported Lean toolchains

| | Versions |
|---|---|
| Browsing snapshots and source-only mode | any (snapshot format is slim's own; the lexer follows Lean's identifier rules of 4.16, 4.21+ and 4.28+, chosen from `lean-toolchain`) |
| Semantic exporter, **tested** end to end on the fixture | `v4.16.0`, `v4.21.0`, `v4.28.0`, `v4.33.1`, `v4.35.0` |
| Exporter, compiled but untested | other 4.16–4.x versions via the same shims, only with `--allow-untested-toolchain` |
| Syntax colouring | any (one grammar: vscode-lean4 0.0.240) |
| Semantic-highlight indexing, **tested** end to end on the fixture | `v4.16.0`, `v4.21.0`, `v4.28.0`, `v4.33.1`, `v4.35.0` (`scripts/test-highlight-toolchains.sh`); others from 4.16 on only with `--allow-untested-toolchain` |

For every tested version the exporter was built, run in the sandbox on the fixture built with
that version, and slim's transitive axiom sets were compared with Lean's own `collectAxioms`
for every local declaration: identical on 4.28.0, 4.33.1, 4.35.0; on 4.16.0/4.21.0 identical up
to Lean's documented 4.28 change (older `#print axioms` does not follow axiom *types*), which
`slim check-axioms` verifies explicitly. CFSG pins `v4.35.0-rc3`, which is not installed here
and not in the tested list.

## Commands

| | |
|---|---|
| `slim demo` | the bundled demo |
| `slim [open] DIR` / `--index FILE [--source-root DIR]` | browse |
| `slim scan DIR [--threads N] [--force]` | source-only index (incremental when it exists) |
| `slim export DIR [--import] [--out F] [--pp notation\|raw] [--library full\|frontier\|none] [--timeout S] [--memory-mib M]` | semantic snapshot (explicit) |
| `slim import SNAP [--source-root DIR \| --index FILE] [--allow-partial]` | transactional, resumable import |
| `slim info DIR` · `search DIR WORDS [--within FILES \| --namespace NS]` · `check-axioms --index FILE` | inspection |
| `slim skim DIR MODULE_OR_DIR [--namespace NS] [--group ID]` | statements without proofs, as text |
| `slim imports DIR [FROM TO] [--limit N]` | import order: layers, endpoints, hubs; or one import path |
| `slim map init DIR [-o FILE]` · `map check DIR [--map FILE] [--fingerprints]` | project map: starting file, validation |
| `slim trail list\|export [DIR] [--trail ID\|TITLE] [-o FILE.md]` · `bookmarks` | reading data |
| `slim highlight build DIR [--yes] [--files PATH]… [--budget S] [--file-timeout S] [--memory-mib M] [--retry] [--refresh]` | semantic-highlight indexing (explicit, resumable) |
| `slim highlight status DIR [--failures]` · `highlight clean DIR [--baseline] [--semantic]` | cache contents and freshness · maintenance |
| `slim export DIR --with-highlight` | export, then the highlight indexing, under one confirmation |
| `slim bench [--full] [--report F]` · `bench --index FILE` · `highlight bench DIR` | measurements |

Tests: `cargo test --release` (unit, fixture, lifecycle and pseudo-terminal tests; the
highlighting tests use a stand-in language server and need `python3`, and one runs the real
server when Lean 4.33.1 and a built `fixtures/demo` are present).

## Other documents

* `docs/highlighting.md` — syntax and semantic colours: use, design, the protocol, the cache,
  freshness, parity with VS Code and its limits. `docs/highlight-phase0.md` — what was
  inspected and decided first; `docs/bench/highlight.md` — measurements on HaagerupIzumi and
  CFSG; `THIRD-PARTY-NOTICES.md` — bundled grammars and themes, licences.
* `examples/YM/lean-view.toml` — a project map for the public YM repository, used by the
  walkthrough above (an unaccepted AI-drafted map: an example of the format).
* `docs/overview.md` — skim, outline counts, import layers, search within, the Mentions
  highlight: what each number means, limits, measurements; index format 2.
* `docs/map.md` — the architecture map, the goal explorer, reading trails and `lean-view.toml`.
* `docs/design.md` — reconnaissance, architecture, data formats, edge semantics, decisions.
* `docs/limitations.md` — what is approximate, missing or untested.
* `docs/benchmarks.md` — measured performance with method and caveats.
* `docs/handoff.md`, `docs/handoff-milestone-2.md` — milestone status: what runs, tests
  executed, gaps, next steps. `docs/milestone-2-notes.md` — decisions and modules of milestone 2.
* `fixtures/demo/README.md` — the demo library and its labelled test fixtures.
