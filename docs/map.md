# The project map, the goal explorer and reading trails

slim's milestone 2 adds three views on top of the declaration browser. They share
declaration identities (fully qualified Lean names) and never blur where a fact comes from.

| Key | View | What it is |
|---|---|---|
| `M` | Architecture map | chosen main results, named groups, landmark declarations |
| `A` | Goal explorer | what a pinned goal rests on, unfolded step by step |
| `T` | Reading trails | your ordered stops, notes, questions and reading states |
| `/` `O` `B` | Declaration browser | search, outline and bookmarks, as in milestone 1 |

A project that ships a map opens on it; one without opens on the outline. `Esc` leaves a view,
`Backspace` or `[` goes back through history (views are history steps), and `?` lists every key.
An action that cannot work in the current session (for example Lean dependencies in a
source-only index) says so when it is asked for. While the cursor is in the search box, letters
are text: press `Esc` first, or use the command palette (`:`).

Try it on the bundled demo (`slim demo`); nothing needs Lean, a build or a network:

1. The map lists two **main results**. `j` selects *Soundness of the fuel-bounded search*; the
   detail pane shows its purpose, formal statement, placements and evidence.
2. `*` pins it as the **goal**; `A` opens the goal explorer on the **curated argument** (rows
   tagged `[cur 1]`, `[cur 2]`, …). `Space` unfolds a step; the detail pane keeps the path from
   the goal and says why each edge is shown.
3. `r` changes the relation kind: source mentions `[lex]`, then **Lean dependencies** `[T]`/`[V]`.
   The Lean view shows that the theorem's proof term goes through a Lean-generated auxiliary
   (hidden by default and counted; `H` shows everything).
4. `c` asks for a **connection** from the goal to another declaration, e.g.
   `Demo.Rel.ReflTransGen.trans`.
5. `a` adds the selected declaration to your trail with a note; `T` shows your trails and the
   shared trail shipped with the map (`Enter` starts your own copy). `s` cycles a stop's state,
   `E` exports the trail as Markdown.

Screens of each step: `docs/screenshots/m2-*.txt`.

## What each kind of evidence means

| Tag | Kind | Source | What it is not |
|---|---|---|---|
| `[cur]`, `[cur 3]` | curated relation / step 3 of a curated argument | `lean-view.toml`, written by a person (or labelled as a suggestion) | a Lean-derived dependency |
| `[lex]` | source mention | an identifier in the declaration's text equals an indexed name | resolved by Lean (opens, notation, dot notation and macros are ignored) |
| `[T]` `[V]` `[C]` | Lean dependency | constants in the elaborated statement / proof term or body / constructor list, from a semantic snapshot | a reading order, or a proof of necessity |

The direction is fixed and displayed: **"A uses B"** runs from a statement A to its prerequisite
B, so unfolding A lists B; **"used by"** is the reverse (`u` switches). The relation kind is
always visible in the header of the goal explorer. The mixed view (`r` until "All kinds") shows
one row per declaration with every label it has. No view is called "proof dependencies".

Unfolding is lazy and finite. A declaration already on the path from the goal is shown as a
back-reference (`↺ cycle`) and not unfolded again. A declaration that merely appears elsewhere in
the outline is marked `↗ again` and can still be unfolded. Lean edges come 50 at a time with a
"more" row. Filters (`L` local only, `G` generated, `y` the part followed: all, what it states, statement only,
proof or body only) hide rows and
say how many; `H` reveals everything. Hiding is a display choice, not a claim of irrelevance.

A **connection** is a shortest path of one relation kind in one direction, found by a search
bounded to 200,000 declarations and 1.5 s (400 declarations for source mentions) that `Esc`
cancels. Its answer is one of: a path; *no path in this data* (everything reachable was
searched); *truncated* (a limit was hit: no conclusion); *unavailable* (for example Lean
dependencies in a source-only index). A path shows connectivity. It is not the best route to
read and does not show that each step is needed.

## `lean-view.toml`

The map is a TOML file named `lean-view.toml` in the project root (or any file given with
`--map FILE`). **slim reads it and never rewrites it**: you edit it in your editor and press
`r` in the map view (or run "Reload the project map" from the palette). A file with a syntax
error, an unknown field or an unsupported `schema` is reported with the offending field, left
exactly as it is, and the rest of slim keeps working. Problems confined to one entry
(duplicate id, unknown parent, …) drop or re-home that entry and are listed under "Map problems".

```sh
slim map init  <PROJECT_DIR> [-o lean-view.toml]   # a starting file; never overwrites
slim map check <PROJECT_DIR> [--map FILE] [--fingerprints]
```

`map init` derives groups from directories and marks every one `origin = "generated"`,
`reviewed = false`: a directory or namespace is not automatically a mathematical chapter. It
never proposes objectives.

All names are fully qualified Lean names. Files, lines and fingerprints are locators, not
identity. `fixtures/demo/lean-view.toml` is a complete example.

```toml
schema = 1                       # required; other versions are refused

[project]
name = "my_package"              # Lake package name (map check warns on a mismatch)
title = "…"                      # optional
description = "…"                # optional
revision = "…"                   # optional: what the map was written against (not checked)

[[source]]                       # an identified paper or document
id = "paper"
citation = "A. Author, Title, 2020"
url = "…"                        # optional

[[objective]]                    # a main result, chosen by a person; never inferred
decl = "Pkg.main_theorem"
title = "…"                      # optional
purpose = "…"                    # optional one or two sentences
explanation = "…"                # optional longer text
paper = [{ text = "Theorem 1.1", source = "paper" }, "Lemma 3.9"]
limitations = ["…"]
origin = "authored"              # authored | generated | ai-suggestion | ai-accepted
source_sha256 = "…"              # optional fingerprint, see "Freshness"

[[group]]
id = "closure"                   # letters, digits, - _ .
title = "…"
parent = "relations"             # optional: nesting
order = 20                       # optional: order among siblings
kind = "component"               # component (default) | foundation (external libraries)
purpose = "…"
explanation = "…"
origin = "authored"
reviewed = true                  # default: true for authored, false otherwise
paper = [ … ]
limitations = ["…"]
landmarks = ["Pkg.a", { decl = "Pkg.b", role = "why it is here" }]
modules = ["Pkg.Closure"]        # optional file locators

[[landmark]]                     # the annotation of one declaration, wherever it is placed
decl = "Pkg.b"
purpose = "…"
explanation = "…"
paper = [ … ]
limitations = ["…"]
origin = "authored"
groups = ["closure", "search"]   # optional extra placements
source_sha256 = "…"

[[relation]]                     # from uses to
from = "Pkg.a"
to = "Pkg.b"
kind = "is defined as"           # free text; default "uses"
note = "…"
origin = "authored"
evidence = "Pkg/Closure.lean"    # optional: where to look

[[argument]]                     # an ordered explanation of one goal
goal = "Pkg.main_theorem"
title = "…"
summary = "…"
steps = [
  { decl = "Pkg.a", why = "…" },
  { decl = "Pkg.c", for = "Pkg.a", why = "…" },   # a sub-step of an earlier step
]

[[trail]]                        # a shared reading trail (a template readers copy)
id = "intro"
title = "…"
description = "…"
stops = [{ decl = "Pkg.a", note = "…", question = "…" }, { group = "closure" }]
```

Notes:

* **One declaration, several placements.** A declaration listed in several groups stays one
  declaration with one annotation. `M` on it cycles through its placements.
* **Paper references keep their literal text.** A `source` must be the id of a `[[source]]`
  entry; otherwise the reference is shown as literal text with no identified source. slim
  also lists fragments such as "Lemma 3.9" or "equation (3.16)" found in a declaration's own
  documentation or comments, as *candidates*: literal text, never attached to a paper.
* **Counts** in the map count curated entries ("43 curated landmarks"). They are not counts of
  verified results, and there is no "percent proved".
* **External foundations.** A `kind = "foundation"` group lists library declarations on purpose.
  They resolve in a semantic snapshot; a source-only index holds only the project's own files and
  says so.

### Unresolved and stale references

A name the open index does not have (renamed, removed, not indexed) or has several times
(source-only indexes) appears under **Unresolved references**, with where the map uses it. For a
missing name slim lists indexed declarations with the same last name component as hints. It
never re-attaches an annotation to a similar-looking declaration: edit the file.

**Freshness.** An annotation may record `source_sha256`: a prefix (16 hex digits are enough) of
the SHA-256 of the declaration's source file when the annotation was written. When the indexed
file no longer matches, the annotation is marked `STALE`. `slim map check --fingerprints`
prints the current values to paste. Without a fingerprint an annotation is never called stale
(and never called fresh either).

### Suggestions from an agent

slim has no model, API key or network dependency. An agent (or a script) can still help by
proposing entries in the file, marked `origin = "ai-suggestion"`. They are shown with a
`[suggestion]` tag and "AI suggestion (not accepted)". A person accepts one by editing it to
`origin = "ai-accepted"` (or `authored` after rewriting it), ideally adding `source_sha256`, so
that the entry keeps its origin and goes stale when its evidence changes; rejecting is deleting
the entry. `slim map check` validates a proposed file before anyone opens it.

## Reading trails, questions and the goal

Personal reading data lives in `~/.local/share/slim/userdata.sqlite` (override
`SLIM_DATA_DIR`), never in the project, keyed by Lake package name and declaration names, so
it survives re-indexing and moving the checkout.

* **Trails**: any number per project. `a` adds the selected declaration or map group to the
  *active* trail (`t` in the trails view chooses it; the first `a` creates "Reading trail").
  Each stop has an explanation (`n`), a question (`Q`), and a state (`s`): unread, read,
  understood, revisit. "Understood" is your annotation, not Lean verification. `-`/`+` reorder,
  `d` removes, `r` resumes at the first stop that is not read, `N`/`e` create and rename.
* **Shared trails** from the map are templates: `Enter` copies one into your data (once).
* **Questions**: `Q` on any declaration or group; the trails view lists them (`s` resolves).
* **Goal**: `*` pins or clears it; it is remembered per project.
* **Stale and unresolved stops** stay in the trail, marked. A stop records the SHA-256 of its
  source file when it was added; a different file later means `STALE`, a name that no longer
  resolves means `UNRESOLVED`. Notes are never dropped.
* **Export**: `E`, or `slim trail export <PROJECT_DIR> [--trail ID|TITLE] [-o FILE.md]`,
  writes Markdown with the project identity, the git commit read from `.git` (slim does not
  run git, and says that uncommitted changes are not detected: the per-stop fingerprints cover
  that), every stop with state, note and question, and stale or unresolved stops called out.
  `slim trail list` lists trails.

Writes are SQLite transactions, so two running slim instances do not interleave half an
update; an action on a trail another instance deleted reports an error. The store's schema
version is checked on open: data written by a newer slim is refused and left untouched.
Milestone-1 trails are migrated into a trail named "Reading trail" on first open; the old table
stays for an older binary.

## Layout and keys

At 80 columns or more the outline and the detail pane are side by side; below that one pane is
shown and `Tab` switches. The line under the title keeps the view and the pinned goal; secondary
text there is dropped first. In the detail pane `w` turns wrapping off and `<` `>` scroll long
statements sideways. Every state shown with colour or a symbol also has a word (`MISSING`,
`STALE`, `lib`, `[generated]`), and `--ascii` replaces the symbols. The mouse is optional: a
click selects, a second click opens, the wheel scrolls.
