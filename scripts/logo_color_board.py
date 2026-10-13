#!/usr/bin/env python3
"""Colour round board for the Suramak mark (concept A, round 2 geometry).

Splits the approved mark path into tile / waveform-bars / text-rules,
renders each colour scheme via rsvg-convert, computes WCAG contrast of the
graphic pairs, and composes ONE labelled board image with PIL.

Run with the brand venv:  ~/.venvs/brand/bin/python scripts/logo_color_board.py
"""
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MARK = os.path.join(ROOT, "assets", "logo", "concepts", "a-speak-to-text.svg")
OUT = os.path.join(ROOT, "assets", "logo", "build")

# (name, tile, glyph, accent-on-bars or None)
SCHEMES = [
    ("System Ink",   "#111111", "#FFFFFF", None),
    ("Ink + Signal", "#111111", "#FFFFFF", "#FF453A"),
    ("Navy",         "#0E1B2A", "#F2F4F7", None),
    ("Paper",        "#F4F1EA", "#14161A", None),
    ("White",        "#FFFFFF", "#111111", None),
    ("System Blue",  "#0A84FF", "#FFFFFF", None),
    ("Amber",        "#FF9F0A", "#1C1C1E", None),
    ("Deep Teal",    "#0A4F4F", "#E6FFFB", None),
]


def wcag_lum(hexs):
    c = [int(hexs[i:i + 2], 16) / 255 for i in (1, 3, 5)]
    c = [x / 12.92 if x <= 0.04045 else ((x + 0.055) / 1.055) ** 2.4 for x in c]
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]


def wcag_ratio(a, b):
    la, lb = wcag_lum(a), wcag_lum(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


def _font(bold, size):
    import os as _os
    from PIL import ImageFont
    for p in (
        f"/usr/share/fonts/truetype/liberation/LiberationSans-{'Bold' if bold else 'Regular'}.ttf",
        f"/usr/share/fonts/truetype/dejavu/DejaVuSans{'-Bold' if bold else ''}.ttf",
        "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
    ):
        if _os.path.exists(p):
            return ImageFont.truetype(p, size)
    raise SystemExit("no usable label font found")


def main():
    svg = open(MARK).read()
    d = re.search(r'd="([^"]+)"', svg).group(1)
    parts = re.findall(r'M[^M]+', d)
    if len(parts) != 7:
        sys.exit(f"expected 7 subpaths (tile + 3 bars + 3 rules), got {len(parts)}")
    tile, bars, rules = parts[0], " ".join(parts[1:4]), " ".join(parts[4:7])
    os.makedirs(OUT, exist_ok=True)

    tiles = []
    for name, tc, gc, ac in SCHEMES:
        if ac:
            body = (f'<path d="{tile}" fill="{tc}"/>'
                    f'<path d="{bars}" fill="{ac}"/>'
                    f'<path d="{rules}" fill="{gc}"/>')
            cr = wcag_ratio(ac, tc)
            note = f"bars {ac} on tile  ·  {cr:.1f}:1"
        else:
            body = f'<path d="{tile}" fill="{tc}"/><path d="{bars} {rules}" fill="{gc}"/>'
            cr = wcag_ratio(gc, tc)
            note = f"glyph {gc} on tile  ·  {cr:.1f}:1"
        s = f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256">{body}</svg>'
        slug = name.lower().replace(" ", "-").replace("+", "")
        p = os.path.join(OUT, f"scheme-{slug}.svg")
        open(p, "w").write(s)
        png = p[:-4] + ".png"
        subprocess.run(["rsvg-convert", "-w", "256", "-h", "256", p, "-o", png], check=True)
        tiles.append((name, tc, gc, note, cr, png))

    from PIL import Image, ImageDraw
    fbig = _font(True, 21)
    fname = _font(True, 18)
    fsm = _font(False, 15)

    cw, ch = 300, 386
    cols = 4
    rows = (len(tiles) + cols - 1) // cols
    W, H = cols * cw, rows * ch + 60
    board = Image.new("RGB", (W, H), "#F2F2F7")
    dr = ImageDraw.Draw(board)
    dr.text((24, 16), "Suramak — colour round  ·  mark A “Speak to Text”  ·  WCAG graphic contrast ≥ 3:1",
            font=fbig, fill="#111111")
    for i, (name, tc, gc, note, cr, png) in enumerate(tiles):
        x, y = (i % cols) * cw, 60 + (i // cols) * ch
        cell = Image.new("RGB", (cw - 16, ch - 16), "#FFFFFF")
        cd = ImageDraw.Draw(cell)
        im = Image.open(png).convert("RGBA")
        white = Image.new("RGBA", im.size, "#FFFFFF")
        im = Image.alpha_composite(white, im).convert("RGB")
        tx = (cw - 16 - 256) // 2
        cell.paste(im, (tx, 14))
        cd.rectangle([tx - 2, 12, tx + 257, 14 + 257], outline="#B0B0B6", width=2)
        ok = "OK" if cr >= 3.0 else "below 3:1"
        cd.text((16, 290), name, font=fname, fill="#111111")
        cd.text((16, 316), f"tile {tc}  ·  glyph {gc}", font=fsm, fill="#3A3A3C")
        cd.text((16, 342), f"{note}  ({ok})", font=fsm, fill="#3A3A3C")
        board.paste(cell, (x + 8, y + 8))
    out = os.path.join(OUT, "colour-board.png")
    board.save(out)
    print("board:", out)
    for t in tiles:
        print(f"  {t[0]:<14} {t[3]}  ratio={t[4]:.2f}")


if __name__ == "__main__":
    main()
