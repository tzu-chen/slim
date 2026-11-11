import SlimDemo.Relation.Basic

/-!
# Reflexive–transitive and transitive closures

`ReflTransGen r` is the reflexive–transitive closure of a relation `r`: it relates `a` to `b`
when there is a finite path `a = x₀, x₁, …, xₙ = b` with `r xᵢ xᵢ₊₁` for each step, possibly
with no steps at all. `TransGen r` is the transitive closure, which requires at least one step.

The lemmas `single`, `trans`, `head`, `lift` and `mono` are the shared workhorses of the
library: the graph and search modules use them for every reachability argument. The closure is
the *least* reflexive transitive relation containing `r` (`ReflTransGen.least`), and taking the
closure twice changes nothing (`ReflTransGen.idem`).
-/

universe u v

namespace Demo.Rel

variable {α : Type u}

/-- The reflexive–transitive closure of `r`: `ReflTransGen r a b` holds when `b` can be reached
from `a` by a finite path of `r`-steps, possibly a path of length zero. -/
inductive ReflTransGen (r : Rel α) (a : α) : α → Prop where
  /-- Every element is reachable from itself by the empty path. -/
  | refl : ReflTransGen r a a
  /-- A path from `a` to `b` followed by one more step `r b c` is a path from `a` to `c`. -/
  | tail {b c : α} : ReflTransGen r a b → r b c → ReflTransGen r a c

namespace ReflTransGen

variable {r : Rel α} {a b c : α}

/-- A single step of `r` is a path of length one in the reflexive–transitive closure. -/
theorem single (h : r a b) : ReflTransGen r a b :=
  tail refl h

/-- The reflexive–transitive closure is transitive: two paths can be concatenated. -/
theorem trans (hab : ReflTransGen r a b) (hbc : ReflTransGen r b c) : ReflTransGen r a c := by
  induction hbc with
  | refl => exact hab
  | tail _ hcd ih => exact tail ih hcd

/-- Prepending a step to a path: if `r a b` and `c` is reachable from `b`, then `c` is
reachable from `a`. -/
theorem head (hab : r a b) (hbc : ReflTransGen r b c) : ReflTransGen r a c :=
  trans (single hab) hbc

/-- Paths can be transported along any map `f` that sends `r`-steps to `s`-steps. -/
theorem lift {β : Type v} {s : Rel β} (f : α → β) (hf : ∀ ⦃x y⦄, r x y → s (f x) (f y))
    (h : ReflTransGen r a b) : ReflTransGen s (f a) (f b) := by
  induction h with
  | refl => exact refl
  | tail _ hbc ih => exact tail ih (hf hbc)

/-- The closure is monotone: enlarging a relation enlarges its reflexive–transitive closure.
This is `lift` along the identity map. -/
theorem mono {s : Rel α} (hrs : Subrel r s) : Subrel (ReflTransGen r) (ReflTransGen s) :=
  fun _ _ h => lift id hrs h

/-- Minimality: the reflexive–transitive closure is the least reflexive transitive relation
containing `r`; every reflexive transitive relation `s` containing `r` contains the closure. -/
theorem least {s : Rel α} (hrefl : Reflexive s) (htrans : Transitive s) (hrs : Subrel r s) :
    Subrel (ReflTransGen r) s := by
  intro a b h
  induction h with
  | refl => exact hrefl a
  | tail _ hbc ih => exact htrans ih (hrs hbc)

/-- Idempotence: the reflexive–transitive closure of the closure is the closure itself. -/
theorem idem : ReflTransGen (ReflTransGen r) = ReflTransGen r :=
  Subrel.antisymm
    (least (fun _ => refl) (fun _ _ _ h₁ h₂ => trans h₁ h₂) (fun _ _ h => h))
    (fun _ _ h => single h)

end ReflTransGen

/-- The transitive closure of `r`: `TransGen r a b` holds when `b` can be reached from `a` by a
path of one or more `r`-steps. -/
inductive TransGen (r : Rel α) (a : α) : α → Prop where
  /-- A single step is a path. -/
  | single {b : α} : r a b → TransGen r a b
  /-- A path from `a` to `b` followed by a step `r b c` is a path from `a` to `c`. -/
  | tail {b c : α} : TransGen r a b → r b c → TransGen r a c

/-- The transitive closure is a composition: `TransGen r a c` holds exactly when some `b` is
reachable from `a` in the reflexive–transitive closure and `r b c` is a final step. -/
theorem transGen_iff_comp {r : Rel α} {a c : α} :
    TransGen r a c ↔ comp (ReflTransGen r) r a c := by
  constructor
  · intro h
    induction h with
    | single hab => exact ⟨a, ReflTransGen.refl, hab⟩
    | tail _ hbc ih =>
      obtain ⟨d, had, hdb⟩ := ih
      exact ⟨_, ReflTransGen.tail had hdb, hbc⟩
  · intro h
    obtain ⟨b, hab, hbc⟩ := h
    induction hab generalizing c with
    | refl => exact TransGen.single hbc
    | tail _ hbd ih => exact TransGen.tail (ih hbd) hbc

end Demo.Rel
