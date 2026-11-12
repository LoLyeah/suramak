#!/usr/bin/env python3
"""Build the bundled OFL faces (WORKORDER-M20A).

The phone used to render egui's baked-in Ubuntu-Light/Hack because
`theme::load_system_fonts` is macOS-only. epaint 0.33 rasterises with
`ab_glyph`, which cannot interpolate a variable font's `wght` axis or apply
OpenType features at runtime — so the faces are **instanced offline** (one
static file per weight) and **subsetted** to the app's coverage before they
are committed.

This script is the only way the files under `assets/fonts/` are produced:

    /usr/bin/python3 scripts/fonts/build_fonts.py            # write the assets
    /usr/bin/python3 scripts/fonts/build_fonts.py --check     # rebuild in a
                                                             # temp dir and
                                                             # compare bytes

It needs `fonttools` (4.68 on the dev VPS) and nothing else — no crates, no
network once the upstream files are cached. The upstream variable fonts are
pinned by sha256; a changed upstream is a deliberate decision, not a silent
drift.

Every produced file is loaded back with `fontTools.ttLib.TTFont` and its cmap
checked for the Indonesian/Latin floor before it is accepted, and each file's
sha256 is printed. `PROVENANCE.md` is generated from the same table so the
committed note cannot disagree with the committed bytes.
"""

from __future__ import annotations

import argparse
import hashlib
import shutil
import sys
import tempfile
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

try:
    from fontTools import subset
    from fontTools.ttLib import TTFont
    from fontTools.varLib.instancer import instantiateVariableFont
except ImportError:  # pragma: no cover - operator error, not a code path
    sys.exit("fonttools is required: /usr/bin/python3 -m pip install fonttools")

REPO = Path(__file__).resolve().parents[2]
ASSETS = REPO / "assets" / "fonts"
CACHE = REPO / "target" / "font-build-cache"

#: The coverage the Deck promises: Basic Latin through Latin Extended-B
#: (Indonesian is pure Latin; the extended ranges carry the loanword
#: diacritics and the Eastern-European neighbours), General Punctuation
#: (curly quotes, dashes, ellipsis), euro, trademark, and the four arrows the
#: diagnostics rows use.
UNICODES = "U+0020-024F,U+2000-206F,U+20AC,U+2122,U+2190-2193"

#: Hard ceiling from the workorder — a face over this fails the build.
MAX_BYTES = 120 * 1024

#: Characters the script refuses to ship a face without. ASCII printable
#: covers Indonesian prose and the UI's labels; the Latin-1 letters cover the
#: loanword accents; the punctuation is what the app actually typesets.
REQUIRED = (
    "".join(chr(c) for c in range(0x20, 0x7F))
    + "ÀÁÂÃÄÅÆÇÈÉÊËÌÍÎÏÐÑÒÓÔÕÖØÙÚÛÜÝÞß"
    + "àáâãäåæçèéêëìíîïðñòóôõöøùúûüýþÿ"
    + "–—‘’“”…€™"
)


@dataclass(frozen=True)
class Upstream:
    key: str
    url: str
    filename: str
    sha256: str
    version: str
    copyright: str


@dataclass(frozen=True)
class Face:
    filename: str
    upstream: str
    #: The instance location: every axis of the variable font pinned.
    location: dict[str, float]
    family: str
    style: str
    weight: int
    #: What the app uses it for (the provenance note's role column).
    role: str
    #: Whether the face must carry the arrow + extended-Latin coverage. The
    #: display serif and the mono are earned faces with a narrower original
    #: coverage; the script reports what they carry instead of failing.
    full_coverage: bool = True


UPSTREAM = {
    "inter": Upstream(
        key="inter",
        url="https://github.com/google/fonts/raw/main/ofl/inter/Inter%5Bopsz,wght%5D.ttf",
        filename="Inter%5Bopsz,wght%5D.ttf",
        sha256="29160a80ff49ddcab2c97711247e08b1fab27a484a329ce8b813d820dc559031",
        version="Version 4.001;git-66647c0bb",
        copyright="Copyright 2020 The Inter Project Authors (https://github.com/rsms/inter)",
    ),
    "newsreader": Upstream(
        key="newsreader",
        url="https://github.com/google/fonts/raw/main/ofl/newsreader/Newsreader%5Bopsz,wght%5D.ttf",
        filename="Newsreader%5Bopsz,wght%5D.ttf",
        sha256="8a08d13f8a6c0d51be379a60af84f945f65369a67e509ee3c3bdcc421254d7c1",
        version="Version 1.003",
        copyright="Copyright 2020 The Newsreader Project Authors (http://github.com/productiontype/Newsreader)",
    ),
    "geistmono": Upstream(
        key="geistmono",
        url="https://github.com/google/fonts/raw/main/ofl/geistmono/GeistMono%5Bwght%5D.ttf",
        filename="GeistMono%5Bwght%5D.ttf",
        sha256="d00e590b8eb3a59acc329b2d044fd143ae935090b7da33199ebee27cc7de8196",
        version="Version 1.701",
        copyright="Copyright 2024 The Geist Project Authors (https://github.com/vercel/geist-font.git)",
    ),
}

FACES = (
    Face(
        filename="Inter-Regular.ttf",
        upstream="inter",
        location={"wght": 400, "opsz": 14},
        family="Inter",
        style="Regular",
        weight=400,
        role="Proportional — body, row values, captions",
    ),
    Face(
        filename="Inter-Medium.ttf",
        upstream="inter",
        location={"wght": 500, "opsz": 14},
        family="Inter",
        style="Medium",
        weight=500,
        role="SansMedium — row titles, numerals",
    ),
    Face(
        filename="Inter-SemiBold.ttf",
        upstream="inter",
        location={"wght": 600, "opsz": 14},
        family="Inter",
        style="SemiBold",
        weight=600,
        role="SansSemiBold — screen titles, section labels",
    ),
    Face(
        filename="Newsreader-Regular.ttf",
        upstream="newsreader",
        location={"wght": 400, "opsz": 18},
        family="Newsreader",
        style="Regular",
        weight=400,
        role="serif_family() — editorial display",
        full_coverage=False,
    ),
    Face(
        filename="GeistMono-Regular.ttf",
        upstream="geistmono",
        location={"wght": 400},
        family="Geist Mono",
        style="Regular",
        weight=400,
        role="Monospace — diagnostics, hashes, paths",
        full_coverage=False,
    ),
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def unicodes(spec: str) -> set[int]:
    """Parse `U+0020-024F,U+20AC` into a codepoint set."""
    out: set[int] = set()
    for part in spec.split(","):
        part = part.strip().removeprefix("U+")
        if "-" in part:
            low, high = part.split("-")
            out.update(range(int(low.removeprefix("U+"), 16), int(high.removeprefix("U+"), 16) + 1))
        else:
            out.add(int(part, 16))
    return out


def fetch(upstream: Upstream, cache: Path, offline: bool) -> Path:
    cache.mkdir(parents=True, exist_ok=True)
    path = cache / upstream.filename
    if not path.exists():
        if offline:
            sys.exit(
                f"{upstream.filename} is not cached ({path}) and --offline was given"
            )
        print(f"fetch  {upstream.url}")
        with urllib.request.urlopen(upstream.url, timeout=120) as response:  # noqa: S310
            path.write_bytes(response.read())
    actual = sha256(path)
    if actual != upstream.sha256:
        sys.exit(
            f"{upstream.filename}: upstream sha256 {actual} does not match the "
            f"pinned {upstream.sha256} — the upstream changed; update the pin "
            f"(and the version) deliberately"
        )
    return path


def rename(font: TTFont, family: str, style: str) -> None:
    """Write `family` / `style` into the name table.

    The upstream variable font's name table describes a variable family
    ("Newsreader 16pt", "Inter"). A static instance must describe itself:
    otherwise a font inspector (and the workorder's provenance check) sees a
    file that claims to be something it is not. `updateFontNames=True` in the
    instancer cannot be used — it requires a STAT AxisValue record for every
    pinned location, and the display faces do not carry one for their optical
    size. The five IDs below are the whole public name surface.
    """
    values = {
        1: family,
        2: style,
        4: f"{family} {style}",
        6: f"{family.replace(' ', '')}-{style}",
        16: family,
        17: style,
    }
    name = font["name"]
    for record in list(name.names):
        if record.nameID in values:
            name.setName(
                values[record.nameID],
                record.nameID,
                record.platformID,
                record.platEncID,
                record.langID,
            )
    # The variations PostScript name prefix describes the variable font.
    name.removeNames(nameID=25)


def build(face: Face, source: Path, out_dir: Path) -> Path:
    """Instance the variable font to one static weight, then subset it."""
    # `recalcTimestamp=False` keeps the upstream `head.modified`: fontTools
    # otherwise stamps the current time on every save, which would make two
    # builds of the same input differ (and `--check` meaningless).
    font = TTFont(source, recalcTimestamp=False)
    # Pin every axis.
    instantiateVariableFont(
        font, face.location, inplace=True, updateFontNames=False
    )
    rename(font, face.family, face.style)

    options = subset.Options()
    # ab_glyph — epaint 0.33's rasteriser — reads advances, kerning from the
    # legacy `kern` table (none of these families has one) and glyph outlines.
    # It never applies GSUB/GPOS, so shipping them is dead weight.
    options.hinting = False
    options.layout_features = []
    options.drop_tables += [
        "DSIG",
        "STAT",
        "GDEF",
        "GPOS",
        "GSUB",
        "MVAR",
        "HVAR",
        "avar",
        "fvar",
        "gvar",
        "meta",
    ]
    options.notdef_outline = True
    options.notdef_glyph = True
    options.name_IDs = [0, 1, 2, 3, 4, 5, 6]

    subsetter = subset.Subsetter(options=options)
    subsetter.populate(unicodes=unicodes(UNICODES))
    subsetter.subset(font)

    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / face.filename
    font.save(path)
    return path


def verify(face: Face, path: Path) -> tuple[int, int, str]:
    """Load the produced file and prove it is a usable static face."""
    size = path.stat().st_size
    if size > MAX_BYTES:
        sys.exit(f"{face.filename}: {size} bytes is over the {MAX_BYTES}-byte ceiling")

    font = TTFont(path)
    if "fvar" in font:
        sys.exit(f"{face.filename}: still a variable font (fvar survived)")
    upem = font["head"].unitsPerEm
    if upem not in (1000, 2048, 2000):
        sys.exit(f"{face.filename}: unexpected unitsPerEm {upem}")

    cmap = font.getBestCmap()
    missing = [ch for ch in REQUIRED if ord(ch) not in cmap]
    if missing:
        sys.exit(f"{face.filename}: missing required characters {missing!r}")

    family = font["name"].getDebugName(1) or ""
    if face.family not in family:
        sys.exit(f"{face.filename}: name table says {family!r}, expected {face.family!r}")

    covered = len(cmap)
    return size, covered, family


def provenance(rows: list[dict[str, str]]) -> str:
    lines = [
        "# Bundled fonts — provenance",
        "",
        "Generated by `scripts/fonts/build_fonts.py`; do not edit by hand.",
        "Run `/usr/bin/python3 scripts/fonts/build_fonts.py --check` to prove",
        "the committed files are exactly what the script builds.",
        "",
        "All five faces are SIL Open Font License 1.1 (see `OFL.txt`).",
        "Each is a **static instance** of the upstream variable font (epaint",
        "0.33 uses `ab_glyph`: no variable axes, no OpenType features at",
        "runtime) subsetted to:",
        "",
        f"`{UNICODES}`",
        "",
        "| file | family / style | weight | role | upstream | version | instance | upstream sha256 | subset sha256 | bytes | glyphs |",
        "|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for row in rows:
        lines.append(
            "| `{file}` | {family} {style} | {weight} | {role} | [{up}]({url}) | {version} | `{loc}` | `{upsha}` | `{sha}` | {bytes} | {glyphs} |".format(
                **row
            )
        )
    lines += [
        "",
        "## Why these settings",
        "",
        "* **Optical size pinned at 14** for Inter: 14 is the axis's UI end",
        "  (its range is 14–32), which is where the 11–22 pt UI range lives; one",
        "  static face keeps the wrap identical across every screen. Newsreader is",
        "  pinned at 18, its family default, for the 21 pt editorial display.",
        "* **Subset without hinting**: `ab_glyph_rasterizer` does no TrueType",
        "  hinting, so the bytecode would only add weight.",
        "* **No GPOS/GSUB**: the rasteriser reads advances and glyph outlines;",
        "  kerning comes from the legacy `kern` table, which none of these",
        "  families ships. The plan's `cv05` / `tnum` bakes would need a glyph",
        "  rewrite, not a feature toggle — timecode columns use",
        "  `type_scale::numeral_cell()` instead (Inter Medium's own tabular",
        "  advance, 0.6479 em).",
        "* **Newsreader and Geist Mono carry a narrower upstream repertoire**",
        "  (no arrows, partial Latin Extended-B). They are earned display/mono",
        "  faces; missing characters fall through to the proportional stack.",
        "",
    ]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Build the bundled OFL static + subset font faces."
    )
    parser.add_argument(
        "--cache-dir",
        type=Path,
        default=CACHE,
        help=f"upstream variable-font cache (default: {CACHE})",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=ASSETS,
        help="where the faces are written (default: assets/fonts)",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="build into a temp dir and compare with the committed assets; "
        "never writes assets/fonts",
    )
    parser.add_argument(
        "--offline",
        action="store_true",
        help="never download; fail if an upstream file is not cached",
    )
    args = parser.parse_args()

    if args.check:
        tmp = Path(tempfile.mkdtemp(prefix="suramak-fonts-"))
        out_dir = tmp
    else:
        tmp = None
        out_dir = args.out_dir

    rows: list[dict[str, str]] = []
    built: list[tuple[Face, Path]] = []

    for face in FACES:
        upstream = UPSTREAM[face.upstream]
        source = fetch(upstream, args.cache_dir, args.offline)
        path = build(face, source, out_dir)
        size, glyphs, family = verify(face, path)
        built.append((face, path))
        rows.append(
            {
                "file": face.filename,
                "family": family,
                "style": face.style,
                "weight": str(face.weight),
                "role": face.role,
                "up": upstream.filename.replace("%5B", "[").replace("%5D", "]"),
                "url": upstream.url,
                "version": upstream.version,
                "loc": ", ".join(
                    f"{axis}={value:g}" for axis, value in face.location.items()
                ),
                "upsha": upstream.sha256,
                "sha": sha256(path),
                "bytes": f"{size:,}",
                "glyphs": str(glyphs),
            }
        )
        print(f"{face.filename:<26} {size:>7} B  {glyphs:>4} glyphs  {rows[-1]['sha']}")

    print()
    if args.check:
        drifted = []
        for face, path in built:
            committed = ASSETS / face.filename
            if not committed.exists():
                drifted.append(f"{face.filename}: missing from assets/fonts")
            elif committed.read_bytes() != path.read_bytes():
                drifted.append(
                    f"{face.filename}: committed {sha256(committed)[:16]} != "
                    f"built {sha256(path)[:16]}"
                )
        if tmp is not None:
            shutil.rmtree(tmp, ignore_errors=True)
        if drifted:
            print("DRIFT:")
            for line in drifted:
                print("  " + line)
            return 1
        print(f"ok: the {len(built)} committed faces are reproducible byte for byte")
        return 0

    note = out_dir / "PROVENANCE.md"
    note.write_text(provenance(rows), encoding="utf-8")
    print(f"wrote {note.relative_to(REPO) if note.is_relative_to(REPO) else note}")

    # The license travels with the faces: when rebuilding into a different
    # directory (a review copy, a temp dir), copy the committed OFL.txt too so
    # the output directory is a complete, distributable set.
    license_path = out_dir / "OFL.txt"
    if not license_path.exists() and (ASSETS / "OFL.txt").exists():
        shutil.copyfile(ASSETS / "OFL.txt", license_path)
    text = license_path.read_text(encoding="utf-8") if license_path.exists() else ""
    if "SIL OPEN FONT LICENSE Version 1.1" not in text:
        sys.exit(f"{license_path} is missing the OFL 1.1 text")
    for upstream in UPSTREAM.values():
        if upstream.copyright not in text:
            sys.exit(f"{license_path} is missing the notice: {upstream.copyright}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
