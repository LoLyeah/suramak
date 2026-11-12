#!/usr/bin/env python3
"""M13I — prove an Android screenshot is the compact Library, not a blank screen.

The emulator smoke has no OCR and no image library on the runner: this script
decodes the PNG with the standard library (zlib) and measures the properties
that only the app's own painted surface has:

  * not the launcher (the shot differs from the pre-launch baseline);
  * not blank (colour variety + luminance spread);
  * the app's material ladder (the modal colour is the compact theme's window
    or panel tone, light or dark);
  * layered chrome (several horizontal bands with distinct modal colours: the
    app bar, the search/card area, the bottom action bar);
  * text/controls in the top and bottom bands (high-contrast pixels against
    the band's own modal colour).

A black surface, a white surface, a crashed-to-launcher screen or a system
dialog all fail at least one of these. The thresholds were calibrated on the
deterministic compact renders in `tests/ui-compact/shots/` (the same egui
surface, 1216x2640): `--self-test` re-checks that calibration plus two
synthetic negative controls, so the gate cannot silently stop biting.

Usage:
  inspect_screencap.py --shot build/android-smoke/library.png \\
                       --baseline build/android-smoke/launcher.png
  inspect_screencap.py --diff before.png after.png --min-mean-diff 0.4 \\
                       --crop-top 98
  inspect_screencap.py --self-test

The `--diff` mode optionally drops the top `--crop-top` pixel rows from **both**
frames before comparing (M19A2). That band is the Android status bar: its clock
rolls over, its battery/network icons redraw, and a still-frame negative control
that sees those pixels can flake even when nothing in the app moved. The band's
height comes from the shell's own top inset times the frame's pixels-per-point
(see `scripts/android/emulator-smoke.sh`), so what is cropped is exactly what
the app does not own.
"""

import argparse
import json
import struct
import sys
import zlib

# The compact material ladder (crates/suramak-ui/src/deck/theme.rs): the
# window / panel tones a full screen of compact chrome is painted in. Cards
# are excluded on purpose — a white or near-black card alone is not the app.
LIGHT_TONES = [(0xE9, 0xEB, 0xF0), (0xF4, 0xF6, 0xFA), (0xED, 0xEF, 0xF4)]
DARK_TONES = [(0x0E, 0x15, 0x26), (0x17, 0x20, 0x3A), (0x12, 0x1A, 0x30)]
APP_TONES = LIGHT_TONES + DARK_TONES

MIN_WIDTH = 480
MIN_HEIGHT = 800
PALETTE_TOLERANCE = 14        # per-channel; the emulator dithers slightly
MODAL_MIN_SHARE = 0.20        # the window tone owns the screen
MIN_DISTINCT_QUANTA = 24      # 4-bit-per-channel buckets
MIN_STDDEV = 10.0
MIN_BAND_DIFF = 8.0           # mean |ΔL| between the shot and the launcher
MIN_CONTRAST_SHARE = 0.004    # 0.4% of a band is text/controls
# The light ladder's app bar (#EDEFF4) and window (#E9EBF0) are three levels
# apart, so a real Library yields 2 distinct band tones; a flat fill yields 1.
MIN_DISTINCT_BANDS = 2
# A status-bar crop may never remove more than half the frame (M19A2): beyond
# that the "still frame" it is meant to rescue has no evidence left in it.
MIN_KEPT_SHARE = 0.5


def read_png(path):
    """Return (width, height, channels, pixels) for an 8-bit PNG."""
    with open(path, "rb") as handle:
        data = handle.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path}: not a PNG")
    pos = 8
    idat = []
    width = height = depth = color_type = interlace = None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        chunk = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            width, height, depth, color_type, _comp, _filt, interlace = struct.unpack(
                ">IIBBBBB", chunk
            )
        elif kind == b"IDAT":
            idat.append(chunk)
        elif kind == b"IEND":
            break
    if depth != 8 or interlace != 0:
        raise ValueError(f"{path}: only 8-bit non-interlaced PNGs are supported")
    if width is None or height is None or color_type is None:
        raise ValueError(f"{path}: missing IHDR")
    channels = {0: 1, 2: 3, 4: 2, 6: 4}.get(color_type)
    if channels is None:
        raise ValueError(f"{path}: unsupported colour type {color_type}")

    raw = zlib.decompress(b"".join(idat))
    stride = width * channels
    out = bytearray(height * stride)
    prev = bytearray(stride)
    src = 0
    for y in range(height):
        filter_type = raw[src]
        src += 1
        line = bytearray(raw[src : src + stride])
        src += stride
        if filter_type == 1:  # Sub
            for i in range(channels, stride):
                line[i] = (line[i] + line[i - channels]) & 0xFF
        elif filter_type == 2:  # Up
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif filter_type == 3:  # Average
            for i in range(stride):
                left = line[i - channels] if i >= channels else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif filter_type == 4:  # Paeth
            for i in range(stride):
                left = line[i - channels] if i >= channels else 0
                up = prev[i]
                up_left = prev[i - channels] if i >= channels else 0
                estimate = left + up - up_left
                da = abs(estimate - left)
                db = abs(estimate - up)
                dc = abs(estimate - up_left)
                if da <= db and da <= dc:
                    predictor = left
                elif db <= dc:
                    predictor = up
                else:
                    predictor = up_left
                line[i] = (line[i] + predictor) & 0xFF
        elif filter_type != 0:
            raise ValueError(f"{path}: bad PNG filter {filter_type}")
        out[y * stride : (y + 1) * stride] = line
        prev = line
    return width, height, channels, bytes(out)


def luminance(pixels, channels):
    """Yield row-major luminance bytes (0.2126R + 0.7152G + 0.0722B)."""
    if channels == 1:
        return pixels
    if channels == 2:
        return pixels[0::2]
    step = channels
    luma = bytearray(len(pixels) // step)
    for index in range(len(luma)):
        base = index * step
        luma[index] = (
            54 * pixels[base] + 183 * pixels[base + 1] + 19 * pixels[base + 2]
        ) >> 8
    return bytes(luma)


def quantize(pixels, channels):
    step = channels
    count = len(pixels) // step
    buckets = {}
    for index in range(count):
        base = index * step
        key = ((pixels[base] >> 4) << 8) | ((pixels[base + 1] >> 4) << 4) | (
            pixels[base + 2] >> 4
        )
        buckets[key] = buckets.get(key, 0) + 1
    return buckets


def nearest_app_tone(color):
    best = None
    for tone in APP_TONES:
        distance = max(abs(color[i] - tone[i]) for i in range(3))
        if best is None or distance < best[1]:
            best = (tone, distance)
    return best


def band_stats(luma, width, height, top, bottom, rows):
    """Modal colour proxy + high-contrast share for one horizontal band."""
    start = int(height * top)
    end = int(height * bottom)
    histogram = [0] * 256
    for y in range(start, end):
        row = luma[y * width : (y + 1) * width]
        for value in row:
            histogram[value] += 1
    total = sum(histogram) if histogram else 0
    if total == 0:
        return None
    modal = max(range(256), key=histogram.__getitem__)
    contrast = 0
    for value, count in enumerate(histogram):
        if abs(value - modal) >= 60:
            contrast += count
    return {
        "rows": rows,
        "modal_luma": modal,
        "contrast_share": contrast / total,
        "modal_share": histogram[modal] / total,
    }


def inspect(shot_path, baseline_path=None):
    width, height, channels, pixels = read_png(shot_path)
    results = []
    metrics = {
        "path": shot_path,
        "width": width,
        "height": height,
        "channels": channels,
    }

    ok = width >= MIN_WIDTH and height >= MIN_HEIGHT and height > width
    results.append(("portrait phone resolution", ok, f"{width}x{height}"))

    buckets = quantize(pixels, channels)
    total = sum(buckets.values())
    modal_key, modal_count = max(buckets.items(), key=lambda item: item[1])
    # The bucket centre, not its lower edge: 4-bit truncation alone can put the
    # lower edge at the tolerance limit for the exact theme tone.
    modal_rgb = (
        ((modal_key >> 8) << 4) | 0x08,
        (((modal_key >> 4) & 0xF) << 4) | 0x08,
        ((modal_key & 0xF) << 4) | 0x08,
    )
    modal_share = modal_count / total
    metrics["modal_rgb"] = list(modal_rgb)
    metrics["modal_share"] = round(modal_share, 4)
    metrics["distinct_quanta"] = len(buckets)

    tone, distance = nearest_app_tone(modal_rgb)
    palette_ok = modal_share >= MODAL_MIN_SHARE and distance <= PALETTE_TOLERANCE
    results.append(
        (
            "compact theme is the dominant surface",
            palette_ok,
            f"modal={tuple(modal_rgb)} share={modal_share:.3f} "
            f"nearest_tone={tuple(tone)} distance={distance}",
        )
    )

    results.append(
        (
            "not a flat fill",
            len(buckets) >= MIN_DISTINCT_QUANTA,
            f"{len(buckets)} distinct 4-bit colours (need {MIN_DISTINCT_QUANTA})",
        )
    )

    luma = luminance(pixels, channels)
    mean = sum(luma) / len(luma)
    variance = sum((value - mean) ** 2 for value in luma) / len(luma)
    stddev = variance ** 0.5
    metrics["luma_stddev"] = round(stddev, 2)
    results.append(("luminance spread", stddev >= MIN_STDDEV, f"stddev={stddev:.1f}"))

    top = band_stats(luma, width, height, 0.0, 0.10, "top")
    bottom = band_stats(luma, width, height, 0.90, 1.0, "bottom")
    middle = band_stats(luma, width, height, 0.35, 0.65, "middle")
    for name, stats in (("top", top), ("bottom", bottom)):
        ok_band = stats is not None and stats["contrast_share"] >= MIN_CONTRAST_SHARE
        results.append(
            (
                f"{name} band carries text/controls",
                ok_band,
                "no pixels"
                if stats is None
                else f"contrast_share={stats['contrast_share']:.4f} "
                f"(need {MIN_CONTRAST_SHARE})",
            )
        )
    metrics["bands"] = {"top": top, "middle": middle, "bottom": bottom}

    # Layered chrome: the app bar, the content surface and the bottom bar have
    # different modal tones. A single flat fill yields one distinct band.
    band_modals = []
    for index in range(20):
        stats = band_stats(luma, width, height, index / 20.0, (index + 1) / 20.0, f"{index}")
        if stats is not None:
            band_modals.append(stats["modal_luma"] // 12)
    distinct_bands = len(set(band_modals))
    metrics["distinct_band_tones"] = distinct_bands
    results.append(
        (
            "layered chrome (app bar / content / bottom bar)",
            distinct_bands >= MIN_DISTINCT_BANDS,
            f"{distinct_bands} distinct band tones (need {MIN_DISTINCT_BANDS})",
        )
    )

    if baseline_path:
        base_w, base_h, base_channels, base_pixels = read_png(baseline_path)
        if (base_w, base_h) != (width, height):
            results.append(
                (
                    "baseline resolution matches",
                    False,
                    f"baseline {base_w}x{base_h} vs shot {width}x{height}",
                )
            )
        else:
            base_luma = luminance(base_pixels, base_channels)
            diff = sum(abs(a - b) for a, b in zip(luma, base_luma)) / len(luma)
            metrics["baseline_mean_abs_diff"] = round(diff, 2)
            results.append(
                (
                    "app replaced the launcher",
                    diff >= MIN_BAND_DIFF,
                    f"mean |ΔL|={diff:.1f} (need {MIN_BAND_DIFF})",
                )
            )

    return results, metrics


def write_solid_png(path, width, height, rgba):
    write_png(path, width, height, rgba)


def write_png(path, width, height, background, patches=()):
    """Write an RGBA PNG: one solid colour plus a few opaque rectangles.

    The synthetic frames the self-test needs are a flat app surface plus the
    status-bar band's moving parts (a clock, a battery icon) — no image library
    is on the CI runner, and this is all a cropper test wants.
    """
    pixel = bytes(background)
    rows = [bytearray(pixel * width) for _ in range(height)]
    for x, y, rect_w, rect_h, colour in patches:
        band = bytes(colour) * rect_w
        for row_y in range(max(0, y), min(height, y + rect_h)):
            start = max(0, x) * len(pixel)
            rows[row_y][start : start + len(band)] = band
    raw = b"".join(b"\x00" + bytes(row) for row in rows)
    compressed = zlib.compress(raw, 6)

    def chunk(kind, payload):
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
        )

    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    with open(path, "wb") as handle:
        handle.write(b"\x89PNG\r\n\x1a\n")
        handle.write(chunk(b"IHDR", header))
        handle.write(chunk(b"IDAT", compressed))
        handle.write(chunk(b"IEND", b""))


def self_test(reference_dir):
    failures = []

    def expect(name, results, should_pass):
        passed = all(result[1] for result in results)
        verdict = "PASS" if passed else "FAIL"
        wanted = "PASS" if should_pass else "FAIL"
        print(f"[self-test] {name}: {verdict} (wanted {wanted})")
        for label, ok, detail in results:
            print(f"    {'ok  ' if ok else 'FAIL'} {label}: {detail}")
        if passed != should_pass:
            failures.append(f"{name}: got {verdict}, wanted {wanted}")

    for shot in ("first-run-download.png", "launch-library.png", "library-light.png"):
        path = f"{reference_dir}/{shot}"
        results, _ = inspect(path)
        expect(f"positive control {shot}", results, True)

    import tempfile
    import os

    with tempfile.TemporaryDirectory() as tmp:
        black = os.path.join(tmp, "black.png")
        white = os.path.join(tmp, "white.png")
        write_solid_png(black, 1080, 2400, (0, 0, 0, 255))
        write_solid_png(white, 1080, 2400, (255, 255, 255, 255))
        for name, path in (("black frame", black), ("white frame", white)):
            results, _ = inspect(path)
            expect(f"negative control {name}", results, False)

        # The scroll proof's comparator (M19A): two captures of the same frame
        # are 0; the same screen against a blank frame is far above the
        # smoke's 0.4 threshold. A comparator that always says "different"
        # would let a stuck list pass the swipe assertion.
        same = os.path.join(reference_dir, "library.png")
        identical = mean_abs_luma_diff(same, same)
        print(f"[self-test] diff identical frames: {identical:.3f} (want 0)")
        if identical != 0.0:
            failures.append(f"diff of identical frames is {identical}, want 0")
        # A blank frame at the reference's own size: the comparator must see it
        # as a different frame by a wide margin.
        blank = os.path.join(tmp, "blank-same-size.png")
        ref_w, ref_h = read_png(same)[:2]
        write_solid_png(blank, ref_w, ref_h, (0, 0, 0, 255))
        differs = mean_abs_luma_diff(same, blank)
        print(f"[self-test] diff library vs black: {differs:.3f} (want > 0.4)")
        if differs <= 0.4:
            failures.append(f"diff of library vs black is {differs}, want > 0.4")

        # M19A2: the status-bar crop. Two captures of a still screen whose only
        # difference is the clock — a minute rollover moves the digits — must
        # move the comparator while uncropped (that is the CI flake this
        # exists for) and compare EXACTLY equal once the band is cropped.
        status_h = 96
        background = (14, 21, 38, 255)
        clock = (236, 238, 244, 255)
        clock_a = os.path.join(tmp, "clock-a.png")
        clock_b = os.path.join(tmp, "clock-b.png")
        write_png(clock_a, 1080, 2400, background, [(760, 18, 96, 60, clock)])
        write_png(clock_b, 1080, 2400, background, [(700, 18, 96, 60, clock)])
        uncropped = mean_abs_luma_diff(clock_a, clock_b)
        cropped = mean_abs_luma_diff(clock_a, clock_b, crop_top=status_h)
        print(
            f"[self-test] ticking clock: uncropped {uncropped:.3f} (want > 0.05), "
            f"cropped {cropped:.3f} (want 0)"
        )
        if uncropped <= 0.05:
            failures.append(
                f"a ticking clock must fail the uncropped still-frame control, got {uncropped}"
            )
        if cropped != 0.0:
            failures.append(f"the cropped still frame differs by {cropped}, want 0")

        # A crop that would empty (or gut) the frame is refused, not silently
        # turned into a comparison of nothing.
        for bad in (-1, 2400, 2401, 1300):
            try:
                mean_abs_luma_diff(clock_a, clock_b, crop_top=bad)
            except ValueError:
                continue
            failures.append(f"crop_top={bad} should have been refused")

    if failures:
        for failure in failures:
            print(f"[self-test] FAILED: {failure}", file=sys.stderr)
        return 1
    print("[self-test] OK: positives pass, blanks fail")
    return 0


def crop_status_band(width, height, channels, pixels, crop_top):
    """Drop the top `crop_top` pixel rows from a decoded frame.

    `crop_top` is the status-bar band, in pixels, measured from the top edge.
    A crop that leaves nothing to compare — or less than half the frame — is
    refused: a diff over an empty or gutted image cannot witness anything, and
    a silently oversized crop would turn the still-frame control into a
    tautology (M19A2).
    """
    if crop_top < 0:
        raise ValueError(f"--crop-top must be >= 0, got {crop_top}")
    if crop_top >= height:
        raise ValueError(
            f"--crop-top {crop_top} would leave no pixels of a {height}-row frame"
        )
    if height - crop_top < height * MIN_KEPT_SHARE:
        raise ValueError(
            f"--crop-top {crop_top} keeps only {height - crop_top} of {height} rows "
            f"(less than {MIN_KEPT_SHARE:.0%} of the frame) — refusing to crop the evidence away"
        )
    stride = width * channels
    return pixels[crop_top * stride :]


def mean_abs_luma_diff(shot_path, other_path, crop_top=0):
    """Mean |ΔL| between two same-size screenshots (0 = identical frames).

    The emulator smoke's scroll proof: the compact surface is one OS window, so
    uiautomator cannot see the list move — the pixels are the only witness.
    `crop_top` removes the status-bar band from both frames first (M19A2).
    """
    width, height, channels, pixels = read_png(shot_path)
    other_w, other_h, other_channels, other_pixels = read_png(other_path)
    if (width, height) != (other_w, other_h):
        raise ValueError(
            f"resolution mismatch: {shot_path} {width}x{height} vs "
            f"{other_path} {other_w}x{other_h}"
        )
    left = luminance(crop_status_band(width, height, channels, pixels, crop_top), channels)
    right = luminance(
        crop_status_band(width, height, other_channels, other_pixels, crop_top),
        other_channels,
    )
    return sum(abs(a - b) for a, b in zip(left, right)) / len(left)


def diff_main(args):
    try:
        diff = mean_abs_luma_diff(args.diff[0], args.diff[1], args.crop_top)
    except ValueError as error:
        print(f"SCREENCAP-DIFF-FAIL: {error}", file=sys.stderr)
        return 1
    width, height = read_png(args.diff[0])[:2]
    band = (
        f"; status-bar band rows 0..{args.crop_top} cropped from both frames "
        f"({width}x{height - args.crop_top} compared)"
        if args.crop_top
        else f"; full frame ({width}x{height} compared)"
    )
    detail = f"mean |ΔL|={diff:.3f} between {args.diff[0]} and {args.diff[1]}{band}"
    if args.json:
        with open(args.json, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "mean_abs_luma_diff": round(diff, 4),
                    "a": args.diff[0],
                    "b": args.diff[1],
                    "crop_top_px": args.crop_top,
                    "compared_rows": height - args.crop_top,
                },
                handle,
                indent=2,
            )
    if args.min_mean_diff is not None and diff < args.min_mean_diff:
        print(f"SCREENCAP-DIFF-FAIL: {detail} (need >= {args.min_mean_diff})")
        return 1
    if args.max_mean_diff is not None and diff > args.max_mean_diff:
        print(f"SCREENCAP-DIFF-FAIL: {detail} (need <= {args.max_mean_diff})")
        return 1
    print(f"SCREENCAP-DIFF-OK: {detail}")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shot")
    parser.add_argument("--baseline")
    parser.add_argument("--json")
    parser.add_argument(
        "--diff",
        nargs=2,
        metavar=("BEFORE", "AFTER"),
        help="compare two screenshots instead of inspecting one",
    )
    parser.add_argument("--min-mean-diff", type=float, default=None)
    parser.add_argument("--max-mean-diff", type=float, default=None)
    parser.add_argument(
        "--crop-top",
        type=int,
        default=0,
        metavar="PIXELS",
        help="drop the top PIXELS rows (the status-bar band) from both frames "
        "before --diff compares them (M19A2)",
    )
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--reference-dir",
        default="tests/ui-compact/shots",
        help="directory holding the deterministic compact renders",
    )
    args = parser.parse_args()

    if args.self_test:
        return self_test(args.reference_dir)
    if args.diff:
        return diff_main(args)
    if not args.shot:
        parser.error("--shot is required (or --diff / --self-test)")

    results, metrics = inspect(args.shot, args.baseline)
    for label, ok, detail in results:
        print(f"{'ok  ' if ok else 'FAIL'} {label}: {detail}")
    if args.json:
        with open(args.json, "w", encoding="utf-8") as handle:
            json.dump({"metrics": metrics, "checks": [
                {"name": label, "ok": ok, "detail": detail} for label, ok, detail in results
            ]}, handle, indent=2)
    failed = [label for label, ok, _ in results if not ok]
    if failed:
        print(f"SCREENCAP-FAIL: {len(failed)} check(s) failed: {', '.join(failed)}")
        return 1
    print("SCREENCAP-OK: the compact Library surface is painted")
    return 0


if __name__ == "__main__":
    sys.exit(main())
