// Reference tokenization with VS Code's own TextMate engine (vscode-textmate
// + vscode-oniguruma), for comparing slim's baseline highlighter.
//
//   npm install --prefix target/hl-ref vscode-textmate vscode-oniguruma
//   node scripts/textmate-reference.mjs [--theme assets/themes/dark_plus.json] FILE... > ref.jsonl
//   node scripts/textmate-reference.mjs --list FILES.txt > ref.jsonl
//
// One JSON object per file: {"path", "lines": [[[startByte, "scope scope …"], …], …]}
// and, with --theme, "styles": [[[startByte, "#rrggbb", fontStyleBits], …], …].
// Offsets are UTF-8 bytes within the line. Only the Lean grammars bundled in
// assets/grammars are registered: other languages' grammars (used by VS Code
// inside fenced code blocks of comments) are deliberately not loaded, which
// is the parity definition in docs/highlighting.md.
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
// HL_REF_PREFIX selects another npm prefix (for instance one holding the exact
// vscode-oniguruma release a VS Code version bundles).
const require = createRequire(path.join(process.env.HL_REF_PREFIX ? path.resolve(process.env.HL_REF_PREFIX) : path.join(root, 'target/hl-ref'), 'x.js'));
const vsctm = require('vscode-textmate');
const oniguruma = require('vscode-oniguruma');

const wasm = fs.readFileSync(require.resolve('vscode-oniguruma/release/onig.wasm')).buffer;
await oniguruma.loadWASM(wasm);
const onigLib = Promise.resolve({
  createOnigScanner: (p) => new oniguruma.OnigScanner(p),
  createOnigString: (s) => new oniguruma.OnigString(s),
});
const grammars = {
  'source.lean4': path.join(root, 'assets/grammars/lean4.json'),
  'source.lean4.markdown': path.join(root, 'assets/grammars/lean4-markdown.json'),
};
const registry = new vsctm.Registry({
  onigLib,
  loadGrammar: async (scope) => {
    const p = grammars[scope];
    if (!p) return null;
    return vsctm.parseRawGrammar(fs.readFileSync(p, 'utf8'), p);
  },
});

function loadTheme(p) {
  const j = JSON.parse(fs.readFileSync(p, 'utf8'));
  // An including theme adds to (and overrides) what it includes, as in VS Code.
  const base = j.include ? loadTheme(path.join(path.dirname(p), j.include)) : { rules: [], fg: null, bg: null };
  const rules = base.rules.concat(j.tokenColors || []);
  return { rules, fg: (j.colors && j.colors['editor.foreground']) || base.fg, bg: (j.colors && j.colors['editor.background']) || base.bg };
}

const args = process.argv.slice(2);
let files = [];
let theme = null;
for (let i = 0; i < args.length; i++) {
  if (args[i] === '--theme') theme = args[++i];
  else if (args[i] === '--list') files = files.concat(fs.readFileSync(args[++i], 'utf8').split('\n').filter(Boolean));
  else files.push(args[i]);
}
let colorMap = null;
if (theme) {
  const t = loadTheme(theme);
  registry.setTheme({ settings: [{ settings: { foreground: t.fg || '#000000', background: t.bg || '#ffffff' } }, ...t.rules] });
  colorMap = registry.getColorMap();
}
const grammar = await registry.loadGrammar('source.lean4');
const MAX_LINE = 20000;
const byteOffsets = (line) => {
  // UTF-16 index -> UTF-8 byte offset
  const out = new Uint32Array(line.length + 1);
  let b = 0;
  for (let i = 0; i < line.length; i++) {
    out[i] = b;
    const c = line.charCodeAt(i);
    if (c < 0x80) b += 1;
    else if (c < 0x800) b += 2;
    else if (c >= 0xd800 && c <= 0xdbff && i + 1 < line.length) { out[i + 1] = b; b += 4; i++; }
    else b += 3;
  }
  out[line.length] = b;
  return out;
};
for (const f of files) {
  let text = fs.readFileSync(f, 'utf8');
  if (text.charCodeAt(0) === 0xfeff) text = text.slice(1);
  const lines = text.split(/\r\n|\n/);
  let stack = vsctm.INITIAL, stack2 = vsctm.INITIAL;
  const outLines = [], outStyles = [];
  for (const line of lines) {
    const off = byteOffsets(line);
    if (line.length > MAX_LINE) {
      // VS Code does not tokenize such lines (editor.maxTokenizationLineLength):
      // one token with the scopes in force, state unchanged.
      const scopes = grammar.tokenizeLine('', stack).tokens[0].scopes.join(' ');
      outLines.push([[0, scopes]]);
      if (colorMap) {
        const md = grammar.tokenizeLine2('', stack2).tokens[1];
        outStyles.push([[0, colorMap[(md >>> 15) & 0x1ff].toLowerCase(), (md >>> 11) & 0xf]]);
      }
      continue;
    }
    const r = grammar.tokenizeLine(line, stack);
    stack = r.ruleStack;
    outLines.push(r.tokens.filter((t) => t.startIndex < line.length || line.length === 0).map((t) => [off[Math.min(t.startIndex, line.length)], t.scopes.join(' ')]));
    if (colorMap) {
      const r2 = grammar.tokenizeLine2(line, stack2);
      stack2 = r2.ruleStack;
      const st = [];
      for (let i = 0; i < r2.tokens.length; i += 2) {
        const start = r2.tokens[i], md = r2.tokens[i + 1];
        if (start >= line.length && line.length > 0) continue;
        st.push([off[start], colorMap[(md >>> 15) & 0x1ff].toLowerCase(), (md >>> 11) & 0xf]);
      }
      outStyles.push(st);
    }
  }
  const o = { path: f, lines: outLines };
  if (colorMap) o.styles = outStyles;
  process.stdout.write(JSON.stringify(o) + '\n');
}
