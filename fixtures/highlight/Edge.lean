def a := 1 -- trailing comment
def b := "a string with -- dashes and /- slashes -/" -- then a comment
def c := 'x' -- char then comment
def d := (1 : Nat)/-inline-/+ 2
def e := x.1.2 + y.foo.bar
def f := @id _ 1
def g := fun x => x |>.succ
theorem not_a_keyword_suffix : theoremish = defish := sorry
def Type' := Type
def h.def.theorem := 1
attribute [instance] foo
@[inline, reducible] def i := 1
@[simp, norm_cast] lemma j : True := trivial
open Nat hiding add renaming mul → times
export Nat (succ)
#exit
this is after exit and is still tokenized by the grammar
/- an unterminated block comment
that runs to the end of the file with "a string" and `code`
