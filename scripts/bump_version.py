#!/usr/bin/env python3
"""Bump the Suramak version everywhere it is mirrored.

Updates: Cargo.toml ([workspace.package] version), packaging/Info.plist
(CFBundleShortVersionString + CFBundleVersion), and rotates CHANGELOG.md
([Unreleased] -> [X.Y.Z] - date).

Usage: python3 scripts/bump_version.py X.Y.Z[-pre]
"""
import datetime
import pathlib
import plistlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


def bump_cargo(version: str) -> None:
    path = ROOT / "Cargo.toml"
    lines = path.read_text().splitlines(keepends=True)
    in_section = False
    done = False
    for i, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("["):
            in_section = stripped == "[workspace.package]"
        elif in_section and stripped.startswith("version") and not done:
            lines[i] = re.sub(r'"[^"]+"', f'"{version}"', line)
            done = True
    if not done:
        sys.exit("Cargo.toml: no version line found in [workspace.package]")
    path.write_text("".join(lines))


def bump_plist(version: str) -> None:
    path = ROOT / "packaging" / "Info.plist"
    with open(path, "rb") as fh:
        data = plistlib.load(fh)
    data["CFBundleShortVersionString"] = version
    data["CFBundleVersion"] = version  # make-app.sh overrides with the git build number
    with open(path, "wb") as fh:
        plistlib.dump(data, fh, sort_keys=False)


def rotate_changelog(version: str) -> None:
    path = ROOT / "CHANGELOG.md"
    text = path.read_text()
    today = datetime.date.today().isoformat()
    if "## [Unreleased]" not in text:
        sys.exit("CHANGELOG.md: no [Unreleased] section to rotate")
    text = text.replace("## [Unreleased]",
                        f"## [Unreleased]\n\n## [{version}] - {today}", 1)
    path.write_text(text)


def main() -> None:
    if len(sys.argv) != 2 or not re.fullmatch(r"\d+\.\d+\.\d+(-[\w.]+)?", sys.argv[1]):
        sys.exit(__doc__)
    version = sys.argv[1]
    bump_cargo(version)
    bump_plist(version)
    rotate_changelog(version)
    print(f"bumped to {version}: Cargo.toml, packaging/Info.plist, CHANGELOG.md")


if __name__ == "__main__":
    main()
