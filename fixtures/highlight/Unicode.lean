namespace Fixture.Unicode

/-- Unicode mathematics: ∀ ε > 0, ∃ δ > 0, |x − y| < δ → ‖f x − f y‖ < ε. -/
theorem ε_δ (f : ℝ → ℝ) (h₁ : ∀ x, f x = x) : ∀ ε > 0, ∃ δ > 0, δ ≤ ε := by
  intro ε hε
  exact ⟨ε, hε, le_refl ε⟩

-- non-BMP letters, wide glyphs and combining marks
def 𝔽𝕢 : Nat := 1
def 𝒜_β (x : Nat) : Nat := x + 𝔽𝕢
def 中文名 : String := "漢字とかな"
def étale : Nat := 2  -- e followed by U+0301
def «quoted name with spaces» : Nat := 3
def «Ünï.cödé» := «quoted name with spaces» + 1

-- tabs: one, two, and inside a line
	def tabbed : Nat := 1
		def tabbed2 : Nat := 2
def mid	:	Nat := 3

-- subscripts, primes and operators
def x₀ := 0
def x₁₂₃ := 123
def f' (n : Nat) := n
def f'' (n : Nat) := f' n
example : x₀ ≤ x₁₂₃ ∧ ¬(x₀ ≠ 0) ∨ x₀ ∈ [0] ↔ True := by simp [x₀, x₁₂₃]

notation:50 a " ≼ " b => a ≤ b
infixl:65 " ⊕ₙ " => Nat.add
prefix:max "√" => Nat.sqrt

def lam := fun (x : Nat) ↦ x ⊕ₙ 1
def lam2 := λ x => x

end Fixture.Unicode
