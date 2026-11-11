/-!
# A heading in module documentation

Some *emphasis*, **strong text**, `inline code` and a [link](https://example.org).

## Lists

* first item with `code`
* second item
  1. nested numbered
  2. another

> a block quote
> over two lines

```lean
theorem inside_fence : True := trivial
```

```python
def not_lean(x):
    return x  # another language: not tokenized here
```

    indented code block

| a | b |
|---|---|
| 1 | 2 |

Text with $x^2$ maths, an <b>html tag</b>, and a trailing backslash \
-/

namespace Fixture.Markdown

/-- Docstring with a list:

- `x` is the input
- the result is **doubled**

and a nested comment /- like this -/ after which the docstring goes on.
See also `Fixture.Markdown.double`. -/
def double (x : Nat) : Nat := 2 * x

-- *not emphasised?* a `code span` in a line comment
/- TODO: _underscore emphasis_ and ~~strike~~ in a block comment -/
def after := 1

/--
```
unlabelled fence with -/ inside? no: the comment ends at the first terminator
-/
def afterFence := 2

end Fixture.Markdown
