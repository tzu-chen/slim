import SlimDemo.Relation.Basic
import SlimDemo.Relation.Closure
import SlimDemo.Relation.Equivalence
import SlimDemo.Graph.Basic
import SlimDemo.Graph.Search
import SlimDemo.Fixtures.Trust
import SlimDemo.Fixtures.Special

/-!
# SlimDemo: finite relations, closures and reachability

An original miniature library used as the demo and test fixture of the slim browser.

* `SlimDemo.Relation.Basic`: reflexive, symmetric and transitive relations, inclusion,
  composition, inverse and extensionality.
* `SlimDemo.Relation.Closure`: the reflexive–transitive closure `ReflTransGen` and the
  transitive closure `TransGen`, with the shared lemmas `single`, `trans`, `head`, `lift`,
  `mono`, minimality and idempotence.
* `SlimDemo.Relation.Equivalence`: the symmetric closure and the equivalence closure.
* `SlimDemo.Graph.Basic`: finite graphs given by an edge list, adjacency, reachability and
  the successor function.
* `SlimDemo.Graph.Search`: a fuel-bounded Boolean search and its soundness theorem.
* `SlimDemo.Fixtures.Trust` and `SlimDemo.Fixtures.Special`: clearly labelled test
  fixtures (missing proofs, a custom axiom, `native_decide`, private and Unicode names, and
  generated declarations). They are not mathematical achievements.
-/
