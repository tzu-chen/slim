import SlimDemo.Relation.Closure

/-!
# Finite graphs and reachability

A finite directed graph on natural-number vertices is given by its list of edges. Adjacency is
membership of a pair in the edge list, and reachability `Graph.Reach` is *defined* as the
reflexive–transitive closure of adjacency. Consequently every closure lemma from
`SlimDemo.Relation.Closure` applies to graphs directly: `Reach.trans` is
`ReflTransGen.trans`, and `Reach.mono` (a subgraph has fewer paths) is `ReflTransGen.mono`.

The hand-written successor function `succsOf` lists the out-neighbours of a vertex; its
characterization `mem_succsOf`, proved by induction on the edge list, is what the search
procedure in `SlimDemo.Graph.Search` relies on.
-/

universe u

namespace Demo

/-- A finite directed graph whose vertices are natural numbers, given by its list of edges;
a pair `(u, v)` in the list is an edge from `u` to `v`. -/
structure Graph where
  /-- The edges of the graph, as ordered pairs `(source, target)`. -/
  edges : List (Nat × Nat)
  deriving DecidableEq, Repr

namespace Graph

/-- Adjacency in a graph: `g.Adj u v` holds when `(u, v)` is an edge of `g`. -/
def Adj (g : Graph) : Rel Nat := fun u v => (u, v) ∈ g.edges

/-- Reachability in a graph: `g.Reach u v` holds when there is a directed path from `u` to `v`.
It is defined as the reflexive–transitive closure of adjacency. -/
def Reach (g : Graph) : Rel Nat := Rel.ReflTransGen g.Adj

/-- Membership in a list built with `::`, proved directly from the membership predicate:
`x ∈ y :: l` holds exactly when `x = y` or `x ∈ l`. -/
theorem mem_cons_iff {α : Type u} {x y : α} {l : List α} : x ∈ y :: l ↔ x = y ∨ x ∈ l := by
  constructor
  · intro h
    cases h with
    | head => exact Or.inl rfl
    | tail _ h => exact Or.inr h
  · intro h
    cases h with
    | inl h => subst h; exact List.Mem.head _
    | inr h => exact List.Mem.tail _ h

/-- The successor function: `succsOf es v` lists, in order, the targets of the edges in `es`
that leave the vertex `v`. -/
def succsOf : List (Nat × Nat) → Nat → List Nat
  | [], _ => []
  | (a, b) :: es, v => if a = v then b :: succsOf es v else succsOf es v

/-- A vertex `w` is listed by the successor function at `v` exactly when `(v, w)` is an edge.
Proved by induction on the edge list. -/
theorem mem_succsOf {es : List (Nat × Nat)} {v w : Nat} : w ∈ succsOf es v ↔ (v, w) ∈ es := by
  induction es with
  | nil => exact ⟨fun h => (nomatch h), fun h => (nomatch h)⟩
  | cons e es ih =>
    obtain ⟨a, b⟩ := e
    show w ∈ (if a = v then b :: succsOf es v else succsOf es v) ↔ (v, w) ∈ (a, b) :: es
    split
    next hav =>
      subst hav
      constructor
      · intro h
        cases mem_cons_iff.1 h with
        | inl hwb => subst hwb; exact List.Mem.head _
        | inr h => exact List.Mem.tail _ (ih.1 h)
      · intro h
        cases mem_cons_iff.1 h with
        | inl hpair => cases hpair; exact List.Mem.head _
        | inr h => exact List.Mem.tail _ (ih.2 h)
    next hav =>
      constructor
      · intro h
        exact List.Mem.tail _ (ih.1 h)
      · intro h
        cases mem_cons_iff.1 h with
        | inl hpair => cases hpair; exact absurd rfl hav
        | inr h => exact ih.2 h

/-- Reachability is transitive: a path from `u` to `v` followed by a path from `v` to `w` gives
a path from `u` to `w`. -/
theorem Reach.trans {g : Graph} {u v w : Nat} (h₁ : g.Reach u v) (h₂ : g.Reach v w) :
    g.Reach u w :=
  Rel.ReflTransGen.trans h₁ h₂

/-- `Subgraph g h` says that every edge of `g` is also an edge of `h`. -/
def Subgraph (g h : Graph) : Prop := ∀ ⦃e⦄, e ∈ g.edges → e ∈ h.edges

/-- Reachability is monotone in the graph: a vertex reachable in a subgraph is reachable in the
larger graph. -/
theorem Reach.mono {g h : Graph} (hgh : Subgraph g h) : Rel.Subrel g.Reach h.Reach :=
  Rel.ReflTransGen.mono fun _ _ hab => hgh hab

end Graph

end Demo
