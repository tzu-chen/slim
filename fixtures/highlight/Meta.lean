import Lean
open Lean Elab Command Term Meta

namespace Fixture.Meta

declare_syntax_cat arith
syntax num : arith
syntax arith " + " arith : arith
syntax "(" arith ")" : arith
syntax "[arith|" arith "]" : term

macro_rules
  | `([arith| $n:num]) => `($n)
  | `([arith| $a + $b]) => `([arith| $a] + [arith| $b])
  | `([arith| ($a)]) => `([arith| $a])

macro "mytriv" : tactic => `(tactic| first | rfl | trivial)

syntax (name := hello) "#hello " ident : command

@[command_elab hello] def elabHello : CommandElab := fun stx => do
  let id := stx[1].getId
  logInfo m!"hello {id} and {id.toString.length + 1} more"
  let msg := s!"nested {"inner " ++ s!"{id}"} done"
  if msg.isEmpty then throwError "empty: {msg}"
  trace[Meta.debug] "trace {msg}"

elab "#count " n:num : command => do
  let k := n.getNat
  for i in [0:k] do
    unless i == 0 do continue
  return ()

def quoted : MacroM Syntax := `(fun x => x + $(Lean.quote 1))
def antiquot (xs : Array Term) : MacroM Term := `(#[$xs,*])
def pat : Syntax → Bool
  | `($_ + $_) => true
  | _ => false

open Lean in
theorem tactics (p q : Prop) (hp : p) (hq : q) : p ∧ q := by
  constructor
  · exact hp
  · first
    | assumption
    | mytriv
  all_goals try simp at *
  <;> omega

set_option maxHeartbeats 400000 in
theorem calcs (a b c : Nat) (h₁ : a = b) (h₂ : b = c) : a = c :=
  calc a = b := h₁
    _ = c := h₂

attribute [local simp] Nat.add_comm in
example (a b : Nat) : a + b = b + a := by simp

deriving instance Repr for Except

initialize registerTraceClass `Fixture.debug

def withTermination (n : Nat) : Nat :=
  if h : n = 0 then 0 else withTermination (n - 1)
termination_by n
decreasing_by omega

mutual
  def isEven : Nat → Bool
    | 0 => true
    | n + 1 => isOdd n
  def isOdd : Nat → Bool
    | 0 => false
    | n + 1 => isEven n
end

unsafe def cast' {α β} (a : α) : β := unsafeCast a

partial def loop (n : Nat) : Nat := loop (n + 1)

def raw := r"raw \n string" ++ r#"with "quotes" inside"#

universe u v
variable {α : Type u} [Inhabited α] (xs : List α)

section
local notation "‖" x "‖" => List.length x
def len := ‖xs‖
end

end Fixture.Meta
