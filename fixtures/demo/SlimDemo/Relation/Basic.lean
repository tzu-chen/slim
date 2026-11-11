/-!
# Relations: basic notions

A *relation* on a type `α` is a predicate `α → α → Prop` on ordered pairs. This module fixes the
vocabulary used throughout the demo library:

* the properties `Reflexive`, `Symmetric` and `Transitive`;
* inclusion of relations (`Subrel`), composition (`comp`) and the inverse relation (`inv`);
* extensionality (`ext`): two relations that relate exactly the same pairs are equal.

Extensionality is proved with `funext` and `propext`, so every result that goes through
`Demo.Rel.ext` depends on the standard axioms `propext` and `Quot.sound`. The dichotomy
`reflexive_or_exists_not_refl` uses the law of excluded middle and therefore also depends on
`Classical.choice`, as does `not_reflexive_iff`, which is proved from it.
-/

universe u

namespace Demo

/-- A binary relation on a type `α`: a proposition `r a b` for every ordered pair `(a, b)`. -/
abbrev Rel (α : Type u) : Type u := α → α → Prop

namespace Rel

variable {α : Type u}

/-- A relation is reflexive when every element is related to itself. -/
def Reflexive (r : Rel α) : Prop := ∀ a, r a a

/-- A relation is symmetric when `r a b` always implies `r b a`. -/
def Symmetric (r : Rel α) : Prop := ∀ ⦃a b⦄, r a b → r b a

/-- A relation is transitive when `r a b` and `r b c` together imply `r a c`. -/
def Transitive (r : Rel α) : Prop := ∀ ⦃a b c⦄, r a b → r b c → r a c

/-- Inclusion of relations: `Subrel r s` says that every pair related by `r` is also related
by `s`. -/
def Subrel (r s : Rel α) : Prop := ∀ ⦃a b⦄, r a b → s a b

/-- Composition of relations: `comp r s a c` holds when some middle element `b` satisfies
`r a b` and `s b c` (first a step of `r`, then a step of `s`). -/
def comp (r s : Rel α) : Rel α := fun a c => ∃ b, r a b ∧ s b c

/-- The inverse (converse) relation: it relates `a` to `b` exactly when `r` relates `b` to `a`. -/
def inv (r : Rel α) : Rel α := fun a b => r b a

/-- Extensionality for relations: two relations that relate exactly the same pairs are equal.
The proof uses function extensionality and propositional extensionality. -/
theorem ext {r s : Rel α} (h : ∀ a b, r a b ↔ s a b) : r = s :=
  funext fun a => funext fun b => propext (h a b)

/-- Antisymmetry of inclusion: two relations included in each other are equal. -/
theorem Subrel.antisymm {r s : Rel α} (hrs : Subrel r s) (hsr : Subrel s r) : r = s :=
  ext fun _ _ => ⟨fun h => hrs h, fun h => hsr h⟩

/-- A symmetric relation coincides with its inverse relation. -/
theorem Symmetric.inv_eq {r : Rel α} (h : Symmetric r) : inv r = r :=
  Subrel.antisymm (fun _ _ hab => h hab) (fun _ _ hab => h hab)

/-- Composition of relations is associative. -/
theorem comp_assoc (r s t : Rel α) : comp (comp r s) t = comp r (comp s t) :=
  ext fun _ _ =>
    ⟨fun ⟨c, ⟨b, hab, hbc⟩, hcd⟩ => ⟨b, hab, c, hbc, hcd⟩,
     fun ⟨b, hab, c, hbc, hcd⟩ => ⟨c, ⟨b, hab, hbc⟩, hcd⟩⟩

/-- Classically, every relation is either reflexive or fails to relate some element to itself.
This uses the law of excluded middle, so it depends on `Classical.choice`. -/
theorem reflexive_or_exists_not_refl (r : Rel α) : Reflexive r ∨ ∃ a, ¬ r a a :=
  (Classical.em (∃ a, ¬ r a a)).elim Or.inr fun h =>
    Or.inl fun a => (Classical.em (r a a)).elim id fun ha => absurd ⟨a, ha⟩ h

/-- A relation fails to be reflexive exactly when some element is not related to itself. The
forward direction is classical: it goes through `reflexive_or_exists_not_refl`. -/
theorem not_reflexive_iff {r : Rel α} : ¬ Reflexive r ↔ ∃ a, ¬ r a a :=
  ⟨fun h => (reflexive_or_exists_not_refl r).elim (fun hr => absurd hr h) id,
   fun ⟨a, ha⟩ hr => ha (hr a)⟩

end Rel

end Demo
