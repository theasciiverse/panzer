#!/usr/bin/env python3
"""Render a `tmux capture-pane -e -p` dump (truecolour SGR) to a PNG.

Usage: tools/ansi2png.py capture.txt out.png [cell_w cell_h]

Half blocks are drawn as pixels, so the picture shows what the terminal shows.
"""
import re
import sys
import unicodedata
from PIL import Image, ImageDraw, ImageFont

FONTS = ["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", "/usr/share/fonts/TTF/DejaVuSansMono.ttf"]
SGR = re.compile(r"\x1b\[([0-9;:]*)m")
BASIC = [(0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16), (36, 114, 200), (188, 63, 188), (17, 168, 205), (229, 229, 229)]
DEF_FG, DEF_BG = (220, 220, 220), (0, 0, 0)


def apply(params, st):
    p = [int(x) if x else 0 for x in params.replace(":", ";").split(";")] if params else [0]
    i = 0
    while i < len(p):
        c = p[i]
        if c == 0:
            st.update(fg=DEF_FG, bg=DEF_BG, rev=False)
        elif c == 7:
            st["rev"] = True
        elif c == 27:
            st["rev"] = False
        elif c in (38, 48) and i + 1 < len(p):
            key = "fg" if c == 38 else "bg"
            if p[i + 1] == 2 and i + 4 < len(p):
                st[key] = tuple(p[i + 2:i + 5])
                i += 4
            elif p[i + 1] == 5 and i + 2 < len(p):
                n = p[i + 2]
                st[key] = BASIC[n] if n < 8 else (128, 128, 128)
                i += 2
        elif c == 39:
            st["fg"] = DEF_FG
        elif c == 49:
            st["bg"] = DEF_BG
        elif 30 <= c <= 37:
            st["fg"] = BASIC[c - 30]
        elif 40 <= c <= 47:
            st["bg"] = BASIC[c - 40]
        i += 1


def main():
    src, out = sys.argv[1], sys.argv[2]
    cw = int(sys.argv[3]) if len(sys.argv) > 3 else 8
    ch = int(sys.argv[4]) if len(sys.argv) > 4 else 16
    font = None
    for f in FONTS:
        try:
            font = ImageFont.truetype(f, ch - 3)
            break
        except OSError:
            pass
    rows = []
    st = dict(fg=DEF_FG, bg=DEF_BG, rev=False)
    for line in open(src, encoding="utf-8").read().split("\n"):
        cells, pos = [], 0
        for m in SGR.finditer(line + "\x1b[m"):
            for c in line[pos:m.start()]:
                fg, bg = (st["bg"], st["fg"]) if st["rev"] else (st["fg"], st["bg"])
                cells.append((c, fg, bg))
                if unicodedata.east_asian_width(c) == "W":
                    cells.append(("", fg, bg))
            if m.start() < len(line):
                apply(m.group(1), st)
            pos = m.end()
        rows.append(cells)
    while rows and not rows[-1]:
        rows.pop()
    w = max(len(r) for r in rows)
    img = Image.new("RGB", (w * cw, len(rows) * ch), DEF_BG)
    d = ImageDraw.Draw(img)
    for y, r in enumerate(rows):
        for x, (c, fg, bg) in enumerate(r):
            x0, y0 = x * cw, y * ch
            d.rectangle([x0, y0, x0 + cw - 1, y0 + ch - 1], fill=bg)
            if c == "▀":
                d.rectangle([x0, y0, x0 + cw - 1, y0 + ch // 2 - 1], fill=fg)
            elif c == "▄":
                d.rectangle([x0, y0 + ch // 2, x0 + cw - 1, y0 + ch - 1], fill=fg)
            elif c == "█":
                d.rectangle([x0, y0, x0 + cw - 1, y0 + ch - 1], fill=fg)
            elif c and c != " ":
                d.text((x0, y0), c, font=font, fill=fg)
    img.save(out)
    print(out, img.size)


if __name__ == "__main__":
    main()
