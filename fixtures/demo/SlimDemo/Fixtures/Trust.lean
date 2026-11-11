import SlimDemo.Graph.Search

/-!
# Test fixtures: missing proofs, custom axioms and compiler trust

**Everything in this module is a deliberate test fixture for slim, not a mathematical
achievement.** The declarations exist so that the navigator's proof-status and axiom panels have
something concrete to report, and so that its findings can be compared with Lean's own
`#print axioms` output.

* `searchWithin_complete` is stated but left unproved with `sorry`.
* `searchWithin_correct` has no `sorry` in its own text but uses `searchWithin_complete`, and
  `search_trans` reaches it through one intermediate lemma.
* `fixture_trust_me` is a tactic macro that expands to `sorry`; `searchWithin_succ` uses it.
* `oracle_axiom` is a custom axiom; `reach_iff_bounded_search` depends on it.
* `sample_search_native` is proved by `native_decide`, which trusts the compiler, whereas
  `sample_reach` checks the same computation with the kernel and needs no axioms at all.
-/

namespace Demo.Fixtures

open Demo.Graph

/-- TEST FIXTURE: a deliberately incomplete theorem whose proof is `sorry`. It states
completeness of the search: every target reachable from the source is found with enough fuel.
The statement is believed true; the proof is omitted on purpose so that slim can display a
missing-proof warning. -/
theorem searchWithin_complete {g : Graph} {s t : Nat} (h : g.Reach s t) :
    ∃ fuel, g.searchWithin fuel s t = true :=
  sorry

/-- TEST FIXTURE: a direct dependency on a missing proof. This "the decision procedure is
correct" theorem contains no `sorry` token, yet it uses `searchWithin_complete`, which is proved
by `sorry`. The absence of the word `sorry` in a proof proves nothing. -/
theorem searchWithin_correct {g : Graph} {s t : Nat} :
    (∃ fuel, g.searchWithin fuel s t = true) ↔ g.Reach s t :=
  ⟨fun ⟨_, h⟩ => searchWithin_sound h, searchWithin_complete⟩

/-- TEST FIXTURE: a dependency on a missing proof through one intermediate lemma. Two successful
searches compose into a successful search from the first source to the last target; the proof
goes through `searchWithin_correct` and therefore depends on `sorryAx`. -/
theorem search_trans {g : Graph} {m n s t u : Nat} (h₁ : g.searchWithin m s t = true)
    (h₂ : g.searchWithin n t u = true) : ∃ fuel, g.searchWithin fuel s u = true :=
  searchWithin_correct.2 ((searchWithin_sound h₁).trans (searchWithin_sound h₂))

/-- TEST FIXTURE: a tactic macro whose expansion is `sorry`. A proof written with it contains no
`sorry` token in its source text but still depends on `sorryAx`. -/
macro "fixture_trust_me" : tactic => `(tactic| sorry)

/-- TEST FIXTURE: proved only by the `fixture_trust_me` macro, that is, by a hidden `sorry`.
The statement says that the search is monotone in its fuel: one more unit of fuel never turns a
successful search into a failure. -/
theorem searchWithin_succ {g : Graph} {n s t : Nat} :
    g.searchWithin n s t = true → g.searchWithin (n + 1) s t = true := by
  fixture_trust_me

/-- TEST FIXTURE: a custom axiom. It asserts without proof that a search with as many units of
fuel as the graph has edges finds every reachable target. (The statement is true, since a
shortest path never uses an edge twice, but here it is assumed rather than proved.) -/
axiom oracle_axiom (g : Graph) (s t : Nat) :
    g.Reach s t → g.searchWithin g.edges.length s t = true

/-- TEST FIXTURE: a theorem that depends on the custom axiom `oracle_axiom`: reachability is
decided by a single search whose fuel is the number of edges. -/
theorem reach_iff_bounded_search {g : Graph} {s t : Nat} :
    g.Reach s t ↔ g.searchWithin g.edges.length s t = true :=
  ⟨oracle_axiom g s t, searchWithin_sound⟩

/-- TEST FIXTURE: a small sample graph with the cycle `1 → 2 → 3 → 1` and an entry path
`4 → 0 → 1`. -/
def sampleGraph : Graph := ⟨[(0, 1), (1, 2), (2, 3), (3, 1), (4, 0)]⟩

/-- TEST FIXTURE: proved with `native_decide`, so the proof trusts the compiler and the code it
generates for the search rather than the kernel. Depending on the Lean version, `#print axioms`
reports this as `Lean.ofReduceBool` (together with `Lean.trustCompiler` on some versions) or as
an auxiliary axiom that `native_decide` generates for this theorem. -/
theorem sample_search_native : sampleGraph.searchWithin 4 4 3 = true := by
  native_decide

/-- TEST FIXTURE: the honest counterpart of `sample_search_native`. The same search is evaluated
by `decide` (checked by the kernel), and the soundness theorem turns the answer into
reachability of `3` from `4` in the sample graph; no axioms are needed. -/
theorem sample_reach : sampleGraph.Reach 4 3 :=
  searchWithin_sound (n := 4) (by decide)

end Demo.Fixtures
