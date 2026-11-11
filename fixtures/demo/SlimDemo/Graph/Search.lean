import SlimDemo.Graph.Basic

/-!
# A fuel-bounded reachability search

`Graph.searchWithin g fuel s t` is a Boolean depth-first search for a path from `s` to `t` that
uses at most `fuel` edges. Its soundness theorem, `searchWithin_sound`, is fully proved: when the
search answers `true`, the target is reachable. It rests on two facts about the helper `anyOf`
and on the successor characterization `mem_succsOf`, and it finishes with the closure lemma
`ReflTransGen.head`.

Completeness (every reachable target is found with enough fuel) is *not* proved here; a
statement with a deliberately missing proof lives in `SlimDemo.Fixtures.Trust`.
-/

namespace Demo.Graph

/-- `anyOf p l` is `true` when some element of the list `l` satisfies the Boolean test `p`. -/
def anyOf (p : Nat → Bool) : List Nat → Bool
  | [] => false
  | x :: xs => p x || anyOf p xs

/-- If `anyOf p l` is `true`, then some member of `l` satisfies the test `p`. -/
theorem anyOf_eq_true {p : Nat → Bool} {l : List Nat} (h : anyOf p l = true) :
    ∃ x, x ∈ l ∧ p x = true := by
  induction l with
  | nil => exact nomatch h
  | cons x xs ih =>
    cases hx : p x with
    | true => exact ⟨x, List.Mem.head _, hx⟩
    | false =>
      have h₀ : (p x || anyOf p xs) = true := h
      rw [hx] at h₀
      have h₁ : anyOf p xs = true := h₀
      obtain ⟨y, hy, hpy⟩ := ih h₁
      exact ⟨y, List.Mem.tail _ hy, hpy⟩

/-- If some member `x` of `l` satisfies the test `p`, then `anyOf p l` is `true`. -/
theorem anyOf_of_mem {p : Nat → Bool} {x : Nat} {l : List Nat} (hx : x ∈ l)
    (hpx : p x = true) : anyOf p l = true := by
  induction l with
  | nil => exact nomatch hx
  | cons y ys ih =>
    show (p y || anyOf p ys) = true
    cases mem_cons_iff.1 hx with
    | inl hxy =>
      subst hxy
      exact (congrArg (fun b => b || anyOf p ys) hpx).trans rfl
    | inr h =>
      exact (congrArg (fun b => p y || b) (ih h)).trans (by cases p y <;> rfl)

/-- Fuel-bounded depth-first search: `g.searchWithin fuel s t` is `true` when the search finds a
path from `s` to `t` using at most `fuel` edges. With no fuel left it only checks `s = t`;
otherwise it also tries every successor of `s` with one unit of fuel less. -/
def searchWithin (g : Graph) : Nat → Nat → Nat → Bool
  | 0, s, t => decide (s = t)
  | fuel + 1, s, t =>
    decide (s = t) || anyOf (fun w => searchWithin g fuel w t) (succsOf g.edges s)

/-- One edge extends a successful search: if `s → w` is an edge and the search from `w` finds
`t` with `n` units of fuel, then the search from `s` finds `t` with `n + 1` units. -/
theorem searchWithin_step {g : Graph} {n s w t : Nat} (hsw : g.Adj s w)
    (hw : g.searchWithin n w t = true) : g.searchWithin (n + 1) s t = true := by
  show (decide (s = t) || anyOf (fun w => g.searchWithin n w t) (succsOf g.edges s)) = true
  have h : anyOf (fun w => g.searchWithin n w t) (succsOf g.edges s) = true :=
    anyOf_of_mem (mem_succsOf.2 hsw) hw
  exact (congrArg (fun b => decide (s = t) || b) h).trans (by cases decide (s = t) <;> rfl)

/-- Soundness of the search: whenever the fuel-bounded search returns `true`, the target `t` is
reachable from the source `s` (there is a path in the graph). -/
theorem searchWithin_sound {g : Graph} : ∀ {n s t : Nat}, g.searchWithin n s t = true →
    g.Reach s t
  | 0, s, t, h => by
    have hst : s = t := of_decide_eq_true h
    subst hst
    exact Rel.ReflTransGen.refl
  | n + 1, s, t, h => by
    have h₀ : (decide (s = t) || anyOf (fun w => g.searchWithin n w t) (succsOf g.edges s))
        = true := h
    cases hd : decide (s = t) with
    | true =>
      have hst : s = t := of_decide_eq_true hd
      subst hst
      exact Rel.ReflTransGen.refl
    | false =>
      rw [hd] at h₀
      have h₁ : anyOf (fun w => g.searchWithin n w t) (succsOf g.edges s) = true := h₀
      obtain ⟨w, hw, hwt⟩ := anyOf_eq_true h₁
      exact Rel.ReflTransGen.head (mem_succsOf.1 hw) (searchWithin_sound hwt)

end Demo.Graph
