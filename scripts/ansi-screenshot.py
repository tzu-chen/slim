#!/usr/bin/env python3
"""Render a terminal capture with ANSI colours (`tmux capture-pane -e -p`) to PNG.

Used to produce real screenshots of slim for the documentation.
Usage: ansi-screenshot.py CAPTURE.txt OUT.png [--bg #000000 --fg #d0d0d0]
Capture with `-N` so trailing blank cells keep their colours.
Needs Pillow and fontTools; uses Noto Sans Mono with STIX Two Math as a
fallback for mathematical symbols.
"""
import re
import sys
import unicodedata

from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFont

REGULAR = "/usr/share/fonts/google-noto/NotoSansMono-Regular.ttf"
BOLD = "/usr/share/fonts/google-noto/NotoSansMono-Bold.ttf"
FALLBACK = "/usr/share/fonts/stix-fonts/STIXTwoMath-Regular.otf"
SIZE = 16

BASIC = [
    (0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16), (36, 114, 200), (188, 63, 188), (17, 168, 205), (229, 229, 229),
    (102, 102, 102), (241, 76, 76), (35, 209, 139), (245, 245, 67), (59, 142, 234), (214, 112, 214), (41, 184, 219), (255, 255, 255),
]


def xterm256(n):
    if n < 16:
        return BASIC[n]
    if n >= 232:
        v = 8 + 10 * (n - 232)
        return (v, v, v)
    n -= 16
    levels = [0, 95, 135, 175, 215, 255]
    return (levels[n // 36], levels[(n // 6) % 6], levels[n % 6])


def hexrgb(h):
    h = h.lstrip("#")
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4))


_fonts = {}


def font(path, size):
    if (path, size) not in _fonts:
        _fonts[(path, size)] = ImageFont.truetype(path, size)
    return _fonts[(path, size)]


def fitted(path, ch, width):
    """The font at SIZE, or a smaller one when the glyph is wider than its
    cells (terminals keep such glyphs inside the cell too). Returns (font, dy)."""
    f = font(path, SIZE)
    w = f.getlength(ch)
    if w <= width + 0.5:
        return f, 0
    size = max(6, int(SIZE * width / w))
    return font(path, size), (SIZE - size) // 2


def parse(path, dfg, dbg):
    """Yields rows of cells (char, fg, bg, bold)."""
    rows = []
    # tmux emits attribute changes as one stream: state carries across lines.
    fg, bg, bold, rev = None, None, False, False
    for line in open(path, encoding="utf-8").read().split("\n"):
        cells = []
        for tok in re.split(r"(\x1b\[[0-9;]*m)", line):
            if tok.startswith("\x1b["):
                codes = [int(c) if c else 0 for c in tok[2:-1].split(";")] or [0]
                i = 0
                while i < len(codes):
                    c = codes[i]
                    if c == 0:
                        fg, bg, bold, rev = None, None, False, False
                    elif c == 1:
                        bold = True
                    elif c == 22:
                        bold = False
                    elif c == 7:
                        rev = True
                    elif c == 27:
                        rev = False
                    elif 30 <= c <= 37:
                        fg = BASIC[c - 30]
                    elif 90 <= c <= 97:
                        fg = BASIC[c - 90 + 8]
                    elif 40 <= c <= 47:
                        bg = BASIC[c - 40]
                    elif 100 <= c <= 107:
                        bg = BASIC[c - 100 + 8]
                    elif c == 39:
                        fg = None
                    elif c == 49:
                        bg = None
                    elif c in (38, 48) and i + 1 < len(codes):
                        if codes[i + 1] == 5 and i + 2 < len(codes):
                            col = xterm256(codes[i + 2])
                            i += 2
                        elif codes[i + 1] == 2 and i + 4 < len(codes):
                            col = tuple(codes[i + 2:i + 5])
                            i += 4
                        else:
                            col = None
                        if c == 38:
                            fg = col
                        else:
                            bg = col
                    i += 1
            else:
                for ch in tok:
                    f, b = fg or dfg, bg or dbg
                    if rev:
                        f, b = b, f
                    cells.append((ch, f, b, bold))
                    if unicodedata.east_asian_width(ch) in ("W", "F"):
                        cells.append(("", f, b, bold))
        rows.append(cells)
    while rows and not rows[-1]:
        rows.pop()
    return rows


def main():
    args = sys.argv[1:]
    src, out = args[0], args[1]
    dbg = hexrgb(args[args.index("--bg") + 1]) if "--bg" in args else (0, 0, 0)
    dfg = hexrgb(args[args.index("--fg") + 1]) if "--fg" in args else (208, 208, 208)
    reg = font(REGULAR, SIZE)
    cmap = TTFont(REGULAR).getBestCmap()
    cw = round(reg.getlength("M"))
    asc, desc = reg.getmetrics()
    ch_h = asc + desc
    rows = parse(src, dfg, dbg)
    width = max(len(r) for r in rows)
    img = Image.new("RGB", (width * cw, len(rows) * ch_h), dbg)
    d = ImageDraw.Draw(img)
    for y, row in enumerate(rows):
        for x, (ch, f, b, bo) in enumerate(row):
            d.rectangle([x * cw, y * ch_h, (x + 1) * cw - 1, (y + 1) * ch_h - 1], fill=b)
        for x, (ch, f, b, bo) in enumerate(row):
            if not ch or ch == " ":
                continue
            path = (BOLD if bo else REGULAR) if ord(ch) in cmap else FALLBACK
            cells = 2 if x + 1 < len(row) and row[x + 1][0] == "" else 1
            fnt, dy = fitted(path, ch, cells * cw)
            d.text((x * cw, y * ch_h + dy), ch, font=fnt, fill=f)
    img.save(out)


if __name__ == "__main__":
    main()
