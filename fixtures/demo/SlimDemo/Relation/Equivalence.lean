import SlimDemo.Relation.Closure

/-!
# Symmetric and equivalence closures

The symmetric closure of a relation `r` relates `a` and `b` when `r a b` or `r b a`. Taking the
reflexive–transitive closure of the symmetric closure gives the equivalence closure `EqvGen r`:
two elements are related when a zigzag path of `r`-steps, walked in either direction, connects
them. The main result, `EqvGen.equivalence`, shows that this is an `Equivalence`.
-/

universe u

namespace Demo.Rel

variable {α : Type u} {r : Rel α}

/-- The symmetric closure of `r`: `a` and `b` are related when `r a b` or `r b a`. -/
def SymmClosure (r : Rel α) : Rel α := fun a b => r a b ∨ r b a

/-- The symmetric closure of any relation is symmetric. -/
theorem SymmClosure.symmetric : Symmetric (SymmClosure r) :=
  fun _ _ h => Or.symm h

/-- The reflexive–transitive closure of a symmetric relation is symmetric: a path can be walked
backwards. -/
theorem ReflTransGen.symmetric (h : Symmetric r) : Symmetric (ReflTransGen r) := by
  intro a b hab
  induction hab with
  | refl => exact ReflTransGen.refl
  | tail _ hbc ih => exact ReflTransGen.head (h hbc) ih

/-- The equivalence closure of `r`: the reflexive–transitive closure of the symmetric closure.
Two elements are related when a zigzag path of `r`-steps connects them. -/
def EqvGen (r : Rel α) : Rel α := ReflTransGen (SymmClosure r)

/-- The equivalence closure is an equivalence relation: it is reflexive, symmetric and
transitive. -/
theorem EqvGen.equivalence : Equivalence (EqvGen r) :=
  ⟨fun _ => ReflTransGen.refl,
   fun h => ReflTransGen.symmetric SymmClosure.symmetric h,
   fun h₁ h₂ => ReflTransGen.trans h₁ h₂⟩

end Demo.Rel
