# `slim_demo`: a miniature Lean library for slim

This package is an original miniature Lean 4 library about finite relations, closures and
reachability in finite graphs. It was written to be the demo and test fixture for slim, the
terminal browser for Lean projects. It uses only core Lean: no Mathlib, Batteries or other
dependencies. It is small on purpose and is not a mathematical contribution. A successful build
shows that Lean accepted these declarations, axioms and placeholders included. It does not show
that any informal claim is true.

The two modules under `SlimDemo/Fixtures/` contain **deliberate test fixtures**: missing
proofs, a custom axiom, compiler-trusted evaluation, private and Unicode names, and declarations
that make Lean generate extra constants. Each one has a docstring that starts with
`TEST FIXTURE:`. None of them should be read as a mathematical result.

## Layout

| Path | Contents |
| --- | --- |
| `lean-toolchain` | `leanprover/lean4:v4.33.1` (the in-repo copy is built with this version) |
| `lakefile.toml` | package `slim_demo` with one default `lean_lib`, `SlimDemo`, and no `require`s |
| `SlimDemo.lean` | root module; imports every module |
| `SlimDemo/Relation/Basic.lean` | `Reflexive`, `Symmetric`, `Transitive`, `Subrel`, `comp`, `inv`, extensionality, a classical dichotomy |
| `SlimDemo/Relation/Closure.lean` | `ReflTransGen` (`refl`/`tail`) and `TransGen` with the shared lemmas `single`, `trans`, `head`, `lift`, `mono`, `least` (minimality), `idem` (idempotence) |
| `SlimDemo/Relation/Equivalence.lean` | symmetric closure, symmetry of closures, the equivalence closure `EqvGen` as an `Equivalence` |
| `SlimDemo/Graph/Basic.lean` | `Graph` (edge list), `Adj`, `Reach` (the closure of `Adj`), the successor function `succsOf` and `mem_succsOf` |
| `SlimDemo/Graph/Search.lean` | the fuel-bounded Boolean search `searchWithin` and its fully proved soundness theorem |
| `SlimDemo/Fixtures/Trust.lean` | test fixtures: `sorry`, a placeholder hidden behind a macro, a custom axiom, `native_decide` |
| `SlimDemo/Fixtures/Special.lean` | test fixtures: private, long and Unicode-named declarations, and generated declarations |
| `AxiomReport.lean` | not part of the library target; runs `#print axioms` on every user-written declaration |
| `lean-view.toml` | slim's project map of this library: two chosen objectives, nested groups, a curated argument for the soundness theorem and a shared reading trail. Curation written for the demo, not verification; one reference is wrong on purpose (see the comment at its top) |
| `axiom-report/` | raw `AxiomReport.lean` output for each tested toolchain, plus the exact commands |

Each module begins with a `/-! … -/` module docstring. Lean requires `import` lines to come
first, so in modules that import something the docstring comes right after the imports.

## Building

```sh
lake build                       # with the toolchain in lean-toolchain (v4.33.1)
lake env lean AxiomReport.lean   # Lean's own axiom report (after building)
```

The library builds on Lean v4.16.0, v4.21.0, v4.28.0, v4.33.1 and v4.35.0. The only warnings are
the two expected `declaration uses 'sorry'` warnings. `axiom-report/README.md` gives the exact
per-version commands. They use a copy under `/tmp` with that version written into
`lean-toolchain`. The proofs avoid library lemma names that differ between these versions (for
example, v4.35.0 deprecates `if_pos`/`if_neg`). They use their own small lemmas proved by
induction, plus `cases`, `induction`, `split`, `subst`, `obtain` and `decide`.

## How the library hangs together

The closure lemmas in `Relation/Closure.lean` are shared by most later results.
`Graph.Reach.trans` is `ReflTransGen.trans`, and `Graph.Reach.mono` is `ReflTransGen.mono`, which
is in turn `ReflTransGen.lift` along the identity. `searchWithin_sound` ends with
`ReflTransGen.head`. `ReflTransGen.idem` combines `least`, `trans` and `single` with
`Subrel.antisymm`, which rests on extensionality. In the mathematical core, only results that go
through extensionality or the classical dichotomy use axioms. Following those edges in slim is
the intended demo.

## Deliberate test fixtures

| Declaration (in `Demo.Fixtures`) | What it exercises |
| --- | --- |
| `searchWithin_complete` | A deliberately incomplete theorem. Search completeness is stated but proved by `sorry` (a missing-proof warning). |
| `searchWithin_correct` | No `sorry` token in its source, but uses `searchWithin_complete` directly. The absence of the word `sorry` proves nothing. |
| `search_trans` | Depends on the `sorry` through one intermediate lemma (`searchWithin_correct`), giving an evidence path of length two. |
| `fixture_trust_me` (tactic macro) | A macro whose expansion is `sorry`. |
| `searchWithin_succ` | Proved only by `fixture_trust_me`, so its source has no `sorry` token but it depends on `sorryAx`. |
| `oracle_axiom` | A custom `axiom`: a true but unproved bound for the search. |
| `reach_iff_bounded_search` | A theorem that depends on the custom axiom. |
| `sampleGraph` | A small concrete graph used by the next two fixtures. |
| `sample_search_native` | Proved by `native_decide`, so it trusts the compiler. Its axiom report differs between Lean versions. |
| `sample_reach` | Control case: the same computation checked by `decide` in the kernel, then turned into reachability by soundness. It uses no axioms. |
| `chain_reach_aux` (`private`) | A private theorem, with kernel name `_private.SlimDemo.Fixtures.Special.0.Demo.Fixtures.chain_reach_aux`, used by a public theorem. |
| `reach_of_long_chain` | A long statement with six hypotheses that wraps across lines; it uses the private lemma. |
| `closure_of_decide_eq` | Implicit `{…}` and instance-implicit `[DecidableEq γ]` arguments. |
| `ℛ` | A letterlike-symbol name (U+211B). |
| `step₁` | A subscript in a name. |
| `reach_αβ` | Greek letters in a name. |
| `«reach is transitive»` | A quoted name containing spaces. |
| `instInhabitedGraph` | An anonymous `instance : Inhabited Graph`; Lean generates the name (the same on all tested versions). |
| `WeightedEdge` | A structure whose constructor, projections and `deriving DecidableEq, Repr` instances are generated. |
| `firstEdge?` | Definition by pattern matching (generated `match_1`); `?` in a name. |
| `halvings` | Definition by well-founded recursion. Its `propext`/`Quot.sound` dependency comes only from the generated termination proof. |
| `hiddenVertex` | An `opaque` constant. |
| `dropUntilFrom` | A `partial def`; the kernel sees an opaque constant built from the anonymous `Inhabited` instance. |

At the end of `Fixtures/Special.lean` there are also two lexical fixtures that are not
declarations. One is a nested block comment containing a fake theorem and a `sorry`. The other is
a line comment containing an unmatched `/-`. A source-only scanner must report neither.

The core modules also contain constructs that make Lean generate declarations: `Graph` is a
structure with `deriving DecidableEq, Repr`, and `succsOf`, `anyOf` and `searchWithin` use
structural recursion with matchers. These are ordinary library content, not fixtures.

## Axiom profile

From Lean's own `#print axioms` (see `axiom-report/`):

* `propext`, `Quot.sound`: `Rel.ext` and its users `Subrel.antisymm`, `Symmetric.inv_eq`,
  `comp_assoc` and `ReflTransGen.idem`, plus `Fixtures.halvings`.
* `propext`, `Classical.choice`, `Quot.sound`: `Rel.reflexive_or_exists_not_refl` and
  `Rel.not_reflexive_iff`.
* `sorryAx`: `searchWithin_complete`, `searchWithin_correct`, `search_trans`,
  `searchWithin_succ`.
* `Demo.Fixtures.oracle_axiom`: the axiom itself and `reach_iff_bounded_search`.
* `sample_search_native`: `Lean.ofReduceBool` on v4.16.0 and v4.21.0;
  `Lean.ofReduceBool, Lean.trustCompiler` on v4.28.0; and the generated axiom
  `Demo.Fixtures.sample_search_native._native.native_decide.ax_1_1` on v4.33.1 and v4.35.0.
* Everything else, including `searchWithin_sound` and the whole graph development, depends on no
  axioms.

## Semantic snapshot used by `slim demo`

`snapshot/slim_demo.lean-v4.33.1.jsonl` was produced from this package's real build artifacts
by `slim export` (exporter 0.1.0, compat shim "modern", Lean 4.33.1, githash
819816b2e0a3bf405af45ae5c7af2491d8f5bee6), inside the bubblewrap sandbox, on 2026-11-10. Its
header records the provenance, scope, sandbox and limits; every local source file's SHA-256 is in
its module records. slim's transitive axiom sets computed from it equal Lean's
`collectAxioms` for all 166 local declarations and the raw `#print axioms` output in
`axiom-report/v4.33.1.txt` for all 73 user-written names (tests: `tests/semantic_fixture.rs`).
Regenerate with `scripts/regen-demo-snapshot.sh`; the records are deterministic (only the header
timestamp changes).
