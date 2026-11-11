# Lean's own axiom report for the demo fixture

Each `vX.Y.Z.txt` file here is the raw, unedited standard output plus standard error of running
`../AxiomReport.lean` with Lean `vX.Y.Z`. The files have no header; this README records how they
were produced. `AxiomReport.lean` runs `#print axioms` on all 73 user-written names of the library:
65 declarations, 4 inductive constructors and 4 structure fields. A private declaration's name
cannot be written in another file, so the report builds `_private.SlimDemo.Fixtures.Special.0.Demo.Fixtures.chain_reach_aux`
with a small macro and passes it to the ordinary `#print axioms` command.

## Exact commands

The commands were run on 2026-11-10 with the toolchains already installed under
`~/.elan/toolchains`. They did not use the network, `elan` toolchain switching or `lake update`.
Each toolchain's `lake` binary was called directly, with that toolchain's `bin` first on `PATH`,
in a fresh copy of the package:

```sh
SRC=$PWD/fixtures/demo          # from the root of a checkout of slim
for V in v4.16.0 v4.21.0 v4.28.0 v4.33.1 v4.35.0; do
  TC=$HOME/.elan/toolchains/leanprover--lean4---$V
  DST=/tmp/slim-fixture-$V
  rm -rf "$DST" && mkdir -p "$DST"
  rsync -a --delete --exclude .lake --exclude lake-manifest.json --exclude axiom-report \
    "$SRC"/ "$DST"/
  printf 'leanprover/lean4:%s\n' "$V" > "$DST/lean-toolchain"
  (cd "$DST" &&
   PATH="$TC/bin:$PATH" "$TC/bin/lake" build &&
   PATH="$TC/bin:$PATH" "$TC/bin/lake" env lean AxiomReport.lean > "$SRC/axiom-report/$V.txt" 2>&1)
done
```

On every version `lake build` succeeded. Its only warnings were the two expected
`declaration uses 'sorry'` warnings, for `Demo.Fixtures.searchWithin_complete` and
`Demo.Fixtures.searchWithin_succ`. `lake env lean AxiomReport.lean` exited with status 0.

## What the reports say

These results are the same on all five versions:

| Axioms reported | Declarations |
| --- | --- |
| none | every declaration not listed below (58 of the 73 names) |
| `[propext, Quot.sound]` | `Demo.Rel.ext`, `Demo.Rel.Subrel.antisymm`, `Demo.Rel.Symmetric.inv_eq`, `Demo.Rel.comp_assoc`, `Demo.Rel.ReflTransGen.idem`, `Demo.Fixtures.halvings` |
| `[propext, Classical.choice, Quot.sound]` | `Demo.Rel.reflexive_or_exists_not_refl`, `Demo.Rel.not_reflexive_iff` |
| `[sorryAx]` | `Demo.Fixtures.searchWithin_complete`, `Demo.Fixtures.searchWithin_correct`, `Demo.Fixtures.search_trans`, `Demo.Fixtures.searchWithin_succ` |
| `[Demo.Fixtures.oracle_axiom]` | `Demo.Fixtures.oracle_axiom`, `Demo.Fixtures.reach_iff_bounded_search` |

`Demo.Fixtures.halvings` gets `propext` and `Quot.sound` from the termination proof that Lean
generates for it (`halvings._proof_1` on v4.33.1, proved by `omega`). Its own source contains
no proof.

Only one line differs between versions: the one for `Demo.Fixtures.sample_search_native`, which is
proved by `native_decide`.

| Toolchains | Reported axioms for `sample_search_native` |
| --- | --- |
| v4.16.0, v4.21.0 | `[Lean.ofReduceBool]` |
| v4.28.0 | `[Lean.ofReduceBool, Lean.trustCompiler]` |
| v4.33.1, v4.35.0 | `[Demo.Fixtures.sample_search_native._native.native_decide.ax_1_1]` |

There are two reasons for this:

* On v4.16.0–v4.28.0, `native_decide` adds an auxiliary definition. It is named
  `sample_search_native._nativeDecide_1` on v4.16.0 and `..._nativeDecide_1_1` on v4.21.0 and
  v4.28.0. The proof goes through the core axiom `Lean.ofReduceBool`, wrapped in an auxiliary
  lemma. That lemma is `SlimDemo.Fixtures.Trust._auxLemma.1` on v4.16.0,
  `Demo.Fixtures.sample_search_native._proof_1_3` on v4.21.0 and `..._proof_1_1` on v4.28.0.
  On v4.33.1 and v4.35.0 the tactic instead adds a fresh axiom per use, stating that the
  evaluated Boolean is `true`, and the proof is that axiom. `Lean.ofReduceBool` no longer appears.
* `#print axioms` itself changed. On v4.21.0 and earlier it does not look inside the types of
  axioms. From v4.28.0 on it does, so the type of `Lean.ofReduceBool` adds `Lean.trustCompiler`.
  That type mentions `Lean.reduceBool`, an `opaque` constant whose value uses
  `Lean.trustCompiler` precisely so that `#print axioms` can track it. The type of
  the v4.33+ auxiliary axiom mentions only the library's own axiom-free definitions. From v4.33.1
  on, `#print axioms` also reads axiom sets for imported declarations from data computed when each
  `.olean` was written, instead of walking the imported proof bodies again.

## Axiom-kind constants that `#print axioms` never reports

Listing every environment constant of the `SlimDemo.*` modules by kind gives different results
on different versions:

* v4.16.0 and v4.21.0 (old code generator): `SlimDemo.Graph.Basic` contains six constants of
  kind *axiom*. These are compiler specializations created for the derived `Repr` and
  `DecidableEq` instances of `Graph`, for example
  `List.repr._at._private.SlimDemo.Graph.Basic.0.Demo.reprGraph.«_@».SlimDemo.Graph.Basic._hyg.114._spec_1`.
  No proof uses them, so `#print axioms` never reports them. An exporter that treats every
  `axiomInfo` as an assumption would misreport them.
* v4.33.1 and v4.35.0: the only axiom-kind constants are `Demo.Fixtures.oracle_axiom` and the
  `native_decide` auxiliary axiom.
* v4.28.0: the only axiom-kind constant is `Demo.Fixtures.oracle_axiom`.
