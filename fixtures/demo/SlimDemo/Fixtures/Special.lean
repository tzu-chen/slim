import SlimDemo.Graph.Search

/-!
# Test fixtures: names, visibility and generated declarations

**Everything in this module is a deliberate test fixture for slim, not a mathematical
achievement.** Each declaration exercises one way in which a Lean declaration can be awkward for
a browser:

* a `private` lemma used by a public theorem (`chain_reach_aux`, `reach_of_long_chain`);
* a long statement that wraps across lines (`reach_of_long_chain`) and a statement with implicit
  and instance-implicit arguments (`closure_of_decide_eq`);
* Unicode names: the letterlike symbol `ℛ`, the subscript in `step₁`, Greek letters in
  `reach_αβ`, and the quoted name `«reach is transitive»` containing spaces;
* declarations that make Lean generate further constants: an anonymous `instance`, a structure
  with derived instances (`WeightedEdge`), a definition by pattern matching (`firstEdge?`), a
  definition by well-founded recursion (`halvings`), an `opaque` constant (`hiddenVertex`) and a
  `partial def` (`dropUntilFrom`);
* at the end of the file, a nested block comment containing a fake declaration and a `sorry`,
  and a line comment containing an unmatched block-comment opener, for source-only scanners.
-/

universe u

namespace Demo.Fixtures

open Demo.Graph Demo.Rel

/-- TEST FIXTURE: a `private` lemma; Lean mangles its kernel name with a `_private` prefix. It is
used by the public theorem `reach_of_long_chain`. Mathematically: two edges joined by paths give
a path from the first vertex to the last. -/
private theorem chain_reach_aux {g : Graph} {a b c d e : Nat} (hab : g.Adj a b)
    (hbc : g.Reach b c) (hcd : g.Adj c d) (hde : g.Reach d e) : g.Reach a e :=
  ReflTransGen.head hab (hbc.trans (ReflTransGen.head hcd hde))

/-- TEST FIXTURE: a theorem with a long statement (six hypotheses, wrapping across several
lines) whose proof uses the private lemma `chain_reach_aux`. If `g` is a subgraph of `h`, a
chain `a → b ⇝ c → d ⇝ e` in `g` together with an edge `e → a` of `h` puts `a`, `b` and `e` on a
common cycle of `h`: `e` is reachable from `a`, `a` from `e`, and `a` from `b`. -/
theorem reach_of_long_chain {g h : Graph} {a b c d e : Nat}
    (hab : g.Adj a b) (hbc : g.Reach b c) (hcd : g.Adj c d) (hde : g.Reach d e)
    (hgh : Subgraph g h) (hea : h.Adj e a) :
    h.Reach a e ∧ h.Reach e a ∧ h.Reach b a :=
  have hae : h.Reach a e := Reach.mono hgh (chain_reach_aux hab hbc hcd hde)
  have hea' : h.Reach e a := ReflTransGen.single hea
  have hbe : h.Reach b e := Reach.mono hgh (hbc.trans (ReflTransGen.head hcd hde))
  ⟨hae, hea', hbe.trans hea'⟩

/-- TEST FIXTURE: implicit `{…}` and instance-implicit `[…]` arguments. Over any type with
decidable equality, two elements that the decision procedure declares equal are related by the
reflexive–transitive closure of every relation. -/
theorem closure_of_decide_eq {γ : Type u} [DecidableEq γ] {r : Rel γ} {a b : γ}
    (h : decide (a = b) = true) : ReflTransGen r a b := by
  have hab : a = b := of_decide_eq_true h
  subst hab
  exact ReflTransGen.refl

/-- TEST FIXTURE: a definition named by the letterlike symbol `ℛ` (U+211B SCRIPT CAPITAL R).
`ℛ g` is the reachability relation of the graph `g`. -/
def ℛ (g : Graph) : Rel Nat := g.Reach

/-- TEST FIXTURE: a name with a subscript digit, `step₁`. A single edge is a reachable step in
`ℛ`. -/
theorem step₁ {g : Graph} {a b : Nat} (h : g.Adj a b) : ℛ g a b :=
  ReflTransGen.single h

/-- TEST FIXTURE: Greek letters in a name, `reach_αβ`. Reachability composes through a middle
vertex. -/
theorem reach_αβ {g : Graph} {α β γ : Nat} (hαβ : ℛ g α β) (hβγ : ℛ g β γ) : ℛ g α γ :=
  Reach.trans hαβ hβγ

/-- TEST FIXTURE: a «quoted name with spaces». Reachability in a graph is a transitive
relation. -/
theorem «reach is transitive» (g : Graph) : Transitive g.Reach :=
  fun _ _ _ h₁ h₂ => reach_αβ h₁ h₂

/-- TEST FIXTURE: an anonymous instance, whose name Lean generates. The default graph is the
graph with no edges. -/
instance : Inhabited Graph := ⟨⟨[]⟩⟩

/-- TEST FIXTURE: a structure for which Lean generates the constructor `WeightedEdge.mk`, the
field projections and, through `deriving`, `DecidableEq` and `Repr` instances. A weighted edge
is an edge of a graph together with a natural-number weight. -/
structure WeightedEdge where
  /-- The source vertex of the edge. -/
  src : Nat
  /-- The target vertex of the edge. -/
  dst : Nat
  /-- The weight attached to the edge. -/
  weight : Nat
  deriving DecidableEq, Repr

/-- TEST FIXTURE: a definition by pattern matching, for which Lean generates a `match_1` matcher.
It returns the first edge of a graph, if there is one. -/
def firstEdge? : Graph → Option (Nat × Nat)
  | ⟨[]⟩ => none
  | ⟨e :: _⟩ => some e

/-- TEST FIXTURE: a definition by well-founded recursion on `n` (the argument is halved at each
call), which Lean compiles through `WellFounded.fix`. `halvings n` counts how many times `n` can
be halved before reaching `1`. -/
def halvings (n : Nat) : Nat :=
  if n ≤ 1 then 0 else halvings (n / 2) + 1
termination_by n

/-- TEST FIXTURE: an `opaque` constant. A value exists, but its definition cannot be unfolded,
so no proof can find out which vertex it is. -/
opaque hiddenVertex : Nat

/-- TEST FIXTURE: a `partial def`. Lean does not check its termination, and the kernel sees only
an opaque constant, which needs the anonymous `Inhabited Graph` instance above. It drops leading
edges until the first remaining edge leaves the vertex `v`. -/
partial def dropUntilFrom (v : Nat) (g : Graph) : Graph :=
  match firstEdge? g with
  | none => g
  | some (a, _) => if a = v then g else dropUntilFrom v ⟨g.edges.drop 1⟩

-- TEST FIXTURE (lexical, not a declaration): this line comment contains an unmatched /- opener.

/- TEST FIXTURE (lexical, not a declaration): a nested block comment. Everything up to the
matching outer closer is a comment, so neither line below declares anything, and the `sorry`
below is not a placeholder in any proof.
/- nested: theorem fake_nested_theorem : False := sorry -/
theorem fake_commented_theorem : False := sorry
-/

end Demo.Fixtures
