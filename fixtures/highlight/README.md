# Highlighting fixtures

Original Lean source written for slim's highlighting tests (not taken from any
library, never compiled). They exercise the baseline grammar: nested and unterminated
comments, documentation with Markdown, strings (escapes, interpolation, raw), character
literals, Unicode identifiers (non-BMP, combining marks, wide glyphs, `«quoted»`), tabs,
notation and macro declarations, quotations, tactic blocks, CRLF line ends, a byte-order
mark, an empty file and a line longer than VS Code's tokenization limit.

`reference/` holds the output of VS Code's own tokenizer for these files:

```sh
npm install --prefix target/hl-ref vscode-textmate vscode-oniguruma   # 9.3.2 / 2.0.1 were used
scripts/regen-highlight-reference.sh
```

`cargo test --test highlight baseline` compares slim's baseline provider with it,
scope stack by scope stack and style by style, for the bundled Dark+ and Light+ themes.
