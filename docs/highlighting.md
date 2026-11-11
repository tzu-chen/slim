# Syntax and semantic highlighting

slim colours Lean source in two layers:

* **Syntax (baseline).** The Lean 4 TextMate grammar of the VS Code Lean extension, run by a
  port of VS Code's own tokenizer, resolved against VS Code's default themes. It needs only
  the text, is computed while you browse and starts no process.
* **Semantic.** Lean's own semantic tokens (keywords introduced by the project's notation
  and macros, local variables, projections, function names), extracted once by an explicit
  job that runs the project's pinned Lean language server, and then read from a cache. Warm
  browsing never starts Lean.

The source view (`s`) and the source excerpt of the declaration view are coloured. The title
of the source pane says what the colours on screen are: `syntax`, `syntax + semantic
(complete)`, `syntax (semantic tokens stale: inputs changed)`, and so on.

## Using it

| | |
|---|---|
| `F4` | highlighting mode: syntax + semantic (default), syntax, off. Remembered per project. |
| `F5` | syntax theme: follows the UI theme (Dark+ for dark themes, Light+ for light ones, Dark High Contrast for `contrast`), or a fixed bundled theme, or your theme file |
| `F6` | details: settings, what is known about the file on screen, indexing progress |
| `R` | compare sources, environment and imports with the caches again, hashing every file |
| `:` Semantic highlighting: build index… | plan the indexing job, show exactly what would run, ask for `yes` |
| `:` Semantic highlighting: refresh this file… | the same for the file on screen only |
| `:` Semantic highlighting: pause / resume indexing, cancel indexing | |
| `:` Semantic highlighting: stale preview | also show semantic colours whose inputs changed, labelled STALE |

```sh
slim PROJECT --highlight semantic|syntax|off          # also SLIM_HIGHLIGHT
slim PROJECT --syntax-theme dark-plus|light-plus|hc-black|hc-light|THEME.json
slim PROJECT --highlight-memory-mib 128               # the highlight worker's file cache

slim highlight build PROJECT          # the indexing job from the shell: prints the plan, asks for yes
slim highlight build PROJECT --yes --budget 3600 --file-timeout 300 --files Some/Dir
slim highlight status PROJECT [--failures]   # cache contents and freshness; runs nothing
slim highlight clean PROJECT [--baseline] [--semantic]
```

`--syntax-theme FILE.json` takes a VS Code colour theme file (JSON with comments, `include`
followed, `tokenColors` and `semanticTokenColors` used).

### Building the semantic index

The job is a trust decision of its own, separate from `slim export`: it makes Lean
elaborate the project's files, which runs the project's macros, tactics and elaborators.
The plan shown before the `yes` names the toolchain, the command, the sandbox and the
limits. It then:

* uses only the toolchain named in `lean-toolchain`, which must already be installed under
  elan; nothing is installed;
* opens every document with `dependencyBuildMode: "never"`, so Lake is asked for the
  existing build only (`lake setup-file … --no-build --no-cache`); a file whose imports are
  not built or are out of date is recorded as *failed* with that reason. Nothing is built,
  fetched or updated;
* runs the server in bubblewrap with no network, the project and toolchain read-only, a
  private `/tmp`, a cleared environment and nothing writable; without a usable sandbox the
  job is refused unless `--sandbox none --trust-project` is given on the command line;
* keeps one document open at a time (`--open-docs N`, at most 4), under a cgroup memory
  limit (`--memory-mib`, default 6144; `--cgroup none` when an outer resource guard already
  confines slim, also `SLIM_HL_CGROUP=none` for the browser), with a wall-clock
  limit per file (`--file-timeout`, default 300 s) and optionally for the run (`--budget`);
* watches the server's memory as its cgroup accounts it, once a second: a file whose
  elaboration takes the heap to 70 % of the limit is closed (which ends its worker) and
  recorded as failed for that reason, so one file cannot take the server, or the job, down
  with it;
* publishes each file's tokens when that file is done. Cancelling, quitting, a crash or the
  budget running out keeps the finished files; the next run opens only what is missing or
  out of date. Files that failed, timed out or were skipped are not retried while their
  inputs are unchanged, unless `--retry` (or `--refresh` for everything) is given.

While it runs, the title bar shows `semantic index 412/1435 (3 failed, 1 partial) 21:08`;
`F6` shows files attempted, complete, partial, failed, skipped and already current, the
elapsed time, the current file and how long it has been processing, and the resident memory
of the server's process tree. No estimate is shown before 20 files are done, and then only
as "at the rate so far". On cancel the server gets SIGTERM for its process group, SIGKILL
after 1.5 s, and slim checks that no process of its tree is left; only processes
slim started are ever signalled.

A finished file means the server processed all of it; it does **not** mean the file is
free of errors, and it is not a verification of anything. The number of errors Lean
reported is recorded and shown.

`lakefile.lean` projects: Lake evaluates that file when the server starts (the plan says
so), and in the read-only sandbox it can only do so if its compiled configuration under
`.lake` is current. `lakefile.toml` is read as data.

## How it is built

```text
textmate.rs   grammar interpreter      source bytes -> scope stacks        (baseline provider)
baseline.rs   whole-file driver, suspend/resume
lsp.rs        minimal LSP client       framing, requests, owned server process
indexer.rs    the explicit job         documents -> validated semantic tokens (semantic provider)
semantic.rs   LSP units -> bytes       relative tokens, legend, coordinate map
codec.rs      compact span storage     256-line chunks, decoded on demand
store.rs      highlight.sqlite         baseline / semantic / env / run tables
env.rs        environment and imports  fingerprints, import closure, artifacts
service.rs    the browser's worker     bounded file cache, freshness facts
theme.rs      VS Code themes           scopes and token types -> logical styles  (style resolver)
render.rs     merged spans, wrapped styled rows for visible lines              (viewport renderer)
app/hl.rs     settings, the job in the UI, terminal styles
```

Threads: the UI thread keeps one immutable snapshot for the file on screen and resolves
styles for the visible lines; the highlight worker tokenizes, reads the cache and checks
freshness, and blocks when idle; the indexing job has its own thread and its own child
processes. None of this runs before the first frame.

### Baseline: why a port of VS Code's tokenizer

| Candidate | Finding | Decision |
|---|---|---|
| [Giallo](https://github.com/getzola/giallo) 0.6.0 | TextMate engine in Rust, "same output as VS Code", ships grammars including Lean. Licence EUPL-1.2 (copyleft). Its bundled grammar version cannot be pinned to the installed Lean extension. | Not adopted: a **licensing** blocker for an Apache-2.0 program, to be decided by the project owner; not a performance finding. |
| [syntect](https://docs.rs/syntect) | Loads Sublime `.sublime-syntax` definitions, not JSON TextMate grammars; the extension's grammar would have to be converted and the conversion verified. | Not adopted. |
| [tree-sitter-lean](https://github.com/Julian/tree-sitter-lean) | Experimental static grammar with its own node kinds; it would not reproduce the extension's scopes, and cannot know project-defined syntax either. | Not adopted. |
| The existing conservative lexer (`src/source/lexer.rs`) | Finds comments, strings and identifiers for the index; knows no scopes. | Kept for indexing only. |
| A port of [vscode-textmate](https://github.com/microsoft/vscode-textmate) on Oniguruma | The engine VS Code uses, on the regular-expression library VS Code uses (`onig` crate, MIT, bundling Oniguruma 6.9.10, BSD-2-Clause), running the extension's JSON grammars unchanged. | **Adopted** (`src/highlight/textmate.rs`, about 1,000 lines without tests). |

The grammars are `lean4.json` and `lean4-markdown.json` of vscode-lean4 0.0.240 (Apache-2.0),
byte-identical copies in `assets/grammars`. The provider identity
(`textmate-e1/vscode-lean4 0.0.240/<hash of both grammars>`) is stored with cached spans, so a
grammar or engine change invalidates baseline spans and nothing else.

The tokenizer keeps the whole scope stack of every token (interned), handles multi-line
state (nested block comments, documentation with Markdown, strings, `begin`/`while` rules,
end patterns with back-references, captures re-tokenized by nested patterns, `\A`/`\G`
anchors), and can stop after any line and resume: a file is tokenized from its top through
the lines on screen plus 200, shown, and finished in slices of 2,048 lines between which the
worker looks for newer requests. A view that starts inside a comment is therefore right:
its lines were tokenized in order. Completed files of 32 KiB or more are written to the
cache, so reopening a large file reads spans instead of tokenizing.

If the bundled grammar could not be compiled or a file's cached spans are damaged, the lines
are shown plain; that is an error path, not a mode.

### Styles

Themes are VS Code colour theme files. Bundled, unmodified, from VS Code 1.142.0:
Dark+ (`dark_vs` + `dark_plus`, the token colours of Dark Modern), Light+ (`light_vs` +
`light_plus`), Dark High Contrast and Light High Contrast.

* TextMate colours: a port of vscode-textmate's theme matcher (rule order, parent selectors
  and the child combinator, specificity, inheritance of unset properties along the scope
  stack).
* Semantic tokens: VS Code's resolution (`getTokenStyle`): the theme's
  `semanticTokenColors` first; otherwise the default rules, which probe TextMate scopes:
  VS Code's built-in map (`variable` → `variable.other.readwrite`, `property` →
  `variable.other.property`, `function` → `entity.name.function`, …) and the Lean
  extension's contribution `keyword` → `keyword.other`, registered later and therefore
  winning over the built-in `keyword.control`. A semantic style replaces the syntax style
  property by property (colour, bold, italic, underline, strikethrough) and leaves the rest;
  a token type without any rule (`leanSorryLike`) changes nothing, so `sorry` keeps the
  grammar's `invalid.illegal` colour.
* Token types and modifiers are decoded with the legend the server sent, stored with the
  tokens; no numeric meaning is assumed.

Changing the theme creates a new style resolver; cached tokens are not touched and Lean is
not involved.

**Compositing order**, lowest first: the view's base text style; the syntax style of the
token; the semantic style, per property; the line overlay (background of the current
declaration's lines), over which token colours stay. The source view has no selection or
in-file search yet; when they come they belong above the line overlay. Unstyled text keeps
slim's own text colour for the theme (the VS Code editor foreground is not used).

**Terminal approximations** (display limits, not missing information):

* 24-bit colour is used when the terminal advertises it, otherwise the nearest xterm-256
  colour that keeps contrast on the theme's background; the `mono` UI theme shows only bold,
  italic and underline.
* A syntax colour with contrast below 3:1 against the background it sits on is replaced by
  the base text colour. With the default pairs (Dark+ on the dark Lean theme, Light+ on the
  light one) no colour used by the grammar on the fixtures is replaced (tested); it happens
  when a theme is forced onto the wrong background.
* Alpha in theme colours is ignored; fonts, ligatures, bracket-pair colours, inlay hints,
  diagnostics squiggles and editor decorations do not exist here.
* Italic, underline and strikethrough are sent as terminal attributes and depend on the
  terminal.

## The semantic protocol

Checked in the toolchain sources (`Lean/Server/FileWorker/SemanticHighlighting.lean`,
`RequestHandling.lean`, `SetupFile.lean`, `Lean/Data/Lsp/Extra.lean`) of every tested
version and exercised by `scripts/test-highlight-toolchains.sh`:

1. `initialize` offers UTF-16, UTF-8 and UTF-32 positions and reads the answer: the legend
   (`semanticTokensProvider.legend`) and `positionEncoding` (absent means UTF-16, which is
   what Lean uses). No legend or no `full` support ends the job with that message.
2. `textDocument/didOpen` with the exact bytes read from disk (their SHA-256 becomes the
   record's identity), a fresh increasing version, `dependencyBuildMode: "never"`.
3. `textDocument/waitForDiagnostics` for that version. Lean answers when the version has
   been processed.
4. The file counts as processed when that answer has arrived **and** a
   `$/lean/fileProgress` notification with an empty `processing` list has been seen for the
   same document version. Reports for other versions are ignored.
5. Only then `textDocument/semanticTokens/full`. In every tested version this request
   returns the *finished prefix* of the document, not the whole (hence steps 3 and 4); if the
   server sends
   `workspace/semanticTokens/refresh` or reports processing again after the request went
   out, the answer is discarded and the request repeated (at most three times).
6. The inputs are compared again (import closure, compiled artifacts, environment); the
   tokens are decoded and validated; the record is written in one transaction;
   `textDocument/didClose`.

What a record's coverage says:

| Coverage | Meaning |
|---|---|
| complete | steps 3–5 as described, every token valid. Errors in the file do not change this; their count is recorded. |
| partial | the time limit was reached (the server's finished prefix is stored, with the line it had reached); or tokens with invalid ranges were dropped (counted, with reasons); or the answer came without a complete-processing report within 5 s |
| failed | imports out of date or `lake setup-file` failed (with what Lake named); a fatal error in the header (with the server's message); the file's worker crashed; the file was stopped for memory; the server stopped; a request failed; malformed token data; inputs changed while the file was processed. No tokens are stored. |
| skipped | not valid UTF-8, larger than `--max-file-mib` (default 4), unreadable |

Server-initiated requests are answered (`client/registerCapability`, refreshes and
`window/workDoneProgress/create` with `null`, `workspace/configuration` with nulls, anything
else with "method not found"). Delta requests are never sent. A malformed frame or message
ends the connection: the stream is not trusted afterwards. If the server exits, the open file
is recorded as failed and the server is started again, at most three times per run; if it
exits because the job is being cancelled, nothing is recorded for the open file. A fatal
progress report is not acted on at once: the reason arrives as a diagnostic just after it.

**Coordinates.** Stored spans are byte ranges within a line of the exact text sent. LSP
positions are converted with the negotiated unit; a position inside a character (a UTF-16
surrogate pair, a UTF-8 sequence) or past the line end is invalid, and the token is dropped
and counted rather than rounded. Lines are split at `\n` as Lean does; a `\r` before it stays
at the end of the line, after every token. Tabs and combining marks are ordinary characters
at this level; the renderer deals with cells (`render.rs`: tabs expand to two cells, control
characters become visible symbols, wide glyphs take two cells, and each display character
keeps the style of the source byte it came from). Display never changes the text.

## The cache

`<cache dir>/projects/<project key>/highlight.sqlite` (WAL mode), next to the index but a
separate file, because the index file is replaced as a whole when it is rebuilt. Never in the
project. Schema version 1 (`PRAGMA user_version`); another version is refused with a pointer
to `slim highlight clean`.

| Table | Contents |
|---|---|
| `baseline` | per path: source SHA-256, provider identity, line count, span blob |
| `semantic` | per path: source SHA-256 and size, line endings, coverage and its reason, line and token counts, processed lines (partial), token legend, position encoding, token blob, error and warning counts, environment fingerprint, digest and size of the import closure, digest of the imports' compiled artifacts, build note, extractor version, elapsed time, timestamp |
| `env` | per fingerprint: `lean-toolchain` contents, identity of the installed `lean` executable, SHA-256 of `lake-manifest.json` and of the Lake configuration file, dependency revisions from the manifest, extractor version; and, as recorded by the run, `lean --version`, the sandbox and the server command |
| `run` | per indexing run: start, end, outcome and counts (an interrupted run has no end) |

Blobs: tokens per line with offsets as variable-length integers, in chunks of 256 lines with
an offset table, so a view decodes only the chunks it shows; the baseline blob carries its
table of scope paths, so it does not depend on in-process ids. Everything read back is
validated; a damaged blob gives plain or syntax-only display.

Paths are relative, so a moved checkout keeps its records as far as the cache key allows
(the key includes the project root; a moved project gets a new cache directory).

## Freshness: separate facts

`F6` shows these for the file on screen; `slim highlight status` summarises them for
the project.

* **Source alignment.** The record's SHA-256 equals that of the bytes shown. If not, the
  cached positions belong to other text and are **never** applied, not even as a preview.
* **Extraction coverage.** complete, partial, failed or skipped, with the reason.
* **Semantic environment.** Toolchain file, the installed toolchain executable, Lake
  configuration, dependency manifest and extractor version as recorded, or what changed.
* **Imported sources.** The file's transitive local imports, followed through the fixed
  `import` header syntax (never through the browser's lexical mentions), have the content
  they had at extraction, by SHA-256. The display says whether every file was hashed in this
  check or some were accepted by size and modification time since an earlier check in the
  session; `R` hashes everything again.
* **Build relation.** Whether the compiled artifacts of those imports are the ones present
  at extraction (size and modification time), and what was known then: for instance that an
  import's source was newer than its `.olean`. Lake's `--no-build` check decides whether such
  a file can be opened at all (it reports "imports are out of date", recorded as failed);
  where it opens, the tokens reflect the compiled version of the import, and the note says so.

Semantic colours are shown when the source is aligned, the environment is the recorded one
and the imported sources are unchanged. If the positions align but the environment or an
import changed, the default is syntax colours only, with a warning in the title; the stale
preview shows the old semantic colours, labelled STALE. While the import check is running
(it reads files, so it follows the first paint) the title says `checking imports` and nothing
is called current. Opening slim checks nothing by elaboration and builds nothing to make
a label look better.

Invalidation is per file through these digests: editing a module makes its own record
unaligned and the records of its transitive importers stale; unrelated modules stay current.
A changed toolchain, Lake configuration, manifest or extractor version makes every record
stale; a changed grammar invalidates baseline spans only.

Not covered: unsaved editor buffers (slim reads files); inputs a metaprogram reads from
outside the project (no project hash can vouch for those); dependency sources under
`.lake/packages` are bound by the manifest hash and revisions, not hashed file by file.

## Parity with VS Code: definition, evidence, discrepancies

**Definition.** For the same file, grammar version and theme: (a) every byte has the same
TextMate scope stack as in VS Code's tokenizer; (b) the same resolved foreground colour and
font-style flags for the bundled themes; (c) Lean's semantic tokens are the ones the same
toolchain's server returns, with the same type and modifiers, at the same positions; (d)
semantic styles are resolved by VS Code's rules. Display differences listed under "Terminal
approximations" are outside the definition.

**Evidence.**

| What | Corpus | Result |
|---|---|---|
| Scope stacks and styles, Dark+, Light+, Dark High Contrast | the original fixtures in `fixtures/highlight` (9 files: nested and unterminated comments, Markdown in documentation, strings with escapes, interpolation and raw strings, characters, non-BMP and combining characters, wide glyphs, `«»` names, tabs, notation and macros, quotations, tactic blocks, CRLF, a byte-order mark, an empty file, a 38 KiB line) | identical for every byte; checked by `cargo test --test highlight baseline` against the committed reference |
| Scope stacks and styles, Dark+ | all 1,435 local files of HaagerupIzumi, 452,005 lines, 30.0 MiB | 0 lines differ |
| Scope stacks and styles, Light+ | all 8,311 files of the Mathlib revision HaagerupIzumi depends on (`0df444a360`), 2,302,188 lines, 92.6 MiB | 0 lines differ |
| Scope stacks and styles, Dark+ | 5,768 files of CFSG (every 40th file by path and the 40 largest), 1,467,393 lines, 133.9 MiB | 0 lines differ |
| Semantic tokens equal to the server's | 26 files of HaagerupIzumi (every 45th of three directories; 12,891 tokens) and 4 files of the demo library | identical token lists (line, byte range, type) |
| The whole procedure on five toolchains | the demo library built with Lean 4.16.0, 4.21.0, 4.28.0, 4.33.1 and 4.35.0 | every file complete; 1,577 tokens on 4.16 and 4.21, 1,624 on 4.28 and 4.33.1, 1,628 on 4.35.0 (the servers differ; each matches its own) |

The reference for the first four rows is VS Code's tokenizer itself: `vscode-textmate` 9.3.2
with `vscode-oniguruma`, the grammars of vscode-lean4 0.0.240 and VS Code 1.142.0's theme
files, driven by `scripts/textmate-reference.mjs`, compared per byte by
`scripts/compare-textmate.py` (`slim highlight dump` produces slim's side). The runs
used vscode-oniguruma 2.0.1; the fixtures and the HaagerupIzumi corpus were repeated with
1.7.0, whose `onig.wasm` has the same SHA-256 as the one inside the installed VS Code 1.142.0,
with identical results. In total 4.22 million lines without a differing byte.

The semantic row uses `scripts/check-semantic-tokens.py`: a second LSP client and a second
decoder, written separately in Python, ask the same server for the same files and compare
with what slim stored.

What was **not** done: no screenshot or token inspection was taken from a running VS Code
window (no display was driven), so the semantic *style* rules are verified against VS Code's
code (the 1.142.0 bundle: the default probe table and `getTokenStyle` were read there) and by
unit tests, not against its pixels. The user's VS Code has semantic highlighting enabled and
no theme set (the default); to compare by eye, use the same theme family (Dark+/Dark Modern
or Light+/Light Modern) and "Developer: Inspect Editor Tokens and Scopes".

**Discrepancy log.**

| # | Where | VS Code | slim | Root cause | Disposition |
|---|---|---|---|---|---|
| 1 | fenced code blocks of other languages inside comments (`Markdown.lean` 20–23) | tokenized with that language's grammar if its extension is installed | the block's own scopes, contents not tokenized | only the Lean grammars are bundled | accepted; part of the definition (the reference is run the same way) |
| 2 | text without any token colour | the theme's `editor.foreground` (Dark+ `#D4D4D4`, Light+ `#000000`) | slim's text colour for the UI theme (`#EEEEEE` dark, `#333333` light) | the UI theme owns the base text colour | accepted |
| 3 | a file starting with a byte-order mark (`Bom.lean`) | the mark is not part of the buffer | the mark is part of line 1 and takes the scope of the token after it; every later offset is the reference's plus 3 bytes | slim shows files as they are | accepted; tested |
| 4 | lines over 20,000 UTF-16 units (`LongLine.lean` 2; a 692,627-character line in CFSG) | not tokenized with the default `editor.maxTokenizationLineLength` | not tokenized, same limit | same rule | none; differs only if the VS Code setting was changed |
| 5 | a colour forced onto an unsuitable background (Dark+ `#DCDCAA` on the light Lean theme) | shown as is | replaced by the base text colour below 3:1 contrast | legibility rule | accepted; cannot happen with the default theme pairs (tested) |
| 6 | terminals without 24-bit colour; the `mono` theme | n/a | nearest xterm-256 colour; no colours at all | terminal | accepted |
| 7 | the context lines around a declaration in the excerpt | n/a | dim, not coloured | keeps the declaration visually apart | accepted |
| 8 | while Lean is still elaborating | semantic colours appear progressively and are refreshed | cached results only: complete, or labelled partial | slim has no live server | by design |
| 9 | servers before Lean 4.28 | may receive overlapping semantic tokens, with undefined result | the later overlapping token is dropped, counted, file recorded partial | older servers do not make tokens disjoint | accepted; did not occur on the fixture with 4.16/4.21; untested on a large old project |
| 10 | a lone `\r` inside a line | a line break | not a line break (Lean splits at `\n`) | slim follows Lean, whose positions the tokens use | untested: no such file in the corpora |
| 11 | semantic token styles | resolved by the running editor | resolved by a port of its rules | not compared against a live editor (see above) | open: compare with "Inspect Editor Tokens and Scopes" on a few files |

No wrong token category, missing semantic range or shifted offset was found in the corpora
above. Entries are added here when one is.

## Supported versions

| | Version | Source |
|---|---|---|
| Lean grammars | vscode-lean4 0.0.240 (`lean4.json`, `lean4-markdown.json`) | the installed extension; SHA-256 in `THIRD-PARTY-NOTICES.md` |
| Tokenizer behaviour | vscode-textmate 9.3.2 | port; reference runs |
| Regular expressions | Oniguruma 6.9.10 via `onig` 6.5.3 | linked |
| Themes and style rules | VS Code 1.142.0 `theme-defaults`, `getTokenStyle` | bundled files; port |
| Lean language servers, tested end to end | 4.16.0, 4.21.0, 4.28.0, 4.33.1, 4.35.0 | `scripts/test-highlight-toolchains.sh`: a copy of the demo library is built with each toolchain's own Lake (4.16 puts `.olean` files in `.lake/build/lib`, later versions in `.lake/build/lib/lean`; both are understood), indexed in the sandbox, and every file must be complete with no process left |
| Other Lean versions from 4.16 | untested | `--allow-untested-toolchain`; the plan says so |
| Lean before 4.16, Lean 3 | not supported | refused |
| Operating system | Linux (process groups, `PR_SET_PDEATHSIG`, `/proc`, bubblewrap, systemd user scopes) | the only one slim runs on |

The legend the tested servers send has 24 token types and 10 modifiers; they use `keyword`,
`variable`, `property`, `function` and `leanSorryLike` on ordinary Lean code (more inside Verso
documentation) and set no modifiers.
Positions are UTF-16 in all of them.

## Tests

`cargo test --release` runs, for highlighting:

* `src/highlight/textmate.rs`, `baseline.rs`: grammar behaviour, multi-line state, byte
  offsets, long lines, the empty file;
* `theme.rs`: Dark+/Light+ colours for Lean scopes, parent selectors and the child
  combinator, user themes with comments and `include`, semantic rules and their scores;
* `semantic.rs`: UTF-16/UTF-8/UTF-32 positions over tabs, combining marks, non-BMP and wide
  characters, CRLF; every way a token can be invalid;
* `codec.rs`, `store.rs`: round trips, chunk-wise decoding, every single-byte corruption of
  a blob, foreign schema versions, atomic replacement seen from a second connection;
* `env.rs`: header imports, a change invalidating importers but not unrelated modules,
  import cycles, stale and missing compiled artifacts, named environment changes;
* `lsp.rs`: strict framing, classification of requests, responses and notifications,
  the initialize result, position encodings;
* `indexer.rs` (scripted server): resume at file boundaries, errors in a file versus stale
  imports, a file over its time limit, a refresh after the token request, reports for other
  versions, malformed data, a dying server and bounded restarts, cancellation, a server
  killed because the job is stopping, inputs changing during extraction, budget, pause,
  unreadable files, several open documents, what the plan refuses;
* `service.rs`: visible lines first and state carried across 3,000 comment lines, spans
  read back from disk, an edited file dropping its old semantic tokens, damaged blobs, the
  memory budget, and the freshness facts one by one;
* `render.rs`, `app/hl.rs`: overlaying semantic tokens, wrapping identical to plain text at
  several widths with tabs and wide characters, the compositing order on the highlighted
  line, stale tokens only in the labelled preview, theme changes without new tokens,
  monochrome and 256-colour output, contrast of the default theme pairs, no cell left at
  terminal defaults, scroll position kept when a snapshot arrives, resizing;
* `tests/highlight.rs`: the real client, cache and command line against a stand-in server
  (`tests/support/fake_lean_server.py`, needs `python3`): exact Unicode ranges in all three
  position encodings, the empty file, stale imports, time limit, crash and restart,
  Ctrl-C with the process tree gone and a rerun resuming, slim itself killed, the
  sandbox leaving the project untouched, no run without confirmation, `clean`; the VS Code
  reference comparison on the fixtures; and, when Lean 4.33.1 and a built `fixtures/demo`
  exist, the real server, including a keyword the fixture defines itself;
* `tests/pty.rs`: the browser in a pseudo-terminal: a coloured source view with no child
  process and no idle CPU use, theme and mode keys, the details overlay; the indexing job
  from the palette declined, confirmed, cancelled with its processes gone, finished files
  coloured from the cache, and a quit while it runs.

Regenerating the reference: `scripts/regen-highlight-reference.sh` (after
`npm install --prefix target/hl-ref vscode-textmate vscode-oniguruma`).
