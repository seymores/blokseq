#!/usr/bin/env python3
"""Render the ANSI screenshots produced by `blok --dump` into PNGs.

The .ans files are written by the app's own ratatui buffer, one SGR run per cell
style, so this is a faithful screenshot and not an artist's impression: the
pixels come from the same render pass the interactive TUI uses.
"""

import os
import re
import sys

from PIL import Image, ImageDraw, ImageFont

FONT_CANDIDATES = [
    "/System/Library/Fonts/SFNSMono.ttf",
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/Supplemental/Andale Mono.ttf",
]

CHROME = 34          # title bar height in px
PAD = 18
CELL_W = 11
CELL_H = 22
FONT_SIZE = 18

SGR = re.compile(r"\x1b\[([0-9;]*)m")

BG = (13, 17, 23)
FG = (205, 214, 228)
SRC_DIR = "dumps"


def load_font():
    for path in FONT_CANDIDATES:
        if os.path.exists(path):
            return ImageFont.truetype(path, FONT_SIZE)
    raise SystemExit("no monospace font found")


# Glyphs a stock macOS monospace font does not have. The app itself avoids these
# (see the note in the design doc) but the substitution keeps the screenshots
# honest if one creeps back in.
FALLBACK = {
    "⌂": "J",
    "⛁": "S",
    "▦": "Q",
    "⌕": "/",
    "↵": "⏎",
    "\ufffe": "?",
}


def build_substitutions(font):
    """Map characters the font has no ink for onto ones it can draw.

    PIL gives a missing glyph an empty mask in this font, so "no ink and not
    whitespace" is the test.
    """
    table = {}
    seen = set()
    for name in os.listdir(SRC_DIR):
        if not name.endswith(".ans"):
            continue
        with open(os.path.join(SRC_DIR, name), encoding="utf-8") as fh:
            for ch in fh.read():
                seen.add(ch)
    for ch in seen:
        if ch.isspace() or ord(ch) < 32:
            continue
        m = font.getmask(ch)
        if m.size[0] == 0 and m.size[1] == 0:
            table[ch] = FALLBACK.get(ch, "?")
    return table


def parse_ansi(text):
    """-> list of rows, each a list of (char, fg, bg, bold, dim, strike, under)."""
    rows = []
    for line in text.split("\n"):
        if line == "":
            continue
        fg, bg = FG, BG
        mods = set()
        cells = []
        i = 0
        while i < len(line):
            m = SGR.match(line, i)
            if m:
                parts = [p for p in m.group(1).split(";") if p != ""]
                j = 0
                while j < len(parts):
                    p = parts[j]
                    if p == "0":
                        fg, bg, mods = FG, BG, set()
                    elif p == "1":
                        mods.add("bold")
                    elif p == "2":
                        mods.add("dim")
                    elif p == "3":
                        mods.add("italic")
                    elif p == "4":
                        mods.add("under")
                    elif p == "9":
                        mods.add("strike")
                    elif p == "38" and j + 4 < len(parts) and parts[j + 1] == "2":
                        fg = tuple(int(x) for x in parts[j + 2:j + 5])
                        j += 4
                    elif p == "48" and j + 4 < len(parts) and parts[j + 1] == "2":
                        bg = tuple(int(x) for x in parts[j + 2:j + 5])
                        j += 4
                    j += 1
                i = m.end()
                continue
            cells.append((line[i], fg, bg, frozenset(mods)))
            i += 1
        rows.append(cells)
    return rows


def mix(a, b, t):
    return tuple(int(round(a[k] + (b[k] - a[k]) * t)) for k in range(3))


def render(rows, font, label, subst=None):
    subst = subst or {}
    cols = max(len(r) for r in rows)
    width = PAD * 2 + cols * CELL_W
    height = CHROME + PAD + len(rows) * CELL_H + PAD
    img = Image.new("RGB", (width, height), BG)
    d = ImageDraw.Draw(img)

    # window chrome
    d.rectangle([0, 0, width, CHROME], fill=(22, 28, 38))
    for k, color in enumerate([(255, 95, 86), (255, 189, 46), (39, 201, 63)]):
        cx = PAD + 6 + k * 20
        d.ellipse([cx - 6, CHROME // 2 - 6, cx + 6, CHROME // 2 + 6], fill=color)
    d.text((PAD + 78, CHROME // 2 - FONT_SIZE // 2 - 1), label, font=font, fill=(130, 145, 168))

    for y, row in enumerate(rows):
        y0 = CHROME + PAD + y * CELL_H
        # merge background runs
        x = 0
        while x < len(row):
            bg = row[x][2]
            run = x
            while run < len(row) and row[run][2] == bg:
                run += 1
            if bg != BG:
                d.rectangle(
                    [PAD + x * CELL_W, y0, PAD + run * CELL_W - 1, y0 + CELL_H - 1],
                    fill=bg,
                )
            x = run
        for x, (ch, fg, bg, mods) in enumerate(row):
            ch = subst.get(ch, ch)
            if ch == " ":
                continue
            color = fg
            if "dim" in mods:
                color = mix(fg, bg, 0.45)
            px = PAD + x * CELL_W
            stroke = 1 if "bold" in mods else 0
            d.text(
                (px, y0 + (CELL_H - FONT_SIZE) // 2 - 2),
                ch,
                font=font,
                fill=color,
                stroke_width=stroke,
                stroke_fill=color,
            )
            if "strike" in mods:
                d.line([px, y0 + CELL_H // 2, px + CELL_W, y0 + CELL_H // 2], fill=color)
            if "under" in mods:
                d.line([px, y0 + CELL_H - 4, px + CELL_W, y0 + CELL_H - 4], fill=color)
    return img


def main():
    src = sys.argv[1] if len(sys.argv) > 1 else "dumps"
    dst = sys.argv[2] if len(sys.argv) > 2 else os.path.join(src, "png")
    only = sys.argv[3:] or None
    global SRC_DIR
    SRC_DIR = src
    os.makedirs(dst, exist_ok=True)
    font = load_font()
    subst = build_substitutions(font)
    if subst:
        print("substituted glyphs:", subst)
    files = sorted(f for f in os.listdir(src) if f.endswith(".ans"))
    if only:
        files = [f for f in files if any(o in f for o in only)]
    for name in files:
        with open(os.path.join(src, name), "r", encoding="utf-8") as fh:
            rows = parse_ansi(fh.read())
        label = name[:-4]
        img = render(rows, font, label, subst)
        out = os.path.join(dst, label + ".png")
        img.save(out)
        print(f"{out}  {img.size[0]}x{img.size[1]}")


if __name__ == "__main__":
    main()
