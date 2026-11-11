#!/usr/bin/env python3
"""Draws slim's logo: assets/logo/slim-light.svg and slim-dark.svg.

The mark is the browser's three panes on a rounded tile; the wordmark is set
in Inter Medium (SIL Open Font License 1.1) and written into the files as
outlines, so nothing depends on the fonts of whoever views them. Colours are
lean-lang.org's light and dark palettes (docs/design.md, section 11).

Usage: scripts/make-logo.py [--font FILE.ttf|.otf] [--out DIR]
Needs fontTools and a font file (default: Inter-Medium.ttf as installed by
Fedora's rsms-inter-fonts).
"""
import argparse
import os

from fontTools.pens.recordingPen import DecomposingRecordingPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.boundsPen import BoundsPen
from fontTools.ttLib import TTFont

PALETTES = {
    "light": dict(text="#333333", primary="#386EE0", on="#F9FBFD"),
    "dark": dict(text="#EEEEEE", primary="#3B94FF", on="#181818"),
}
TILE = 112        # the tile is TILE x TILE; the canvas has no margin around it
GAP = 30          # between tile and wordmark
ASCENT = 76       # height of the `l`, in canvas units


def contours(glyph_set, name):
    """The glyph's contours as separate (svg path, bounds) pairs, in font units."""
    # Composite glyphs (an i made of a stem and a dot) are drawn out.
    rec = DecomposingRecordingPen(glyph_set)
    glyph_set[name].draw(rec)
    out, cur = [], []
    for op, args in rec.value:
        cur.append((op, args))
        if op in ("closePath", "endPath"):
            sp, bp = SVGPathPen(glyph_set), BoundsPen(glyph_set)
            for o, a in cur:
                getattr(sp, o)(*a)
                getattr(bp, o)(*a)
            out.append((sp.getCommands(), bp.bounds))
            cur = []
    return out


def wordmark(font, colors):
    """`slim` as outlines: (svg, width). Baseline at y=0, y pointing down."""
    gs, cmap, hmtx = font.getGlyphSet(), font.getBestCmap(), font["hmtx"]
    l_top = max(b[3] for _, b in contours(gs, cmap[ord("l")]))
    k = ASCENT / l_top
    x, parts, right = 0.0, [], 0.0
    for ch in "slim":
        name = cmap[ord(ch)]
        cs = contours(gs, name)
        # The dot of the i is its highest contour: it takes the accent colour.
        dot = max(range(len(cs)), key=lambda i: cs[i][1][1]) if ch == "i" and len(cs) > 1 else None
        for i, (d, b) in enumerate(cs):
            fill = colors["primary"] if i == dot else colors["text"]
            parts.append(f'<path transform="translate({x * k:.2f} 0) scale({k:.5f} {-k:.5f})" d="{d}" fill="{fill}"/>')
            right = max(right, (x + b[2]) * k)
        x += hmtx[name][0]
    left = min(b[0] for _, b in contours(gs, cmap[ord("s")])) * k
    return f'<g transform="translate({-left:.2f} 0)">' + "".join(parts) + "</g>", right - left


def logo(font, colors):
    word, width = wordmark(font, colors)
    tile = (
        f'<rect width="{TILE}" height="{TILE}" rx="26" fill="{colors["primary"]}"/>'
        f'<rect x="20" y="22" width="16" height="68" rx="5" fill="{colors["on"]}" opacity="0.55"/>'
        f'<rect x="44" y="22" width="30" height="68" rx="5" fill="{colors["on"]}"/>'
        f'<rect x="82" y="22" width="12" height="68" rx="5" fill="{colors["on"]}" opacity="0.55"/>'
    )
    w = TILE + GAP + width
    baseline = (TILE + ASCENT) / 2
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.1f} {TILE}" width="{w:.1f}" height="{TILE}" role="img" aria-label="slim">\n'
        f"<title>slim</title>\n{tile}\n"
        f'<g transform="translate({TILE + GAP} {baseline:.1f})">{word}</g>\n</svg>\n'
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--font", default="/usr/share/fonts/rsms-inter-fonts/Inter-Medium.ttf")
    ap.add_argument("--out", default=os.path.join(os.path.dirname(__file__), "..", "assets", "logo"))
    ap.add_argument("--suffix", default="")
    a = ap.parse_args()
    font = TTFont(a.font)
    os.makedirs(a.out, exist_ok=True)
    for mode, colors in PALETTES.items():
        path = os.path.join(a.out, f"slim-{mode}{a.suffix}.svg")
        open(path, "w").write(logo(font, colors))
        print("wrote", os.path.normpath(path))


if __name__ == "__main__":
    main()
