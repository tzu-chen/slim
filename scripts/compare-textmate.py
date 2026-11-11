#!/usr/bin/env python3
"""Compare slim's baseline tokens with the VS Code reference.

  node scripts/textmate-reference.mjs --theme assets/themes/dark_plus.json --list files.txt > ref.jsonl
  slim highlight dump --syntax-theme dark-plus --list files.txt > ours.jsonl
  scripts/compare-textmate.py ref.jsonl ours.jsonl [--default-fg '#000000']

Scopes are compared per byte after merging adjacent tokens with equal scope
stacks; styles are compared per byte (colour and font-style bits). The
reference reports the theme default colour where slim reports null.
Exit status 1 when anything differs.
"""
import json, sys

def expand(toks, key=lambda t: t[1]):
    """[(start, value)] -> [(start, value)] with adjacent equal values merged."""
    out = []
    for t in toks:
        v = key(t)
        if out and out[-1][1] == v:
            continue
        if out and out[-1][0] == t[0]:
            out[-1] = (t[0], v)
            if len(out) > 1 and out[-2][1] == v:
                out.pop()
            continue
        out.append((t[0], v))
    return out

def main():
    args = [a for a in sys.argv[1:] if not a.startswith('--')]
    default_fg = '#000000'
    if '--default-fg' in sys.argv:
        default_fg = sys.argv[sys.argv.index('--default-fg') + 1].lower()
    ref, ours = open(args[0]), open(args[1])
    files = lines = bad_scope = bad_style = style_lines = 0
    shown = 0
    for lr, lo in zip(ref, ours):
        r, o = json.loads(lr), json.loads(lo)
        assert r['path'] == o['path'], (r['path'], o['path'])
        files += 1
        if len(r['lines']) != len(o['lines']):
            print(f"{r['path']}: line count {len(r['lines'])} vs {len(o['lines'])}")
            bad_scope += 1
            continue
        for i, (a, b) in enumerate(zip(r['lines'], o['lines'])):
            lines += 1
            if expand(a) != expand(b):
                bad_scope += 1
                if shown < 15:
                    shown += 1
                    print(f"{r['path']}:{i+1}: scopes differ\n  ref  {expand(a)}\n  ours {expand(b)}")
        if 'styles' in r and 'styles' in o:
            for i, (a, b) in enumerate(zip(r['styles'], o['styles'])):
                style_lines += 1
                ea = expand(a, lambda t: (t[1], t[2]))
                eb = expand(b, lambda t: ((t[1] or default_fg), t[2]))
                if ea != eb:
                    bad_style += 1
                    if shown < 15:
                        shown += 1
                        print(f"{r['path']}:{i+1}: styles differ\n  ref  {ea}\n  ours {eb}")
    print(json.dumps({"files": files, "lines": lines, "scope_mismatch_lines": bad_scope, "style_lines": style_lines, "style_mismatch_lines": bad_style}))
    sys.exit(1 if bad_scope or bad_style else 0)

main()
