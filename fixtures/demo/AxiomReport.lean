import SlimDemo

/-!
# Lean's own axiom report for the slim demo fixture

This file is not part of the `SlimDemo` library target. It runs `#print axioms` on every
user-written declaration of the library (in source order, module by module) so that slim's
transitive axiom analysis can be validated against Lean itself. Run it after building, from the
package root, with `lake env lean AxiomReport.lean` (see `axiom-report/README.md`).

Inductive constructors and structure fields (such as `Demo.Graph.edges`) are included because
their names are written in the source. The anonymous instance is listed under the name Lean
generates for it, `Demo.Fixtures.instInhabitedGraph` (the same on every tested toolchain).
Constants that Lean generates on its own (structure constructors `mk`, matchers, equation
lemmas, `rec`, `noConfusion`, derived instances, `native_decide` auxiliaries, the macro's
parser and expander) are not listed separately.
-/

/-- `#print_private_axioms M d` runs `#print axioms` on the private declaration `d` of module `M`.
A private name such as `_private.M.0.d` cannot be written as an identifier in another file, so it
is assembled here and handed to the ordinary `#print axioms` command. -/
macro "#print_private_axioms " m:ident d:ident : command =>
  `(#print axioms $(Lean.mkIdent
      (Lean.Name.append (Lean.Name.mkNum (Lean.Name.append `_private m.getId) 0) d.getId)))

-- SlimDemo.Relation.Basic
#print axioms Demo.Rel
#print axioms Demo.Rel.Reflexive
#print axioms Demo.Rel.Symmetric
#print axioms Demo.Rel.Transitive
#print axioms Demo.Rel.Subrel
#print axioms Demo.Rel.comp
#print axioms Demo.Rel.inv
#print axioms Demo.Rel.ext
#print axioms Demo.Rel.Subrel.antisymm
#print axioms Demo.Rel.Symmetric.inv_eq
#print axioms Demo.Rel.comp_assoc
#print axioms Demo.Rel.reflexive_or_exists_not_refl
#print axioms Demo.Rel.not_reflexive_iff

-- SlimDemo.Relation.Closure
#print axioms Demo.Rel.ReflTransGen
#print axioms Demo.Rel.ReflTransGen.refl
#print axioms Demo.Rel.ReflTransGen.tail
#print axioms Demo.Rel.ReflTransGen.single
#print axioms Demo.Rel.ReflTransGen.trans
#print axioms Demo.Rel.ReflTransGen.head
#print axioms Demo.Rel.ReflTransGen.lift
#print axioms Demo.Rel.ReflTransGen.mono
#print axioms Demo.Rel.ReflTransGen.least
#print axioms Demo.Rel.ReflTransGen.idem
#print axioms Demo.Rel.TransGen
#print axioms Demo.Rel.TransGen.single
#print axioms Demo.Rel.TransGen.tail
#print axioms Demo.Rel.transGen_iff_comp

-- SlimDemo.Relation.Equivalence
#print axioms Demo.Rel.SymmClosure
#print axioms Demo.Rel.SymmClosure.symmetric
#print axioms Demo.Rel.ReflTransGen.symmetric
#print axioms Demo.Rel.EqvGen
#print axioms Demo.Rel.EqvGen.equivalence

-- SlimDemo.Graph.Basic
#print axioms Demo.Graph
#print axioms Demo.Graph.edges
#print axioms Demo.Graph.Adj
#print axioms Demo.Graph.Reach
#print axioms Demo.Graph.mem_cons_iff
#print axioms Demo.Graph.succsOf
#print axioms Demo.Graph.mem_succsOf
#print axioms Demo.Graph.Reach.trans
#print axioms Demo.Graph.Subgraph
#print axioms Demo.Graph.Reach.mono

-- SlimDemo.Graph.Search
#print axioms Demo.Graph.anyOf
#print axioms Demo.Graph.anyOf_eq_true
#print axioms Demo.Graph.anyOf_of_mem
#print axioms Demo.Graph.searchWithin
#print axioms Demo.Graph.searchWithin_step
#print axioms Demo.Graph.searchWithin_sound

-- SlimDemo.Fixtures.Trust
#print axioms Demo.Fixtures.searchWithin_complete
#print axioms Demo.Fixtures.searchWithin_correct
#print axioms Demo.Fixtures.search_trans
#print axioms Demo.Fixtures.searchWithin_succ
#print axioms Demo.Fixtures.oracle_axiom
#print axioms Demo.Fixtures.reach_iff_bounded_search
#print axioms Demo.Fixtures.sampleGraph
#print axioms Demo.Fixtures.sample_search_native
#print axioms Demo.Fixtures.sample_reach

-- SlimDemo.Fixtures.Special
#print_private_axioms SlimDemo.Fixtures.Special Demo.Fixtures.chain_reach_aux
#print axioms Demo.Fixtures.reach_of_long_chain
#print axioms Demo.Fixtures.closure_of_decide_eq
#print axioms Demo.Fixtures.ℛ
#print axioms Demo.Fixtures.step₁
#print axioms Demo.Fixtures.reach_αβ
#print axioms Demo.Fixtures.«reach is transitive»
#print axioms Demo.Fixtures.instInhabitedGraph
#print axioms Demo.Fixtures.WeightedEdge
#print axioms Demo.Fixtures.WeightedEdge.src
#print axioms Demo.Fixtures.WeightedEdge.dst
#print axioms Demo.Fixtures.WeightedEdge.weight
#print axioms Demo.Fixtures.firstEdge?
#print axioms Demo.Fixtures.halvings
#print axioms Demo.Fixtures.hiddenVertex
#print axioms Demo.Fixtures.dropUntilFrom
