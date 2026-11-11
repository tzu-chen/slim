/-
Original fixture for slim's highlighting tests: ordinary declarations,
comments, strings and numbers. Not part of any library.
-/
import Init.Data.Nat.Basic

namespace Fixture.Basics

/-- A documented structure with fields and a default. -/
structure Point (α : Type) where
  x : α
  y : α := x
  deriving Repr

-- a line comment with a "string" and /- no block start -/ inside
/- a block comment /- nested once /- and twice -/ still nested -/ done -/
def origin : Point Nat := { x := 0, y := 0 }

/-! Module documentation in the middle of a file. -/

theorem add_zero' (n : Nat) : n + 0 = n := by
  induction n with
  | zero => rfl
  | succ k ih => simp [ih]

@[simp] theorem Point.x_origin : origin.x = 0 := rfl

private noncomputable def hidden : Nat := Classical.choice ⟨0⟩

protected abbrev Point.swap {α} (p : Point α) : Point α := ⟨p.y, p.x⟩

instance : Inhabited (Point Nat) := ⟨origin⟩

inductive Tree (α : Type) where
  | leaf : Tree α
  | node (l : Tree α) (v : α) (r : Tree α) : Tree α

class HasSize (α : Type) where
  size : α → Nat

axiom unproved : ∀ n : Nat, n = n + 0

def numbers : List Float := [0, 42, 0x2A, 0XFF, 3.14, 1e10, 2.5e-3, 1_000]

def strings : List String :=
  ["plain", "esc \n \t \\ \" \x41 \u03B1", "multi
line", "", "-- not a comment", "/- nor this -/"]

def chars : List Char := ['a', '\n', '\'', '\\', '\x41', '\u03B1', 'α']

def truth : Bool := true && !false

example : True := sorry

def usesProp (p : Prop) (t : Type) (s : Sort 1) : Prop := p

#eval strings.length
#check @Point.swap
#print axioms add_zero'

end Fixture.Basics
