# Third-party notices

slim itself is licensed under Apache-2.0 (`LICENSE`). It bundles or links the
following material from other projects. Nothing here changes slim's own licence.

## Bundled data files (compiled into the binary)

| Files | From | Licence | Notes |
|---|---|---|---|
| `assets/grammars/lean4.json`, `assets/grammars/lean4-markdown.json` | [vscode-lean4](https://github.com/leanprover/vscode-lean4) 0.0.240, `vscode-lean4/syntaxes/` (as installed from the VS Code marketplace) | Apache-2.0, copy in `assets/grammars/LICENSE-vscode-lean4.txt` | The Lean 4 TextMate grammar and its Markdown grammar for comments, unmodified. `lean4-markdown.json` is derived by that project from the Markdown grammar of VS Code / [microsoft/vscode-markdown-tm-grammar](https://github.com/microsoft/vscode-markdown-tm-grammar) (MIT, Copyright (c) Microsoft Corporation). |
| `assets/themes/dark_plus.json`, `dark_vs.json`, `light_plus.json`, `light_vs.json`, `hc_black.json`, `hc_light.json` | [Visual Studio Code](https://github.com/microsoft/vscode) 1.142.0, `extensions/theme-defaults/themes/` | MIT, Copyright (c) Microsoft Corporation | The default colour themes, unmodified. Only `tokenColors`, `semanticTokenColors`, `semanticHighlighting` and the editor foreground/background are read. |

SHA-256 of the bundled files:

```
ac004106cdefc8567ce32250a677e4b3635864e04d5aafcbdab33778d6f81e0b  assets/grammars/lean4.json
d429c9d34767fc9ae1e3648333e464ad2262d611daae67a2a35389abd81fd329  assets/grammars/lean4-markdown.json
88f5b662378cbe39473a4a8d916a1b4ec580f85858876eaec440288aee2852df  assets/themes/dark_plus.json
38db2163183afd088cf621115087b60a23726856488eabc9fb893aff15b35470  assets/themes/dark_vs.json
1551bcb764405d8dc0d8945aa16701bd0602d4c217503e4c22c1dcaf8c939e59  assets/themes/hc_black.json
1e9f335cc039264ed7ae5b32e3c23fa6d93731dd30bcd1c3e37ce91a07ae5ed7  assets/themes/hc_light.json
bf64f3ec2a80788fad115086bd10fd4136dd1d760ec478f4107756f098d101ea  assets/themes/light_plus.json
fa7d01466a6a32a2b7be30529d12b1fec528959aa5e9938e1a9802550bf19a52  assets/themes/light_vs.json
```

The MIT licence text that applies to the Visual Studio Code files:

```
MIT License

Copyright (c) Microsoft Corporation. All rights reserved.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Ported logic (no code copied verbatim)

* `src/highlight/textmate.rs` and the TextMate part of `src/highlight/theme.rs` are a Rust
  port of the tokenizer and theme matcher of
  [vscode-textmate](https://github.com/microsoft/vscode-textmate) 9.3.2 (MIT, Copyright (c)
  Microsoft Corporation), written from its published behaviour and checked against it.
* The semantic-token style resolution in `src/highlight/theme.rs` follows Visual Studio Code
  1.142.0 (`ColorThemeData.getTokenStyle` and the default token-style rules; MIT).

## Linked libraries added for highlighting

| Crate | Licence | Notes |
|---|---|---|
| [onig](https://crates.io/crates/onig) 6.5.3, onig_sys 69.9.3 | MIT | Rust bindings; `onig_sys` compiles and statically links [Oniguruma](https://github.com/kkos/oniguruma) 6.9.10 (BSD-2-Clause, Copyright (c) 2002-2021 K.Kosako), the regular-expression library VS Code's TextMate engine uses. |

The other crates are listed in `Cargo.lock` and described in `docs/design.md` §10.

## Used only by development scripts (not shipped, not linked)

`scripts/textmate-reference.mjs` runs vscode-textmate 9.3.2 and vscode-oniguruma 2.0.1 (both
MIT), installed with npm into `target/hl-ref`, to produce the reference tokenization in
`fixtures/highlight/reference/`. Those reference files are the tools' output for slim's
own fixture files.

## Evaluated and not adopted

* [Giallo](https://github.com/getzola/giallo) 0.6.0 is licensed EUPL-1.2, a copyleft licence;
  linking it into this Apache-2.0 program would need a licensing decision that is the project
  owner's to make. It was therefore not adopted (a licensing reason, not a performance one).

## Logo

The wordmark in `assets/logo/` is set in [Inter](https://github.com/rsms/inter) Medium
(Copyright © The Inter Project Authors, SIL Open Font License 1.1) and stored as outlines; the
font itself is not bundled. `scripts/make-logo.py` draws the files. The colours are lean-lang.org's.
