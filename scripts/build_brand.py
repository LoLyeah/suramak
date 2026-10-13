#!/usr/bin/env python3
"""Build every Suramak brand asset from the vector masters.

Masters (assets/logo/brand/*.svg) are bootstrapped from the approved concept
geometry on first run and are hand-kept afterwards — this script never
overwrites an existing master. All rasters under assets/logo/icons/** and
assets/logo/build/** are ALWAYS regenerated and then verified (IHDR
dimensions + colour probes). Never hand-edit a PNG.

Usage:  ~/.venvs/brand/bin/python scripts/build_brand.py [--font PATH]
"""
import argparse
import math
import os
import re
import struct
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CONCEPT = os.path.join(ROOT, "assets/logo/concepts/a-speak-to-text.svg")
BRAND = os.path.join(ROOT, "assets/logo/brand")
ICONS = os.path.join(ROOT, "assets/logo/icons")
BUILD = os.path.join(ROOT, "assets/logo/build")
DEFAULT_FONT = os.path.expanduser("~/.cache/suramak/fonts/inter-var.ttf")
WORD = "Suramak"

INK, SIGNAL, PAPER = "#111111", "#FF453A", "#FFFFFF"
PANE, PAPERBG = "#0b1228", "#F4F1EA"


def read(p):
    return open(p).read()


def write(p, s):
    os.makedirs(os.path.dirname(p), exist_ok=True)
    open(p, "w").write(s)


def rsvg(svg_path, w, h, out_png):
    os.makedirs(os.path.dirname(out_png), exist_ok=True)
    subprocess.run(["rsvg-convert", "-w", str(w), "-h", str(h), svg_path, "-o", out_png], check=True)


def svg_doc(body, vb):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vb}">{body}</svg>\n'


def inner_svg(path):
    return re.search(r"<svg[^>]*>(.*)</svg>", read(path), re.S).group(1)


def _font(bold, size):
    from PIL import ImageFont
    for p in (
        f"/usr/share/fonts/truetype/liberation/LiberationSans-{'Bold' if bold else 'Regular'}.ttf",
        f"/usr/share/fonts/truetype/dejavu/DejaVuSans{'-Bold' if bold else ''}.ttf",
    ):
        if os.path.exists(p):
            return ImageFont.truetype(p, size)
    raise SystemExit("no usable label font")


def wcag_lum(hexs):
    c = [int(hexs[i:i + 2], 16) / 255 for i in (1, 3, 5)]
    c = [x / 12.92 if x <= 0.04045 else ((x + 0.055) / 1.055) ** 2.4 for x in c]
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]


def wcag_ratio(a, b):
    la, lb = wcag_lum(a), wcag_lum(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


# ---------------------------------------------------------------- masters ----


def split_mark(svg_text):
    d = re.search(r'd="([^"]+)"', svg_text).group(1)
    parts = re.findall(r"M[^M]+", d)
    if len(parts) != 7:
        sys.exit(f"concept geometry changed: expected 7 subpaths, got {len(parts)}")
    tile, bars, rules = parts[0], " ".join(parts[1:4]), " ".join(parts[4:7])
    return tile, bars, rules


def bootstrap_masters():
    os.makedirs(BRAND, exist_ok=True)
    tile, bars, rules = split_mark(read(CONCEPT))
    made = []
    mark = os.path.join(BRAND, "mark.svg")
    if not os.path.exists(mark):
        write(mark, svg_doc(
            f'<path d="{tile}" fill="{INK}"/><path d="{bars}" fill="{SIGNAL}"/>'
            f'<path d="{rules}" fill="{PAPER}"/>', "0 0 256 256"))
        made.append("mark.svg")
    rev = os.path.join(BRAND, "mark-reversed.svg")
    if not os.path.exists(rev):
        write(rev, svg_doc(
            f'<path d="{tile}" fill="{PAPER}"/><path d="{bars}" fill="{SIGNAL}"/>'
            f'<path d="{rules}" fill="{INK}"/>', "0 0 256 256"))
        made.append("mark-reversed.svg")
    mono = os.path.join(BRAND, "mark-mono.svg")
    if not os.path.exists(mono):
        write(mono, read(CONCEPT))  # knockout: tile + real holes
        made.append("mark-mono.svg")
    bleed = os.path.join(BRAND, "mark-bleed.svg")
    if not os.path.exists(bleed):
        k = 256.0 / 208.0  # tile 24..232 -> 0..256 about the centre
        g = f"translate(128 128) scale({k:.6f}) translate(-128 -128)"
        write(bleed, svg_doc(
            f'<g transform="{g}"><path d="{tile}" fill="{INK}"/>'
            f'<path d="{bars}" fill="{SIGNAL}"/><path d="{rules}" fill="{PAPER}"/></g>',
            "0 0 256 256"))
        made.append("mark-bleed.svg")
    print("masters bootstrapped:", ", ".join(made) if made else "(all present)")


# --------------------------------------------------------------- wordmark ----


def wordmark_body(font_path, text, cap_px, wght=600):
    """Outlined word as an SVG <g> (baseline y=0, glyphs above), + metrics."""
    from fontTools.ttLib import TTFont
    from fontTools.varLib import instancer
    from fontTools.pens.svgPathPen import SVGPathPen
    from fontTools.pens.boundsPen import BoundsPen
    from fontTools.pens.transformPen import TransformPen
    from fontTools.misc.transform import Transform

    f = TTFont(font_path)
    if "fvar" in f:
        axes = {a.axisTag for a in f["fvar"].axes}
        loc = {k: v for k, v in {"wght": wght, "opsz": 28}.items() if k in axes}
        f = instancer.instantiateVariableFont(f, loc, inplace=False)
    gs = f.getGlyphSet()
    cmap = f.getBestCmap()
    hmtx = f["hmtx"]

    items, x = [], 0
    for ch in text:
        gname = cmap[ord(ch)]
        items.append((gname, x))
        x += hmtx[gname][0]
    total_u = x

    bp = BoundsPen(gs)
    gs[cmap[ord("S")]].draw(bp)
    cap_u = bp.bounds[3]
    sc = cap_px / cap_u

    parts = []
    for gname, gx in items:
        pen = SVGPathPen(gs)
        gs[gname].draw(TransformPen(pen, Transform(1, 0, 0, 1, gx, 0)))
        parts.append(pen.getCommands())
    d = " ".join(parts)
    body = f'<g transform="scale({sc:.6f} {(-sc):.6f})"><path d="{d}"/></g>'
    return body, total_u * sc, cap_px


def build_wordmark(font_path):
    body, w, cap = wordmark_body(font_path, WORD, 160)
    pad = 24
    W, H = math.ceil(w + 2 * pad), math.ceil(cap + 2 * pad)
    out = os.path.join(BRAND, "wordmark.svg")
    body = (body
            .replace('<g transform="scale(', f'<g transform="translate({pad} {pad + cap}) scale(')
            .replace("<path d=", f'<path fill="{INK}" d=', 1))
    write(out, svg_doc(body, f"0 0 {W} {H}"))
    return out, W, H


def build_lockups(font_path):
    sb = 0.875  # mark box scale: tile drawn 21..203 in a 224 box
    tile_l, tile_r = 24 * sb, 232 * sb
    gap = 44
    outs = []
    for name, mark_file, word_fill in (
        ("lockup-h.svg", "mark.svg", INK),
        ("lockup-h-reversed.svg", "mark-reversed.svg", PAPER),
    ):
        word, ww, cap = wordmark_body(font_path, WORD, 64)
        word = word.replace("<path d=", f'<path fill="{word_fill}" d=', 1)
        base_x = tile_r + gap
        base_y = 128 + cap * 0.375  # optical centre: lowercase mass sits above the baseline
        W = math.ceil(base_x + ww + 32)
        body = (f'<g transform="translate(0 16) scale({sb})">{inner_svg(os.path.join(BRAND, mark_file))}</g>'
                f'<g transform="translate({base_x:.1f} {base_y:.1f})">{word}</g>')
        out = os.path.join(BRAND, name)
        write(out, svg_doc(body, f"0 0 {W} 256"))
        outs.append((out, W))
    return outs


# ------------------------------------------------------------------ icons ----


def build_icons():
    bleed = os.path.join(BRAND, "mark-bleed.svg")
    macos = os.path.join(ICONS, "macos", "AppIcon.iconset")
    for size, name in [
        (16, "icon_16x16"), (32, "icon_16x16@2x"), (32, "icon_32x32"), (64, "icon_32x32@2x"),
        (128, "icon_128x128"), (256, "icon_128x128@2x"), (256, "icon_256x256"),
        (512, "icon_256x256@2x"), (512, "icon_512x512"), (1024, "icon_512x512@2x"),
    ]:
        rsvg(bleed, size, size, os.path.join(macos, name + ".png"))
    rsvg(bleed, 1024, 1024, os.path.join(ICONS, "icon-1024.png"))

    win = os.path.join(ICONS, "windows")
    wfiles = []
    for s in (16, 24, 32, 48, 64, 128, 256):
        p = os.path.join(win, f"suramak-{s}.png")
        rsvg(bleed, s, s, p)
        wfiles.append(p)
    subprocess.run(["magick", *wfiles, os.path.join(win, "suramak.ico")], check=True)

    linux = os.path.join(ICONS, "linux")
    for s in (16, 24, 32, 48, 64, 128, 256, 512):
        rsvg(bleed, s, s, os.path.join(linux, f"suramak-{s}.png"))

    web = os.path.join(ICONS, "web")
    for s in (16, 32, 48):
        rsvg(bleed, s, s, os.path.join(web, f"favicon-{s}.png"))
    subprocess.run(["magick",
                    os.path.join(web, "favicon-16.png"),
                    os.path.join(web, "favicon-32.png"),
                    os.path.join(web, "favicon-48.png"),
                    os.path.join(web, "favicon.ico")], check=True)


# ----------------------------------------------------------------- verify ----


def png_dims(path):
    b = open(path, "rb").read(33)
    if b[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit(f"{path}: not a PNG")
    return struct.unpack(">II", b[16:24])


def probe(path, points, tol=10):
    from PIL import Image
    im = Image.open(path).convert("RGBA")
    w, h = im.size
    for nx, ny, expect in points:
        px = im.getpixel((int(nx * w), int(ny * h)))
        exp = tuple(int(expect[i:i + 2], 16) for i in (1, 3, 5)) + (255,)
        if not all(abs(a - b) <= tol for a, b in zip(px, exp)):
            raise SystemExit(f"probe failed: {path} at ({nx},{ny}) = {px} expected {exp}")


def verify():
    base = os.path.join(ICONS, "macos", "AppIcon.iconset")
    for p, s in [
        (os.path.join(base, "icon_16x16.png"), 16),
        (os.path.join(base, "icon_512x512@2x.png"), 1024),
        (os.path.join(ICONS, "linux", "suramak-512.png"), 512),
    ]:
        got = png_dims(p)
        if got != (s, s):
            raise SystemExit(f"{p}: {got} != {(s, s)}")
    # probe points given in concept coords, mapped into the bleed frame
    k = 256 / 208

    def bl(x, y):
        return ((x - 128) * k + 128) / 256, ((y - 128) * k + 128) / 256

    pts = [(*bl(200, 41), INK), (*bl(52, 128), SIGNAL), (*bl(191, 98), PAPER)]
    probe(os.path.join(base, "icon_512x512@2x.png"), pts)
    probe(os.path.join(ICONS, "linux", "suramak-512.png"), pts)
    print("verify: dimensions + colour probes OK")


# ----------------------------------------------------------- review sheets ----


def build_review_assets():
    lockup = os.path.join(BRAND, "lockup-h.svg")
    m = re.search(r'viewBox="0 0 (\d+) (\d+)"', read(lockup))
    W, H = int(m.group(1)), int(m.group(2))
    rsvg(lockup, W * 2, H * 2, os.path.join(ICONS, "readme-logo.png"))

    from PIL import Image, ImageDraw
    fsm = _font(False, 17)
    panels = []
    for bg, mark, label in (
        (PANE, "mark-reversed.svg", "DARK surfaces → reversed (white tile)"),
        (PAPERBG, "mark.svg", "LIGHT surfaces → primary (ink tile)"),
    ):
        p = 560, 340
        im = Image.new("RGB", p, bg)
        tmp = os.path.join(BUILD, "_panel-mark.png")
        rsvg(os.path.join(BRAND, mark), 160, 160, tmp)
        mk = Image.open(tmp).convert("RGBA")
        white = Image.new("RGBA", mk.size, bg)
        im.paste(Image.alpha_composite(white, mk).convert("RGB"), (200, 80))
        d = ImageDraw.Draw(im)
        tile_col = PAPER if "reversed" in mark else INK
        r = wcag_ratio(tile_col, bg)
        d.text((24, 20), label, font=fsm, fill="#8a8f9c" if bg == PANE else "#5a5a5a")
        d.text((24, 46), f"tile {tile_col} vs bg {bg}  ·  {r:.1f}:1", font=fsm,
               fill="#8a8f9c" if bg == PANE else "#5a5a5a")
        panels.append(im)
    board = Image.new("RGB", (panels[0].width + panels[1].width + 12, 340), "#dddddd")
    board.paste(panels[0], (0, 0))
    board.paste(panels[1], (panels[0].width + 12, 0))
    board.save(os.path.join(BUILD, "theme-matrix.png"))
    print("review assets: readme-logo.png + theme-matrix.png")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--font", default=DEFAULT_FONT)
    args = ap.parse_args()
    if not os.path.exists(args.font):
        sys.exit(f"font not found: {args.font} (fetch Inter once)")
    os.makedirs(BUILD, exist_ok=True)

    bootstrap_masters()
    wm, W, H = build_wordmark(args.font)
    print(f"wordmark.svg  {W}x{H}")
    for out, W in build_lockups(args.font):
        print(f"{os.path.basename(out)}  width {W}")
    build_icons()
    verify()
    build_review_assets()
    print("done")


if __name__ == "__main__":
    main()
